//! Zed extension entry point for Skript.
//!
//! The editor half of this extension — grammar, queries, indentation, snippets —
//! works with no language server at all. This file's only job is to find or
//! fetch `skript-lsp` and hand Zed a command to run it, degrading with a useful
//! message rather than an error when it cannot.
//!
//! Binary resolution follows the order every well-behaved Zed extension uses,
//! and which the extension registry guidelines require: an explicit user
//! setting wins, then a copy already on `$PATH`, and only then does the
//! extension download one. Extensions must never bundle the server itself.

use std::fs;

use zed::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result};

/// GitHub repository that publishes `skript-lsp` release binaries.
const RELEASE_REPO: &str = "DaisyCatTs/SkriptZed";

/// How long a successful version check stays good for. Without this, every Zed
/// restart hits the GitHub API and a rate-limited response would otherwise take
/// the language server down with it.
const UPDATE_CHECK_TTL_SECONDS: u64 = 24 * 60 * 60;

const NOT_FOUND_MESSAGE: &str = concat!(
    "Could not find or download skript-lsp.\n\n",
    "The Skript extension still provides syntax highlighting, indentation, ",
    "folding by indent, outline and snippets without it — completion, hover, ",
    "diagnostics, go-to-definition and rename need the language server.\n\n",
    "Install it, or point Zed at an existing build:\n\n",
    "  \"lsp\": { \"skript-lsp\": { \"binary\": { \"path\": \"/path/to/skript-lsp\" } } }",
);

struct SkriptExtension {
    /// Resolved once per extension process, then re-`stat`ed on each use so a
    /// binary deleted underneath us is re-resolved rather than reported as
    /// working.
    cached_binary_path: Option<String>,
}

impl SkriptExtension {
    fn language_server_binary(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree).ok();
        let binary_settings = settings.and_then(|settings| settings.binary);
        let configured_args = binary_settings
            .as_ref()
            .and_then(|binary| binary.arguments.clone());

        // 1. An explicit path in the user's settings always wins.
        if let Some(path) = binary_settings.and_then(|binary| binary.path) {
            return Ok(zed::Command {
                command: path,
                args: configured_args.unwrap_or_default(),
                env: worktree.shell_env(),
            });
        }

        // 2. A copy the user installed themselves.
        if let Some(path) = worktree.which(&exe_name("skript-lsp")) {
            return Ok(zed::Command {
                command: path,
                args: configured_args.unwrap_or_default(),
                env: worktree.shell_env(),
            });
        }

        // 3. Fall back to a Zed-managed download.
        Ok(zed::Command {
            command: self.download_binary(language_server_id)?,
            args: configured_args.unwrap_or_default(),
            env: worktree.shell_env(),
        })
    }

    fn download_binary(&mut self, language_server_id: &LanguageServerId) -> Result<String> {
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        // An install we made earlier is better than nothing if the network or
        // the GitHub API is unavailable, so find it before doing anything that
        // can fail.
        let existing = newest_local_install();

        if existing.is_some() && !update_check_due() {
            let path = existing.unwrap();
            self.cached_binary_path = Some(path.clone());
            return Ok(path);
        }

        match self.fetch_latest(language_server_id) {
            Ok(path) => {
                record_update_check();
                self.cached_binary_path = Some(path.clone());
                Ok(path)
            }
            Err(error) => match existing {
                Some(path) => {
                    // Reported rather than swallowed: the user is running a
                    // stale server and should know why.
                    println!("skript-lsp update check failed, using existing install: {error}");
                    self.cached_binary_path = Some(path.clone());
                    Ok(path)
                }
                None => {
                    zed::set_language_server_installation_status(
                        language_server_id,
                        &zed::LanguageServerInstallationStatus::Failed(error.clone()),
                    );
                    Err(format!("{NOT_FOUND_MESSAGE}\n\nUnderlying error: {error}"))
                }
            },
        }
    }

    fn fetch_latest(&self, language_server_id: &LanguageServerId) -> Result<String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            RELEASE_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (os, arch) = zed::current_platform();
        let asset_name = format!(
            "skript-lsp-{arch}-{os}.{extension}",
            arch = match arch {
                zed::Architecture::Aarch64 => "aarch64",
                zed::Architecture::X8664 => "x86_64",
                zed::Architecture::X86 =>
                    return Err("skript-lsp is not built for 32-bit x86".into()),
            },
            os = match os {
                zed::Os::Mac => "apple-darwin",
                zed::Os::Linux => "unknown-linux-gnu",
                zed::Os::Windows => "pc-windows-msvc",
            },
            extension = match os {
                zed::Os::Mac | zed::Os::Linux => "tar.gz",
                zed::Os::Windows => "zip",
            },
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| {
                format!(
                    "release {} has no asset named {asset_name}",
                    release.version
                )
            })?;

        let version_dir = version_dir(&release.version);
        let binary_path = format!("{version_dir}/{}", exe_name("skript-lsp"));

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(
                &asset.download_url,
                &version_dir,
                match os {
                    zed::Os::Mac | zed::Os::Linux => zed::DownloadedFileType::GzipTar,
                    zed::Os::Windows => zed::DownloadedFileType::Zip,
                },
            )
            .map_err(|error| format!("failed to download {asset_name}: {error}"))?;

            zed::make_file_executable(&binary_path)?;
            remove_other_versions(&version_dir);
        }

        Ok(binary_path)
    }
}

impl zed::Extension for SkriptExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        self.language_server_binary(language_server_id, worktree)
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(
            LspSettings::for_worktree(language_server_id.as_ref(), worktree)
                .ok()
                .and_then(|settings| settings.settings),
        )
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(
            LspSettings::for_worktree(language_server_id.as_ref(), worktree)
                .ok()
                .and_then(|settings| settings.initialization_options),
        )
    }

    /// Skript's completion labels carry the raw pattern (`give %item type% to
    /// %players%`), which reads much better split into the syntax itself and a
    /// dimmed detail than as one undifferentiated string.
    fn label_for_completion(
        &self,
        _language_server_id: &LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<zed::CodeLabel> {
        let name = completion.label;
        let detail = completion.detail.unwrap_or_default();

        let highlight = match completion.kind? {
            zed::lsp::CompletionKind::Function | zed::lsp::CompletionKind::Method => "function",
            zed::lsp::CompletionKind::Event => "function.builtin",
            zed::lsp::CompletionKind::Variable => "variable",
            zed::lsp::CompletionKind::Field | zed::lsp::CompletionKind::Property => "property",
            zed::lsp::CompletionKind::Class | zed::lsp::CompletionKind::Struct => "type",
            zed::lsp::CompletionKind::Constant => "constant",
            zed::lsp::CompletionKind::Keyword => "keyword",
            _ => "",
        };

        let mut code = name.clone();
        if !detail.is_empty() {
            code.push_str("  ");
            code.push_str(&detail);
        }

        Some(zed::CodeLabel {
            spans: vec![
                zed::CodeLabelSpan::literal(name.clone(), Some(highlight.to_string())),
                zed::CodeLabelSpan::literal(
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!("  {detail}")
                    },
                    Some("comment".to_string()),
                ),
            ],
            filter_range: (0..name.len()).into(),
            code,
        })
    }
}

// ------------------------------------------------------------------ helpers

fn exe_name(stem: &str) -> String {
    match zed::current_platform().0 {
        zed::Os::Windows => format!("{stem}.exe"),
        _ => stem.to_string(),
    }
}

fn version_dir(version: &str) -> String {
    format!("skript-lsp-{version}")
}

/// The newest `skript-lsp-*` directory already present in the extension's work
/// directory that actually contains a binary.
fn newest_local_install() -> Option<String> {
    let mut candidates: Vec<String> = fs::read_dir(".")
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("skript-lsp-"))
        .collect();

    // Compare version components numerically. Sorting the directory names
    // lexically put `skript-lsp-0.9.0` above `skript-lsp-0.10.0`, so once the
    // minor version reached double digits the offline fallback would launch an
    // older server than the one installed.
    candidates.sort_by_key(|name| std::cmp::Reverse(version_key(name)));

    candidates.into_iter().find_map(|dir| {
        let path = format!("{dir}/{}", exe_name("skript-lsp"));
        fs::metadata(&path)
            .is_ok_and(|stat| stat.is_file())
            .then_some(path)
    })
}

/// The numeric version components of a `skript-lsp-<version>` directory name.
///
/// Anything unparseable sorts lowest, so a stray directory never wins.
fn version_key(directory: &str) -> Vec<u64> {
    directory
        .trim_start_matches("skript-lsp-")
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn update_check_marker() -> &'static str {
    ".skript-lsp-update-check"
}

fn update_check_due() -> bool {
    let Ok(metadata) = fs::metadata(update_check_marker()) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    modified
        .elapsed()
        .map(|elapsed| elapsed.as_secs() >= UPDATE_CHECK_TTL_SECONDS)
        .unwrap_or(true)
}

fn record_update_check() {
    let _ = fs::write(update_check_marker(), b"");
}

/// Keeps only the version we just installed, so the work directory does not
/// accumulate a copy of every release ever downloaded.
fn remove_other_versions(keep: &str) {
    let Ok(entries) = fs::read_dir(".") else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with("skript-lsp-") && name != keep {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

zed::register_extension!(SkriptExtension);
