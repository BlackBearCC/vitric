//! Hot-reload end-to-end: modify rules/scripts on disk → reload → behavior refreshes, world
//! unchanged;
//! broken code reload fails → old logic intact.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

/// Copy coin-run into a temp directory (the test modifies files, must not touch the shared
/// example).
fn copy_example(tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run");
    let dst = std::env::temp_dir().join(format!("vitric-reload-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    for sub in ["", "scenes", "rules", "scripts", "assets"] {
        fs::create_dir_all(dst.join(sub)).unwrap();
    }
    for rel in [
        "vitric.json",
        "schema.json",
        "animations.json",
        "scenes/main.json",
        "rules/game.json",
        "scripts/systems.js",
    ] {
        fs::copy(src.join(rel), dst.join(rel)).unwrap();
    }
    for entry in fs::read_dir(src.join("assets")).unwrap() {
        let p = entry.unwrap().path();
        fs::copy(&p, dst.join("assets").join(p.file_name().unwrap())).unwrap();
    }
    dst
}

#[test]
fn reload_swaps_rules_and_keeps_world() {
    let dir = copy_example("rules");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();

    // First eat one coin: score 1
    sim.inject_input("right", "pressed");
    for _ in 0..12 {
        sim.step(&mut rt).unwrap();
    }
    let player = sim.world.entity("player").unwrap();
    assert_eq!(sim.world.get_field(player, "Score.value").unwrap(), &json!(1));

    // Change the rule: coin value goes up 100× (add by cannot be changed to multiply by 100 as a
    // literal — change to by 100)
    let rules_path = dir.join("rules/game.json");
    let patched = fs::read_to_string(&rules_path)
        .unwrap()
        .replace(r#""by": "other.Coin.value""#, r#""by": 100"#);
    fs::write(&rules_path, patched).unwrap();

    let summary = rt.reload().unwrap();
    assert!(summary["rules"].as_array().unwrap().iter().any(|r| r == "collect-coin"));

    // World unchanged: score still 1, two coins left
    assert_eq!(sim.world.get_field(player, "Score.value").unwrap(), &json!(1));
    assert_eq!(sim.world.query(&["Coin"]).len(), 2);

    // New behavior takes effect: eat the next coin +100
    for _ in 0..12 {
        sim.step(&mut rt).unwrap();
    }
    assert_eq!(sim.world.get_field(player, "Score.value").unwrap(), &json!(101));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn broken_reload_keeps_old_logic_working() {
    let dir = copy_example("broken");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();

    // Write a broken script
    fs::write(dir.join("scripts/systems.js"), "syntax error (((").unwrap();
    let err = rt.reload().unwrap_err();
    assert!(err.contains("systems.js"), "错误要指到文件: {err}");

    // Old logic intact: runs normally, eats coins normally
    sim.inject_input("right", "pressed");
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    let player = sim.world.entity("player").unwrap();
    assert_eq!(sim.world.get_field(player, "Score.value").unwrap(), &json!(3));

    fs::remove_dir_all(&dir).unwrap();
}
