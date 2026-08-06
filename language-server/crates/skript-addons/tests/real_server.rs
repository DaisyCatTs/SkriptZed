//! Scans a real Minecraft server directory, when one is available.
//!
//! Point `SKRIPT_TEST_SERVER` at a server root to run this. Without it the
//! test skips, so CI and a fresh clone stay green.

use std::path::PathBuf;

#[test]
fn scans_a_real_server_directory() {
    let Ok(root) = std::env::var("SKRIPT_TEST_SERVER") else {
        eprintln!("skipping: set SKRIPT_TEST_SERVER to a server root to run this");
        return;
    };
    let root = PathBuf::from(root);
    assert!(root.is_dir(), "{} is not a directory", root.display());

    let found = skript_addons::detect(std::slice::from_ref(&root));
    assert!(
        found.is_known(),
        "no plugins/ directory found under {}",
        root.display()
    );

    eprintln!(
        "plugins dir: {}",
        found.plugins_dir.as_ref().unwrap().display()
    );
    eprintln!("read {} plugin(s):", found.plugins.len());
    for plugin in &found.plugins {
        let tag = if plugin.is_skript {
            " [Skript]"
        } else if plugin.is_addon {
            " [addon]"
        } else {
            ""
        };
        eprintln!(
            "  {:<22} {:<12}{tag}   ({})",
            plugin.name,
            plugin.version,
            plugin.path.file_name().unwrap().to_string_lossy()
        );
    }
    eprintln!("Skript addons: {:?}", found.addon_names());
    eprintln!("Skript version: {:?}", found.skript_version());

    for plugin in &found.plugins {
        assert!(!plugin.name.is_empty());
    }
}
