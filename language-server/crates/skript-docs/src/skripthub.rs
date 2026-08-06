//! The SkriptHub addon syntax catalog.
//!
//! <https://skripthub.net/api/v1/addonsyntaxlist/> — keyless, 8,210 entries
//! across 168 addons, 12,877 individual patterns, 7.3 MB (1.2 MB gzipped).
//! It is the only comprehensive machine-readable source of addon syntax: no
//! major addon publishes its own, and `/sk gen-docs` covers Skript alone.
//!
//! There is **no per-addon endpoint and no pagination** — every query parameter
//! is silently ignored — so the whole file is fetched and filtered here.
//!
//! The schema is not Skript's. Differences that matter:
//!
//! * `syntax_pattern` packs several patterns into **one string**, separated by
//!   `\n` *or* `\r\n` (older records use CRLF). 3,366 entries do this.
//! * `description`, `keywords`, `entries`, `return_type` and `event_values` are
//!   plain strings, not arrays.
//! * There is no `since` — SkriptHub tracks the *addon* version a syntax
//!   appeared in (`compatible_addon_version`), never the Skript version. Entries
//!   converted here therefore carry no `min_version` and are treated as
//!   available in every Skript version, which is the honest default.
//! * `mark_as_removed` and `removed_since` exist in the schema but are **never
//!   populated** on any of the 8,210 records, so they are ignored.

use serde::Deserialize;

use crate::model::{AddonRef, Category, Docs, Entry, Reference, StringOrVec};

/// One SkriptHub syntax record.
#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// One or more patterns, newline-separated.
    #[serde(default)]
    pub syntax_pattern: Option<String>,
    #[serde(default)]
    pub syntax_type: String,
    /// The addon version this syntax first appeared in.
    #[serde(default)]
    pub compatible_addon_version: Option<String>,
    #[serde(default)]
    pub compatible_minecraft_version: Option<String>,
    #[serde(default)]
    pub required_plugins: Vec<RequiredPlugin>,
    #[serde(default)]
    pub addon: Option<AddonInfo>,
    #[serde(default)]
    pub return_type: Option<String>,
    #[serde(default)]
    pub event_values: Option<String>,
    /// A namespaced id such as `skbee:effect:open_real_inventory`, or a bare
    /// Java class name such as `ExprVersion` on older records.
    #[serde(default)]
    pub json_id: Option<String>,
    #[serde(default)]
    pub event_cancellable: bool,
    #[serde(default)]
    pub keywords: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequiredPlugin {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddonInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub link_to_addon: String,
    /// SkriptHub's popularity score. Used only to rank completion.
    #[serde(default)]
    pub usage_score: f64,
}

/// Maps SkriptHub's `syntax_type` onto our categories.
fn category_of(syntax_type: &str) -> Option<Category> {
    Some(match syntax_type.trim().to_ascii_lowercase().as_str() {
        "expression" => Category::Expression,
        "effect" => Category::Effect,
        "condition" => Category::Condition,
        "event" => Category::Event,
        "section" => Category::Section,
        "structure" => Category::Structure,
        "type" => Category::Type,
        "function" => Category::Function,
        _ => return None,
    })
}

/// Splits SkriptHub's packed pattern string.
///
/// Both `\n` and `\r\n` occur, and blank lines are common padding.
fn split_patterns(packed: &str) -> Vec<String> {
    packed
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

impl Record {
    /// A stable identifier, preferring SkriptHub's own.
    fn entry_id(&self) -> String {
        match &self.json_id {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            // Namespaced by source so a synthesised id can never collide with a
            // real Skript one.
            _ => format!("skripthub:{}", self.id),
        }
    }

    fn into_entry(self) -> Option<(Category, Entry)> {
        let category = category_of(&self.syntax_type)?;
        let patterns = split_patterns(self.syntax_pattern.as_deref().unwrap_or_default());
        if patterns.is_empty() {
            return None;
        }

        // `requirements` collects everything the syntax needs beyond its own
        // addon: other plugins, and a minimum Minecraft version when stated.
        let mut requirements: Vec<String> = self
            .required_plugins
            .iter()
            .map(|plugin| plugin.name.clone())
            .filter(|name| !name.is_empty())
            .collect();
        if let Some(mc) = self.compatible_minecraft_version.as_deref() {
            if !mc.trim().is_empty() {
                requirements.push(format!("Minecraft {}", mc.trim()));
            }
        }

        let addon = self.addon.as_ref().map(|info| AddonRef {
            name: info.name.clone(),
            url: info.link_to_addon.clone(),
            since_version: self
                .compatible_addon_version
                .as_deref()
                .map(str::trim)
                .filter(|version| !version.is_empty())
                .map(str::to_string),
        });

        let returns = self
            .return_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Reference {
                id: value.to_string(),
                name: value.to_string(),
            });

        // A comma-separated string here, unlike Skript's array of references.
        let event_values = self
            .event_values
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Reference {
                id: value.to_string(),
                name: value.to_string(),
            })
            .collect();

        let keywords = self
            .keywords
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();

        let entry = Entry {
            id: self.entry_id(),
            name: self.title.clone(),
            // SkriptHub carries no Skript `since`, so this stays absent and the
            // entry is never treated as needing a newer Skript.
            since: StringOrVec::Absent,
            description: self
                .description
                .as_deref()
                .map(|text| vec![text.to_string()])
                .unwrap_or_default(),
            patterns,
            requirements,
            returns,
            event_values,
            cancellable: self.event_cancellable,
            keywords,
            addon,
            ..Default::default()
        };

        Some((category, entry))
    }
}

/// The result of loading the SkriptHub catalog.
pub struct Catalog {
    pub docs: Docs,
    /// Records skipped because their `syntax_type` is unknown to us.
    pub skipped: usize,
}

/// Parses the raw SkriptHub response into our model.
///
/// `keep` decides which addons are loaded. Loading all 168 costs ~12,900
/// patterns and fills completion with syntax for plugins the user does not run,
/// so callers normally pass the set they actually detected.
pub fn parse_filtered(
    json: &str,
    keep: impl Fn(&str) -> bool,
) -> Result<Catalog, serde_json::Error> {
    let records: Vec<Record> = serde_json::from_str(json)?;
    let mut docs = Docs::default();
    let mut skipped = 0usize;

    for record in records {
        let addon_name = record
            .addon
            .as_ref()
            .map(|info| info.name.clone())
            .unwrap_or_default();
        if !keep(&addon_name) {
            continue;
        }
        match record.into_entry() {
            Some((category, entry)) => docs.push(category, entry),
            None => skipped += 1,
        }
    }

    docs.source.name = "SkriptHub".to_string();
    Ok(Catalog { docs, skipped })
}

/// Every addon named in the catalog, with its popularity score.
pub fn addons(json: &str) -> Result<Vec<(String, f64)>, serde_json::Error> {
    let records: Vec<Record> = serde_json::from_str(json)?;
    let mut seen: Vec<(String, f64)> = Vec::new();
    for record in records {
        let Some(info) = record.addon else { continue };
        if info.name.is_empty() || seen.iter().any(|(name, _)| *name == info.name) {
            continue;
        }
        seen.push((info.name, info.usage_score));
    }
    seen.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real record, copied verbatim from the live API.
    const SKBEE: &str = r#"[{
        "id": 11919,
        "creator": 1,
        "title": "Open Real Inventory",
        "description": "Open real inventories to players.",
        "syntax_pattern": "open real anvil [inventory] to %players%\nopen real loom [inventory] to %players%",
        "compatible_addon_version": "3.6.0",
        "compatible_minecraft_version": "",
        "syntax_type": "effect",
        "required_plugins": [],
        "addon": {
            "name": "SkBee",
            "link_to_addon": "https://github.com/ShaneBeee/SkBee/",
            "usage_score": 534.8
        },
        "return_type": "",
        "event_values": "",
        "json_id": "skbee:effect:open_real_inventory",
        "event_cancellable": false,
        "entries": "",
        "keywords": "",
        "mark_as_removed": false,
        "removed_since": null
    }]"#;

    #[test]
    fn converts_a_real_record() {
        let catalog = parse_filtered(SKBEE, |_| true).unwrap();
        assert_eq!(catalog.docs.effects.len(), 1);

        let entry = &catalog.docs.effects[0];
        assert_eq!(entry.name, "Open Real Inventory");
        assert_eq!(entry.id, "skbee:effect:open_real_inventory");
        assert_eq!(entry.description, vec!["Open real inventories to players."]);

        let addon = entry.addon.as_ref().expect("addon should be attached");
        assert_eq!(addon.name, "SkBee");
        assert_eq!(addon.since_version.as_deref(), Some("3.6.0"));
        assert_eq!(entry.provider(), "SkBee");
    }

    #[test]
    fn splits_the_packed_pattern_string() {
        let catalog = parse_filtered(SKBEE, |_| true).unwrap();
        assert_eq!(catalog.docs.effects[0].patterns.len(), 2);
    }

    #[test]
    fn splits_on_crlf_too() {
        // Older records use CRLF; splitting on '\n' alone leaves a stray '\r'
        // on every pattern, which would break literal matching.
        let patterns = split_patterns("first %player%\r\nsecond %player%\r\n");
        assert_eq!(patterns, vec!["first %player%", "second %player%"]);
    }

    #[test]
    fn drops_blank_padding_lines() {
        assert_eq!(split_patterns("a\n\n\nb\n"), vec!["a", "b"]);
    }

    #[test]
    fn filters_by_addon() {
        let kept = parse_filtered(SKBEE, |name| name == "SkBee").unwrap();
        assert_eq!(kept.docs.effects.len(), 1);

        let dropped = parse_filtered(SKBEE, |name| name == "SomethingElse").unwrap();
        assert!(dropped.docs.effects.is_empty());
    }

    #[test]
    fn carries_no_skript_version() {
        // SkriptHub tracks addon versions, never Skript ones. Inventing a
        // `since` would make addon syntax vanish under version filtering.
        let catalog = parse_filtered(SKBEE, |_| true).unwrap();
        assert!(catalog.docs.effects[0].min_version.is_none());
        assert!(catalog.docs.effects[0].since.first().is_none());
    }

    #[test]
    fn maps_every_syntax_type_the_api_uses() {
        // The eight values observed across all 8,210 records.
        for syntax_type in [
            "expression",
            "effect",
            "condition",
            "event",
            "section",
            "structure",
            "type",
            "function",
        ] {
            assert!(
                category_of(syntax_type).is_some(),
                "{syntax_type} has no category"
            );
        }
        assert!(category_of("something-new").is_none());
    }

    #[test]
    fn collects_required_plugins_and_minecraft_versions() {
        let json = r#"[{
            "id": 1, "title": "T", "syntax_pattern": "do thing",
            "syntax_type": "effect",
            "compatible_minecraft_version": "1.21.2",
            "required_plugins": [{"name": "Citizens", "link": "x"}],
            "addon": {"name": "A", "link_to_addon": "", "usage_score": 0}
        }]"#;
        let catalog = parse_filtered(json, |_| true).unwrap();
        let requirements = &catalog.docs.effects[0].requirements;
        assert!(requirements.contains(&"Citizens".to_string()));
        assert!(requirements.iter().any(|r| r.contains("1.21.2")));
    }

    #[test]
    fn skips_records_with_no_pattern() {
        let json = r#"[{"id": 1, "title": "T", "syntax_type": "effect", "syntax_pattern": ""}]"#;
        let catalog = parse_filtered(json, |_| true).unwrap();
        assert!(catalog.docs.effects.is_empty());
        assert_eq!(catalog.skipped, 1);
    }

    #[test]
    fn synthesises_an_id_when_json_id_is_absent() {
        // 1,255 of 8,210 records have no json_id. A synthesised one must never
        // collide with a real Skript id, or dedup would drop the wrong entry.
        let json = r#"[{"id": 42, "title": "T", "syntax_type": "effect", "syntax_pattern": "x"}]"#;
        let catalog = parse_filtered(json, |_| true).unwrap();
        assert_eq!(catalog.docs.effects[0].id, "skripthub:42");
    }

    #[test]
    fn lists_addons_by_popularity() {
        let json = r#"[
            {"id":1,"title":"a","syntax_type":"effect","syntax_pattern":"x",
             "addon":{"name":"Quiet","link_to_addon":"","usage_score":1.0}},
            {"id":2,"title":"b","syntax_type":"effect","syntax_pattern":"y",
             "addon":{"name":"Popular","link_to_addon":"","usage_score":99.0}}
        ]"#;
        let addons = addons(json).unwrap();
        assert_eq!(addons[0].0, "Popular");
        assert_eq!(addons[1].0, "Quiet");
    }
}
