//! Doc comments: the run of `#` lines directly above a declaration.
//!
//! Skript has no documentation syntax of its own, so this reads the two
//! conventions other Skript editors already use, so a documented file reads
//! the same whichever editor wrote it:
//!
//! * Javadoc-style tags, as r4tsk/VSkript read them:
//!   `@param <name> <text>`, `@return(s) <text>`, `@deprecated [text]`, plus
//!   `@since <text>` and `@example <code>`. Any other `@tag` line is kept in
//!   the description as written.
//! * The `#d` family from the Findiepman Skript extension: `#d <text>`,
//!   `#dp <name> <text>`, `#dr <text>`, `#dex <code>`, `#ddep <reason>`. When a
//!   run contains any of these, its plain `#` lines are ordinary comments in
//!   that dialect and are left out.
//!
//! What counts as "directly above":
//!
//! * the lines must touch the declaration: a blank line ends the run, so a
//!   file header or a section banner above a gap never becomes a function's
//!   documentation;
//! * a comment indented deeper than the declaration belongs to the body above
//!   it (the last line of the previous function), not to this declaration;
//! * a `#-#` line is Skript's section suppressor, not prose: it is skipped
//!   without ending the run;
//! * a `###` block comment counts when its closing `###` is the line directly
//!   above. Blocks are prose more often than commented-out code when they sit
//!   flush against a declaration, and treating them like a `#` run means the
//!   two styles behave the same. A `#` run directly above a closing `###`
//!   stops there: the two are never merged.

use crate::text::Range;

/// A declaration's parsed documentation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocComment {
    /// Markdown: lines joined with `\n`, paragraphs with a blank line.
    pub description: String,
    pub params: Vec<DocParam>,
    pub returns: Option<String>,
    /// `Some("")` for a bare `@deprecated`.
    pub deprecated: Option<String>,
    pub since: Option<String>,
    /// Each example as one block of code.
    pub examples: Vec<String>,
    /// The comment lines, first to last.
    pub range: Range,
}

/// One `@param` / `#dp` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocParam {
    /// As written, minus any `{_` `}` around it.
    pub name: String,
    pub text: String,
    /// The name on its line, for a diagnostic that points at it.
    pub name_range: Range,
}

impl DocComment {
    /// The `@param` text for `name`, matched case-insensitively.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|param| param.name.eq_ignore_ascii_case(name))
            .map(|param| param.text.as_str())
    }

    fn is_empty(&self) -> bool {
        self.description.is_empty()
            && self.params.is_empty()
            && self.returns.is_none()
            && self.deprecated.is_none()
            && self.since.is_none()
            && self.examples.is_empty()
    }
}

/// The documentation directly above the declaration on `line`, if any.
pub fn above(lines: &[&str], line: u32) -> Option<DocComment> {
    let line = line as usize;
    let declaration = lines.get(line)?;
    let indent = crate::indent_width(declaration);
    let above = line.checked_sub(1)?;

    if is_block_delimiter(lines[above]) && closes_a_block(lines, above) {
        let opener = (0..above).rev().find(|&at| is_block_delimiter(lines[at]))?;
        let body: Vec<(u32, &str)> = (opener + 1..above)
            .map(|at| (at as u32, lines[at]))
            .collect();
        let mut doc = parse(&body)?;
        doc.range = Range::new(
            crate::Position::new(opener as u32, 0),
            crate::Position::new(above as u32, lines[above].len() as u32),
        );
        return Some(doc);
    }

    let mut run = Vec::new();
    for at in (0..=above).rev() {
        let text = lines[at];
        let trimmed = text.trim_start();
        if !trimmed.starts_with('#')
            || is_block_delimiter(text)
            || crate::indent_width(text) > indent
        {
            break;
        }
        run.push((at as u32, text));
    }
    run.reverse();
    let first = run.first()?.0;
    let last = run.last()?;
    let mut doc = parse(&run)?;
    doc.range = Range::new(
        crate::Position::new(first, 0),
        crate::Position::new(last.0, last.1.len() as u32),
    );
    Some(doc)
}

/// A line whose trimmed content is exactly `###`, which toggles a block
/// comment (the scanner's rule; `### foo` is an ordinary comment).
fn is_block_delimiter(line: &str) -> bool {
    line.trim() == "###"
}

/// Whether the delimiter on `at` closes a block: it is the second, fourth, …
/// delimiter in the file.
fn closes_a_block(lines: &[&str], at: usize) -> bool {
    lines[..=at]
        .iter()
        .filter(|line| is_block_delimiter(line))
        .count()
        % 2
        == 0
}

/// The `#d` dialect's tags.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Description,
    Param,
    Return,
    Example,
    Deprecated,
}

fn dialect(token: &str) -> Option<Dialect> {
    Some(match token {
        "#d" => Dialect::Description,
        "#dp" => Dialect::Param,
        "#dr" => Dialect::Return,
        "#dex" => Dialect::Example,
        "#ddep" => Dialect::Deprecated,
        _ => return None,
    })
}

/// Which tag a continuation line extends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Param,
    Return,
    Deprecated,
    Since,
    Example,
}

#[derive(Default)]
struct Builder {
    doc: DocComment,
    paragraphs: Vec<Vec<String>>,
    open: Option<Open>,
}

impl Builder {
    fn describe(&mut self, text: &str) {
        self.open = None;
        // A banner (`# ====`) says nothing about the declaration.
        if !text.chars().any(char::is_alphanumeric) {
            return;
        }
        match self.paragraphs.last_mut() {
            Some(paragraph) => paragraph.push(text.to_string()),
            None => self.paragraphs.push(vec![text.to_string()]),
        }
    }

    fn break_paragraph(&mut self) {
        self.open = None;
        if self.paragraphs.last().is_some_and(|last| !last.is_empty()) {
            self.paragraphs.push(Vec::new());
        }
    }

    /// `@param name text` / `#dp name text`. `rest` starts at byte `column`
    /// of line `line`.
    fn param(&mut self, line: u32, column: usize, rest: &str) {
        let lead = rest.len() - rest.trim_start().len();
        let rest = rest.trim_start();
        let token = rest.split_whitespace().next().unwrap_or("");
        if token.is_empty() {
            return;
        }
        let name = token
            .trim_start_matches('{')
            .trim_start_matches('_')
            .trim_end_matches('}')
            .trim_end_matches(':');
        let start = column + lead;
        let text = rest[token.len()..].trim();
        // `@param p - the player` and `@param p: the player` both read as
        // separators, not as part of the text.
        let text = text.strip_prefix("- ").unwrap_or(text).trim();
        self.doc.params.push(DocParam {
            name: name.to_string(),
            text: text.to_string(),
            name_range: Range::on_line(line, start as u32, (start + token.len()) as u32),
        });
        self.open = Some(Open::Param);
    }

    /// The slot a single-valued tag fills.
    fn slot(&mut self, open: Open) -> Option<&mut Option<String>> {
        match open {
            Open::Return => Some(&mut self.doc.returns),
            Open::Deprecated => Some(&mut self.doc.deprecated),
            Open::Since => Some(&mut self.doc.since),
            Open::Param | Open::Example => None,
        }
    }

    fn set(&mut self, open: Open, text: &str) {
        if let Some(slot) = self.slot(open) {
            *slot = Some(text.to_string());
        }
        self.open = Some(open);
    }

    fn example(&mut self, text: &str, combine: bool) {
        match self.doc.examples.last_mut() {
            Some(last) if combine => {
                // An `@example` alone left an empty block to fill.
                if !last.is_empty() {
                    last.push('\n');
                }
                last.push_str(text);
            }
            _ => self.doc.examples.push(text.to_string()),
        }
        self.open = Some(Open::Example);
    }

    /// A line under an open tag that is not itself a tag.
    fn continue_open(&mut self, open: Open, text: &str) {
        let append = |slot: &mut String| {
            if !slot.is_empty() {
                slot.push(' ');
            }
            slot.push_str(text);
        };
        match open {
            Open::Param => {
                if let Some(param) = self.doc.params.last_mut() {
                    append(&mut param.text);
                }
            }
            Open::Example => self.example(text, true),
            single => {
                if let Some(slot) = self.slot(single) {
                    append(slot.get_or_insert_with(String::new));
                }
            }
        }
    }

    /// One line of the Javadoc-style convention, already stripped of its `#`s.
    /// `content` starts at byte `column` of line `line`; `raw` is the same
    /// line with its indentation kept, which an example's code needs.
    fn tagged(&mut self, line: u32, column: usize, content: &str, raw: &str) {
        if content.is_empty() {
            self.break_paragraph();
            return;
        }
        let Some(after) = content.strip_prefix('@') else {
            match self.open {
                Some(Open::Example) => self.example(raw, true),
                Some(open) => self.continue_open(open, content),
                None => self.describe(content),
            }
            return;
        };
        let tag_len = after.find(char::is_whitespace).unwrap_or(after.len());
        let rest = &after[tag_len..];
        let text = rest.trim();
        match after[..tag_len].to_ascii_lowercase().as_str() {
            "param" | "parameter" | "arg" => self.param(line, column + 1 + tag_len, rest),
            "return" | "returns" => self.set(Open::Return, text),
            "deprecated" => self.set(Open::Deprecated, text),
            "since" => self.set(Open::Since, text),
            // `@example` alone opens a block for the lines below it.
            "example" | "examples" if text.is_empty() => {
                self.doc.examples.push(String::new());
                self.open = Some(Open::Example);
            }
            "example" | "examples" => self.example(text, false),
            // Unknown tags are text, so nothing a writer adds is lost.
            _ => self.describe(content),
        }
    }

    fn finish(mut self) -> Option<DocComment> {
        self.doc.description = self
            .paragraphs
            .iter()
            .filter(|paragraph| !paragraph.is_empty())
            .map(|paragraph| paragraph.join("\n"))
            .collect::<Vec<_>>()
            .join("\n\n");
        self.doc
            .examples
            .retain(|example| !example.trim().is_empty());
        for example in &mut self.doc.examples {
            *example = dedent(example);
        }
        (!self.doc.is_empty()).then_some(self.doc)
    }
}

/// Parses comment lines (with their line numbers) into documentation.
fn parse(lines: &[(u32, &str)]) -> Option<DocComment> {
    let mut builder = Builder::default();
    let uses_dialect = lines
        .iter()
        .any(|(_, text)| dialect(text.split_whitespace().next().unwrap_or("")).is_some());

    for &(line, text) in lines {
        let indent = text.len() - text.trim_start().len();
        let body = text[indent..].trim_end();
        if body.starts_with("#-#") {
            continue;
        }
        let token = body.split_whitespace().next().unwrap_or("");
        if uses_dialect {
            let Some(tag) = dialect(token) else {
                continue;
            };
            let rest = &body[token.len()..];
            let text = rest.trim();
            match tag {
                Dialect::Description if text.is_empty() => builder.break_paragraph(),
                Dialect::Description => builder.describe(text),
                Dialect::Param => builder.param(line, indent + token.len(), rest),
                Dialect::Return => builder.set(Open::Return, text),
                // "Multiple #dex lines are combined" into one example.
                Dialect::Example => {
                    let combine = builder.open == Some(Open::Example);
                    builder.example(rest.strip_prefix(' ').unwrap_or(rest), combine)
                }
                Dialect::Deprecated => builder.set(Open::Deprecated, text),
            }
            continue;
        }
        let hashes = body.len() - body.trim_start_matches('#').len();
        let after = &body[hashes..];
        let pad = after.len() - after.trim_start().len();
        builder.tagged(line, indent + hashes + pad, after.trim_start(), after);
    }
    builder.finish()
}

/// Strips the indentation every line of an example shares, keeping the nesting
/// inside it: `#   if x:` / `#   \tstop` is written indented for readability.
fn dedent(example: &str) -> String {
    let lines: Vec<&str> = example.lines().collect();
    let common = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| line.get(common..).unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source: &str) -> Option<DocComment> {
        let lines: Vec<&str> = source.lines().collect();
        let declaration = lines
            .iter()
            .rposition(|line| {
                let line = line.trim_start();
                line.starts_with("function") || line.starts_with("public")
            })
            .expect("a declaration");
        above(&lines, declaration as u32)
    }

    #[test]
    fn reads_the_javadoc_style_tags() {
        let found = doc(concat!(
            "# Gives a welcome kit to a player.\n",
            "#\n",
            "# @param p the player to give it to\n",
            "# @param amount how many kits\n",
            "# @return whether the kit was given\n",
            "# @deprecated use giveKit2() instead\n",
            "# @since 1.2\n",
            "# @example giveKit(player, 1)\n",
            "function giveKit(p: player, amount: number) :: boolean:\n",
        ))
        .expect("a doc comment");
        assert_eq!(found.description, "Gives a welcome kit to a player.");
        let params: Vec<(&str, &str)> = found
            .params
            .iter()
            .map(|param| (param.name.as_str(), param.text.as_str()))
            .collect();
        assert_eq!(
            params,
            vec![
                ("p", "the player to give it to"),
                ("amount", "how many kits")
            ]
        );
        assert_eq!(found.returns.as_deref(), Some("whether the kit was given"));
        assert_eq!(found.deprecated.as_deref(), Some("use giveKit2() instead"));
        assert_eq!(found.since.as_deref(), Some("1.2"));
        assert_eq!(found.examples, vec!["giveKit(player, 1)"]);
        // The name is located for the stale-`@param` diagnostic.
        assert_eq!(found.params[0].name_range, Range::on_line(2, 9, 10));
        assert_eq!(found.range.start.line, 0);
        assert_eq!(found.range.end.line, 7);
    }

    #[test]
    fn reads_the_d_dialect_and_ignores_plain_lines_beside_it() {
        let found = doc(concat!(
            "# helper, see the wiki\n",
            "#d Rolls a drop chance.\n",
            "#dp p the player who killed the boss\n",
            "#dp chance percent chance\n",
            "#dr true when the drop should be given\n",
            "#dex if rollDrop({_a}, 10):\n",
            "#dex \tsend \"yes\"\n",
            "#ddep use roll() instead\n",
            "function rollDrop(p: player, chance: number) :: boolean:\n",
        ))
        .expect("a doc comment");
        assert_eq!(found.description, "Rolls a drop chance.");
        assert_eq!(found.param("chance"), Some("percent chance"));
        assert_eq!(found.params[0].name_range, Range::on_line(2, 4, 5));
        assert_eq!(
            found.returns.as_deref(),
            Some("true when the drop should be given")
        );
        assert_eq!(
            found.examples,
            vec!["if rollDrop({_a}, 10):\n\tsend \"yes\""]
        );
        assert_eq!(found.deprecated.as_deref(), Some("use roll() instead"));
    }

    #[test]
    fn a_blank_line_breaks_the_run() {
        assert_eq!(doc("# Unrelated file header.\n\nfunction f():\n"), None);
        let found = doc("# Header.\n\n# Real doc.\nfunction f():\n").unwrap();
        assert_eq!(found.description, "Real doc.");
    }

    #[test]
    fn suppressor_lines_and_banners_are_not_prose() {
        let found = doc("# ======\n# Does a thing.\n#-# keep\nfunction f():\n").unwrap();
        assert_eq!(found.description, "Does a thing.");
    }

    #[test]
    fn a_deeper_comment_belongs_to_the_body_above() {
        let found = doc("function a():\n\tstop\n\t# end of a\nfunction b():\n");
        assert_eq!(found, None);
    }

    #[test]
    fn a_closing_block_comment_directly_above_counts() {
        let found = doc(concat!(
            "###\n",
            "Teleports home.\n",
            "@param p who\n",
            "###\n",
            "function home(p: player):\n",
        ))
        .unwrap();
        assert_eq!(found.description, "Teleports home.");
        assert_eq!(found.param("p"), Some("who"));
        assert_eq!((found.range.start.line, found.range.end.line), (0, 3));
    }

    #[test]
    fn a_hash_run_does_not_reach_into_a_block_above_it() {
        let found = doc("###\nold code\n###\n# The doc.\nfunction f():\n").unwrap();
        assert_eq!(found.description, "The doc.");
    }

    #[test]
    fn unknown_tags_stay_text_and_tags_continue_onto_the_next_line() {
        let found = doc(concat!(
            "# Opens the shop.\n",
            "# @author Daisy\n",
            "# @param p the player\n",
            "#   who opened it\n",
            "# @example\n",
            "#   openShop(player)\n",
            "#   openShop(all players)\n",
            "function openShop(p: player):\n",
        ))
        .unwrap();
        assert_eq!(found.description, "Opens the shop.\n@author Daisy");
        assert_eq!(found.param("P"), Some("the player who opened it"));
        assert_eq!(
            found.examples,
            vec!["openShop(player)\nopenShop(all players)"]
        );
    }

    #[test]
    fn a_bare_deprecated_tag_is_still_a_deprecation() {
        let found = doc("# @deprecated\nfunction f():\n").unwrap();
        assert_eq!(found.deprecated.as_deref(), Some(""));
    }

    #[test]
    fn a_method_reads_its_own_indented_comment() {
        let found = doc("class A:\n\t# Resets it.\n\tpublic reset():\n").unwrap();
        assert_eq!(found.description, "Resets it.");
    }
}
