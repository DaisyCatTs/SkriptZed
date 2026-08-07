//! Diagnostics.
//!
//! The brief asked for warnings that are *informative rather than noisy*, and
//! for Skript that constraint does real work: any addon can register syntax we
//! have never heard of, so "this line matched no known pattern" is a hint that
//! must default to **off**. Turning it on by default would light up every
//! script on every server that runs SkBee or DiSky.
//!
//! What is reported instead is only what is certainly wrong: indentation Skript
//! itself would reject, calls to functions that do not exist anywhere in the
//! workspace, duplicate declarations, and syntax upstream has marked deprecated.

use skript_docs::Catalog;
use skript_index::{Document, Position, Range, SymbolKind, Workspace};

/// Severity, mirroring LSP's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub code: &'static str,
    /// Other places worth looking at to understand this diagnostic.
    ///
    /// For a duplicate declaration the fix is almost never "edit this line" —
    /// it is "go and look at the other one". A client renders these as
    /// clickable links, so the range has to be somewhere useful, not a
    /// restatement of `range`.
    pub related: Vec<Related>,
}

/// A second location a diagnostic refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub range: Range,
    pub message: String,
}

/// What the user has switched on.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Report lines that match no known pattern. Off by default — see above.
    pub unknown_syntax: bool,
    /// Report syntax upstream has marked deprecated.
    pub deprecated_syntax: bool,
    /// Whether the project index has finished reading the folder.
    ///
    /// Not a user setting. "This function does not exist" is only true once
    /// every script has been looked at, and an editor opens its buffer long
    /// before that — so until this is set, the check is skipped rather than
    /// answered wrongly. Claiming a function is missing and taking it back a
    /// second later is worse than saying nothing for a second.
    pub project_indexed: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            unknown_syntax: false,
            deprecated_syntax: true,
            project_indexed: false,
        }
    }
}

/// Produces diagnostics for one document.
pub fn check(
    document: &Document,
    workspace: &Workspace,
    catalog: Option<&Catalog>,
    // `uninstalled` carries syntax from addons the server does not have. It is
    // `None` whenever the environment is unknown, which is what keeps
    // `requires-addon` from firing on a guess.
    uninstalled: Option<&Catalog>,
    options: Options,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    check_indentation(document, &mut out);
    check_block_comments(document, &mut out);
    check_duplicate_declarations(document, &mut out);
    if options.project_indexed {
        check_unknown_functions(document, workspace, &mut out);
    }

    if let Some(catalog) = catalog {
        check_catalog(document, catalog, uninstalled, options, &mut out);
    }

    out.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start.line,
            diagnostic.range.start.character,
        )
    });
    out
}

/// Skript infers one indent unit per file from the first indented line, forbids
/// mixing tabs and spaces inside a single indent, and requires every deeper
/// line to be an exact multiple of that unit. The grammar is deliberately
/// lenient about this so a half-typed file still parses; the strictness lives
/// here, where it produces a message instead of a broken tree.
fn check_indentation(document: &Document, out: &mut Vec<Diagnostic>) {
    let mut unit: Option<&str> = None;
    let prose = block_comment_lines(document.text());

    for (number, line) in document.text().lines().enumerate() {
        // A `###` block's interior is prose, and its leading whitespace means
        // nothing. Reading it as code let a three-space comment line decide the
        // file's indent unit, which then flagged every real tab-indented line
        // as inconsistent. `skript-format` already treats these lines as
        // verbatim; the two must agree.
        if prose.contains(&number) {
            continue;
        }

        let indent_len = line.len() - line.trim_start().len();
        if indent_len == 0 || line.trim().is_empty() {
            continue;
        }
        // A comment line's indentation is not the script's indentation. Skript
        // strips comments before it looks at layout at all, so aligning one
        // with spaces in an otherwise tab-indented file is legal — and common,
        // because editors and pasted snippets do it constantly. Letting such a
        // line set the unit made the *code* below it report
        // "uses tabs here but spaces earlier in the file", which is both wrong
        // and impossible to act on: the line it points at is correct.
        //
        // `###` prose is already skipped above for the same reason; ordinary
        // `#` lines were missed.
        if line.trim_start().starts_with('#') {
            continue;
        }
        let indent = &line[..indent_len];
        let range = Range::new(
            Position::new(number as u32, 0),
            Position::new(number as u32, indent_len as u32),
        );

        if indent.contains(' ') && indent.contains('\t') {
            out.push(Diagnostic {
                range,
                severity: Severity::Error,
                message: "Indentation mixes tabs and spaces. Skript requires one or the other \
                          within a single indent."
                    .into(),
                code: "mixed-indentation",
                related: Vec::new(),
            });
            continue;
        }

        match unit {
            None => unit = Some(indent),
            Some(unit) => {
                let same_kind = unit.starts_with(' ') == indent.starts_with(' ');
                if !same_kind {
                    out.push(Diagnostic {
                        range,
                        severity: Severity::Error,
                        message: format!(
                            "Indentation uses {} here but {} earlier in the file. Skript infers \
                             one indent unit per script.",
                            if indent.starts_with(' ') {
                                "spaces"
                            } else {
                                "tabs"
                            },
                            if unit.starts_with(' ') {
                                "spaces"
                            } else {
                                "tabs"
                            },
                        ),
                        code: "inconsistent-indentation",
                        related: Vec::new(),
                    });
                } else if indent_len % unit.len() != 0 {
                    out.push(Diagnostic {
                        range,
                        severity: Severity::Error,
                        message: format!(
                            "Indentation is {indent_len} characters, which is not a multiple of \
                             this script's indent unit of {}.",
                            unit.len()
                        ),
                        code: "indent-not-a-multiple",
                        related: Vec::new(),
                    });
                }
            }
        }
    }
}

/// A `###` block comment that is never closed swallows the rest of the file.
fn check_block_comments(document: &Document, out: &mut Vec<Diagnostic>) {
    let mut open_at: Option<u32> = None;

    for (number, line) in document.text().lines().enumerate() {
        if line.trim() != "###" {
            continue;
        }
        open_at = match open_at {
            Some(_) => None,
            None => Some(number as u32),
        };
    }

    if let Some(line) = open_at {
        out.push(Diagnostic {
            range: Range::new(Position::new(line, 0), Position::new(line, 3)),
            severity: Severity::Error,
            message: "This block comment is never closed. Add a line containing only `###`.".into(),
            code: "unclosed-block-comment",
            related: Vec::new(),
        });
    }
}

fn check_duplicate_declarations(document: &Document, out: &mut Vec<Diagnostic>) {
    let mut seen: Vec<((SymbolKind, String), Range)> = Vec::new();

    for symbol in document.symbols().flat() {
        if !matches!(
            symbol.kind,
            SymbolKind::Function
                | SymbolKind::LocalFunction
                | SymbolKind::Command
                | SymbolKind::Option
        ) {
            continue;
        }
        let key = (symbol.kind, symbol.name.clone());
        if let Some((_, first)) = seen.iter().find(|(other, _)| *other == key) {
            out.push(Diagnostic {
                range: symbol.selection_range,
                severity: Severity::Error,
                message: format!(
                    "`{}` is declared more than once in this script.",
                    symbol.name
                ),
                code: "duplicate-declaration",
                related: vec![Related {
                    range: *first,
                    message: format!("`{}` is first declared here.", symbol.name),
                }],
            });
        } else {
            seen.push((key, symbol.selection_range));
        }
    }
}

/// Whether a call is a skript-reflect constructor rather than a Skript call.
///
/// skript-reflect writes `new ArrayList()`, which the index reads as a call to
/// a function named `ArrayList`. Nothing will ever declare that function, so
/// flagging it produces an error the user cannot fix and must ignore — the
/// worst kind.
fn preceded_by_new(document: &Document, reference: &skript_index::Reference) -> bool {
    let line = document.line(reference.range.start.line);
    let before = &line[..(reference.range.start.character as usize).min(line.len())];
    before.trim_end().ends_with("new")
}

fn check_unknown_functions(document: &Document, workspace: &Workspace, out: &mut Vec<Diagnostic>) {
    // Gathered once. Calling `Workspace::definitions` per reference walked every
    // indexed document and rebuilt its whole symbol tree each time, so a file
    // with N calls cost N full-workspace scans on every keystroke.
    let mut declared: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for other in workspace.documents() {
        let same_file = other.uri() == document.uri();
        for symbol in other.symbols().flat() {
            if !symbol.kind.is_function() {
                continue;
            }
            // A `local function` is invisible outside the file declaring it.
            if symbol.kind == SymbolKind::LocalFunction && !same_file {
                continue;
            }
            declared.insert(symbol.name.as_str());
        }
    }

    for reference in &document.symbols().references {
        if reference.kind != SymbolKind::Function {
            continue;
        }
        // `new ArrayList()` is skript-reflect constructing a Java object, not a
        // call to a Skript function — and no Skript function will ever be
        // declared for it, so the diagnostic can never be acted on. Reported
        // from real use: a script full of legitimate skript-reflect lit up red.
        //
        // Matched on the text before the name rather than on the name's shape,
        // because `new` is the only thing that distinguishes a constructor;
        // capitalisation is a convention Skript does not enforce.
        if preceded_by_new(document, reference) {
            continue;
        }
        if !declared.contains(reference.name.as_str()) {
            out.push(Diagnostic {
                range: reference.range,
                severity: Severity::Error,
                message: format!(
                    "No function named `{}` is declared in this project.",
                    reference.name
                ),
                code: "unknown-function",
                related: Vec::new(),
            });
        }
    }
}

/// Catalog-backed checks: deprecation, and optionally unrecognised syntax.
/// Lines that declare a structure entry rather than run a statement.
///
/// Taken from the index rather than re-derived from the text: the parse tree
/// already distinguishes `description: Go home` inside a `command` from a line
/// of the same shape inside a trigger, and guessing from a `key: value` shape
/// would misfire on ordinary effects that contain a colon.
fn structure_entry_lines(document: &Document) -> std::collections::HashSet<u32> {
    document
        .symbols()
        .flat()
        .into_iter()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Entry
                    | SymbolKind::Option
                    | SymbolKind::Alias
                    | SymbolKind::GlobalVariable
                    | SymbolKind::LocalVariable
            )
        })
        .map(|symbol| symbol.range.start.line)
        .collect()
}

/// Line numbers inside `###` block comments, delimiters excluded.
fn block_comment_lines(text: &str) -> std::collections::HashSet<usize> {
    let mut inside = false;
    let mut lines = std::collections::HashSet::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim() == "###" {
            inside = !inside;
            continue;
        }
        if inside {
            lines.insert(number);
        }
    }
    lines
}

fn check_catalog(
    document: &Document,
    catalog: &Catalog,
    uninstalled: Option<&Catalog>,
    options: Options,
    out: &mut Vec<Diagnostic>,
) {
    let prose = block_comment_lines(document.text());
    let entries = structure_entry_lines(document);

    for (number, raw) in document.text().lines().enumerate() {
        let trimmed = raw.trim();
        // Prose inside a `###` block is not syntax and must never be reported
        // as unrecognised.
        if trimmed.is_empty() || trimmed.starts_with('#') || prose.contains(&number) {
            continue;
        }
        // A structure's entries are indented, but they are not statements:
        // `description:` in a command, `prefix:` in `options:`, `{score::*} = 0`
        // in `variables:`. None of them can match an effect, section or
        // condition, so once the role filter stopped the catch-all expressions
        // absorbing them, every one became an "unknown syntax" hint. The parse
        // tree already knows which lines these are.
        if entries.contains(&(number as u32)) {
            continue;
        }
        let code = trimmed.trim_end_matches(':');
        let indent = (raw.len() - raw.trim_start().len()) as u32;
        // Indentation says whether this line opens a structure or sits inside
        // one, which rules out the categories that could never explain it.
        // Without that filter the three catch-all expressions match every line
        // ever written and nothing is reportable as unknown.
        let role = skript_docs::LineRole::from_indent(indent as usize);
        let range = Range::new(
            Position::new(number as u32, indent),
            Position::new(number as u32, raw.trim_end().len() as u32),
        );

        match catalog.classify_line(code, role) {
            Some((id, _)) => {
                if !options.deprecated_syntax {
                    continue;
                }
                let Some(entry) = catalog.entry(id) else {
                    continue;
                };
                if entry.is_deprecated() {
                    let mut message = format!("`{}` is deprecated.", entry.name);
                    if let Some(note) = entry.deprecated.note() {
                        message.push(' ');
                        message.push_str(&note);
                    }
                    out.push(Diagnostic {
                        range,
                        severity: Severity::Warning,
                        message,
                        code: "deprecated-syntax",
                        related: Vec::new(),
                    });
                }
            }
            None => {
                // A line we cannot place might belong to an addon the server
                // does not have. That is a far more useful thing to say than
                // "unknown syntax", and it is only sayable because detection
                // told us what is actually installed.
                let from_addon = uninstalled
                    .and_then(|rest| rest.classify_line(code, role))
                    .and_then(|(id, _)| rest_entry_addon(uninstalled?, id));

                if let Some((addon, since)) = from_addon {
                    let version = since.map(|v| format!(" {v} or newer")).unwrap_or_default();
                    out.push(Diagnostic {
                        range,
                        severity: Severity::Warning,
                        message: format!(
                            "This is `{addon}` syntax, and `{addon}`{version} is not installed on \
                             this server."
                        ),
                        code: "requires-addon",
                        related: Vec::new(),
                    });
                } else if options.unknown_syntax {
                    out.push(Diagnostic {
                        range,
                        severity: Severity::Hint,
                        message: "This line does not match any known syntax. If it comes from an \
                                  addon, add it to your server or list it in `addons`."
                            .into(),
                        code: "unknown-syntax",
                        related: Vec::new(),
                    });
                }
            }
        }
    }
}

/// The addon an uninstalled-catalog entry belongs to.
fn rest_entry_addon(
    catalog: &Catalog,
    id: skript_docs::EntryId,
) -> Option<(String, Option<String>)> {
    let entry = catalog.entry(id)?;
    let addon = entry.addon.as_ref()?;
    Some((addon.name.clone(), addon.since_version.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_source(source: &str) -> Vec<Diagnostic> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        // Borrowing the document out of the workspace it lives in is fine here
        // because nothing mutates during the check.
        let snapshot = Workspace::new();
        let _ = &snapshot;
        check(
            document,
            &workspace,
            None,
            None,
            // The workspace here *is* the whole project, and it is fully built
            // before the check runs — the startup race this flag guards against
            // cannot happen in a unit test.
            Options {
                project_indexed: true,
                ..Options::default()
            },
        )
    }

    fn codes(source: &str) -> Vec<&'static str> {
        check_source(source).into_iter().map(|d| d.code).collect()
    }

    #[test]
    fn accepts_ordinary_tab_indented_script() {
        assert!(codes("on join:\n\tsend \"hi\"\n\tif {_x} is set:\n\t\tstop\n").is_empty());
    }

    #[test]
    fn accepts_ordinary_space_indented_script() {
        assert!(codes("on join:\n    send \"hi\"\n        stop\n").is_empty());
    }

    #[test]
    fn rejects_tabs_and_spaces_in_one_indent() {
        assert!(codes("on join:\n \tsend \"hi\"\n").contains(&"mixed-indentation"));
    }

    #[test]
    fn rejects_switching_indent_character_mid_file() {
        let found = codes("on join:\n\tsend \"a\"\non quit:\n    send \"b\"\n");
        assert!(found.contains(&"inconsistent-indentation"));
    }

    #[test]
    fn rejects_an_indent_that_is_not_a_multiple_of_the_unit() {
        let found = codes("on join:\n    send \"a\"\n      send \"b\"\n");
        assert!(found.contains(&"indent-not-a-multiple"));
    }

    /// Reported from real use: an otherwise tab-indented script showed
    /// "uses tabs here but spaces earlier in the file" on correct lines,
    /// because a comment above them happened to be aligned with spaces.
    /// Skript strips comments before it looks at layout, so their indentation
    /// is not the script's.
    #[test]
    fn a_comment_indented_differently_is_not_an_error() {
        let found = codes(
            "on join:
    # aligned with spaces
	send \"hi\"
",
        );
        assert!(
            !found.contains(&"inconsistent-indentation"),
            "a comment must not set the file's indent unit, got {found:?}"
        );
    }

    #[test]
    fn real_mixing_is_still_reported_around_a_comment() {
        // The comment is skipped, but the two *code* lines still disagree.
        let found = codes(
            "on join:
    send \"a\"
	# note
	send \"b\"
",
        );
        assert!(found.contains(&"inconsistent-indentation"));
    }

    #[test]
    fn reports_an_unclosed_block_comment() {
        assert!(codes("###\nnever closed\n").contains(&"unclosed-block-comment"));
        assert!(!codes("###\nclosed\n###\n").contains(&"unclosed-block-comment"));
    }

    #[test]
    fn reports_duplicate_functions_and_commands() {
        let found = codes("function a():\n\tstop\n\nfunction a():\n\tstop\n");
        assert!(found.contains(&"duplicate-declaration"));
    }

    #[test]
    fn a_duplicate_points_at_the_first_declaration() {
        let source = "function a():\n\tstop\n\nfunction a():\n\tstop\n";
        let duplicate = check_source(source)
            .into_iter()
            .find(|d| d.code == "duplicate-declaration")
            .expect("the duplicate is reported");

        let related = duplicate
            .related
            .first()
            .expect("it says where the first one is");
        // Line 3, not line 0: the report sits on the second declaration and the
        // link goes back to the first, which is the one the user has to look at.
        assert_eq!(duplicate.range.start.line, 3);
        assert_eq!(related.range.start.line, 0);
    }

    /// Reported from real use: a script full of legitimate skript-reflect
    /// showed an unfixable error on every Java constructor.
    #[test]
    fn a_java_constructor_is_not_a_missing_skript_function() {
        let found = codes(
            "on join:
	set {_list} to new ArrayList()
",
        );
        assert!(
            !found.contains(&"unknown-function"),
            "`new X()` is skript-reflect, not a Skript call, got {found:?}"
        );
    }

    #[test]
    fn reports_a_call_to_a_function_that_does_not_exist() {
        let found = codes("on join:\n\tset {_x} to missing_function()\n");
        assert!(found.contains(&"unknown-function"));
    }

    #[test]
    fn accepts_a_call_to_a_function_declared_in_the_same_file() {
        let found = codes("function helper():\n\tstop\n\non join:\n\tset {_x} to helper()\n");
        assert!(!found.contains(&"unknown-function"));
    }

    #[test]
    fn unknown_syntax_is_off_by_default() {
        // Any addon can register syntax we do not know; defaulting this on
        // would flood every real server's scripts.
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", "on join:\n\tsome addon effect here\n");
        let document = workspace.get("file:///t.sk").unwrap();
        let catalog = Catalog::build(skript_docs::fallback_docs());

        let quiet = check(
            document,
            &workspace,
            Some(&catalog),
            None,
            Options::default(),
        );
        assert!(!quiet.iter().any(|d| d.code == "unknown-syntax"));

        let loud = check(
            document,
            &workspace,
            Some(&catalog),
            None,
            Options {
                unknown_syntax: true,
                ..Options::default()
            },
        );
        assert!(loud.iter().any(|d| d.code == "unknown-syntax"));
    }

    #[test]
    fn diagnostics_come_back_in_document_order() {
        let found = check_source("on join:\n \tbad indent\n\tset {_x} to nope()\n");
        for pair in found.windows(2) {
            assert!(pair[0].range.start <= pair[1].range.start);
        }
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;

    fn codes_for(source: &str) -> Vec<&'static str> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        check(document, &workspace, None, None, Options::default())
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn block_comment_prose_does_not_set_the_indent_unit() {
        // Regression: the three-space prose line made the file "space
        // indented", so the tab-indented prose line and every real tab-indented
        // line after it were flagged. This is skript-format's own test fixture.
        let found =
            codes_for("###\n   deliberately indented prose\n\tand a tab\n###\non join:\n\tstop\n");
        assert!(
            found.is_empty(),
            "prose inside a block comment was read as code: {found:?}"
        );
    }

    #[test]
    fn real_indentation_errors_are_still_caught_around_a_block_comment() {
        let found = codes_for("###\n   prose\n###\non join:\n \tmixed\n");
        assert!(found.contains(&"mixed-indentation"), "got {found:?}");
    }
}

#[cfg(test)]
mod entry_line_regressions {
    use super::*;

    /// With `unknown_syntax` on, a command's entries and an `options:` block
    /// must produce no hints. They are indented, so the role filter classifies
    /// them as statements — and no entry can ever match an effect, section or
    /// condition, which made the setting unusable on any real script.
    #[test]
    fn structure_entries_are_not_unknown_syntax() {
        let source = "options:
	prefix: &6[Server]&r

aliases:
	stones = stone, granite

variables:
	{score::*} = 0

command /home:
	description: Go home
	permission: skript.home
	usage: /home
	trigger:
		stop
";

        let catalog = skript_docs::Catalog::build(skript_docs::fallback_docs());
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();

        let found = check(
            document,
            &workspace,
            Some(&catalog),
            None,
            Options {
                unknown_syntax: true,
                ..Options::default()
            },
        );

        let flagged: Vec<u32> = found
            .iter()
            .filter(|d| d.code == "unknown-syntax")
            .map(|d| d.range.start.line)
            .collect();

        assert!(
            flagged.is_empty(),
            "structure entries reported as unknown syntax on lines {flagged:?}"
        );
    }
}
