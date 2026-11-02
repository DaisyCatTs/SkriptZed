//! The MCP server against a fixture catalog and a fixture project.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use skript_addons::Detection;
use skript_docs::model::AddonRef;
use skript_docs::{Catalog, Docs};

use super::tools::{MAX_SEARCH_LIMIT, MAX_TEXT};
use super::*;

const CORE: &str = r#"{
  "source": { "name": "fixture", "version": "2.16.0" },
  "effects": [
    { "id": "EffActionBar", "name": "Action Bar", "since": ["2.3"],
      "description": ["Sends an action bar message to the given player(s)."],
      "patterns": ["send [the] action[ ]bar [with text] %string% [to %players%]"],
      "examples": ["send action bar \"Hello\" to player"] },
    { "id": "EffMessage", "name": "Message", "since": ["1.0"],
      "description": ["Sends a message to the given player."],
      "patterns": ["(message|send [message[s]]) %objects% [to %commandsenders%]"] },
    { "id": "EffTeleport", "name": "Teleport", "since": ["1.0"],
      "description": ["Teleports an entity to a location."],
      "patterns": ["teleport %entities% to %location%"] }
  ],
  "conditions": [
    { "id": "CondIsOp", "name": "Is Operator", "since": ["2.7"],
      "patterns": ["%offlineplayers% (is|are) [not] op[s]"] }
  ],
  "events": [
    { "id": "EvtJoin", "name": "Join", "since": ["1.0"],
      "description": ["Called when a player joins the server."],
      "patterns": ["[player] (login|logging in|join[ing])"] }
  ],
  "functions": [
    { "id": "floor", "name": "floor", "since": ["2.2"],
      "patterns": ["floor(n: number)"] }
  ]
}"#;

const ADDON: &str = r#"{
  "effects": [
    { "id": "SkBeeToast", "name": "Send Toast", "since": ["1.0"],
      "description": ["Sends a toast to players."],
      "patterns": ["send toast %string% to %players%"] }
  ]
}"#;

const NOT_INSTALLED: &str = r#"{
  "effects": [
    { "id": "DanceEff", "name": "Dance", "patterns": ["make %players% dance wildly"] }
  ]
}"#;

fn addon(json: &str, name: &str) -> Docs {
    let mut docs = Docs::parse(json).expect("fixture parses");
    docs.attribute_to(&AddonRef {
        name: name.into(),
        url: String::new(),
        since_version: None,
    });
    docs
}

fn knowledge() -> Knowledge {
    let mut docs = Docs::parse(CORE).expect("fixture parses");
    docs.merge(addon(ADDON, "SkBee"));
    docs.resolve_versions();
    let mut rest = addon(NOT_INSTALLED, "DanceAddon");
    rest.resolve_versions();
    Knowledge {
        catalog: Catalog::build(docs),
        uninstalled: Some(Catalog::build(rest)),
        detection: Detection::default(),
        degraded: None,
    }
}

/// A throwaway project folder, removed on drop.
struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("skript-lsp-mcp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(
            root.join("scripts/main.sk"),
            concat!(
                "options:\n",
                "\tprefix: &6[Test]\n",
                "\n",
                "command /heal [<player>]:\n",
                "\tpermission: test.heal\n",
                "\ttrigger:\n",
                "\t\tsend action bar \"healed\" to player\n",
                "\n",
                "function greet(p: player, times: number = 1) :: text:\n",
                "\treturn \"hi\"\n",
                "\n",
                "local function helper():\n",
                "\tstop\n",
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/classes.sk"),
            concat!(
                "class Wallet:\n",
                "\tprivate balance: number = 0\n",
                "\tpublic deposit(amount: number) :: boolean:\n",
                "\t\tadd {_amount} to self::balance\n",
                "\n",
                "struct Message:\n",
                "\tcontents: string\n",
            ),
        )
        .unwrap();
        Project(root)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn server(project: Option<PathBuf>) -> Server {
    Server::new(Arc::new(Pending::ready(knowledge())), project)
}

fn request(server: &Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
    let response = server
        .handle(&line.to_string())
        .expect("a request is answered");
    assert!(!response.contains('\n'), "a response must be one line");
    serde_json::from_str(&response).unwrap()
}

/// Calls a tool and returns (text, isError).
fn call(server: &Server, tool: &str, arguments: Value) -> (String, bool) {
    let response = request(
        server,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    );
    let result = &response["result"];
    let text = result["content"][0]["text"].as_str().unwrap().to_string();
    assert_eq!(result["content"][0]["type"], "text");
    (text, result["isError"].as_bool().unwrap())
}

fn call_json(server: &Server, tool: &str, arguments: Value) -> Value {
    let (text, is_error) = call(server, tool, arguments);
    assert!(!is_error, "{tool} failed: {text}");
    assert!(text.len() <= MAX_TEXT);
    serde_json::from_str(&text).expect("tool answers are JSON")
}

// ------------------------------------------------------------------ framing

#[test]
fn reads_one_message_per_line_and_skips_blank_lines() {
    let mut input = Cursor::new(b"{\"a\":1}\r\n\n  \n{\"b\":2}\n{\"c\":3}".to_vec());
    let read = |input: &mut Cursor<Vec<u8>>| read_message(input, 1024).unwrap();
    assert_eq!(
        read(&mut input),
        Some(Incoming::Message("{\"a\":1}".into()))
    );
    assert_eq!(
        read(&mut input),
        Some(Incoming::Message("{\"b\":2}".into()))
    );
    // The last message may arrive without a trailing newline.
    assert_eq!(
        read(&mut input),
        Some(Incoming::Message("{\"c\":3}".into()))
    );
    assert_eq!(read(&mut input), None);
}

#[test]
fn an_oversized_line_is_dropped_whole_and_the_next_one_still_reads() {
    let mut text = "x".repeat(100);
    text.push_str("\n{\"ok\":true}\n");
    let mut input = Cursor::new(text.into_bytes());
    assert_eq!(
        read_message(&mut input, 20).unwrap(),
        Some(Incoming::TooLong)
    );
    assert_eq!(
        read_message(&mut input, 20).unwrap(),
        Some(Incoming::Message("{\"ok\":true}".into()))
    );

    // Drained a buffer at a time, as stdin delivers it.
    let mut text = "x".repeat(100);
    text.push_str("\n{\"ok\":true}\n");
    let mut input = std::io::BufReader::with_capacity(8, Cursor::new(text.into_bytes()));
    assert_eq!(
        read_message(&mut input, 20).unwrap(),
        Some(Incoming::TooLong)
    );
    assert_eq!(
        read_message(&mut input, 20).unwrap(),
        Some(Incoming::Message("{\"ok\":true}".into()))
    );
}

#[test]
fn a_parse_error_is_answered_with_a_null_id() {
    let response: Value = serde_json::from_str(&server(None).handle("{not json").unwrap()).unwrap();
    assert_eq!(response["id"], Value::Null);
    assert_eq!(response["error"]["code"], -32700);
}

#[test]
fn notifications_and_responses_get_no_answer() {
    let server = server(None);
    assert_eq!(
        server.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
        None
    );
    assert_eq!(
        server.handle(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#
        ),
        None
    );
    assert_eq!(
        server.handle(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
        None
    );
    // Even an unknown method sent as a notification stays unanswered.
    assert_eq!(server.handle(r#"{"jsonrpc":"2.0","method":"nope"}"#), None);
}

#[test]
fn ping_unknown_methods_and_bad_requests() {
    let server = server(None);
    assert_eq!(request(&server, "ping", Value::Null)["result"], json!({}));
    let unknown = request(&server, "resources/list", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(unknown["id"], 7);
    let not_object: Value = serde_json::from_str(&server.handle("42").unwrap()).unwrap();
    assert_eq!(not_object["error"]["code"], -32600);
}

#[test]
fn a_batch_is_answered_as_a_batch() {
    let server = server(None);
    let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},
                    {"jsonrpc":"2.0","method":"notifications/initialized"},
                    {"jsonrpc":"2.0","id":2,"method":"ping"}]"#
        .replace('\n', "");
    let response: Value = serde_json::from_str(&server.handle(&batch).unwrap()).unwrap();
    let ids: Vec<i64> = response
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![1, 2]);
}

// ------------------------------------------------------- initialize & tools

#[test]
fn initialize_echoes_a_supported_version_and_offers_the_newest_otherwise() {
    let server = server(None);
    let answer = request(
        &server,
        "initialize",
        json!({ "protocolVersion": "2025-03-26" }),
    );
    let result = &answer["result"];
    assert_eq!(result["protocolVersion"], "2025-03-26");
    assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(result["serverInfo"]["name"], "skript-lsp");
    assert!(result["instructions"]
        .as_str()
        .unwrap()
        .contains("search_syntax"));

    let unknown = request(
        &server,
        "initialize",
        json!({ "protocolVersion": "1999-01-01" }),
    );
    assert_eq!(unknown["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
}

#[test]
fn tools_list_names_four_read_only_tools() {
    let tools = request(&server(None), "tools/list", json!({}))["result"]["tools"].clone();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["search_syntax", "get_docs", "classify", "project_symbols"]
    );
    for tool in tools.as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
}

#[test]
fn an_unknown_tool_is_a_protocol_error() {
    let response = request(
        &server(None),
        "tools/call",
        json!({ "name": "delete_everything", "arguments": {} }),
    );
    assert_eq!(response["error"]["code"], -32602);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("delete_everything"));
}

#[test]
fn bad_arguments_are_tool_errors_the_model_can_read() {
    let server = server(None);
    for (tool, arguments) in [
        ("search_syntax", json!({})),
        ("search_syntax", json!({ "query": "   " })),
        (
            "search_syntax",
            json!({ "query": "x", "category": "spell" }),
        ),
        ("search_syntax", json!({ "query": "x", "limit": 0 })),
        ("search_syntax", json!({ "query": 5 })),
        ("get_docs", json!({})),
        ("classify", json!({ "lines": "send hi" })),
        ("classify", json!({ "lines": [1, 2] })),
        ("project_symbols", json!({ "kind": "spells" })),
    ] {
        let (text, is_error) = call(&server, tool, arguments.clone());
        assert!(is_error, "{tool} {arguments} should fail, got {text}");
        assert!(!text.is_empty());
    }
    let response = request(
        &server,
        "tools/call",
        json!({ "name": "search_syntax", "arguments": [1] }),
    );
    assert_eq!(response["result"]["isError"], true);
}

#[test]
fn a_tool_call_before_the_catalog_arrives_says_so() {
    let server =
        Server::new(Arc::new(Pending::empty()), None).with_catalog_wait(Duration::from_millis(10));
    let (text, is_error) = call(&server, "search_syntax", json!({ "query": "send" }));
    assert!(is_error);
    assert!(text.contains("still loading"));
}

#[test]
fn a_degraded_catalog_is_flagged_in_every_answer() {
    let mut knowledge = knowledge();
    knowledge.degraded = Some("offline".into());
    let server = Server::new(Arc::new(Pending::ready(knowledge)), None);
    let answer = call_json(&server, "search_syntax", json!({ "query": "teleport" }));
    assert!(answer["warning"].as_str().unwrap().contains("offline"));
    let (docs, _) = call(&server, "get_docs", json!({ "name_or_id": "Teleport" }));
    assert!(docs.contains("offline"));
}

// ------------------------------------------------------------ search_syntax

#[test]
fn search_finds_action_bar_from_the_words_of_its_pattern() {
    let answer = call_json(
        &server(None),
        "search_syntax",
        json!({ "query": "send action bar" }),
    );
    let first = &answer["results"][0];
    assert_eq!(first["name"], "Action Bar");
    assert_eq!(first["category"], "effect");
    assert_eq!(first["addon"], "Skript");
    assert_eq!(first["since"], "2.3");
    assert_eq!(first["id"], "EffActionBar");
    assert!(first["patterns"][0]
        .as_str()
        .unwrap()
        .contains("action[ ]bar"));
    assert!(first["description"]
        .as_str()
        .unwrap()
        .starts_with("Sends an action bar"));
}

#[test]
fn search_reaches_addon_syntax_and_tolerates_a_typo() {
    let server = server(None);
    let toast = call_json(&server, "search_syntax", json!({ "query": "toast" }));
    assert_eq!(toast["results"][0]["name"], "Send Toast");
    assert_eq!(toast["results"][0]["addon"], "SkBee");

    let typo = call_json(&server, "search_syntax", json!({ "query": "teleprot" }));
    assert_eq!(typo["results"][0]["name"], "Teleport");

    let compact = call_json(&server, "search_syntax", json!({ "query": "actionbar" }));
    assert_eq!(compact["results"][0]["name"], "Action Bar");
}

#[test]
fn search_filters_by_category_and_caps_the_limit() {
    let server = server(None);
    let events = call_json(
        &server,
        "search_syntax",
        json!({ "query": "join", "category": "event" }),
    );
    assert!(events["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["category"] == "event"));
    assert_eq!(events["results"][0]["name"], "Join");

    let capped = call_json(
        &server,
        "search_syntax",
        json!({ "query": "s", "limit": 100_000 }),
    );
    assert!(capped["results"].as_array().unwrap().len() <= MAX_SEARCH_LIMIT);

    let none = call_json(&server, "search_syntax", json!({ "query": "zzzqqq" }));
    assert!(none["results"].as_array().unwrap().is_empty());
    assert!(none["hint"].is_string());
}

#[test]
fn search_results_never_exceed_the_text_cap() {
    // Hundreds of entries with long patterns: the answer must stay bounded
    // and still be valid JSON, with the cut reported.
    let long = "word ".repeat(200);
    let effects: Vec<Value> = (0..300)
        .map(|n| {
            json!({ "id": format!("e{n}"), "name": format!("Huge {n}"),
                    "description": [long.clone()],
                    "patterns": [format!("huge {n} {long}"), long.clone(), long.clone(),
                                 long.clone(), long.clone()] })
        })
        .collect();
    let docs = Docs::parse(&json!({ "effects": effects }).to_string()).unwrap();
    let knowledge = Knowledge {
        catalog: Catalog::build(docs),
        uninstalled: None,
        detection: Detection::default(),
        degraded: None,
    };
    let server = Server::new(Arc::new(Pending::ready(knowledge)), None);
    let answer = call_json(
        &server,
        "search_syntax",
        json!({ "query": "huge", "limit": 25 }),
    );
    let first = &answer["results"][0];
    assert_eq!(first["patterns"].as_array().unwrap().len(), 4);
    assert_eq!(first["more_patterns"], 1);
    assert!(first["patterns"][0].as_str().unwrap().chars().count() <= 201);
}

// ----------------------------------------------------------------- get_docs

#[test]
fn get_docs_renders_the_full_hover_card_by_name_or_id() {
    let server = server(None);
    let (by_name, is_error) = call(&server, "get_docs", json!({ "name_or_id": "action bar" }));
    assert!(!is_error);
    assert!(by_name.contains("**Action Bar**"));
    assert!(by_name.contains("send [the] action[ ]bar"));
    assert!(by_name.contains("send action bar \"Hello\" to player"));
    assert!(by_name.contains("since 2.3"));

    let (by_id, _) = call(&server, "get_docs", json!({ "name_or_id": "EffActionBar" }));
    assert_eq!(by_id, by_name);
}

#[test]
fn get_docs_names_the_addon_and_suggests_on_a_miss() {
    let server = server(None);
    let (toast, _) = call(&server, "get_docs", json!({ "name_or_id": "Send Toast" }));
    assert!(toast.starts_with("from SkBee"), "{toast}");

    let (miss, is_error) = call(&server, "get_docs", json!({ "name_or_id": "action bars" }));
    assert!(is_error);
    assert!(miss.contains("Closest: Action Bar (effect)"), "{miss}");
}

// ----------------------------------------------------------------- classify

#[test]
fn classify_matches_the_editors_classification() {
    let project = Project::new("classify");
    let server = server(Some(project.0.clone()));
    let answer = call_json(
        &server,
        "classify",
        json!({ "lines": [
            "on join:",
            "\tsend action bar \"hi\" to player",
            "\tsend toast \"x\" to player",
            "",
            "\t# just a comment",
            "\tgreet(player, 2)",
            "\tset {_x} to floor(2.5)",
            "\tmake player dance wildly",
            "\tthis is not skript at all",
            "function local_one(a: number):",
            "\tstop"
        ] }),
    );
    let lines = answer["lines"].as_array().unwrap();
    let by_line = |number: u64| {
        lines
            .iter()
            .find(|line| line["line"] == number)
            .unwrap_or_else(|| panic!("no result for line {number}: {answer}"))
    };

    assert_eq!(by_line(1)["kind"], "syntax");
    assert_eq!(by_line(1)["category"], "event");
    assert_eq!(by_line(1)["name"], "Join");

    assert_eq!(by_line(2)["category"], "effect");
    assert_eq!(by_line(2)["name"], "Action Bar");
    assert_eq!(by_line(2)["addon"], "Skript");

    assert_eq!(by_line(3)["name"], "Send Toast");
    assert_eq!(by_line(3)["addon"], "SkBee");

    // Blank lines are left out; comments are named as such.
    assert!(lines.iter().all(|line| line["line"] != 4));
    assert_eq!(by_line(5)["kind"], "comment");

    // A call to a project function is resolved against the project's files.
    assert_eq!(by_line(6)["kind"], "function call");
    assert_eq!(by_line(6)["name"], "greet");
    assert_eq!(by_line(6)["declared_in"], "scripts/main.sk:9");

    assert_eq!(by_line(8)["kind"], "unknown");
    assert!(by_line(8)["note"].as_str().unwrap().contains("DanceAddon"));

    assert_eq!(by_line(9)["kind"], "unknown");
    assert!(by_line(9).get("note").is_none());

    assert_eq!(by_line(10)["kind"], "declaration");
    assert_eq!(by_line(10)["declares"], "function");
    assert_eq!(by_line(10)["name"], "local_one");
}

#[test]
fn classify_caps_the_number_of_lines() {
    let lines: Vec<String> = (0..500)
        .map(|_| "\tteleport player to spawn".into())
        .collect();
    let answer = call_json(&server(None), "classify", json!({ "lines": lines }));
    let classified = answer["lines"].as_array().unwrap().len();
    assert!(classified <= tools::MAX_CLASSIFY_LINES);
    assert!(answer["skipped_lines"].as_str().unwrap().starts_with("300"));
}

// ---------------------------------------------------------- project_symbols

#[test]
fn project_symbols_lists_what_the_scripts_declare() {
    let project = Project::new("symbols");
    let answer = call_json(
        &server(Some(project.0.clone())),
        "project_symbols",
        json!({}),
    );
    assert_eq!(answer["scripts_read"], 2);
    let symbols = answer["symbols"].as_array().unwrap();
    let find = |name: &str| {
        symbols
            .iter()
            .find(|symbol| symbol["name"] == name)
            .unwrap_or_else(|| panic!("no symbol {name}: {answer}"))
    };

    let greet = find("greet");
    assert_eq!(greet["kind"], "function");
    assert_eq!(
        greet["signature"],
        "greet(p: player, times: number = 1) :: text"
    );
    assert_eq!(greet["file"], "scripts/main.sk");
    assert_eq!(greet["line"], 9);

    assert_eq!(find("helper")["kind"], "local function");
    assert_eq!(find("heal")["kind"], "command");
    assert_eq!(find("prefix")["kind"], "option");

    let wallet = find("Wallet");
    assert_eq!(wallet["kind"], "class");
    let shapes: Vec<&str> = wallet["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["shape"].as_str().unwrap())
        .collect();
    assert_eq!(
        shapes,
        vec!["balance: number", "deposit(amount: number) :: boolean"]
    );

    let message = find("Message");
    assert_eq!(message["kind"], "struct");
    assert_eq!(message["members"][0]["shape"], "contents: string");
}

#[test]
fn project_symbols_filters_by_kind_and_name_and_reports_a_limit() {
    let project = Project::new("filters");
    let server = server(Some(project.0.clone()));
    let functions = call_json(&server, "project_symbols", json!({ "kind": "function" }));
    let names: Vec<&str> = functions["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["greet", "helper"]);

    let structs = call_json(&server, "project_symbols", json!({ "kind": "struct" }));
    assert_eq!(structs["symbols"].as_array().unwrap().len(), 1);

    let named = call_json(&server, "project_symbols", json!({ "query": "WALL" }));
    assert_eq!(named["symbols"][0]["name"], "Wallet");

    let limited = call_json(&server, "project_symbols", json!({ "limit": 1 }));
    assert_eq!(limited["symbols"].as_array().unwrap().len(), 1);
    assert!(limited["limited"]
        .as_str()
        .unwrap()
        .contains("showing the first 1"));
}

#[test]
fn project_symbols_without_a_project_is_a_tool_error() {
    let (text, is_error) = call(&server(None), "project_symbols", json!({}));
    assert!(is_error);
    assert!(text.contains("No project"));
}

// ----------------------------------------------------------------- options

#[test]
fn parses_the_command_line() {
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        Options::parse(&args(&["--project", "/p", "--settings", "{}"])).unwrap(),
        Options {
            project: Some(PathBuf::from("/p")),
            settings: Some("{}".into()),
            help: false,
        }
    );
    assert!(Options::parse(&args(&["--help"])).unwrap().help);
    assert!(Options::parse(&args(&["--project"])).is_err());
    assert!(Options::parse(&args(&["--write-files"])).is_err());
}

#[test]
fn settings_keep_the_serde_defaults_when_none_are_given() {
    // `Settings::default()` would leave this empty and skip addon syntax.
    assert_eq!(read_settings(None).addon_syntax_source, "skripthub");
    assert_eq!(
        read_settings(Some("not json")).addon_syntax_source,
        "skripthub"
    );
    let minestom = read_settings(Some(r#"{"platform": "minestom"}"#));
    assert!(minestom.is_minestom());
    assert_eq!(minestom.addon_syntax_source, "skripthub");
}
