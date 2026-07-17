//! Scene switching (the load-scene convention event) end-to-end:
//! the menu → level → next-level flow must happen entirely inside the deterministic pipeline —
//! the switch is triggered by a rule emit, recording replay reproduces it bit-by-bit, snapshots recover across switches.
//! The Persist marker component = the cross-scene survival convention (player/score/inventory need no new system).

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::{self, Runtime};

/// Flow test project: menu/two levels/conflict scene/anonymous Persist scene + a set of switching rules.
/// `hero` carries Persist to survive across scenes; gold bookkeeping verifies start fires only once,
/// scene-loaded fires once per switch (+100 vs +10 fingerprints cannot be confused).
fn write_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-scenes-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "rules", "assets"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        json!({
            "name": "scene-flow",
            "schema": "schema.json",
            "entry": "scenes/menu.json",
            "scenes": [
                "scenes/menu.json", "scenes/level1.json", "scenes/level2.json",
                "scenes/clash.json", "scenes/anon.json"
            ],
            "rules": ["rules/flow.json"],
            "seed": 5
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        json!({"components": {
            "Persist": {"fields": {}},
            "Player": {"fields": {"gold": {"type": "int", "default": 0}}},
            "Tag": {"fields": {"label": {"type": "text", "default": ""}}},
            "Sprite": {"fields": {
                "image": {"type": "text", "default": ""},
                "w": {"type": "number", "default": 1},
                "h": {"type": "number", "default": 1}
            }}
        }})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/menu.json"),
        json!({"entities": [
            {"name": "title", "components": {"Tag": {"label": "menu"}}},
            {"name": "hero", "components": {"Player": {}, "Persist": {}}}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/level1.json"),
        json!({"entities": [{"name": "room1", "components": {"Tag": {"label": "level1"}}}]})
            .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/level2.json"),
        json!({"entities": [{"name": "room2", "components": {"Tag": {"label": "level2"}}}]})
            .to_string(),
    )
    .unwrap();
    // A scene that name-collides with the survivor hero: switching to it must error explicitly
    fs::write(
        dir.join("scenes/clash.json"),
        json!({"entities": [{"name": "hero", "components": {"Tag": {"label": "clash"}}}]})
            .to_string(),
    )
    .unwrap();
    // A scene with an anonymous Persist entity: switching away from it must error explicitly
    fs::write(
        dir.join("scenes/anon.json"),
        json!({"entities": [
            {"components": {"Persist": {}}},
            {"name": "room-anon", "components": {"Tag": {"label": "anon"}}}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("rules/flow.json"),
        json!({"rules": [
            {"id": "boot-bonus", "on": {"event": "start"},
             "do": [{"add": "@hero.Player.gold", "by": 100}]},
            {"id": "scene-bonus", "on": {"event": "scene-loaded"},
             "do": [{"add": "@hero.Player.gold", "by": 10}]},
            {"id": "earn", "on": {"event": "input", "filter": {"action": "earn", "phase": "pressed"}},
             "do": [{"add": "@hero.Player.gold", "by": 1}]},
            {"id": "start-game", "on": {"event": "input", "filter": {"action": "start", "phase": "pressed"}},
             "do": [{"emit": "load-scene", "data": {"scene": "scenes/level1.json"}}]},
            {"id": "next-level", "on": {"event": "input", "filter": {"action": "next", "phase": "pressed"}},
             "do": [{"emit": "load-scene", "data": {"scene": "scenes/level2.json"}}]},
            {"id": "warp-nowhere", "on": {"event": "input", "filter": {"action": "warp", "phase": "pressed"}},
             "do": [{"emit": "load-scene", "data": {"scene": "scenes/ghost.json"}}]},
            {"id": "to-clash", "on": {"event": "input", "filter": {"action": "clash", "phase": "pressed"}},
             "do": [{"emit": "load-scene", "data": {"scene": "scenes/clash.json"}}]},
            {"id": "to-anon", "on": {"event": "input", "filter": {"action": "anon", "phase": "pressed"}},
             "do": [{"emit": "load-scene", "data": {"scene": "scenes/anon.json"}}]},
            {"id": "double-load", "on": {"event": "input", "filter": {"action": "double", "phase": "pressed"}},
             "do": [
                {"emit": "load-scene", "data": {"scene": "scenes/level1.json"}},
                {"emit": "load-scene", "data": {"scene": "scenes/level2.json"}}
             ]},
            {"id": "no-scene-field", "on": {"event": "input", "filter": {"action": "nodata", "phase": "pressed"}},
             "do": [{"emit": "load-scene", "data": {}}]}
        ]})
        .to_string(),
    )
    .unwrap();
    dir
}

#[test]
fn load_scene_swaps_world_and_persist_survives() {
    let dir = write_project("switch");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    use vitric_sim::GameLogic;

    sim.step(&mut rt).unwrap(); // tick 0: start event → gold 100
    rt.drain_observed();
    let title = sim.world.entity("title").unwrap();
    let hero_old = sim.world.entity("hero").unwrap();
    for _ in 0..2 {
        sim.inject_input("earn", "pressed");
        sim.step(&mut rt).unwrap();
    }
    rt.drain_observed();

    // Switch: within this step the rule emits load-scene → the pipeline tail swaps the world
    sim.inject_input("start", "pressed");
    sim.step(&mut rt).unwrap();
    let observed = rt.drain_observed();
    assert!(observed.iter().any(|e| e.name == "load-scene"), "{observed:?}");
    assert!(
        observed.iter().any(|e| e.name == "scene-loaded"
            && e.data.get("scene") == Some(&json!("scenes/level1.json"))),
        "切换即发 scene-loaded（下一 tick 送达规则）: {observed:?}"
    );

    // The old world disappears entirely: entities gone, handles dead, names released
    assert!(sim.world.entity("title").is_err(), "菜单实体应已销毁");
    assert!(!sim.world.is_alive(title), "旧句柄必须失效");
    assert!(!sim.world.is_alive(hero_old), "幸存者也是重建的，旧句柄同样失效");
    // New world = level1's entities + the Persist survivor (state intact)
    let room = sim.world.entity("room1").unwrap();
    assert_eq!(sim.world.get_field(room, "Tag.label").unwrap(), &json!("level1"));
    let hero = sim.world.entity("hero").unwrap();
    assert_eq!(
        sim.world.get_field(hero, "Player.gold").unwrap(),
        &json!(102),
        "gold = 100(start) + 2(earn)，切换不动幸存者状态"
    );

    // Next tick: scene-loaded reaches the rule (+10); start is not re-emitted (no second +100)
    sim.step(&mut rt).unwrap();
    assert_eq!(
        sim.world.get_field(hero, "Player.gold").unwrap(),
        &json!(112),
        "scene-loaded 是每场景初始化钩子（+10）；start 只在 tick 0 发一次"
    );

    // Switch one more level: flow = menu → level1 → level2
    sim.inject_input("next", "pressed");
    sim.step(&mut rt).unwrap();
    assert!(sim.world.entity("room1").is_err());
    sim.world.entity("room2").unwrap();
    let hero = sim.world.entity("hero").unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(sim.world.get_field(hero, "Player.gold").unwrap(), &json!(122));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unknown_scene_is_explicit_and_lists_manifest_scenes() {
    let dir = write_project("unknown");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.inject_input("warp", "pressed");
    let err = sim.step(&mut rt).unwrap_err().to_string();
    assert!(err.contains("scenes/ghost.json"), "点名坏引用: {err}");
    assert!(
        err.contains("scenes/level1.json") && err.contains("scenes/menu.json"),
        "列出清单里的可用场景: {err}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn persist_name_collision_is_explicit() {
    let dir = write_project("clash");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.inject_input("clash", "pressed");
    let err = sim.step(&mut rt).unwrap_err().to_string();
    assert!(err.contains("hero"), "点名冲突实体: {err}");
    assert!(err.contains("scenes/clash.json"), "点名目标场景: {err}");
    assert!(err.contains("重名"), "说清原因: {err}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn anonymous_persist_entity_is_explicit_error_on_next_switch() {
    let dir = write_project("anon");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    // Switching into a scene with an anonymous Persist entity is fine...
    sim.inject_input("anon", "pressed");
    sim.step(&mut rt).unwrap();
    sim.world.entity("room-anon").unwrap();
    // ...but switching away from it, the anonymous survivor cannot be referenced → explicit error
    sim.inject_input("start", "pressed");
    let err = sim.step(&mut rt).unwrap_err().to_string();
    assert!(err.contains("Persist") && err.contains("没有名字"), "{err}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn multiple_or_malformed_load_scene_in_one_tick_is_explicit() {
    let dir = write_project("double");
    // Two load-scenes in the same tick: there is no answer for where to go, explicit error
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.inject_input("double", "pressed");
    let err = sim.step(&mut rt).unwrap_err().to_string();
    assert!(err.contains("同一 tick") && err.contains("load-scene"), "{err}");

    // data missing the scene field: report the correct form
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.inject_input("nodata", "pressed");
    let err = sim.step(&mut rt).unwrap_err().to_string();
    assert!(err.contains("缺少 scene 字段"), "{err}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn recording_with_scene_switch_replays_bit_identically() {
    let dir = write_project("replay");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.start_recording();
    // Within 90 ticks two input-triggered switches (menu→level1→level2), straddling the tick-60 checkpoint
    for t in 0..90 {
        if t == 10 {
            sim.inject_input("earn", "pressed");
        }
        if t == 20 {
            sim.inject_input("start", "pressed");
        }
        if t == 70 {
            sim.inject_input("next", "pressed");
        }
        sim.step(&mut rt).unwrap();
    }
    let rec = sim.stop_recording().unwrap();
    sim.world.entity("room2").unwrap();
    assert!(rec.checkpoints.iter().any(|&(t, _)| t == 60), "应有覆盖切换的校验点");

    // Cold-boot replay: checkpoint-by-checkpoint + final-state hash match (the switch is entirely driven by recorded input)
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.replay(&rec, &mut rt2).expect("跨场景切换的录像必须逐位重放");
    assert_eq!(sim2.world.state_hash(), rec.final_hash);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn snapshot_after_switch_restores_identical_trajectory() {
    let dir = write_project("snap");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    for _ in 0..5 {
        sim.step(&mut rt).unwrap();
    }
    // The "dirty moment" right after a switch: carryover still holds scene-loaded not yet delivered to the rule
    sim.inject_input("start", "pressed");
    sim.step(&mut rt).unwrap();
    let snap = sim.snapshot(&rt);

    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }
    let h_direct = sim.world.state_hash();

    // Fresh-process semantics: re-boot + restore, then run 30 more ticks must be bit-identical
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.restore(&snap, &mut rt2).unwrap();
    for _ in 0..30 {
        sim2.step(&mut rt2).unwrap();
    }
    assert_eq!(sim2.world.state_hash(), h_direct, "跨切换快照恢复后轨迹分歧");
    // The scene-loaded hook is honored once on each trajectory (gold fingerprint matches)
    let hero = sim2.world.entity("hero").unwrap();
    assert_eq!(sim2.world.get_field(hero, "Player.gold").unwrap(), &json!(110));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_validates_every_scene_in_manifest() {
    let dir = write_project("check");
    // Good project: check passes, the report lists all scenes
    let report = runtime::check(&dir).expect("全部场景合法，check 该过");
    assert_eq!(report["scenes"].as_array().unwrap().len(), 5);

    // Stuff a missing-image Sprite reference into a non-entry scene: check must go red and name the scene + image
    fs::write(
        dir.join("scenes/level2.json"),
        json!({"entities": [
            {"name": "room2", "components": {"Sprite": {"image": "ghost.png", "w": 1, "h": 1}}}
        ]})
        .to_string(),
    )
    .unwrap();
    let err = runtime::check(&dir).expect_err("非入口场景的坏引用也要在 check 期抓住");
    assert!(err.contains("ghost.png"), "点名缺失贴图: {err}");
    assert!(err.contains("scenes/level2.json"), "点名所在场景: {err}");
    fs::remove_dir_all(&dir).unwrap();
}
