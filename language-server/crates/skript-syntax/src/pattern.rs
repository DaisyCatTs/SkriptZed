//! Parser for Skript's syntax-pattern language.
//!
//! Every effect, condition, expression, event and section in Skript is
//! registered as a *pattern string* rather than as grammar. `docs.json` for core
//! Skript 2.16 alone publishes 2,117 of them. They look like this:
//!
//! ```text
//! give %item types% to %players%
//! [the] leash holder[s] of %entities%
//! (spawn|summon) %number% of %entity types% [%directions% %locations%]
//! [local[ly]] suppress [the] (conflict|deprecated syntax) warning[s]
//! using [[the] experiment] <.+>
//! ```
//!
//! The metasyntax is small but easy to get subtly wrong:
//!
//! | Form            | Meaning                                                  |
//! |-----------------|----------------------------------------------------------|
//! | `[x]`           | optional                                                  |
//! | `(a\|b)`        | choice                                                    |
//! | `a\|b` (bare)   | choice, when it appears directly inside `[…]` or `(…)`    |
//! | `%type%`        | an expression slot of that type                           |
//! | `%~type%`       | slot passed by reference                                  |
//! | `%-type%`       | slot that may be absent                                   |
//! | `%*type%`       | literal-only slot                                         |
//! | `%a/b%`         | slot accepting several types                              |
//! | `<regex>`       | raw regular expression slot                               |
//! | `:tag` / `1¦`   | parse marks — they select a variant, they match no text   |
//! | `\|` `\[` `\]`  | escaped literal                                           |
//!
//! Parse marks are the fiddly part: `[:local]` means "optional literal `local`,
//! and remember that it matched". They must not become part of the literal text.

use std::fmt;

/// One element of a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Literal text. Always stored lowercase and whitespace-collapsed, because
    /// Skript matches case-insensitively.
    Literal(String),
    /// `[…]` — matches its contents or nothing.
    Optional(Vec<Node>),
    /// `(a|b|c)` — matches exactly one branch.
    Choice(Vec<Vec<Node>>),
    /// `%type%`
    Slot(Slot),
    /// `<regex>` — the raw source is kept; we do not compile it, because a slot
    /// that accepts anything is all the matcher needs to know.
    Regex(String),
}

/// A `%…%` expression slot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slot {
    /// The accepted type names, already split on `/`.
    pub types: Vec<String>,
    /// `%~type%` — the slot is passed by reference.
    pub by_reference: bool,
    /// `%-type%` — the slot may resolve to nothing.
    pub nullable: bool,
    /// `%*type%` — only a literal may be used here.
    pub literal_only: bool,
}

impl Slot {
    /// `%objects%` and `%~objects%` accept anything, which matters for ranking:
    /// a pattern whose slots are all `object` is the least specific match.
    pub fn is_object(&self) -> bool {
        self.types
            .iter()
            .all(|ty| ty == "object" || ty == "objects")
    }
}

/// A parsed Skript syntax pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub nodes: Vec<Node>,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// Byte offset into the pattern source.
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

impl Pattern {
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let mut parser = Parser {
            chars: source.char_indices().peekable(),
            source,
        };
        let nodes = parser.parse_sequence(None)?;
        Ok(Self {
            nodes,
            source: source.to_string(),
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Literals that any successful match MUST contain, in order.
    ///
    /// Only literals on the mandatory spine count: anything inside `[…]` can be
    /// skipped, and a literal inside one branch of a `(a|b)` can be avoided by
    /// taking the other branch. This is what the index uses to reject the vast
    /// majority of patterns for a given line without running the matcher.
    pub fn required_literals(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for node in &self.nodes {
            match node {
                Node::Literal(text) => out.extend(text.split_whitespace()),
                // A word present in *every* branch is still required, which
                // rescues patterns like `(is|are) set` whose only mandatory
                // literal lives inside the choice.
                Node::Choice(branches) => {
                    if let Some((first, rest)) = branches.split_first() {
                        let mut common: Vec<&str> = literal_words(first);
                        for branch in rest {
                            let words = literal_words(branch);
                            common.retain(|word| words.contains(word));
                        }
                        out.extend(common);
                    }
                }
                Node::Optional(_) | Node::Slot(_) | Node::Regex(_) => {}
            }
        }
        out
    }

    /// How specific this pattern is, used to rank competing matches.
    ///
    /// More mandatory literal words is more specific; a slot that accepts
    /// `object` tells us nothing and is therefore penalised.
    pub fn specificity(&self) -> i32 {
        let mut score = self.required_literals().len() as i32 * 10;

        // A mandatory choice between fixed words constrains the line almost as
        // much as a literal does. Without this, `(exit|stop) [trigger]` scores
        // zero — it has no individually-required word — and loses to any
        // expression that happens to accept a bare `%objects%`.
        for node in &self.nodes {
            if let Node::Choice(branches) = node {
                if branches.iter().all(|branch| {
                    !branch.is_empty() && branch.iter().all(|node| matches!(node, Node::Literal(_)))
                }) {
                    score += 8;
                }
            }
        }

        for node in flatten(&self.nodes) {
            if let Node::Slot(slot) = node {
                score += if slot.is_object() { -2 } else { 1 };
            }
        }
        score
    }

    /// Every `%…%` slot in the pattern, in source order.
    pub fn slots(&self) -> Vec<&Slot> {
        flatten(&self.nodes)
            .into_iter()
            .filter_map(|node| match node {
                Node::Slot(slot) => Some(slot),
                _ => None,
            })
            .collect()
    }
}

fn literal_words(nodes: &[Node]) -> Vec<&str> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Node::Literal(text) => Some(text.split_whitespace()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn flatten(nodes: &[Node]) -> Vec<&Node> {
    let mut out = Vec::new();
    for node in nodes {
        out.push(node);
        match node {
            Node::Optional(inner) => out.extend(flatten(inner)),
            Node::Choice(branches) => {
                for branch in branches {
                    out.extend(flatten(branch));
                }
            }
            _ => {}
        }
    }
    out
}

// ------------------------------------------------------------------- parser

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    source: &'a str,
}

impl<'a> Parser<'a> {
    /// Parses until `terminator` (or end of input when `None`), splitting on
    /// any top-level `|` into a choice.
    fn parse_sequence(&mut self, terminator: Option<char>) -> Result<Vec<Node>, ParseError> {
        let mut branches: Vec<Vec<Node>> = Vec::new();
        let mut current: Vec<Node> = Vec::new();
        let mut literal = String::new();

        macro_rules! flush_literal {
            () => {
                let trimmed = collapse(&literal);
                if !trimmed.is_empty() {
                    current.push(Node::Literal(trimmed));
                }
                literal.clear();
            };
        }

        while let Some(&(offset, ch)) = self.chars.peek() {
            match ch {
                _ if Some(ch) == terminator => {
                    self.chars.next();
                    flush_literal!();
                    branches.push(std::mem::take(&mut current));
                    return Ok(finish(branches));
                }
                '\\' => {
                    self.chars.next();
                    if let Some((_, escaped)) = self.chars.next() {
                        literal.push(escaped);
                    }
                }
                '|' => {
                    self.chars.next();
                    flush_literal!();
                    branches.push(std::mem::take(&mut current));
                }
                '[' => {
                    self.chars.next();
                    flush_literal!();
                    let inner = self.parse_sequence(Some(']'))?;
                    // `[:tag]` and `[1¦x]` carry a parse mark; the mark itself
                    // matches no text and has already been stripped below.
                    current.push(Node::Optional(inner));
                }
                '(' => {
                    self.chars.next();
                    flush_literal!();
                    let inner = self.parse_sequence(Some(')'))?;
                    current.push(match inner.as_slice() {
                        [Node::Choice(_)] => inner.into_iter().next().unwrap(),
                        _ => Node::Choice(vec![inner]),
                    });
                }
                '%' => {
                    self.chars.next();
                    flush_literal!();
                    current.push(Node::Slot(self.parse_slot(offset)?));
                }
                // `<` opens a regex slot — but it is also Skript's less-than
                // operator, and `CondCompare` really does register
                // `(… |<) %objects%`. Treat it as a regex only when a closing
                // `>` follows before the current group ends.
                '<' if self.regex_closes(offset) => {
                    self.chars.next();
                    flush_literal!();
                    current.push(Node::Regex(self.take_until('>', offset)?));
                }
                ':' if literal_ends_a_parse_mark(&literal) => {
                    // `[:local]` — the colon marks the rest as a tagged
                    // alternative. The tag name is ordinary literal text.
                    self.chars.next();
                }
                '¦' => {
                    // `1¦value` — a numbered parse mark. Drop the digits that
                    // preceded it along with the marker.
                    self.chars.next();
                    while literal.ends_with(|c: char| c.is_ascii_digit()) {
                        literal.pop();
                    }
                }
                _ => {
                    self.chars.next();
                    literal.push(ch);
                }
            }
        }

        if let Some(expected) = terminator {
            return Err(ParseError {
                message: format!("unclosed `{}`", opening_for(expected)),
                offset: self.source.len(),
            });
        }

        flush_literal!();
        branches.push(current);
        Ok(finish(branches))
    }

    fn parse_slot(&mut self, offset: usize) -> Result<Slot, ParseError> {
        let body = self.take_until('%', offset)?;
        let mut slot = Slot::default();
        let mut rest = body.as_str();

        loop {
            match rest.chars().next() {
                Some('~') => slot.by_reference = true,
                Some('-') => slot.nullable = true,
                Some('*') => slot.literal_only = true,
                _ => break,
            }
            rest = &rest[1..];
        }

        slot.types = rest
            .split('/')
            .map(|ty| ty.trim().to_ascii_lowercase())
            .filter(|ty| !ty.is_empty())
            .collect();

        Ok(slot)
    }

    /// Does the `<` at `offset` open a regex slot, or is it a literal
    /// less-than? A regex is closed by `>` before the enclosing group ends, so
    /// `<.+>` is a regex while `<)` and `<=)` are the comparison operators.
    fn regex_closes(&self, offset: usize) -> bool {
        for ch in self.source[offset + 1..].chars() {
            match ch {
                '>' => return true,
                '|' | ')' | ']' | '%' => return false,
                _ => {}
            }
        }
        false
    }

    fn take_until(&mut self, close: char, opened_at: usize) -> Result<String, ParseError> {
        let mut out = String::new();
        for (_, ch) in self.chars.by_ref() {
            if ch == close {
                return Ok(out);
            }
            out.push(ch);
        }
        Err(ParseError {
            message: format!("unclosed `{}`", opening_for(close)),
            offset: opened_at,
        })
    }
}

/// A `:` only introduces a parse mark at the start of a group or after `|`,
/// i.e. when nothing but whitespace precedes it in the current literal run.
fn literal_ends_a_parse_mark(literal: &str) -> bool {
    literal.trim().is_empty()
}

fn finish(mut branches: Vec<Vec<Node>>) -> Vec<Node> {
    if branches.len() == 1 {
        branches.pop().unwrap_or_default()
    } else {
        vec![Node::Choice(branches)]
    }
}

fn opening_for(close: char) -> char {
    match close {
        ']' => '[',
        ')' => '(',
        '>' => '<',
        '%' => '%',
        other => other,
    }
}

/// Lowercases and collapses internal whitespace so that literal comparison is a
/// plain string equality later on.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.extend(ch.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(text: &str) -> Node {
        Node::Literal(text.to_string())
    }

    #[test]
    fn parses_a_plain_literal() {
        let pattern = Pattern::parse("cancel event").unwrap();
        assert_eq!(pattern.nodes, vec![lit("cancel event")]);
    }

    #[test]
    fn lowercases_and_collapses_whitespace() {
        let pattern = Pattern::parse("Cancel   The  Event").unwrap();
        assert_eq!(pattern.nodes, vec![lit("cancel the event")]);
    }

    #[test]
    fn parses_optionals() {
        let pattern = Pattern::parse("[the] leash holder[s]").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![
                Node::Optional(vec![lit("the")]),
                lit("leash holder"),
                Node::Optional(vec![lit("s")]),
            ]
        );
    }

    #[test]
    fn parses_choices() {
        let pattern = Pattern::parse("(spawn|summon)").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![Node::Choice(vec![vec![lit("spawn")], vec![lit("summon")]])]
        );
    }

    #[test]
    fn parses_a_bare_alternation_inside_an_optional() {
        let pattern = Pattern::parse("[a|b]").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![Node::Optional(vec![Node::Choice(vec![
                vec![lit("a")],
                vec![lit("b")]
            ])])]
        );
    }

    #[test]
    fn parses_slots_with_every_modifier() {
        let pattern =
            Pattern::parse("%~objects% %-player% %*number% %living entities/locations%").unwrap();
        let slots = pattern.slots();
        assert_eq!(slots.len(), 4);
        assert!(slots[0].by_reference);
        assert!(slots[0].is_object());
        assert!(slots[1].nullable);
        assert!(slots[2].literal_only);
        assert_eq!(
            slots[3].types,
            vec!["living entities".to_string(), "locations".to_string()]
        );
    }

    #[test]
    fn parses_a_regex_slot() {
        let pattern = Pattern::parse("using [[the] experiment] <.+>").unwrap();
        assert!(matches!(pattern.nodes.last(), Some(Node::Regex(r)) if r == ".+"));
    }

    #[test]
    fn strips_parse_marks() {
        // `[:local]` must yield the literal `local`, not `:local`.
        let pattern = Pattern::parse("[:local] function").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![Node::Optional(vec![lit("local")]), lit("function")]
        );
    }

    #[test]
    fn strips_numbered_parse_marks() {
        let pattern = Pattern::parse("(1¦first|2¦second)").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![Node::Choice(vec![vec![lit("first")], vec![lit("second")]])]
        );
    }

    #[test]
    fn honours_escapes() {
        let pattern = Pattern::parse(r"a \| b").unwrap();
        assert_eq!(pattern.nodes, vec![lit("a | b")]);
    }

    #[test]
    fn required_literals_skip_optionals() {
        let pattern = Pattern::parse("[the] leash holder[s] of %entities%").unwrap();
        assert_eq!(pattern.required_literals(), vec!["leash", "holder", "of"]);
    }

    #[test]
    fn required_literals_keep_words_common_to_every_branch() {
        let pattern = Pattern::parse("(is|are) set").unwrap();
        assert_eq!(pattern.required_literals(), vec!["set"]);

        let pattern = Pattern::parse("(cancel the|cancel this) event").unwrap();
        assert_eq!(pattern.required_literals(), vec!["cancel", "event"]);
    }

    #[test]
    fn reports_unclosed_groups() {
        assert!(Pattern::parse("[the").is_err());
        assert!(Pattern::parse("(a|b").is_err());
        assert!(Pattern::parse("%player").is_err());
    }

    #[test]
    fn specificity_prefers_concrete_patterns() {
        let vague = Pattern::parse("%objects%").unwrap();
        let concrete = Pattern::parse("give %item types% to %players%").unwrap();
        assert!(concrete.specificity() > vague.specificity());
    }
}
