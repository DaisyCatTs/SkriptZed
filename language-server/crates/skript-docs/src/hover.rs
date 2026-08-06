//! Renders a documentation entry as the Markdown that Zed shows on hover.
//!
//! The brief for this extension asked hover to carry description, syntax,
//! parameters, examples, notes, version support and addon support. `docs.json`
//! supplies all of it; this module is only about presenting it without turning
//! the popup into a wall of text.

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
        out.push_str("> ⚠️ **Deprecated.**");
        if let Some(note) = entry.deprecated.note() {
            out.push(' ');
            out.push_str(&note);
        }
        out.push_str("\n\n");
    }

    // ---- description -------------------------------------------------------
    if !entry.description.is_empty() {
        out.push_str(&entry.description.join("\n"));
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
            .map(|value| format!("`event-{}`", value.display()))
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
    if !footer.is_empty() {
        out.push_str("---\n\n");
        out.push_str(&footer.join(" · "));
    }

    out.trim_end().to_string()
}

/// A one-line summary, for completion item details and signature help.
pub fn summary(entry: &Entry) -> String {
    entry
        .description
        .first()
        .map(|line| line.trim().to_string())
        .unwrap_or_default()
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
