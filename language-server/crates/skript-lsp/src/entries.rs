//! Documentation for structure entries.
//!
//! An entry is the `key: value` line inside a structure — `permission:` in a
//! command, `cooldown:`, `trigger:`. They are 12% of the lines in Skript's own
//! example scripts, and were once the only thing in a real script that got no
//! hover at all.
//!
//! **This table is curated, and that is deliberate.** Everything else in this
//! server comes from Skript's published database, because effects and conditions
//! are registered at runtime and hardcoding them would be wrong. Entries are the
//! exception: `docs.json` has no field describing them at all — the `structures`
//! entry for `Command` carries only patterns and a prose example. So there is
//! nothing to read, and the alternative to a table is nothing.
//!
//! The set mirrors `SubCommandEntryData.SUB_COMMAND_VALIDATOR` in Skript 2.17,
//! which one validator serves for a command and for each of its subcommands.
//! The published `Command` examples show only some of it: `prefix:` and the
//! `tab completions` spelling of `suggestions:` appear in no example at all.
//! An addon may register entries that are not here; those simply get no
//! hover, exactly like unknown syntax.

use skript_index::{Document, Symbol, SymbolKind};

/// One structure entry: the key as written, and what it does.
pub struct Entry {
    pub key: &'static str,
    /// Other keys Skript accepts for the same entry.
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    /// Shown under the summary as `Example: …` when present.
    pub example: Option<&'static str>,
    /// What completion inserts, when it is more than `key: `. Only the
    /// entries that open a section need one.
    pub snippet: Option<&'static str>,
}

/// The entries Skript's `command` structure, and each `subcommand` in it,
/// accepts.
pub const COMMAND: &[Entry] = &[
    Entry {
        key: "usage",
        aliases: &[],
        summary: "Shown when the command is used incorrectly. If omitted, Skript builds one from \
                  the command's arguments.",
        example: Some("usage: /home set/remove <name>"),
        snippet: None,
    },
    Entry {
        key: "description",
        aliases: &[],
        summary: "What the command does. Shown in help listings. Only the root command may \
                  have one, not a subcommand.",
        example: Some("description: Travel to one of your homes."),
        snippet: None,
    },
    Entry {
        key: "prefix",
        aliases: &[],
        summary: "The namespace the command is registered under, so `/myplugin:home` also \
                  works. Defaults to `skript`. Only the root command may have one, and it may \
                  not contain spaces or `§`.",
        example: Some("prefix: myplugin"),
        snippet: None,
    },
    Entry {
        key: "permission",
        aliases: &[],
        summary: "The permission node a sender must hold. Without one, anybody may run the \
                  command.",
        example: Some("permission: skript.command.home"),
        snippet: None,
    },
    Entry {
        key: "permission message",
        aliases: &[],
        summary: "Sent when the sender lacks the permission above. Deprecated since Skript 2.17: \
                  a command the player may not run is no longer sent to their client at all, so \
                  this message is never seen.",
        example: Some("permission message: You cannot do that."),
        snippet: None,
    },
    Entry {
        key: "aliases",
        aliases: &[],
        summary: "Other names the command answers to, separated by commas. Only the root \
                  command may have them.",
        example: Some("aliases: /h, /sethome"),
        snippet: None,
    },
    Entry {
        key: "executable by",
        aliases: &[],
        summary: "Who may run it: `players`, `operators`, `console` or `blocks` (command \
                  blocks), combined with commas or `and`. Defaults to everyone. A subcommand \
                  cannot widen what its parent allows.",
        example: Some("executable by: players and console"),
        snippet: None,
    },
    Entry {
        key: "cooldown",
        aliases: &[],
        summary: "How long a player must wait between uses. A timespan, so `15 seconds` or \
                  `2 minutes`.",
        example: Some("cooldown: 15 seconds"),
        snippet: None,
    },
    Entry {
        key: "cooldown message",
        aliases: &[],
        summary: "Sent while the cooldown is still running. `%remaining time%` and \
                  `%elapsed time%` are available.",
        example: Some("cooldown message: Wait %remaining time%."),
        snippet: None,
    },
    Entry {
        key: "cooldown bypass",
        aliases: &[],
        summary: "A permission that exempts its holder from the cooldown.",
        example: Some("cooldown bypass: skript.command.home.admin"),
        snippet: None,
    },
    Entry {
        key: "cooldown storage",
        aliases: &[],
        summary: "A variable to keep the cooldown in, so it survives a restart. Without this the \
                  cooldown is forgotten on reload.",
        example: Some("cooldown storage: {cooldown::%player%}"),
        snippet: None,
    },
    Entry {
        key: "trigger",
        aliases: &[],
        summary: "The code that runs. Every command needs one unless it has subcommands, and a \
                  command with an optional subcommand (`subcommand [help]:`) may not have one.",
        example: None,
        snippet: Some("trigger:\n\t$0"),
    },
    Entry {
        key: "suggestions",
        aliases: &["tab completions", "tab complete"],
        summary: "Code that runs while the player is typing, to offer tab completions for the \
                  arguments. Add to `the suggestions for the <name> argument`; `the current \
                  input` is what has been typed so far. Skript 2.17+.",
        example: Some(
            "suggestions:\n\tloop {homes::%player%::*}:\n\t\tadd loop-index to the suggestions \
             for the name argument",
        ),
        snippet: Some("suggestions:\n\t$0"),
    },
    Entry {
        key: "subcommand",
        aliases: &[],
        summary: "A subcommand with its own pattern, `<argument>` slots included, and its own \
                  entries: `trigger:`, `suggestions:`, `permission:`, `executable by:` and the \
                  rest. Subcommands nest. `subcommand [help]:` makes the word optional. \
                  Skript 2.17+.",
        example: Some("subcommand set <name: text>:\n\ttrigger:\n\t\tsend \"Set %{_name}%\""),
        snippet: Some("subcommand ${1:name}:\n\ttrigger:\n\t\t$0"),
    },
    Entry {
        key: "subcommands",
        aliases: &[],
        summary: "Groups subcommands that share entries such as `permission:` or `executable \
                  by:`. A group cannot have a `trigger:` of its own. Skript 2.17+.",
        example: Some(
            "subcommands:\n\tpermission: home.admin\n\tsubcommand delete <name: text>:\n\t\t\
             trigger:\n\t\t\tdelete {homes::%player%::%{_name}%}",
        ),
        snippet: Some("subcommands:\n\tsubcommand ${1:name}:\n\t\ttrigger:\n\t\t\t$0"),
    },
];

/// Looks a key up, ignoring case and surrounding whitespace.
///
/// `subcommand <pattern>` is a header rather than a `key:` line, so anything
/// starting with the word finds the `subcommand` entry.
pub fn lookup(entries: &'static [Entry], key: &str) -> Option<&'static Entry> {
    let wanted = key.trim().trim_end_matches(':').trim();
    let is_subcommand = wanted
        .get(..11)
        .is_some_and(|start| start.eq_ignore_ascii_case("subcommand "));
    entries.iter().find(|entry| {
        entry.key.eq_ignore_ascii_case(wanted)
            || entry
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(wanted))
            || (is_subcommand && entry.key == "subcommand")
    })
}

/// Renders the hover card for an entry.
pub fn hover(entry: &Entry) -> String {
    let mut out = format!("**{}** · command entry\n\n{}", entry.key, entry.summary);
    if !entry.aliases.is_empty() {
        let aliases: Vec<String> = entry.aliases.iter().map(|a| format!("`{a}`")).collect();
        out.push_str(&format!("\n\nAlso written {}.", aliases.join(" or ")));
    }
    if let Some(example) = entry.example {
        out.push_str(&format!("\n\n```skript\n{example}\n```"));
    }
    out
}

/// The completion label: the key as it is typed. A `subcommand` is followed
/// by its pattern, not a colon.
pub fn label(entry: &Entry) -> String {
    match entry.key {
        "subcommand" => "subcommand".to_string(),
        key => format!("{key}:"),
    }
}

/// What accepting the entry inserts, as an LSP snippet.
pub fn snippet(entry: &Entry) -> String {
    entry
        .snippet
        .map_or_else(|| format!("{}: $0", entry.key), str::to_string)
}

/// Whether `line` is where a command's entries go, rather than code.
///
/// That is anywhere in a command's body that is not inside one of its
/// sections, and the same again inside each `subcommand` and `subcommands:`
/// group, which take the same entries. A `trigger:` or `suggestions:` body is
/// code.
pub fn in_entry_position(document: &Document, line: u32) -> bool {
    entry_scope(document, line).is_some()
}

/// Whose entries a line takes: the command's own, or a subcommand's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryScope {
    Command,
    /// Inside a `subcommand` or a `subcommands:` group.
    Subcommand,
}

/// Like [`in_entry_position`], saying which kind of body the line is in.
pub fn entry_scope(document: &Document, line: u32) -> Option<EntryScope> {
    let command = document
        .symbols()
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Command && inside(symbol, line))?;
    scope_among(&command.children, line, EntryScope::Command)
}

/// Whether `entry` may be written in `scope`. Skript's validator for a
/// subcommand is the command's minus `prefix`, which only the root may set.
pub fn allowed_in(entry: &Entry, scope: EntryScope) -> bool {
    !(scope == EntryScope::Subcommand && entry.key == "prefix")
}

/// Strictly below the header, up to and including the last line.
fn inside(symbol: &Symbol, line: u32) -> bool {
    line > symbol.range.start.line && line <= symbol.range.end.line
}

fn scope_among(children: &[Symbol], line: u32, scope: EntryScope) -> Option<EntryScope> {
    // Deliberately not testing whether a child has children of its own: a
    // trigger whose body is plain statements has none, which once made entry
    // keys appear in the middle of a trigger.
    match children.iter().find(|child| inside(child, line)) {
        None => Some(scope),
        Some(child) if takes_entries(child) => {
            scope_among(&child.children, line, EntryScope::Subcommand)
        }
        Some(_) => None,
    }
}

fn takes_entries(symbol: &Symbol) -> bool {
    let name = symbol.name.to_ascii_lowercase();
    symbol.kind == SymbolKind::Entry && (name == "subcommands" || name.starts_with("subcommand "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_skript_accepts_is_present() {
        // `SubCommandEntryData.SUB_COMMAND_VALIDATOR`, Skript 2.17.
        for key in [
            "usage",
            "description",
            "prefix",
            "suggestions",
            "permission",
            "permission message",
            "aliases",
            "executable by",
            "cooldown",
            "cooldown message",
            "cooldown bypass",
            "cooldown storage",
            "trigger",
            "subcommand",
            "subcommands",
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

    #[test]
    fn aliases_and_subcommand_headers_find_their_entry() {
        assert_eq!(
            lookup(COMMAND, "Tab Completions").unwrap().key,
            "suggestions"
        );
        // What hover passes for `subcommand set <name: text>:`.
        assert_eq!(
            lookup(COMMAND, "\tsubcommand set <name").unwrap().key,
            "subcommand"
        );
        assert_eq!(lookup(COMMAND, "subcommands").unwrap().key, "subcommands");
        assert!(lookup(COMMAND, "subcommandx").is_none());
    }

    #[test]
    fn section_entries_insert_a_body() {
        let entry = lookup(COMMAND, "suggestions").unwrap();
        assert_eq!(snippet(entry), "suggestions:\n\t$0");
        assert_eq!(
            snippet(lookup(COMMAND, "cooldown").unwrap()),
            "cooldown: $0"
        );
        assert_eq!(label(lookup(COMMAND, "subcommand").unwrap()), "subcommand");
        assert!(hover(entry).contains("`tab completions`"));
    }

    const HOME: &str = concat!(
        "command /home:\n",                      // 0
        "\tpermission: home.use\n",              // 1
        "\tsubcommand set <name: text>:\n",      // 2
        "\t\texecutable by: players\n",          // 3
        "\t\ttrigger:\n",                        // 4
        "\t\t\tsend \"set\"\n",                  // 5
        "\tsubcommands:\n",                      // 6
        "\t\tpermission: home.admin\n",          // 7
        "\t\tsubcommand delete <name: text>:\n", // 8
        "\t\t\tsuggestions:\n",                  // 9
        "\t\t\t\tstop\n",                        // 10
        "\t\t\tcooldown: 1 second\n",            // 11
        "\ttrigger:\n",                          // 12
        "\t\tsend \"home\"\n",                   // 13
    );

    #[test]
    fn subcommands_take_entries_and_their_sections_do_not() {
        let document = Document::new("file:///t.sk", HOME);
        let entry_lines: Vec<u32> = (0..14)
            .filter(|line| in_entry_position(&document, *line))
            .collect();
        assert_eq!(entry_lines, vec![1, 2, 3, 4, 6, 7, 8, 9, 11, 12]);
    }
}
