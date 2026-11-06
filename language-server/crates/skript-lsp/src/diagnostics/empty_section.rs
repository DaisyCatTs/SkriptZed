//! A section header with nothing under it.
//!
//! Skript's config reader warns "Empty configuration section!" for any line
//! ending in `:` whose section holds only blank and comment lines. The check
//! lives in `SectionNode.load_i`, before any structure sees the node, and a
//! script is loaded with `allowEmptySections` off, so it applies to
//! `options:`, `variables:` and `aliases:` exactly as to `on join:`.
//!
//! Transcribed from that reader rather than read from the parse tree. The
//! tree gives a comment-only body a `block`, where Skript counts it as empty,
//! and the reader has edges worth matching:
//!
//! * a line whose code is followed by `#-#` never opens a section;
//! * the warning comes from the line that *ends* the section, one indented by
//!   fewer levels. A deeper line that is not exactly one level in is an
//!   indentation error instead, and nothing is said here;
//! * a header on the file's last line warns, but one followed only by blank
//!   or comment lines up to the end of the file does not: the reader's loop
//!   runs out before it reaches the check.

use skript_index::{Document, Range};

use super::layout::Lines;
use super::{Diagnostic, Severity};

pub(super) const CODE: &str = "empty-section";

/// Skript's own message, from `SectionNode.M_EMPTY_SECTION`.
const MESSAGE: &str = "Empty configuration section! You might want to indent one or more of \
                       the subsequent lines to make them belong to this section or remove the \
                       colon at the end of the line if you don't want this line to start a \
                       section.";

/// Skript's `fullLinePattern`, `([^#]|##)*#-#(\s.*)?`, matched against the
/// raw line: the first lone `#` starts `#-#`, followed by whitespace or the
/// end of the line.
fn suppressed(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#' {
            if bytes.get(index + 1) == Some(&b'#') {
                index += 2;
                continue;
            }
            return line[index..].starts_with("#-#")
                && line[index + 3..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace);
        }
        index += 1;
    }
    false
}

/// The indent unit of the top-level structure around `number`: Skript sets
/// it from the structure's first indented line. `None` when that line mixes
/// tabs and spaces, which is an error of its own.
fn structure_unit<'t>(lines: &Lines<'t>, number: usize) -> Option<&'t str> {
    let start = (0..=number).rev().find(|&at| {
        lines
            .code(at)
            .is_some_and(|code| skript_index::indent_width(code) == 0)
    })?;
    let first = (start + 1..lines.lines.len())
        .filter_map(|at| lines.code(at))
        .find(|code| skript_index::indent_width(code) > 0)?;
    let unit = &first[..skript_index::indent_width(first)];
    let uniform = unit.bytes().all(|byte| byte == unit.as_bytes()[0]);
    uniform.then_some(unit)
}

/// How many `unit`s `indent` is, when it is exactly that.
fn levels(indent: &str, unit: &str) -> Option<usize> {
    if indent.is_empty() {
        return Some(0);
    }
    let count = indent.len() / unit.len();
    (indent.len().is_multiple_of(unit.len()) && unit.repeat(count) == indent).then_some(count)
}

/// Whether a code line indented by `next` closes a section opened by a
/// header indented by `header`, rather than belonging to it.
fn ends_section(lines: &Lines<'_>, number: usize, header: &str, next: &str) -> bool {
    if next.is_empty() {
        return true;
    }
    // A top-level header's first indented line sets the unit, so it is
    // always inside.
    if header.is_empty() {
        return false;
    }
    let Some(unit) = structure_unit(lines, number) else {
        return false;
    };
    match (levels(header, unit), levels(next, unit)) {
        (Some(header), Some(next)) => next <= header,
        _ => false,
    }
}

pub(super) fn check(document: &Document, out: &mut Vec<Diagnostic>) {
    let lines = Lines::new(document.text());
    let total = lines.lines.len();

    for number in 0..total {
        let Some(code) = lines.code(number) else {
            continue;
        };
        if !code.trim_end().ends_with(':') || suppressed(lines.lines[number]) {
            continue;
        }
        let indent = skript_index::indent_width(code);
        let next = (number + 1..total).find_map(|at| lines.code(at));
        let empty = match next {
            None => number + 1 == total,
            Some(next) => ends_section(
                &lines,
                number,
                &code[..indent],
                &next[..skript_index::indent_width(next)],
            ),
        };
        if !empty {
            continue;
        }
        out.push(Diagnostic {
            range: Range::on_line(
                number as u32,
                indent as u32,
                (indent + code.trim().len()) as u32,
            ),
            severity: Severity::Warning,
            message: MESSAGE.into(),
            code: CODE,
            related: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(source: &str) -> Vec<u32> {
        let document = Document::new("file:///t.sk", source);
        let mut out = Vec::new();
        check(&document, &mut out);
        out.into_iter().map(|d| d.range.start.line).collect()
    }

    #[test]
    fn a_header_followed_by_a_dedent_is_empty() {
        assert_eq!(lines("on join:\non quit:\n\tstop\n"), vec![0]);
        assert_eq!(
            lines("on join:\n\tif player is op:\n\tsend \"hi\"\n"),
            vec![1]
        );
        // Two levels in, closed by a line one level in.
        assert_eq!(
            lines("on join:\n\tloop 3 times:\n\t\tif x:\n\t\tstop\n"),
            vec![2]
        );
    }

    #[test]
    fn the_message_is_skripts_own() {
        let document = Document::new("file:///t.sk", "options:\non join:\n\tstop\n");
        let mut out = Vec::new();
        check(&document, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, Severity::Warning);
        assert!(out[0].message.starts_with("Empty configuration section!"));
        assert_eq!(out[0].range, Range::on_line(0, 0, 8));
    }

    /// Skript's reader counts blank and comment lines as nothing.
    #[test]
    fn a_body_of_only_comments_is_still_empty() {
        assert_eq!(
            lines("on join:\n\t# todo\n\n###\nprose\n###\non quit:\n\tstop\n"),
            vec![0]
        );
    }

    #[test]
    fn a_header_on_the_last_line_is_empty() {
        assert_eq!(lines("on join:\n\tstop\non quit:"), vec![2]);
        assert_eq!(lines("on join:\n\tstop\non quit:\n"), vec![2]);
    }

    /// The reader's loop ends before its check when only blank or comment
    /// lines follow the header.
    #[test]
    fn trailing_blank_or_comment_lines_hide_it() {
        assert!(lines("on join:\n\tstop\non quit:\n\n").is_empty());
        assert!(lines("on join:\n\tstop\non quit:\n\t# later\n").is_empty());
    }

    #[test]
    fn a_body_is_not_empty() {
        assert!(lines("on join:\n\tsend \"hi\"\n").is_empty());
        assert!(lines("options:\n\tprefix: x\nvariables:\n\t{a} = 1\n").is_empty());
        assert!(lines("on join:\n    if x:\n        stop\n    stop\n").is_empty());
    }

    #[test]
    fn hash_dash_hash_keeps_a_colon_from_opening_a_section() {
        assert!(lines("on join:\n\tsend \"a\" to player: #-#\n\tstop\n").is_empty());
        // `##` is an escaped `#`, so the marker after it still counts.
        assert!(lines("on join:\n\tsend {a##}: #-# x\n\tstop\n").is_empty());
        // `#-#-` is not the marker.
        assert_eq!(lines("on join:\n\tsend a: #-#-\n\tstop\n"), vec![1]);
    }

    #[test]
    fn a_colon_inside_a_comment_or_string_is_not_a_header() {
        assert!(lines("on join:\n\tstop # note:\n").is_empty());
        assert!(lines("on join:\n\tsend \"a:\"\n").is_empty());
    }

    /// A deeper line that is not one level in is an indentation error, which
    /// Skript reports instead of this.
    #[test]
    fn a_misindented_line_is_not_this_warning() {
        assert!(lines("on join:\n\tif x:\n\t\t\tstop\n").is_empty());
        assert!(lines("on join:\n    if x:\n  stop\n").is_empty());
    }
}
