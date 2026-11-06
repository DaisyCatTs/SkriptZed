//! What a doc comment says that the code contradicts.
//!
//! * an `@param` naming no parameter of its function or method, which is how
//!   documentation drifts when a parameter is renamed;
//! * a call to a function or method documented `@deprecated`.
//!
//! Both are hints: neither stops a script loading. Commands and skript-reflect
//! syntax are left out of the `@param` check, because their arguments have no
//! names to compare against.

use skript_index::{Document, SymbolKind, Workspace};

use super::{Diagnostic, Options, Severity};
use crate::navigation::member_definitions;

pub(super) const UNKNOWN_PARAM: &str = "doc-unknown-param";
/// Shared with the catalog's deprecation check on purpose: it is what gives the
/// call a strikethrough (`DiagnosticTag::DEPRECATED`), and the same
/// `deprecatedSyntaxDiagnostics` switch turns both off.
const DEPRECATED: &str = "deprecated-syntax";

pub(super) fn check(
    document: &Document,
    workspace: &Workspace,
    options: Options,
    out: &mut Vec<Diagnostic>,
) {
    unknown_params(document, out);
    if options.deprecated_syntax {
        deprecated_calls(document, workspace, out);
    }
}

fn unknown_params(document: &Document, out: &mut Vec<Diagnostic>) {
    let symbols = document.symbols();
    for symbol in symbols.flat() {
        if !matches!(
            symbol.kind,
            SymbolKind::Function | SymbolKind::LocalFunction | SymbolKind::Method
        ) {
            continue;
        }
        let Some(doc) = symbols.doc_for(symbol.selection_range) else {
            continue;
        };
        for param in &doc.params {
            let declared = symbol
                .children
                .iter()
                .any(|child| child.name.eq_ignore_ascii_case(&param.name));
            if declared {
                continue;
            }
            out.push(Diagnostic {
                range: param.name_range,
                severity: Severity::Hint,
                message: format!(
                    "`@param {}` names no parameter of `{}`",
                    param.name, symbol.name
                ),
                code: UNKNOWN_PARAM,
                related: Vec::new(),
            });
        }
    }
}

/// A call is deprecated only when every declaration it can mean is: a
/// deprecated overload beside a current one, or a ClassySK method that only
/// one class deprecates, says nothing certain about this call.
fn deprecated_calls(document: &Document, workspace: &Workspace, out: &mut Vec<Diagnostic>) {
    for reference in &document.symbols().references {
        if !matches!(reference.kind, SymbolKind::Function | SymbolKind::Method) {
            continue;
        }
        let definitions = member_definitions(workspace, document, reference);
        let notes: Option<Vec<&str>> = definitions
            .iter()
            .map(|(target, symbol)| {
                target
                    .symbols()
                    .doc_for(symbol.selection_range)
                    .and_then(|doc| doc.deprecated.as_deref())
            })
            .collect();
        let Some(note) = notes.and_then(|notes| notes.first().copied()) else {
            continue;
        };
        let message = if note.is_empty() {
            "deprecated".to_string()
        } else {
            format!("deprecated: {note}")
        };
        out.push(Diagnostic {
            range: reference.name_range,
            severity: Severity::Hint,
            message,
            code: DEPRECATED,
            related: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(files: &[(&str, &str)]) -> Vec<(u32, &'static str, String)> {
        let mut workspace = Workspace::new();
        for (uri, text) in files {
            workspace.open(*uri, *text);
        }
        let document = workspace.get(files[0].0).unwrap();
        let mut out = Vec::new();
        check(document, &workspace, Options::default(), &mut out);
        out.sort_by_key(|d| d.range.start);
        out.into_iter()
            .map(|d| (d.range.start.line, d.code, d.message))
            .collect()
    }

    #[test]
    fn an_at_param_naming_no_parameter_is_a_hint() {
        let hints = found(&[(
            "file:///t.sk",
            concat!(
                "# @param p the player\n",
                "# @param amount renamed since\n",
                "function give(p: player, count: number):\n",
                "\tstop\n",
            ),
        )]);
        assert_eq!(
            hints,
            vec![(
                1,
                UNKNOWN_PARAM,
                "`@param amount` names no parameter of `give`".into()
            )]
        );
    }

    /// A command's arguments are unnamed, so its `@param`s cannot be stale.
    #[test]
    fn commands_are_not_checked_for_params() {
        let hints = found(&[(
            "file:///t.sk",
            "# @param target who\ncommand /tp <player>:\n\ttrigger:\n\t\tstop\n",
        )]);
        assert!(hints.is_empty(), "{hints:?}");
    }

    #[test]
    fn a_call_to_a_deprecated_function_is_tagged_across_files() {
        let hints = found(&[
            ("file:///use.sk", "on join:\n\told(player)\n\tnew(player)\n"),
            (
                "file:///lib.sk",
                concat!(
                    "# @deprecated use new() instead\n",
                    "function old(p: player):\n",
                    "\tstop\n",
                    "function new(p: player):\n",
                    "\tstop\n",
                ),
            ),
        ]);
        assert_eq!(
            hints,
            vec![(1, DEPRECATED, "deprecated: use new() instead".into())]
        );
    }

    #[test]
    fn a_bare_tag_in_either_dialect_deprecates_without_a_reason() {
        for tag in ["#ddep", "# @deprecated"] {
            let source = format!("{tag}\nfunction old():\n\tstop\non join:\n\told()\n");
            let hints = found(&[("file:///t.sk", &source)]);
            assert_eq!(hints, vec![(4, DEPRECATED, "deprecated".into())], "{tag}");
        }
    }

    /// One class deprecating `reset` says nothing about `{_x}::reset()`,
    /// whose receiver could be the other class.
    #[test]
    fn a_method_is_deprecated_only_when_every_candidate_is() {
        let source = concat!(
            "class A:\n",
            "\t# @deprecated\n",
            "\tpublic reset():\n",
            "\t\tstop\n",
            "class B:\n",
            "\tpublic reset():\n",
            "\t\tstop\n",
            "on join:\n",
            "\t{_x}::reset()\n",
            "\tA::reset()\n",
        );
        let hints = found(&[("file:///t.sk", source)]);
        assert_eq!(hints, vec![(9, DEPRECATED, "deprecated".into())]);
    }

    /// `docs.sk` promises exactly two hints and a clean parse.
    #[test]
    fn the_sample_project_doc_file_reports_what_it_says() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/sample-project/docs.sk");
        let text = std::fs::read_to_string(path).expect("examples/sample-project/docs.sk");
        let mut workspace = Workspace::new();
        workspace.open("file:///docs.sk", text);
        let document = workspace.get("file:///docs.sk").unwrap();
        assert!(!document.has_errors());
        let mut out = Vec::new();
        check(document, &workspace, Options::default(), &mut out);
        let codes: Vec<&str> = out.iter().map(|d| d.code).collect();
        assert_eq!(codes, vec![UNKNOWN_PARAM, DEPRECATED], "{out:?}");
        let documented = document.symbols().docs.len();
        assert_eq!(documented, 4, "welcomeKit, rollDrop, oldKit and /kit");
    }

    #[test]
    fn the_deprecation_switch_turns_the_call_hint_off() {
        let mut workspace = Workspace::new();
        workspace.open(
            "file:///t.sk",
            "# @deprecated\nfunction old():\n\tstop\non join:\n\told()\n",
        );
        let document = workspace.get("file:///t.sk").unwrap();
        let mut out = Vec::new();
        let options = Options {
            deprecated_syntax: false,
            ..Options::default()
        };
        check(document, &workspace, options, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}
