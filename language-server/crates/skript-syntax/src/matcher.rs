//! Matches a line of Skript against a parsed [`Pattern`].
//!
//! The algorithm is ordinary backtracking over a token stream, with two
//! Skript-specific wrinkles:
//!
//! 1. **Slots are variable-width.** `%players%` may consume one token or ten,
//!    and the only thing that pins it down is the literal that follows. Slots
//!    are therefore matched lazily — shortest first — so `give %items% to
//!    %players%` binds `%items%` at the first `to` rather than the last.
//!
//! 2. **A token is not a word.** `"hello world"`, `{my::var}` and `%player's
//!    tool%` each have to stay whole, or the literal `to` inside a message
//!    would be mistaken for pattern syntax.
//!
//! Backtracking over several unbounded slots is exponential in the worst case,
//! so the matcher runs on a step budget. Exhausting it reports "no match",
//! which degrades to an unclassified line rather than a hung editor.

use crate::pattern::{Node, Pattern, Slot};

/// Upper bound on matcher steps for a single pattern/line pair.
///
/// Real Skript patterns have at most a handful of slots, so this is generous;
/// it exists to make a pathological line impossible to weaponise.
const STEP_BUDGET: u32 = 20_000;

/// One token of a line, with its span in the original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    /// Lowercased, so comparisons against pattern literals are plain equality.
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// What a `%…%` slot, or a `<regex>` hole, captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotCapture {
    pub slot: Slot,
    /// The captured text, exactly as it appeared in the line.
    pub text: String,
    pub start: usize,
    pub end: usize,
    /// A `<regex>` hole rather than a slot; `slot` is then empty.
    ///
    /// Recorded so a caller can tell what a regex swallowed apart from the
    /// pattern's own words. Without it, ClassySK's `[a] new [instance of]
    /// <[A-Z]\w*>` painted the class name as literal text, so colouring had
    /// to throw away every match that used a regex. Ranking ignores these:
    /// see `literal_coverage`.
    pub regex: bool,
}

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Match {
    pub captures: Vec<SlotCapture>,
}

impl Match {
    /// Non-whitespace bytes of `line` that the pattern spelled out itself,
    /// rather than leaving to a slot or a regex hole.
    pub fn literal_coverage(&self, line: &str) -> usize {
        let non_space = |text: &str| {
            text.bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .count()
        };
        let captured: usize = self
            .captures
            .iter()
            .filter_map(|capture| line.get(capture.start..capture.end))
            .map(non_space)
            .sum();
        non_space(line).saturating_sub(captured)
    }

    /// Whether the pattern accounted for any of `line` with its own words.
    ///
    /// A match that explains nothing is a catch-all: Skript's implicit
    /// conditional is registered as `implicit:<.+>`, and it matches every
    /// line there is.
    pub fn explains_text(&self, line: &str) -> bool {
        self.literal_coverage(line) > 0
    }
}

/// Splits a line into tokens, keeping strings, variables and interpolations
/// whole.
pub(crate) fn tokenize(line: &str) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut push = |start: usize, end: usize| {
        tokens.push(Token {
            text: line[start..end].to_lowercase(),
            start,
            end,
        });
    };

    while index < bytes.len() {
        let ch = bytes[index];

        if ch.is_ascii_whitespace() {
            index += 1;
            continue;
        }

        let start = index;
        match ch {
            b'"' => index = skip_string(bytes, index),
            b'{' => index = skip_delimited(bytes, index, b'{', b'}'),
            b'%' => index = skip_delimited(bytes, index, b'%', b'%'),
            _ => {
                // Only a `%` that starts a token opens an interpolation; one
                // glued to a word is a percent sign. Breaking the word there
                // made `assert 1_23_4% is 1234/100` open an "interpolation"
                // that ran into the string and swallowed the `is`.
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'"' | b'{')
                {
                    index += 1;
                }
                for (from, to) in sub_word_ranges(&line[start..index]) {
                    push(start + from, start + to);
                }
                continue;
            }
        }

        // Defensive: an unterminated delimiter must still make progress.
        if index == start {
            index += 1;
        }
        push(start, index);
    }

    tokens
}

/// The words of a pattern literal, cut exactly as [`tokenize`] cuts a line.
///
/// Both sides have to agree on where a word ends, or a literal can never equal
/// a token: see [`sub_word_ranges`].
pub(crate) fn sub_words(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace().flat_map(|word| {
        sub_word_ranges(word)
            .into_iter()
            .map(move |(from, to)| &word[from..to])
    })
}

/// Where one whitespace-free word splits into separately matched pieces.
///
/// Skript writes a lot of punctuation flush against a slot: `%entities%'[s]
/// custom name` (332 core patterns and 1,934 addon ones), `by x %number%, y
/// %number%`, `chance of %number%[\%]`. The line then has `victim's`, `45,` or
/// `10%` as one word, a slot cannot take half a token, and none of those
/// patterns could match unless the thing before the punctuation happened to be
/// a `{variable}` (whose closing brace already ends a token). So a comma always
/// stands alone, and a trailing `'s`, `'` or `%` is cut off its word.
fn sub_word_ranges(word: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (at, ch) in word.char_indices() {
        if ch == ',' {
            push_piece(word, start, at, &mut out);
            out.push((at, at + 1));
            start = at + 1;
        }
    }
    push_piece(word, start, word.len(), &mut out);
    out
}

fn push_piece(word: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    if start >= end {
        return;
    }
    let piece = &word[start..end];
    let suffix = if piece.len() > 2 && (piece.ends_with("'s") || piece.ends_with("'S")) {
        2
    } else if piece.len() > 1 && (piece.ends_with('\'') || piece.ends_with('%')) {
        1
    } else {
        0
    };
    if suffix == 0 {
        out.push((start, end));
    } else {
        out.push((start, end - suffix));
        out.push((end - suffix, end));
    }
}

/// Consumes a quoted string, including any `%…%` interpolation inside it.
///
/// An interpolation is an expression and may hold quotes of its own:
/// Skript's tests write `"%formatted "<green><bold>Hello world!"%"`. Ending
/// the string at the first inner quote split it into fragments that pattern
/// literals could then line up with.
fn skip_string(bytes: &[u8], open: usize) -> usize {
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' if bytes.get(index + 1) == Some(&b'"') => index += 2,
            b'"' => return index + 1,
            b'%' if bytes.get(index + 1) == Some(&b'%') => index += 2,
            // An unclosed `%` is Skript's error to report; read it as text.
            b'%' => index = interpolation_end(bytes, index).unwrap_or(index + 1),
            _ => index += 1,
        }
    }
    index
}

/// The index just past the `%` closing the interpolation opened at `open`.
fn interpolation_end(bytes: &[u8], open: usize) -> Option<usize> {
    let (mut depth, mut in_string) = (0usize, false);
    for (index, &byte) in bytes.iter().enumerate().skip(open + 1) {
        match byte {
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => depth = depth.saturating_sub(1),
            b'%' if !in_string && depth == 0 => return Some(index + 1),
            _ => {}
        }
    }
    None
}

/// Consumes a delimited run, tolerating nesting for `{}` and the doubled
/// escape (`%%`) for an interpolation.
fn skip_delimited(bytes: &[u8], mut index: usize, open: u8, close: u8) -> usize {
    let nests = open != close;
    let mut depth = 0usize;
    index += 1;
    depth += 1;

    while index < bytes.len() {
        let ch = bytes[index];
        if !nests && ch == close {
            // `""` and `%%` are escapes, not terminators.
            if bytes.get(index + 1) == Some(&close) {
                index += 2;
                continue;
            }
            return index + 1;
        }
        if nests {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
        }
        index += 1;
    }

    index
}

/// Matches `line` against `pattern`, returning the slot captures on success.
pub(crate) fn match_pattern(pattern: &Pattern, line: &str) -> Option<Match> {
    let tokens = tokenize(line);
    let mut state = State {
        tokens: &tokens,
        line,
        steps: 0,
    };
    let mut captures = Vec::new();

    state.match_nodes(&pattern.nodes, None, 0, &mut captures)?;
    Some(Match { captures })
}

/// What is still to be matched after the current node list runs out.
///
/// A pattern must describe the *whole* line, and that requirement has to be part
/// of the search rather than a test applied to its first answer. Checking
/// `end == tokens.len()` after `match_nodes` returned meant a pattern ending in a
/// slot succeeded on the first token it could take and then failed the length
/// test with no chance to backtrack — so `wait %timespan%` matched `wait 1 tick`
/// never, and `loop %objects%` matched `loop {_x::*}` but not `loop all players`.
///
/// Entering a group has to remember the nodes that follow it, so the tail is a
/// stack of node lists, threaded as a linked list through the call frames.
struct Cont<'p, 'c> {
    nodes: &'p [Node],
    next: Option<&'c Cont<'p, 'c>>,
}

struct State<'a> {
    tokens: &'a [Token],
    line: &'a str,
    steps: u32,
}

/// The fewest tokens `nodes` followed by `cont` can possibly consume.
///
/// Used only to prune, so it must never over-estimate: an optional contributes
/// nothing, a choice contributes its cheapest branch, and a slot contributes the
/// one token it is required to take.
fn min_tokens(nodes: &[Node], cont: Option<&Cont<'_, '_>>) -> usize {
    let here: usize = nodes
        .iter()
        .map(|node| match node {
            Node::Literal(text) => sub_words(text).count(),
            Node::Optional(_) => 0,
            Node::Choice(branches) => branches
                .iter()
                .map(|branch| min_tokens(branch, None))
                .min()
                .unwrap_or(0),
            Node::Slot(_) | Node::Regex(_) => 1,
        })
        .sum();

    here + cont.map_or(0, |tail| min_tokens(tail.nodes, tail.next))
}

impl State<'_> {
    /// Matches `nodes`, then `cont`, starting at token `pos`. Returns the
    /// position after the last consumed token, which on success is always the
    /// end of the line.
    fn match_nodes<'p>(
        &mut self,
        nodes: &'p [Node],
        cont: Option<&Cont<'p, '_>>,
        pos: usize,
        captures: &mut Vec<SlotCapture>,
    ) -> Option<usize> {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            return None;
        }

        let Some((node, rest)) = nodes.split_first() else {
            // This node list is spent. Resume the enclosing one, or — if there
            // is none — demand that the line is spent too.
            return match cont {
                Some(tail) => self.match_nodes(tail.nodes, tail.next, pos, captures),
                None => (pos == self.tokens.len()).then_some(pos),
            };
        };

        match node {
            Node::Literal(text) => {
                let mut cursor = pos;
                for word in sub_words(text) {
                    if self.tokens.get(cursor).map(|t| t.text.as_str()) != Some(word) {
                        return None;
                    }
                    cursor += 1;
                }
                self.match_nodes(rest, cont, cursor, captures)
            }

            Node::Optional(inner) => {
                // Present first: a longer match is the more informative one, and
                // trying it first means the common case does not backtrack.
                let mark = captures.len();
                let tail = Cont {
                    nodes: rest,
                    next: cont,
                };
                if let Some(end) = self.match_nodes(inner, Some(&tail), pos, captures) {
                    return Some(end);
                }
                captures.truncate(mark);
                self.match_nodes(rest, cont, pos, captures)
            }

            Node::Choice(branches) => {
                let tail = Cont {
                    nodes: rest,
                    next: cont,
                };
                for branch in branches {
                    let mark = captures.len();
                    if let Some(end) = self.match_nodes(branch, Some(&tail), pos, captures) {
                        return Some(end);
                    }
                    captures.truncate(mark);
                }
                None
            }

            Node::Slot(slot) => self.match_variable_width(rest, cont, pos, captures, Some(slot)),

            Node::Regex(_) => self.match_variable_width(rest, cont, pos, captures, None),
        }
    }

    /// Slots and regex holes both consume one or more tokens; the following
    /// nodes decide how many. Shortest first, so a trailing literal anchors the
    /// slot at its first occurrence rather than its last.
    fn match_variable_width<'p>(
        &mut self,
        rest: &'p [Node],
        cont: Option<&Cont<'p, '_>>,
        pos: usize,
        captures: &mut Vec<SlotCapture>,
        slot: Option<&Slot>,
    ) -> Option<usize> {
        // A slot cannot eat tokens that the rest of the pattern still needs, and
        // since the match must now reach the end of the line, "the rest" is
        // exactly known. Without this bound every trailing slot walks 1..N and
        // fails N-1 times before taking the whole tail — the single hottest path
        // there is, because most patterns end in a slot.
        let available = self.tokens.len().saturating_sub(pos);
        let reserved = min_tokens(rest, cont);
        let most = available.checked_sub(reserved).filter(|take| *take > 0)?;

        for take in 1..=most {
            self.steps += 1;
            if self.steps > STEP_BUDGET {
                return None;
            }

            let end = pos + take;
            let mark = captures.len();

            let start_byte = self.tokens[pos].start;
            let end_byte = self.tokens[end - 1].end;
            captures.push(SlotCapture {
                slot: slot.cloned().unwrap_or_default(),
                text: self.line[start_byte..end_byte].to_string(),
                start: start_byte,
                end: end_byte,
                regex: slot.is_none(),
            });

            if let Some(finish) = self.match_nodes(rest, cont, end, captures) {
                return Some(finish);
            }
            captures.truncate(mark);
        }
        None
    }
}

/// A slot that the end of a line can begin.
///
/// What argument completion needs: `teleport player to ` is not a match of
/// `teleport %entities% to %location%`, because the line stops before the
/// pattern does, but it says exactly which slot is being typed next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSlot {
    pub slot: Slot,
    /// How many of the line's tokens the pattern spelled out with its own
    /// words on the way there. A pattern that explains more of the line is
    /// the likelier one when several leave a slot open.
    pub literals: usize,
}

/// Every slot `pattern` can have open at the end of `line`, where the line's
/// tokens are all consumed and the next node to match is that slot.
///
/// Unlike [`match_pattern`] this explores every path rather than stopping at
/// the first: a choice or an optional can leave different slots open
/// (`send %objects% [to %audiences%]` after `send x to ` versus `send `).
pub(crate) fn open_slots(pattern: &Pattern, line: &str) -> Vec<OpenSlot> {
    let tokens = tokenize(line);
    let mut walk = PrefixWalk {
        tokens: &tokens,
        steps: 0,
        found: Vec::new(),
    };
    walk.walk(&pattern.nodes, None, 0, 0);
    let mut found = walk.found;
    found.dedup();
    found
}

struct PrefixWalk<'a> {
    tokens: &'a [Token],
    steps: u32,
    found: Vec<OpenSlot>,
}

impl PrefixWalk<'_> {
    fn walk<'p>(
        &mut self,
        nodes: &'p [Node],
        cont: Option<&Cont<'p, '_>>,
        pos: usize,
        literals: usize,
    ) {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            return;
        }
        let Some((node, rest)) = nodes.split_first() else {
            // A pattern that ends before the line does leaves nothing open.
            if let Some(tail) = cont {
                self.walk(tail.nodes, tail.next, pos, literals);
            }
            return;
        };
        let end = self.tokens.len();
        match node {
            Node::Literal(text) => {
                let mut cursor = pos;
                for word in sub_words(text) {
                    if self.tokens.get(cursor).map(|t| t.text.as_str()) != Some(word) {
                        return;
                    }
                    cursor += 1;
                }
                self.walk(rest, cont, cursor, literals + (cursor - pos));
            }
            Node::Optional(inner) => {
                let tail = Cont {
                    nodes: rest,
                    next: cont,
                };
                self.walk(inner, Some(&tail), pos, literals);
                self.walk(rest, cont, pos, literals);
            }
            Node::Choice(branches) => {
                let tail = Cont {
                    nodes: rest,
                    next: cont,
                };
                for branch in branches {
                    self.walk(branch, Some(&tail), pos, literals);
                }
            }
            Node::Slot(slot) if pos == end => self.found.push(OpenSlot {
                slot: slot.clone(),
                literals,
            }),
            // A slot or regex hole before the end takes one token or more,
            // but never runs past the end: the slot being typed starts there.
            Node::Slot(_) | Node::Regex(_) => {
                for next in pos + 1..=end {
                    self.steps += 1;
                    if self.steps > STEP_BUDGET {
                        return;
                    }
                    self.walk(rest, cont, next, literals);
                }
            }
        }
    }
}

#[cfg(test)]
mod open_slot_tests {
    use super::*;
    use crate::pattern::Pattern;

    fn open(pattern: &str, line: &str) -> Vec<Vec<String>> {
        open_slots(&Pattern::parse(pattern).unwrap(), line)
            .into_iter()
            .map(|open| open.slot.types)
            .collect()
    }

    #[test]
    fn the_slot_after_the_typed_words_is_open() {
        assert_eq!(open("kill %entities%", "kill "), vec![vec!["entities"]]);
        assert_eq!(
            open("teleport %entities% to %location%", "teleport player to "),
            vec![vec!["location"]]
        );
        // The first slot takes `player`; it never stays open past it.
        assert_eq!(
            open("teleport %entities% to %location%", "teleport player "),
            Vec::<Vec<String>>::new()
        );
    }

    #[test]
    fn an_optional_tail_can_open_its_slot() {
        let message = "(message|send [message[s]]) %objects% [to %audiences%]";
        assert_eq!(open(message, r#"send "hi" to "#), vec![vec!["audiences"]]);
        assert_eq!(open(message, "send "), vec![vec!["objects"]]);
    }

    #[test]
    fn a_line_the_pattern_cannot_start_opens_nothing() {
        assert!(open("kill %entities%", "teleport ").is_empty());
        assert!(open("cancel [the] event", "cancel ").is_empty());
        // Finished: the pattern has nothing left to open.
        assert!(open("kill %entities%", "kill player ").is_empty());
    }

    #[test]
    fn literals_count_the_words_spelled_out() {
        let found = open_slots(
            &Pattern::parse("teleport %entities% to %location%").unwrap(),
            "teleport player to ",
        );
        assert_eq!(found[0].literals, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::Pattern;

    fn matched(pattern: &str, line: &str) -> Option<Match> {
        match_pattern(&Pattern::parse(pattern).unwrap(), line)
    }

    fn captures(pattern: &str, line: &str) -> Vec<String> {
        matched(pattern, line)
            .expect("expected a match")
            .captures
            .into_iter()
            .map(|capture| capture.text)
            .collect()
    }

    #[test]
    fn matches_a_literal() {
        assert!(matched("cancel event", "cancel event").is_some());
        assert!(matched("cancel event", "cancel the event").is_none());
    }

    #[test]
    fn is_case_insensitive() {
        assert!(matched("cancel event", "Cancel Event").is_some());
    }

    #[test]
    fn honours_optionals() {
        assert!(matched("cancel [the] event", "cancel event").is_some());
        assert!(matched("cancel [the] event", "cancel the event").is_some());
    }

    #[test]
    fn honours_choices() {
        assert!(matched("(spawn|summon) %number%", "spawn 3").is_some());
        assert!(matched("(spawn|summon) %number%", "summon 3").is_some());
        assert!(matched("(spawn|summon) %number%", "conjure 3").is_none());
    }

    #[test]
    fn captures_slots() {
        assert_eq!(
            captures("give %item types% to %players%", "give 1 diamond to player"),
            vec!["1 diamond", "player"]
        );
    }

    #[test]
    fn binds_a_slot_lazily_so_a_later_literal_anchors_it() {
        // The message contains the word `to`; the slot must not swallow it and
        // then fail, nor stop at it.
        assert_eq!(
            captures(
                "send %texts% to %players%",
                r#"send "go to spawn" to player"#
            ),
            vec![r#""go to spawn""#, "player"]
        );
    }

    #[test]
    fn keeps_strings_variables_and_interpolations_whole() {
        let tokens = tokenize(r#"set {my::var} to "a b" and %player's tool%"#);
        let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "set",
                "{my::var}",
                "to",
                "\"a b\"",
                "and",
                "%player's tool%"
            ]
        );
    }

    #[test]
    fn handles_doubled_quote_escapes() {
        let tokens = tokenize(r#"send "he said ""hi""" to player"#);
        let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["send", r#""he said ""hi""""#, "to", "player"]);
    }

    #[test]
    fn requires_the_whole_line_to_be_consumed() {
        assert!(matched("cancel event", "cancel event now").is_none());
    }

    #[test]
    fn capture_spans_point_back_into_the_line() {
        let line = "give 1 diamond to player";
        let matched = matched("give %item types% to %players%", line).unwrap();
        let first = &matched.captures[0];
        assert_eq!(&line[first.start..first.end], "1 diamond");
    }

    #[test]
    fn gives_up_rather_than_hanging_on_a_pathological_line() {
        // Six unbounded slots and no anchoring literals: the search space is
        // huge, and the step budget must cut it off.
        let pattern = "%objects% %objects% %objects% %objects% %objects% %objects%";
        let line = "a b c d e f g h i j k l m n o p q r s t u v w x y z";
        let _ = matched(pattern, line); // must simply return, quickly
    }
}

#[cfg(test)]
mod whole_line_regressions {
    use super::*;
    use crate::pattern::Pattern;

    fn matches(pattern: &str, line: &str) -> bool {
        match_pattern(&Pattern::parse(pattern).unwrap(), line).is_some()
    }

    #[test]
    fn a_trailing_slot_may_consume_more_than_one_token() {
        // The whole-line requirement used to be tested *after* the search
        // returned, so a pattern ending in a slot took the first token it could
        // and then failed the length check with no chance to backtrack. Every
        // one of these is ordinary Skript that silently classified as nothing.
        assert!(matches("wait [for] %timespan%", "wait 3 seconds"));
        assert!(matches("loop %objects%", "loop all players"));
        assert!(matches(
            "set %~objects% to %objects%",
            "set {_x} to player's location"
        ));
        assert!(matches("%objects%", "one two three four"));
    }

    #[test]
    fn a_pattern_still_has_to_cover_the_whole_line() {
        // The other half: backtracking must not let a pattern match a prefix.
        assert!(!matches("stop", "stop the music"));
        assert!(!matches("cancel [the] event", "cancel the event now"));
        assert!(!matches("%objects% has passed", "{_d} has passed already"));

        // Note what is deliberately *not* asserted here: a pattern ending in a
        // slot really does swallow a trailing tail, because slots are untyped —
        // `%timespan%` will take any four tokens. Slot types are documentation,
        // not validation, and enforcing them would mean rejecting every line
        // built from an expression this server cannot evaluate. The whole-line
        // rule only constrains what a *literal* has to line up with.
        assert!(matches("wait [for] %timespan%", "wait 3 seconds then go"));
    }

    #[test]
    fn captures_are_the_text_the_slot_actually_took() {
        let matched = match_pattern(
            &Pattern::parse("give %~objects% %objects%").unwrap(),
            "give the player 3 gold ingots",
        )
        .expect("should match");
        let texts: Vec<&str> = matched.captures.iter().map(|c| c.text.as_str()).collect();
        // Shortest-first means the leading slot yields as little as it can.
        assert_eq!(texts, vec!["the", "player 3 gold ingots"]);
    }
}

#[cfg(test)]
mod glued_group_regressions {
    use super::*;
    use crate::pattern::Pattern;

    fn matches(pattern: &str, line: &str) -> bool {
        match_pattern(&Pattern::parse(pattern).unwrap(), line).is_some()
    }

    #[test]
    fn a_group_glued_to_a_word_matches_the_whole_word() {
        // 38% of Skript's published patterns write a group flush against a word.
        // Matching is token-based, so these only work because `fuse_glued`
        // spells the run out at parse time.
        assert!(matches(
            "[the] event is cancel[l]ed",
            "the event is cancelled"
        ));
        assert!(matches("[the] event is cancel[l]ed", "event is canceled"));
        assert!(matches("%objects% ha(s|ve) passed", "{_d} has passed"));
        assert!(matches("%objects% ha(s|ve) passed", "{_d} have passed"));
        assert!(matches("[right|left]click[ing]", "rightclick"));
        assert!(matches("[right|left]click[ing]", "leftclicking"));
        assert!(matches("[right|left]click[ing]", "click"));
        assert!(matches("%objects% block[s]", "{_x} blocks"));
    }

    #[test]
    fn a_branch_that_begins_with_a_space_keeps_it() {
        // The negation forms Skript writes everywhere. Both spellings of each
        // must work: the contraction is glued to the word before it, while the
        // spelled-out form carries a leading space *inside* the group.
        //
        // Collapsing that space away fused `is` + `not` into `isnot`, so
        // `{_x} is not set` — an everyday condition — matched nothing at all.
        for (pattern, line) in [
            (
                "%~objects% (do[es](n't| not) exist|(is|are)(n't| not) set)",
                "{_x} is not set",
            ),
            (
                "%~objects% (do[es](n't| not) exist|(is|are)(n't| not) set)",
                "{_x} isn't set",
            ),
            (
                "%~objects% (do[es](n't| not) exist|(is|are)(n't| not) set)",
                "{_x} does not exist",
            ),
            ("%dates% ha(s|ve)[(n't| not)] passed", "{_d} has not passed"),
            ("%dates% ha(s|ve)[(n't| not)] passed", "{_d} has passed"),
            (
                "%players% can('t| not) see %entities%",
                "{_p} can not see {_e}",
            ),
            (
                "%players% can('t| not) see %entities%",
                "{_p} can't see {_e}",
            ),
            (
                "%scripts/worlds% (is|are)[(n't| not)] loaded",
                "{_w} is not loaded",
            ),
            (
                "plugin[s] %texts% (is|are)(n't| not) enabled",
                "plugin \"x\" is not enabled",
            ),
        ] {
            assert!(matches(pattern, line), "{line:?} did not match {pattern}");
        }
    }

    #[test]
    fn a_whitespace_only_branch_is_a_real_alternative() {
        // `[ |-]` offers "a space, a hyphen, or nothing". The space branch used
        // to collapse to the empty string and vanish, leaving `off hand`
        // unmatchable while `offhand` and `off-hand` worked.
        for (pattern, line) in [
            (
                "%living entities%'[s] off[ |-]hand[s] (is|are) raised",
                "{_e}'s off hand is raised",
            ),
            (
                "%living entities%'[s] off[ |-]hand[s] (is|are) raised",
                "{_e}'s off-hand is raised",
            ),
            (
                "%players/texts% (is|are) IP[(-| )]banned",
                "{_p} is IP banned",
            ),
            (
                "%players/texts% (is|are) IP[(-| )]banned",
                "{_p} is IP-banned",
            ),
        ] {
            assert!(matches(pattern, line), "{line:?} did not match {pattern}");
        }
    }

    #[test]
    fn a_glued_group_does_not_match_across_a_space() {
        // `block[s]` is one word; it must not also accept `block s`.
        assert!(!matches("%objects% block[s]", "{_x} block s"));
    }

    #[test]
    fn spacing_still_separates_ordinary_groups() {
        // `[the] event` is written with a space, so it must stay two nodes.
        assert!(matches("[the] event", "the event"));
        assert!(matches("[the] event", "event"));
        assert!(!matches("[the] event", "theevent"));
    }
}

#[cfg(test)]
mod punctuation_regressions {
    use super::*;
    use crate::pattern::Pattern;

    fn matches(pattern: &str, line: &str) -> bool {
        match_pattern(&Pattern::parse(pattern).unwrap(), line).is_some()
    }

    fn texts(line: &str) -> Vec<String> {
        tokenize(line).into_iter().map(|token| token.text).collect()
    }

    /// 332 core patterns and 1,934 addon ones put `'s` flush against a slot.
    /// They matched only when the owner was a `{variable}`, whose brace
    /// already ends a token; `victim's` was one word a slot could not split.
    #[test]
    fn a_possessive_after_a_slot_matches_any_owner() {
        for line in [
            "victim's main hand is raised",
            "the player's main hand is raised",
            "{_e}'s main hand is raised",
            "players' main hands are raised",
        ] {
            assert!(
                matches("%living entities%'[s] [main] hand[s] (is|are) raised", line),
                "{line:?}"
            );
        }
        assert!(matches(
            "(cancel|interrupt) [the] us[ag]e of %living entities%'[s] [active|current] item",
            "interrupt the usage of the player's active item"
        ));
        assert!(matches("%strings%'s file contents", "{_f}'s file contents"));
        assert!(matches(
            "%strings%'s file contents",
            "arg-1's file contents"
        ));
    }

    #[test]
    fn a_possessive_capture_stops_before_the_apostrophe() {
        let line = "the player's main hand is raised";
        let found = match_pattern(
            &Pattern::parse("%living entities%'[s] [main] hand[s] (is|are) raised").unwrap(),
            line,
        )
        .unwrap();
        assert_eq!(found.captures[0].text, "the player");
    }

    /// `by x %number%, y %number%`: the line writes `45,` as one word.
    #[test]
    fn a_comma_after_a_slot_is_its_own_token() {
        assert!(matches(
            "rotate %quaternions/displays% by x %number%, y %number%(, [and]| and) z %number%",
            "rotate {_quaternion} by x 45, y 90, z 135"
        ));
        assert!(matches(
            "rotate %quaternions/displays% by x %number%, y %number%(, [and]| and) z %number%",
            "rotate {_q} by x 45, y 90 and z 135"
        ));
        assert!(matches(
            "[a] [new] vector [(from|at|to)] %number%,[ ]%number%(,[ ]| and )%number%",
            "vector 1,2,3"
        ));
        assert_eq!(texts("a, b,c"), vec!["a", ",", "b", ",", "c"]);
    }

    /// A percent sign glued to a number is not an interpolation. Reading it as
    /// one ran on into the string and swallowed `is`.
    #[test]
    fn a_percent_glued_to_a_word_is_a_percent_sign() {
        assert_eq!(
            texts(r#"assert 1_23_4% is 1234/100 with "x%% y""#),
            vec![
                "assert",
                "1_23_4",
                "%",
                "is",
                "1234/100",
                "with",
                r#""x%% y""#
            ]
        );
        assert!(matches(r"chance of %number%[(\%)]", "chance of 10%"));
        assert!(matches(r"chance of %number%[(\%)]", "chance of 0.5"));
        // A `%` that opens a token is still an interpolation.
        assert_eq!(
            texts("give %player's tool% x"),
            vec!["give", "%player's tool%", "x"]
        );
    }

    /// Quotes inside an interpolation do not end the string: Skript's own
    /// tests write `"%formatted "<green><bold>Hello world!"%"`.
    #[test]
    fn a_string_keeps_quotes_inside_its_interpolations() {
        assert_eq!(
            texts(r#"send "%formatted "<green>Hello to you"%" to player"#),
            vec![
                "send",
                r#""%formatted "<green>hello to you"%""#,
                "to",
                "player"
            ]
        );
        assert_eq!(
            texts(r#"send "%{_list::%player%}% to" to player"#),
            vec!["send", r#""%{_list::%player%}% to""#, "to", "player"]
        );
        // An unclosed `%` is read as text, so the string still ends.
        assert_eq!(
            texts(r#"send "50% off" to player"#),
            vec!["send", r#""50% off""#, "to", "player"]
        );
    }

    #[test]
    fn matching_is_case_insensitive_beyond_ascii() {
        assert!(matches("élever %objects%", "ÉLEVER x"));
    }
}
