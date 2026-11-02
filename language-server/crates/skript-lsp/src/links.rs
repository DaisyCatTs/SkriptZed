//! Document links — the URLs in a script, made clickable.
//!
//! Two sources, both purely lexical:
//!
//! * **Format tags** that carry a click-to-open URL inside a string, such as
//!   `<link:https://…>`. Skript's other click tags (`cmd`, `sgt`,
//!   `insertion`, …) carry commands or chat text, never a URL, so they are not
//!   linked even though `highlights.scm` colours them the same way.
//! * **Bare `http(s)://` URLs** anywhere on a line, comments included, since
//!   that is where people put docs links.
//!
//! Only `http` and `https` targets are ever produced. Script text is untrusted,
//! and a `file:` or custom-scheme link is one click away from opening
//! something local.
//!
//! This works on raw lines rather than the tree: a URL in a comment, a string
//! or an `options:` value is linked the same way, and a half-typed line that
//! fails to parse still gets its links.

/// One link, positioned within a single line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundLink {
    pub line: u32,
    /// Byte columns within the line, start inclusive, end exclusive, covering just the URL.
    pub start: u32,
    pub end: u32,
    pub target: String,
}

/// Tag names whose value is a URL, matched case-insensitively and followed by
/// `:`. `open url` with a space is Skript's own spelling; `click:open_url` is
/// the MiniMessage form.
const URL_TAGS: &[&str] = &["click:open_url", "open_url", "open url", "link", "url"];

const SCHEMES: &[&str] = &["https://", "http://"];

/// Punctuation that usually ends the sentence around a URL rather than the URL
/// itself. `)` and `]` are handled separately, because they can be balanced.
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', '\''];

/// Every link in `text`, in document order.
pub fn document_links(text: &str) -> Vec<FoundLink> {
    let mut links = Vec::new();
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        links.extend(line_links(line, index as u32));
    }
    links
}

fn line_links(line: &str, line_number: u32) -> Vec<FoundLink> {
    let tags = tag_links(line);
    let mut spans: Vec<(usize, usize)> = tags.clone();

    // A tag's URL would also be found by the bare scan, with the same range.
    // Skipping anything that starts inside a tag keeps it to one link.
    let mut from = 0;
    while let Some(start) = find_scheme(line, from) {
        if let Some(&(_, tag_end)) = tags.iter().find(|(s, e)| (*s..*e).contains(&start)) {
            from = tag_end;
            continue;
        }
        match bare_url_end(line, start) {
            Some(end) => {
                spans.push((start, end));
                from = end;
            }
            None => from = start + 1,
        }
    }

    spans.sort_unstable();
    spans
        .into_iter()
        .map(|(start, end)| FoundLink {
            line: line_number,
            start: start as u32,
            end: end as u32,
            target: line[start..end].to_string(),
        })
        .collect()
}

/// Byte ranges of the URL values of URL-bearing format tags.
///
/// The body rule mirrors the grammar's `format_tag`: everything up to `>`, and
/// never `<`, `"` or `%`. A tag containing `%` is not a tag to the grammar
/// either — its URL is built at runtime — so it is left to the bare scan, which
/// stops at the interpolation.
fn tag_links(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let mut open = 0;
    while let Some(offset) = line[open..].find('<') {
        let body_start = open + offset + 1;
        let body_len = line[body_start..]
            .find(['<', '>', '"', '%'])
            .filter(|&len| bytes[body_start + len] == b'>');
        let Some(body_len) = body_len else {
            open = body_start;
            continue;
        };
        let body = &line[body_start..body_start + body_len];
        if let Some(value_offset) = url_tag_value_offset(body) {
            let value = &body[value_offset..];
            if is_web_url(value) && !value.contains(char::is_whitespace) {
                let start = body_start + value_offset;
                spans.push((start, start + value.len()));
            }
        }
        open = body_start + body_len + 1;
    }
    spans
}

/// Where the value starts within a tag body, if the tag is a URL-bearing one.
fn url_tag_value_offset(body: &str) -> Option<usize> {
    // Compared as bytes: slicing the body as a `str` at a tag name's length
    // could land inside a multi-byte character.
    let bytes = body.as_bytes();
    URL_TAGS.iter().find_map(|name| {
        let name_bytes = bytes.get(..name.len())?;
        let matches = name_bytes.eq_ignore_ascii_case(name.as_bytes())
            && bytes.get(name.len()) == Some(&b':');
        matches.then_some(name.len() + 1)
    })
}

/// The next byte offset at or after `from` where an `http://` or `https://`
/// scheme begins as a word of its own.
fn find_scheme(line: &str, from: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    (from..bytes.len()).find(|&index| {
        let starts_scheme = SCHEMES.iter().any(|scheme| {
            bytes
                .get(index..index + scheme.len())
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(scheme.as_bytes()))
        });
        // `xhttps://` is not a URL someone meant to write.
        let word_start = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
        starts_scheme && word_start
    })
}

/// Where a bare URL starting at `start` ends, or `None` if it has no host.
fn bare_url_end(line: &str, start: usize) -> Option<usize> {
    let rest = &line[start..];
    let mut end = rest.len();
    for (offset, c) in rest.char_indices() {
        if ends_bare_url(c, &rest[offset..]) {
            end = offset;
            break;
        }
    }
    let url = trim_trailing(&rest[..end]);
    is_web_url(url).then_some(start + url.len())
}

/// Whether `c`, the first character of `tail`, terminates a bare URL.
///
/// `%` normally opens a Skript interpolation, which is no part of the target.
/// A `%` followed by two hex digits is percent-encoding instead and stays.
fn ends_bare_url(c: char, tail: &str) -> bool {
    match c {
        '"' | '<' | '>' => true,
        '%' => !tail
            .as_bytes()
            .get(1..3)
            .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)),
        _ => c.is_whitespace(),
    }
}

/// Strips sentence punctuation off the end of `url`, keeping a closing bracket
/// that balances an opening one inside it (`…/Foo_(bar)`).
fn trim_trailing(mut url: &str) -> &str {
    loop {
        let Some(last) = url.chars().last() else {
            return url;
        };
        let strip = match last {
            ')' => url.matches('(').count() < url.matches(')').count(),
            ']' => url.matches('[').count() < url.matches(']').count(),
            _ => TRAILING_PUNCTUATION.contains(&last),
        };
        if !strip {
            return url;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
}

/// `http(s)://` followed by a non-empty host.
fn is_web_url(value: &str) -> bool {
    SCHEMES.iter().any(|scheme| {
        let Some(head) = value.get(..scheme.len()) else {
            return false;
        };
        let host = value[scheme.len()..].split(['/', '?', '#']).next();
        head.eq_ignore_ascii_case(scheme) && host.is_some_and(|host| !host.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(line: u32, start: u32, end: u32, target: &str) -> FoundLink {
        FoundLink {
            line,
            start,
            end,
            target: target.to_string(),
        }
    }

    /// The targets alone, for tests that only care what was linked.
    fn targets(text: &str) -> Vec<String> {
        document_links(text).into_iter().map(|l| l.target).collect()
    }

    /// Every link's range must slice exactly its target out of its line.
    fn assert_ranges_slice_targets(text: &str) {
        let lines: Vec<&str> = text.split('\n').collect();
        for found in document_links(text) {
            let line = lines[found.line as usize].trim_end_matches('\r');
            assert_eq!(
                &line[found.start as usize..found.end as usize],
                found.target
            );
        }
    }

    #[test]
    fn empty_text_has_no_links() {
        assert!(document_links("").is_empty());
        assert!(document_links("\n\r\n\n").is_empty());
    }

    #[test]
    fn plain_script_has_no_links() {
        let text = "on join:\n\tsend \"<red>Welcome %player%\" to player\n";
        assert!(document_links(text).is_empty());
    }

    #[test]
    fn link_tag_covers_only_the_url() {
        let text = "send \"<link:https://example.com/a>Click\"";
        let start = text.find("https").unwrap() as u32;
        assert_eq!(
            document_links(text),
            vec![link(0, start, start + 21, "https://example.com/a")]
        );
    }

    #[test]
    fn every_url_bearing_tag_name_is_linked() {
        for name in [
            "link",
            "url",
            "open_url",
            "open url",
            "click:open_url",
            "LINK",
            "Open_Url",
        ] {
            let text = format!("send \"<{name}:https://example.com>hi\"");
            assert_eq!(targets(&text), vec!["https://example.com"], "tag {name}");
        }
    }

    #[test]
    fn tag_url_keeps_trailing_punctuation() {
        // The tag delimits the URL explicitly, so nothing is second-guessed.
        let text = "send \"<link:https://example.com/a.>x\"";
        assert_eq!(targets(text), vec!["https://example.com/a."]);
    }

    #[test]
    fn command_bearing_tags_are_not_linked() {
        for name in [
            "cmd",
            "sgt",
            "suggest_command",
            "run_command",
            "insertion",
            "tooltip",
        ] {
            let text = format!("send \"<{name}:/spawn>hi\"");
            assert!(document_links(&text).is_empty(), "tag {name}");
        }
    }

    #[test]
    fn url_inside_a_command_tag_is_still_a_bare_url() {
        let text = "send \"<sgt:see https://example.com>hi\"";
        assert_eq!(targets(text), vec!["https://example.com"]);
    }

    #[test]
    fn non_web_schemes_in_tags_are_ignored() {
        for value in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com",
            "example.com",
            "https://",
        ] {
            let text = format!("send \"<link:{value}>x\"");
            assert!(document_links(&text).is_empty(), "value {value}");
        }
    }

    #[test]
    fn tag_with_interpolation_links_the_static_prefix() {
        let text = "send \"<link:https://example.com/u/%player%>profile\"";
        assert_eq!(targets(text), vec!["https://example.com/u/"]);
    }

    #[test]
    fn tag_url_is_not_linked_twice() {
        let text = "send \"<url:https://example.com>https://other.org\"";
        assert_eq!(
            targets(text),
            vec!["https://example.com", "https://other.org"]
        );
    }

    #[test]
    fn closing_and_colour_tags_are_ignored() {
        let text = "send \"<link>x</link><#FF00AA><url>\"";
        assert!(document_links(text).is_empty());
    }

    #[test]
    fn bare_url_in_comment() {
        let text = "# Docs: https://docs.skriptlang.org/effects.html";
        let start = text.find("https").unwrap() as u32;
        assert_eq!(
            document_links(text),
            vec![link(
                0,
                start,
                text.len() as u32,
                "https://docs.skriptlang.org/effects.html"
            )]
        );
    }

    #[test]
    fn bare_url_ends_at_whitespace_quote_and_angle_brackets() {
        assert_eq!(targets("https://a.com/x y"), vec!["https://a.com/x"]);
        assert_eq!(targets("\"https://a.com/x\""), vec!["https://a.com/x"]);
        assert_eq!(targets("<https://a.com/x>"), vec!["https://a.com/x"]);
        assert_eq!(targets("https://a.com/x<b>"), vec!["https://a.com/x"]);
        assert_eq!(targets("https://a.com/x\ty"), vec!["https://a.com/x"]);
    }

    #[test]
    fn trailing_sentence_punctuation_is_stripped() {
        for suffix in [".", ",", ";", ":", "!", "?", "'", ").", "]", "...", "),"] {
            let text = format!("see https://a.com/x{suffix}");
            assert_eq!(targets(&text), vec!["https://a.com/x"], "suffix {suffix:?}");
        }
    }

    #[test]
    fn inner_punctuation_is_kept() {
        assert_eq!(
            targets("https://a.com/x.y?q=1,2;3:4#frag"),
            vec!["https://a.com/x.y?q=1,2;3:4#frag"]
        );
    }

    #[test]
    fn balanced_brackets_are_kept() {
        assert_eq!(
            targets("see https://en.wikipedia.org/wiki/Skript_(language)."),
            vec!["https://en.wikipedia.org/wiki/Skript_(language)"]
        );
        assert_eq!(
            targets("(see https://en.wikipedia.org/wiki/Skript_(language))"),
            vec!["https://en.wikipedia.org/wiki/Skript_(language)"]
        );
        assert_eq!(
            targets("[https://a.com/list[0]]"),
            vec!["https://a.com/list[0]"]
        );
    }

    #[test]
    fn unbalanced_closing_bracket_is_stripped() {
        assert_eq!(targets("(https://a.com/x)"), vec!["https://a.com/x"]);
    }

    #[test]
    fn interpolation_ends_a_bare_url() {
        let text = "send \"Visit https://a.com/u/%player%\"";
        assert_eq!(targets(text), vec!["https://a.com/u/"]);
    }

    #[test]
    fn percent_encoding_is_kept() {
        assert_eq!(
            targets("# https://a.com/a%20b%2Fc"),
            vec!["https://a.com/a%20b%2Fc"]
        );
    }

    #[test]
    fn http_and_mixed_case_schemes() {
        assert_eq!(targets("http://a.com"), vec!["http://a.com"]);
        assert_eq!(targets("HTTPS://A.com/X"), vec!["HTTPS://A.com/X"]);
    }

    #[test]
    fn scheme_without_host_is_ignored() {
        assert!(document_links("https://").is_empty());
        assert!(document_links("https://.").is_empty());
        assert!(document_links("https:///path").is_empty());
        assert!(document_links("https:// a.com").is_empty());
    }

    #[test]
    fn other_schemes_are_never_linked() {
        assert!(document_links("# file:///C:/server/plugins").is_empty());
        assert!(document_links("# ftp://a.com mailto:a@b.com").is_empty());
    }

    #[test]
    fn scheme_glued_to_a_word_is_ignored() {
        assert!(document_links("xhttps://a.com").is_empty());
        assert_eq!(targets("(https://a.com"), vec!["https://a.com"]);
        assert_eq!(targets("=https://a.com"), vec!["https://a.com"]);
    }

    #[test]
    fn several_urls_on_one_line_in_order() {
        let text = "# https://a.com and http://b.org/x, also https://c.net.";
        assert_eq!(
            targets(text),
            vec!["https://a.com", "http://b.org/x", "https://c.net"]
        );
    }

    #[test]
    fn links_are_sorted_when_tag_follows_bare_url() {
        let text = "send \"https://a.com <link:https://b.com>x\"";
        let found = document_links(text);
        assert_eq!(found.len(), 2);
        assert!(found[0].start < found[1].start);
        assert_eq!(found[0].target, "https://a.com");
        assert_eq!(found[1].target, "https://b.com");
    }

    #[test]
    fn line_numbers_and_crlf() {
        let text = "on load:\r\n\t# https://a.com\r\n\tsend \"<link:https://b.com>x\"\r\n";
        let found = document_links(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], link(1, 3, 16, "https://a.com"));
        assert_eq!(found[1].line, 2);
        assert_eq!(found[1].target, "https://b.com");
        assert_ranges_slice_targets(text);
    }

    #[test]
    fn crlf_does_not_leak_into_a_line_ending_url() {
        assert_eq!(targets("# https://a.com\r\n"), vec!["https://a.com"]);
    }

    #[test]
    fn columns_are_bytes_after_multibyte_characters() {
        let text = "# ünïcödé → https://a.com/é";
        let start = text.find("https").unwrap() as u32;
        let found = document_links(text);
        assert_eq!(
            found,
            vec![link(0, start, text.len() as u32, "https://a.com/é")]
        );
        assert_ranges_slice_targets(text);
    }

    #[test]
    fn multibyte_after_url_with_trailing_punctuation() {
        let text = "« https://a.com/x. »";
        assert_eq!(targets(text), vec!["https://a.com/x"]);
        assert_ranges_slice_targets(text);
    }

    #[test]
    fn unicode_whitespace_ends_a_url() {
        // U+00A0 no-break space.
        assert_eq!(targets("https://a.com/x\u{a0}y"), vec!["https://a.com/x"]);
    }

    #[test]
    fn multibyte_tag_names_do_not_panic() {
        assert!(document_links("send \"<linké:x><urлl><é>\"").is_empty());
    }

    #[test]
    fn unterminated_tag_falls_back_to_bare_scan() {
        let text = "send \"<link:https://a.com";
        assert_eq!(targets(text), vec!["https://a.com"]);
    }

    #[test]
    fn tag_with_whitespace_in_value_falls_back_to_bare_scan() {
        let text = "send \"<link:https://a.com/x y>z\"";
        assert_eq!(targets(text), vec!["https://a.com/x"]);
    }

    #[test]
    fn realistic_script() {
        let text = "\
# Rules: https://example.com/rules (read them!)
command /discord:
\ttrigger:
\t\tsend \"<link:https://discord.gg/abc><u>Join us</u></link>\" to player
\t\tsend \"<cmd:/spawn>Spawn\" to player
";
        let found = document_links(text);
        assert_eq!(
            found
                .iter()
                .map(|l| (l.line, l.target.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (0, "https://example.com/rules"),
                (3, "https://discord.gg/abc"),
            ]
        );
        assert_ranges_slice_targets(text);
    }
}
