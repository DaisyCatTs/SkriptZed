//! Where a line's code ends and its comment begins.
//!
//! One rule for the whole server. Semantic tokens, colour swatches, the
//! catalog diagnostics and variable completion each used to carry their own
//! comment scanner, and they disagreed: one honoured `%` inside a string, one
//! did not, and the diagnostics never stripped comments at all, so
//! `send "hi" # greet` was reported as unknown syntax.
//!
//! The rule is a transcription of Skript's `Node.splitLine` state machine:
//!
//! * a `#` outside a string starts a comment, and so does a line whose first
//!   non-blank character is `#`;
//! * a doubled `#`, `"` or `%` is an escape. A doubled `#` only counts outside
//!   a string, where a lone one would end the line; inside a string a lone `#`
//!   is already literal;
//! * a `%` inside a string drops back to code, so `"%{_x} # y%"` is cut at the
//!   `#`. The closing `%` restores whatever state preceded the opening one.
//!
//! Braces are deliberately **not** part of the rule. `splitLine`'s transition
//! table has `{` and `}` cases, but its loop only ever feeds it `%`, `"` and
//! `#`, so a `#` in a variable name outside a string (`{a#b}`) really does end
//! the line when Skript loads it. Matching that is what keeps every feature
//! here agreeing with the server about what the script says.

/// The byte offset where `line`'s comment starts, if it has one.
pub fn comment_start(line: &str) -> Option<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        Str,
    }

    let indent = skript_index::indent_width(line);
    if line[indent..].starts_with('#') {
        return Some(indent);
    }

    let bytes = line.as_bytes();
    let mut state = State::Code;
    let mut previous = State::Code;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'%' | b'"' | b'#') {
            if (byte != b'#' || state != State::Str) && bytes.get(index + 1) == Some(&byte) {
                index += 2;
                continue;
            }
            let before = state;
            state = match (byte, state) {
                (b'"', State::Code) => State::Str,
                (b'"', State::Str) => State::Code,
                (b'%', State::Code) => previous,
                (b'%', State::Str) => State::Code,
                (b'#', State::Code) => return Some(index),
                _ => state,
            };
            if byte == b'%' && state == State::Code {
                previous = before;
            }
        }
        index += 1;
    }
    None
}

/// `line` with its comment removed. Leading whitespace is kept, so byte
/// columns into the result are columns into `line`.
pub fn code_part(line: &str) -> &str {
    &line[..comment_start(line).unwrap_or(line.len())]
}

/// Whether `line` opens or closes a `###` block comment.
pub fn is_block_comment_delimiter(line: &str) -> bool {
    line.trim() == "###"
}

/// Every line of `text` with its number, except the delimiters and interior
/// of `###` block comments.
///
/// A block comment's interior is prose. Classifying it would paint arbitrary
/// English as syntax, and it is what `skript-format` and the indentation
/// diagnostics already treat as verbatim: they all have to agree.
pub fn code_lines(text: &str) -> impl Iterator<Item = (u32, &str)> {
    let mut in_block_comment = false;
    text.lines().enumerate().filter_map(move |(number, line)| {
        if is_block_comment_delimiter(line) {
            in_block_comment = !in_block_comment;
            return None;
        }
        (!in_block_comment).then_some((number as u32, line))
    })
}

/// Line numbers inside `###` block comments, delimiters excluded.
pub fn block_comment_lines(text: &str) -> std::collections::HashSet<usize> {
    let mut inside = false;
    let mut lines = std::collections::HashSet::new();
    for (number, line) in text.lines().enumerate() {
        if is_block_comment_delimiter(line) {
            inside = !inside;
            continue;
        }
        if inside {
            lines.insert(number);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_inside_a_string_is_literal() {
        assert_eq!(
            code_part(r#"send "item #1" to player"#),
            r#"send "item #1" to player"#
        );
        assert_eq!(code_part("stop # done"), "stop ");
    }

    #[test]
    fn a_doubled_hash_is_an_escape_outside_a_string() {
        assert_eq!(code_part("set {count##} to 1"), "set {count##} to 1");
        assert_eq!(comment_start("\tprefix: <##ff0000>Hi"), None);
    }

    #[test]
    fn a_whole_line_comment_starts_at_its_hash() {
        assert_eq!(comment_start("# note"), Some(0));
        assert_eq!(comment_start("\t## not an escape here"), Some(1));
        assert_eq!(code_part("    # aligned"), "    ");
    }

    /// `splitLine` never feeds braces to its state machine, so Skript reads a
    /// lone `#` in a variable name outside a string as a comment.
    #[test]
    fn braces_do_not_protect_a_hash() {
        assert_eq!(code_part("set {a#b} to 1"), "set {a");
    }

    #[test]
    fn interpolation_returns_to_code() {
        assert_eq!(
            code_part(r#"send "%{_x}% &a" # &c"#),
            r#"send "%{_x}% &a" "#
        );
        // `%` inside a string is code, where a single `#` is a comment.
        assert_eq!(code_part(r#"send "%{_x} # &c%""#), r#"send "%{_x} "#);
        assert_eq!(comment_start(r#"send "100%% &a""#), None);
    }

    #[test]
    fn doubled_quotes_do_not_end_the_string() {
        assert_eq!(comment_start(r#"send "say ""hi"" #1 &e""#), None);
    }

    #[test]
    fn block_comments_are_skipped_with_their_delimiters() {
        let text = "a\n###\nprose # here\n  ###  \nb\n";
        let kept: Vec<(u32, &str)> = code_lines(text).collect();
        assert_eq!(kept, vec![(0, "a"), (4, "b")]);
        assert_eq!(
            block_comment_lines(text).into_iter().collect::<Vec<_>>(),
            vec![2]
        );
    }
}
