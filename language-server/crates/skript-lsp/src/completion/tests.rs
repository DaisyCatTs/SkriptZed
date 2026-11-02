use super::*;
use tower_lsp::lsp_types::{TextDocumentIdentifier, TextDocumentPositionParams};

fn document(source: &str) -> Workspace {
    let mut workspace = Workspace::new();
    workspace.open("file:///t.sk", source);
    workspace
}

#[test]
fn completion_context_narrows_by_position() {
    assert_eq!(categories_for("on "), vec![Category::Event]);
    assert!(categories_for("\tif ").contains(&Category::Condition));
    assert!(categories_for("\tsend \"%").contains(&Category::Expression));
    assert!(categories_for("").contains(&Category::Structure));
}

/// Entry keys belong directly in a command's body, and nowhere else.
/// Offering them inside `trigger:` replaces the effect and function
/// suggestions the user needs there, which is worse than offering nothing.
#[test]
fn entries_belong_in_the_command_body_but_not_in_its_trigger() {
    let workspace = document(
        "command /hello <text>:\n\tpermission: skript.hello\n\ttrigger:\n\t\tsend \"hi\" to player\n\t\tstop\n\non join:\n\tstop\n",
    );
    let document = workspace.get("file:///t.sk").unwrap();

    assert!(in_command_entry_position(document, 1), "the entry line");
    // Inside the trigger's body. An earlier version tested whether the
    // trigger had *child sections*; a trigger of plain statements has none,
    // so it wrongly reported an entry position here.
    assert!(!in_command_entry_position(document, 3), "inside trigger");
    assert!(!in_command_entry_position(document, 4), "inside trigger");
    // Outside the command entirely.
    assert!(!in_command_entry_position(document, 0), "the header itself");
    assert!(!in_command_entry_position(document, 7), "inside an event");
}

/// Skript syntax is multi-word and the client's "current word" stops at a
/// space, so accepting after `send m` used to replace only the `m` and leave
/// the keyword duplicated.
#[test]
fn a_statement_fragment_covers_the_whole_phrase() {
    assert_eq!(fragment_start("\t\tsend m"), 2);
    assert_eq!(fragment_start("send"), 0);
}

#[test]
fn a_nested_fragment_starts_after_its_delimiter() {
    for line in [
        "\tsend \"%pla",
        "\tset {_x} to greet(player, am",
        "\tset {_x} to f(pl",
    ] {
        let start = fragment_start(line);
        assert!(
            !line[start..].contains(['%', '(', ',']),
            "{line:?} -> {:?} still spans a delimiter",
            &line[start..]
        );
    }
}

#[test]
fn local_scope_is_the_trigger_with_its_parameters() {
    let workspace = document(
        "function greet(who: player, n: number):\n\tsend \"hi\" to {_who}\n\non join:\n\tset {_x} to 1\n",
    );
    let document = workspace.get("file:///t.sk").unwrap();

    let (range, parameters) = local_scope(document, 1).expect("a scope");
    assert_eq!((range.start.line, range.end.line), (0, 1));
    assert_eq!(parameters, vec!["who", "n"]);

    let (range, parameters) = local_scope(document, 4).expect("a scope");
    assert_eq!(range.start.line, 3);
    assert!(parameters.is_empty(), "an event declares no parameters");
}

/// Each ClassySK method runs as its own trigger.
#[test]
fn local_scope_narrows_to_a_classysk_method() {
    let workspace = document(concat!(
        "class Wallet:\n",
        "\tpublic deposit(amount: number):\n",
        "\t\tset {_i} to {_amount}\n",
        "\tpublic reset():\n",
        "\t\tset {_i} to 0\n",
    ));
    let document = workspace.get("file:///t.sk").unwrap();

    let (range, parameters) = local_scope(document, 2).expect("a scope");
    assert_eq!((range.start.line, range.end.line), (1, 2));
    assert_eq!(parameters, vec!["amount"]);

    let (range, parameters) = local_scope(document, 4).expect("a scope");
    assert_eq!(range.start.line, 3);
    assert!(parameters.is_empty());
}

/// An unclosed `{_` can make the parse tree run structures together; the
/// scope is clamped to what the indentation says.
#[test]
fn local_scope_is_clamped_by_indentation_while_a_brace_is_open() {
    let workspace = document("on join:\n\tset {_a} to 1\n\tsend {_\non quit:\n\tset {_b} to 2\n");
    let document = workspace.get("file:///t.sk").unwrap();
    let (range, _) = local_scope(document, 2).expect("a scope");
    assert_eq!(range.start.line, 0);
    assert!(range.end.line <= 2, "ran into the next trigger: {range:?}");
}

fn variable_labels(source: &str, line: u32, character: u32) -> Vec<String> {
    let workspace = document(source);
    let document = workspace.get("file:///t.sk").unwrap();
    let text = document.line(line);
    let prefix = &text[..character as usize];
    let open = variables::open_brace(prefix).expect("inside braces");
    variable_completions(
        &workspace,
        document,
        skript_index::Position::new(line, character),
        open,
        text,
        Encoding::Utf8,
    )
    .into_iter()
    .map(|item| item.label)
    .collect()
}

#[test]
fn variable_completion_offers_one_list_level_at_a_time() {
    let source = concat!(
        "on join:\n",
        "\tset {data::%player%::coins} to 1\n",
        "\tset {data::%player%::kills} to 1\n",
        "\tset {data::settings} to 1\n",
        "\tsend {data::\n",
    );
    let labels = variable_labels(source, 4, 13);
    assert_eq!(labels, vec!["%player%::", "settings", "*"]);

    let source = concat!(
        "on join:\n",
        "\tset {data::%player%::coins} to 1\n",
        "\tsend {data::%player%::\n",
    );
    let labels = variable_labels(source, 2, 23);
    assert_eq!(labels, vec!["coins", "*"]);
}

#[test]
fn local_variable_completion_stays_in_its_trigger() {
    let source = concat!(
        "function f(who: player):\n",
        "\tset {_mine} to 1\n",
        "\tsend {_\n",
        "on quit:\n",
        "\tset {_theirs} to 1\n",
    );
    let labels = variable_labels(source, 2, 8);
    assert!(labels.contains(&"_mine".to_string()), "{labels:?}");
    assert!(
        labels.contains(&"_who".to_string()),
        "a parameter: {labels:?}"
    );
    assert!(!labels.contains(&"_theirs".to_string()), "{labels:?}");
}

const TELEPORT_DOCS: &str = r#"{
  "effects": [
    { "id": "t.teleport", "name": "Teleport", "patterns": ["teleport %entities% to %location%"] }
  ]
}"#;

fn labels_for(state: &State, line: u32, character: u32) -> Vec<String> {
    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            position: Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    };
    match completion(state, &params) {
        Some(CompletionResponse::List(list)) => list.items,
        Some(CompletionResponse::Array(items)) => items,
        None => Vec::new(),
    }
    .into_iter()
    .map(|item| item.label)
    .collect()
}

fn crowded_state(typed: &str) -> State {
    let mut source = String::new();
    for index in 0..300 {
        source.push_str(&format!("function helper{index}():\n\tstop\n"));
    }
    source.push_str(&format!("on join:\n\t{typed}\n"));
    let mut state = State::default();
    state.workspace.open("file:///t.sk", source);
    state.catalog = Some(skript_docs::Catalog::build(
        skript_docs::Docs::parse(TELEPORT_DOCS).unwrap(),
    ));
    state
}

/// 300 project functions used to fill the whole capped list whatever was
/// typed, because the project bonus kept every one of them above zero.
#[test]
fn project_symbols_that_do_not_match_do_not_crowd_out_the_catalog() {
    let state = crowded_state("eleport");
    let labels = labels_for(&state, 601, 8);
    assert!(labels.contains(&"Teleport".to_string()), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label.starts_with("helper")),
        "non-matching project functions survived: {} items",
        labels.len()
    );
}

#[test]
fn a_matching_project_symbol_still_comes_first() {
    let state = crowded_state("helper12");
    let labels = labels_for(&state, 601, 9);
    assert_eq!(labels.first().map(String::as_str), Some("helper12"));
}

#[test]
fn an_abbreviation_reaches_catalog_syntax() {
    let state = crowded_state("tlp");
    let labels = labels_for(&state, 601, 4);
    assert!(labels.contains(&"Teleport".to_string()), "{labels:?}");
}

/// Skript only accepts `prefix:` on the root command.
#[test]
fn prefix_is_offered_on_the_command_but_not_a_subcommand() {
    let has_prefix = |scope| {
        command_entry_items("pre", scope)
            .iter()
            .any(|(_, item)| item.label == "prefix:")
    };
    assert!(has_prefix(entries::EntryScope::Command));
    assert!(!has_prefix(entries::EntryScope::Subcommand));
}

#[test]
fn the_trigger_entry_snippet_opens_an_indented_body() {
    let items = command_entry_items("trig", entries::EntryScope::Command);
    let trigger = items
        .iter()
        .find(|(_, item)| item.label == "trigger:")
        .expect("the trigger entry");
    assert_eq!(trigger.1.insert_text.as_deref(), Some("trigger:\n\t$0"));
}

/// What Zed shows: it filters every item by the whole "word" before the
/// cursor, `{`, `%` and `:` included, with a case-insensitive subsequence
/// match against the filter text.
fn shown_by_zed(state: &State, line: u32, character: u32) -> Vec<String> {
    let document = state.workspace.get("file:///t.sk").unwrap();
    let prefix = &document.line(line)[..character as usize];
    let query = prefix[editor_query_start(prefix)..].to_lowercase();
    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t.sk".parse().unwrap(),
            },
            position: Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    };
    let items = match completion(state, &params) {
        Some(CompletionResponse::List(list)) => list.items,
        Some(CompletionResponse::Array(items)) => items,
        None => Vec::new(),
    };
    items
        .into_iter()
        .filter(|item| {
            let candidate = item
                .filter_text
                .as_deref()
                .unwrap_or(&item.label)
                .to_lowercase();
            let mut rest = candidate.chars();
            query.chars().all(|wanted| rest.any(|c| c == wanted))
        })
        .map(|item| item.label)
        .collect()
}

/// The server sent the right variables all along, and Zed hid every one: it
/// filtered `_count` by `{_`.
#[test]
fn variable_completions_survive_the_editors_filter() {
    let source = "on join:\n\tset {_count} to 1\n\tset {data::settings::color} to 2\n\tset {_x} to {_\n\tsend \"%{_\n\tset {_y} to {data::\n\tset {_z} to {data::settings::co\n";
    let state = State {
        workspace: document(source),
        ..Default::default()
    };
    let lines: Vec<&str> = source.lines().collect();
    let at_end = |line: usize| (line as u32, lines[line].len() as u32);

    for line in [3, 4] {
        let (l, c) = at_end(line);
        let shown = shown_by_zed(&state, l, c);
        assert!(
            shown.iter().any(|label| label.contains("count")),
            "line {line}: {shown:?}"
        );
    }
    let (l, c) = at_end(5);
    assert!(shown_by_zed(&state, l, c)
        .iter()
        .any(|label| label.starts_with("settings")));
    let (l, c) = at_end(6);
    assert!(shown_by_zed(&state, l, c)
        .iter()
        .any(|label| label == "color"));
}

/// `to vis` should reach `{_visitor}` without first typing `{_`, and `{v`
/// should find a local as well as globals.
#[test]
fn locals_are_offered_from_a_bare_word_and_a_bare_brace() {
    let source = "on join:\n    set {_visitor} to player\n    set {data::motd} to \"hi\"\n    send {data::motd} to vis\n    send {data::motd} to {v}\n";
    let state = State {
        workspace: document(source),
        ..Default::default()
    };
    let bare = shown_by_zed(&state, 3, "    send {data::motd} to vis".len() as u32);
    assert!(bare.contains(&"{_visitor}".to_string()), "{bare:?}");
    let brace = shown_by_zed(&state, 4, "    send {data::motd} to {v".len() as u32);
    assert!(brace.contains(&"_visitor".to_string()), "{brace:?}");
}
