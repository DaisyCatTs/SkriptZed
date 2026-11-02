//! `skript-lsp mcp`: a read-only Skript knowledge server for an AI agent,
//! speaking the Model Context Protocol over stdio.
//!
//! Strictly opt-in. Nothing starts it but an explicit `mcp` argument, and the
//! Zed extension does not register it: a user who wants it adds it to Zed's
//! `context_servers` themselves (see the README).
//!
//! No tool writes a file or talks to a game server. The one side effect is the
//! language server's own: the syntax databases are downloaded into the same
//! cache, so both share one copy.
//!
//! MCP over stdio is JSON-RPC 2.0 with one message per line. That is small
//! enough to implement on serde_json here, rather than pull in an SDK and a
//! second async stack for four read-only tools.

mod project;
mod search;
mod tools;

#[cfg(test)]
mod tests;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use skript_addons::Detection;
use skript_docs::Catalog;

use crate::loading::{load_everything, Loaded};
use crate::settings::{parse_settings, Settings};

/// Newest first. The client's version is echoed when it is one of these, and
/// the newest offered otherwise, as the spec's negotiation rule asks. Zed
/// 0.233 requests 2025-03-26; its main branch requests 2025-11-25.
const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// How long a tool call waits for the catalog before answering "still
/// loading". Below Zed's 60 second tool timeout, so the model gets a useful
/// answer instead of a transport error.
const CATALOG_WAIT: Duration = Duration::from_secs(45);

/// A single message larger than this is refused unread. Nothing legitimate
/// comes close, and reading an unbounded line would let one runaway client
/// message exhaust memory.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

const INSTRUCTIONS: &str = "Read-only knowledge about the Skript scripting language (Minecraft) \
     and the open project. Use search_syntax to find syntax by meaning, get_docs for one entry's \
     full documentation, classify to see what lines of Skript resolve to, and project_symbols \
     for the functions, commands, options and classes the project declares. Skript syntax is \
     defined by Skript and its addons at runtime, so prefer these tools over guessing patterns.";

const HELP: &str = "\
skript-lsp mcp: a read-only Skript knowledge server (Model Context Protocol over stdio)

Usage: skript-lsp mcp [--project <dir>] [--settings <json>]

  --project <dir>    The project to read scripts and plugins/ from.
                     Defaults to the working directory.
  --settings <json>  The language server's settings object, e.g.
                     '{\"addons\": [\"SkBee\"], \"platform\": \"minestom\"}'.

It is started by an editor's AI agent, and only when the user enables it.
Its tools never write files and never contact a game server.";

/// What the catalog load produced, in the shape the tools read.
pub struct Knowledge {
    pub catalog: Catalog,
    pub uninstalled: Option<Catalog>,
    pub detection: Detection,
    /// Set when the real database could not be loaded and the built-in
    /// fallback stands in. Every tool answer carries it, because a model
    /// told "no such syntax" by a near-empty catalog would believe it.
    pub degraded: Option<String>,
}

impl From<Loaded> for Knowledge {
    fn from(loaded: Loaded) -> Self {
        Self {
            catalog: loaded.catalog,
            uninstalled: loaded.uninstalled,
            detection: loaded.detection,
            degraded: loaded.degraded,
        }
    }
}

/// The catalog, which arrives from a background thread a few seconds after
/// start. `initialize` must not wait for it: Zed gives a server a bounded time
/// to answer, and a cold SkriptHub download alone can take longer.
pub struct Pending {
    slot: Mutex<Option<Arc<Knowledge>>>,
    arrived: Condvar,
}

impl Pending {
    pub fn empty() -> Self {
        Self {
            slot: Mutex::new(None),
            arrived: Condvar::new(),
        }
    }

    #[cfg(test)]
    pub fn ready(knowledge: Knowledge) -> Self {
        let pending = Self::empty();
        pending.set(knowledge);
        pending
    }

    pub fn set(&self, knowledge: Knowledge) {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *slot = Some(Arc::new(knowledge));
        self.arrived.notify_all();
    }

    fn wait(&self, timeout: Duration) -> Option<Arc<Knowledge>> {
        let slot = self
            .slot
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (slot, _) = self
            .arrived
            .wait_timeout_while(slot, timeout, |slot| slot.is_none())
            .unwrap_or_else(|poison| poison.into_inner());
        slot.clone()
    }
}

/// A JSON-RPC error object.
#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    const PARSE: i64 = -32700;
    const INVALID_REQUEST: i64 = -32600;
    const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The protocol half: one parsed message in, at most one response out.
/// Kept free of I/O so tests drive it directly.
pub struct Server {
    knowledge: Arc<Pending>,
    project: Option<PathBuf>,
    catalog_wait: Duration,
}

impl Server {
    pub fn new(knowledge: Arc<Pending>, project: Option<PathBuf>) -> Self {
        Self {
            knowledge,
            project,
            catalog_wait: CATALOG_WAIT,
        }
    }

    #[cfg(test)]
    pub fn with_catalog_wait(mut self, wait: Duration) -> Self {
        self.catalog_wait = wait;
        self
    }

    /// Handles one line of input, as [`run`] does in pieces. `None` when
    /// nothing is owed in return: a notification, or a response to a request
    /// we never send.
    #[cfg(test)]
    pub fn handle(&self, line: &str) -> Option<String> {
        let response = match serde_json::from_str::<Value>(line) {
            Ok(message) => self.handle_value(message)?,
            Err(error) => error_response(
                Value::Null,
                RpcError::new(RpcError::PARSE, format!("parse error: {error}")),
            ),
        };
        // serde_json escapes newlines inside strings, so the compact form is
        // always exactly one line, which is what stdio framing requires.
        Some(response.to_string())
    }

    /// Handles one parsed message, or a batch of them.
    pub fn handle_value(&self, message: Value) -> Option<Value> {
        match message {
            // Batches were part of 2025-03-26, which Zed 0.233 speaks.
            Value::Array(batch) if !batch.is_empty() => {
                let responses: Vec<Value> = batch
                    .into_iter()
                    .filter_map(|item| self.handle_one(item))
                    .collect();
                (!responses.is_empty()).then_some(Value::Array(responses))
            }
            message => self.handle_one(message),
        }
    }

    fn handle_one(&self, message: Value) -> Option<Value> {
        let Value::Object(object) = message else {
            return Some(error_response(
                Value::Null,
                RpcError::new(RpcError::INVALID_REQUEST, "expected a JSON-RPC object"),
            ));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method") else {
            // A response to something we sent. This server sends no requests.
            return None;
        };
        let Some(method) = method.as_str() else {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                RpcError::new(RpcError::INVALID_REQUEST, "`method` must be a string"),
            ));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);

        let result = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools::list()),
            "tools/call" => self.call(&params),
            _ if method.starts_with("notifications/") => return None,
            _ => Err(RpcError::new(
                RpcError::METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            )),
        };

        // A notification gets no answer, even an error.
        let id = id?;
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => error_response(id, error),
        })
    }

    fn call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(RpcError::INVALID_PARAMS, "missing tool `name`"))?;
        let tool = tools::Tool::from_name(name).ok_or_else(|| {
            RpcError::new(RpcError::INVALID_PARAMS, format!("unknown tool: {name}"))
        })?;
        let empty = serde_json::Map::new();
        // Argument problems are tool results with `isError`, not protocol
        // errors (MCP 2025-11-25): the model can read them and correct itself.
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Ok(tool_result(Err("`arguments` must be an object".into()))),
        };

        let knowledge =
            if tool.needs_catalog() {
                match self.knowledge.wait(self.catalog_wait) {
                    Some(knowledge) => Some(knowledge),
                    None => return Ok(tool_result(Err(
                        "The Skript syntax database is still loading. Try again in a few seconds."
                            .into(),
                    ))),
                }
            } else {
                None
            };

        let context = tools::Context {
            knowledge: knowledge.as_deref(),
            project: self.project.as_deref(),
        };
        Ok(tool_result(tool.run(arguments, &context)))
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| SUPPORTED_VERSIONS.contains(version))
        .unwrap_or(SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "skript-lsp",
            "title": "Skript knowledge (read-only)",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

fn tool_result(outcome: Result<String, String>) -> Value {
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

fn error_response(id: Value, error: RpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": error.code, "message": error.message },
    })
}

/// One line read off the input.
#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    Message(String),
    /// Over [`MAX_MESSAGE_BYTES`]; its bytes were consumed and dropped.
    TooLong,
}

/// Reads the next newline-delimited message, without its line ending.
/// `Ok(None)` at end of input. Blank lines are skipped.
pub fn read_message<R: BufRead>(reader: &mut R, limit: usize) -> std::io::Result<Option<Incoming>> {
    loop {
        let mut buffer = Vec::new();
        let read = <&mut R as std::io::Read>::take(&mut *reader, limit as u64 + 1)
            .read_until(b'\n', &mut buffer)?;
        if read == 0 {
            return Ok(None);
        }
        if buffer.len() > limit && buffer.last() != Some(&b'\n') {
            // Drain the rest of the oversized line so the next read starts
            // at a message boundary, without buffering it: `read_until` here
            // held the whole line in memory, which is what the cap prevents.
            loop {
                let available = reader.fill_buf()?;
                if available.is_empty() {
                    break;
                }
                match available.iter().position(|&byte| byte == b'\n') {
                    Some(end) => {
                        reader.consume(end + 1);
                        break;
                    }
                    None => {
                        let length = available.len();
                        reader.consume(length);
                    }
                }
            }
            return Ok(Some(Incoming::TooLong));
        }
        let text = String::from_utf8_lossy(&buffer);
        let text = text.trim_end_matches(['\n', '\r']);
        if text.trim().is_empty() {
            continue;
        }
        return Ok(Some(Incoming::Message(text.to_string())));
    }
}

/// Command-line options after `mcp`.
#[derive(Debug, Default, PartialEq)]
struct Options {
    project: Option<PathBuf>,
    settings: Option<String>,
    help: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Options::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let flag = arg.as_str();
            let mut value = || {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag {
                "--project" => options.project = Some(PathBuf::from(value()?)),
                "--settings" => options.settings = Some(value()?),
                "--help" | "-h" => options.help = true,
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(options)
    }
}

/// Settings as the language server reads them. Built through serde even
/// when none are given: `Settings::default()` skips the serde defaults, so it
/// would leave the addon source empty and never load addon syntax.
fn read_settings(raw: Option<&str>) -> Settings {
    let fallback = || {
        parse_settings(&json!({}))
            .0
            .expect("an empty object is valid settings")
    };
    let Some(raw) = raw else {
        return fallback();
    };
    let value = match serde_json::from_str::<Value>(raw) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("skript-lsp mcp: ignoring --settings, it is not JSON: {error}");
            return fallback();
        }
    };
    let (settings, problems) = parse_settings(&value);
    for problem in problems {
        eprintln!("skript-lsp mcp: ignored setting: {problem}");
    }
    settings.unwrap_or_else(fallback)
}

/// Runs the server until stdin closes. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("skript-lsp mcp: {error}\n\n{HELP}");
            return 2;
        }
    };
    if options.help {
        // Safe on stdout: with --help no protocol session starts.
        println!("{HELP}");
        return 0;
    }

    // Zed starts a local context server in the project root, so the working
    // directory is the right default.
    let project = options
        .project
        .or_else(|| std::env::current_dir().ok())
        .filter(|path| path.is_dir());
    let settings = read_settings(options.settings.as_deref());

    let pending = Arc::new(Pending::empty());
    spawn_catalog_load(settings, project.clone(), pending.clone());

    let server = Arc::new(Server::new(pending, project));
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let mut stdin = std::io::stdin().lock();

    loop {
        let incoming = match read_message(&mut stdin, MAX_MESSAGE_BYTES) {
            Ok(Some(incoming)) => incoming,
            Ok(None) => return 0,
            Err(error) => {
                eprintln!("skript-lsp mcp: reading stdin failed: {error}");
                return 1;
            }
        };
        let message = match incoming {
            Incoming::Message(text) => serde_json::from_str::<Value>(&text),
            Incoming::TooLong => {
                let error = RpcError::new(RpcError::INVALID_REQUEST, "message too large");
                write_line(&stdout, &error_response(Value::Null, error));
                continue;
            }
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                let error = RpcError::new(RpcError::PARSE, format!("parse error: {error}"));
                write_line(&stdout, &error_response(Value::Null, error));
                continue;
            }
        };

        // A tool call can wait on the catalog, so it runs on its own thread
        // and a `ping` sent meanwhile is still answered.
        let is_tool_call = message.get("method").and_then(Value::as_str) == Some("tools/call");
        if is_tool_call {
            let (server, stdout) = (server.clone(), stdout.clone());
            std::thread::spawn(move || {
                if let Some(response) = server.handle_value(message) {
                    write_line(&stdout, &response);
                }
            });
        } else if let Some(response) = server.handle_value(message) {
            write_line(&stdout, &response);
        }
    }
}

fn write_line(stdout: &Mutex<std::io::Stdout>, message: &Value) {
    let mut stdout = stdout.lock().unwrap_or_else(|poison| poison.into_inner());
    // A closed stdout means the client is gone; the read loop ends next.
    let _ = writeln!(stdout, "{message}").and_then(|()| stdout.flush());
}

/// Loads the catalog exactly as the language server does, off the main
/// thread. Progress goes to stderr, which Zed keeps in its log.
fn spawn_catalog_load(settings: Settings, project: Option<PathBuf>, pending: Arc<Pending>) {
    std::thread::spawn(move || {
        // Where `plugins/` is looked for: the configured server folder, else
        // the project, as `State::scan_roots` decides for the editor.
        let roots: Vec<PathBuf> = match &settings.server_path {
            Some(path) => vec![PathBuf::from(path)],
            None => project.into_iter().collect(),
        };
        let loaded = load_everything(&settings, &roots);
        for message in &loaded.messages {
            eprintln!("skript-lsp mcp: {message}");
        }
        if let Some(warning) = &loaded.degraded {
            eprintln!("skript-lsp mcp: {warning}");
        }
        pending.set(Knowledge::from(loaded));
    });
}
