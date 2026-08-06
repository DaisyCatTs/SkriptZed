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

/// Converts an internal position (byte column) to an LSP one.
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
