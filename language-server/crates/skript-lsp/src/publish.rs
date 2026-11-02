//! Publishing diagnostics, the server's own and SkAnalyzer's.

use std::sync::Arc;

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, Location,
    MessageType, NumberOrString, Url,
};

use crate::analyzer;
use crate::convert::{to_lsp_range, Encoding};
use crate::diagnostics;
use crate::loading::cache_directory;
use crate::server::{Backend, State};
use crate::settings::DEFAULT_ANALYZER_TIMEOUT_MS;

impl Backend {
    /// Checks one document and sends its diagnostics.
    pub async fn publish(&self, uri: &str) {
        let Ok(url) = Url::parse(uri) else {
            return;
        };
        let found = {
            let state = self.state.read().await;
            let Some(document) = state.workspace.get(uri) else {
                return;
            };
            document_diagnostics(&state, document, &url)
        };
        self.client.publish_diagnostics(url, found, None).await;
    }

    /// Runs SkAnalyzer on a saved file and republishes it with the findings.
    pub async fn check_with_analyzer(&self, uri: Url) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let Some(analyzer) = self.analyzer_for_check().await else {
            return;
        };

        let outcome = analyzer.check(&path).await;
        let key = uri.to_string();
        match outcome {
            Ok(mut results) => {
                // Keyed by the path exactly as it was asked for.
                let findings = results.remove(&path).unwrap_or_default();
                self.state
                    .write()
                    .await
                    .analyzer
                    .findings
                    .insert(key.clone(), findings);
                self.publish(&key).await;
            }
            // One request the shell refused; the shell itself is fine.
            Err(
                error
                @ (analyzer::AnalyzerError::Rejected(_) | analyzer::AnalyzerError::BadPath(_)),
            ) => {
                self.client
                    .log_message(MessageType::WARNING, format!("SkAnalyzer: {error}"))
                    .await;
            }
            // The process is gone; the next save starts a new one.
            Err(error) => {
                self.state.write().await.analyzer.running = None;
                self.client
                    .log_message(MessageType::WARNING, format!("SkAnalyzer stopped: {error}"))
                    .await;
            }
        }
    }

    /// The running SkAnalyzer, starting it if it is enabled and not yet up.
    ///
    /// `None` when it is off, already given up on, or still starting for an
    /// earlier save: a check is skipped rather than queued behind a
    /// multi-second Java start-up.
    pub async fn analyzer_for_check(&self) -> Option<Arc<analyzer::Analyzer>> {
        let config = {
            let mut state = self.state.write().await;
            let settings = &state.analyzer.settings;
            if !settings.is_enabled() || state.analyzer.given_up || state.analyzer.starting {
                return None;
            }
            if let Some(running) = &state.analyzer.running {
                return Some(running.clone());
            }
            let Some(config) = analyzer_config(&state) else {
                state.analyzer.given_up = true;
                drop(state);
                self.client
                    .show_message(
                        MessageType::WARNING,
                        "SkAnalyzer is enabled but `analyzer.jar` is not set, so it stays off \
                         until its settings change or the language server restarts.",
                    )
                    .await;
                return None;
            };
            state.analyzer.starting = true;
            config
        };

        let plugins = config.work_dir.join("plugins");
        if let Err(error) = std::fs::create_dir_all(&plugins) {
            // Not fatal here: SkAnalyzer reports the missing folder itself,
            // with more context than this would.
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("could not create {}: {error}", plugins.display()),
                )
                .await;
        }

        let started = analyzer::Analyzer::start(config).await;
        let mut state = self.state.write().await;
        state.analyzer.starting = false;
        match started {
            Ok(analyzer) => {
                let analyzer = Arc::new(analyzer);
                state.analyzer.running = Some(analyzer.clone());
                Some(analyzer)
            }
            Err(error) => {
                // Every start failure is a setup problem (Java, the jar, the
                // Skript build), so retrying on each save would only repeat it.
                state.analyzer.given_up = true;
                drop(state);
                self.client
                    .show_message(
                        MessageType::WARNING,
                        format!(
                            "SkAnalyzer could not start: {error}. It stays off until its \
                             settings change or the language server restarts."
                        ),
                    )
                    .await;
                None
            }
        }
    }
}

/// How to start SkAnalyzer, or `None` when no jar is configured.
fn analyzer_config(state: &State) -> Option<analyzer::AnalyzerConfig> {
    let settings = &state.analyzer.settings;
    let jar = settings.jar.clone()?;
    // No addons named: load the ones detected in the server's plugins/, the
    // way SkEditor points SkAnalyzer at a plugins folder. Skript itself is
    // left out; the analyzer brings its own patched build.
    let plugins = if settings.plugins.is_empty() {
        state
            .detection
            .addons()
            .filter(|addon| !addon.is_skript)
            .map(|addon| addon.path.clone())
            .collect()
    } else {
        settings.plugins.iter().map(Into::into).collect()
    };
    Some(analyzer::AnalyzerConfig {
        java: settings
            .java
            .clone()
            .unwrap_or_else(|| "java".into())
            .into(),
        jar: jar.into(),
        work_dir: settings
            .work_dir
            .clone()
            .map(Into::into)
            .unwrap_or_else(|| cache_directory().join("analyzer")),
        plugins,
        timeout: std::time::Duration::from_millis(
            settings.timeout_ms.unwrap_or(DEFAULT_ANALYZER_TIMEOUT_MS),
        ),
    })
}

/// Every diagnostic for `document`: the server's checks, then SkAnalyzer's
/// findings from the last save.
fn document_diagnostics(
    state: &State,
    document: &skript_index::Document,
    url: &Url,
) -> Vec<Diagnostic> {
    // Switching diagnostics off silences this server's own checks only; the
    // analyzer and RCON have their own switches and were asked for explicitly.
    let found = if !state.settings.features.diagnostics {
        Vec::new()
    } else {
        diagnostics::check(
            document,
            &state.workspace,
            state.catalog.as_ref(),
            // Only offered when a `plugins/` directory was found: without one we
            // do not know what is installed, and saying an addon is missing would
            // be a guess.
            state
                .detection
                .is_known()
                .then_some(())
                .and(state.uninstalled.as_ref()),
            state.diagnostics,
        )
    };
    let text = document.text();
    let mut converted: Vec<Diagnostic> = found
        .into_iter()
        .map(|diagnostic| to_lsp_diagnostic(text, url, diagnostic, state.encoding))
        .collect();
    // Real Skript's verdict from the last save, kept until the next one, the
    // way a build check's results are.
    if let Some(found) = state.analyzer.findings.get(document.uri()) {
        converted.extend(analyzer_diagnostics(document, found, state.encoding));
    }
    // The running server's verdict on the last save, kept the same way.
    if let Some(found) = state.rcon.findings.get(document.uri()) {
        converted.extend(crate::rcon::server_diagnostics(
            document,
            found,
            state.encoding,
        ));
    }
    converted
}

fn to_lsp_diagnostic(
    text: &str,
    url: &Url,
    diagnostic: diagnostics::Diagnostic,
    encoding: Encoding,
) -> Diagnostic {
    Diagnostic {
        range: to_lsp_range(text, diagnostic.range, encoding),
        severity: Some(match diagnostic.severity {
            diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
            diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
            diagnostics::Severity::Hint => DiagnosticSeverity::HINT,
        }),
        code: Some(NumberOrString::String(diagnostic.code.to_string())),
        source: Some("skript".into()),
        // Semantic tokens carry a `deprecated` modifier, but only on the
        // syntax's literal spans and only if the theme opts in. The tag is the
        // theme-independent path, and it is what gives the line a
        // strikethrough rather than just a colour.
        // A local set but never read is faded as dead code the same way.
        tags: if diagnostic.code == "deprecated-syntax" {
            Some(vec![DiagnosticTag::DEPRECATED])
        } else if diagnostic.is_unnecessary() {
            Some(vec![DiagnosticTag::UNNECESSARY])
        } else {
            None
        },
        // Rendered as a link the user can jump to, which for a duplicate
        // declaration is the actual fix. Related locations are always in this
        // same file: every check that produces one works from a single
        // document's symbol tree.
        related_information: (!diagnostic.related.is_empty()).then(|| {
            diagnostic
                .related
                .into_iter()
                .map(|related| DiagnosticRelatedInformation {
                    location: Location {
                        uri: url.clone(),
                        range: to_lsp_range(text, related.range, encoding),
                    },
                    message: related.message,
                })
                .collect()
        }),
        message: diagnostic.message,
        ..Default::default()
    }
}

/// SkAnalyzer's findings as diagnostics. Skript reports a line, never a
/// column, so each one covers the line's code, indentation excluded.
pub fn analyzer_diagnostics(
    document: &skript_index::Document,
    findings: &[analyzer::Finding],
    encoding: Encoding,
) -> Vec<Diagnostic> {
    findings
        .iter()
        .map(|finding| {
            let line = document.line(finding.line);
            let start = skript_index::indent_width(line) as u32;
            let end = line.trim_end().len() as u32;
            let range = skript_index::Range::on_line(finding.line, start, end.max(start));
            Diagnostic {
                range: to_lsp_range(document.text(), range, encoding),
                severity: Some(match finding.severity {
                    analyzer::FindingSeverity::Error => DiagnosticSeverity::ERROR,
                    analyzer::FindingSeverity::Warning => DiagnosticSeverity::WARNING,
                    analyzer::FindingSeverity::Info => DiagnosticSeverity::HINT,
                }),
                code: Some(NumberOrString::String("skanalyzer".into())),
                source: Some("SkAnalyzer".into()),
                message: finding.message.clone(),
                ..Default::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(line: u32, severity: analyzer::FindingSeverity) -> analyzer::Finding {
        analyzer::Finding {
            line,
            severity,
            message: "Can't understand this condition/effect: frobnicate".into(),
        }
    }

    #[test]
    fn a_finding_covers_its_line_without_the_indentation() {
        let document =
            skript_index::Document::new("file:///t.sk", "on join:\n\t\tfrobnicate   \n\n");
        let found = analyzer_diagnostics(
            &document,
            &[
                finding(1, analyzer::FindingSeverity::Error),
                finding(2, analyzer::FindingSeverity::Info),
            ],
            Encoding::Utf16,
        );

        assert_eq!(found[0].range.start.character, 2);
        assert_eq!(found[0].range.end.character, 12);
        assert_eq!(found[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(found[0].source.as_deref(), Some("SkAnalyzer"));
        // An empty line yields an empty range, never an inverted one.
        assert_eq!(found[1].range.start, found[1].range.end);
        assert_eq!(found[1].severity, Some(DiagnosticSeverity::HINT));
    }

    #[test]
    fn analyzer_config_needs_a_jar_and_defaults_the_rest() {
        let mut state = State::default();
        assert!(analyzer_config(&state).is_none());

        state.analyzer.settings.jar = Some("shell.jar".into());
        let config = analyzer_config(&state).expect("configured");
        assert_eq!(config.java, std::path::PathBuf::from("java"));
        assert_eq!(
            config.timeout,
            std::time::Duration::from_millis(DEFAULT_ANALYZER_TIMEOUT_MS)
        );
        assert!(config.work_dir.ends_with("analyzer"));
    }
}
