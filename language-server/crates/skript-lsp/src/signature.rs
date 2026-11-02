//! Signature help and parameter-name inlay hints: the two features that read
//! a call's arguments against its declaration.

use skript_docs::Category;
use skript_index::SymbolKind;
use tower_lsp::lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, ParameterInformation,
    ParameterLabel, SignatureHelp, SignatureHelpParams, SignatureInformation,
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
    labels.dedup();
    if labels.is_empty() {
        return None;
    }

    let signatures: Vec<SignatureInformation> = labels
        .into_iter()
        .map(|label| {
            let parameters = signature_parameters(&label);
            let active = active_parameter(&call, &parameters);
            SignatureInformation {
                parameters: Some(
                    parameters
                        .into_iter()
                        .map(|parameter| ParameterInformation {
                            label: ParameterLabel::Simple(parameter),
                            documentation: None,
                        })
                        .collect(),
                ),
                active_parameter: Some(active),
                label,
                documentation: None,
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

/// Every signature the call could mean, as `label` strings.
///
/// A ClassySK method is matched by name alone, so each class declaring it is
/// one signature. The user's own functions come before the catalog's, which
/// supplies Skript's built-ins (`floor(n: number)`) and addon functions.
fn signature_labels(state: &State, uri: &str, call: &CallAt) -> Vec<String> {
    if call.is_member {
        return state
            .workspace
            .definitions(SymbolKind::Method, &call.name, uri)
            .into_iter()
            .map(|(target, method)| {
                let class = enclosing_class(target, method.range)
                    .map(|class| format!("{}::", class.name))
                    .unwrap_or_default();
                format!("{class}{}{}", method.name, method.detail)
            })
            .collect();
    }
    let declared: Vec<String> = state
        .workspace
        .definitions(SymbolKind::Function, &call.name, uri)
        .into_iter()
        .map(|(_, function)| format!("{}{}", function.name, function.detail))
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
        .collect()
}

/// A named argument (`updateEquip(e: …)`) is matched to its parameter by name;
/// otherwise it is the comma count.
fn active_parameter(call: &CallAt, parameters: &[String]) -> u32 {
    call.named
        .as_deref()
        .and_then(|named| {
            parameters.iter().position(|parameter| {
                parameter
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('?')
                    .eq_ignore_ascii_case(named)
            })
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
        if reference.kind != SymbolKind::Function {
            continue;
        }
        let line = reference.range.start.line;
        if line < from.line || line > to.line {
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

    #[test]
    fn hints_name_each_argument_of_a_declared_function() {
        let mut state = State::default();
        let source = "function give(p: player, n: number):\n\tstop\non join:\n\tgive(player, 3)\n";
        state.workspace.open("file:///t.sk", source);
        let params = InlayHintParams {
            text_document: tower_lsp::lsp_types::TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            range: tower_lsp::lsp_types::Range::new(
                tower_lsp::lsp_types::Position::new(0, 0),
                tower_lsp::lsp_types::Position::new(9, 0),
            ),
            work_done_progress_params: Default::default(),
        };
        let labels: Vec<(u32, String)> = inlay_hint(&state, &params)
            .unwrap()
            .into_iter()
            .map(|hint| match hint.label {
                InlayHintLabel::String(label) => (hint.position.character, label),
                InlayHintLabel::LabelParts(_) => unreachable!(),
            })
            .collect();
        assert_eq!(labels, vec![(6, "p:".into()), (14, "n:".into())]);
    }
}
