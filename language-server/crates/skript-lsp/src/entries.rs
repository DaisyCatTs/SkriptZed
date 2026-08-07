//! Documentation for structure entries.
//!
//! An entry is the `key: value` line inside a structure — `permission:` in a
//! command, `cooldown:`, `trigger:`. They are 12% of the lines in Skript's own
//! example scripts, and until now they were the only thing in a real script that
//! got no hover at all.
//!
//! **This table is curated, and that is deliberate.** Everything else in this
//! server comes from Skript's published database, because effects and conditions
//! are registered at runtime and hardcoding them would be wrong. Entries are the
//! exception: `docs.json` has no field describing them at all — the `structures`
//! entry for `Command` carries only patterns and a prose example. So there is
//! nothing to read, and the alternative to a table is nothing.
//!
//! The set is taken from Skript's own published example for `Command`, so it is
//! accurate to the version this server targets. An addon may register entries
//! that are not here; those simply get no hover, exactly like unknown syntax.

/// One structure entry: the key as written, and what it does.
pub struct Entry {
    pub key: &'static str,
    pub summary: &'static str,
    /// Shown under the summary as `Example: …` when present.
    pub example: Option<&'static str>,
}

/// The entries Skript's `command` structure accepts.
pub const COMMAND: &[Entry] = &[
    Entry {
        key: "usage",
        summary: "Shown when the command is used incorrectly. If omitted, Skript builds one from \
                  the command's arguments.",
        example: Some("usage: /home set/remove <name>"),
    },
    Entry {
        key: "description",
        summary: "What the command does. Shown in help listings.",
        example: Some("description: Travel to one of your homes."),
    },
    Entry {
        key: "permission",
        summary: "The permission node a sender must hold. Without one, anybody may run the \
                  command.",
        example: Some("permission: skript.command.home"),
    },
    Entry {
        key: "permission message",
        summary: "Sent when the sender lacks the permission above. Defaults to Skript's own \
                  message.",
        example: Some("permission message: You cannot do that."),
    },
    Entry {
        key: "aliases",
        summary: "Other names the command answers to, separated by commas.",
        example: Some("aliases: /h, /sethome"),
    },
    Entry {
        key: "executable by",
        summary: "Who may run it: `players`, `console`, or `players and console`. Defaults to \
                  both.",
        example: Some("executable by: players"),
    },
    Entry {
        key: "cooldown",
        summary: "How long a player must wait between uses. A timespan, so `15 seconds` or \
                  `2 minutes`.",
        example: Some("cooldown: 15 seconds"),
    },
    Entry {
        key: "cooldown message",
        summary: "Sent while the cooldown is still running. `%remaining time%` and \
                  `%elapsed time%` are available.",
        example: Some("cooldown message: Wait %remaining time%."),
    },
    Entry {
        key: "cooldown bypass",
        summary: "A permission that exempts its holder from the cooldown.",
        example: Some("cooldown bypass: skript.command.home.admin"),
    },
    Entry {
        key: "cooldown storage",
        summary: "A variable to keep the cooldown in, so it survives a restart. Without this the \
                  cooldown is forgotten on reload.",
        example: Some("cooldown storage: {cooldown::%player%}"),
    },
    Entry {
        key: "trigger",
        summary: "The code that runs. This is the only entry that opens a section, and every \
                  command needs one.",
        example: None,
    },
];

/// Looks a key up, ignoring case and surrounding whitespace.
pub fn lookup(entries: &'static [Entry], key: &str) -> Option<&'static Entry> {
    let wanted = key.trim().trim_end_matches(':').trim();
    entries
        .iter()
        .find(|entry| entry.key.eq_ignore_ascii_case(wanted))
}

/// Renders the hover card for an entry.
pub fn hover(entry: &Entry) -> String {
    let mut out = format!("**{}** — command entry\n\n{}", entry.key, entry.summary);
    if let Some(example) = entry.example {
        out.push_str(&format!("\n\n```skript\n{example}\n```"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_skript_documents_is_present() {
        // Taken from the `command` example published in docs.json, so this is
        // the authoritative set for the targeted Skript version.
        for key in [
            "usage",
            "permission",
            "permission message",
            "aliases",
            "executable by",
            "cooldown",
            "cooldown message",
            "cooldown bypass",
            "cooldown storage",
            "trigger",
        ] {
            assert!(lookup(COMMAND, key).is_some(), "missing entry {key:?}");
        }
    }

    #[test]
    fn lookup_ignores_case_and_a_trailing_colon() {
        assert!(lookup(COMMAND, "Cooldown Message:").is_some());
        assert!(lookup(COMMAND, "  trigger:  ").is_some());
        assert!(lookup(COMMAND, "not an entry").is_none());
    }
}
