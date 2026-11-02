//! Building the syntax catalog in the background at startup.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use skript_addons::Detection;
use skript_docs::{Catalog, Docs, DocsSource};
use tower_lsp::lsp_types::MessageType;

use crate::server::Backend;
use crate::settings::{Settings, MINESTOM_DOCS_NAME};

/// How long to wait before each new attempt when the syntax database could
/// not be loaded. A session started offline otherwise stayed on the tiny
/// built-in catalog until the editor was restarted.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(10 * 60),
];

impl Backend {
    /// Builds the catalog from the current settings in the background,
    /// installs it, and republishes every open document.
    ///
    /// Everything published before the catalog arrives was judged without it:
    /// no built-in functions, no addon or deprecation checks. The SkriptHub
    /// catalog is an 8 MB download, so that window is seconds long, and a file
    /// opened inside it kept showing `floor(…)` as undeclared until it was
    /// edited. Hence the republish.
    ///
    /// When the load degrades to the fallback catalog, it is retried after
    /// each of [`RETRY_DELAYS`]. A newer load (after a settings change)
    /// supersedes this one, which then stops without installing anything.
    pub async fn spawn_catalog_load(&self) {
        let (settings, roots, generation) = {
            let mut state = self.state.write().await;
            state.catalog_generation += 1;
            (
                state.settings.clone(),
                state.scan_roots(),
                state.catalog_generation,
            )
        };
        let backend = self.handle();
        tokio::spawn(async move {
            let attempt = || {
                let (settings, roots) = (settings.clone(), roots.clone());
                async move {
                    tokio::task::spawn_blocking(move || load_everything(&settings, &roots))
                        .await
                        .map_err(|error| error.to_string())
                }
            };
            let deliver = |loaded: Result<Loaded, String>, attempt: usize| {
                let backend = backend.handle();
                async move { backend.install(loaded, attempt, generation).await }
            };
            with_backoff(&RETRY_DELAYS, attempt, tokio::time::sleep, deliver).await;
        });
    }

    /// Hands one load attempt's result to the state and the user. Returns
    /// whether to try again.
    async fn install(
        &self,
        loaded: Result<Loaded, String>,
        attempt: usize,
        generation: u64,
    ) -> bool {
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("loading the Skript syntax database panicked: {error}"),
                    )
                    .await;
                return false;
            }
        };
        let degraded = loaded.degraded.is_some();
        // A retry that failed again changes nothing and is not worth a word.
        if attempt > 0 && degraded {
            return self.state.read().await.catalog_generation == generation;
        }
        {
            let mut state = self.state.write().await;
            if state.catalog_generation != generation {
                return false;
            }
            state.catalog = Some(loaded.catalog);
            state.uninstalled = loaded.uninstalled;
            state.detection = loaded.detection;
        }
        for message in &loaded.messages {
            self.client.log_message(MessageType::INFO, message).await;
        }
        // A pop-up, not a log line: the log pane is not somewhere a first-time
        // user thinks to look when completion is empty.
        if let Some(warning) = &loaded.degraded {
            self.client
                .show_message(MessageType::WARNING, warning)
                .await;
        } else if attempt > 0 {
            self.client
                .show_message(
                    MessageType::INFO,
                    "The Skript syntax database is now available; hover and completion are back.",
                )
                .await;
        }
        self.republish_open().await;
        degraded
    }
}

/// Runs `attempt` once, then again after each of `delays` for as long as
/// `deliver` asks for another try. `deliver` gets each result with its
/// attempt number, starting at 0.
async fn with_backoff<T, A, AF, S, SF, D, DF>(delays: &[Duration], attempt: A, sleep: S, deliver: D)
where
    A: Fn() -> AF,
    AF: Future<Output = T>,
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
    D: Fn(T, usize) -> DF,
    DF: Future<Output = bool>,
{
    let waits = std::iter::once(None).chain(delays.iter().copied().map(Some));
    for (number, wait) in waits.enumerate() {
        if let Some(wait) = wait {
            sleep(wait).await;
        }
        if !deliver(attempt().await, number).await {
            return;
        }
    }
}

/// Everything the background load produces.
pub struct Loaded {
    pub catalog: Catalog,
    /// Syntax for addons that are *not* installed, for the `requires-addon`
    /// diagnostic. `None` unless we know what is installed.
    pub uninstalled: Option<Catalog>,
    pub detection: Detection,
    pub messages: Vec<String>,
    /// Set when the syntax database could not be loaded and the tiny built-in
    /// catalog took over. Carried separately from `messages` because this one
    /// has to be *shown* to the user, not logged: the fallback has no events,
    /// expressions or conditions at all, so hover and completion go quiet, and
    /// a silent degradation reads exactly like a broken extension.
    pub degraded: Option<String>,
}

/// Where the downloaded syntax databases are cached.
///
/// Deliberately not the temp directory. `std::env::temp_dir()` is `/tmp` on
/// Linux unless `$TMPDIR` says otherwise, and `/tmp/skript-lsp` is a fixed,
/// predictable name in a world-writable sticky directory: any other local user
/// can create it first and leave a crafted `docs-latest.json` there, which the
/// server would then load as the authoritative description of the language.
/// `/tmp` is also swept by systemd-tmpfiles, which defeats the 24-hour cache TTL
/// and re-downloads roughly 9 MB far more often than intended.
///
/// Falls back to the temp directory only when the platform's cache location
/// cannot be determined, since a poor cache still beats refusing to start.
pub fn cache_directory() -> PathBuf {
    // Every branch checks `is_absolute`, not just the XDG one: a relative value
    // would put the cache wherever the server happened to be started from.
    let absolute = |path: PathBuf| path.is_absolute().then_some(path);

    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .and_then(absolute)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .and_then(absolute)
            .map(|home| home.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .and_then(absolute)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .and_then(absolute)
                    .map(|home| home.join(".cache"))
            })
    };

    base.unwrap_or_else(std::env::temp_dir).join("skript-lsp")
}

/// Detects the project's addons, then builds the catalog from every source.
///
/// Order matters: Skript's own database goes in first so that SkriptHub's
/// 1,237 duplicate copies of core syntax lose the id-based dedup to the
/// authoritative entries.
pub fn load_everything(settings: &Settings, roots: &[PathBuf]) -> Loaded {
    let cache_dir = cache_directory();
    let mut messages = Vec::new();

    // With `platform` unset, a skript-minestom server is recognised from its
    // folder layout, so nobody has to know the setting exists.
    let detected;
    let settings = match settings
        .platform_is_auto()
        .then(|| skript_addons::platform::detect_minestom(roots))
        .flatten()
    {
        Some(server) => {
            messages.push(format!(
                "skript-minestom server found at {}, using its syntax (set \"platform\": \"bukkit\" to override)",
                server.display()
            ));
            detected = Settings {
                platform: Some("minestom".into()),
                ..settings.clone()
            };
            &detected
        }
        None => settings,
    };

    let detection = detect_addons(settings, roots, &mut messages);
    let (mut docs, degraded) = load_core(settings, &cache_dir, &mut messages);

    // An explicit setting wins, then the installed Skript JAR, then whatever
    // database was loaded.
    let target_version = settings
        .skript_version
        .as_deref()
        .or_else(|| detection.skript_version())
        .or(Some(docs.source.version.as_str()))
        .and_then(skript_docs::version::Version::parse);

    let uninstalled = load_addons(settings, &detection, &mut docs, &cache_dir, &mut messages);

    // Its addons are written for Minestom and published only on its own docs
    // site, never on SkriptHub. There is no `plugins/` folder to detect them
    // from, and there are few of them, so every published one is loaded.
    if settings.is_minestom() {
        load_minestom_addons(&mut docs, &cache_dir, &mut messages);
    }

    load_custom(settings, &mut docs, &cache_dir, &mut messages);

    docs.resolve_versions();
    let catalog = Catalog::build(docs).with_target_version(target_version);

    if let Some(version) = target_version {
        messages.push(format!("targeting Skript {version}"));
    }

    Loaded {
        catalog,
        uninstalled,
        detection,
        messages,
        degraded,
    }
}

/// What the project's `plugins/` directory has installed.
fn detect_addons(settings: &Settings, roots: &[PathBuf], messages: &mut Vec<String>) -> Detection {
    let detection = if settings.addons.is_off() {
        Detection::default()
    } else {
        skript_addons::detect(roots)
    };

    if let Some(dir) = &detection.plugins_dir {
        messages.push(format!(
            "found {} plugin(s) in {}: {} Skript addon(s){}",
            detection.plugins.len(),
            dir.display(),
            detection.addons().count(),
            match detection.addon_names().as_slice() {
                [] => String::new(),
                names => format!(": {}", names.join(", ")),
            }
        ));
    }
    detection
}

/// Skript's own database, or the built-in fallback and a warning to show.
fn load_core(
    settings: &Settings,
    cache_dir: &Path,
    messages: &mut Vec<String>,
) -> (Docs, Option<String>) {
    let docs = match skript_docs::source::load(&settings.docs_source(), cache_dir) {
        Ok(docs) => docs,
        Err(error) => {
            let warning = format!(
                "Skript syntax database unavailable ({error}). Highlighting, indentation, \
                 folding, outline, go-to-definition and rename all still work, but hover and \
                 completion need the database and will be nearly empty until it downloads. \
                 Check your connection and restart the language server."
            );
            return (skript_docs::fallback_docs(), Some(warning));
        }
    };

    let flavour = if settings.is_minestom() {
        MINESTOM_DOCS_NAME
    } else {
        "Skript"
    };
    // skript-minestom's database carries no Skript version.
    let version = match docs.source.version.as_str() {
        "" => String::new(),
        version => format!(" {version}"),
    };
    messages.push(format!(
        "loaded {flavour}{version} syntax: {} entries, {} patterns",
        docs.total_entries(),
        docs.total_patterns(),
    ));
    (docs, None)
}

/// Merges SkriptHub's syntax for the wanted addons into `docs`, and returns a
/// catalog of every other addon's syntax when the installed set is known.
fn load_addons(
    settings: &Settings,
    detection: &Detection,
    docs: &mut Docs,
    cache_dir: &Path,
    messages: &mut Vec<String>,
) -> Option<Catalog> {
    let wanted: Vec<String> = match settings.addons.explicit() {
        Some(names) => names.to_vec(),
        None if settings.addons.is_off() => Vec::new(),
        None => detection.addon_names(),
    };
    if wanted.is_empty()
        || !settings
            .addon_syntax_source
            .eq_ignore_ascii_case("skripthub")
    {
        return None;
    }

    let text = match skript_docs::source::fetch_text(&DocsSource::SkriptHub, cache_dir) {
        Ok(text) => text,
        Err(error) => {
            messages.push(format!("could not fetch addon syntax: {error}"));
            return None;
        }
    };
    let matches = |name: &str| wanted.iter().any(|w| w.eq_ignore_ascii_case(name));

    match skript_docs::skripthub::parse_filtered(&text, matches) {
        Ok(addon_catalog) => {
            messages.push(format!(
                "loaded addon syntax for {}: {} patterns",
                wanted.join(", "),
                addon_catalog.docs.total_patterns(),
            ));
            docs.merge(addon_catalog.docs);
        }
        Err(error) => messages.push(format!("could not read addon syntax: {error}")),
    }

    // Only worth building when we know what is installed: without that, "this
    // addon is missing" is a guess.
    if !detection.is_known() {
        return None;
    }
    let rest = skript_docs::skripthub::parse_filtered(&text, |name| {
        !name.is_empty() && !matches(name) && !name.eq_ignore_ascii_case("Skript")
    })
    .ok()?;
    let mut rest_docs = rest.docs;
    rest_docs.resolve_versions();
    Some(Catalog::build(rest_docs))
}

/// User-supplied syntax files.
///
/// A folder stands for every `.json` in it, so a server's whole set of
/// generated docs (one file per addon, which is how `/sk gen-docs` writes
/// them) is one setting instead of a list kept in sync by hand.
fn load_custom(settings: &Settings, docs: &mut Docs, cache_dir: &Path, messages: &mut Vec<String>) {
    let mut custom_files: Vec<String> = Vec::new();
    for path in &settings.custom_syntax_paths {
        let candidate = Path::new(path);
        if !candidate.is_dir() {
            custom_files.push(path.clone());
            continue;
        }
        let mut found: Vec<String> = std::fs::read_dir(candidate)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|file| {
                        file.extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                    })
                    .map(|file| file.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        if found.is_empty() {
            messages.push(format!("no .json syntax files in {path}"));
        }
        custom_files.extend(found);
    }

    for path in &custom_files {
        match skript_docs::source::load(&DocsSource::Custom(path.into()), cache_dir) {
            Ok(custom) => {
                messages.push(format!(
                    "loaded custom syntax from {path}: {} entries",
                    custom.total_entries()
                ));
                docs.merge(custom);
            }
            Err(error) => messages.push(format!("could not load custom syntax {path}: {error}")),
        }
    }
}

/// The addon names in the skript-minestom docs site's manifest.
fn minestom_addon_names(manifest: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(manifest).map_err(|error| error.to_string())?;
    let addons = value
        .get("addons")
        .cloned()
        .ok_or_else(|| "it has no `addons` list".to_string())?;
    serde_json::from_value(addons).map_err(|error| error.to_string())
}

/// Merges every addon the skript-minestom docs site lists, each attributed to
/// its addon so hover can say where a line comes from. Failures are reported
/// and skipped: one unreachable addon must not cost the rest.
fn load_minestom_addons(docs: &mut Docs, cache_dir: &Path, messages: &mut Vec<String>) {
    use skript_docs::source::{minestom_doc_url, MINESTOM_DOCS_SITE, MINESTOM_MANIFEST_URL};

    let manifest = match skript_docs::source::fetch_text(
        &DocsSource::Url(MINESTOM_MANIFEST_URL.to_string()),
        cache_dir,
    ) {
        Ok(text) => text,
        Err(error) => {
            messages.push(format!(
                "could not fetch the skript-minestom addon list: {error}"
            ));
            return;
        }
    };
    // A manifest that does not parse used to read as "no addons", which is
    // indistinguishable from a site that publishes none.
    let names = match minestom_addon_names(&manifest) {
        Ok(names) => names,
        Err(error) => {
            messages.push(format!(
                "could not read the skript-minestom addon list: {error}"
            ));
            return;
        }
    };

    for name in names
        .iter()
        .filter(|name| !name.eq_ignore_ascii_case(MINESTOM_DOCS_NAME))
    {
        match skript_docs::source::load(&DocsSource::Url(minestom_doc_url(name)), cache_dir) {
            Ok(mut addon) => {
                addon.attribute_to(&skript_docs::model::AddonRef {
                    name: name.clone(),
                    url: MINESTOM_DOCS_SITE.to_string(),
                    since_version: None,
                });
                messages.push(format!(
                    "loaded skript-minestom addon {name}: {} entries",
                    addon.total_entries()
                ));
                docs.merge(addon);
            }
            Err(error) => messages.push(format!(
                "could not load skript-minestom addon {name}: {error}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Runs [`with_backoff`] against a scripted series of outcomes (`true`
    /// meaning "still degraded") and returns the waits it took and the
    /// attempts it delivered.
    async fn run(outcomes: &[bool]) -> (Vec<Duration>, Vec<usize>) {
        let waits = Arc::new(Mutex::new(Vec::new()));
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let next = Arc::new(Mutex::new(outcomes.iter().copied()));
        with_backoff(
            &RETRY_DELAYS,
            || {
                let next = next.clone();
                async move { next.lock().unwrap().next().unwrap_or(false) }
            },
            |wait| {
                waits.lock().unwrap().push(wait);
                async {}
            },
            |degraded, attempt| {
                delivered.lock().unwrap().push(attempt);
                async move { degraded }
            },
        )
        .await;
        let waits = waits.lock().unwrap().clone();
        let delivered = delivered.lock().unwrap().clone();
        (waits, delivered)
    }

    #[tokio::test]
    async fn a_healthy_load_is_not_retried() {
        let (waits, delivered) = run(&[false]).await;
        assert!(waits.is_empty());
        assert_eq!(delivered, vec![0]);
    }

    #[tokio::test]
    async fn a_degraded_load_is_retried_with_growing_delays_until_it_recovers() {
        let (waits, delivered) = run(&[true, true, false]).await;
        assert_eq!(waits, RETRY_DELAYS[..2].to_vec());
        assert_eq!(delivered, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn retrying_stops_after_the_last_delay() {
        let (waits, delivered) = run(&[true; 10]).await;
        assert_eq!(waits, RETRY_DELAYS.to_vec());
        assert_eq!(delivered, vec![0, 1, 2, 3]);
        assert!(RETRY_DELAYS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn reads_the_minestom_manifest() {
        assert_eq!(
            minestom_addon_names(r#"{"addons": ["skript-minestom", "SkBee"]}"#),
            Ok(vec!["skript-minestom".to_string(), "SkBee".to_string()])
        );
    }

    /// A broken manifest is reported, not mistaken for an empty addon list.
    #[test]
    fn a_malformed_minestom_manifest_is_an_error() {
        assert!(minestom_addon_names("<html>rate limited</html>").is_err());
        assert!(minestom_addon_names(r#"{"other": []}"#).is_err());
        assert!(minestom_addon_names(r#"{"addons": "SkBee"}"#).is_err());
    }
}
