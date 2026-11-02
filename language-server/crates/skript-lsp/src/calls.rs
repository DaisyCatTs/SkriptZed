//! Reading function calls out of a line of text.
//!
//! Inlay hints, signature help and semantic tokens all need to split
//! `giveKit(p, "a, b", f(1, 2))` into its arguments. Each used to carry its own
//! scanner, and the copies drifted: one ignored braces, so the comma in
//! `{_a, b}` split an argument; one counted `[` as nesting and the others did
//! not. They share this one now.
//!
//! The rules, applied left to right:
//!
//! * a `"` opens or closes a string, and nothing inside a string is structure.
//!   A doubled `""` is an escaped quote, which toggling twice handles for free;
//! * inside `{…}` (a variable name) nothing is structure either, strings
//!   included;
//! * `(` and `)` nest; a `,` at the call's own depth separates arguments.

/// A structural byte found by [`scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Open,
    Close,
    Comma,
}

/// The structural bytes of `text` from `from` onwards, with their offsets.
fn scan(text: &str, from: usize) -> impl Iterator<Item = (usize, Mark)> + '_ {
    let (mut in_string, mut braces) = (false, 0u32);
    text.bytes()
        .enumerate()
        .skip(from)
        .filter_map(move |(index, byte)| {
            match byte {
                b'"' if braces == 0 => in_string = !in_string,
                _ if in_string => {}
                b'{' => braces += 1,
                b'}' => braces = braces.saturating_sub(1),
                _ if braces > 0 => {}
                b'(' => return Some((index, Mark::Open)),
                b')' => return Some((index, Mark::Close)),
                b',' => return Some((index, Mark::Comma)),
                _ => {}
            }
            None
        })
}

/// The arguments of the call whose `(` is at `open`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arguments {
    /// Byte spans in the scanned text, whitespace included, one per argument.
    /// An unclosed call's last span runs to the end of the text.
    pub spans: Vec<(usize, usize)>,
    /// Whether the matching `)` was found. A call still being typed is not.
    pub closed: bool,
}

/// Splits the call whose `(` is at byte `open` into its arguments.
pub fn arguments(text: &str, open: usize) -> Arguments {
    let mut spans = Vec::new();
    let mut depth = 0u32;
    let mut start = open + 1;
    for (index, mark) in scan(text, open + 1) {
        match mark {
            Mark::Open => depth += 1,
            Mark::Close if depth == 0 => {
                spans.push((start, index));
                return Arguments {
                    spans,
                    closed: true,
                };
            }
            Mark::Close => depth -= 1,
            Mark::Comma if depth == 0 => {
                spans.push((start, index));
                start = index + 1;
            }
            Mark::Comma => {}
        }
    }
    spans.push((start.min(text.len()), text.len()));
    Arguments {
        spans,
        closed: false,
    }
}

/// The parameter a named argument (`e: {_e}`) fills, and the byte length of
/// that `name:` prefix with its leading whitespace. A ClassySK `Wallet::limit`
/// is an access, not a name.
pub fn named_argument(argument: &str) -> Option<(&str, usize)> {
    let lead = skript_index::indent_width(argument);
    let rest = &argument[lead..];
    let name_len = rest
        .char_indices()
        .find(|(_, ch)| !(ch.is_alphanumeric() || *ch == '_'))
        .map_or(rest.len(), |(at, _)| at);
    let after = &rest[name_len..];
    (name_len > 0 && after.starts_with(':') && !after.starts_with("::"))
        .then(|| (&rest[..name_len], lead + name_len + 1))
}

/// The identifier ending at byte `end` of `text`, as a start offset.
///
/// Walks characters rather than adding 1 to a byte offset: a multi-byte
/// delimiter such as `§` would otherwise leave the start mid-character and
/// panic on the slice.
fn identifier_start(text: &str, end: usize) -> usize {
    text[..end]
        .char_indices()
        .rev()
        .find(|(_, ch)| !(ch.is_alphanumeric() || *ch == '_'))
        .map_or(0, |(at, ch)| at + ch.len_utf8())
}

/// Byte columns at which each argument of a call starts.
///
/// `after_name` is the column just past the function name, so the `(` must be
/// the next thing on the line apart from whitespace; anything else means this
/// is not the call it was said to be, and nothing is returned. A call still
/// being typed still gets its offsets so far.
pub fn argument_offsets(line: &str, after_name: usize) -> Vec<u32> {
    let rest = line.get(after_name..).unwrap_or("");
    let gap = skript_index::indent_width(rest);
    if !rest[gap..].starts_with('(') {
        return Vec::new();
    }
    let open = after_name + gap;
    arguments(line, open)
        .spans
        .into_iter()
        .filter_map(|(start, end)| {
            let argument = &line[start..end];
            let lead = skript_index::indent_width(argument);
            (lead < argument.len()).then_some((start + lead) as u32)
        })
        .collect()
}

/// The call the cursor is inside, as signature help needs it.
#[derive(Debug, PartialEq, Eq)]
pub struct CallAt {
    pub name: String,
    /// `x::name(` is a ClassySK method rather than a Skript function.
    pub is_member: bool,
    /// Zero-based, by top-level comma count.
    pub argument: u32,
    /// The parameter a named argument (`e: …`) is filling, if this one is.
    pub named: Option<String>,
}

/// The innermost call left open at the end of `prefix`, and which argument
/// the end of `prefix` is in. A call that has already closed is not
/// enclosing, and a `(` with no name in front of it is not a call.
pub fn call_at(prefix: &str) -> Option<CallAt> {
    // Every `(` still open, innermost last, with where its current argument
    // starts and how many commas came before it.
    let mut open: Vec<(usize, usize, u32)> = Vec::new();
    for (index, mark) in scan(prefix, 0) {
        match mark {
            Mark::Open => open.push((index, index + 1, 0)),
            Mark::Close => {
                open.pop();
            }
            Mark::Comma => {
                if let Some(innermost) = open.last_mut() {
                    innermost.1 = index + 1;
                    innermost.2 += 1;
                }
            }
        }
    }
    let &(paren, argument_start, argument) = open.last()?;
    let name_start = identifier_start(prefix, paren);
    let name = &prefix[name_start..paren];
    if name.is_empty() {
        return None;
    }
    Some(CallAt {
        name: name.to_string(),
        is_member: prefix[..name_start].ends_with("::"),
        argument,
        named: named_argument(&prefix[argument_start..]).map(|(name, _)| name.to_string()),
    })
}

/// The parameters of a signature label (`f(a: text, xs: integers = (1, 7)) ::
/// item`), split only at top-level commas, so a default holding a list stays
/// one parameter.
pub fn signature_parameters(label: &str) -> Vec<String> {
    let Some(open) = label.find('(') else {
        return Vec::new();
    };
    let Arguments { mut spans, closed } = arguments(label, open);
    // An unclosed label's last part is not a finished parameter.
    if !closed {
        spans.pop();
    }
    spans
        .into_iter()
        .map(|(start, end)| label[start..end].trim().to_string())
        .filter(|parameter| !parameter.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts<'a>(text: &'a str, arguments: &Arguments) -> Vec<&'a str> {
        arguments
            .spans
            .iter()
            .map(|&(start, end)| &text[start..end])
            .collect()
    }

    #[test]
    fn splits_call_arguments_around_nesting_strings_and_variables() {
        let code = r#"f(p: {_a, b}, g("x, y"), (1, 2))"#;
        let found = arguments(code, 1);
        assert!(found.closed);
        assert_eq!(
            texts(code, &found),
            vec!["p: {_a, b}", r#" g("x, y")"#, " (1, 2)"]
        );
    }

    #[test]
    fn an_unclosed_call_is_reported_as_such() {
        let found = arguments("f(a, b", 1);
        assert!(!found.closed);
        assert_eq!(texts("f(a, b", &found), vec!["a", " b"]);
    }

    #[test]
    fn a_quote_inside_braces_is_part_of_the_name() {
        let code = r#"f({_x::"}, b)"#;
        assert_eq!(texts(code, &arguments(code, 1)), vec![r#"{_x::"}"#, " b"]);
    }

    #[test]
    fn skips_only_a_genuine_named_argument_prefix() {
        assert_eq!(named_argument(" p: {_p}"), Some(("p", 3)));
        assert_eq!(named_argument("player's tool"), None);
        assert_eq!(named_argument("Wallet::limit"), None);
        assert_eq!(named_argument(""), None);
    }

    #[test]
    fn argument_offsets_point_at_each_argument() {
        let line = "\tset {_x} to greet(a, b)";
        // `greet` ends at column 18.
        let offsets = argument_offsets(line, 18);
        assert_eq!(offsets.len(), 2);
        assert_eq!(&line[offsets[0] as usize..offsets[0] as usize + 1], "a");
        assert_eq!(&line[offsets[1] as usize..offsets[1] as usize + 1], "b");
    }

    #[test]
    fn a_comma_inside_a_string_or_a_nested_call_does_not_split() {
        let line = "greet(\"a, b\", f(1, 2), c)";
        let offsets = argument_offsets(line, 5);
        assert_eq!(offsets.len(), 3, "got {offsets:?}");
    }

    #[test]
    fn a_name_not_followed_by_a_paren_yields_nothing() {
        // Guards against hinting on something that only looked like a call.
        assert!(argument_offsets("set {_x} to 5", 3).is_empty());
        assert!(argument_offsets("f", 9).is_empty());
    }

    #[test]
    fn an_empty_or_half_typed_argument_list_hints_what_is_there() {
        assert!(argument_offsets("f()", 1).is_empty());
        assert_eq!(argument_offsets("f(a, ", 1), vec![2]);
        assert_eq!(argument_offsets("f (a)", 1), vec![3]);
    }

    #[test]
    fn finds_the_enclosing_call_and_argument_index() {
        let at = |prefix: &str| call_at(prefix).map(|call| (call.name, call.argument));
        assert_eq!(at("\tset {_x} to greet("), Some(("greet".to_string(), 0)));
        assert_eq!(
            at("\tset {_x} to greet(\"a\", "),
            Some(("greet".to_string(), 1))
        );
        // A closed call is no longer enclosing.
        assert_eq!(at("\tset {_x} to greet(\"a\") "), None);
        // A paren inside a string must not open a call.
        assert_eq!(at("\tsend \"a (b\" "), None);
        // Nested calls report the innermost.
        assert_eq!(
            at("\tset {_x} to outer(inner("),
            Some(("inner".to_string(), 0))
        );
        // Back in the outer call once the inner one closes.
        assert_eq!(
            at("\tset {_x} to outer(inner(1, 2), "),
            Some(("outer".to_string(), 1))
        );
    }

    #[test]
    fn commas_inside_strings_and_braces_do_not_count() {
        let call = call_at(r#"greet("a, b", {_x, y}, "#).expect("inside greet");
        assert_eq!(call.argument, 2);
    }

    #[test]
    fn a_paren_with_no_name_is_not_a_call() {
        assert_eq!(call_at("set {_x} to ("), None);
    }

    #[test]
    fn a_member_call_is_recognised() {
        let call = call_at("\t{_bar}::play(").expect("a call");
        assert_eq!(call.name, "play");
        assert!(call.is_member);
        assert!(!call_at("\tplay(").expect("a call").is_member);
    }

    #[test]
    fn a_named_argument_is_detected_in_the_current_argument_only() {
        let call = call_at("updateEquip(p: {_p}, e: ").expect("a call");
        assert_eq!(call.named.as_deref(), Some("e"));
        assert_eq!(call.argument, 1);

        let positional = call_at("updateEquip(p: {_p}, Wallet::").expect("a call");
        assert_eq!(positional.named, None);
    }

    #[test]
    fn a_multibyte_character_before_the_name_does_not_panic() {
        let call = call_at("send \"§a\" — greet(").expect("a call");
        assert_eq!(call.name, "greet");
    }

    #[test]
    fn signature_parameters_split_only_at_top_level_commas() {
        assert_eq!(
            signature_parameters("f(a: text, xs: integers = (1, 7)) :: item"),
            vec!["a: text", "xs: integers = (1, 7)"]
        );
        assert_eq!(
            signature_parameters(r#"f(s: text = "a, b")"#),
            vec![r#"s: text = "a, b""#]
        );
        assert!(signature_parameters("f()").is_empty());
        assert!(signature_parameters("no parens").is_empty());
        // An unclosed label keeps only its finished parameters.
        assert_eq!(signature_parameters("f(a: text, b"), vec!["a: text"]);
    }
}
