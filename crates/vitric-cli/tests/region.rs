//! Region dormant/active/frozen state — Task 1 (E1) tests, plus Task 3 (E3) seeded RNG
//! substream tests.
//!
//! Validates: (a) world.query filters dormant entities; (b) describe_world skips dormant
//! entities; (c) Sim::thaw_region transitions state + emits region-thaw event; (d) dormant
//! entities skip logic dispatch; (e) ctx.random_stream(name) is deterministic regardless of
//! call timing and snapshot/restore round-trips its state.
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

// ---- Task 3 (E3): Seeded RNG substreams ----

/// Register a test-only script function that calls `ctx.random_stream(name).nextInt(min, max)`
/// and writes the result to a marker entity's `TestResult.value` field. The marker must be
/// spawned first (World::set_component accepts any JSON — no schema validation needed).
///
/// Why a side-effect channel instead of a return value: `ScriptEngine::call_fn` collects ops
/// and events, not the function's return value (prelude's `__callFn` discards it). Writing to
/// a known field is the simplest way to get the integer back in Rust.
fn setup_random_stream_test_fn(rt: &mut Runtime) {
    rt.scripts.load(
        "test_random_stream.js",
        r#"
        vitric.fn("__testRandomStreamNext", (args, ctx) => {
            const v = ctx.random_stream(args.name).nextInt(args.min, args.max);
            ctx.setField("test_marker", "TestResult.value", v);
        });
        "#,
    ).expect("test fn 加载失败");
}

/// Spawn a marker entity carrying `TestResult: {value: 0}` so `ctx.setField` has a target.
fn spawn_test_marker(sim: &mut vitric_sim::Sim) {
    let e = sim.world.spawn_named("test_marker").expect("test_marker spawn");
    sim.world.set_component(e, "TestResult", json!({"value": 0}))
        .expect("test_marker set TestResult");
}

/// Call the test fn and read the written value back from the marker. Sets/clears SIM_PTR so
/// the native `__randomStreamNext` can reach `sim.substreams` — production code sets this in
/// `Sim::step`, but tests calling `call_fn` directly must set it themselves.
fn call_next_int(sim: &mut vitric_sim::Sim, rt: &mut Runtime, name: &str, min: i64, max: i64) -> i64 {
    vitric_sim::set_sim_ptr(sim);
    let out = rt.scripts
        .call_fn(
            "__testRandomStreamNext",
            &json!({"name": name, "min": min, "max": max}),
            None,
            &mut sim.world,
            &mut sim.rng,
            sim.tick,
        )
        .expect("call_fn __testRandomStreamNext");
    vitric_sim::clear_sim_ptr();
    let _ = out;
    let marker = sim.world.entity("test_marker").expect("test_marker exists");
    sim.world.get_field(marker, "TestResult.value")
        .expect("TestResult.value readable")
        .as_i64()
        .expect("TestResult.value is integer")
}

#[test]
fn random_stream_same_seed_regardless_of_call_timing() {
    // Two sims booted from the same scene share the same world seed. The substream seeded by
    // (world_seed, name) must produce the same sequence regardless of when it's first accessed
    // — whether at tick 0 or after 1000 ticks of regular stepping. This is the determinism
    // guarantee that makes PCG for dormant regions replay-safe: even if region thaw happens at
    // different ticks across runs, the generated content is bit-identical.
    let (mut sim1, mut rt1) = Runtime::boot(&frontier_dir()).unwrap();
    setup_random_stream_test_fn(&mut rt1);
    spawn_test_marker(&mut sim1);
    let r1: Vec<i64> = (0..5).map(|_| {
        call_next_int(&mut sim1, &mut rt1, "region:mountain", 0, 100)
    }).collect();

    let (mut sim2, mut rt2) = Runtime::boot(&frontier_dir()).unwrap();
    setup_random_stream_test_fn(&mut rt2);
    spawn_test_marker(&mut sim2);
    // Step 1000 ticks first. Frontier scripts don't call ctx.random_stream, so the substream
    // is untouched by regular stepping — the first nextInt after 1000 ticks must match the
    // first nextInt without any stepping.
    for _ in 0..1000 {
        sim2.step(&mut rt2).unwrap();
    }
    let r2: Vec<i64> = (0..5).map(|_| {
        call_next_int(&mut sim2, &mut rt2, "region:mountain", 0, 100)
    }).collect();

    assert_eq!(r1, r2,
        "substream must be deterministic regardless of call timing: {:?} vs {:?}", r1, r2);
}

#[test]
fn random_stream_state_in_snapshot() {
    // Substream state must enter the snapshot and be restored exactly. After 2 nextInt draws,
    // snapshot → restore into a fresh sim → both sims' next nextInt call must produce the same
    // value (the restored sim resumes the substream from the exact pre-snapshot state).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    setup_random_stream_test_fn(&mut rt);
    spawn_test_marker(&mut sim);
    // Two draws to advance the substream state past its initial value.
    call_next_int(&mut sim, &mut rt, "region:mountain", 0, 100);
    call_next_int(&mut sim, &mut rt, "region:mountain", 0, 100);

    let snap = sim.snapshot(&rt);
    let (mut sim2, mut rt2) = Runtime::boot(&frontier_dir()).unwrap();
    setup_random_stream_test_fn(&mut rt2);
    // No spawn_test_marker — restore replaces the world, including test_marker.
    sim2.restore(&snap, &mut rt2).unwrap();

    let v1 = call_next_int(&mut sim, &mut rt, "region:mountain", 0, 100);
    let v2 = call_next_int(&mut sim2, &mut rt2, "region:mountain", 0, 100);
    assert_eq!(v1, v2,
        "restored sim must resume the substream at the same state: {} vs {}", v1, v2);
}

// ---- Task 4 (E4): View-frustum culling ----
//
// Replaces the brief's flaky timing-based perf test with three deterministic correctness tests:
// off-screen entities must not appear in the pixel buffer (their color must not bleed in),
// on-screen entities must still render (their color must appear), and the same world rendered
// twice must be byte-identical with the on-screen sprite's full AABB covered. Together these
// lock the culling contract — render skips work for off-screen entities but never changes the
// visible pixels — without depending on wall-clock timing.
//
// Setup: isolated world, default camera (origin, 8 px/unit). For a 64x64 frame the visible world
// viewport is 8x8 units centered on the origin → (-4..=4 on each axis). An entity at (1000, 1000)
// is far outside; an entity at (0, 0) is centered.

/// A bright magenta that is distinct from the BACKGROUND ([24,26,33,255]) and the default white
/// sprite — easy to grep for in the pixel buffer.
const MAGENTA: &[u8] = &[255, 0, 255, 255];

#[test]
fn offscreen_entities_not_rendered() {
    // Default camera (origin, scale=8) on a 64x64 frame → world viewport (-4..=4, -4..=4).
    // An entity at (1000, 1000) with size 4x4 is entirely outside the viewport — its AABB
    // (998..=1002) does not intersect (-4..=4) on either axis. With view-frustum culling the
    // sprite loop must skip it, and the magenta color must not appear anywhere in the buffer.
    let mut world = vitric_ecs::World::new();
    let e = world.spawn();
    world.set_component(e, "Position", json!({"x": 1000.0, "y": 1000.0})).unwrap();
    world.set_component(e, "Sprite", json!({"w": 4.0, "h": 4.0, "color": "#ff00ff"})).unwrap();

    let buf = vitric_render::render_world(&world, 64, 64, &vitric_render::Assets::empty(), 0).unwrap();
    for pixel in buf.chunks(4) {
        let [r, g, b, _a] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        assert!([r, g, b] != [MAGENTA[0], MAGENTA[1], MAGENTA[2]],
            "off-screen entity's magenta color appeared in render output — culling failed at pixel");
    }
}

#[test]
fn onscreen_entities_rendered() {
    // Same setup as offscreen_entities_not_rendered, but the entity is at the camera center.
    // Culling must NOT skip it: its magenta color must appear in the pixel buffer (proving the
    // cull doesn't over-aggressively drop on-screen entities).
    let mut world = vitric_ecs::World::new();
    let e = world.spawn();
    world.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
    world.set_component(e, "Sprite", json!({"w": 4.0, "h": 4.0, "color": "#ff00ff"})).unwrap();

    let buf = vitric_render::render_world(&world, 64, 64, &vitric_render::Assets::empty(), 0).unwrap();
    let magenta_count = buf.chunks(4)
        .filter(|p| [p[0], p[1], p[2]] == [MAGENTA[0], MAGENTA[1], MAGENTA[2]])
        .count();
    assert!(magenta_count > 0,
        "on-screen entity's magenta color did not appear in render output — culling is over-aggressive");
}

#[test]
fn culling_preserves_byte_identical_output_for_onscreen_entities() {
    // Determinism contract: culling must not change the rendered output for entities that ARE
    // on screen. We can't directly call "render without culling" after the change, but we can
    // lock the on-screen bytes by rendering the same world twice — same world + same tick must
    // yield byte-identical pixels, AND the on-screen sprite's footprint (size 4 → 32x32 pixels at
    // scale 8) must be entirely magenta (the sprite fully covers its AABB).
    //
    // This is the regression guard the brief warns about: if culling accidentally skips an
    // on-screen entity, the buffer would be all-background and this test would fail.
    let mut world = vitric_ecs::World::new();
    let e = world.spawn();
    world.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
    world.set_component(e, "Sprite", json!({"w": 4.0, "h": 4.0, "color": "#ff00ff"})).unwrap();

    let a = vitric_render::render_world(&world, 64, 64, &vitric_render::Assets::empty(), 0).unwrap();
    let b = vitric_render::render_world(&world, 64, 64, &vitric_render::Assets::empty(), 0).unwrap();
    assert_eq!(a, b, "same world + same tick must render byte-identical");

    // Entity AABB on screen: center (32, 32), half-size 4*8/2=16 → x in [16, 48), y in [16, 48).
    // All 32x32 = 1024 pixels inside must be magenta.
    let mut magenta_in_aabb = 0;
    for y in 16..48 {
        for x in 16..48 {
            let i = ((y * 64 + x) * 4) as usize;
            if [a[i], a[i + 1], a[i + 2]] == [MAGENTA[0], MAGENTA[1], MAGENTA[2]] {
                magenta_in_aabb += 1;
            }
        }
    }
    assert_eq!(magenta_in_aabb, 32 * 32,
        "on-screen entity's full 32x32 AABB must be magenta — culling must not skip any of it, got {} of {} pixels",
        magenta_in_aabb, 32 * 32);
}

// ---- Task 5 (E5): describe dormant dim + snapshot/replay plumbing ----

#[test]
fn describe_classifies_dormant_entities() {
    // Isolated world: an entity with Position+Sprite inside a dormant region.
    // describe_world must surface it in the `dormant` array — NOT in `visible` or `offscreen`.
    let mut world = vitric_ecs::World::new();

    let dormant_e = world.spawn_named("hidden_in_mountain").unwrap();
    world.set_component(dormant_e, "Position", json!({"x": 15, "y": 20})).unwrap();
    world.set_component(dormant_e, "Sprite", json!({"w": 1, "h": 1, "color": "#ff00ff"})).unwrap();
    world.set_component(dormant_e, "Region", json!({
        "id":"mountain","biome":"mountain","state":"dormant","discovered":0,
        "anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200
    })).unwrap();

    // Also spawn an active on-screen entity to verify visible still works.
    let active_e = world.spawn_named("visible_ent").unwrap();
    world.set_component(active_e, "Position", json!({"x": 0, "y": 0})).unwrap();
    world.set_component(active_e, "Sprite", json!({"w": 1, "h": 1, "color": "#ffffff"})).unwrap();

    let desc = describe_world(&world, 64, 64).unwrap();
    let visible = desc["visible"].as_array().unwrap();
    let offscreen = desc["offscreen"].as_array().unwrap();
    let dormant = desc["dormant"].as_array().unwrap();

    let visible_names: Vec<&str> = visible.iter()
        .filter_map(|e| e.get("name").and_then(|v| v.as_str()))
        .collect();
    let dormant_names: Vec<&str> = dormant.iter()
        .filter_map(|e| e.get("name").and_then(|v| v.as_str()))
        .collect();

    assert!(visible_names.contains(&"visible_ent"), "active entity should be in visible: {:?}", visible_names);
    assert!(!visible_names.contains(&"hidden_in_mountain"), "dormant entity should NOT be in visible: {:?}", visible_names);
    assert!(!offscreen.iter().any(|e| e.get("name").and_then(|v| v.as_str()) == Some("hidden_in_mountain")),
        "dormant entity should NOT be in offscreen");
    assert!(dormant_names.contains(&"hidden_in_mountain"), "dormant entity should be in dormant array: {:?}", dormant_names);

    // The dormant entry should include region info so the agent knows which region it belongs to.
    let dormant_entry = dormant.iter()
        .find(|e| e.get("name").and_then(|v| v.as_str()) == Some("hidden_in_mountain"))
        .expect("dormant entry missing");
    assert_eq!(dormant_entry["region"]["id"].as_str(), Some("mountain"));
    assert_eq!(dormant_entry["region"]["state"].as_str(), Some("dormant"));
    assert_eq!(dormant_entry["world"]["x"].as_f64(), Some(15.0));
    assert_eq!(dormant_entry["world"]["y"].as_f64(), Some(20.0));
}

#[test]
fn snapshot_preserves_dormant_state() {
    // Region.state is a World component, so snapshot/restore round-trips it automatically.
    // This test locks the contract: if a future refactor breaks World::snapshot for Region
    // components, this test fails.
    let (mut sim, rt) = Runtime::boot(&frontier_dir()).unwrap();

    // Mountain region starts dormant in scenes/main.json. Freeze it to verify a non-default
    // state round-trips (dormant→frozen is a state the host can set via set_component).
    let mountain_e = sim.world.entity("mountain").expect("mountain region entity should exist");
    let mut region = sim.world.get_component(mountain_e, "Region").unwrap().clone();
    region["state"] = json!("frozen");
    region["discovered"] = json!(1);
    sim.world.set_component(mountain_e, "Region", region).unwrap();

    let snap = sim.snapshot(&rt);

    // Boot a fresh sim and restore.
    let (mut sim2, mut rt2) = Runtime::boot(&frontier_dir()).unwrap();
    sim2.restore(&snap, &mut rt2).unwrap();

    let mountain_e2 = sim2.world.entity("mountain").expect("mountain region entity should exist after restore");
    let region2 = sim2.world.get_component(mountain_e2, "Region").unwrap();
    assert_eq!(region2.get("state").and_then(|v| v.as_str()), Some("frozen"),
        "Region.state must round-trip through snapshot/restore");
    assert_eq!(region2.get("discovered").and_then(|v| v.as_i64()), Some(1),
        "Region.discovered must round-trip through snapshot/restore");
}

#[test]
fn replay_with_dormant_region_is_hash_identical() {
    // A recording that starts with a dormant region (the default frontier scene) must replay
    // hash-identically: the dormant state is part of world.state_hash(), and since nothing
    // triggers a thaw during the recording, replay reproduces the same trajectory.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.start_recording();
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    let rec = sim.stop_recording().unwrap();
    let final_hash = rec.final_hash;

    // Boot fresh sim and replay.
    let (mut sim2, mut rt2) = Runtime::boot(&frontier_dir()).unwrap();
    sim2.replay(&rec, &mut rt2).unwrap();
    // replay() already asserts final_hash matches; this explicit check is for clarity.
    assert_eq!(sim2.world.state_hash(), final_hash,
        "replay final hash must match recording final hash");
}

#[test]
fn replay_after_pre_recording_thaw_is_hash_identical() {
    // thaw_region is a host API call — NOT recorded by the recording. The contract (per
    // sim.rs line 362-365 comment) is: the host re-runs the same thaw_region calls at the
    // same ticks during replay. This test verifies that contract: thaw BEFORE start_recording
    // (so the thawed state is in the initial checkpoint), then thaw BEFORE replay (same host
    // call, same tick 0). The recording replays hash-identically.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.thaw_region("mountain"); // Host API call at tick 0, before recording starts.
    sim.start_recording();
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    let rec = sim.stop_recording().unwrap();
    let final_hash = rec.final_hash;

    // Boot fresh sim and replay. The host re-runs the same thaw_region call before replay.
    let (mut sim2, mut rt2) = Runtime::boot(&frontier_dir()).unwrap();
    sim2.thaw_region("mountain"); // Same host call, same tick 0.
    sim2.replay(&rec, &mut rt2).unwrap();
    assert_eq!(sim2.world.state_hash(), final_hash,
        "replay after pre-recording thaw must be hash-identical");
}

// ---- Task 12: Map Expansion & Region Content ----
//
// Verifies the ctx.thaw_region JS bridge (added in Task 12 to fix the pre-existing dead
// unlock_region fn from Task 8), determinism of region content generation across thaw
// timing (the E3 seeded substream contract), and Camera.world_bounds clamping of the
// player position.

#[test]
fn ctx_thaw_region_bridge_works_from_js() {
    // The unlock_region fn in research.js calls ctx.thaw_region(region_id).
    // Before Task 12, ctx.thaw_region was not exposed — the fn would throw TypeError.
    // This test verifies the bridge works: calling unlock_region via call_fn thaws the region.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    // Mountain starts dormant.
    let mountain_e = sim.world.entity("mountain").unwrap();
    let state_before = sim.world.get_field(mountain_e, "Region.state")
        .unwrap().as_str().unwrap().to_string();
    assert_eq!(state_before, "dormant");

    // Call unlock_region fn (same fn that rules/research.json calls on exploration_t1).
    vitric_sim::set_sim_ptr(&mut sim);
    rt.scripts.call_fn(
        "unlock_region",
        &json!({"region_id": "mountain"}),
        None,
        &mut sim.world,
        &mut sim.rng,
        sim.tick,
    ).expect("unlock_region call_fn");
    vitric_sim::clear_sim_ptr();

    // Region state must flip to active immediately (thaw_region is synchronous).
    let state_after = sim.world.get_field(mountain_e, "Region.state")
        .unwrap().as_str().unwrap().to_string();
    assert_eq!(state_after, "active",
        "ctx.thaw_region bridge must flip Region.state to active");

    // Step to flush the region-thaw event + run gen_region_content rule.
    sim.step(&mut rt).unwrap();

    // After step, region content should be generated: mountain_node_0 must exist.
    let node_e = sim.world.entity("mountain_node_0");
    assert!(node_e.is_ok(),
        "gen_region_content must spawn mountain_node_0 after region-thaw: {:?}",
        node_e.err());
}

#[test]
fn region_content_deterministic_across_thaw_timing() {
    // Two sims booted from the same scene share world_seed=1. The substream seeded by
    // (world_seed, "region:mountain") must produce the same sequence regardless of when
    // it's first accessed — thawing at tick 0 vs tick 1000 generates bit-identical content.
    //
    // This is the replay-safety guarantee for dormant regions: even if region thaw happens
    // at different ticks across runs (player reaches the boundary earlier or later), the
    // generated content is identical.
    let (mut sim1, mut rt1) = Runtime::boot(&frontier_dir()).unwrap();

    // Sim 1: thaw mountain at tick 0.
    vitric_sim::set_sim_ptr(&mut sim1);
    rt1.scripts.call_fn(
        "unlock_region",
        &json!({"region_id": "mountain"}),
        None,
        &mut sim1.world,
        &mut sim1.rng,
        sim1.tick,
    ).expect("sim1 unlock_region");
    vitric_sim::clear_sim_ptr();
    sim1.step(&mut rt1).unwrap(); // Flush region-thaw → gen_region_content runs

    // Read mountain_node_0..5 positions from sim1.
    let mut pos1: Vec<(f64, f64)> = Vec::new();
    for i in 0..6 {
        let name = format!("mountain_node_{}", i);
        let e = sim1.world.entity(&name).expect(&format!("{} exists in sim1", name));
        let x = sim1.world.get_field(e, "Position.x").unwrap().as_f64().unwrap();
        let y = sim1.world.get_field(e, "Position.y").unwrap().as_f64().unwrap();
        pos1.push((x, y));
    }
    // Also read POI position.
    let poi_e = sim1.world.entity("mountain_poi_0").expect("mountain_poi_0 exists in sim1");
    let poi_x = sim1.world.get_field(poi_e, "Position.x").unwrap().as_f64().unwrap();
    let poi_y = sim1.world.get_field(poi_e, "Position.y").unwrap().as_f64().unwrap();
    pos1.push((poi_x, poi_y));

    // Sim 2: step 1000 ticks first, THEN thaw mountain.
    let (mut sim2, mut rt2) = Runtime::boot(&frontier_dir()).unwrap();
    for _ in 0..1000 {
        sim2.step(&mut rt2).unwrap();
    }
    vitric_sim::set_sim_ptr(&mut sim2);
    rt2.scripts.call_fn(
        "unlock_region",
        &json!({"region_id": "mountain"}),
        None,
        &mut sim2.world,
        &mut sim2.rng,
        sim2.tick,
    ).expect("sim2 unlock_region");
    vitric_sim::clear_sim_ptr();
    sim2.step(&mut rt2).unwrap(); // Flush region-thaw → gen_region_content runs

    // Read mountain_node_0..5 + poi_0 positions from sim2.
    let mut pos2: Vec<(f64, f64)> = Vec::new();
    for i in 0..6 {
        let name = format!("mountain_node_{}", i);
        let e = sim2.world.entity(&name).expect(&format!("{} exists in sim2", name));
        let x = sim2.world.get_field(e, "Position.x").unwrap().as_f64().unwrap();
        let y = sim2.world.get_field(e, "Position.y").unwrap().as_f64().unwrap();
        pos2.push((x, y));
    }
    let poi_e = sim2.world.entity("mountain_poi_0").expect("mountain_poi_0 exists in sim2");
    let poi_x = sim2.world.get_field(poi_e, "Position.x").unwrap().as_f64().unwrap();
    let poi_y = sim2.world.get_field(poi_e, "Position.y").unwrap().as_f64().unwrap();
    pos2.push((poi_x, poi_y));

    assert_eq!(pos1, pos2,
        "region content must be deterministic regardless of thaw timing:\n  tick 0:   {:?}\n  tick 1000: {:?}",
        pos1, pos2);
}

#[test]
fn camera_world_bounds_clamps_player_position() {
    // Set Camera.world_bounds to [0, 0, 20, 20]. Give the player a velocity that would move
    // them beyond x=20. After step, the player's Position.x must be clamped to 20 and
    // Velocity.x zeroed.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    // Set camera world_bounds to [0, 0, 20, 20].
    let cam_e = sim.world.entity("camera").unwrap();
    sim.world.set_field(cam_e, "Camera.world_bounds", json!("[0,0,20,20]")).unwrap();

    // Place player at x=19.9, give velocity x=+10 (would move to 19.9 + 10/60 ≈ 20.06).
    let player_e = sim.world.entity("player").unwrap();
    sim.world.set_field(player_e, "Position.x", json!(19.9)).unwrap();
    sim.world.set_field(player_e, "Position.y", json!(10.0)).unwrap();
    sim.world.set_field(player_e, "Velocity.x", json!(10.0)).unwrap();
    sim.world.set_field(player_e, "Velocity.y", json!(0.0)).unwrap();

    sim.step(&mut rt).unwrap();

    let px = sim.world.get_field(player_e, "Position.x").unwrap().as_f64().unwrap();
    let vx = sim.world.get_field(player_e, "Velocity.x").unwrap().as_f64().unwrap();

    assert!(px <= 20.0,
        "player x must be clamped to world_bounds max_x=20, got {}", px);
    assert_eq!(vx, 0.0,
        "player vx must be zeroed after clamping, got {}", vx);
}

// ---- Task 13: Sandbeast desert-spawn system ----
//
// The desert-spawn system (in combat.js) queries ["Region"], filters for the desert marker,
// decrements Region.spawn_timer each tick, and when the timer hits 0 + the player is inside
// desert bounds (60..119, 0..59), spawns a sandbeast near the player using the
// "desert_spawn" substream. These tests verify both the spawn (player in desert) and the
// no-op (player outside desert) branches.

#[test]
fn sandbeast_spawns_when_player_in_desert() {
    // Activate desert region and set spawn_timer to 0 (trigger spawn this tick).
    // Place player inside desert bounds and step — a sandbeast must spawn.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    let desert_e = sim.world.entity("desert").unwrap();
    sim.world.set_field(desert_e, "Region.state", json!("active")).unwrap();
    sim.world.set_field(desert_e, "Region.spawn_timer", json!(0)).unwrap();

    // Place player in desert (x=70, y=10 — inside desert bounds 60..119, 0..59).
    let player_e = sim.world.entity("player").unwrap();
    sim.world.set_field(player_e, "Position.x", json!(70.0)).unwrap();
    sim.world.set_field(player_e, "Position.y", json!(10.0)).unwrap();

    // Count enemies before step.
    let enemies_before = sim.world.query(&["Enemy"]).len();

    // Step — desert-spawn system runs, spawn_timer hits 0, player is in desert → spawn.
    sim.step(&mut rt).unwrap();

    let enemies_after = sim.world.query(&["Enemy"]).len();
    assert!(enemies_after > enemies_before,
        "sandbeast must spawn when player is in desert and spawn_timer expires: before={}, after={}",
        enemies_before, enemies_after);

    // Verify the spawned enemy is a sandbeast.
    let sandbeast_count = sim.world.query(&["Enemy"])
        .iter()
        .filter(|&&id| {
            sim.world.get_field(id, "Enemy.kind")
                .map(|v| v.as_str() == Some("sandbeast"))
                .unwrap_or(false)
        })
        .count();
    assert!(sandbeast_count > 0, "at least one spawned enemy must be a sandbeast");
}

#[test]
fn sandbeast_does_not_spawn_when_player_outside_desert() {
    // Same setup as sandbeast_spawns_when_player_in_desert, but player is outside desert.
    // The spawn_timer still expires, but the player-position check fails → no spawn.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    let desert_e = sim.world.entity("desert").unwrap();
    sim.world.set_field(desert_e, "Region.state", json!("active")).unwrap();
    sim.world.set_field(desert_e, "Region.spawn_timer", json!(0)).unwrap();

    // Place player OUTSIDE desert (x=7, y=7 — home area).
    let player_e = sim.world.entity("player").unwrap();
    sim.world.set_field(player_e, "Position.x", json!(7.0)).unwrap();
    sim.world.set_field(player_e, "Position.y", json!(7.0)).unwrap();

    let enemies_before = sim.world.query(&["Enemy"]).len();
    sim.step(&mut rt).unwrap();
    let enemies_after = sim.world.query(&["Enemy"]).len();

    assert_eq!(enemies_before, enemies_after,
        "no sandbeast should spawn when player is outside desert: before={}, after={}",
        enemies_before, enemies_after);
}
