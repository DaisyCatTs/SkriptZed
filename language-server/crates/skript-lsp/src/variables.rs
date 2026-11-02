//! Completion inside a variable's braces.
//!
//! Skript declares almost no variables — `{_total}` exists because a line set
//! it — so the index's declarations are the wrong source. Every name a script
//! has written is. List variables complete one level at a time:
//! after `{data::` the choices are whatever this project has put at that level
//! (`%player's uuid%`, `settings`, `*`), not every full name that starts there.

use std::collections::HashMap;

/// A variable name as written between the braces, with where it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub line: u32,
    /// Byte columns of the braces themselves, `{` inclusive to `}` exclusive.
    pub start: u32,
    pub end: u32,
    /// The text inside the braces: `_data::%player%::coins`.
    pub name: String,
}

/// One choice for the level being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The level's text, as the project spells it.
    pub segment: String,
    /// What the editor inserts: the segment, then `::` to go a level deeper,
    /// or `}` to finish when the closing brace is not already there.
    pub insert: String,
    pub detail: &'static str,
    /// How many times this exact choice appears; more is more likely.
    pub uses: usize,
}

/// The byte index of the innermost `{` still open at the end of `prefix`, when
/// it opens a variable rather than an `{@option}`.
pub fn open_brace(prefix: &str) -> Option<usize> {
    let mut open = Vec::new();
    for (index, byte) in prefix.bytes().enumerate() {
        match byte {
            b'{' => open.push(index),
            b'}' => {
                open.pop();
            }
            _ => {}
        }
    }
    let at = *open.last()?;
    (!prefix[at + 1..].starts_with('@')).then_some(at)
}

/// Every `{…}` on every line of `text`, nested ones included: `{_b}` in
/// `{_a::{_b}}` is a variable too. Option references are not variables, and
/// neither is anything in a comment, where `# set {_old} to 1` used to offer
/// `_old` as a name the script uses.
pub fn occurrences(text: &str) -> Vec<Occurrence> {
    let mut out = Vec::new();
    for (line, raw) in crate::lines::code_lines(text) {
        let raw = crate::lines::code_part(raw);
        let mut open: Vec<usize> = Vec::new();
        for (index, byte) in raw.bytes().enumerate() {
            match byte {
                b'{' => open.push(index),
                b'}' => {
                    let Some(start) = open.pop() else { continue };
                    let name = &raw[start + 1..index];
                    if !name.is_empty() && !name.starts_with('@') {
                        out.push(Occurrence {
                            line,
                            start: start as u32,
                            end: index as u32 + 1,
                            name: name.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Splits a name at its list separators, ignoring any `::` inside a nested
/// variable or a `%…%` expression: `data::%{_k}::x%::coins` has two levels
/// after `data`, not three.
pub fn levels(name: &str) -> Vec<&str> {
    let bytes = name.as_bytes();
    let mut out = Vec::new();
    let (mut braces, mut in_percent, mut start) = (0u32, false, 0usize);
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'%' if braces == 0 => in_percent = !in_percent,
            b':' if braces == 0 && !in_percent && bytes.get(index + 1) == Some(&b':') => {
                out.push(&name[start..index]);
                index += 2;
                start = index;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    out.push(&name[start..]);
    out
}

/// Whether a name is trigger-local.
pub fn is_local(name: &str) -> bool {
    name.starts_with('_')
}

/// The choices for the level being typed in `typed` (everything after the
/// open brace), drawn from `names`.
///
/// Only names of the same kind are considered — a local never suggests a
/// global — and only those whose earlier levels match what is already typed,
/// compared case-insensitively as Skript compares variable names.
/// `closes` says whether a `}` already follows the cursor, so finishing a name
/// does not leave two.
pub fn suggest<'a>(
    typed: &str,
    names: impl IntoIterator<Item = &'a str>,
    closes: bool,
) -> (usize, Vec<Suggestion>) {
    let typed_levels = levels(typed);
    let done = &typed_levels[..typed_levels.len() - 1];
    // Byte offset, within `typed`, where the level being typed begins.
    let level_start = typed.len() - typed_levels.last().map_or(0, |level| level.len());
    let local = is_local(typed);

    // segment (lower-cased) → (spelling, leaf uses, branch uses)
    let mut found: HashMap<String, (String, usize, usize)> = HashMap::new();
    let mut deeper_exists = false;
    for name in names {
        if is_local(name) != local {
            continue;
        }
        let name_levels = levels(name);
        if name_levels.len() <= done.len() {
            continue;
        }
        let matches_so_far = done
            .iter()
            .zip(&name_levels)
            .all(|(typed_level, level)| typed_level.eq_ignore_ascii_case(level));
        if !matches_so_far {
            continue;
        }
        let segment = name_levels[done.len()];
        if segment.is_empty() || segment == "*" {
            if segment == "*" {
                deeper_exists = true;
            }
            continue;
        }
        let entry = found
            .entry(segment.to_ascii_lowercase())
            .or_insert_with(|| (segment.to_string(), 0, 0));
        if name_levels.len() > done.len() + 1 {
            entry.2 += 1;
        } else {
            entry.1 += 1;
        }
        if !done.is_empty() {
            deeper_exists = true;
        }
    }

    let close = if closes { "" } else { "}" };
    let mut out = Vec::new();
    for (segment, leaf, branch) in found.into_values() {
        if leaf > 0 {
            out.push(Suggestion {
                insert: format!("{segment}{close}"),
                detail: if local { "local variable" } else { "variable" },
                uses: leaf,
                segment: segment.clone(),
            });
        }
        if branch > 0 {
            out.push(Suggestion {
                insert: format!("{segment}::"),
                detail: "list, continue with ::",
                uses: branch,
                segment: format!("{segment}::"),
            });
        }
    }
    // `{data::*}` — every value of the list — is always valid once inside one.
    if deeper_exists {
        out.push(Suggestion {
            segment: "*".into(),
            insert: format!("*{close}"),
            detail: "every value in the list",
            uses: 0,
        });
    }
    out.sort_by(|a, b| b.uses.cmp(&a.uses).then_with(|| a.segment.cmp(&b.segment)));
    (level_start, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(typed: &str, names: &[&str]) -> Vec<String> {
        suggest(typed, names.iter().copied(), true)
            .1
            .into_iter()
            .map(|suggestion| suggestion.segment)
            .collect()
    }

    #[test]
    fn finds_the_open_brace_but_not_an_option() {
        assert_eq!(open_brace("set {_te"), Some(4));
        assert_eq!(open_brace("set {_a} to {da"), Some(12));
        assert_eq!(open_brace("set {_a::{_k"), Some(9));
        assert_eq!(open_brace("send {@pre"), None);
        assert_eq!(open_brace("set {_a} to 1"), None);
    }

    #[test]
    fn collects_every_name_nested_ones_included() {
        let names: Vec<String> = occurrences("set {_a::{_k}} to {@x}\n\tadd 1 to {coins}\n")
            .into_iter()
            .map(|occurrence| occurrence.name)
            .collect();
        assert_eq!(names, vec!["_k", "_a::{_k}", "coins"]);
    }

    #[test]
    fn variables_in_comments_are_not_occurrences() {
        let text = "# set {_old} to 1
set {_a} to 1 # was {_b}
###
{_prose}
###
send \"#{_c}\"
";
        let names: Vec<String> = occurrences(text)
            .into_iter()
            .map(|occurrence| occurrence.name)
            .collect();
        assert_eq!(names, vec!["_a", "_c"]);
    }

    #[test]
    fn splits_levels_but_not_inside_expressions() {
        assert_eq!(
            levels("data::%player%::coins"),
            vec!["data", "%player%", "coins"]
        );
        assert_eq!(levels("a::%{_m::x}%::b"), vec!["a", "%{_m::x}%", "b"]);
        assert_eq!(levels("_x"), vec!["_x"]);
        assert_eq!(levels("data::"), vec!["data", ""]);
    }

    #[test]
    fn locals_and_globals_never_mix() {
        let names = ["_text", "_total", "coins", "-cache"];
        assert_eq!(segments("_t", &names), vec!["_text", "_total"]);
        assert_eq!(segments("c", &names), vec!["-cache", "coins"]);
    }

    /// The level after `data::` offers what the project put there, plus `*`.
    #[test]
    fn a_list_completes_one_level_at_a_time() {
        let names = [
            "data::%player's uuid%::coins",
            "data::%player's uuid%::kills",
            "data::%player's uuid%::coins",
            "data::settings",
            "other::x",
        ];
        assert_eq!(
            segments("data::", &names),
            vec!["%player's uuid%::", "settings", "*"]
        );
        assert_eq!(
            segments("data::%player's uuid%::", &names),
            vec!["coins", "kills", "*"]
        );
        // Earlier levels compare case-insensitively, as Skript does.
        assert_eq!(
            segments("DATA::%PLAYER'S UUID%::k", &names),
            vec!["coins", "kills", "*"]
        );
    }

    #[test]
    fn finishing_a_name_adds_the_brace_only_when_missing() {
        let (_, open) = suggest("_t", ["_text"], false);
        assert_eq!(open[0].insert, "_text}");
        let (_, closed) = suggest("_t", ["_text"], true);
        assert_eq!(closed[0].insert, "_text");
    }

    #[test]
    fn reports_where_the_level_being_typed_starts() {
        let (start, _) = suggest("data::%p", ["data::%p%"], true);
        assert_eq!(start, "data::".len());
    }
}
