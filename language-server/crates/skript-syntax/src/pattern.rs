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
//! | `:x` `tag:` `1:` `1¦` | parse marks: they select a variant, match no text   |
//! | `\|` `\[` `\]`  | escaped literal                                           |
//!
//! Parse marks are the fiddly part: `[:local]` means "optional literal `local`,
//! and remember that it matched". They must not become part of the literal text.

use std::fmt;

use crate::matcher::sub_words;

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
            depth: 0,
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
                Node::Literal(text) => out.extend(sub_words(text)),
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

    /// Whether any part of the pattern is a `<regex>` slot.
    ///
    /// A regex is unbounded — Skript's catch-all event structure is literally
    /// `[on] … <.+> …` — and the matcher records no span for what it consumed.
    /// Anything scoring a *match* rather than a pattern therefore has to refuse
    /// to credit these, or a pattern that explains nothing looks like one that
    /// explains the entire line.
    pub fn has_regex(&self) -> bool {
        flatten(&self.nodes)
            .into_iter()
            .any(|node| matches!(node, Node::Regex(_)))
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

/// How many spelled-out forms a glued run may expand to before we give up on it.
///
/// The cross product is exponential in the number of glued groups. Real patterns
/// stay far below this — the widest in core Skript is
/// `(is|are)[(n't| not)]` at six — but an addon is free to write something silly
/// and must not be able to blow up parsing.
const MAX_FUSED_FORMS: usize = 64;

/// Joins nodes that are written with no space between them into whole words.
///
/// Skript's metasyntax is character-level, but matching a line is only tractable
/// token-level, and 38% of published patterns straddle that divide:
/// `cancel[l]ed`, `block[s]`, `[right|left]click`, `ha(s|ve)`, `toggl(e|ing)`.
/// Left alone, `Node::Literal("cancel")` is compared against the whole token
/// `cancelled` and fails — and worse, the inverted index keys the pattern on a
/// word that can never appear, so it is not even offered to the matcher.
///
/// Expanding a glued run to its literal alternatives fixes both at once, and
/// costs nothing at match time. `cancel[l]ed` becomes
/// `Choice(["cancelled", "canceled"])`.
///
/// `glued[i]` says node `i` is written flush against node `i - 1`.
fn fuse_glued(nodes: Vec<Node>, glued: &[bool]) -> Vec<Node> {
    if nodes.len() < 2 || glued.len() != nodes.len() {
        return nodes;
    }

    let mut out: Vec<Node> = Vec::new();
    let mut run: Vec<Node> = Vec::new();

    for (index, node) in nodes.into_iter().enumerate() {
        // A slot or regex has no spelled-out form, so it can never be part of a
        // run; it closes the one before it and starts nothing.
        let fusable = matches!(node, Node::Literal(_) | Node::Optional(_) | Node::Choice(_));

        if !glued[index] || !fusable {
            out.extend(flush_run(&mut run));
        }
        if fusable {
            run.push(node);
        } else {
            out.push(node);
        }
    }
    out.extend(flush_run(&mut run));
    out
}

/// Collapses a pending glued run into one node, or returns it untouched.
fn flush_run(run: &mut Vec<Node>) -> Vec<Node> {
    let taken = std::mem::take(run);
    if taken.len() < 2 {
        return taken;
    }
    match spell_out(&taken) {
        Some(forms) => vec![forms],
        // Too wide to enumerate: leave the run as it was. Matching stays
        // token-based and this pattern keeps its old behaviour rather than
        // gaining a wrong one.
        None => taken,
    }
}

/// Every whole-word string a glued run can produce, as a single node.
fn spell_out(run: &[Node]) -> Option<Node> {
    let mut forms = vec![String::new()];

    for node in run {
        let endings = word_forms(node)?;
        if forms.len().checked_mul(endings.len())? > MAX_FUSED_FORMS {
            return None;
        }
        // Concatenated raw, and only tidied once the whole run is assembled.
        // Collapsing at each step trimmed the trailing space off an
        // intermediate form, so the ` ` branch of `off[ |-]hand` produced
        // `off` + `hand` = `offhand` and the spaced spelling was lost.
        forms = forms
            .iter()
            .flat_map(|prefix| {
                endings
                    .iter()
                    .map(move |ending| format!("{prefix}{ending}"))
            })
            .collect();
    }

    // An all-optional run can vanish entirely; that empty form is the `Optional`
    // wrapper, not a branch that matches an empty token.
    let optional = forms.iter().any(|form| form.trim().is_empty());
    // `collapse_in_group`, not `collapse`, because this node may itself end up
    // inside a wider glued run. Trimming here loses the separator: the run
    // `mouse` + `[( |-)]` spells `"mouse "`, `"mouse-"` and `"mouse"`, and
    // plain `collapse` turned the first into the third — so by the time
    // `click` was appended, only `mouseclick` and `mouse-click` survived and
    // `mouse click` could not be written at all. Same for `right click`.
    let mut spelled: Vec<String> = forms
        .iter()
        .map(|form| collapse_in_group(form))
        .filter(|form| !form.is_empty())
        .collect();
    spelled.sort();
    spelled.dedup();

    let node = match spelled.len() {
        0 => return None,
        1 => Node::Literal(spelled.pop().expect("length checked")),
        _ => Node::Choice(
            spelled
                .into_iter()
                .map(|form| vec![Node::Literal(form)])
                .collect(),
        ),
    };

    Some(if optional {
        Node::Optional(vec![node])
    } else {
        node
    })
}

/// The text alternatives a node can stand for, `""` meaning "absent".
fn word_forms(node: &Node) -> Option<Vec<String>> {
    match node {
        Node::Literal(text) => Some(vec![text.clone()]),
        Node::Optional(inner) => {
            let mut forms = sequence_forms(inner)?;
            forms.push(String::new());
            Some(forms)
        }
        Node::Choice(branches) => {
            let mut forms = Vec::new();
            for branch in branches {
                forms.extend(sequence_forms(branch)?);
            }
            Some(forms)
        }
        Node::Slot(_) | Node::Regex(_) => None,
    }
}

/// The cross product of a sequence's word forms.
fn sequence_forms(nodes: &[Node]) -> Option<Vec<String>> {
    let mut forms = vec![String::new()];
    for node in nodes {
        let endings = word_forms(node)?;
        if forms.len().checked_mul(endings.len())? > MAX_FUSED_FORMS {
            return None;
        }
        forms = forms
            .iter()
            .flat_map(|prefix| {
                endings
                    .iter()
                    .map(move |ending| format!("{prefix}{ending}"))
            })
            .collect();
    }
    Some(forms)
}

fn literal_words(nodes: &[Node]) -> Vec<&str> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Node::Literal(text) => Some(sub_words(text)),
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
    /// How many groups enclose the current position.
    depth: usize,
}

/// Far deeper than any real pattern (core Skript's deepest is 5). Patterns
/// also come from the user's own skript-reflect definitions, and every walk
/// over a pattern recurses, so `effect [[[[…` two thousand deep overflowed
/// the stack and took the whole server down.
const MAX_NESTING: usize = 32;

impl<'a> Parser<'a> {
    /// A group opened at `offset`, up to its `close`.
    fn nested(&mut self, offset: usize, close: char) -> Result<Vec<Node>, ParseError> {
        if self.depth >= MAX_NESTING {
            return Err(ParseError {
                message: format!("groups nested deeper than {MAX_NESTING}"),
                offset,
            });
        }
        self.depth += 1;
        let inner = self.parse_sequence(Some(close));
        self.depth -= 1;
        inner
    }

    /// Parses until `terminator` (or end of input when `None`), splitting on
    /// any top-level `|` into a choice.
    // `flush_literal!` updates `gap` on every path, including the final flush
    // before returning, where nothing reads it again. Tracking that per
    // expansion would make the macro harder to follow than the warning is worth.
    #[allow(unused_assignments)]
    fn parse_sequence(&mut self, terminator: Option<char>) -> Result<Vec<Node>, ParseError> {
        let mut branches: Vec<Vec<Node>> = Vec::new();
        let mut current: Vec<Node> = Vec::new();
        let mut literal = String::new();

        // Whether whitespace separates the last node pushed from the next one.
        // 38% of Skript's patterns glue a group straight onto a word —
        // `cancel[l]ed`, `[right|left]click`, `ha(s|ve)`, `block[s]` — and the
        // collapsed literal text alone cannot tell those apart from `[the] event`.
        // `fuse_glued` needs this to know what to join. Starts true: the first
        // node has nothing before it.
        let mut gap = true;
        let mut glued: Vec<bool> = Vec::new();

        macro_rules! flush_literal {
            () => {
                // Inside a group, edge whitespace is kept — see
                // `collapse_in_group`. At the top level it is noise, and
                // trimming keeps node text readable.
                let trimmed = if terminator.is_some() {
                    collapse_in_group(&literal)
                } else {
                    collapse(&literal)
                };
                if !trimmed.is_empty() {
                    glued.push(!gap && !literal.starts_with(char::is_whitespace));
                    current.push(Node::Literal(trimmed));
                    gap = literal.ends_with(char::is_whitespace);
                } else if !literal.is_empty() {
                    gap = true;
                }
                literal.clear();
            };
        }

        macro_rules! push_group {
            ($node:expr) => {
                glued.push(!gap);
                current.push($node);
                gap = false;
            };
        }

        // A parse mark's colon may only appear at the very start of a group or
        // immediately after a `|`. Testing "the pending literal is blank"
        // instead also matched the position just after `%…%`, `)` or `]` —
        // every one of which flushes the literal — so `%number%:%number%` lost
        // its colon and silently matched the wrong lines.
        let mut at_alternative_start = true;

        while let Some(&(offset, ch)) = self.chars.peek() {
            if at_alternative_start {
                if let Some(length) = parse_tag_length(&self.source[offset..]) {
                    // A parse tag selects a variant and matches no text.
                    while self
                        .chars
                        .next_if(|&(at, _)| at < offset + length)
                        .is_some()
                    {}
                    at_alternative_start = false;
                    continue;
                }
            }
            let consumed_something = !matches!(ch, ' ' | '\t');
            match ch {
                _ if Some(ch) == terminator => {
                    self.chars.next();
                    flush_literal!();
                    branches.push(fuse_glued(std::mem::take(&mut current), &glued));
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
                    branches.push(fuse_glued(std::mem::take(&mut current), &glued));
                    glued.clear();
                    gap = true;
                }
                '[' => {
                    self.chars.next();
                    flush_literal!();
                    let inner = self.nested(offset, ']')?;
                    // `[:tag]` and `[1¦x]` carry a parse mark; the mark itself
                    // matches no text and has already been stripped below.
                    push_group!(Node::Optional(inner));
                }
                '(' => {
                    self.chars.next();
                    flush_literal!();
                    let inner = self.nested(offset, ')')?;
                    push_group!(match inner.as_slice() {
                        [Node::Choice(_)] => inner.into_iter().next().unwrap(),
                        _ => Node::Choice(vec![inner]),
                    });
                }
                '%' => {
                    self.chars.next();
                    flush_literal!();
                    push_group!(Node::Slot(self.parse_slot(offset)?));
                }
                // `<` opens a regex slot — but it is also Skript's less-than
                // operator, and `CondCompare` really does register
                // `(… |<) %objects%`. Treat it as a regex only when a closing
                // `>` follows before the current group ends.
                '<' if self.regex_closes(offset) => {
                    self.chars.next();
                    flush_literal!();
                    push_group!(Node::Regex(self.take_regex(offset)?));
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

            // Only whitespace and the opening of a group keep us at the start
            // of an alternative.
            if consumed_something && !matches!(ch, '|') {
                at_alternative_start = false;
            } else if ch == '|' {
                at_alternative_start = true;
            }
        }

        if let Some(expected) = terminator {
            return Err(ParseError {
                message: format!("unclosed `{}`", opening_for(expected)),
                offset: self.source.len(),
            });
        }

        flush_literal!();
        branches.push(fuse_glued(current, &glued));
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
        regex_length(&self.source[offset + 1..]).is_some()
    }

    /// Consumes a `<regex>`'s body and its closing `>`; the `<` is already
    /// consumed. See [`regex_length`] for how the end is found.
    fn take_regex(&mut self, opened_at: usize) -> Result<String, ParseError> {
        let Some(length) = regex_length(&self.source[opened_at + 1..]) else {
            return Err(ParseError {
                message: "unclosed `<`".into(),
                offset: opened_at,
            });
        };
        let body = self.source[opened_at + 1..opened_at + 1 + length].to_string();
        // The body, then the `>`.
        for _ in 0..body.chars().count() + 1 {
            self.chars.next();
        }
        Ok(body)
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

/// The byte length of a regex body that starts right after a `<`, when that
/// `<` opens one; `None` when it is a literal less-than.
///
/// A regex runs to the `>` that closes it, read as a regex: a `[…]` is a
/// character class there, not an optional group, and `\>` or a `>` inside a
/// class does not close it. Treating `[` as Skript's optional bracket turned
/// ClassySK's `<[A-Z]\w*>` into the literal text `<a-zw*>`, so every addon
/// pattern with a bracketed regex — ClassySK, oopsk, packet field names — could
/// never match.
///
/// A less-than is told apart by what follows it before any closing `>`: Skript
/// writes comparison choices as `(<|>)` and `[<=]`, so a `|`, `)` or `]` at the
/// top level, or a `%`, means the `<` was a literal.
fn regex_length(rest: &str) -> Option<usize> {
    let mut depth = 0u32;
    let mut in_class = false;
    let mut chars = rest.char_indices();
    while let Some((at, ch)) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            ']' if in_class => in_class = false,
            _ if in_class => {}
            '[' => in_class = true,
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            '>' if depth == 0 => return (at > 0).then_some(at),
            '|' | ')' | ']' | '%' if depth == 0 => return None,
            '%' => return None,
            _ => {}
        }
    }
    None
}

/// The byte length of a parse tag at the start of `rest`, colon included.
///
/// Skript 2.6 replaced `1¦` with `1:` and added named tags, so an alternative
/// may open with `:`, `1:`, `-1:` or `name:`: `(1:last|2:first)`,
/// `(adult:[an] adult|baby:[a] baby)`, `(show|not:hide)`. Only the bare `:`
/// form used to be stripped, so the others became literal text (`1:last`,
/// `adult:`) that no line contains, and 26 addon patterns could never match.
///
/// `::` is not a tag. It is literal text in skript-reflect style patterns.
fn parse_tag_length(rest: &str) -> Option<usize> {
    let name = rest.strip_prefix('-').unwrap_or(rest);
    let name_length = name
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(name.len());
    let after = &name[name_length..];
    if !after.starts_with(':') || after[1..].starts_with(':') {
        return None;
    }
    // A leading `-` is only part of a numeric tag; `-:` is not a tag.
    if name.len() != rest.len() && name_length == 0 {
        return None;
    }
    Some(rest.len() - after.len() + 1)
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
/// Like [`collapse`], but keeps a single space at each end.
///
/// Used for literals *inside* a group, because there the edge space carries
/// meaning that fusion needs. `(is|are)(n't| not)` has branches `n't` and
/// ` not`; trimming the second turns `is` + ` not` into `isnot` and the
/// pattern stops matching `{_x} is not set`. `off[ |-]hand` is worse — the
/// ` ` branch collapses to nothing at all, so `off hand` becomes unreachable.
///
/// Preserving the space is safe everywhere else: literal matching, index keys
/// and specificity all go through `split_whitespace`, which ignores it.
fn collapse_in_group(text: &str) -> String {
    let inner = collapse(text);
    if inner.is_empty() {
        // A whitespace-only branch, as in `[ |-]`. It matches no token, but it
        // is the difference between `off hand` and `offhand`.
        return if text.is_empty() {
            String::new()
        } else {
            " ".to_string()
        };
    }
    let lead = text.starts_with(char::is_whitespace);
    let trail = text.ends_with(char::is_whitespace);
    format!(
        "{}{inner}{}",
        if lead { " " } else { "" },
        if trail { " " } else { "" }
    )
}

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
        // `[s]` is glued to `holder`, so it is spelled out rather than left as a
        // separate node — matching is token-based and `holder` never appears as
        // a token of `leash holders`. See `fuse_glued`.
        let pattern = Pattern::parse("[the] leash holder[s]").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![
                Node::Optional(vec![lit("the")]),
                Node::Choice(vec![vec![lit("leash holder")], vec![lit("leash holders")]]),
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

    /// `[A-Z]` inside a regex is a character class, not an optional group.
    /// Misreading it made ClassySK, oopsk and packet-field patterns unmatchable.
    #[test]
    fn a_regex_with_a_character_class_stays_a_regex() {
        let last = |source: &str| Pattern::parse(source).unwrap().nodes.last().cloned();
        assert_eq!(
            last(r"class <[A-Z]\w*>"),
            Some(Node::Regex(r"[A-Z]\w*".into()))
        );
        assert_eq!(last(r"[a] <([\w ]+)> struct"), Some(lit("struct")));
        assert_eq!(last(r"x <[^>]+>"), Some(Node::Regex("[^>]+".into())));
    }

    /// Skript writes comparison choices with a bare `<`; those stay literal.
    #[test]
    fn a_less_than_in_a_choice_is_not_a_regex() {
        for source in [
            "%number% (<|>) %number%",
            "%number% [<=] %number%",
            "a < %number%",
        ] {
            let pattern = Pattern::parse(source).unwrap();
            assert!(
                !pattern.has_regex(),
                "{source} parsed a regex: {:?}",
                pattern.nodes
            );
        }
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
        // `holder` is deliberately absent: the line may say `holders`, so it is
        // not a word every match contains. Indexing on it made the pattern
        // unreachable for the plural — the index key has to be a word that
        // genuinely always appears, and `leash` is one.
        let pattern = Pattern::parse("[the] leash holder[s] of %entities%").unwrap();
        assert_eq!(pattern.required_literals(), vec!["leash", "of"]);
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

    /// Run on a small stack, as the server's main thread is: deep nesting is
    /// refused rather than recursed into until the stack overflows.
    #[test]
    fn refuses_pathologically_deep_nesting() {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let deep = format!("{}x{}", "[".repeat(5000), "]".repeat(5000));
                assert!(Pattern::parse(&deep).is_err());
                let deep = format!("{}x{}", "(".repeat(5000), ")".repeat(5000));
                assert!(Pattern::parse(&deep).is_err());
                let fine = format!("{}x{}", "[".repeat(MAX_NESTING), "]".repeat(MAX_NESTING));
                assert!(Pattern::parse(&fine).is_ok());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn specificity_prefers_concrete_patterns() {
        let vague = Pattern::parse("%objects%").unwrap();
        let concrete = Pattern::parse("give %item types% to %players%").unwrap();
        assert!(concrete.specificity() > vague.specificity());
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;

    #[test]
    fn a_colon_after_a_slot_is_literal_text() {
        // Regression: the colon was read as a parse mark whenever the pending
        // literal was blank — which includes the position right after `%…%`,
        // `)` or `]`, because those all flush the literal. The colon vanished,
        // so the pattern matched lines it should not.
        let pattern = Pattern::parse("%number%:%number%").unwrap();
        let literals: Vec<&str> = pattern
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Literal(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            literals,
            vec![":"],
            "the colon was swallowed: {:?}",
            pattern.nodes
        );
    }

    #[test]
    fn a_colon_after_a_choice_is_literal_text() {
        let pattern = Pattern::parse("(a|b):c").unwrap();
        let literals: Vec<&str> = pattern
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Literal(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // Fused: `:c` is glued to the choice, so the forms are spelled out
        // whole. The point of the test is that the colon survives as text
        // rather than being eaten as a parse mark, and it does — inside both
        // branches now, which is also what the tokenizer will see, since `:` is
        // not a token boundary.
        assert!(literals.is_empty(), "got {:?}", pattern.nodes);
        assert_eq!(
            pattern.nodes,
            vec![Node::Choice(vec![
                vec![Node::Literal("a:c".into())],
                vec![Node::Literal("b:c".into())],
            ])],
        );
    }

    #[test]
    fn a_real_parse_mark_is_still_stripped() {
        let pattern = Pattern::parse("[:local] function").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![
                Node::Optional(vec![Node::Literal("local".into())]),
                Node::Literal("function".into())
            ]
        );
    }

    #[test]
    fn a_parse_mark_after_an_alternative_is_still_stripped() {
        let pattern = Pattern::parse("(:first|:second)").unwrap();
        assert_eq!(
            pattern.nodes,
            vec![Node::Choice(vec![
                vec![Node::Literal("first".into())],
                vec![Node::Literal("second".into())]
            ])]
        );
    }
}

#[cfg(test)]
mod parse_tag_regressions {
    use super::*;

    fn lit(text: &str) -> Node {
        Node::Literal(text.to_string())
    }

    /// Skript 2.6's numbered and named tags. Only the bare `:` used to be
    /// stripped, so these parsed as the literal text `1:last` and `adult:`.
    #[test]
    fn numbered_and_named_tags_are_stripped() {
        assert_eq!(
            Pattern::parse("(1:last|2:first) message").unwrap().nodes,
            vec![
                Node::Choice(vec![vec![lit("last")], vec![lit("first")]]),
                lit("message")
            ]
        );
        assert_eq!(
            Pattern::parse("[-1:temporary|1:permanent] bounds")
                .unwrap()
                .nodes,
            vec![
                Node::Optional(vec![Node::Choice(vec![
                    vec![lit("temporary")],
                    vec![lit("permanent")]
                ])]),
                lit("bounds")
            ]
        );
        assert_eq!(
            Pattern::parse("(show|not:hide) name").unwrap().nodes,
            vec![
                Node::Choice(vec![vec![lit("show")], vec![lit("hide")]]),
                lit("name")
            ]
        );
        // A tag in front of a group, as in `(adult:[an] adult|baby:[a] baby)`.
        let pattern = Pattern::parse("is (adult:[an] adult|baby:[a] baby)").unwrap();
        assert_eq!(pattern.required_literals(), vec!["is"]);
        assert!(!pattern.source().is_empty());
    }

    #[test]
    fn a_tag_selects_but_matches_no_text() {
        let pattern = Pattern::parse("(1:last|2:first) message").unwrap();
        assert!(crate::matcher::match_pattern(&pattern, "last message").is_some());
        assert!(crate::matcher::match_pattern(&pattern, "1:last message").is_none());
    }

    /// `::` is literal text, and a colon later in a word is never a tag.
    #[test]
    fn colons_that_are_not_tags_stay_literal() {
        assert_eq!(parse_tag_length("::x"), None);
        assert_eq!(parse_tag_length("a::b"), None);
        assert_eq!(parse_tag_length("-:x"), None);
        assert_eq!(parse_tag_length("foo bar:x"), None);
        assert_eq!(parse_tag_length(":x"), Some(1));
        assert_eq!(parse_tag_length("-1:x"), Some(3));
        assert_eq!(parse_tag_length("not:x"), Some(4));
        assert_eq!(
            Pattern::parse("x %number%:%number%")
                .unwrap()
                .required_literals(),
            vec!["x", ":"]
        );
    }

    /// Empty branches, `(s|)` and `(|first|last)`, appear in 95 addon patterns.
    #[test]
    fn an_empty_branch_is_an_absent_alternative() {
        let pattern = Pattern::parse("[the] (|first|last) index of %string%").unwrap();
        for line in ["the index of x", "first index of x", "the last index of x"] {
            assert!(
                crate::matcher::match_pattern(&pattern, line).is_some(),
                "{line:?}"
            );
        }
        let glued = Pattern::parse("[the] region(s|) %direction%").unwrap();
        assert!(crate::matcher::match_pattern(&glued, "regions north").is_some());
        assert!(crate::matcher::match_pattern(&glued, "the region north").is_some());
    }

    #[test]
    fn nested_optionals_with_alternatives_match_every_spelling() {
        let pattern = Pattern::parse("[all [[of] the]|the|every] items").unwrap();
        for line in [
            "items",
            "all items",
            "all the items",
            "all of the items",
            "every items",
        ] {
            assert!(
                crate::matcher::match_pattern(&pattern, line).is_some(),
                "{line:?}"
            );
        }
        assert!(crate::matcher::match_pattern(&pattern, "of the items").is_none());
    }
}
