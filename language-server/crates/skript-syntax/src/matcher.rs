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

    let end = state.match_nodes(&pattern.nodes, 0, &mut captures)?;
    // A pattern describes the whole line, so leftover tokens mean no match.
    (end == tokens.len()).then_some(Match { captures })
}

struct State<'a> {
    tokens: &'a [Token],
    line: &'a str,
    steps: u32,
}

impl State<'_> {
    /// Matches `nodes` starting at token `pos`, returning the position after
    /// the last consumed token.
    fn match_nodes(
        &mut self,
        nodes: &[Node],
        pos: usize,
        captures: &mut Vec<SlotCapture>,
    ) -> Option<usize> {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            return None;
        }

        let Some((node, rest)) = nodes.split_first() else {
            return Some(pos);
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
                self.match_nodes(rest, cursor, captures)
            }

            Node::Optional(inner) => {
                // Present first: a longer match is the more informative one, and
                // trying it first means the common case does not backtrack.
                let mark = captures.len();
                if let Some(after) = self.match_nodes(inner, pos, captures) {
                    if let Some(end) = self.match_nodes(rest, after, captures) {
                        return Some(end);
                    }
                }
                captures.truncate(mark);
                self.match_nodes(rest, pos, captures)
            }

            Node::Choice(branches) => {
                for branch in branches {
                    let mark = captures.len();
                    if let Some(after) = self.match_nodes(branch, pos, captures) {
                        if let Some(end) = self.match_nodes(rest, after, captures) {
                            return Some(end);
                        }
                    }
                    captures.truncate(mark);
                }
                None
            }

            Node::Slot(slot) => self.match_variable_width(rest, pos, captures, Some(slot)),

            Node::Regex(_) => self.match_variable_width(rest, pos, captures, None),
        }
    }

    /// Slots and regex holes both consume one or more tokens; the following
    /// nodes decide how many. Shortest first, so a trailing literal anchors the
    /// slot at its first occurrence rather than its last.
    fn match_variable_width(
        &mut self,
        rest: &[Node],
        pos: usize,
        captures: &mut Vec<SlotCapture>,
        slot: Option<&Slot>,
    ) -> Option<usize> {
        for take in 1..=self.tokens.len().saturating_sub(pos) {
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

            if let Some(finish) = self.match_nodes(rest, end, captures) {
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
