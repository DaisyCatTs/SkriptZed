//! The newer checks against real scripts: Skript's own 587 test and example
//! files, and the sample project.
//!
//! Every finding on that corpus has to be one Skript itself would report.
//! Skript's tests deliberately contain a few errors, so the known findings
//! are listed below with the reason each is right; anything else fails the
//! test. Skips when `vendor/docs.json` or the corpus has not been fetched,
//! the same as `skript-docs/tests/event_audit.rs`.

use std::path::{Path, PathBuf};

use skript_docs::{Catalog, Docs};

use super::{check, Options};
use crate::server::State;

/// The checks this harness gates.
const CODES: &[&str] = &[
    super::empty_section::CODE,
    super::event_values::CODE,
    super::newer_syntax::CODE,
    super::unknown_event::CODE,
    super::unreachable::CODE,
];

/// `(file suffix, 1-based line, code)` of each finding Skript also reports.
/// All three are empty sections inside tests that expect a parse error;
/// Skript's config reader warns about each before the test runs.
const EXPECTED: &[(&str, u32, &str)] = &[
    // `command /entries_test_1:` with no body, "expect error for missing
    // trigger".
    ("tests/bukkit/command/StructCommand.sk", 19, "empty-section"),
    // `subcommand test:` with no body, "expect error for invalid subcommand".
    ("tests/bukkit/command/StructCommand.sk", 39, "empty-section"),
    // A `parse:` holding only "# intentionally empty to cause an error".
    ("tests/syntaxes/sections/SecParse.sk", 3, "empty-section"),
];

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn scripts(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scripts(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "sk") {
            out.push(path);
        }
    }
}

fn catalog(target: &str) -> Option<Catalog> {
    let path = repository().join("vendor/docs.json");
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).expect("vendor/docs.json is readable");
    let mut docs = Docs::parse(&text).expect("vendor/docs.json parses");
    docs.resolve_versions();
    Some(Catalog::build(docs).with_target_version(skript_docs::version::Version::parse(target)))
}

/// Every finding of the gated checks, as `(path, line, code, message)`.
fn findings(catalog: Catalog, files: &[PathBuf]) -> Vec<(String, u32, &'static str, String)> {
    let mut state = State::default();
    for file in files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        state
            .workspace
            .open(format!("file:///{}", file.display()), text);
    }
    state.catalog = Some(catalog);
    crate::custom_syntax::sync(&mut state);
    let catalog = state.catalog.as_ref();
    let options = Options {
        events_known: true,
        target_version_known: true,
        newer_syntax: true,
        ..Options::default()
    };

    let mut out = Vec::new();
    for document in state.workspace.documents() {
        for diagnostic in check(document, &state.workspace, catalog, None, options) {
            if CODES.contains(&diagnostic.code) {
                out.push((
                    document.uri().replace('\\', "/"),
                    diagnostic.range.start.line,
                    diagnostic.code,
                    diagnostic.message,
                ));
            }
        }
    }
    out.sort();
    out
}

fn corpus_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    scripts(&repository().join("vendor/skript-corpus"), &mut files);
    scripts(&repository().join("examples/sample-project"), &mut files);
    files
}

/// The bad cases still fire against the real database, so a silent corpus
/// run means the checks are quiet, not inert.
#[test]
fn each_check_fires_against_the_real_database() {
    let Some(catalog) = catalog("2.9.0") else {
        eprintln!("vendor/docs.json not fetched; skipping");
        return;
    };
    let mut state = State {
        catalog: Some(catalog),
        ..State::default()
    };
    let cases: &[(&str, &str, &str)] = &[
        (
            "function f():\n\tsend \"hi\" to player\n",
            super::event_values::CODE,
            "There's no player in a function event.",
        ),
        (
            "every 5 seconds:\n\tteleport player to spawn of world \"world\"\n",
            super::event_values::CODE,
            "There's no player in a periodical event.",
        ),
        (
            "on jion:\n\tstop\n",
            super::unknown_event::CODE,
            "Did you mean `on join`?",
        ),
        (
            "on join:\n\tstop\n\tsend \"a\"\n",
            super::unreachable::CODE,
            "Unreachable code.",
        ),
        (
            "on join:\n\tif player is op:\non quit:\n\tstop\n",
            super::empty_section::CODE,
            "Empty configuration section!",
        ),
    ];
    let options = Options {
        events_known: true,
        target_version_known: true,
        newer_syntax: true,
        ..Options::default()
    };
    for (source, code, message) in cases {
        state.workspace.open("file:///t.sk", *source);
        let document = state.workspace.get("file:///t.sk").unwrap();
        let found = check(
            document,
            &state.workspace,
            state.catalog.as_ref(),
            None,
            options,
        );
        assert!(
            found
                .iter()
                .any(|d| d.code == *code && d.message.contains(message)),
            "{source:?}: {found:#?}"
        );
    }
    let silent = [
        "on load:\n\tsend \"a\" to player\n",
        "on join:\n\tsend \"a\" to player\n",
        "command /a:\n\ttrigger:\n\t\tsend \"a\" to player\n",
        "function f():\n\tset {_p} to all players\n",
    ];
    for source in silent {
        state.workspace.open("file:///t.sk", source);
        let document = state.workspace.get("file:///t.sk").unwrap();
        let found = check(
            document,
            &state.workspace,
            state.catalog.as_ref(),
            None,
            options,
        );
        assert!(
            !found.iter().any(|d| CODES.contains(&d.code)),
            "{source:?}: {found:#?}"
        );
    }
}

/// What `requires-newer-skript` says about Skript's own scripts on an older
/// server. Not a gate: the corpus really does use new syntax. Run with
/// `--ignored --nocapture` to read the findings.
#[test]
#[ignore]
fn newer_syntax_on_an_older_target() {
    let Some(catalog) = catalog("2.10.0") else {
        return;
    };
    let found = findings(catalog, &corpus_files());
    for (path, line, code, message) in found.iter().filter(|f| f.2 == super::newer_syntax::CODE) {
        eprintln!("{path}:{}: {code}: {message}", line + 1);
    }
}

#[test]
fn real_scripts_report_only_what_skript_reports() {
    let files = corpus_files();
    let Some(catalog) = catalog("2.17.1") else {
        eprintln!("vendor/docs.json not fetched; skipping");
        return;
    };
    if files.len() < 100 {
        eprintln!("corpus not fetched; skipping");
        return;
    }
    let found = findings(catalog, &files);
    for (path, line, code, message) in &found {
        eprintln!("{path}:{}: {code}: {message}", line + 1);
    }
    let unexpected: Vec<_> = found
        .iter()
        .filter(|(path, line, code, _)| {
            !EXPECTED.iter().any(|(suffix, at, expected)| {
                path.ends_with(suffix) && *at == line + 1 && expected == code
            })
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "{} unexpected finding(s): {unexpected:#?}",
        unexpected.len()
    );
}
