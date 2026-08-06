//! Position conversion between tree-sitter and LSP.
//!
//! These two disagree about what a "character" is, and getting it wrong shows
//! up as highlights and rename edits landing one or two columns off — but only
//! on lines containing non-ASCII text, which is exactly the kind of bug that
//! survives testing.
//!
//! * tree-sitter's `Point::column` is a **byte** offset within the line.
//! * LSP defaults to **UTF-16 code units**, and only uses UTF-8 when the client
//!   and server agree on it during initialisation.
//!
//! Skript scripts carry plenty of non-ASCII: `§` colour codes, `¦` parse marks,
//! and player-facing messages in every language, so this is not hypothetical.

use skript_index::{Position, Range};
use tower_lsp::lsp_types as lsp;

/// Which encoding the client negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

impl Encoding {
    /// Picks UTF-8 when the client offers it, since it makes every conversion a
    /// no-op; otherwise falls back to the LSP default.
    pub fn negotiate(client: Option<&[lsp::PositionEncodingKind]>) -> Self {
        match client {
            Some(kinds) if kinds.contains(&lsp::PositionEncodingKind::UTF8) => Encoding::Utf8,
            _ => Encoding::Utf16,
        }
    }

    pub fn kind(self) -> lsp::PositionEncodingKind {
        match self {
            Encoding::Utf8 => lsp::PositionEncodingKind::UTF8,
            Encoding::Utf16 => lsp::PositionEncodingKind::UTF16,
        }
    }
}

/// A document's lines, indexed once.
///
/// `nth_line` walks the whole buffer, and `semantic::encode` converts two
/// columns per token — so on a 1,000-line file with a few thousand tokens the
/// naive path rescanned the document about ten thousand times per request.
/// Building this once turns that into a single pass.
pub struct LineIndex<'a> {
    lines: Vec<&'a str>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            lines: text
                .split('\n')
                .map(|line| line.trim_end_matches('\r'))
                .collect(),
        }
    }

    fn line(&self, line: u32) -> &str {
        self.lines.get(line as usize).copied().unwrap_or("")
    }

    /// Converts a byte column on `line` to the client's column encoding.
    pub fn to_column(&self, line: u32, byte: u32, encoding: Encoding) -> u32 {
        match encoding {
            Encoding::Utf8 => byte,
            Encoding::Utf16 => {
                let text = self.line(line);
                let byte = (byte as usize).min(text.len());
                text[..floor_char_boundary(text, byte)]
                    .encode_utf16()
                    .count() as u32
            }
        }
    }

    pub fn to_lsp_position(&self, position: Position, encoding: Encoding) -> lsp::Position {
        lsp::Position {
            line: position.line,
            character: self.to_column(position.line, position.character, encoding),
        }
    }

    pub fn to_lsp_range(&self, range: Range, encoding: Encoding) -> lsp::Range {
        lsp::Range {
            start: self.to_lsp_position(range.start, encoding),
            end: self.to_lsp_position(range.end, encoding),
        }
    }
}

/// Converts an internal position (byte column) to an LSP one.
///
/// Fine for a one-off; build a [`LineIndex`] when converting many positions
/// against the same document.
pub fn to_lsp_position(text: &str, position: Position, encoding: Encoding) -> lsp::Position {
    let character = match encoding {
        Encoding::Utf8 => position.character,
        Encoding::Utf16 => {
            let line = nth_line(text, position.line);
            let byte = (position.character as usize).min(line.len());
            // Round a mid-character byte offset down to a boundary rather than
            // panicking on a slice.
            let prefix = &line[..floor_char_boundary(line, byte)];
            prefix.encode_utf16().count() as u32
        }
    };
    lsp::Position {
        line: position.line,
        character,
    }
}

/// Converts an LSP position back to an internal one (byte column).
pub fn from_lsp_position(text: &str, position: lsp::Position, encoding: Encoding) -> Position {
    let character = match encoding {
        Encoding::Utf8 => position.character,
        Encoding::Utf16 => {
            let line = nth_line(text, position.line);
            let mut utf16 = 0usize;
            let mut bytes = 0usize;
            for ch in line.chars() {
                if utf16 >= position.character as usize {
                    break;
                }
                utf16 += ch.len_utf16();
                bytes += ch.len_utf8();
            }
            bytes as u32
        }
    };
    Position {
        line: position.line,
        character,
    }
}

pub fn to_lsp_range(text: &str, range: Range, encoding: Encoding) -> lsp::Range {
    lsp::Range {
        start: to_lsp_position(text, range.start, encoding),
        end: to_lsp_position(text, range.end, encoding),
    }
}

fn nth_line(text: &str, line: u32) -> &str {
    text.split('\n')
        .nth(line as usize)
        .unwrap_or("")
        .trim_end_matches('\r')
}

/// The text of `line` up to `character`, clamped to a real char boundary.
///
/// The obvious `&line[..character.min(line.len())]` **panics**, and the length
/// clamp does not save it: slicing a `&str` at a byte offset that lands inside a
/// multi-byte character is a panic, and under the UTF-8 position encoding
/// `from_lsp_position` passes the client's column straight through as a byte
/// offset. A client that is one change out of sync, or that counts codepoints
/// while advertising `utf-8`, then kills the server on any line containing `§`,
/// `é` or an emoji — which in Skript means most lines that contain a message.
///
/// Truncating to the boundary below is the right recovery: the caller wants the
/// prefix in order to read what the user has typed, and half a character is not
/// something they typed.
pub fn line_prefix(line: &str, character: u32) -> &str {
    let byte = (character as usize).min(line.len());
    &line[..floor_char_boundary(line, byte)]
}

/// `str::floor_char_boundary` is still unstable, so this is the stable spelling.
fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "send \"héllo ✦\" to player";

    #[test]
    fn utf8_is_a_pass_through() {
        let position = Position::new(0, 7);
        let lsp = to_lsp_position(LINE, position, Encoding::Utf8);
        assert_eq!(lsp.character, 7);
        assert_eq!(from_lsp_position(LINE, lsp, Encoding::Utf8).character, 7);
    }

    #[test]
    fn utf16_accounts_for_multibyte_characters() {
        // `é` is two bytes but one UTF-16 unit, so byte 8 is UTF-16 column 7.
        let lsp = to_lsp_position(LINE, Position::new(0, 8), Encoding::Utf16);
        assert_eq!(lsp.character, 7);
    }

    #[test]
    fn utf16_round_trips() {
        for byte in 0..LINE.len() as u32 {
            let position = Position::new(0, byte);
            let lsp = to_lsp_position(LINE, position, Encoding::Utf16);
            let back = from_lsp_position(LINE, lsp, Encoding::Utf16);
            // Round-tripping lands on a character boundary at or before the
            // original byte, never past it.
            assert!(back.character <= byte);
        }
    }

    #[test]
    fn a_byte_offset_inside_a_character_does_not_panic() {
        // Byte 7 is the middle of `é`.
        let lsp = to_lsp_position(LINE, Position::new(0, 7), Encoding::Utf16);
        assert!(lsp.character <= 7);
    }

    #[test]
    fn an_out_of_range_line_is_treated_as_empty() {
        let lsp = to_lsp_position(LINE, Position::new(99, 3), Encoding::Utf16);
        assert_eq!(lsp.character, 0);
    }

    #[test]
    fn negotiation_prefers_utf8_when_offered() {
        assert_eq!(
            Encoding::negotiate(Some(&[
                lsp::PositionEncodingKind::UTF16,
                lsp::PositionEncodingKind::UTF8
            ])),
            Encoding::Utf8
        );
        assert_eq!(Encoding::negotiate(None), Encoding::Utf16);
    }
}

#[cfg(test)]
mod line_prefix_tests {
    use super::line_prefix;

    #[test]
    fn a_column_inside_a_multibyte_character_does_not_panic() {
        // The bug this guards: `&line[..n]` panics when `n` lands inside a
        // character, and clamping to `line.len()` does not prevent that. Skript
        // messages are full of `§` and emoji, so the offsets below are ordinary,
        // not contrived.
        let line = "send \"héllo ✦\" to player";
        // Walk every byte offset, including the ones inside `é` and `✦`.
        for byte in 0..=line.len() + 4 {
            let prefix = line_prefix(line, byte as u32);
            assert!(line.starts_with(prefix));
        }
    }

    #[test]
    fn it_truncates_to_the_boundary_below() {
        let line = "aé";
        // `é` occupies bytes 1..3, so byte 2 is mid-character.
        assert_eq!(line_prefix(line, 2), "a");
        assert_eq!(line_prefix(line, 3), "aé");
    }

    #[test]
    fn a_column_past_the_end_yields_the_whole_line() {
        assert_eq!(line_prefix("set {_x}", 999), "set {_x}");
    }
}
