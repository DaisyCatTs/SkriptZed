//! Telling a skript-minestom server from a Bukkit one, from its folder alone.
//!
//! skript-minestom is a standalone server jar with Skript built in, and lays
//! its server out differently: Skript's data folder sits directly in the
//! server folder (`<server>/Skript/scripts`), not under `plugins/`, and its
//! `server.properties` is written by Minestom with keys Bukkit never uses
//! (`auth-type`, `dispatcher-threads`). Both facts come from skript-minestom's
//! source (`JavaPlugin.getDataFolder`, `PropertyUtils`).
//!
//! A wrong guess swaps out the whole syntax database, so only those markers
//! count. Script content is never used: skript-minestom keeps Skript's
//! structures, and nothing in a `.sk` file reliably names the platform.

use std::path::{Path, PathBuf};

/// `server.properties` keys only Minestom writes.
const MINESTOM_PROPERTIES: &[&str] = &["auth-type", "dispatcher-threads"];

/// Files at a Bukkit-family server root.
const BUKKIT_MARKERS: &[&str] = &[
    "bukkit.yml",
    "spigot.yml",
    "paper.yml",
    "config/paper-global.yml",
];

/// How far up from a workspace root to look.
const MAX_ASCENT: usize = 8;

/// Biggest `server.properties` read; the real one is about a kilobyte.
const MAX_PROPERTIES_BYTES: u64 = 64 * 1024;

/// The skript-minestom server folder a workspace belongs to, if it is one.
pub fn detect_minestom(roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().find_map(|root| minestom_server_above(root))
}

fn minestom_server_above(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    for _ in 0..MAX_ASCENT {
        let dir = current?;
        // The workspace may be the server folder, `Skript/`, or anything
        // inside it, such as `Skript/scripts/` itself.
        let server = if is_named(dir, "Skript") {
            dir.parent()
        } else if dir.join("Skript").is_dir() {
            Some(dir)
        } else {
            None
        };
        if let Some(server) = server.filter(|server| is_minestom_server(server)) {
            return Some(server.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Whether `server`, holding a `Skript/` folder, is a skript-minestom server.
fn is_minestom_server(server: &Path) -> bool {
    let skript = server.join("Skript");
    // A Bukkit server's Skript folder lives in `plugins/`, so a `Skript/`
    // there means this is a Bukkit plugin folder, not a Minestom server.
    if is_named(server, "plugins") || !skript.join("scripts").is_dir() {
        return false;
    }
    if BUKKIT_MARKERS
        .iter()
        .any(|marker| server.join(marker).is_file())
    {
        return false;
    }
    minestom_properties(&server.join("server.properties"))
}

fn minestom_properties(path: &Path) -> bool {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut text = String::new();
    if file
        .take(MAX_PROPERTIES_BYTES)
        .read_to_string(&mut text)
        .is_err()
    {
        return false;
    }
    text.lines().any(|line| {
        let key = line.split(['=', ':']).next().unwrap_or("").trim();
        MINESTOM_PROPERTIES.contains(&key)
    })
}

fn is_named(dir: &Path, name: &str) -> bool {
    dir.file_name()
        .and_then(|own| own.to_str())
        .is_some_and(|own| own.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(label: &str, properties: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skript-platform-test-{label}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Skript/scripts/sub")).unwrap();
        std::fs::write(dir.join("server.properties"), properties).unwrap();
        dir
    }

    /// The `server.properties` skript-minestom's `PropertyUtils` writes.
    const MINESTOM: &str =
        "server-ip=0.0.0.0\nserver-port=25565\nauth-type=ONLINE\ndispatcher-threads=4\n";
    /// A Paper server's, which has none of Minestom's keys.
    const PAPER: &str = "online-mode=true\nlevel-name=world\nserver-port=25565\n";

    #[test]
    fn finds_a_minestom_server_from_any_folder_inside_it() {
        let root = server("minestom", MINESTOM);
        for start in [
            root.clone(),
            root.join("Skript"),
            root.join("Skript/scripts"),
            root.join("Skript/scripts/sub"),
        ] {
            assert_eq!(
                detect_minestom(std::slice::from_ref(&start)),
                Some(root.clone()),
                "{start:?}"
            );
        }
    }

    #[test]
    fn a_bukkit_server_is_not_minestom() {
        // Skript under plugins/, as on Paper.
        let root = server("paper", PAPER);
        std::fs::create_dir_all(root.join("plugins/Skript/scripts")).unwrap();
        assert_eq!(
            detect_minestom(&[root.join("plugins/Skript/scripts")]),
            None
        );
        assert_eq!(detect_minestom(std::slice::from_ref(&root)), None);
    }

    #[test]
    fn minestom_keys_do_not_outvote_a_bukkit_marker() {
        let root = server("mixed", MINESTOM);
        std::fs::write(root.join("bukkit.yml"), "settings: {}\n").unwrap();
        assert_eq!(detect_minestom(&[root]), None);
    }

    #[test]
    fn a_skript_folder_without_minestom_properties_is_not_enough() {
        let root = server("bare", PAPER);
        assert_eq!(detect_minestom(&[root.join("Skript/scripts")]), None);
        let none = server("none", "");
        std::fs::remove_file(none.join("server.properties")).unwrap();
        assert_eq!(detect_minestom(&[none]), None);
    }
}
