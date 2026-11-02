//! User settings, and applying them to the running server.

use skript_docs::DocsSource;

use crate::server::State;

/// The skript-minestom syntax database's own name on its docs site. It is
/// both the core database's name and the one entry of the addon list that is
/// not an addon.
pub const MINESTOM_DOCS_NAME: &str = "skript-minestom";

/// How long one SkAnalyzer check may take when `analyzer.timeoutMs` is unset.
pub const DEFAULT_ANALYZER_TIMEOUT_MS: u64 = 15_000;

/// User-facing settings, passed through Zed's
/// `lsp.skript-lsp.initialization_options`.
///
/// `Default` is written out by hand and is what serde fills missing keys from.
/// It used to be derived, with the real defaults in field-level `serde(default
/// = ...)` attributes, so the two disagreed: a user with no settings at all got
/// the derived value, an empty `addonSyntaxSource`, and never any addon syntax.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Pin the syntax database to a specific Skript version.
    pub skript_version: Option<String>,
    /// A `docs.json` generated on your own server, to match its exact Skript
    /// build. Note this covers Skript itself only: `/sk gen-docs` is scoped to
    /// one addon, so it never contains a third-party addon's syntax.
    pub docs_path: Option<String>,
    /// Fetch the database from somewhere other than docs.skriptlang.org.
    pub docs_url: Option<String>,
    /// Report lines that match no known syntax. Off by default.
    pub unknown_syntax_diagnostics: bool,
    /// Report syntax upstream has marked deprecated. On by default.
    pub deprecated_syntax_diagnostics: bool,
    /// Report locals read but never set, or set but never read. On by
    /// default; heuristic, so it is the one most likely to be unwanted.
    pub local_variable_diagnostics: bool,
    /// Let the formatter put a blank line under a multi-line structure that
    /// runs straight into the next. It never removes one.
    pub format_blank_lines: bool,

    /// Which addons to load syntax for.
    ///
    /// `"auto"` reads the plugin manifests in the project's `plugins/`
    /// directory, `"off"` loads none, and a list names them explicitly.
    pub addons: AddonSetting,

    /// Where `plugins/` lives, when it is not at or above the workspace root.
    pub server_path: Option<String>,

    /// Where addon syntax comes from. `"skripthub"` or `"off"`.
    pub addon_syntax_source: String,

    /// Extra syntax files, in either Skript's or SkriptHub's schema. The escape
    /// hatch for private or unpublished addons.
    pub custom_syntax_paths: Vec<String>,

    /// `"minestom"` for a skript-minestom server: its own syntax database and
    /// the addons its docs site publishes, instead of Bukkit Skript's.
    /// Unset (or `"auto"`) detects one from the server folder; `"bukkit"`
    /// never does.
    pub platform: Option<String>,

    /// Check each saved script against the real Skript through SkAnalyzer.
    pub analyzer: AnalyzerSettings,

    /// Reload each saved script on a running server over RCON.
    pub rcon: RconSettings,

    /// One switch per editor feature, for anyone who wants less.
    pub features: Features,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            skript_version: None,
            docs_path: None,
            docs_url: None,
            unknown_syntax_diagnostics: false,
            deprecated_syntax_diagnostics: true,
            local_variable_diagnostics: true,
            format_blank_lines: true,
            addons: AddonSetting::default(),
            server_path: None,
            addon_syntax_source: "skripthub".to_string(),
            custom_syntax_paths: Vec::new(),
            platform: None,
            analyzer: AnalyzerSettings::default(),
            rcon: RconSettings::default(),
            features: Features::default(),
        }
    }
}

/// `features` in the settings. Everything here is on by default; the
/// integrations that start processes or contact servers (`analyzer`, `rcon`)
/// have their own blocks and are off by default.
///
/// The server still advertises every capability, so a switch takes effect
/// live: a disabled request is answered with nothing rather than unregistered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Features {
    pub hover: bool,
    pub completion: bool,
    /// Offering `{_var}` and list-variable levels seen elsewhere in the file.
    pub variable_completion: bool,
    pub signature_help: bool,
    /// Parameter names at call sites.
    pub inlay_hints: bool,
    /// The language server's colouring on top of tree-sitter's.
    pub semantic_tokens: bool,
    /// Swatches on `&c`, `<#ff0000>` and friends.
    pub colors: bool,
    /// Clickable URLs.
    pub links: bool,
    pub folding: bool,
    pub formatting: bool,
    pub code_actions: bool,
    pub rename: bool,
    /// Go to definition, references and same-symbol highlights.
    pub navigation: bool,
    pub call_hierarchy: bool,
    /// The outline and workspace symbol search.
    pub symbols: bool,
    /// Every diagnostic this server reports itself. The analyzer and RCON
    /// findings follow their own blocks.
    pub diagnostics: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            hover: true,
            completion: true,
            variable_completion: true,
            signature_help: true,
            inlay_hints: true,
            semantic_tokens: true,
            colors: true,
            links: true,
            folding: true,
            formatting: true,
            code_actions: true,
            rename: true,
            navigation: true,
            call_hierarchy: true,
            symbols: true,
            diagnostics: true,
        }
    }
}

/// `rcon` in the settings. Off by default: it contacts a server and changes
/// what that server runs.
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RconSettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// Never logged or shown; `Debug` redacts it.
    pub password: Option<String>,
    /// For the whole exchange: connect, log in, reload, read the reply.
    pub timeout_ms: u64,
    /// The server's `plugins/Skript/scripts`, when it cannot be found from
    /// the detected `plugins/` or the saved file's own path.
    pub scripts_root: Option<String>,
}

impl Default for RconSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".into(),
            port: 25575,
            password: None,
            timeout_ms: 5_000,
            scripts_root: None,
        }
    }
}

impl std::fmt::Debug for RconSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RconSettings")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("timeout_ms", &self.timeout_ms)
            .field("scripts_root", &self.scripts_root)
            .finish()
    }
}

/// `analyzer` in the settings. SkAnalyzer is LGPL and runs real Skript, which
/// is GPL; the user installs both, and this server only starts the process.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnalyzerSettings {
    /// Unset means "on when `jar` is set": pointing at a jar is already a
    /// deliberate opt-in, and a second switch to flip was a common stumble.
    pub enabled: Option<bool>,
    /// The SkAnalyzer shell jar.
    pub jar: Option<String>,
    /// A Java 25+ executable; `java` on the PATH by default.
    pub java: Option<String>,
    /// A folder the analyzer owns. Its `plugins/` must hold the Skratched
    /// Skript build; defaults to a folder in the server's cache.
    pub work_dir: Option<String>,
    /// Addon jars to load alongside Skript.
    pub plugins: Vec<String>,
    /// Per check; [`DEFAULT_ANALYZER_TIMEOUT_MS`] when unset.
    pub timeout_ms: Option<u64>,
}

impl AnalyzerSettings {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(self.jar.is_some())
    }
}

/// How addons are chosen.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(untagged)]
pub enum AddonSetting {
    /// `"auto"` or `"off"`.
    Mode(String),
    /// An explicit list of addon names.
    Named(Vec<String>),
}

impl Default for AddonSetting {
    fn default() -> Self {
        AddonSetting::Mode("auto".to_string())
    }
}

impl AddonSetting {
    pub fn is_off(&self) -> bool {
        matches!(self, AddonSetting::Mode(mode) if mode.eq_ignore_ascii_case("off"))
    }

    /// The explicitly named addons, if the user listed them.
    pub fn explicit(&self) -> Option<&[String]> {
        match self {
            AddonSetting::Named(names) => Some(names),
            _ => None,
        }
    }
}

impl Settings {
    /// `platform` unset or `"auto"`: detected from the server folder.
    pub fn platform_is_auto(&self) -> bool {
        self.platform
            .as_deref()
            .is_none_or(|platform| platform.eq_ignore_ascii_case("auto"))
    }

    pub fn is_minestom(&self) -> bool {
        self.platform
            .as_deref()
            .is_some_and(|platform| platform.eq_ignore_ascii_case("minestom"))
    }

    pub fn docs_source(&self) -> DocsSource {
        if let Some(path) = &self.docs_path {
            return DocsSource::Local(path.into());
        }
        if let Some(url) = &self.docs_url {
            return DocsSource::Url(url.clone());
        }
        if self.is_minestom() {
            return DocsSource::Url(skript_docs::source::minestom_doc_url(MINESTOM_DOCS_NAME));
        }
        match &self.skript_version {
            Some(version) => DocsSource::Version(version.clone()),
            None => DocsSource::Latest,
        }
    }
}

/// Every key [`Settings`] reads, as the client spells it.
const SETTING_KEYS: &[&str] = &[
    "skriptVersion",
    "docsPath",
    "docsUrl",
    "unknownSyntaxDiagnostics",
    "deprecatedSyntaxDiagnostics",
    "localVariableDiagnostics",
    "formatBlankLines",
    "addons",
    "serverPath",
    "addonSyntaxSource",
    "customSyntaxPaths",
    "platform",
    "analyzer",
    "rcon",
    "features",
];

/// Every key [`AnalyzerSettings`] reads.
const ANALYZER_KEYS: &[&str] = &["enabled", "jar", "java", "workDir", "plugins", "timeoutMs"];

/// Every key [`RconSettings`] reads.
const RCON_KEYS: &[&str] = &[
    "enabled",
    "host",
    "port",
    "password",
    "timeoutMs",
    "scriptsRoot",
];

/// Every key [`Features`] reads.
const FEATURE_KEYS: &[&str] = &[
    "hover",
    "completion",
    "variableCompletion",
    "signatureHelp",
    "inlayHints",
    "semanticTokens",
    "colors",
    "links",
    "folding",
    "formatting",
    "codeActions",
    "rename",
    "navigation",
    "callHierarchy",
    "symbols",
    "diagnostics",
];

/// Keeps each entry of `object` that `T` accepts on its own, and describes
/// each one it had to drop. `prefix` is prepended to reported key names.
/// Settings whose value must never be echoed back, even when it is invalid.
/// `rcon` itself is here for a password typed where the block should be.
const SECRET_SETTINGS: &[&str] = &["rcon.password", "rcon"];

fn valid_entries<T: serde::de::DeserializeOwned>(
    object: &serde_json::Map<String, serde_json::Value>,
    known: &[&str],
    prefix: &str,
    problems: &mut Vec<String>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut kept = serde_json::Map::new();
    for (key, value) in object {
        if !known.contains(&key.as_str()) {
            problems.push(format!("`{prefix}{key}` is not a setting"));
            continue;
        }
        let single =
            serde_json::Value::Object(serde_json::Map::from_iter([(key.clone(), value.clone())]));
        match serde_json::from_value::<T>(single) {
            Ok(_) => {
                kept.insert(key.clone(), value.clone());
            }
            // serde quotes the offending value ("invalid type: integer
            // `1234`"), which for a numeric RCON password put it in a
            // window and the log.
            Err(_) if SECRET_SETTINGS.contains(&format!("{prefix}{key}").as_str()) => problems
                .push(format!(
                    "`{prefix}{key}` has the wrong type; its value is not shown"
                )),
            Err(error) => problems.push(format!("`{prefix}{key}` is invalid ({error})")),
        }
    }
    kept
}

/// Lays the client's `settings` over its `initialization_options`.
///
/// Zed sends these separately (`lsp.skript-lsp.initialization_options` at
/// start, `lsp.skript-lsp.settings` at start and on every change), and the
/// later used to replace the former whole: adding `"features"` under
/// `settings` silently dropped the `serverPath` set in the other block.
/// Keys in `overlay` win; a nested block (`analyzer`, `rcon`, `features`) is
/// merged key by key, so switching off one feature keeps the rest.
pub fn layer_settings(base: &serde_json::Value, overlay: &serde_json::Value) -> serde_json::Value {
    let (Some(base), Some(overlay)) = (base.as_object(), overlay.as_object()) else {
        return if overlay.is_null() {
            base.clone()
        } else {
            overlay.clone()
        };
    };
    let mut merged = base.clone();
    for (key, value) in overlay {
        let layered = match (merged.get(key), value) {
            (Some(serde_json::Value::Object(inner)), serde_json::Value::Object(over)) => {
                let mut inner = inner.clone();
                inner.extend(over.iter().map(|(k, v)| (k.clone(), v.clone())));
                serde_json::Value::Object(inner)
            }
            _ => value.clone(),
        };
        merged.insert(key.clone(), layered);
    }
    serde_json::Value::Object(merged)
}

/// Reads settings one key at a time, keeping every valid value.
///
/// Deserialising the whole object at once threw away *every* setting when a
/// single value had the wrong type (`"addons": true` reset the docs path,
/// the diagnostics toggles and the analyzer too), and silently ignored a
/// misspelt key. The returned list names each dropped key, for one message to
/// the user. `None` when the value is not an object at all.
pub fn parse_settings(value: &serde_json::Value) -> (Option<Settings>, Vec<String>) {
    let Some(object) = value.as_object() else {
        return (
            None,
            vec![format!("expected an object of settings, got `{value}`")],
        );
    };
    let mut problems = Vec::new();
    let mut kept = serde_json::Map::new();
    for (key, value) in object {
        // Nested blocks are checked field by field too, so one bad field does
        // not reset the whole block (and switch the analyzer off).
        let fields = match (key.as_str(), value.as_object()) {
            ("analyzer", Some(fields)) => Some(valid_entries::<AnalyzerSettings>(
                fields,
                ANALYZER_KEYS,
                "analyzer.",
                &mut problems,
            )),
            ("rcon", Some(fields)) => Some(valid_entries::<RconSettings>(
                fields,
                RCON_KEYS,
                "rcon.",
                &mut problems,
            )),
            ("features", Some(fields)) => Some(valid_entries::<Features>(
                fields,
                FEATURE_KEYS,
                "features.",
                &mut problems,
            )),
            _ => None,
        };
        if let Some(fields) = fields {
            kept.insert(key.clone(), serde_json::Value::Object(fields));
            continue;
        }
        let single = serde_json::Map::from_iter([(key.clone(), value.clone())]);
        kept.extend(valid_entries::<Settings>(
            &single,
            SETTING_KEYS,
            "",
            &mut problems,
        ));
    }
    let settings = serde_json::from_value(serde_json::Value::Object(kept))
        .expect("every kept entry deserialised on its own");
    (Some(settings), problems)
}

impl Settings {
    /// Whether changing from `previous` to these settings changes what the
    /// catalog is built from, so it has to be loaded again.
    pub fn affects_catalog(&self, previous: &Settings) -> bool {
        self.docs_path != previous.docs_path
            || self.docs_url != previous.docs_url
            || self.skript_version != previous.skript_version
            || self.addons != previous.addons
            || self.server_path != previous.server_path
            || self.platform != previous.platform
            || self.custom_syntax_paths != previous.custom_syntax_paths
            || self.addon_syntax_source != previous.addon_syntax_source
    }
}

/// The settings that can change while the server runs, applied in one place
/// so `initialize` and a later configuration change cannot drift apart.
pub fn apply_settings(state: &mut State, settings: &Settings) {
    state.diagnostics = crate::diagnostics::Options {
        unknown_syntax: settings.unknown_syntax_diagnostics,
        deprecated_syntax: settings.deprecated_syntax_diagnostics,
        local_variables: settings.local_variable_diagnostics,
        // Not a setting: reapplying configuration must not make the server
        // forget it has already read the project.
        project_indexed: state.diagnostics.project_indexed,
    };
    // Changed analyzer settings take effect on the next save: the running
    // process is dropped, and an earlier setup failure is forgiven.
    if state.analyzer.settings != settings.analyzer {
        state.analyzer.settings = settings.analyzer.clone();
        state.analyzer.running = None;
        state.analyzer.given_up = false;
        if !settings.analyzer.is_enabled() {
            state.analyzer.findings.clear();
        }
    }
    // New RCON settings may fix whatever failed, so the next failure gets
    // its one window again.
    if state.rcon.settings != settings.rcon {
        state.rcon.settings = settings.rcon.clone();
        state.rcon.warned = false;
        if !settings.rcon.enabled {
            state.rcon.findings.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A numeric password is a type error, and serde's message quotes it.
    #[test]
    fn an_invalid_rcon_password_is_never_echoed() {
        for value in [
            serde_json::json!({ "rcon": { "password": 987654 } }),
            serde_json::json!({ "rcon": "987654" }),
        ] {
            let (_, problems) = parse_settings(&value);
            assert_eq!(problems.len(), 1, "{problems:?}");
            assert!(!problems[0].contains("987654"), "{problems:?}");
        }
        // Other settings keep serde's detail, which is what makes them fixable.
        let (_, problems) = parse_settings(&serde_json::json!({ "rcon": { "port": "x" } }));
        assert!(problems[0].contains("invalid type"), "{problems:?}");
    }

    #[test]
    fn settings_choose_the_right_docs_source() {
        let local = Settings {
            docs_path: Some("/tmp/docs.json".into()),
            ..Default::default()
        };
        assert!(matches!(local.docs_source(), DocsSource::Local(_)));

        let pinned = Settings {
            skript_version: Some("2.15.3".into()),
            ..Default::default()
        };
        assert!(matches!(pinned.docs_source(), DocsSource::Version(_)));

        assert!(matches!(
            Settings::default().docs_source(),
            DocsSource::Latest
        ));

        // skript-minestom has its own database, and the platform name is
        // matched case-insensitively.
        let minestom = Settings {
            platform: Some("Minestom".into()),
            ..Default::default()
        };
        assert!(minestom.is_minestom());
        let DocsSource::Url(url) = minestom.docs_source() else {
            panic!("a minestom server reads its docs from a URL");
        };
        assert_eq!(
            url,
            skript_docs::source::minestom_doc_url(MINESTOM_DOCS_NAME)
        );

        // An explicit path still wins over the platform.
        let pinned_minestom = Settings {
            platform: Some("minestom".into()),
            docs_path: Some("/srv/docs.json".into()),
            ..Default::default()
        };
        assert!(matches!(
            pinned_minestom.docs_source(),
            DocsSource::Local(_)
        ));
    }

    #[test]
    fn settings_deserialize_with_their_defaults() {
        let settings: Settings =
            serde_json::from_value(serde_json::json!({ "addons": ["SkBee"] })).unwrap();
        assert!(settings.deprecated_syntax_diagnostics);
        assert!(!settings.unknown_syntax_diagnostics);
        assert_eq!(settings.addon_syntax_source, "skripthub");
        assert_eq!(settings.addons.explicit(), Some(&["SkBee".to_string()][..]));

        let off: Settings = serde_json::from_value(serde_json::json!({ "addons": "OFF" })).unwrap();
        assert!(off.addons.is_off());
        assert!(!Settings::default().addons.is_off());
    }

    /// One bad value used to cost every other setting.
    #[test]
    fn a_bad_value_drops_only_itself_and_is_reported() {
        let (settings, problems) = parse_settings(&serde_json::json!({
            "addons": true,
            "docsPath": "/srv/docs.json",
            "unknownSyntaxDiagnostics": true,
            "docPath": "/typo",
            "analyzer": { "enabled": true, "timeoutMs": "soon", "jarr": "x" },
        }));
        let settings = settings.expect("an object");
        assert_eq!(settings.docs_path.as_deref(), Some("/srv/docs.json"));
        assert!(settings.unknown_syntax_diagnostics);
        assert_eq!(settings.addons, AddonSetting::default());
        assert_eq!(settings.analyzer.enabled, Some(true));
        assert_eq!(settings.analyzer.timeout_ms, None);

        let joined = problems.join("; ");
        for key in [
            "`addons`",
            "`docPath`",
            "`analyzer.timeoutMs`",
            "`analyzer.jarr`",
        ] {
            assert!(joined.contains(key), "{key} not reported: {joined}");
        }
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    #[test]
    fn valid_settings_report_nothing() {
        let (settings, problems) = parse_settings(&serde_json::json!({
            "addons": "off",
            "analyzer": { "jar": "a.jar" },
        }));
        assert!(problems.is_empty(), "{problems:?}");
        assert!(settings.unwrap().addons.is_off());
    }

    /// `Default` and serde's missing-key defaults used to disagree, so a
    /// user with no settings at all never got addon syntax.
    #[test]
    fn no_settings_and_empty_settings_agree() {
        let (parsed, problems) = parse_settings(&serde_json::json!({}));
        assert!(problems.is_empty());
        let parsed = parsed.unwrap();
        assert_eq!(parsed, Settings::default());
        assert_eq!(parsed.addon_syntax_source, "skripthub");
        assert!(parsed.deprecated_syntax_diagnostics);
        assert!(parsed.features.hover && parsed.features.diagnostics);
        assert!(!parsed.analyzer.is_enabled());
        assert!(!parsed.rcon.enabled);
    }

    #[test]
    fn a_feature_switch_drops_only_itself_when_invalid() {
        let (settings, problems) = parse_settings(&serde_json::json!({
            "features": { "hover": false, "inlayHints": "nope", "hovr": false },
        }));
        let features = settings.unwrap().features;
        assert!(!features.hover);
        assert!(features.inlay_hints, "an invalid value keeps the default");
        assert!(features.completion, "unmentioned switches stay on");
        let joined = problems.join("; ");
        assert!(joined.contains("`features.inlayHints`"), "{joined}");
        assert!(joined.contains("`features.hovr`"), "{joined}");
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn a_configured_jar_turns_the_analyzer_on_unless_switched_off() {
        let with_jar = |enabled: Option<bool>| AnalyzerSettings {
            enabled,
            jar: Some("shell.jar".into()),
            ..Default::default()
        };
        assert!(with_jar(None).is_enabled());
        assert!(!with_jar(Some(false)).is_enabled());
        assert!(!AnalyzerSettings::default().is_enabled());
        // Enabled without a jar is still "enabled"; the analyzer then reports
        // the missing jar once instead of silently doing nothing.
        let no_jar = AnalyzerSettings {
            enabled: Some(true),
            ..Default::default()
        };
        assert!(no_jar.is_enabled());
    }

    #[test]
    fn settings_layer_over_initialization_options() {
        let base = serde_json::json!({
            "serverPath": "/srv/mc",
            "features": { "hover": false, "links": false },
            "rcon": { "port": 25580 },
        });
        let overlay = serde_json::json!({
            "features": { "hover": true },
            "rcon": { "enabled": true },
            "addons": "off",
        });
        let merged = layer_settings(&base, &overlay);
        assert_eq!(merged["serverPath"], "/srv/mc", "an unrelated key survives");
        assert_eq!(merged["features"]["hover"], true, "the overlay wins");
        assert_eq!(merged["features"]["links"], false, "a nested block merges");
        assert_eq!(merged["rcon"]["port"], 25580);
        assert_eq!(merged["rcon"]["enabled"], true);
        assert_eq!(merged["addons"], "off");
        assert_eq!(layer_settings(&base, &serde_json::Value::Null), base);
    }

    #[test]
    fn a_non_object_is_rejected_whole() {
        let (settings, problems) = parse_settings(&serde_json::json!(["nope"]));
        assert!(settings.is_none());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn only_catalog_inputs_force_a_reload() {
        let base = Settings::default();
        let toggled = Settings {
            unknown_syntax_diagnostics: true,
            ..Default::default()
        };
        assert!(!toggled.affects_catalog(&base));
        for changed in [
            Settings {
                docs_path: Some("x".into()),
                ..Default::default()
            },
            Settings {
                addons: AddonSetting::Named(vec!["SkBee".into()]),
                ..Default::default()
            },
            Settings {
                platform: Some("minestom".into()),
                ..Default::default()
            },
            Settings {
                custom_syntax_paths: vec!["a.json".into()],
                ..Default::default()
            },
        ] {
            assert!(changed.affects_catalog(&base), "{changed:?}");
        }
    }

    #[test]
    fn apply_settings_updates_diagnostics_but_keeps_the_index_flag() {
        let mut state = State::default();
        state.diagnostics.project_indexed = true;
        let settings = Settings {
            unknown_syntax_diagnostics: true,
            deprecated_syntax_diagnostics: false,
            ..Default::default()
        };
        apply_settings(&mut state, &settings);
        assert!(state.diagnostics.unknown_syntax);
        assert!(!state.diagnostics.deprecated_syntax);
        assert!(
            state.diagnostics.project_indexed,
            "reapplying settings must not forget the project was read"
        );
    }

    #[test]
    fn changed_analyzer_settings_forgive_an_earlier_failure() {
        let mut state = State::default();
        state.analyzer.given_up = true;
        state
            .analyzer
            .findings
            .insert("file:///a.sk".into(), Vec::new());

        // Unchanged settings leave the failure alone.
        apply_settings(&mut state, &Settings::default());
        assert!(state.analyzer.given_up);

        // Enabling it is a change: the failure is forgiven.
        let enabled = Settings {
            analyzer: AnalyzerSettings {
                jar: Some("shell.jar".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        apply_settings(&mut state, &enabled);
        assert!(!state.analyzer.given_up);
        assert!(state.analyzer.settings.is_enabled());
        assert_eq!(state.analyzer.findings.len(), 1);

        // Turning it off again drops its stale findings.
        apply_settings(&mut state, &Settings::default());
        assert!(state.analyzer.findings.is_empty());
    }

    #[test]
    fn rcon_is_off_by_default_and_parsed_field_by_field() {
        assert!(!Settings::default().rcon.enabled);
        let (settings, problems) = parse_settings(&serde_json::json!({
            "rcon": { "enabled": true, "port": 70000, "password": "pw", "scriptsRoot": "/srv" },
        }));
        let rcon = settings.expect("an object").rcon;
        assert!(rcon.enabled);
        assert_eq!(rcon.port, 25575, "an out-of-range port falls back");
        assert_eq!(rcon.host, "127.0.0.1");
        assert_eq!(rcon.timeout_ms, 5_000);
        assert_eq!(rcon.scripts_root.as_deref(), Some("/srv"));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("`rcon.port`"));

        // Changed settings forgive an earlier warning; disabling drops findings.
        let mut state = State::default();
        state.rcon.warned = true;
        state
            .rcon
            .findings
            .insert("file:///a.sk".into(), Vec::new());
        let enabled = Settings {
            rcon: rcon.clone(),
            ..Default::default()
        };
        apply_settings(&mut state, &enabled);
        assert!(!state.rcon.warned);
        assert_eq!(state.rcon.findings.len(), 1);
        apply_settings(&mut state, &Settings::default());
        assert!(state.rcon.findings.is_empty());
    }
}
