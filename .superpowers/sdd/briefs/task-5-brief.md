# Task 5: E5 — Snapshot/replay/describe plumbing

**Files:**
- Modify: `crates/vitric-render/src/lib.rs` (`describe_world_with_assets` — add `dormant` array)
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `describe_world` output gains a top-level `dormant` array listing entities whose `Region.state` is `"dormant"` or `"frozen"`. The existing `visible` / `offscreen` arrays are unchanged (they already exclude dormant entities). Snapshot preserves dormant state (already true — `Region` is a World component). Replay of a recording that starts with a dormant region is hash-identical (already true — dormant state is part of `world.state_hash()`).

## Context: real API (NOT the plan's pseudocode)

The plan's pseudocode uses a fictional `TestSim` helper (`sim.spawn_named`, `sim.add_component`, `sim.render_describe`, `sim.snapshot`, `sim.restore`). **These do not exist.** Adapt to the real API:

- **Isolated world tests**: `vitric_ecs::World::new()` + `world.spawn_named("name")` + `world.set_component(id, "Comp", json!({...}))` + `vitric_render::describe_world(&world, width, height)`. See existing tests `dormant_entities_excluded_from_query` and `dormant_entities_skipped_in_render` in `crates/vitric-cli/tests/region.rs` for the pattern.
- **Full sim tests**: `Runtime::boot(&frontier_dir())` returns `(mut sim, mut rt)`. `sim.world` is the `World`, `sim.step(&mut rt)` advances one tick. `sim.thaw_region("mountain")` thaws a region. See `dormant_entities_skip_logic_systems` for the pattern.
- **Snapshot/restore**: `sim.snapshot(&rt)` returns `serde_json::Value`. There is no `TestSim::restore` — instead boot a fresh `(sim2, rt2)` and call `sim2.restore(&snap, &mut rt2)`.
- **Recording/replay**: `sim.start_recording()`, step N ticks, `sim.stop_recording()` returns `Option<Recording>`. Then boot a fresh `(sim2, rt2)` and call `sim2.replay(&rec, &mut rt2)`. The replay verifies checkpoint-by-checkpoint hash equality + final hash equality.

## Architecture: why the plan's `replay_with_region_thaw_is_hash_identical` test is replaced

The plan's Step 5 test shells out to `vitric replay tests/fixtures/region_thaw_recording.json`, implying a recording that includes a `thaw_region` call. **This is architecturally impossible with the current design:**

- `thaw_region` is a **host API call** (line 280 of `crates/vitric-sim/src/sim.rs`). The comment at line 362-365 explicitly states: *"Host API events (thaw_region, etc.). NOT recorded — host API calls are deterministic given the same host program, so replay re-runs the same calls at the same ticks."*
- `Sim::replay` / `replay_observed` (line 452-504) steps through ticks internally and only provides a post-tick observer callback that **may only look, not write**. There is no between-tick hook for the host to call `thaw_region`.
- Therefore: a recording that includes a mid-recording `thaw_region` call will diverge on replay (the thaw changed `Region.state`, but replay doesn't re-run the thaw, so the next checkpoint hash mismatches).

This is a **design decision, not a bug**: `thaw_region` is intentionally not recorded because the host program is responsible for re-calling it at the same ticks. The `Sim::replay` API is for input-driven replays (where all state changes come from recorded inputs/replies). Adding a between-tick host hook to `replay` is out of scope for E5 (it would be a new replay mode).

**Replacement tests (meaningful and correct):**

1. `replay_with_dormant_region_is_hash_identical` — boot sim (mountain region is dormant in `scenes/main.json`), start_recording, step 60 ticks (no thaw), stop_recording, boot fresh sim, replay — passes (dormant state preserved through pipeline).
2. `replay_after_pre_recording_thaw_is_hash_identical` — boot sim, `thaw_region("mountain")` BEFORE `start_recording` (so the thawed state is in the initial checkpoint), start_recording, step 60 ticks, stop_recording, boot fresh sim, thaw_region again before replay, replay — passes (thawed state preserved).

Test #2 calls `thaw_region` before BOTH recording and replay (same host program, same calls, same ticks) — this is the contract the comment describes. The thaw happens at tick 0 (before recording starts), so the initial checkpoint captures the thawed state, and replay starts from the same thawed state.

## Step 1: Write failing test for describe dormant classification

```rust
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
```

## Step 2: Run test to verify it fails

Run: `~/.cargo/bin/cargo test -p vitric-cli --test region describe_classifies_dormant_entities`
Expected: FAIL — `dormant` field does not exist in describe output (or test panics on missing key).

## Step 3: Extend describe_world_with_assets with dormant classification

In `crates/vitric-render/src/lib.rs`, modify `describe_world_with_assets` (starts at line ~2725):

**After** the existing `for id in world.query(&["Position", "Sprite"])` loop (which populates `visible` and `offscreen`, skipping dormant entities), add a second pass that collects dormant entities:

```rust
// Dormant entities: surfaced in a separate `dormant` array so the agent can reason about
// what's in unexplored regions. These entities are NOT in `visible` or `offscreen` (the
// query loop above already skips them via world.query's dormant filter). We iterate all
// entities (world.entities, NOT world.query) because query filters dormant — to list them
// we must bypass the filter.
let mut dormant: Vec<DescribeRow> = Vec::new();
for id in world.entities() {
    if !world.is_dormant(id) { continue; }
    // Only include entities that have Position+Sprite (same minimal components as visible/offscreen).
    if !world.has_component(id, "Position") || !world.has_component(id, "Sprite") { continue; }

    let px = num(world, id, "Position.x")?;
    let py = num(world, id, "Position.y")?;
    let sw = num(world, id, "Sprite.w")?;
    let sh = num(world, id, "Sprite.h")?;
    let color = world
        .get_field(id, "Sprite.color")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "#ffffff".to_string());
    let image = world
        .get_field(id, "Sprite.image")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let name = world.name_of(id).map(String::from);
    let rot = rot_of(world, id)?;

    // Region info: which dormant region this entity belongs to.
    let region_info = world
        .get_component(id, "Region")
        .ok()
        .and_then(|r| {
            Some(serde_json::json!({
                "id": r.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                "state": r.get("state").and_then(|v| v.as_str()).unwrap_or(""),
            }))
        })
        .unwrap_or(serde_json::json!({}));

    let mut entry = serde_json::Map::new();
    entry.insert("id".into(), json!(id.to_string()));
    if let Some(n) = &name {
        entry.insert("name".into(), json!(n));
    }
    entry.insert("world".into(), json!({"x": px, "y": py}));
    let mut sprite = json!({"w": sw, "h": sh, "color": color});
    if !image.is_empty() {
        sprite["image"] = json!(image);
    }
    if rot != 0.0 {
        sprite["rot"] = json!(rot);
    }
    entry.insert("sprite".into(), sprite);
    entry.insert("region".into(), region_info);

    dormant.push(DescribeRow {
        named: name.is_some(),
        dist: 0.0, // No focal-point distance for dormant entities (they're not on-screen).
        id,
        value: serde_json::Value::Object(entry),
    });
}

// Apply the same sort as visible/offscreen when a focal point exists (named first, then id).
if focal_id.is_some() {
    let sort_rows = |rows: &mut Vec<DescribeRow>| {
        rows.sort_by(|a, b| {
            b.named.cmp(&a.named).then(a.id.cmp(&b.id))
        });
    };
    sort_rows(&mut dormant);
}
let dormant: Vec<serde_json::Value> = dormant.into_iter().map(|r| r.value).collect();
```

Then in the final JSON assembly (where `visible` and `offscreen` are inserted into the result object), add:

```rust
result.insert("dormant".into(), json!(dormant));
```

**Important**: Place the `dormant` key AFTER `offscreen` in the JSON object to maintain a logical order (`visible`, `offscreen`, `dormant`). The existing code builds the result object — find where `visible` and `offscreen` are inserted and add `dormant` right after.

## Step 4: Run describe test to verify it passes

Run: `~/.cargo/bin/cargo test -p vitric-cli --test region describe_classifies_dormant_entities`
Expected: PASS.

## Step 5: Write snapshot round-trip + replay consistency tests

```rust
#[test]
fn snapshot_preserves_dormant_state() {
    // Region.state is a World component, so snapshot/restore round-trips it automatically.
    // This test locks the contract: if a future refactor breaks World::snapshot for Region
    // components, this test fails.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

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
```

## Step 6: Run all E5 tests

Run: `~/.cargo/bin/cargo test -p vitric-cli --test region --`
Expected: PASS — all region tests including the 4 new E5 tests.

## Step 7: Run full test suite + frontier gate

Run:
- `~/.cargo/bin/cargo test --workspace -- --skip typescript` (the 2 typescript tests are pre-existing failures due to missing esbuild binary — skip them)
- `~/.cargo/bin/cargo run --release -- gate games/frontier`

Expected: all pass. Gate hash must be `0xab58ec29d99275df` (unchanged — describe_world changes don't affect the deterministic trajectory; snapshot/replay tests don't change any production code path).

## Step 8: Commit

```bash
git add crates/vitric-render/src/lib.rs crates/vitric-cli/tests/region.rs
git commit -m "feat(render,sim): E5 describe dormant dim + snapshot/replay plumbing

describe_world output gains a top-level \`dormant\` array listing entities
whose Region.state is dormant/frozen. The existing visible/offscreen
arrays are unchanged (they already exclude dormant entities via world.query
filtering). Dormant entries include region info (id, state) so the agent
can reason about unexplored regions.

Snapshot/restore already preserves dormant state (Region is a World
component) — locked by an explicit test. Replay of a recording that
starts with a dormant region is hash-identical — locked by a test.
Pre-recording thaw_region (host API call re-run at replay) is also
hash-identical — locked by a test.

Note: thaw_region DURING recording is NOT replayable via Sim::replay (by
design — host API calls are not recorded; the host must re-run them at
the same ticks during replay). This is a design decision, not a bug;
adding a between-tick host hook to replay is out of scope for E5."
git push origin main
```

## Report contract

Write your full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/briefs/task-5-report.md` with these sections:
1. **Status**: DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED
2. **Commits**: list commit SHA(s) pushed to main
3. **Test results**: table of commands and pass/fail (include the gate hash)
4. **Files touched**: table of file + line count changes
5. **Deviations from brief**: any deviations from this brief (e.g., if you found a simpler way, or if a test couldn't be written as specified)
6. **Concerns**: any doubts or observations the reviewer should know

Return in your response: status, commit SHAs, one-line test summary, and concerns (if any).
