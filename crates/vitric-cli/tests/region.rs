//! Region dormant/active/frozen state — Task 1 (E1) tests.
//!
//! Validates: (a) world.query filters dormant entities; (b) describe_world skips dormant
//! entities; (c) Sim::thaw_region transitions state + emits region-thaw event; (d) dormant
//! entities skip logic dispatch.
//!
//! Test setup follows the existing `saves.rs` / `glow.rs` pattern: `Runtime::boot(dir)` for
//! full scene + logic, or direct `World::new()` for isolated world-level checks (no schema
//! validation needed — `World::set_component` accepts any JSON).

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_render::describe_world;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

#[test]
fn dormant_entities_excluded_from_query() {
    // Isolated world: no scene, no schema validation — we just need to verify that query()
    // filters entities whose Region.state is "dormant" or "frozen".
    let mut world = vitric_ecs::World::new();

    let active_e = world.spawn_named("active_ent").unwrap();
    world.set_component(active_e, "Position", json!({"x":0,"y":0})).unwrap();
    world.set_component(active_e, "Region", json!({
        "id":"home","biome":"home","state":"active","discovered":1,
        "anchor_x":0,"anchor_y":0,"w":28,"h":12,"dormant_ticks":0
    })).unwrap();

    let dormant_e = world.spawn_named("dormant_ent").unwrap();
    world.set_component(dormant_e, "Position", json!({"x":100,"y":100})).unwrap();
    world.set_component(dormant_e, "Region", json!({
        "id":"mountain","biome":"mountain","state":"dormant","discovered":0,
        "anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0
    })).unwrap();

    let results = world.query(&["Position"]);
    let names: Vec<&str> = results.iter()
        .filter_map(|id| world.name_of(*id))
        .collect();
    assert!(names.contains(&"active_ent"), "active_ent should be in query results: {:?}", names);
    assert!(!names.contains(&"dormant_ent"), "dormant_ent should be excluded from query: {:?}", names);
}

#[test]
fn dormant_entities_skipped_in_render() {
    // describe_world should not surface dormant entities in `visible` (on-screen) or
    // `offscreen` lists. The Position is on-screen (0,0 with default camera), so without
    // dormant filtering it would land in `visible`.
    let mut world = vitric_ecs::World::new();
    let dormant_e = world.spawn_named("hidden_sprite").unwrap();
    world.set_component(dormant_e, "Position", json!({"x":0,"y":0})).unwrap();
    world.set_component(dormant_e, "Sprite", json!({"w":1,"h":1,"image":"rock.png"})).unwrap();
    world.set_component(dormant_e, "Region", json!({
        "id":"mountain","biome":"mountain","state":"dormant","discovered":0,
        "anchor_x":0,"anchor_y":12,"w":30,"h":28,
        "dormant_ticks":0,"spawn_timer":7200
    })).unwrap();

    let describe = describe_world(&world, 320, 180).unwrap();
    let visible = describe["visible"].as_array().unwrap();
    let offscreen = describe["offscreen"].as_array().unwrap();
    let all_names: Vec<&str> = visible.iter()
        .chain(offscreen.iter())
        .filter_map(|e| e.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(!all_names.contains(&"hidden_sprite"),
        "dormant entity should not appear in describe (visible or offscreen): {:?}",
        all_names);
}

#[test]
fn dormant_entities_skip_logic_systems() {
    // Spawn a Crop+Sprite entity inside a dormant region. The crop-grow system queries
    // ["Crop","Sprite"] — without dormant filtering at the query level it would advance the
    // timer; with dormant filtering the entity is skipped and timer stays at 0.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let e = sim.world.spawn_named("frozen_crop").unwrap();
    sim.world.set_component(e, "Position", json!({"x":40,"y":40})).unwrap();
    sim.world.set_component(e, "Sprite", json!({"w":1,"h":1,"color":"#7fbf5a"})).unwrap();
    sim.world.set_component(e, "Crop", json!({"kind":"wheat","stage":0,"timer":0,"_tend_t":0})).unwrap();
    sim.world.set_component(e, "Region", json!({
        "id":"mountain","biome":"mountain","state":"dormant","discovered":0,
        "anchor_x":0,"anchor_y":12,"w":30,"h":28,
        "dormant_ticks":0,"spawn_timer":7200
    })).unwrap();

    // 60 ticks = 1 second of sim time. CROP_DAY_SEC=60 means this is daytime (crop would grow
    // if it were processed). Timer must remain 0 because the dormant filter excludes the
    // entity from the crop-grow system's query.
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    let timer = sim.world.get_field(e, "Crop.timer").unwrap().as_f64().unwrap();
    assert_eq!(timer, 0.0,
        "dormant entity's Crop.timer should not advance — crop-grow system must skip it");
}

#[test]
fn thaw_region_activates_entities() {
    // mountain is a dormant region marker in scenes/main.json (added by Task 1 Step 12).
    // thaw_region must flip state → "active", set discovered=1, and queue a `region-thaw`
    // event that reaches the logic inbox on the next step (where rules can react to it).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.thaw_region("mountain");
    let mountain_e = sim.world.entity("mountain").unwrap();
    let region = sim.world.get_component(mountain_e, "Region").unwrap();
    assert_eq!(region.get("state").unwrap().as_str(), Some("active"));
    assert_eq!(region.get("discovered").unwrap().as_i64(), Some(1));

    // The event sits in pending_events until the next step flushes it into the logic inbox.
    // StepReport.events is the list of events fed to the logic this tick — it must contain
    // region-thaw (alongside the always-present start/tally/census-tick/time-tick events).
    let report = sim.step(&mut rt).unwrap();
    assert!(report.events.iter().any(|e| e.name == "region-thaw"),
        "region-thaw event must be fed to the logic on the tick after thaw_region: {:?}",
        report.events.iter().map(|e| e.name.as_str()).collect::<Vec<_>>());

    // dormant_ticks must NOT have advanced for the (now-active) mountain region — once
    // thawed, it's no longer dormant so accumulate_dormant_ticks skips it.
    let dormant_ticks = sim.world.get_field(mountain_e, "Region.dormant_ticks")
        .unwrap().as_i64().unwrap();
    assert_eq!(dormant_ticks, 0, "active region should not accumulate dormant_ticks");
}

#[test]
fn catch_up_advances_dormant_crop_on_thaw() {
    // A Crop entity inside a dormant region accumulates `dormant_ticks` while frozen out
    // of the regular crop-grow system (the dormant filter excludes it from the query).
    // On thaw, the engine queues a catch-up; the next step flushes it, invoking each
    // system's optional catch_up function for entities in the thawed region. crop-grow
    // declares a catch_up that fast-forwards `Crop.timer`/`Crop.stage` by the dormant
    // tick budget — so the timer must jump from 0 to ~elapsed_seconds after the flush.
    //
    // The crop-grow system queries ["Crop","Sprite"] (see games/frontier/scripts/crops.js),
    // so the test entity must carry both components to be matched by the catch_up filter.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let crop_e = sim.world.spawn_named("mountain_crop").unwrap();
    sim.world.set_component(crop_e, "Position", json!({"x":15,"y":15})).unwrap();
    sim.world.set_component(crop_e, "Sprite", json!({"w":1,"h":1,"color":"#7fbf5a"})).unwrap();
    sim.world.set_component(crop_e, "Crop", json!({"kind":"wheat","stage":0,"timer":0,"_tend_t":0})).unwrap();
    sim.world.set_component(crop_e, "Region", json!({
        "id":"mountain","biome":"mountain","state":"dormant","discovered":0,
        "anchor_x":0,"anchor_y":12,"w":30,"h":28,
        "dormant_ticks":0,"spawn_timer":7200
    })).unwrap();

    // 3600 ticks = 60 seconds of sim time. The mountain region accumulates dormant_ticks
    // each tick (Task 1's accumulate_dormant_ticks). The crop-grow system skips the dormant
    // entity, so Crop.timer stays at 0.
    for _ in 0..3600 {
        sim.step(&mut rt).unwrap();
    }
    let timer_before = sim.world.get_field(crop_e, "Crop.timer")
        .unwrap().as_f64().unwrap();
    assert_eq!(timer_before, 0.0,
        "dormant crop's timer must not advance before thaw");

    // Mountain region's dormant_ticks is now 3600. thaw_region queues a catch-up; the
    // catch_up itself does NOT run inline — it runs on the NEXT step (when pending_catch_ups
    // is flushed before logic.on_tick). So we must step at least once before checking.
    sim.thaw_region("mountain");
    sim.step(&mut rt).unwrap(); // Flushes pending_catch_ups → catch_up runs

    let timer_after = sim.world.get_field(crop_e, "Crop.timer")
        .unwrap().as_f64().unwrap();
    assert!(timer_after > 0.0,
        "catch_up should advance timer by ~60s of dormant budget, got {}", timer_after);

    // dormant_ticks on the mountain region must be reset to 0 after catch-up (the budget
    // was consumed). The mountain_crop marker's own Region.dormant_ticks is NOT reset
    // (it's a separate Region component on a separate entity — only the region entity
    // named in thaw_region is reset).
    let mountain_e = sim.world.entity("mountain").unwrap();
    let dormant_ticks = sim.world.get_field(mountain_e, "Region.dormant_ticks")
        .unwrap().as_i64().unwrap();
    assert_eq!(dormant_ticks, 0,
        "dormant_ticks must be reset to 0 after catch-up consumes the budget");
}
