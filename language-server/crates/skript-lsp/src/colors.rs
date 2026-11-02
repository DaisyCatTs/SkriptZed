//! Colour swatches: `textDocument/documentColor` and `colorPresentation`.
//!
//! Recognised spellings, each checked against what Skript itself parses
//! (`TextComponentParser` / `TextComponentUtils` since 2.15, `ChatMessages`
//! before that):
//!
//! * `&0`–`&f` — the 16 legacy chat colours. `&k`–`&o` and `&r` are formatting,
//!   not colours, and get no swatch.
//! * `&x&r&r&g&g&b&b` — Spigot's legacy hex, which Skript converts natively.
//! * `&#rrggbb` — not Skript-native, but common in the wild (other chat
//!   plugins expand it) and the grammar's `legacy_color` accepts it.
//! * `<#rrggbb>`, `<##rrggbb>` — MiniMessage hex and Skript's older double-hash
//!   form, which the parser still rewrites to the single-hash one.
//! * `<red>`, `<dark_grey>`, `<dark blue>`, `<orange>`, `<color:red>`, … —
//!   MiniMessage's named colours plus the compatibility aliases Skript
//!   registers on top of them.
//!
//! The scan is textual rather than tree-based because the comment rule that
//! matters is Skript's (`Node.splitLine`, transcribed in `lines`), and it
//! differs from the grammar's in one visible place: outside a string, a single
//! `#` ends the line even inside `&#ff0000` or `<#ff0000>`. Such a bare option
//! value really is commented out when Skript loads it, so it gets no swatch.

/// One colour written in the script.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundColor {
    pub line: u32,
    /// Byte columns within the line, start inclusive, end exclusive, covering
    /// the whole code (`&6`, `&#ff0000`, `<#ff0000>`, `<red>`).
    pub start: u32,
    pub end: u32,
    /// 0.0..=1.0 each.
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    const fn from_u32(value: u32) -> Self {
        Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    fn from_floats(red: f32, green: f32, blue: f32) -> Self {
        // A NaN survives `clamp` but saturates to 0 in the cast.
        let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgb(channel(red), channel(green), channel(blue))
    }

    fn hex(self, upper: bool) -> String {
        let Rgb(r, g, b) = self;
        if upper {
            format!("{r:02X}{g:02X}{b:02X}")
        } else {
            format!("{r:02x}{g:02x}{b:02x}")
        }
    }
}

/// The vanilla chat palette, by legacy code and canonical MiniMessage name.
const PALETTE: [(u8, &str, Rgb); 16] = [
    (b'0', "black", Rgb::from_u32(0x000000)),
    (b'1', "dark_blue", Rgb::from_u32(0x0000AA)),
    (b'2', "dark_green", Rgb::from_u32(0x00AA00)),
    (b'3', "dark_aqua", Rgb::from_u32(0x00AAAA)),
    (b'4', "dark_red", Rgb::from_u32(0xAA0000)),
    (b'5', "dark_purple", Rgb::from_u32(0xAA00AA)),
    (b'6', "gold", Rgb::from_u32(0xFFAA00)),
    (b'7', "gray", Rgb::from_u32(0xAAAAAA)),
    (b'8', "dark_gray", Rgb::from_u32(0x555555)),
    (b'9', "blue", Rgb::from_u32(0x5555FF)),
    (b'a', "green", Rgb::from_u32(0x55FF55)),
    (b'b', "aqua", Rgb::from_u32(0x55FFFF)),
    (b'c', "red", Rgb::from_u32(0xFF5555)),
    (b'd', "light_purple", Rgb::from_u32(0xFF55FF)),
    (b'e', "yellow", Rgb::from_u32(0xFFFF55)),
    (b'f', "white", Rgb::from_u32(0xFFFFFF)),
];

/// Names that are not canonical but still resolve to a colour: MiniMessage's
/// own `grey` spellings, then `TextComponentParser.registerCompatibilityTags`.
const ALIASES: &[(&str, &str)] = &[
    ("grey", "gray"),
    ("dark_grey", "dark_gray"),
    ("dark_cyan", "dark_aqua"),
    ("dark_turquoise", "dark_aqua"),
    ("cyan", "dark_aqua"),
    ("purple", "dark_purple"),
    ("dark_yellow", "gold"),
    ("orange", "gold"),
    ("light_grey", "gray"),
    ("light_gray", "gray"),
    ("silver", "gray"),
    ("dark_silver", "dark_gray"),
    ("light_blue", "blue"),
    ("indigo", "blue"),
    ("light_green", "green"),
    ("lime_green", "green"),
    ("lime", "green"),
    ("light_cyan", "aqua"),
    ("light_aqua", "aqua"),
    ("turquoise", "aqua"),
    ("light_red", "red"),
    ("pink", "light_purple"),
    ("magenta", "light_purple"),
    ("light_yellow", "yellow"),
];

/// Skript maps `<brown>` to `DyeColor.BROWN`, the only alias off the palette.
const BROWN: Rgb = Rgb::from_u32(0x835432);

/// MiniMessage's argument-taking colour tag: `<color:red>`, `<c:#ff0000>`.
const COLOR_TAG_NAMES: [&str; 3] = ["color", "colour", "c"];

/// Longest tag body worth considering. The longest real one is around 20
/// bytes; the cap keeps a line full of unclosed `<a` linear to scan.
const MAX_TAG_BODY: usize = 32;

fn palette_by_code(code: u8) -> Option<Rgb> {
    let code = code.to_ascii_lowercase();
    PALETTE
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, _, rgb)| *rgb)
}

/// Resolves a lowercase, underscore-separated colour name.
fn named_color(name: &str) -> Option<Rgb> {
    if name == "brown" {
        return Some(BROWN);
    }
    let canonical = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |&(_, canonical)| canonical);
    PALETTE
        .iter()
        .find(|(_, n, _)| *n == canonical)
        .map(|(_, _, rgb)| *rgb)
}

fn parse_hex6(digits: &[u8]) -> Option<Rgb> {
    if digits.len() != 6 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let text = std::str::from_utf8(digits).ok()?;
    u32::from_str_radix(text, 16).ok().map(Rgb::from_u32)
}

/// Returns every colour in `text`, skipping comments the way Skript does
/// (see `lines`).
pub fn document_colors(text: &str) -> Vec<FoundColor> {
    let mut found = Vec::new();
    for (index, line) in crate::lines::code_lines(text) {
        scan_line(crate::lines::code_part(line).as_bytes(), index, &mut found);
    }
    found
}

fn scan_line(code: &[u8], line: u32, found: &mut Vec<FoundColor>) {
    let mut i = 0;
    while i < code.len() {
        let matched = match code[i] {
            b'&' | b'<' if is_escaped(code, i) => None,
            b'&' => match_legacy(&code[i..]),
            b'<' => match_tag(&code[i..]),
            _ => None,
        };
        let Some((len, Rgb(r, g, b))) = matched else {
            i += 1;
            continue;
        };
        found.push(FoundColor {
            line,
            start: i as u32,
            end: (i + len) as u32,
            red: f32::from(r) / 255.0,
            green: f32::from(g) / 255.0,
            blue: f32::from(b) / 255.0,
        });
        // Skip the whole code so `&x&f&f…` is not re-read as six `&f`s.
        i += len;
    }
}

/// An odd run of backslashes escapes the code, in both Skript's legacy-code
/// pattern and MiniMessage.
fn is_escaped(code: &[u8], at: usize) -> bool {
    code[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// `s` starts at an `&`. Returns the matched length and colour.
fn match_legacy(s: &[u8]) -> Option<(usize, Rgb)> {
    match *s.get(1)? {
        b'#' => parse_hex6(s.get(2..8)?).map(|rgb| (8, rgb)),
        // `x` is not a palette code, so a malformed run matches nothing here.
        b'x' => spigot_hex(s).map(|rgb| (14, rgb)),
        code => palette_by_code(code).map(|rgb| (2, rgb)),
    }
}

/// `&x&r&r&g&g&b&b`; Skript's pattern takes only a lowercase `x`.
fn spigot_hex(s: &[u8]) -> Option<Rgb> {
    let (pairs, _) = s.get(2..14)?.as_chunks::<2>();
    if !pairs.iter().all(|&[amp, _]| amp == b'&') {
        return None;
    }
    let digits: Vec<u8> = pairs.iter().map(|&[_, digit]| digit).collect();
    parse_hex6(&digits)
}

/// `s` starts at a `<`. Returns the matched length and colour.
fn match_tag(s: &[u8]) -> Option<(usize, Rgb)> {
    // `<` must be followed immediately by a tag character: this is what keeps
    // `{_x} < 5` and `</red>` out.
    let first = *s.get(1)?;
    if first != b'#' && !first.is_ascii_alphabetic() {
        return None;
    }
    let close = s
        .iter()
        .take(MAX_TAG_BODY + 2)
        .skip(1)
        .position(|&b| b == b'>' || b == b'<')
        .map(|offset| offset + 1)?;
    if s[close] != b'>' {
        return None;
    }
    let body = std::str::from_utf8(&s[1..close]).ok()?;
    tag_color(body).map(|rgb| (close + 1, rgb))
}

/// The colour a tag body (between `<` and `>`) names, if any.
fn tag_color(body: &str) -> Option<Rgb> {
    if let Some(digits) = body.strip_prefix("##").or_else(|| body.strip_prefix('#')) {
        return parse_hex6(digits.as_bytes());
    }
    let lower = body.to_ascii_lowercase();
    if let Some((name, value)) = lower.split_once(':') {
        if !COLOR_TAG_NAMES.contains(&name) {
            return None;
        }
        return match value.strip_prefix('#') {
            Some(digits) => parse_hex6(digits.as_bytes()),
            None => named_color(value),
        };
    }
    named_color(&single_word_name(&lower)?)
}

/// Skript rewrites `<dark blue>` to `<dark_blue>` before MiniMessage sees it,
/// but only for exactly two letter-only words joined by one space.
fn single_word_name(lower: &str) -> Option<String> {
    let Some((left, right)) = lower.split_once(' ') else {
        return Some(lower.to_owned());
    };
    let is_word = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphabetic());
    (is_word(left) && is_word(right)).then(|| format!("{left}_{right}"))
}

/// How the original code was spelled, so a replacement can keep the style.
enum Style<'a> {
    /// `&c`; the code character, to keep its case.
    LegacyCode(u8),
    /// `&#rrggbb`
    LegacyHex { upper: bool },
    /// `&x&r&r&g&g&b&b`
    SpigotHex { upper: bool },
    /// `<#rrggbb>` or, with `double`, `<##rrggbb>`
    TagHex { double: bool, upper: bool },
    /// `<red>`, `<dark blue>`; the original text, reused if the colour is kept.
    TagNamed(&'a str),
    /// `<color:…>`; the tag name as written.
    TagColorArg(&'a str),
}

fn has_upper_hex(digits: &str) -> bool {
    digits.bytes().any(|b| b.is_ascii_uppercase())
}

fn style_of(original: &str) -> Style<'_> {
    let bytes = original.as_bytes();
    if bytes.first() == Some(&b'&') {
        return match bytes.get(1).copied() {
            Some(b'#') => Style::LegacyHex {
                upper: has_upper_hex(&original[2..]),
            },
            Some(b'x') if bytes.len() > 2 => Style::SpigotHex {
                upper: has_upper_hex(&original[2..].replace(['&', 'x'], "")),
            },
            Some(code) => Style::LegacyCode(code),
            None => Style::LegacyHex { upper: false },
        };
    }
    let body = original
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(original);
    if let Some(digits) = body.strip_prefix("##") {
        return Style::TagHex {
            double: true,
            upper: has_upper_hex(digits),
        };
    }
    if let Some(digits) = body.strip_prefix('#') {
        return Style::TagHex {
            double: false,
            upper: has_upper_hex(digits),
        };
    }
    match body.split_once(':') {
        Some((name, _)) => Style::TagColorArg(name),
        None => Style::TagNamed(original),
    }
}

fn palette_entry(rgb: Rgb) -> Option<(u8, &'static str)> {
    PALETTE
        .iter()
        .find(|(_, _, candidate)| *candidate == rgb)
        .map(|(code, name, _)| (*code, *name))
}

/// Replacement spellings for a colour picked in the editor, given the
/// original code's text, keeping the original's style: `&#RRGGBB` stays
/// legacy-hex, `<#RRGGBB>` stays a tag, and a named `<red>` or a `&c` becomes
/// the matching hex form of its own style (`<#rrggbb>` / `&#rrggbb`) — unless
/// the pick lands exactly on a palette colour, when the code or name is kept.
/// Always returns at least one string; the first is preferred.
pub fn presentations(original: &str, red: f32, green: f32, blue: f32) -> Vec<String> {
    let rgb = Rgb::from_floats(red, green, blue);
    let legacy_hex = |upper: bool| format!("&#{}", rgb.hex(upper));
    let spigot_hex = |upper: bool| {
        let digits = rgb.hex(upper);
        let pairs: String = digits.chars().map(|d| format!("&{d}")).collect();
        format!("&x{pairs}")
    };
    let tag_hex = |upper: bool| format!("<#{}>", rgb.hex(upper));
    let tag_double_hex = |upper: bool| format!("<##{}>", rgb.hex(upper));

    let candidates = match style_of(original) {
        Style::LegacyCode(code) => {
            let mut list = Vec::new();
            if let Some((palette_code, _)) = palette_entry(rgb) {
                let code_char = if code.is_ascii_uppercase() {
                    palette_code.to_ascii_uppercase()
                } else {
                    palette_code
                };
                list.push(format!("&{}", code_char as char));
            }
            // `&x&r&r…` first: it is the hex form Skript itself renders. `&#rrggbb` only
            // works where another chat plugin expands it.
            list.extend([spigot_hex(false), legacy_hex(false)]);
            list
        }
        Style::LegacyHex { upper } => vec![legacy_hex(upper), spigot_hex(upper)],
        Style::SpigotHex { upper } => vec![spigot_hex(upper), legacy_hex(upper)],
        Style::TagHex { double, upper } if double => {
            vec![tag_double_hex(upper), tag_hex(upper)]
        }
        Style::TagHex { upper, .. } => vec![tag_hex(upper), tag_double_hex(upper)],
        Style::TagNamed(text) => {
            let mut list = Vec::new();
            let body = text.trim_start_matches('<').trim_end_matches('>');
            if tag_color(body) == Some(rgb) {
                list.push(text.to_owned());
            }
            if let Some((_, name)) = palette_entry(rgb) {
                list.push(format!("<{name}>"));
            }
            list.extend([tag_hex(false), tag_double_hex(false)]);
            list
        }
        Style::TagColorArg(name) => {
            let hex_value = original.split_once(":#").map(|(_, digits)| digits);
            let hex_arg = format!(
                "<{name}:#{}>",
                rgb.hex(hex_value.is_some_and(has_upper_hex))
            );
            let named_arg = palette_entry(rgb).map(|(_, n)| format!("<{name}:{n}>"));
            // Hex stays hex; a named value stays named when the pick allows it.
            let mut list = match hex_value {
                Some(_) => [Some(hex_arg), named_arg].into_iter().flatten().collect(),
                None => [named_arg, Some(hex_arg)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>(),
            };
            list.push(tag_hex(false));
            list
        }
    };
    dedup_keep_order(candidates)
}

fn dedup_keep_order(items: Vec<String>) -> Vec<String> {
    let mut unique: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (line, start, end, "rrggbb") for compact assertions.
    fn spans(text: &str) -> Vec<(u32, u32, u32, String)> {
        document_colors(text)
            .into_iter()
            .map(|c| {
                let rgb = Rgb::from_floats(c.red, c.green, c.blue);
                (c.line, c.start, c.end, rgb.hex(false))
            })
            .collect()
    }

    fn one(text: &str) -> (u32, u32, u32, String) {
        let found = spans(text);
        assert_eq!(
            found.len(),
            1,
            "expected exactly one colour in {text:?}: {found:?}"
        );
        found.into_iter().next().unwrap()
    }

    fn hexes(text: &str) -> Vec<String> {
        spans(text).into_iter().map(|(_, _, _, hex)| hex).collect()
    }

    #[test]
    fn legacy_codes_use_the_vanilla_palette() {
        let text = "send \"&0&1&2&3&4&5&6&7&8&9&a&b&c&d&e&f\"";
        assert_eq!(
            hexes(text),
            [
                "000000", "0000aa", "00aa00", "00aaaa", "aa0000", "aa00aa", "ffaa00", "aaaaaa",
                "555555", "5555ff", "55ff55", "55ffff", "ff5555", "ff55ff", "ffff55", "ffffff",
            ]
        );
        assert_eq!(one("send \"&6hi\""), (0, 6, 8, "ffaa00".into()));
    }

    #[test]
    fn legacy_codes_are_case_insensitive() {
        assert_eq!(one("send \"&C!\"").3, "ff5555");
    }

    #[test]
    fn formatting_codes_are_not_colours() {
        assert!(spans("send \"&l&m&n&o&k&r&L&R\"").is_empty());
    }

    #[test]
    fn legacy_hex() {
        assert_eq!(one("send \"&#Ff8000x\""), (0, 6, 14, "ff8000".into()));
        assert!(spans("send \"&#ff80\"").is_empty());
        assert!(spans("send \"&#gg0000\"").is_empty());
    }

    #[test]
    fn spigot_hex_is_one_colour_not_six_codes() {
        assert_eq!(
            one("send \"&x&f&f&0&0&a&aHi\""),
            (0, 6, 20, "ff00aa".into())
        );
        // An incomplete run is not hex; its `&f` codes still count on their own.
        assert_eq!(hexes("send \"&x&f&f\""), ["ffffff", "ffffff"]);
    }

    #[test]
    fn hex_tags_single_and_double_hash() {
        assert_eq!(one("send \"<#00FF00>hi\""), (0, 6, 15, "00ff00".into()));
        assert_eq!(one("send \"<##00ff00>hi\""), (0, 6, 16, "00ff00".into()));
        assert!(spans("send \"<#00ff0>\"").is_empty());
        assert!(spans("send \"<###00ff00>\"").is_empty());
    }

    #[test]
    fn named_tags_and_spellings() {
        assert_eq!(one("send \"<red>x\""), (0, 6, 11, "ff5555".into()));
        assert_eq!(one("send \"<dark_blue>\"").3, "0000aa");
        assert_eq!(one("send \"<grey>\"").3, "aaaaaa");
        assert_eq!(one("send \"<dark_grey>\"").3, "555555");
        assert_eq!(one("send \"<dark blue>\""), (0, 6, 17, "0000aa".into()));
        assert_eq!(one("send \"<light purple>\"").3, "ff55ff");
        assert_eq!(one("send \"<RED>\"").3, "ff5555");
    }

    #[test]
    fn skript_compatibility_aliases() {
        assert_eq!(one("send \"<orange>\"").3, "ffaa00");
        assert_eq!(one("send \"<pink>\"").3, "ff55ff");
        assert_eq!(one("send \"<lime green>\"").3, "55ff55");
        assert_eq!(one("send \"<cyan>\"").3, "00aaaa");
        assert_eq!(one("send \"<brown>\"").3, "835432");
    }

    #[test]
    fn color_argument_tags() {
        assert_eq!(one("send \"<color:red>\""), (0, 6, 17, "ff5555".into()));
        assert_eq!(one("send \"<colour:#00ff00>\"").3, "00ff00");
        assert_eq!(one("send \"<c:gold>\"").3, "ffaa00");
        assert!(spans("send \"<color:nope>\"").is_empty());
    }

    #[test]
    fn non_colour_tags_are_skipped() {
        let text = "send \"</red><bold><link:https://x.y><reset><dark  blue><red blue>\"";
        assert!(spans(text).is_empty());
    }

    #[test]
    fn operators_are_not_colours() {
        assert!(spans("if {_a} is 1 && {_b} is 2:").is_empty());
        assert!(spans("if a & b:").is_empty());
        assert!(spans("if {_x} < 5:").is_empty());
        assert!(spans("if {_x} <5 and {_y} > 3:").is_empty());
        assert!(spans("set {_x} to {_y} <- 1").is_empty());
        assert!(spans("command /give <player> <item>:").is_empty());
    }

    #[test]
    fn unclosed_tag_is_not_a_colour() {
        assert!(spans("send \"<red and more\"").is_empty());
        assert!(spans("send \"<red <blue\"").is_empty());
    }

    #[test]
    fn bare_option_values_count() {
        assert_eq!(one("\tprefix: &6[Server]&r"), (0, 9, 11, "ffaa00".into()));
        assert_eq!(one("\tprefix: <gold>[Server]").3, "ffaa00");
    }

    #[test]
    fn trailing_comments_are_skipped() {
        assert_eq!(hexes("send \"&a\" # &c and <red>"), ["55ff55"]);
        assert!(spans("# send \"&c\"").is_empty());
        assert!(spans("    ## &c").is_empty());
    }

    #[test]
    fn hash_inside_a_string_is_literal() {
        assert_eq!(hexes("send \"item #1 &c\" # &a"), ["ff5555"]);
        assert_eq!(hexes("send \"<#ff0000>\""), ["ff0000"]);
    }

    #[test]
    fn single_hash_outside_a_string_is_a_comment_like_skript() {
        // Skript loads this option as `prefix: ` — the rest is a comment.
        assert!(spans("\tprefix: <#ff0000>Hi").is_empty());
        assert!(spans("\tprefix: &#ff0000Hi").is_empty());
        // Doubled, the hash is an escape and the tag survives.
        assert_eq!(one("\tprefix: <##ff0000>Hi").3, "ff0000");
    }

    #[test]
    fn interpolation_returns_to_code_context() {
        assert_eq!(hexes("send \"%{_x}% &a\" # &c"), ["55ff55"]);
        // `%` inside a string is code, where a single `#` is a comment.
        assert!(spans("send \"%{_x} # &c%\"").is_empty());
        assert_eq!(hexes("send \"100%% &a\""), ["55ff55"]);
    }

    #[test]
    fn escaped_quotes_do_not_end_the_string() {
        assert_eq!(hexes("send \"say \"\"hi\"\" #1 &e\""), ["ffff55"]);
    }

    #[test]
    fn block_comments_are_skipped() {
        let text = "send \"&a\"\n###\nsend \"&c\"\n  ### not a closer\n  ###  \nsend \"&e\"\n";
        assert_eq!(
            spans(text),
            [(0, 6, 8, "55ff55".into()), (5, 6, 8, "ffff55".into())]
        );
    }

    #[test]
    fn unterminated_block_comment_hides_the_rest() {
        assert!(spans("###\nsend \"&c\"\nsend \"&a\"").is_empty());
    }

    #[test]
    fn backslash_escapes_a_code() {
        assert!(spans("send \"\\<red>\\&c\"").is_empty());
        assert_eq!(hexes("send \"\\\\<red>\""), ["ff5555"]);
    }

    #[test]
    fn columns_are_bytes_after_multibyte_text() {
        // `é` is two bytes and `🎉` four, so `&c` starts at byte 13.
        let text = "send \"é🎉 &c\"";
        assert_eq!(one(text), (0, 13, 15, "ff5555".into()));
        assert_eq!(&text[13..15], "&c");
    }

    #[test]
    fn several_colours_across_lines_and_crlf() {
        let text = "on join:\r\n\tsend \"&aHi <#123456>there\"\r\n\tsend \"<blue>!\"\r\n";
        assert_eq!(
            spans(text),
            [
                (1, 7, 9, "55ff55".into()),
                (1, 12, 21, "123456".into()),
                (2, 7, 13, "5555ff".into()),
            ]
        );
    }

    #[test]
    fn float_channels_are_exact_for_palette_colours() {
        let found = &document_colors("send \"&6\"")[0];
        assert_eq!(found.red, 1.0);
        assert!((found.green - 170.0 / 255.0).abs() < f32::EPSILON);
        assert_eq!(found.blue, 0.0);
    }

    fn rgb(hex: u32) -> (f32, f32, f32) {
        let Rgb(r, g, b) = Rgb::from_u32(hex);
        (
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        )
    }

    fn present(original: &str, hex: u32) -> Vec<String> {
        let (r, g, b) = rgb(hex);
        presentations(original, r, g, b)
    }

    #[test]
    fn presentation_keeps_legacy_hex() {
        assert_eq!(present("&#ff0000", 0x12ab34)[0], "&#12ab34");
        assert_eq!(present("&#FF0000", 0x12ab34)[0], "&#12AB34");
    }

    #[test]
    fn presentation_keeps_spigot_hex() {
        assert_eq!(present("&x&f&f&0&0&0&0", 0x12ab34)[0], "&x&1&2&a&b&3&4");
    }

    #[test]
    fn presentation_keeps_hex_tags() {
        assert_eq!(present("<#ff0000>", 0x12ab34)[0], "<#12ab34>");
        assert_eq!(present("<##ff0000>", 0x12ab34)[0], "<##12ab34>");
        assert_eq!(present("<#FF0000>", 0x12ab34)[0], "<#12AB34>");
    }

    /// Skript's own hex spelling first; `&#` only works through other plugins.
    #[test]
    fn presentation_turns_a_legacy_code_into_native_hex() {
        let found = present("&c", 0x12ab34);
        assert_eq!(found[0], "&x&1&2&a&b&3&4");
        assert!(found.contains(&"&#12ab34".to_string()));
    }

    #[test]
    fn presentation_keeps_a_legacy_code_on_a_palette_colour() {
        assert_eq!(present("&c", 0xFF5555)[0], "&c");
        assert_eq!(present("&c", 0xFFAA00)[0], "&6");
        assert_eq!(present("&C", 0x55FFFF)[0], "&B");
    }

    #[test]
    fn presentation_turns_a_named_tag_into_a_hex_tag() {
        assert_eq!(present("<red>", 0x12ab34)[0], "<#12ab34>");
    }

    #[test]
    fn presentation_keeps_a_named_tag_on_a_palette_colour() {
        assert_eq!(present("<dark blue>", 0x0000AA)[0], "<dark blue>");
        assert_eq!(present("<orange>", 0xFFAA00)[0], "<orange>");
        assert_eq!(present("<red>", 0x55FF55)[0], "<green>");
    }

    #[test]
    fn presentation_keeps_color_argument_tags() {
        assert_eq!(present("<color:red>", 0x12ab34)[0], "<color:#12ab34>");
        assert_eq!(present("<color:red>", 0xFFAA00)[0], "<color:gold>");
        assert_eq!(present("<colour:#ff0000>", 0xFFAA00)[0], "<colour:#ffaa00>");
    }

    #[test]
    fn presentations_are_never_empty_and_unique() {
        for original in ["&c", "&#ff0000", "<red>", "<#ff0000>", "", "garbage"] {
            let list = present(original, 0xFF5555);
            assert!(!list.is_empty(), "{original:?}");
            let mut sorted = list.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len(), "{original:?}: {list:?}");
        }
    }

    #[test]
    fn presentation_round_trips_through_the_scanner() {
        for original in [
            "&c",
            "&#ff0000",
            "&x&f&f&0&0&0&0",
            "<red>",
            "<##ff0000>",
            "<c:red>",
        ] {
            for spelling in present(original, 0x3a7bd5) {
                let text = format!("send \"{spelling}\"");
                assert_eq!(one(&text).3, "3a7bd5", "{original:?} -> {spelling:?}");
            }
        }
    }

    #[test]
    fn out_of_range_floats_are_clamped() {
        assert_eq!(
            presentations("<#000000>", 2.0, -1.0, f32::NAN)[0],
            "<#ff0000>"
        );
    }
}
