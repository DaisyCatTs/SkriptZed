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

/// What a `%…%` slot captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotCapture {
    pub slot: Slot,
    /// The captured text, exactly as it appeared in the line.
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Match {
    pub captures: Vec<SlotCapture>,
}

/// Splits a line into tokens, keeping strings, variables and interpolations
/// whole.
pub(crate) fn tokenize(line: &str) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        let ch = bytes[index];

        if ch.is_ascii_whitespace() {
            index += 1;
            continue;
        }

        let start = index;
        match ch {
            b'"' => index = skip_delimited(bytes, index, b'"', b'"'),
            b'{' => index = skip_delimited(bytes, index, b'{', b'}'),
            b'%' => index = skip_delimited(bytes, index, b'%', b'%'),
            _ => {
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'"' | b'{' | b'%')
                {
                    index += 1;
                }
            }
        }

        // Defensive: an unterminated delimiter must still make progress.
        if index == start {
            index += 1;
        }

        tokens.push(Token {
            text: line[start..index].to_ascii_lowercase(),
            start,
            end: index,
        });
    }

    tokens
}

/// Consumes a delimited run, tolerating nesting for `{}` and the doubled-quote
/// escape Skript uses inside strings.
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
            Node::Literal(text) => text.split_whitespace().count(),
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
                for word in text.split_whitespace() {
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
        let Some(most) = available.checked_sub(reserved).filter(|take| *take > 0) else {
            return None;
        };

        for take in 1..=most {
            self.steps += 1;
            if self.steps > STEP_BUDGET {
                return None;
            }

            let end = pos + take;
            let mark = captures.len();

            if let Some(slot) = slot {
                let start_byte = self.tokens[pos].start;
                let end_byte = self.tokens[end - 1].end;
                captures.push(SlotCapture {
                    slot: slot.clone(),
                    text: self.line[start_byte..end_byte].to_string(),
                    start: start_byte,
                    end: end_byte,
                });
            }

            if let Some(finish) = self.match_nodes(rest, cont, end, captures) {
                return Some(finish);
            }
            captures.truncate(mark);
        }
        None
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
