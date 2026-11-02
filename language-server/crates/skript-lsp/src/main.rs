//! `skript-lsp`: a language server for Skript.
//!
//! Speaks LSP 3.17 over **stdio**, which is what Zed (and every other editor)
//! expects. Nothing is written to stdout except protocol traffic; logs go to
//! stderr, because a stray `println!` corrupts the message stream.
//!
//! The server is built in layers so that each one degrades independently:
//!
//! * `skript-index` parses the open files and finds what they declare and use.
//!   It needs nothing external, so outline, folding, go-to-definition,
//!   find-references and rename work offline and immediately.
//! * `skript-docs` supplies meaning: hover text, completion and the
//!   effect/condition/expression classification behind semantic tokens. It is
//!   downloaded at runtime; if that fails, a small built-in catalog takes over
//!   and everything above still works.

mod analyzer;
mod calls;
mod capabilities;
mod code_actions;
mod colors;
mod completion;
mod convert;
mod custom_syntax;
mod diagnostics;
mod document;
mod entries;
mod hierarchy;
mod hover;
mod indexing;
mod lines;
mod links;
mod loading;
mod mcp;
mod navigation;
mod outline;
mod publish;
mod rcon;
mod semantic;
mod server;
mod settings;
mod signature;
mod variables;

use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::{LspService, Server};

use server::{Backend, State};

#[tokio::main]
async fn main() {
    // `mcp` is a different program sharing this binary and its catalog
    // loading: the opt-in knowledge server for an AI agent. Checked first so
    // `skript-lsp mcp --help` describes it rather than the language server.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("mcp") {
        std::process::exit(mcp::run(&args[1..]));
    }

    // Answered before anything else, because the alternative is a binary that
    // silently blocks on stdin. `skript-lsp --version` looks like it hung, and
    // there is otherwise no way to check which build Zed downloaded without
    // reading the JSON-RPC log.
    if std::env::args().skip(1).any(|argument| {
        matches!(
            argument.as_str(),
            "--version" | "-V" | "--help" | "-h" | "-?"
        )
    }) {
        println!("skript-lsp {}", env!("CARGO_PKG_VERSION"));
        println!();
        println!("A language server for Skript, speaking LSP over stdio.");
        println!("It is started by an editor, not run directly.");
        return;
    }

    // stdout is the protocol channel and must carry nothing else.
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        state: Arc::new(RwLock::new(State::default())),
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}
