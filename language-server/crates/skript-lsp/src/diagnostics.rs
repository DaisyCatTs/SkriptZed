//! Diagnostics.
//!
//! Only what is certainly wrong is reported by default:
//!
//! * indentation Skript itself would reject (mixed tabs and spaces, a unit
//!   that changes inside one structure, a depth that is not a multiple of it);
//! * an unclosed `###` block comment;
//! * a function, command or option declared twice in one script;
//! * a call to a function declared nowhere in the project or the catalog;
//! * a call with more arguments than its function takes, or fewer than it
//!   needs ([`arity`]);
//! * an `{@option}` the script's `options:` block never declares;
//! * syntax upstream has marked deprecated;
//! * syntax that belongs to an addon the server does not have installed
//!   (`requires-addon`), when its `plugins/` folder is known.
//!
//! Warnings and hints cover what is wrong as far as a careful reading can
//! tell, and say nothing when it cannot:
//!
//! * `cancel event` in an event that cannot be cancelled ([`cancel`]);
//! * a local variable read but never set, or set but never read, in one
//!   trigger ([`locals`]).
//!
//! "This line matches no known pattern" is opt-in. Any addon can register
//! syntax the catalog has never heard of, so turning it on by default would
//! light up every script on every server that runs SkBee or DiSky.

use skript_docs::Catalog;
use skript_index::{Document, Position, Range, SymbolKind, Workspace};

use crate::lines;

mod arity;
mod cancel;
mod layout;
mod locals;

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

impl Diagnostic {
    /// Whether a client should render the range as dead code: faded, and
    /// left out of the problems count by editors that honour the tag.
    pub fn is_unnecessary(&self) -> bool {
        self.code == locals::NEVER_READ
    }
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
    /// Report locals that are read but never set, or set but never read.
    pub local_variables: bool,
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
            local_variables: true,
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
    check_unknown_options(document, &mut out);
    if options.local_variables {
        locals::check(document, catalog, &mut out);
    }
    if options.project_indexed {
        check_unknown_functions(document, workspace, catalog, &mut out);
        // Another file may declare an overload with a different arity.
        arity::check(document, workspace, catalog, &mut out);
    }

    if let Some(catalog) = catalog {
        check_catalog(document, catalog, uninstalled, options, &mut out);
        cancel::check(document, catalog, &mut out);
    }

    out.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start.line,
            diagnostic.range.start.character,
        )
    });
    out
}

/// Skript infers one indent unit per **top-level structure** from that
/// structure's first indented line, forbids
/// mixing tabs and spaces inside a single indent, and requires every deeper
/// line to be an exact multiple of that unit. The grammar is deliberately
/// lenient about this so a half-typed file still parses; the strictness lives
/// here, where it produces a message instead of a broken tree.
fn check_indentation(document: &Document, out: &mut Vec<Diagnostic>) {
    let mut unit: Option<&str> = None;
    let prose = lines::block_comment_lines(document.text());

    for (number, line) in document.text().lines().enumerate() {
        // A `###` block's interior is prose, and its leading whitespace means
        // nothing. Reading it as code let a three-space comment line decide the
        // file's indent unit, which then flagged every real tab-indented line
        // as inconsistent. `skript-format` already treats these lines as
        // verbatim; the two must agree.
        if prose.contains(&number) {
            continue;
        }

        if line.trim().is_empty() {
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

        let indent_len = skript_index::indent_width(line);
        if indent_len == 0 {
            // A new top-level structure begins, and Skript infers the indent
            // unit **per structure**, not per file — reported as issue #2 by
            // someone whose 99 working scripts we were flagging.
            //
            // Skript's own test suite settles it: 7 of its 540 shipped scripts
            // mix tab- and space-indentation between structures, and every one
            // of them is consistent *within* each structure. Those files load,
            // so a per-file rule is stricter than the language.
            unit = None;
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
                            "Indentation uses {} here but {} earlier in this structure. Skript \
                             infers one indent unit per structure, so mixing between structures \
                             is fine, but mixing inside one is not.",
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
                } else if !indent_len.is_multiple_of(unit.len()) {
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

/// `{@name}` with no `name:` in this script's `options:` block.
///
/// Certain, unlike most "unknown" checks: options belong to the script that
/// declares them — no addon, no other file and no runtime state can supply
/// one — so a missing declaration is always a real error.
///
/// Nested options (Skript 2.7+) are written `{@parent.child}`, and the index
/// records only `parent`; a dotted name counts as declared when its first part
/// is. Names compare case-insensitively, which can only make this quieter.
fn check_unknown_options(document: &Document, out: &mut Vec<Diagnostic>) {
    let declared: std::collections::HashSet<String> = document
        .symbols()
        .flat()
        .into_iter()
        .filter(|symbol| symbol.kind == SymbolKind::Option)
        .map(|symbol| symbol.name.trim().to_ascii_lowercase())
        .collect();

    for reference in &document.symbols().references {
        if reference.kind != SymbolKind::Option {
            continue;
        }
        let name = reference.name.trim().to_ascii_lowercase();
        let root = name.split('.').next().unwrap_or(&name);
        if declared.contains(&name) || declared.contains(root) {
            continue;
        }
        out.push(Diagnostic {
            range: reference.range,
            severity: Severity::Error,
            message: format!(
                "No option named `{}` in this script's `options:` block.",
                reference.name.trim()
            ),
            code: "unknown-option",
            related: Vec::new(),
        });
    }
}

fn check_unknown_functions(
    document: &Document,
    workspace: &Workspace,
    catalog: Option<&Catalog>,
    out: &mut Vec<Diagnostic>,
) {
    // Gathered once. Calling `Workspace::definitions` per reference walked every
    // indexed document and rebuilt its whole symbol tree each time, so a file
    // with N calls cost N full-workspace scans on every keystroke.
    let mut declared: std::collections::HashSet<&str> = std::collections::HashSet::new();

    // Skript ships 45 functions of its own — `floor`, `abs`, `mod`, `uuid` —
    // registered at runtime and declared in no script. Checking only the
    // workspace made every one of them an error, which is issue #3: calling
    // `floor(2.8)` reported "No function named `floor` is declared in this
    // project" on a line that is perfectly valid.
    //
    // Addon functions come through the same catalog, so a server running an
    // addon that registers functions gets them too.
    if let Some(catalog) = catalog {
        for entry in catalog.docs().entries(skript_docs::Category::Function) {
            declared.insert(entry.name.as_str());
        }
    }
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
                    // ClassySK declarations: a class header, field or method
                    // header is shaped by the grammar, not by any pattern.
                    | SymbolKind::Class
                    | SymbolKind::Field
                    | SymbolKind::Method
            )
        })
        .map(|symbol| symbol.range.start.line)
        .collect()
}

/// Catalog-backed checks: deprecation, and optionally unrecognised syntax.
///
/// Each line is judged without its comment. Matching the raw line made
/// `send "hi" # greet` unknown syntax, and hid a deprecated statement behind
/// any trailing comment.
fn check_catalog(
    document: &Document,
    catalog: &Catalog,
    uninstalled: Option<&Catalog>,
    options: Options,
    out: &mut Vec<Diagnostic>,
) {
    let entries = structure_entry_lines(document);

    // Prose inside a `###` block is not syntax and must never be reported as
    // unrecognised.
    for (number, raw) in lines::code_lines(document.text()) {
        let trimmed = lines::code_part(raw).trim();
        if trimmed.is_empty() {
            continue;
        }
        // A structure's entries are indented, but they are not statements:
        // `description:` in a command, `prefix:` in `options:`, `{score::*} = 0`
        // in `variables:`. None of them can match an effect, section or
        // condition, so once the role filter stopped the catch-all expressions
        // absorbing them, every one became an "unknown syntax" hint. The parse
        // tree already knows which lines these are.
        if entries.contains(&number)
            || crate::completion::is_documented_entry(catalog, document, number)
        {
            continue;
        }
        let code = trimmed.trim_end_matches(':');
        let indent = skript_index::indent_width(raw) as u32;
        // Indentation says whether this line opens a structure or sits inside
        // one, which rules out the categories that could never explain it.
        // Without that filter the three catch-all expressions match every line
        // ever written and nothing is reportable as unknown.
        let role = skript_docs::LineRole::from_indent(indent as usize);
        let range = Range::on_line(number, indent, indent + trimmed.len() as u32);

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
                // told us what is installed.
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
        assert!(codes("on join:\n\tsend \"hi\"\n\tif player is op:\n\t\tstop\n").is_empty());
    }

    #[test]
    fn accepts_ordinary_space_indented_script() {
        assert!(codes("on join:\n    send \"hi\"\n        stop\n").is_empty());
    }

    #[test]
    fn rejects_tabs_and_spaces_in_one_indent() {
        assert!(codes("on join:\n \tsend \"hi\"\n").contains(&"mixed-indentation"));
    }

    /// This test used to assert the opposite, and it was wrong: it encoded a
    /// per-file indent rule that Skript does not have. Issue #2 reported the
    /// resulting false errors, and Skript's own test suite settles it — 7 of
    /// its 540 shipped scripts switch indent character between structures.
    /// Kept, inverted, so the mistake cannot be reintroduced.
    #[test]
    fn switching_indent_character_between_structures_is_allowed() {
        let found = codes("on join:\n\tsend \"a\"\non quit:\n    send \"b\"\n");
        assert!(
            !found.contains(&"inconsistent-indentation"),
            "Skript infers the unit per structure, got {found:?}"
        );
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
        let found = codes("on join:\n    # aligned with spaces\n\tsend \"hi\"\n");
        assert!(
            !found.contains(&"inconsistent-indentation"),
            "a comment must not set the file's indent unit, got {found:?}"
        );
    }

    #[test]
    fn real_mixing_is_still_reported_around_a_comment() {
        // The comment is skipped, but the two *code* lines still disagree.
        let found = codes("on join:\n    send \"a\"\n\t# note\n\tsend \"b\"\n");
        assert!(found.contains(&"inconsistent-indentation"));
    }

    /// Issue #2. Skript infers the indent unit per top-level structure, not per
    /// file — 7 of the 540 scripts Skript itself ships mix tab and space
    /// indentation between structures, and they all load. A per-file rule made
    /// every one of them an error.
    #[test]
    fn each_structure_gets_its_own_indent_unit() {
        let found = codes("on join:\n\tsend \"a\"\n\non quit:\n    send \"b\"\n");
        assert!(
            !found.contains(&"inconsistent-indentation"),
            "structures may differ from each other, got {found:?}"
        );
    }

    #[test]
    fn mixing_inside_one_structure_is_still_an_error() {
        let found = codes("on join:\n\tsend \"a\"\n    send \"b\"\n");
        assert!(
            found.contains(&"inconsistent-indentation"),
            "one structure must pick a side, got {found:?}"
        );
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
        let found = codes("on join:\n\tset {_list} to new ArrayList()\n");
        assert!(
            !found.contains(&"unknown-function"),
            "`new X()` is skript-reflect, not a Skript call, got {found:?}"
        );
    }

    #[test]
    fn reports_an_option_the_script_never_declares() {
        let found = codes("options:\n\tprefix: &6[S]\n\non join:\n\tsend \"{@prefix}\" to player\n\tset {_x} to {@missing}\n");
        assert_eq!(
            found
                .iter()
                .filter(|code| **code == "unknown-option")
                .count(),
            1,
            "{found:?}"
        );
    }

    /// `{@parent.child}` is how a nested option (Skript 2.7+) is used.
    #[test]
    fn accepts_declared_and_nested_options() {
        let found = codes(
            "options:\n\tmax: 5\n\tcolors:\n\t\tmain: &a\n\non join:\n\tset {_x} to {@max}\n\tset {_c} to {@colors.main}\n",
        );
        assert!(!found.contains(&"unknown-option"), "{found:?}");
    }

    /// Reported from real use: every method in a ClassySK script — its
    /// declaration, `self::start()`, `{_bar}::play(…)` — was flagged as an
    /// undeclared Skript function.
    #[test]
    fn a_classysk_method_is_not_a_missing_skript_function() {
        let found = codes(concat!(
            "class Bar:\n",
            "\tpublic play(text: string):\n",
            "\t\tself::start()\n",
            "\tpublic start():\n",
            "\t\tstop\n",
            "on join:\n",
            "\t{_bar}::play(\"hi\")\n",
            "\tBar::start()\n",
        ));
        assert!(
            !found.contains(&"unknown-function"),
            "a method is not a Skript function, got {found:?}"
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

    const DEPRECATED_SEND: &str = r#"{
      "effects": [
        { "id": "t.send", "name": "Send", "patterns": ["send %string%"], "deprecated": "use tell" }
      ]
    }"#;

    fn catalog_codes(source: &str, options: Options) -> Vec<(&'static str, Range)> {
        let catalog = Catalog::build(skript_docs::Docs::parse(DEPRECATED_SEND).unwrap());
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        check(document, &workspace, Some(&catalog), None, options)
            .into_iter()
            .map(|d| (d.code, d.range))
            .collect()
    }

    /// The catalog check never stripped comments, so a trailing one made a
    /// known line unknown, and hid it from the deprecation check.
    #[test]
    fn a_trailing_comment_does_not_hide_the_statement() {
        let options = Options {
            unknown_syntax: true,
            ..Options::default()
        };
        let found = catalog_codes("on join:\n\tsend \"hi\" # greet\n", options);
        // This tiny catalog has no `on join` event, so only line 1 matters.
        let codes: Vec<&str> = found
            .iter()
            .filter(|(_, range)| range.start.line == 1)
            .map(|(code, _)| *code)
            .collect();
        assert_eq!(codes, vec!["deprecated-syntax"], "{found:?}");

        // The range covers the statement, not the comment after it.
        let (_, range) = found
            .iter()
            .find(|(code, _)| *code == "deprecated-syntax")
            .unwrap();
        assert_eq!((range.start.character, range.end.character), (1, 10));
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment_to_the_catalog_check() {
        let found = catalog_codes("on join:\n\tsend \"#1\"\n", Options::default());
        let (_, range) = found
            .iter()
            .find(|(code, _)| *code == "deprecated-syntax")
            .expect("the whole statement still matches");
        assert_eq!(range.end.character, 10);
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
        let source = "options:\n\tprefix: &6[Server]&r\n\naliases:\n\tstones = stone, granite\n\nvariables:\n\t{score::*} = 0\n\ncommand /home:\n\tdescription: Go home\n\tpermission: skript.home\n\tusage: /home\n\ttrigger:\n\t\tstop\n";

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
