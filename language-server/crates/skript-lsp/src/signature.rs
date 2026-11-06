//! Signature help and inlay hints: the features that read a use against its
//! declaration, a call's arguments against the function's parameters and an
//! `{@option}` against the `options:` block.

use skript_docs::Category;
use skript_index::symbols::DocComment;
use skript_index::{Document, Reference, Symbol, SymbolKind};
use tower_lsp::lsp_types::{
    Documentation, InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, InlayHintTooltip,
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpParams, SignatureInformation,
};

use crate::calls::{argument_offsets, call_at, signature_parameters, CallAt};
use crate::convert::{self, from_lsp_position};
use crate::navigation::enclosing_class;
use crate::server::State;

pub fn signature_help(state: &State, params: &SignatureHelpParams) -> Option<SignatureHelp> {
    let at = &params.text_document_position_params;
    let uri = at.text_document.uri.as_str();
    let document = state.workspace.get(uri)?;
    let position = from_lsp_position(document.text(), at.position, state.encoding);
    let prefix = convert::line_prefix(document.line(position.line), position.character);
    let call = call_at(prefix)?;

    let mut labels = signature_labels(state, uri, &call);
    labels.dedup_by(|a, b| a.0 == b.0);
    if labels.is_empty() {
        return None;
    }

    let signatures: Vec<SignatureInformation> = labels
        .into_iter()
        .map(|(label, doc)| {
            let parameters = signature_parameters(&label);
            let active = active_parameter(&call, &parameters);
            SignatureInformation {
                parameters: Some(
                    parameters
                        .into_iter()
                        .map(|parameter| ParameterInformation {
                            documentation: doc
                                .and_then(|doc| doc.param(parameter_name(&parameter)))
                                .filter(|text| !text.is_empty())
                                .map(|text| Documentation::String(text.to_string())),
                            label: ParameterLabel::Simple(parameter),
                        })
                        .collect(),
                ),
                active_parameter: Some(active),
                documentation: doc
                    .map(signature_documentation)
                    .filter(|text| !text.is_empty())
                    .map(|text| Documentation::MarkupContent(crate::hover::markdown(text))),
                label,
            }
        })
        .collect();
    let active_parameter = signatures.first().and_then(|first| first.active_parameter);

    Some(SignatureHelp {
        signatures,
        active_signature: Some(0),
        active_parameter,
    })
}

/// Every signature the call could mean, as `label` strings, each with the
/// declaration's doc comment when it has one.
///
/// A ClassySK method is matched by name alone, so each class declaring it is
/// one signature. The user's own functions come before the catalog's, which
/// supplies Skript's built-ins (`floor(n: number)`) and addon functions.
fn signature_labels<'s>(
    state: &'s State,
    uri: &str,
    call: &CallAt,
) -> Vec<(String, Option<&'s DocComment>)> {
    let doc =
        |target: &'s Document, symbol: &Symbol| target.symbols().doc_for(symbol.selection_range);
    if call.is_member {
        return state
            .workspace
            .definitions(SymbolKind::Method, &call.name, uri)
            .into_iter()
            .map(|(target, method)| {
                let class = enclosing_class(target, method.range)
                    .map(|class| format!("{}::", class.name))
                    .unwrap_or_default();
                let label = format!("{class}{}{}", method.name, method.detail);
                (label, doc(target, method))
            })
            .collect();
    }
    let declared: Vec<(String, Option<&DocComment>)> = state
        .workspace
        .definitions(SymbolKind::Function, &call.name, uri)
        .into_iter()
        .map(|(target, function)| {
            let label = format!("{}{}", function.name, function.detail);
            (label, doc(target, function))
        })
        .collect();
    if !declared.is_empty() {
        return declared;
    }
    let Some(catalog) = &state.catalog else {
        return Vec::new();
    };
    catalog
        .docs()
        .entries(Category::Function)
        .iter()
        .filter(|entry| entry.name == call.name)
        .filter_map(|entry| entry.patterns.first().cloned())
        .map(|label| (label, None))
        .collect()
}

/// `amount` from a parameter label `amount?: number = 1`.
fn parameter_name(parameter: &str) -> &str {
    parameter
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('?')
}

/// What a signature's own documentation shows: the doc comment without its
/// parameters, which each parameter carries itself, and without examples,
/// which a popup over the line being typed has no room for.
fn signature_documentation(doc: &DocComment) -> String {
    let summary = DocComment {
        params: Vec::new(),
        examples: Vec::new(),
        ..doc.clone()
    };
    crate::hover::doc_markdown(&summary, None, None)
}

/// A named argument (`updateEquip(e: …)`) is matched to its parameter by name;
/// otherwise it is the comma count.
fn active_parameter(call: &CallAt, parameters: &[String]) -> u32 {
    call.named
        .as_deref()
        .and_then(|named| {
            parameters
                .iter()
                .position(|parameter| parameter_name(parameter).eq_ignore_ascii_case(named))
        })
        .map_or(call.argument, |index| index as u32)
}

/// Parameter-name hints at function call sites.
///
/// Skript's call syntax carries no argument names, so `giveKit(p, 3, true)` is
/// unreadable without opening the declaration. This is the one place the index
/// already knows something the source does not show.
pub fn inlay_hint(state: &State, params: &InlayHintParams) -> Option<Vec<InlayHint>> {
    let uri = params.text_document.uri.as_str();
    let document = state.workspace.get(uri)?;
    let text = document.text();
    let from = from_lsp_position(text, params.range.start, state.encoding);
    let to = from_lsp_position(text, params.range.end, state.encoding);

    let mut hints = Vec::new();
    for reference in &document.symbols().references {
        let line = reference.range.start.line;
        if line < from.line || line > to.line {
            continue;
        }
        if reference.kind == SymbolKind::Option {
            hints.extend(option_hint(state, document, reference));
            continue;
        }
        if reference.kind != SymbolKind::Function {
            continue;
        }

        // The declaration is the only source of parameter names. A call to an
        // unknown function gets no hints rather than invented ones.
        let Some((_, symbol)) = state
            .workspace
            .definitions(SymbolKind::Function, &reference.name, uri)
            .into_iter()
            .next()
        else {
            continue;
        };
        // The declaration's own parameter symbols, rather than re-parsing the
        // rendered signature string.
        let names: Vec<&str> = symbol
            .children
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }

        let offsets = argument_offsets(document.line(line), reference.range.end.character as usize);
        for (name, offset) in names.iter().zip(offsets) {
            hints.push(InlayHint {
                position: convert::to_lsp_position(
                    text,
                    skript_index::Position::new(line, offset),
                    state.encoding,
                ),
                label: InlayHintLabel::String(format!("{name}:")),
                kind: Some(InlayHintKind::PARAMETER),
                text_edits: None,
                tooltip: None,
                padding_left: Some(false),
                padding_right: Some(true),
                data: None,
            });
        }
    }

    Some(hints)
}

/// How much of an option's value a hint shows before cutting it off.
const OPTION_HINT_CHARS: usize = 30;

/// `= "&6[Shop]"` after `{@prefix}`: the value Skript substitutes there.
///
/// Options are pasted in as raw text before the script is parsed, so the value
/// is the one thing that explains the line, and it lives in an `options:`
/// block that is usually a screen away. Only a value the script's own block
/// states plainly is shown: an option it never declares, one declared twice
/// with different values, and a nested option (an `entry_section`, which
/// holds children rather than a value) get nothing.
fn option_hint(state: &State, document: &Document, reference: &Reference) -> Option<InlayHint> {
    let definitions =
        state
            .workspace
            .definitions(SymbolKind::Option, &reference.name, document.uri());
    let (_, first) = definitions.first()?;
    let value = first.detail.trim();
    if value.is_empty()
        || definitions
            .iter()
            .any(|(_, other)| other.detail.trim() != value)
    {
        return None;
    }

    // After the closing brace, so the hint never splits `{@prefix}` itself.
    let line = document.line(reference.name_range.end.line);
    let end = reference.name_range.end.character as usize;
    let rest = line.get(end..)?;
    let close = rest.find('}').filter(|&at| rest[..at].trim().is_empty())?;
    let column = (end + close + 1) as u32;

    let shown = if value.chars().count() > OPTION_HINT_CHARS {
        let cut: String = value.chars().take(OPTION_HINT_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        value.to_string()
    };
    // Quoted unless the value already is, so `&6[Shop]` reads as the text it is.
    let label = if shown.starts_with('"') {
        format!("= {shown}")
    } else {
        format!("= \"{shown}\"")
    };
    Some(InlayHint {
        position: convert::to_lsp_position(
            document.text(),
            skript_index::Position::new(reference.name_range.end.line, column),
            state.encoding,
        ),
        label: InlayHintLabel::String(label),
        kind: None,
        text_edits: None,
        tooltip: Some(InlayHintTooltip::String(format!(
            "Option `{}`, set on line {}",
            first.name,
            first.range.start.line + 1
        ))),
        padding_left: Some(true),
        padding_right: Some(false),
        data: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(named: Option<&str>, argument: u32) -> CallAt {
        CallAt {
            name: "f".into(),
            is_member: false,
            argument,
            named: named.map(Into::into),
        }
    }

    #[test]
    fn a_named_argument_activates_its_parameter() {
        let parameters = vec!["p: player".to_string(), "e?: entity".to_string()];
        assert_eq!(active_parameter(&call(Some("e"), 0), &parameters), 1);
        assert_eq!(active_parameter(&call(Some("E"), 0), &parameters), 1);
        // An unknown name falls back to the comma count.
        assert_eq!(active_parameter(&call(Some("zz"), 1), &parameters), 1);
        assert_eq!(active_parameter(&call(None, 0), &parameters), 0);
    }

    /// Every inlay hint in `source`, as `(line, column, label)`.
    fn hints(source: &str) -> Vec<(u32, u32, String)> {
        let mut state = State::default();
        state.workspace.open("file:///t.sk", source);
        let params = InlayHintParams {
            text_document: tower_lsp::lsp_types::TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            range: tower_lsp::lsp_types::Range::new(
                tower_lsp::lsp_types::Position::new(0, 0),
                tower_lsp::lsp_types::Position::new(99, 0),
            ),
            work_done_progress_params: Default::default(),
        };
        let mut found: Vec<(u32, u32, String)> = inlay_hint(&state, &params)
            .unwrap()
            .into_iter()
            .map(|hint| match hint.label {
                InlayHintLabel::String(label) => {
                    (hint.position.line, hint.position.character, label)
                }
                InlayHintLabel::LabelParts(_) => unreachable!(),
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn hints_name_each_argument_of_a_declared_function() {
        let found =
            hints("function give(p: player, n: number):\n\tstop\non join:\n\tgive(player, 3)\n");
        assert_eq!(found, vec![(3, 6, "p:".into()), (3, 14, "n:".into())]);
    }

    #[test]
    fn an_option_shows_its_value_after_the_closing_brace() {
        let found = hints(concat!(
            "options:\n",
            "\tprefix: &6[Shop]\n",
            "\tquoted: \"hi\"\n",
            "\tlong: &aThis value is far too long to show in full here\n",
            "on join:\n",
            "\tsend \"{@prefix} welcome\"\n",
            "\tsend {@quoted}\n",
            "\tsend \"{@long}\"\n",
            "\tsend \"{@missing}\"\n",
        ));
        assert_eq!(
            found,
            vec![
                (5, 16, "= \"&6[Shop]\"".into()),
                (6, 15, "= \"hi\"".into()),
                (7, 14, "= \"&aThis value is far too long…\"".into()),
            ]
        );
    }

    #[test]
    fn a_nested_option_gets_no_hint() {
        let found = hints(concat!(
            "options:\n",
            "\tcolours:\n",
            "\t\tmain: &6\n",
            "on join:\n",
            "\tsend \"{@colours}{@colours.main}\"\n",
        ));
        assert_eq!(found, Vec::new());
    }

    fn help(source: &str, line: u32, character: u32) -> SignatureHelp {
        let mut state = State::default();
        state.workspace.open("file:///t.sk", source);
        let params = SignatureHelpParams {
            context: None,
            text_document_position_params: tower_lsp::lsp_types::TextDocumentPositionParams {
                text_document: tower_lsp::lsp_types::TextDocumentIdentifier {
                    uri: "file:///t.sk".parse().unwrap(),
                },
                position: tower_lsp::lsp_types::Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
        };
        signature_help(&state, &params).expect("signature help")
    }

    #[test]
    fn each_param_carries_its_doc_text() {
        let source = concat!(
            "# Gives a kit.\n",
            "# @param p who gets it\n",
            "# @return whether it fit\n",
            "# @deprecated use kit2\n",
            "function kit(p: player, n: number) :: boolean:\n",
            "\treturn true\n",
            "on join:\n",
            "\tkit(\n",
        );
        let found = help(source, 7, 5);
        let signature = &found.signatures[0];
        let params = signature.parameters.as_ref().unwrap();
        assert_eq!(
            params[0].documentation,
            Some(Documentation::String("who gets it".into()))
        );
        assert_eq!(params[1].documentation, None);
        let Some(Documentation::MarkupContent(markup)) = &signature.documentation else {
            panic!("no signature documentation");
        };
        assert_eq!(
            markup.value,
            "**Deprecated:** use kit2\n\nGives a kit.\n\n**Returns:** whether it fit"
        );
    }
}
