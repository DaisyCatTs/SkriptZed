//! Turning a Skript pattern into the snippet a completion inserts.

/// Reads the branches of a `(a|b|c)` choice whose `(` has just been consumed,
/// up to and including its matching `)`. Nested groups stay inside their
/// branch as text.
pub fn read_branches(chars: &mut impl Iterator<Item = char>) -> Vec<String> {
    let mut branches: Vec<String> = vec![String::new()];
    let mut inner = 0i32;
    for ch in chars.by_ref() {
        let current = branches.last_mut().expect("never empty");
        match ch {
            '(' => {
                inner += 1;
                current.push(ch);
            }
            ')' if inner == 0 => break,
            ')' => {
                inner -= 1;
                current.push(ch);
            }
            '|' if inner == 0 => branches.push(String::new()),
            _ => current.push(ch),
        }
    }
    branches
}

/// Turns a Skript pattern into an LSP snippet, with a tab stop per slot.
///
/// The form inserted when the user has typed nothing to disambiguate a choice.
#[cfg(test)]
pub fn snippet_for(pattern: &str) -> String {
    snippet_for_typed(pattern, "")
}

/// As [`snippet_for`], but preferring whichever alternative the user has begun
/// to type, so accepting `Message` after typing `send` does not rewrite the word
/// under the cursor into `message`.
pub fn snippet_for_typed(pattern: &str, typed: &str) -> String {
    let mut out = String::new();
    let mut stop = 1;
    expand(pattern, typed, &mut stop, &mut out);
    // Collapse the double spaces left by dropped optionals.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Expands one level of pattern text into `out`.
///
/// Recursive, because a chosen branch is itself pattern text: Skript writes
/// `(message|send [message[s]])`, so the optional lives *inside* the
/// alternative. Pushing a branch verbatim leaked `[message[s]]` into the snippet
/// as literal characters.
fn expand(pattern: &str, typed: &str, stop: &mut usize, out: &mut String) {
    let mut chars = pattern.chars();
    let mut depth = 0i32;

    while let Some(ch) = chars.next() {
        match ch {
            // Optional parts are dropped: the shortest correct form is the best
            // starting point, and the user can add the rest.
            '[' => depth += 1,
            ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            '(' => {
                let branches = read_branches(&mut chars);
                let chosen = pick_branch(&branches, typed).to_string();
                expand(&chosen, typed, stop, out);
            }
            '%' => {
                let name: String = chars.by_ref().take_while(|&ch| ch != '%').collect();
                let name = name.trim_start_matches(['~', '-', '*']);
                out.push_str(&format!("${{{stop}:{name}}}"));
                *stop += 1;
            }
            '<' => {
                chars.by_ref().find(|&ch| ch == '>');
                out.push_str(&format!("${stop}"));
                *stop += 1;
            }
            _ => out.push(ch),
        }
    }
}

/// Which branch of a choice to insert.
///
/// The one the user has started typing, when there is one; otherwise the
/// first, which is the shortest correct form.
///
/// Both sides are reduced to their leading word. A branch carries its own tail
/// (Skript writes `(message|send [message[s]])`) and `typed` is the whole
/// fragment entered so far, which may already have moved past the keyword.
/// Comparing leading words is the only thing that lines the two up.
pub fn pick_branch<'a>(branches: &'a [String], typed: &str) -> &'a str {
    let first_word = |text: &str| {
        text.split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_ascii_lowercase()
    };

    let wanted = first_word(typed);
    if !wanted.is_empty() {
        for branch in branches {
            let candidate = branch.trim();
            let head = first_word(candidate);
            if !head.is_empty() && head.starts_with(&wanted) {
                return candidate;
            }
        }
    }
    branches
        .first()
        .map(|branch| branch.trim())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_drop_optionals_and_number_the_slots() {
        assert_eq!(
            snippet_for("give %item types% to %players%"),
            "give ${1:item types} to ${2:players}"
        );
        assert_eq!(snippet_for("cancel [the] event"), "cancel event");
        assert_eq!(snippet_for("(spawn|summon) %number%"), "spawn ${1:number}");
    }

    #[test]
    fn snippets_strip_slot_modifiers() {
        assert_eq!(snippet_for("filter %~objects%"), "filter ${1:objects}");
    }

    #[test]
    fn a_regex_hole_becomes_a_bare_tab_stop() {
        assert_eq!(snippet_for("command <.+>"), "command $1");
    }

    /// Skript's real Message pattern, where the optional lives *inside* the
    /// alternative. Matching on whole branch text meant `send [message[s]]`
    /// never matched somebody typing `send`, and pushing the branch verbatim
    /// leaked `[message[s]]` into the snippet as literal characters.
    const MESSAGE: &str = "(message|send [message[s]]) %objects% [to %audiences%]";

    #[test]
    fn a_typed_keyword_survives_and_its_optional_does_not_leak() {
        for typed in ["send", "se", "send m"] {
            let snippet = snippet_for_typed(MESSAGE, typed);
            assert!(
                snippet.starts_with("send"),
                "typing {typed:?} inserted {snippet:?}"
            );
            assert!(!snippet.contains('['), "an optional leaked: {snippet:?}");
        }
    }

    #[test]
    fn the_default_is_still_the_shortest_form() {
        assert_eq!(snippet_for(MESSAGE), "message ${1:objects}");
    }

    #[test]
    fn tab_stops_stay_sequential_through_a_chosen_branch() {
        // A slot inside the branch must not restart the numbering.
        let snippet = snippet_for_typed("(give %items% to|hand) %players%", "give");
        assert!(snippet.contains("${1:"), "{snippet:?}");
        assert!(snippet.contains("${2:"), "{snippet:?}");
    }

    #[test]
    fn an_unrelated_word_falls_back_to_the_first_branch() {
        let list = vec!["message".to_string(), "send".to_string()];
        assert_eq!(pick_branch(&list, "teleport"), "message");
        assert_eq!(pick_branch(&list, ""), "message");
    }

    #[test]
    fn branches_keep_nested_groups_whole() {
        let mut chars = "a|(b|c) d|e) tail".chars();
        assert_eq!(read_branches(&mut chars), vec!["a", "(b|c) d", "e"]);
        assert_eq!(chars.collect::<String>(), " tail");
    }
}
