//! Reading a Bukkit/Paper plugin manifest.
//!
//! Two formats are in use, and **both must be handled**:
//!
//! * `plugin.yml` — the classic Bukkit descriptor. Dependencies live in the
//!   flat `depend`, `softdepend`, `loadbefore` and `loadafter` lists.
//! * `paper-plugin.yml` — Paper's newer descriptor, with a nested
//!   `dependencies: { server: { Skript: { required: true } } }` block.
//!
//! This is not cosmetic. **SkBee — the most popular Skript addon there is —
//! ships only `paper-plugin.yml`.** A `plugin.yml`-only reader silently fails
//! to detect it, which is exactly the sort of gap that looks like "addons just
//! don't work" to a user.
//!
//! The parser here handles the small, regular subset of YAML these files use:
//! top-level scalars, top-level block/flow sequences, and the one nested
//! `dependencies.server.<Name>` mapping. That is a deliberate limit — pulling a
//! full YAML engine in to read two strings and a list is not a good trade, and
//! a manifest that defeats this parser degrades to "not detected" rather than
//! to a wrong answer.

/// What a plugin's manifest tells us.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// Every plugin named as a dependency, from whichever schema was used.
    pub dependencies: Vec<String>,
}

impl Manifest {
    /// Whether this plugin extends Skript.
    ///
    /// Addons are ordinary plugins in `plugins/` — nothing about their location
    /// marks them out — so the only signal is that they depend on Skript.
    pub fn is_skript_addon(&self) -> bool {
        // Skript itself is not an addon, but it *is* worth detecting: its
        // version is the best available answer for the user's target version.
        self.dependencies
            .iter()
            .any(|name| name.eq_ignore_ascii_case("Skript"))
    }

    pub fn is_skript_itself(&self) -> bool {
        self.name.eq_ignore_ascii_case("Skript")
    }
}

/// Parses either manifest format.
pub fn parse(text: &str) -> Option<Manifest> {
    let mut manifest = Manifest::default();
    let mut in_dependencies = false;
    let mut pending_list_key: Option<String> = None;

    for raw in text.lines() {
        // Comments and blank lines carry nothing.
        let without_comment = strip_comment(raw);
        if without_comment.trim().is_empty() {
            continue;
        }

        let indent = without_comment.len() - without_comment.trim_start().len();
        let line = without_comment.trim();

        // A block-sequence item continues whichever key introduced it.
        if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix('-')) {
            if let Some(key) = &pending_list_key {
                if is_dependency_key(key) {
                    push_name(&mut manifest.dependencies, item);
                }
            }
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        let value = value.trim();

        // Leaving column zero ends any top-level list.
        if indent == 0 {
            pending_list_key = None;
            in_dependencies = key.eq_ignore_ascii_case("dependencies");
        }

        match key.to_ascii_lowercase().as_str() {
            "name" if indent == 0 => manifest.name = scalar(value),
            "version" if indent == 0 => manifest.version = scalar(value),

            _ if is_dependency_key(key) => {
                if value.is_empty() {
                    // A block sequence follows on the next lines.
                    pending_list_key = Some(key.to_string());
                } else {
                    // Or a flow sequence on this one: `[Skript, Vault]`.
                    for item in value.trim_matches(['[', ']']).split(',') {
                        push_name(&mut manifest.dependencies, item);
                    }
                }
            }

            // Paper's nested form. Anything indented under `dependencies:` that
            // is a mapping key names a plugin — `server:` and `bootstrap:` are
            // the grouping levels and are skipped.
            _ if in_dependencies
                && indent > 0
                && value.is_empty()
                && !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "server" | "bootstrap" | "required" | "load" | "join-classpath"
                ) =>
            {
                push_name(&mut manifest.dependencies, key);
            }

            _ => {}
        }
    }

    (!manifest.name.is_empty()).then_some(manifest)
}

fn is_dependency_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "depend" | "softdepend" | "loadbefore" | "loadafter"
    )
}

/// Removes a trailing `# comment`, leaving `#` inside quotes alone.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'"' | b'\'' => match quote {
                Some(open) if open == byte => quote = None,
                None => quote = Some(byte),
                _ => {}
            },
            b'#' if quote.is_none()
                // Only a `#` that starts a token is a comment.
                && (index == 0 || bytes[index - 1].is_ascii_whitespace()) =>
            {
                return &line[..index];
            }
            _ => {}
        }
    }
    line
}

fn scalar(value: &str) -> String {
    value.trim().trim_matches(['"', '\'']).trim().to_string()
}

fn push_name(into: &mut Vec<String>, raw: &str) {
    let name = scalar(raw);
    if !name.is_empty() && !into.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
        into.push(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SkBee's real manifest. It has **no `plugin.yml` at all**.
    const SKBEE_PAPER: &str = r#"
main: com.shanebeestudios.skbee.SkBee
name: SkBee
version: '3.6.0'
description: "A simple Skript addon."
api-version: '1.21'
author: ShaneBee
website: https://github.com/ShaneBeee/SkBee
folia-supported: true

dependencies:
  server:
    Skript:
      load: BEFORE
      required: true
    SkQuery:
      load: AFTER
      required: false
"#;

    /// skript-reflect's real manifest, in the classic format.
    const REFLECT_CLASSIC: &str = r#"
name: skript-reflect
version: 2.5.1
description: Reflection utilities for Skript.
authors: [Bryan Terce, TPGamesNL, 'SkriptLang Team']
website: https://github.com/SkriptLang/skript-reflect
api-version: 1.19
main: com.btk5h.skriptmirror.SkriptMirror
softdepend: [Skript]
loadbefore: [skript-mirror]
"#;

    #[test]
    fn reads_papers_nested_dependency_block() {
        let manifest = parse(SKBEE_PAPER).expect("SkBee's manifest should parse");
        assert_eq!(manifest.name, "SkBee");
        assert_eq!(manifest.version, "3.6.0");
        assert!(
            manifest.is_skript_addon(),
            "SkBee must be detected — it is the most popular addon and ships \
             only paper-plugin.yml. Dependencies found: {:?}",
            manifest.dependencies
        );
        assert!(manifest.dependencies.iter().any(|d| d == "SkQuery"));
    }

    #[test]
    fn reads_the_classic_flow_sequence_form() {
        let manifest = parse(REFLECT_CLASSIC).expect("skript-reflect should parse");
        assert_eq!(manifest.name, "skript-reflect");
        assert_eq!(manifest.version, "2.5.1");
        assert!(manifest.is_skript_addon());
    }

    #[test]
    fn reads_a_block_sequence() {
        let manifest =
            parse("name: Thing\nversion: 1.0\nsoftdepend:\n  - Skript\n  - Vault\n").unwrap();
        assert!(manifest.is_skript_addon());
        assert!(manifest.dependencies.iter().any(|d| d == "Vault"));
    }

    #[test]
    fn a_plugin_that_does_not_depend_on_skript_is_not_an_addon() {
        let manifest = parse("name: LuckPerms\nversion: 5.5.50\nsoftdepend: [Vault]\n").unwrap();
        assert!(!manifest.is_skript_addon());
        assert_eq!(manifest.name, "LuckPerms");
    }

    #[test]
    fn recognises_skript_itself() {
        let manifest = parse("name: Skript\nversion: 2.16.1\n").unwrap();
        assert!(manifest.is_skript_itself());
        assert!(!manifest.is_skript_addon());
    }

    #[test]
    fn strips_quotes_from_scalars() {
        let manifest = parse("name: 'My Plugin'\nversion: \"1.2.3\"\n").unwrap();
        assert_eq!(manifest.name, "My Plugin");
        assert_eq!(manifest.version, "1.2.3");
    }

    #[test]
    fn ignores_comments_but_not_hashes_inside_values() {
        let manifest = parse("name: Thing   # the plugin\nversion: 1.0\n").unwrap();
        assert_eq!(manifest.name, "Thing");

        let manifest = parse("name: C#Plugin\nversion: 1.0\n").unwrap();
        assert_eq!(manifest.name, "C#Plugin");
    }

    #[test]
    fn is_case_insensitive_about_the_skript_dependency() {
        let manifest = parse("name: T\nversion: 1\nsoftdepend: [skript]\n").unwrap();
        assert!(manifest.is_skript_addon());
    }

    #[test]
    fn a_manifest_without_a_name_is_rejected() {
        // Better to report nothing than to invent a nameless addon.
        assert!(parse("version: 1.0\n").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn unbuilt_gradle_placeholders_survive_without_confusing_us() {
        // The repo source carries `version: '$version'`; only a built JAR has
        // the real number. Detection must not choke on the placeholder.
        let manifest = parse("name: SkBee\nversion: '$version'\n").unwrap();
        assert_eq!(manifest.name, "SkBee");
        assert_eq!(manifest.version, "$version");
    }
}
