//! Quick fixes, and whole-file formatting.

use std::collections::HashMap;

use skript_index::{Document, SymbolKind, Workspace};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Diagnostic,
    DocumentFormattingParams, DocumentRangeFormattingParams, FormattingOptions, NumberOrString,
    Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::convert::{from_lsp_position, to_lsp_range, Encoding};
use crate::server::State;

/// Quick fixes for the diagnostics the client hands back.
///
/// No `data` round-trip is needed: `CodeActionParams` returns each diagnostic
/// with its `range` and `code` intact, which is everything these fixes need.
pub fn code_action(state: &State, params: &CodeActionParams) -> Option<Vec<CodeActionOrCommand>> {
    let uri = &params.text_document.uri;
    let document = state.workspace.get(uri.as_str())?;

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut offered_indent_fix = false;
    let line_count = document.text().lines().count() as u32;
    let end_of_file = Range::new(Position::new(line_count, 0), Position::new(line_count, 0));
    let fix = |title: &str, edits: Vec<TextEdit>, diagnostic: &Diagnostic| {
        quick_fix(title, uri.clone(), edits, diagnostic.clone())
    };

    for diagnostic in &params.context.diagnostics {
        let Some(NumberOrString::String(code)) = &diagnostic.code else {
            continue;
        };

        match code.as_str() {
            // All three indentation codes share one real fix, and the
            // formatter already knows how to produce it. One action rather
            // than three, offered once however many lines are flagged.
            "mixed-indentation" | "inconsistent-indentation" | "indent-not-a-multiple"
                if !offered_indent_fix =>
            {
                offered_indent_fix = true;
                // Indentation only: a quick fix for one problem must not
                // also restyle the file's spacing.
                let options = skript_format::Options {
                    blank_line_between_structures: false,
                    ..Default::default()
                };
                if let Some(formatted) = skript_format::format(document, options) {
                    let whole = Range::new(Position::new(0, 0), Position::new(line_count, 0));
                    actions.push(fix(
                        "Fix indentation in this file",
                        vec![TextEdit::new(whole, formatted)],
                        diagnostic,
                    ));
                }
            }

            // The whole line goes, newline included, so no blank line is left
            // where the statement was.
            "event-not-cancellable" => {
                let line = diagnostic.range.start.line;
                let whole_line = Range::new(Position::new(line, 0), Position::new(line + 1, 0));
                actions.push(fix(
                    "Remove `cancel event`",
                    vec![TextEdit::new(whole_line, String::new())],
                    diagnostic,
                ));
            }

            // The suggestions are worked out again from the text under the
            // diagnostic rather than carried in it: the catalog is right
            // here, and `CodeActionParams` hands back only range and code.
            "unknown-event" => {
                let Some(catalog) = state.catalog.as_ref() else {
                    continue;
                };
                let text = document.text();
                let start = from_lsp_position(text, diagnostic.range.start, state.encoding);
                let end = from_lsp_position(text, diagnostic.range.end, state.encoding);
                if start.line != end.line {
                    continue;
                }
                let line = document.line(start.line);
                let Some(typed) = line.get(start.character as usize..end.character as usize) else {
                    continue;
                };
                for suggestion in crate::diagnostics::event_suggestions(catalog, typed) {
                    actions.push(fix(
                        &format!("Change to `on {suggestion}`"),
                        vec![TextEdit::new(diagnostic.range, suggestion)],
                        diagnostic,
                    ));
                }
            }

            "unclosed-block-comment" => actions.push(fix(
                "Close the block comment",
                vec![TextEdit::new(end_of_file, "###\n".into())],
                diagnostic,
            )),

            "unknown-option" => {
                let Some(name) =
                    reference_name(document, diagnostic, SymbolKind::Option, state.encoding)
                else {
                    continue;
                };
                let name = name.trim();
                actions.push(fix(
                    &format!("Add `{name}` to options:"),
                    vec![add_option_edit(document, name, state.encoding)],
                    diagnostic,
                ));
            }

            "unknown-function" => {
                let Some(name) =
                    reference_name(document, diagnostic, SymbolKind::Function, state.encoding)
                else {
                    continue;
                };

                // A near-miss first: accepting a typo correction is far more
                // often what was meant than declaring a new function.
                if let Some(suggestion) = closest_function(&state.workspace, name) {
                    actions.push(fix(
                        &format!("Change to `{suggestion}`"),
                        vec![TextEdit::new(diagnostic.range, suggestion)],
                        diagnostic,
                    ));
                }

                actions.push(fix(
                    &format!("Create function `{name}`"),
                    vec![TextEdit::new(
                        end_of_file,
                        format!("\nfunction {name}():\n\treturn\n"),
                    )],
                    diagnostic,
                ));
            }

            _ => {}
        }
    }

    (!actions.is_empty()).then_some(actions)
}

/// The name of the `kind` reference a diagnostic was reported on.
///
/// Matched by the diagnostic's start position. Matching by line alone picked
/// the first call on the line, so with two unknown calls side by side the
/// second one's fix offered to create the first one.
fn reference_name<'d>(
    document: &'d Document,
    diagnostic: &Diagnostic,
    kind: SymbolKind,
    encoding: Encoding,
) -> Option<&'d str> {
    let start = from_lsp_position(document.text(), diagnostic.range.start, encoding);
    document
        .symbols()
        .references
        .iter()
        .find(|reference| reference.kind == kind && reference.range.start == start)
        .map(|reference| reference.name.as_str())
}

/// Declares `name` as an empty option: appended to the script's `options:`
/// block at its entries' indent, or a new block at the top of the file. The
/// value is left for the user, since any default would be a guess.
fn add_option_edit(document: &Document, name: &str, encoding: Encoding) -> TextEdit {
    let text = document.text();
    let options = document
        .symbols()
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Structure && symbol.name == "options");

    let (at, new_text) = match options {
        Some(block) => {
            let indent = block
                .children
                .first()
                .map(|entry| {
                    let line = document.line(entry.selection_range.start.line);
                    line[..skript_index::indent_width(line)].to_string()
                })
                .unwrap_or_else(|| "\t".into());
            let after = block.range.end.line + 1;
            // The block may end on the file's last line with no newline.
            let lead = if after as usize >= text.lines().count() && !text.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            (
                skript_index::Position::new(after, 0),
                format!("{lead}{indent}{name}: \n"),
            )
        }
        None => (
            skript_index::Position::new(0, 0),
            format!("options:\n\t{name}: \n\n"),
        ),
    };
    TextEdit {
        range: to_lsp_range(text, skript_index::Range::new(at, at), encoding),
        new_text,
    }
}

/// Builds a quick-fix action carrying one file's edits.
fn quick_fix(
    title: &str,
    uri: Url,
    edits: Vec<TextEdit>,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri, edits)])),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// The declared function whose name is nearest `name`, when one is near enough
/// to be worth offering.
///
/// The threshold scales with length, so a three-letter name matches nothing and
/// a long one tolerates a couple of typos. Suggesting a wildly different name is
/// worse than suggesting none.
fn closest_function(workspace: &Workspace, name: &str) -> Option<String> {
    let limit = (name.len() / 3).clamp(1, 3);
    let mut best: Option<(usize, String)> = None;

    for document in workspace.documents() {
        for symbol in document.symbols().flat() {
            if !symbol.kind.is_function() || symbol.name == name {
                continue;
            }
            let distance = edit_distance(name, &symbol.name);
            if distance <= limit && best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
                best = Some((distance, symbol.name.clone()));
            }
        }
    }
    best.map(|(_, name)| name)
}

/// Levenshtein distance, kept to two rows.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];

    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(left != right);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// The editor's indent choice, plus the user's formatter settings.
fn format_options(state: &State, options: &FormattingOptions) -> skript_format::Options {
    skript_format::Options {
        hard_tabs: !options.insert_spaces,
        tab_size: options.tab_size as usize,
        blank_line_between_structures: state.settings.format_blank_lines,
        ..Default::default()
    }
}

pub fn formatting(state: &State, params: &DocumentFormattingParams) -> Option<Vec<TextEdit>> {
    let document = state.workspace.get(params.text_document.uri.as_str())?;
    let options = format_options(state, &params.options);

    // `None` when already formatted or the file does not parse: either way,
    // changing nothing is the right answer.
    let formatted = skript_format::format(document, options)?;

    // One edit spanning the document. Skript's indentation is its syntax, so
    // partial edits applied out of order could change which block a line
    // belongs to.
    //
    // The end is the start of the line *after* the last one, which for a
    // document of N lines is line N. Going further (N + 1) is out of range,
    // and a client that does not clamp rejects the edit outright, so
    // formatting would silently do nothing.
    let line_count = document.text().lines().count() as u32;
    Some(vec![TextEdit {
        range: Range::new(Position::new(0, 0), Position::new(line_count, 0)),
        new_text: formatted,
    }])
}

/// "Format selection". The edit covers whole top-level structures, which
/// may reach past the selection: Skript infers the indent unit per
/// structure, so reindenting half of one could make it invalid.
///
/// Only line numbers are read from the range, and those mean the same in
/// every position encoding.
pub fn range_formatting(
    state: &State,
    params: &DocumentRangeFormattingParams,
) -> Option<Vec<TextEdit>> {
    let document = state.workspace.get(params.text_document.uri.as_str())?;
    let Range { start, end } = params.range;
    // A selection of whole lines ends at column 0 of the line after them.
    let last = if end.character == 0 && end.line > start.line {
        end.line - 1
    } else {
        end.line
    };
    let edit = skript_format::format_lines(
        document,
        format_options(state, &params.options),
        start.line as usize,
        last as usize,
    )?;
    Some(vec![TextEdit {
        range: Range::new(
            Position::new(edit.start_line as u32, 0),
            Position::new(edit.end_line as u32 + 1, 0),
        ),
        new_text: edit.text,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{CodeActionContext, TextDocumentIdentifier};

    #[test]
    fn edit_distance_is_the_usual_one() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn a_typo_is_suggested_but_an_unrelated_name_is_not() {
        let mut workspace = Workspace::new();
        workspace.open(
            "file:///t.sk",
            "function giveKit(p: player):\n\treturn\n\nfunction teleportHome(p: player):\n\treturn\n",
        );
        // One transposed letter: worth offering.
        assert_eq!(
            closest_function(&workspace, "giveKti"),
            Some("giveKit".to_string())
        );
        // Nothing like either. Suggesting a wildly different name is worse than
        // suggesting none, so the threshold scales with length.
        assert_eq!(closest_function(&workspace, "payout"), None);
    }

    #[test]
    fn a_short_name_does_not_match_everything() {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", "function ab(p: player):\n\treturn\n");
        // `xy` is two edits from `ab`, which for a two-letter name is the whole
        // word; the limit is 1 so nothing is offered.
        assert_eq!(closest_function(&workspace, "xy"), None);
    }

    fn option_edit(source: &str) -> TextEdit {
        let document = Document::new("file:///t.sk", source);
        add_option_edit(&document, "max", Encoding::Utf8)
    }

    #[test]
    fn an_option_is_appended_to_the_existing_block_at_its_indent() {
        let edit = option_edit("options:\n    prefix: x\n\non join:\n\tstop\n");
        assert_eq!(edit.new_text, "    max: \n");
        assert_eq!(edit.range.start, Position::new(2, 0));
        assert_eq!(edit.range.start, edit.range.end);
    }

    #[test]
    fn a_new_options_block_goes_at_the_top() {
        let edit = option_edit("on join:\n\tsend \"{@max}\"\n");
        assert_eq!(edit.new_text, "options:\n\tmax: \n\n");
        assert_eq!(edit.range.start, Position::new(0, 0));
    }

    /// The block ends on the last line, which has no newline to append after.
    #[test]
    fn an_option_block_at_the_end_of_a_file_without_a_newline_gets_one() {
        let edit = option_edit("on join:\n\tstop\noptions:\n\tprefix: x");
        assert_eq!(edit.new_text, "\n\tmax: \n");
    }

    fn actions_for(source: &str, diagnostic: Diagnostic) -> Vec<String> {
        let mut state = State::default();
        state.workspace.open("file:///t.sk", source);
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            range: diagnostic.range,
            context: CodeActionContext {
                diagnostics: vec![diagnostic],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        code_action(&state, &params)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect()
    }

    fn range_edits(source: &str, range: Range) -> Option<Vec<TextEdit>> {
        let mut state = State::default();
        state.workspace.open("file:///t.sk", source);
        range_formatting(
            &state,
            &DocumentRangeFormattingParams {
                text_document: TextDocumentIdentifier {
                    uri: "file:///t.sk".parse().unwrap(),
                },
                range,
                options: FormattingOptions {
                    tab_size: 4,
                    insert_spaces: false,
                    ..Default::default()
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    #[test]
    fn range_formatting_rewrites_the_selected_structure_only() {
        let source = "on join:
    send \"a\"

on quit:
  stop
";
        // Lines 0 and 1 selected, ending at column 0 of line 2.
        let edits = range_edits(source, Range::new(Position::new(0, 3), Position::new(2, 0)))
            .expect("an edit");
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].range,
            Range::new(Position::new(0, 0), Position::new(2, 0))
        );
        assert_eq!(
            edits[0].new_text,
            "on join:
	send \"a\"
"
        );
    }

    #[test]
    fn range_formatting_an_already_formatted_selection_does_nothing() {
        let source = "on join:
	stop

on quit:
  stop
";
        assert!(
            range_edits(source, Range::new(Position::new(1, 0), Position::new(1, 5))).is_none()
        );
    }

    #[test]
    fn the_indentation_fix_does_not_add_blank_lines() {
        let source = "on join:
 	stop
on quit:
	stop
";
        let mut state = State::default();
        state.workspace.open("file:///t.sk", source);
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(1, 0), Position::new(1, 2)),
            code: Some(NumberOrString::String("mixed-indentation".into())),
            ..Default::default()
        };
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            range: diagnostic.range,
            context: CodeActionContext {
                diagnostics: vec![diagnostic],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let Some(CodeActionOrCommand::CodeAction(action)) =
            code_action(&state, &params).and_then(|mut actions| actions.pop())
        else {
            panic!("no fix offered");
        };
        let edits = &action.edit.unwrap().changes.unwrap()[&params.text_document.uri];
        assert_eq!(
            edits[0].new_text,
            "on join:
	stop
on quit:
	stop
"
        );
    }

    #[test]
    fn removing_cancel_event_deletes_the_whole_line() {
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(1, 1), Position::new(1, 13)),
            code: Some(NumberOrString::String("event-not-cancellable".into())),
            ..Default::default()
        };
        let titles = actions_for(
            "on join:
	cancel event
",
            diagnostic,
        );
        assert_eq!(titles, vec!["Remove `cancel event`"]);
    }

    #[test]
    fn a_mistyped_event_is_offered_its_correction() {
        let mut state = State {
            catalog: Some(skript_docs::Catalog::build(
                skript_docs::Docs::parse(
                    r#"{ "events": [{ "id": "join", "name": "On Join", "patterns": ["[player] join[ing]"] }] }"#,
                )
                .unwrap(),
            )),
            ..State::default()
        };
        state.workspace.open("file:///t.sk", "on jion:\n\tstop\n");
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(0, 3), Position::new(0, 7)),
            code: Some(NumberOrString::String("unknown-event".into())),
            ..Default::default()
        };
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            range: diagnostic.range,
            context: CodeActionContext {
                diagnostics: vec![diagnostic],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let Some(CodeActionOrCommand::CodeAction(action)) =
            code_action(&state, &params).and_then(|mut actions| actions.pop())
        else {
            panic!("no fix offered");
        };
        assert_eq!(action.title, "Change to `on join`");
        let edits = &action.edit.unwrap().changes.unwrap()[&params.text_document.uri];
        assert_eq!(edits[0].new_text, "join");
        assert_eq!(edits[0].range, params.range);
    }

    /// Two unknown calls on one line: each fix must name its own call.
    #[test]
    fn an_unknown_function_fix_names_the_call_it_was_reported_on() {
        let source = "on join:\n\tset {_x} to first() + second()\n";
        let column = source.lines().nth(1).unwrap().find("second").unwrap() as u32;
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(1, column), Position::new(1, column + 6)),
            code: Some(NumberOrString::String("unknown-function".into())),
            ..Default::default()
        };
        let titles = actions_for(source, diagnostic);
        assert_eq!(titles, vec!["Create function `second`"]);
    }
}
