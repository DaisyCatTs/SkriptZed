//! Renders a documentation entry as the Markdown that Zed shows on hover.
//!
//! A card carries the description, syntax, event values, examples, version
//! and requirements, and a link to the entry's page. `docs.json` supplies all
//! of it; this module is only about presenting it without turning the popup
//! into a wall of text.

use crate::model::{Category, Entry};

/// Builds the hover card for `entry`.
pub fn render(category: Category, entry: &Entry) -> String {
    let mut out = String::new();

    // ---- title line: name, kind, and a deprecation warning if any ----------
    out.push_str("**");
    out.push_str(if entry.name.is_empty() {
        &entry.id
    } else {
        &entry.name
    });
    out.push_str("**  \n");
    out.push_str(&format!("*{}*", category.label()));

    if let Some(returns) = &entry.returns {
        if !returns.display().is_empty() {
            out.push_str(&format!(" → `{}`", returns.display()));
        }
    }
    out.push_str("\n\n");

    if entry.is_deprecated() {
        out.push_str("> **Deprecated.**");
        if let Some(note) = entry.deprecated.note() {
            out.push(' ');
            out.push_str(&note);
        }
        out.push_str("\n\n");
    }

    // ---- description -------------------------------------------------------
    if !entry.description.is_empty() {
        out.push_str(&html_to_markdown(&entry.description.join("\n")));
        out.push_str("\n\n");
    }

    // ---- syntax ------------------------------------------------------------
    if !entry.patterns.is_empty() {
        out.push_str("**Syntax**\n\n```skript\n");
        for pattern in &entry.patterns {
            out.push_str(pattern);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    // ---- event values ------------------------------------------------------
    // These are what `event-block` and friends resolve to, and they are the
    // single most looked-up thing about an event.
    if !entry.event_values.is_empty() {
        out.push_str("**Event values**\n\n");
        let values: Vec<String> = entry
            .event_values
            .iter()
            // SkriptHub records already carry the prefix (`event-player`);
            // Skript's own docs do not (`player`).
            .map(|value| {
                let name = value.display();
                format!("`event-{}`", name.strip_prefix("event-").unwrap_or(name))
            })
            .collect();
        out.push_str(&values.join(", "));
        out.push_str("\n\n");
    }

    if entry.cancellable {
        out.push_str("Cancellable with `cancel event`.\n\n");
    }

    // ---- restricted to certain events --------------------------------------
    if !entry.events.is_empty() {
        let names: Vec<&str> = entry.events.iter().map(|e| e.display()).collect();
        out.push_str(&format!("**Only in**: {}\n\n", names.join(", ")));
    }

    // ---- examples ----------------------------------------------------------
    // Capped: some entries ship a dozen, and a hover popup is not a manual.
    if !entry.examples.is_empty() {
        out.push_str("**Example**\n\n```skript\n");
        for example in entry.examples.iter().take(2) {
            out.push_str(example.trim_end());
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    // ---- provenance footer -------------------------------------------------
    let mut footer = Vec::new();
    if let Some(since) = entry.since.first() {
        footer.push(format!("since {since}"));
    }
    if !entry.requirements.is_empty() {
        footer.push(format!("requires {}", entry.requirements.join(", ")));
    }
    if !entry.id.is_empty() {
        footer.push(format!("`{}`", entry.id));
    }
    if let Some(page) = docs_page(category, entry) {
        footer.push(format!("[View on docs.skriptlang.org]({page})"));
    }
    if !footer.is_empty() {
        out.push_str("---\n\n");
        out.push_str(&footer.join(" · "));
    }

    out.trim_end().to_string()
}

/// The entry's section on docs.skriptlang.org, for core Skript entries.
///
/// Each category has one page, and each entry is anchored by its `docs.json`
/// id. Checked against the live site for every id in Skript 2.17.1's eight
/// paged categories, with none missing; types live on `classes.html`.
/// Properties and experiments have no page (both 404), and an addon's syntax
/// is not on Skript's site at all, so those get no link.
pub fn docs_page(category: Category, entry: &Entry) -> Option<String> {
    if entry.id.is_empty() || entry.addon.is_some() {
        return None;
    }
    let page = match category {
        Category::Effect => "effects",
        Category::Condition => "conditions",
        Category::Expression => "expressions",
        Category::Event => "events",
        Category::Structure => "structures",
        Category::Section => "sections",
        Category::Type => "classes",
        Category::Function => "functions",
        Category::Property | Category::Experiment => return None,
    };
    Some(format!(
        "{DOCS_SITE}{page}.html#{}",
        entry.id.replace(' ', "%20")
    ))
}

/// A one-line summary, for completion item details and signature help.
pub fn summary(entry: &Entry) -> String {
    entry
        .description
        .first()
        .map(|line| html_to_markdown(line).trim().to_string())
        .unwrap_or_default()
}

/// Where a relative link in Skript's own documentation points.
const DOCS_SITE: &str = "https://docs.skriptlang.org/";

/// Descriptions in `docs.json` are written as HTML for the docs site — 571
/// links, `<code>`, `<ul><li>` lists — and Zed renders hover Markdown, so the
/// tags showed up raw.
///
/// Only real HTML tags are rewritten. Descriptions also quote Skript itself —
/// `<red>`, `<player>` — and those must survive untouched, so anything not
/// recognised stays exactly as written.
pub fn html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut link_target: Option<String> = None;
    let mut rest = html;

    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find('>') else {
            out.push_str(after);
            rest = "";
            break;
        };
        let tag = &after[1..close];
        let (closing, body) = match tag.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, tag),
        };
        let name = body
            .split(|ch: char| ch.is_whitespace() || ch == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        let replacement: Option<String> = match (name.as_str(), closing) {
            ("a", false) => {
                link_target = href(body);
                Some(if link_target.is_some() {
                    "[".into()
                } else {
                    String::new()
                })
            }
            ("a", true) => Some(match link_target.take() {
                Some(target) => format!("]({target})"),
                None => String::new(),
            }),
            ("code", _) => Some("`".into()),
            ("strong" | "b", _) => Some("**".into()),
            ("em" | "i", _) => Some("*".into()),
            ("br", _) => Some("  \n".into()),
            ("ul" | "ol", _) => Some("\n".into()),
            ("li", false) => Some("\n- ".into()),
            ("p", false) => Some("\n\n".into()),
            ("li", true) | ("p" | "span" | "div", _) => Some(String::new()),
            _ => None,
        };
        match replacement {
            Some(text) => out.push_str(&text),
            None => out.push_str(&after[..=close]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);

    // Entities last, so an escaped `&lt;b&gt;` stays text rather than a tag.
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    // A list rewritten from `<ul>\n<li>` leaves blank lines between items and
    // runs of them around the list. Trailing spaces are kept: two of them are
    // Markdown's line break, which is what `<br>` became.
    let mut lines: Vec<&str> = Vec::new();
    for line in decoded.lines() {
        let blank = line.trim().is_empty();
        let previous_blank = lines.last().is_some_and(|last| last.trim().is_empty());
        if blank && (previous_blank || lines.is_empty()) {
            continue;
        }
        let is_item = line.trim_start().starts_with("- ");
        if is_item && previous_blank && lines.len() >= 2 {
            let before = lines[lines.len() - 2].trim_start();
            if before.starts_with("- ") {
                lines.pop();
            }
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// The absolute target of an `<a …>` tag, or `None` for an in-page anchor,
/// which has nowhere to go from an editor.
fn href(tag: &str) -> Option<String> {
    let start = tag.find("href=")? + "href=".len();
    let quoted = &tag[start..];
    let quote = quoted
        .chars()
        .next()
        .filter(|ch| *ch == '\'' || *ch == '"')?;
    let value = &quoted[1..];
    let value = &value[..value.find(quote)?];
    if value.starts_with("http://") || value.starts_with("https://") {
        Some(value.to_string())
    } else if value.starts_with('#') || value.is_empty() {
        None
    } else {
        Some(format!("{DOCS_SITE}{}", value.trim_start_matches("./")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Deprecated, Reference, StringOrVec};

    fn entry() -> Entry {
        Entry {
            id: "EffGive".into(),
            name: "Give".into(),
            since: StringOrVec::One("1.0".into()),
            description: vec!["Gives the specified items to a player.".into()],
            patterns: vec!["give %item types% to %players%".into()],
            examples: vec!["give a diamond to player".into()],
            ..Default::default()
        }
    }

    #[test]
    fn rewrites_the_html_docs_json_descriptions_use() {
        let html = "Changes many <a href='./expressions'>expressions</a>. Use <code>set</code>:<ul>\n<li>Horses: <b>armour</b></li>\n<li>Wolves</li>\n</ul>\nDone &amp; dusted<br>next";
        let markdown = html_to_markdown(html);
        assert!(
            markdown.contains("[expressions](https://docs.skriptlang.org/expressions)"),
            "{markdown}"
        );
        assert!(markdown.contains("`set`"));
        assert!(markdown.contains("- Horses: **armour**"), "{markdown}");
        assert!(markdown.contains("- Wolves"));
        assert!(markdown.contains("Done & dusted  \nnext"), "{markdown}");
        assert!(!markdown.contains("<li>") && !markdown.contains("<ul>"));
    }

    /// Skript's own tags are quoted in descriptions and are not HTML.
    #[test]
    fn leaves_skript_tags_and_anchors_alone() {
        assert_eq!(
            html_to_markdown("send \"<red>hi\" to <player>"),
            "send \"<red>hi\" to <player>"
        );
        assert_eq!(
            html_to_markdown("see <a href='#EffHealth'>heal</a>"),
            "see heal"
        );
    }

    #[test]
    fn renders_the_essentials() {
        let text = render(Category::Effect, &entry());
        assert!(text.contains("**Give**"));
        assert!(text.contains("*effect*"));
        assert!(text.contains("Gives the specified items"));
        assert!(text.contains("give %item types% to %players%"));
        assert!(text.contains("since 1.0"));
        assert!(text.contains("`EffGive`"));
    }

    #[test]
    fn links_core_entries_to_their_docs_page() {
        let text = render(Category::Effect, &entry());
        assert!(
            text.contains(
                "[View on docs.skriptlang.org](https://docs.skriptlang.org/effects.html#EffGive)"
            ),
            "{text}"
        );

        let mut kind = entry();
        kind.id = "itemtype".into();
        assert_eq!(
            docs_page(Category::Type, &kind).as_deref(),
            Some("https://docs.skriptlang.org/classes.html#itemtype")
        );
        assert!(docs_page(Category::Experiment, &kind).is_none());
        assert!(docs_page(Category::Property, &kind).is_none());

        kind.id.clear();
        assert!(docs_page(Category::Effect, &kind).is_none());
    }

    #[test]
    fn addon_entries_get_no_skript_docs_link() {
        let mut addon = entry();
        addon.addon = Some(crate::model::AddonRef {
            name: "SkBee".into(),
            url: String::new(),
            since_version: None,
        });
        assert!(docs_page(Category::Effect, &addon).is_none());
        assert!(!render(Category::Effect, &addon).contains("docs.skriptlang.org"));
    }

    #[test]
    fn flags_deprecation_prominently() {
        let mut entry = entry();
        entry.deprecated = Deprecated::Note("use `give` instead".into());
        let text = render(Category::Effect, &entry);
        assert!(text.contains("Deprecated"));
        assert!(text.contains("use `give` instead"));
    }

    #[test]
    fn shows_event_values_and_cancellability() {
        let mut entry = entry();
        entry.cancellable = true;
        entry.event_values = vec![Reference {
            id: "block".into(),
            name: "block".into(),
        }];
        let text = render(Category::Event, &entry);
        assert!(text.contains("`event-block`"));
        assert!(text.contains("cancel event"));
    }

    #[test]
    fn survives_a_completely_empty_entry() {
        // A malformed or partial addon dump must still render something.
        let text = render(Category::Expression, &Entry::default());
        assert!(text.contains("*expression*"));
    }

    #[test]
    fn caps_the_number_of_examples() {
        let mut entry = entry();
        entry.examples = (0..10).map(|i| format!("line {i}")).collect();
        let text = render(Category::Effect, &entry);
        assert!(text.contains("line 0"));
        assert!(text.contains("line 1"));
        assert!(!text.contains("line 2"));
    }
}
