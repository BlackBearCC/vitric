//! Snapshot integrity: take a snapshot at a "dirty moment" — carryover (events emitted by the script last tick) non-empty,
//! pending_inputs (injected but undigested input) non-empty — after restore, continuing to run must be bit-identical to the original trajectory.
//! Snapshot tests at clean tick boundaries do not catch these two gaps; this test plugs them specifically.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

/// Minimal project: the script emits a pulse event every tick (carryover necessarily straddles ticks),
/// the rule counts both pulse and input into a counter (losing either changes the state hash).
fn write_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-snap-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "rules", "scripts"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        json!({
            "name": "snap-test",
            "schema": "schema.json",
            "entry": "scenes/main.json",
            "scenes": ["scenes/main.json"],
            "rules": ["rules/game.json"],
            "scripts": ["scripts/systems.js"],
            "seed": 7
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        json!({"components": {
            "Pulse": {"fields": {"count": {"type": "int", "default": 0}}}
        }})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        json!({"entities": [{"name": "counter", "components": {"Pulse": {}}}]}).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("rules/game.json"),
        json!({"rules": [
            {"id": "count-pulse", "on": {"event": "pulse"},
             "do": [{"add": "@counter.Pulse.count", "by": 1}]},
            {"id": "count-input", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
             "do": [{"add": "@counter.Pulse.count", "by": 100}]}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scripts/systems.js"),
        r#"vitric.system("pulser", { query: ["Pulse"], writes: [] }, (entities, ctx) => {
            ctx.emit("pulse", {});
        });"#,
    )
    .unwrap();
    dir
}

#[test]
fn dirty_moment_snapshot_restores_identical_trajectory() {
    let dir = write_project("dirty");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }
    // Manufacture a dirty moment: there is undigested input, carryover holds last tick's pulse
    sim.inject_input("right", "pressed");
    let snap = sim.snapshot(&rt);

    // Original trajectory continues for 30 ticks
    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }
    let h_direct = sim.world.state_hash();
    let count_direct = {
        let c = sim.world.entity("counter").unwrap();
        sim.world.get_field(c, "Pulse.count").unwrap().clone()
    };

    // Fresh-process semantics: re-boot then restore, then run 30 more ticks — must be bit-identical
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.restore(&snap, &mut rt2).unwrap();
    for _ in 0..30 {
        sim2.step(&mut rt2).unwrap();
    }
    assert_eq!(sim2.world.state_hash(), h_direct, "restore 后轨迹分歧（快照漏状态）");
    let c2 = sim2.world.entity("counter").unwrap();
    assert_eq!(sim2.world.get_field(c2, "Pulse.count").unwrap(), &count_direct);
    // The injected input did take effect (+100 happens on both sides)
    assert!(count_direct.as_i64().unwrap() >= 100, "pending input 在快照里丢了");

    let _ = fs::remove_dir_all(&dir);
}
