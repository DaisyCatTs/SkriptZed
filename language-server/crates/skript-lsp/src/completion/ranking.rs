//! Scoring completion items against what the user has typed.
//!
//! The server decides relevance rather than leaving the client to order
//! hundreds of items by name, where the useful one sits wherever the alphabet
//! put it.

use super::snippet::read_branches;

/// Score for a word that is exactly the query.
const EXACT_WORD: i32 = 1000;
/// Score for a word that starts with the query.
const PREFIX_WORD: i32 = 800;
/// Score for a word that merely contains the query.
const CONTAINED_WORD: i32 = 300;
/// Score for a text whose letters contain the query's in order (`tlp` in
/// `teleport`), starting at a word. Below every real word match: it exists so
/// an abbreviation still reaches the item, not to outrank anything.
const SUBSEQUENCE: i32 = 50;
/// Taken off a word match per word it sits after: the leading word is the one
/// being typed.
const POSITION_PENALTY: i32 = 40;
/// The most [`POSITION_PENALTY`] can take off in total.
const MAX_POSITION_PENALTY: i32 = 200;

/// Lexicographically, `sortText` sorts ascending; scores are subtracted from
/// this so a higher score sorts first.
const SORT_KEY_CEILING: i32 = 9999;

/// A score that sorts before every computed one.
pub const TOP_SCORE: i32 = SORT_KEY_CEILING;

/// How well `text` answers what the user has typed.
///
/// Higher is better; zero means no match at all. The shape matters more than
/// the exact numbers: a word *starting* with the query beats one merely
/// containing it, and the first word (the keyword you type) beats a later one.
/// An abbreviation whose letters appear in order still scores, just lowest.
pub fn relevance(text: &str, typed: &str) -> i32 {
    let query = typed
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if query.is_empty() {
        return 1;
    }

    let text = text.to_ascii_lowercase();
    let mut best = 0;

    for (index, word) in text.split_whitespace().enumerate() {
        let score = if word == query {
            EXACT_WORD
        } else if word.starts_with(&query) {
            PREFIX_WORD
        } else if word.contains(&query) {
            CONTAINED_WORD
        } else {
            continue;
        };
        best = best.max(score - (index as i32 * POSITION_PENALTY).min(MAX_POSITION_PENALTY));
    }

    if best == 0 && is_word_subsequence(&text, &query) {
        return SUBSEQUENCE;
    }
    best
}

/// Whether `query`'s characters appear in `text` in order, the first of them
/// at the start of a word.
fn is_word_subsequence(text: &str, query: &str) -> bool {
    let mut chars = query.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let rest: Vec<char> = chars.collect();
    let starts_word = |at: usize| at == 0 || text[..at].ends_with(char::is_whitespace);
    text.char_indices()
        .filter(|&(at, ch)| ch == first && starts_word(at))
        .any(|(at, ch)| {
            let mut wanted = rest.iter().peekable();
            for next in text[at + ch.len_utf8()..].chars() {
                if wanted.peek() == Some(&&next) {
                    wanted.next();
                }
            }
            wanted.peek().is_none()
        })
}

/// Relevance against the words a pattern can *begin* with.
///
/// `(message|send [message[s]]) %objects%` opens with either `message` or
/// `send`; both are things a user types first, and both deserve a leading-word
/// score rather than whatever position they happen to occupy once the pattern
/// is flattened.
pub fn relevance_to_keywords(pattern: &str, typed: &str) -> i32 {
    leading_keywords(pattern)
        .into_iter()
        .map(|keyword| relevance(&keyword, typed))
        .max()
        .unwrap_or(0)
}

/// The words a pattern can start with.
///
/// One word for a plain pattern, or every alternative when it opens with a
/// choice. Optionals are skipped: `[the] egg will hatch` starts with `egg` as
/// far as anyone typing is concerned.
fn leading_keywords(pattern: &str) -> Vec<String> {
    let mut chars = pattern.chars().peekable();
    let mut depth = 0i32;

    while let Some(&ch) = chars.peek() {
        match ch {
            '[' => depth += 1,
            ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            ' ' => {}
            '(' => {
                chars.next();
                // Each branch is itself pattern text, so recurse for its own
                // opening word.
                return read_branches(&mut chars)
                    .iter()
                    .flat_map(|branch| leading_keywords(branch))
                    .collect();
            }
            // A slot or regex hole means the pattern opens with no keyword.
            '%' | '<' => return Vec::new(),
            _ => {
                let word: String = chars
                    .by_ref()
                    .take_while(|ch| ch.is_alphanumeric() || *ch == '-')
                    .collect();
                return if word.is_empty() {
                    Vec::new()
                } else {
                    vec![word]
                };
            }
        }
        chars.next();
    }
    Vec::new()
}

/// A `sortText` that preserves the server's ordering.
///
/// Clients sort lexicographically, so the score is inverted into a fixed-width
/// number. The name breaks ties so equally relevant items keep a stable,
/// predictable order rather than jumping about between keystrokes.
pub fn sort_key(score: i32, name: &str) -> String {
    format!("{:05}{name}", (SORT_KEY_CEILING - score).max(0))
}

/// Whether a syntax item can stand for the whole phrase typed so far.
///
/// A catalog item replaces everything from the start of the statement, but
/// [`relevance`] scores only the last word. So finishing `kill player` and
/// pressing Enter accepted "Entity/Player/World from UUID", whose name
/// contains "player", and the line became `player from uuids`. An item must
/// account for every typed word, each as the start of one of its
/// words. Once an argument has been written (a string, a variable, a number)
/// the phrase is past its keywords, and no syntax snippet should replace it.
pub fn covers_phrase(pattern: &str, filter: &str, typed: &str) -> bool {
    // A quote can only be the end of a string just closed (`"Welcome %x%"|`):
    // accepting anything there replaced the closing quote.
    if typed.contains('"') {
        return false;
    }
    let typed_words: Vec<String> = typed.split_whitespace().map(str::to_lowercase).collect();
    if typed_words.len() <= 1 {
        return true;
    }
    if typed.contains(['{', '(', '%']) || typed.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    // The phrase starts where the syntax starts: `send m` is Message, which
    // opens with `send`, not Make Say, which merely has both words later on.
    let first = &typed_words[0];
    if !leading_keywords(pattern)
        .iter()
        .any(|keyword| keyword.to_lowercase().starts_with(first.as_str()))
    {
        return false;
    }
    // The rest in any order: the filter text is deduplicated, so a word the
    // pattern repeats (`message[s]`) appears only once in it.
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    typed_words[1..].iter().all(|typed_word| {
        words
            .iter()
            .any(|word| word.starts_with(typed_word.as_str()))
    })
}

/// What the client should match the user's typing against.
///
/// The entry name plus the pattern's literal words, so either finds the item:
/// somebody who knows it as "Message" and somebody who just types `send` both
/// get there. Slots and tab stops are stripped; nobody types `%objects%`.
pub fn filter_text_for(name: &str, pattern: &str) -> String {
    // Every alternative, not just the first. The snippet picks one branch of
    // `(message|send)` because it has to insert *something*; filtering wants
    // the opposite: `send` must be matchable even though the snippet inserts
    // `message`.
    let mut text = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();

    while let Some(ch) = chars.next() {
        match ch {
            // Slots and regex holes are not typed by anyone.
            '%' => {
                chars.by_ref().find(|&next| next == '%');
                text.push(' ');
            }
            '<' => {
                chars.by_ref().find(|&next| next == '>');
                text.push(' ');
            }
            // Group and alternation syntax is structure, not text.
            '(' | ')' | '[' | ']' | '|' => text.push(' '),
            _ => text.push(ch),
        }
    }

    let mut words: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        if !words.contains(&word) {
            words.push(word);
        }
    }

    if words.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", words.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_starting_with_the_query_beats_one_containing_it() {
        let starts = relevance("send message", "sen");
        let contains = relevance("resend", "sen");
        assert!(starts > contains, "{starts} vs {contains}");
    }

    #[test]
    fn an_exact_word_beats_a_prefix() {
        assert!(relevance("send objects", "send") > relevance("sender name", "send"));
    }

    #[test]
    fn the_leading_word_beats_a_later_one() {
        // Both contain `player`, but the one that starts with it is what the
        // user is typing.
        let leading = relevance("player is op", "player");
        let later = relevance("teleport the player somewhere", "player");
        assert!(leading > later, "{leading} vs {later}");
    }

    #[test]
    fn no_match_scores_zero_so_it_can_be_dropped() {
        assert_eq!(relevance("teleport entities", "zzz"), 0);
    }

    #[test]
    fn an_empty_query_keeps_everything() {
        assert!(relevance("anything at all", "") > 0);
    }

    /// `tlp` still reaches Teleport, but below anything with a real word
    /// match, so it never crowds out what was typed in full.
    #[test]
    fn an_abbreviation_matches_as_a_subsequence_ranked_last() {
        let abbreviated = relevance("Teleport teleport entities", "tlp");
        assert!(abbreviated > 0, "tlp did not reach Teleport");
        assert!(abbreviated < relevance("resend", "sen"));
        // The first letter must begin a word, and order matters.
        assert_eq!(relevance("teleport", "lpt"), 0);
        assert_eq!(relevance("teleport", "ept"), 0);
    }

    /// Clients sort `sortText` lexicographically, so a better score must produce
    /// a string that sorts earlier.
    #[test]
    fn the_sort_key_preserves_the_ordering() {
        let better = sort_key(1000, "Message");
        let worse = sort_key(300, "Anything");
        assert!(
            better < worse,
            "a higher score sorted later: {better:?} vs {worse:?}"
        );
        // Fixed width, or 1000 would sort beside 100.
        assert_eq!(better.len() - "Message".len(), 5);
    }

    #[test]
    fn a_choice_yields_every_opening_word() {
        let mut words = leading_keywords("(message|send [message[s]]) %objects%");
        words.sort();
        assert_eq!(words, vec!["message", "send"]);
    }

    #[test]
    fn an_optional_prefix_is_skipped() {
        // Nobody types `[the]`; the keyword is `egg`.
        assert_eq!(
            leading_keywords("[the] egg (will|won't) hatch"),
            vec!["egg"]
        );
    }

    #[test]
    fn a_pattern_opening_with_a_slot_has_no_keyword() {
        assert!(leading_keywords("%objects% (is|are) set").is_empty());
        assert!(leading_keywords("<.+>").is_empty());
    }

    /// Typing `send` must score Message as highly as anything literally named
    /// "Send …", because `send` is one of the words Message begins with.
    #[test]
    fn a_leading_alternative_scores_as_a_leading_word() {
        let message = relevance_to_keywords("(message|send [message[s]]) %objects%", "send");
        let named = relevance("Send Block Change make see as", "send");
        assert!(
            message >= named,
            "Message scored {message}, an entry merely named Send scored {named}"
        );
    }

    /// Completion must be reachable by the word a user types.
    ///
    /// The label is Skript's documentation *title*, and Zed filters on the label
    /// alone. Measured against the published database, 60 of 139 effects have
    /// their pattern's leading keyword nowhere in their name, so typing `send`
    /// did not surface "Message", and typing `make` did not surface "Consume
    /// Brewing Fuel". For a language written as English prose that defeats the
    /// purpose of completion.
    #[test]
    fn the_keyword_a_user_types_is_matchable() {
        for (name, pattern, typed) in [
            (
                "Message",
                "(message|send) [message[s]] %objects% [to %commandsenders%]",
                "send",
            ),
            (
                "Consume Brewing Fuel",
                "make %blocks% [not] consume [the] fuel",
                "make",
            ),
            (
                "Apply Fishing Lure",
                "(reel|pull) in [the] hook[ed] entity",
                "reel",
            ),
            (
                "Teleport",
                "teleport %entities% (to|%direction%) %location%",
                "teleport",
            ),
        ] {
            let filter = filter_text_for(name, pattern).to_lowercase();
            assert!(
                filter.contains(typed),
                "typing {typed:?} cannot reach {name:?}; filter text was {filter:?}"
            );
            // The name must still work for anybody who knows it.
            assert!(
                filter.contains(&name.to_lowercase()),
                "{name:?} is no longer findable by its own name"
            );
        }
    }

    #[test]
    fn slots_and_tab_stops_are_not_matchable_text() {
        // Nobody types `%objects%` or `${1:...}`.
        let filter = filter_text_for("Message", "(message|send) %objects% [to %commandsenders%]");
        assert!(!filter.contains('%'), "a slot leaked in: {filter:?}");
        assert!(!filter.contains('$'), "a tab stop leaked in: {filter:?}");
    }

    #[test]
    fn a_pattern_with_no_literals_still_yields_the_name() {
        assert_eq!(filter_text_for("Entities", "%*entity types%"), "Entities");
    }

    /// Finishing a line and pressing Enter accepted the top suggestion, which
    /// replaced the whole statement: `kill player` became `player from uuids`.
    #[test]
    fn a_finished_line_is_not_covered_by_unrelated_syntax() {
        let covers = |name: &str, pattern: &str, typed: &str| {
            covers_phrase(pattern, &filter_text_for(name, pattern), typed)
        };
        let uuid = (
            "Entity/Player/World from UUID",
            "(player|offline player|entity|world) from %uuids%",
        );
        assert!(!covers(uuid.0, uuid.1, "kill player"));
        assert!(!covers("Kill", "kill %entities%", "kill player"));
        assert!(!covers(
            "Teleport",
            "teleport %entities% to %location%",
            "teleport player to spawn"
        ));
        // Arguments mean the keywords are done.
        let message = ("Message", "(message|send) %objects% [to %commandsenders%]");
        assert!(!covers(message.0, message.1, "send \"hi\" to player"));
        assert!(!covers(
            "Change",
            "set %~objects% to %objects%",
            "set {_x} to 5"
        ));
        assert!(!covers("Parse", "%text% parsed as %*classinfo%", "\""));
    }

    #[test]
    fn a_phrase_being_typed_is_still_covered() {
        let covers = |name: &str, pattern: &str, typed: &str| {
            covers_phrase(pattern, &filter_text_for(name, pattern), typed)
        };
        let message = ("Message", "(message|send) %objects% [to %commandsenders%]");
        assert!(covers(message.0, message.1, "send"));
        assert!(covers(message.0, message.1, "send m"));
        assert!(covers("Cancel Event", "cancel [the] event", "cancel ev"));
        // Anchored at the start: Make Say has both words, but not first.
        assert!(!covers(
            "Make Say",
            "make %players% send message %texts%",
            "send m"
        ));
    }
}
