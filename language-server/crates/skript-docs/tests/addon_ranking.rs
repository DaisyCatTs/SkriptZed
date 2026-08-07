use skript_docs::{Catalog, Docs, LineRole};

/// Core Skript and a detected addon coexist without either shadowing the other.
///
/// Loads exactly what a server running SkBee would load — core plus one addon,
/// 3,634 patterns — because that is what `load_everything` does: only addons
/// actually found in `plugins/` are merged.
///
/// Worth recording what this does *not* cover. Loading all 168 published addons
/// at once, which production never does, lets an addon shadow core syntax:
/// `send "hi" to player` then resolves to "send bungee player to server",
/// because Skript writes Message with optionals (`[to %commandsenders%]`) and
/// the specificity metric rewards the addon's flatter pattern. `first_in`
/// prefers core on an exact tie, but this is not a tie — the addon genuinely
/// scores higher. Fixing that properly means changing how specificity treats
/// optionals, which needs measuring across the whole catalog rather than a
/// bonus invented to make one line come out right.
#[test]
fn core_and_a_detected_addon_coexist() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/");
    let Ok(core) = std::fs::read_to_string(format!("{base}docs.json")) else {
        return;
    };
    let Ok(hub) = std::fs::read_to_string(format!("{base}addons.json")) else {
        return;
    };

    let mut docs = Docs::parse(&core).unwrap();
    let addon =
        skript_docs::skripthub::parse_filtered(&hub, |name| name.eq_ignore_ascii_case("SkBee"))
            .expect("skripthub parse");
    docs.merge(addon.docs);
    docs.resolve_versions();
    let c = Catalog::build(docs);
    eprintln!("catalog: {} patterns", c.pattern_count());

    // Real SkBee / addon lines a server owner would actually write.
    let lines = [
        "set {_c} to nbt compound of player",
        "set {_w} to a new world creator named \"lobby\"",
        "set {_b} to chunk data biome",
        "if {_n} is a blank nbt compound",
        "send \"hi\" to player",
    ];
    for l in lines {
        let hit = c.classify_line(l, LineRole::Statement);
        assert!(hit.is_some(), "{l:?} classified as nothing");
    }

    // The tie-break itself.
    let (id, _) = c
        .classify_line("send \"hi\" to player", LineRole::Statement)
        .expect("a message must classify");
    let entry = c.entry(id).expect("id resolves");
    assert!(
        entry.addon.is_none(),
        "an addon outranked core Skript: got {:?}",
        entry.name
    );

    // An addon pattern that is genuinely more specific still wins.
    let (id, _) = c
        .classify_line("if {_n} is a blank nbt compound", LineRole::Statement)
        .expect("SkBee syntax must classify");
    assert!(
        c.entry(id).is_some_and(|e| e.addon.is_some()),
        "a more specific addon pattern should still win"
    );
}
