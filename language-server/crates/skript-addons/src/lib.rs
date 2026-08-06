//! Detecting which Skript addons a project actually uses.
//!
//! Scripts cannot declare their dependencies — Skript has no `require`, no
//! `import` for addons, and no project manifest of any kind. A script simply
//! uses addon syntax and fails at parse time if the plugin is missing. So the
//! only way to know which addons are in play is to look at the server the
//! scripts belong to.
//!
//! That matters because the alternative is loading all 168 addons SkriptHub
//! knows about — 12,877 patterns of syntax for plugins the user does not run,
//! which makes completion noisy and "unknown syntax" meaningless.
//!
//! Detection reads the **plugin manifest inside each JAR**, never the filename.
//! Filenames in the wild look like `EssentialsX-2.22.0 (1).jar` (a browser's
//! duplicate suffix) and `LuckPerms-Bukkit-5.5.50.jar` (a platform infix);
//! parsing them is guesswork, and the manifest is authoritative.

pub mod manifest;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use manifest::Manifest;

/// A plugin found in a `plugins/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedAddon {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    /// Whether it declares a dependency on Skript.
    pub is_addon: bool,
    /// Whether this *is* Skript. Its version is the best available answer for
    /// the user's target Skript version.
    pub is_skript: bool,
}

/// What a scan of a project found.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    /// The `plugins/` directory, when one was found.
    ///
    /// `None` means we do not know the environment — and the server must then
    /// stay quiet rather than claim an addon is missing.
    pub plugins_dir: Option<PathBuf>,
    pub plugins: Vec<DetectedAddon>,
}

impl Detection {
    /// The Skript addons, ignoring ordinary plugins.
    pub fn addons(&self) -> impl Iterator<Item = &DetectedAddon> {
        self.plugins.iter().filter(|plugin| plugin.is_addon)
    }

    pub fn addon_names(&self) -> Vec<String> {
        self.addons().map(|addon| addon.name.clone()).collect()
    }

    /// The installed Skript version, if Skript itself was found.
    pub fn skript_version(&self) -> Option<&str> {
        self.plugins
            .iter()
            .find(|plugin| plugin.is_skript)
            .map(|plugin| plugin.version.as_str())
            .filter(|version| !version.is_empty())
    }

    /// True when we actually know what is installed.
    pub fn is_known(&self) -> bool {
        self.plugins_dir.is_some()
    }
}

/// Scans for a `plugins/` directory at or above each root, and reads every JAR.
///
/// Scripts usually live at `plugins/Skript/scripts/`, so the workspace root is
/// often several levels below the directory we need.
pub fn detect(roots: &[PathBuf]) -> Detection {
    let Some(plugins_dir) = roots.iter().find_map(|root| find_plugins_dir(root)) else {
        return Detection::default();
    };

    let mut plugins = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jar") {
                continue;
            }
            if let Some(detected) = read_jar(&path) {
                plugins.push(detected);
            }
        }
    }

    plugins.sort_by_key(|a| a.name.to_lowercase());

    Detection {
        plugins_dir: Some(plugins_dir),
        plugins,
    }
}

/// Walks up from `start` looking for a `plugins/` directory.
///
/// Bounded, so opening `/` or a deep path does not turn into a filesystem walk.
fn find_plugins_dir(start: &Path) -> Option<PathBuf> {
    const MAX_ASCENT: usize = 8;

    let mut current = Some(start);
    for _ in 0..MAX_ASCENT {
        let dir = current?;

        // The root may itself be the plugins directory, or contain one.
        if dir.file_name().is_some_and(|name| name == "plugins") && dir.is_dir() {
            return Some(dir.to_path_buf());
        }
        let candidate = dir.join("plugins");
        if candidate.is_dir() {
            return Some(candidate);
        }

        current = dir.parent();
    }
    None
}

/// Reads a plugin's manifest out of its JAR.
///
/// A JAR is a ZIP; `paper-plugin.yml` is tried first because SkBee — and a
/// growing number of modern addons — ship only that one.
fn read_jar(path: &Path) -> Option<DetectedAddon> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let manifest = ["paper-plugin.yml", "plugin.yml"]
        .into_iter()
        .find_map(|name| {
            let mut entry = archive.by_name(name).ok()?;
            let mut text = String::new();
            entry.read_to_string(&mut text).ok()?;
            manifest::parse(&text)
        })?;

    Some(DetectedAddon {
        is_addon: manifest.is_skript_addon(),
        is_skript: manifest.is_skript_itself(),
        name: manifest.name,
        version: manifest.version,
        path: path.to_path_buf(),
    })
}

/// A cheap fingerprint of a `plugins/` directory, for deciding whether a rescan
/// is needed. Changes when a JAR is added, removed or replaced.
pub fn fingerprint(plugins_dir: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return 0;
    };

    let mut stamps: Vec<(String, u64)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jar"))
        .map(|entry| {
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0);
            (entry.file_name().to_string_lossy().to_string(), modified)
        })
        .collect();

    stamps.sort();
    stamps.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a JAR containing one manifest, so detection is exercised against
    /// a real ZIP rather than a stub.
    fn write_jar(dir: &Path, filename: &str, manifest_name: &str, body: &str) -> PathBuf {
        let path = dir.join(filename);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file::<_, ()>(manifest_name, Default::default())
            .unwrap();
        zip.write_all(body.as_bytes()).unwrap();
        zip.finish().unwrap();
        path
    }

    fn fixture(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skript-addons-test-{label}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        dir
    }

    #[test]
    fn detects_an_addon_that_ships_only_paper_plugin_yml() {
        let root = fixture("paper");
        let plugins = root.join("plugins");
        write_jar(
            &plugins,
            "SkBee-3.6.0.jar",
            "paper-plugin.yml",
            "name: SkBee\nversion: '3.6.0'\ndependencies:\n  server:\n    Skript:\n      required: true\n",
        );

        let found = detect(std::slice::from_ref(&root));
        assert!(found.is_known());
        let names = found.addon_names();
        assert_eq!(names, vec!["SkBee"], "SkBee was not detected: {names:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_a_classic_plugin_yml_addon_and_ignores_other_plugins() {
        let root = fixture("classic");
        let plugins = root.join("plugins");
        write_jar(
            &plugins,
            "skript-reflect.jar",
            "plugin.yml",
            "name: skript-reflect\nversion: 2.5.1\nsoftdepend: [Skript]\n",
        );
        write_jar(
            &plugins,
            "LuckPerms-Bukkit-5.5.50.jar",
            "plugin.yml",
            "name: LuckPerms\nversion: 5.5.50\nsoftdepend: [Vault]\n",
        );

        let found = detect(std::slice::from_ref(&root));
        assert_eq!(found.plugins.len(), 2, "both plugins should be read");
        assert_eq!(found.addon_names(), vec!["skript-reflect"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn awkward_filenames_do_not_matter() {
        // The exact names from a real server. A filename parser would read the
        // version as `(1)` or the name as `LuckPerms-Bukkit`.
        let root = fixture("filenames");
        let plugins = root.join("plugins");
        write_jar(
            &plugins,
            "EssentialsX-2.22.0 (1).jar",
            "plugin.yml",
            "name: Essentials\nversion: 2.22.0\nsoftdepend: [Vault]\n",
        );
        write_jar(
            &plugins,
            "packetevents-spigot-2.13.0 (1).jar",
            "plugin.yml",
            "name: packetevents\nversion: 2.13.0\n",
        );

        let found = detect(std::slice::from_ref(&root));
        let essentials = found
            .plugins
            .iter()
            .find(|plugin| plugin.name == "Essentials")
            .expect("name should come from the manifest, not the filename");
        assert_eq!(essentials.version, "2.22.0");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_the_plugins_directory_from_deep_inside_the_scripts_folder() {
        // Scripts really do live at plugins/Skript/scripts/, several levels
        // below the directory we need.
        let root = fixture("ascent");
        let plugins = root.join("plugins");
        write_jar(
            &plugins,
            "SkBee.jar",
            "paper-plugin.yml",
            "name: SkBee\nversion: '3.6.0'\ndependencies:\n  server:\n    Skript:\n      required: true\n",
        );
        let scripts = plugins.join("Skript").join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();

        let found = detect(&[scripts]);
        assert_eq!(found.addon_names(), vec!["SkBee"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_skripts_own_version() {
        let root = fixture("skript");
        write_jar(
            &root.join("plugins"),
            "Skript.jar",
            "plugin.yml",
            "name: Skript\nversion: 2.16.1\n",
        );

        let found = detect(std::slice::from_ref(&root));
        assert_eq!(found.skript_version(), Some("2.16.1"));
        // Skript is not one of its own addons.
        assert!(found.addon_names().is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_plugins_directory_means_we_know_nothing() {
        // The distinction that gates the `requires-addon` diagnostic: an empty
        // result because there is no server is not the same as an empty result
        // because the server has no addons.
        let dir = std::env::temp_dir().join("skript-addons-test-none");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let found = detect(std::slice::from_ref(&dir));
        assert!(!found.is_known());
        assert!(found.plugins.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_or_manifestless_jar_is_skipped_quietly() {
        let root = fixture("corrupt");
        let plugins = root.join("plugins");
        std::fs::write(plugins.join("broken.jar"), b"this is not a zip").unwrap();
        write_jar(&plugins, "nomanifest.jar", "README.txt", "nothing here");

        let found = detect(std::slice::from_ref(&root));
        assert!(found.is_known());
        assert!(found.plugins.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_fingerprint_changes_when_a_jar_appears() {
        let root = fixture("fingerprint");
        let plugins = root.join("plugins");
        let before = fingerprint(&plugins);

        write_jar(&plugins, "New.jar", "plugin.yml", "name: New\nversion: 1\n");
        assert_ne!(before, fingerprint(&plugins));

        let _ = std::fs::remove_dir_all(&root);
    }
}
