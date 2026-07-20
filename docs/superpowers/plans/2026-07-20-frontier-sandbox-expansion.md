# Frontier Sandbox Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand frontier from a 9-day demo into a deep, sandbox-grade game with 6 new systems (seasons/weather, combat, tech tree, companions expansion, trading/diplomacy, map expansion), 5 new engine capabilities (Region dormant/active, catch_up scheduling, seeded RNG substreams, view-frustum culling, snapshot/replay plumbing), pacing rebalance, and README upgrade.

**Architecture:** Engine capabilities (E1-E5) land first as they are the foundation for map expansion and region-aware systems. Game systems follow in dependency order: independent systems (seasons, tech tree, companions) first, then dependent systems (combat → tech tree; trading → companions + map; map expansion → E1+E3), then pacing + gate + README polish. All work is data-driven — no component-external state channels.

**Tech Stack:** Rust workspace (vitric-ecs, vitric-sim, vitric-render, vitric-script, vitric-control, vitric-cli, vitric-data, vitric-rules, vitric-playtest), frontier game (rules JSON, scripts JS, schema.json, scenes/main.json). Tests via `cargo test -p <crate>` and `vitric check`/`vitric gate games/frontier`.

## Global Constraints

- **Code comments MUST be English** (`//`, `///`, `//!`, `/* */`). Project-wide convention since 2026-07-17.
- **String literals keep original language** (panic messages, game dialogue, UI labels in Chinese where existing patterns are Chinese).
- **Every field written by a JS system OR read by a rule OR accessed via ctx.getField/setField MUST be declared in `schema.json`** — non-negotiable, see `.superpowers/sdd/review-checklist.md`.
- **Determinism is the contract**: no `SystemTime`, no `thread_rng()`, no floating-point ops that aren't bit-stable across platforms. Use `ctx.random()` or `ctx.random_stream(name)` only.
- **Auto commit + push after each task completes and passes tests** — remote is SSH `git@github.com:BlackBearCC/vitric.git`, branch `main`.
- **Backward compatibility**: existing `examples/` and existing frontier gate recording must still pass after engine changes. Do not break `vitric gate games/frontier` until Task 15 explicitly re-records.

## File Structure

**Engine crates modified:**
- `crates/vitric-ecs/src/world.rs` — `query()` adds dormant filtering; new `is_dormant` helper.
- `crates/vitric-sim/src/sim.rs` — `step()` skips dormant entities in logic dispatch; new `thaw_region()` entry point; `invoke_catch_up_for_region()`.
- `crates/vitric-sim/src/pcg.rs` — new `Substream` struct + `Substream::new(world_seed, name)`.
- `crates/vitric-render/src/lib.rs` — `render_world()` adds view-frustum + dormant culling; `describe_world()` extends offscreen classification with `dormant` dim.
- `crates/vitric-script/src/lib.rs` — `ctx.random_stream(name)` API; `vitric.system()` accepts optional `catch_up` fn.
- `crates/vitric-cli/src/runtime.rs` — camera `world_bounds` clamping in motion integration.
- `crates/vitric-cli/tests/region.rs` (new) — engine integration tests for E1-E5.

**Frontier game files modified:**
- `games/frontier/schema.json` — add Region, Season, Weather, Hp, Enemy, Weapon, Guard, Research, TechPoint, Faction components; extend Persona, Colony, Mode, Camera, Inventory.
- `games/frontier/scenes/main.json` — extend wild region; add dormant region markers; add new UI entities.
- `games/frontier/scripts/` — modify clock.js, flare.js, crops.js, colony.js, companion.js, economy.js, poi.js, hud.js, world.js; add region.js, combat.js, research.js, faction.js.
- `games/frontier/rules/` — add combat.json, research.json, faction.json, region.json; modify flare.json, hud.json, ui.json, affordability.json, companion.json, quest.json.
- `games/frontier/qa/clear.json` — re-recorded in Task 15.
- `games/frontier/tools/record_clear.py` — updated for new pacing + acceleration mode.
- `games/frontier/tools/test_progression.py` — extended with new system tests.
- `README.md`, `README.zh-CN.md` — hero GIF, Frontier section, capabilities matrix.

---

## Phase 1: Engine Capabilities (Tasks 1-5)

### Task 1: E1 — Region dormant/active

**Files:**
- Modify: `crates/vitric-ecs/src/world.rs`
- Modify: `crates/vitric-sim/src/sim.rs`
- Modify: `crates/vitric-render/src/lib.rs`
- Modify: `games/frontier/schema.json` (add Region component)
- Test: `crates/vitric-cli/tests/region.rs` (new)

**Interfaces:**
- Produces: `world::query()` filters entities with dormant `Region` component; `Sim::thaw_region(id)` transitions state + emits `region-thaw{id}` event; renderer skips dormant entities.
- Consumes: none (foundation task).

- [ ] **Step 1: Write failing test for dormant query filtering**

Create `crates/vitric-cli/tests/region.rs`:

```rust
use vitric_cli::test_support::TestSim;

#[test]
fn dormant_entities_excluded_from_query() {
    let mut sim = TestSim::with_schema("games/frontier/schema.json");
    let active_e = sim.spawn_named("active_ent");
    sim.add_component(active_e, "Position", r#"{"x":0,"y":0}"#);
    sim.add_component(active_e, "Region", r#"{"id":"home","biome":"home","state":"active","discovered":1,"anchor_x":0,"anchor_y":0,"w":28,"h":12,"dormant_ticks":0}"#);

    let dormant_e = sim.spawn_named("dormant_ent");
    sim.add_component(dormant_e, "Position", r#"{"x":100,"y":100}"#);
    sim.add_component(dormant_e, "Region", r#"{"id":"mountain","biome":"mountain","state":"dormant","discovered":0,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0}"#);

    let results = sim.query(&["Position"]);
    let names: Vec<&str> = results.iter().map(|id| sim.name_of(*id).unwrap()).collect();
    assert!(names.contains(&"active_ent"));
    assert!(!names.contains(&"dormant_ent"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vitric-cli --test region dormant_entities_excluded_from_query`
Expected: FAIL — query returns both entities (no Region filtering yet).

- [ ] **Step 3: Add Region component to frontier schema.json**

In `games/frontier/schema.json`, add to `components`:

```json
"Region": {
  "fields": {
    "id": { "type": "text", "default": "" },
    "biome": { "type": "text", "default": "home" },
    "state": { "type": "enum", "variants": ["dormant", "active", "frozen"], "default": "active" },
    "discovered": { "type": "int", "default": 0 },
    "anchor_x": { "type": "number", "default": 0 },
    "anchor_y": { "type": "number", "default": 0 },
    "w": { "type": "int", "default": 0 },
    "h": { "type": "int", "default": 0 },
    "dormant_ticks": { "type": "int", "default": 0 },
    "spawn_timer": { "type": "number", "default": 7200 }
  }
}
```

- [ ] **Step 4: Implement dormant filtering in world.query**

In `crates/vitric-ecs/src/world.rs`, modify `query`:

```rust
pub fn query(&self, component_names: &[&str]) -> Vec<EntityId> {
    self.entities.iter()
        .filter(|(_, id)| {
            let has_components = component_names.iter()
                .all(|cn| self.has_component(**id, cn));
            if !has_components { return false; }
            if self.is_dormant(**id) { return false; }
            true
        })
        .map(|(_, id)| *id)
        .collect()
}

pub fn is_dormant(&self, id: EntityId) -> bool {
    if let Some(region) = self.get_component(id, "Region") {
        if let Some(state) = region.get("state").and_then(|v| v.as_str()) {
            return state == "dormant" || state == "frozen";
        }
    }
    false
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p vitric-cli --test region dormant_entities_excluded_from_query`
Expected: PASS.

- [ ] **Step 6: Write failing test for render skip**

Append to `crates/vitric-cli/tests/region.rs`:

```rust
#[test]
fn dormant_entities_skipped_in_render() {
    let mut sim = TestSim::with_schema("games/frontier/schema.json");
    let dormant_e = sim.spawn_named("hidden_sprite");
    sim.add_component(dormant_e, "Position", r#"{"x":50,"y":50}"#);
    sim.add_component(dormant_e, "Sprite", r#"{"w":1,"h":1,"image":"rock.png"}"#);
    sim.add_component(dormant_e, "Region", r#"{"id":"mountain","state":"dormant","discovered":0,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200}"#);

    let describe = sim.render_describe();
    let all_names: Vec<&str> = describe.on_screen.iter()
        .chain(describe.off_screen.iter())
        .map(|e| e.name.as_str())
        .collect();
    assert!(!all_names.contains(&"hidden_sprite"));
}
```

- [ ] **Step 7: Run render test to verify it fails**

Run: `cargo test -p vitric-cli --test region dormant_entities_skipped_in_render`
Expected: FAIL.

- [ ] **Step 8: Implement render skip in render_world and describe_world**

In `crates/vitric-render/src/lib.rs`:

```rust
fn is_renderable(world: &World, id: EntityId) -> bool {
    !world.is_dormant(id)
}

// In render_world entity loop:
for &id in world.query(&["Position", "Sprite"]).iter() {
    if !is_renderable(world, id) { continue; }
    // ... existing render logic
}
```

Apply same filter in `describe_world`.

- [ ] **Step 9: Run render test to verify it passes**

Run: `cargo test -p vitric-cli --test region dormant_entities_skipped_in_render`
Expected: PASS.

- [ ] **Step 10: Write failing test for sim skip + thaw**

```rust
#[test]
fn dormant_entities_skip_logic_systems() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    let e = sim.spawn_named("frozen_crop");
    sim.add_component(e, "Position", r#"{"x":40,"y":40}"#);
    sim.add_component(e, "Crop", r#"{"kind":"wheat","stage":0,"timer":0,"_tend_t":0}"#);
    sim.add_component(e, "Region", r#"{"id":"mountain","state":"dormant","discovered":0,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200}"#);

    sim.step(60);
    let timer = sim.get_component(e, "Crop").get("timer").unwrap().as_f64().unwrap();
    assert_eq!(timer, 0.0);
}

#[test]
fn thaw_region_activates_entities() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    sim.thaw_region("mountain");
    let mountain_e = sim.entity("mountain").unwrap();
    let region = sim.get_component(mountain_e, "Region");
    assert_eq!(region.get("state").unwrap().as_str(), Some("active"));
    let events = sim.recent_events();
    assert!(events.iter().any(|e| e.name == "region-thaw"));
}
```

- [ ] **Step 11: Run sim tests to verify they fail**

Run: `cargo test -p vitric-cli --test region --`
Expected: FAIL — `thaw_region` method doesn't exist.

- [ ] **Step 12: Implement sim skip + thaw_region**

In `crates/vitric-sim/src/sim.rs`:

```rust
pub fn step(&mut self) {
    // ... existing motion/camera/collision
    for system in &self.logic_systems {
        let entities = self.world.query(&system.queries); // Already filters dormant
        (system.fn_ptr)(&mut self.world, &entities, &mut self.ctx);
    }
    // Accumulate dormant_ticks on dormant regions
    self.accumulate_dormant_ticks();
    // ... existing tick advance + hash
}

fn accumulate_dormant_ticks(&mut self) {
    // For each Region entity with state=dormant or frozen, increment dormant_ticks
    let region_entities: Vec<EntityId> = self.world.query(&["Region"]);
    // Note: query filters dormant entities, so we need a different approach
    // Use entities_iter() which doesn't filter
    for (_, id) in self.world.entities_iter() {
        if let Some(region) = self.world.get_component(*id, "Region") {
            if let Some(state) = region.get("state").and_then(|v| v.as_str()) {
                if state == "dormant" || state == "frozen" {
                    let dt = self.world.get_component(*id, "Region").get("dormant_ticks")
                        .and_then(|v| v.as_i64()).unwrap_or(0);
                    self.world.set_field(*id, "Region.dormant_ticks", json!(dt + 1));
                }
            }
        }
    }
}

pub fn thaw_region(&mut self, id: &str) {
    let region_e = self.world.entity(id).expect("region entity must exist");
    if let Some(mut region) = self.world.get_component(region_e, "Region").cloned() {
        let was_discovered = region.get("discovered").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
        region["state"] = json!("active");
        region["discovered"] = json!(1);
        self.world.set_component(region_e, "Region", region).unwrap();
        self.ctx.emit("region-thaw", json!({"id": id}));
        if was_discovered {
            self.invoke_catch_up_for_region(id);
        }
    }
}

fn invoke_catch_up_for_region(&mut self, _region_id: &str) {
    // Stub — implemented in Task 2
}
```

- [ ] **Step 13: Run sim tests to verify they pass**

Run: `cargo test -p vitric-cli --test region --`
Expected: PASS.

- [ ] **Step 14: Run full engine test suite for regressions**

Run: `cargo test --workspace`
Expected: all existing tests pass.

- [ ] **Step 15: Commit**

```bash
git add crates/vitric-ecs/src/world.rs crates/vitric-sim/src/sim.rs crates/vitric-render/src/lib.rs crates/vitric-cli/tests/region.rs games/frontier/schema.json
git commit -m "feat(ecs,sim,render): E1 Region dormant/active

Add Region component with dormant/active/frozen states. world.query,
render_world, and sim logic dispatch all skip dormant entities.
state_hash still covers dormant entities. Sim::thaw_region() transitions
state and emits region-thaw event."
git push origin main
```

---

### Task 2: E2 — Catch_up system API

**Files:**
- Modify: `crates/vitric-script/src/lib.rs`
- Modify: `crates/vitric-sim/src/sim.rs`
- Modify: `games/frontier/scripts/crops.js` (declare catch_up)
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `vitric.system(name, opts, fn, catch_up_fn?)` — 4th optional arg; `Sim::invoke_catch_up_for_region(id)` calls each system's catch_up for entities in that region.
- Consumes: Task 1's `thaw_region` stub.

- [ ] **Step 1: Write failing test for catch_up invocation**

Append to `crates/vitric-cli/tests/region.rs`:

```rust
#[test]
fn catch_up_advances_dormant_crop_on_thaw() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    let crop_e = sim.spawn_named("mountain_crop");
    sim.add_component(crop_e, "Position", r#"{"x":15,"y":15}"#);
    sim.add_component(crop_e, "Crop", r#"{"kind":"wheat","stage":0,"timer":0,"_tend_t":0}"#);
    sim.add_component(crop_e, "Region", r#"{"id":"mountain","state":"dormant","discovered":0,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200}"#);

    sim.step(3600);
    let timer_before = sim.get_component(crop_e, "Crop").get("timer").unwrap().as_f64().unwrap();
    assert_eq!(timer_before, 0.0);

    sim.thaw_region("mountain");
    let timer_after = sim.get_component(crop_e, "Crop").get("timer").unwrap().as_f64().unwrap();
    assert!(timer_after > 0.0, "catch_up should advance timer, got {}", timer_after);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vitric-cli --test region catch_up_advances_dormant_crop_on_thaw`
Expected: FAIL.

- [ ] **Step 3: Extend vitric.system API to accept catch_up**

In `crates/vitric-script/src/lib.rs`, modify the system registration to accept an optional 4th argument (the catch_up function). Store it in the `System` struct as `catch_up: Option<JSValue>`.

- [ ] **Step 4: Implement invoke_catch_up_for_region in sim.rs**

In `crates/vitric-sim/src/sim.rs`:

```rust
fn invoke_catch_up_for_region(&mut self, region_id: &str) {
    let region_e = self.world.entity(region_id).unwrap();
    let dormant_ticks = self.world.get_component(region_e, "Region")
        .get("dormant_ticks").and_then(|v| v.as_i64()).unwrap_or(0);
    if dormant_ticks == 0 { return; }

    // Find all entities in this region (using entities_iter, not query which filters dormant)
    let region_entities: Vec<EntityId> = self.world.entities_iter()
        .filter(|(_, id)| {
            if let Some(r) = self.world.get_component(**id, "Region") {
                r.get("id").and_then(|v| v.as_str()) == Some(region_id)
            } else { false }
        })
        .map(|(_, id)| *id)
        .collect();

    for system in &self.logic_systems {
        if let Some(catch_up_fn) = &system.catch_up {
            for &entity in &region_entities {
                if system.queries.iter().all(|q| self.world.has_component(entity, q)) {
                    catch_up_fn(entity, &mut self.world, &mut self.ctx, dormant_ticks);
                }
            }
        }
    }

    // Reset dormant_ticks
    self.world.set_field(region_e, "Region.dormant_ticks", json!(0));
}
```

- [ ] **Step 5: Add catch_up to frontier's crop-grow system**

In `games/frontier/scripts/crops.js`, modify `crop-grow` system to add 4th arg:

```javascript
vitric.system("crop-grow",
  { query: ["Crop", "Position"], writes: ["Crop"] },
  function(entities, ctx) {
    const SECONDS_PER_STAGE = 4.0;
    const dt = ctx.dt;
    for (const e of entities) {
      let timer = ctx.getField(e, "Crop.timer") + dt;
      let stage = ctx.getField(e, "Crop.stage");
      if (timer >= SECONDS_PER_STAGE) {
        timer -= SECONDS_PER_STAGE;
        stage += 1;
        if (stage >= 3) {
          ctx.emit("crop-ready", { entity: e });
          stage = 2;
        }
      }
      ctx.setField(e, "Crop.timer", timer);
      ctx.setField(e, "Crop.stage", stage);
    }
  },
  function catch_up(entity, ctx, dormant_ticks) {
    const SECONDS_PER_STAGE = 4.0;
    const dt_per_tick = 1.0 / 60.0;
    const elapsed_seconds = dormant_ticks * dt_per_tick;
    let timer = ctx.getField(entity, "Crop.timer") + elapsed_seconds;
    let stage = ctx.getField(entity, "Crop.stage");
    while (timer >= SECONDS_PER_STAGE && stage < 2) {
      timer -= SECONDS_PER_STAGE;
      stage += 1;
      if (stage >= 3) {
        ctx.emit("crop-ready", { entity });
        stage = 2;
      }
    }
    ctx.setField(entity, "Crop.timer", timer);
    ctx.setField(entity, "Crop.stage", stage);
  }
);
```

- [ ] **Step 6: Run catch_up test to verify it passes**

Run: `cargo test -p vitric-cli --test region catch_up_advances_dormant_crop_on_thaw`
Expected: PASS.

- [ ] **Step 7: Run full test suite for regressions**

Run: `cargo test --workspace && cargo run --release -- check games/frontier`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/vitric-script/src/lib.rs crates/vitric-sim/src/sim.rs crates/vitric-cli/tests/region.rs games/frontier/scripts/crops.js
git commit -m "feat(script,sim): E2 catch_up system API

Systems can declare optional catch_up(entity, ctx, dormant_ticks).
On region thaw, engine invokes catch_up for each entity in the region.
crop-grow declares catch_up as reference implementation."
git push origin main
```

---

### Task 3: E3 — Seeded RNG substreams

**Files:**
- Modify: `crates/vitric-sim/src/pcg.rs`
- Modify: `crates/vitric-script/src/lib.rs`
- Modify: `crates/vitric-sim/src/sim.rs` (snapshot/restore/hash integration)
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `ctx.random_stream(name)` returns `{ next(): number [0,1), nextInt(min,max): int }`. Substream state persisted in snapshot and hashed.

- [ ] **Step 1: Write failing test for substream determinism**

```rust
#[test]
fn random_stream_same_seed_regardless_of_call_timing() {
    let mut sim1 = TestSim::with_scene("games/frontier/scenes/main.json");
    let mut sim2 = TestSim::with_scene("games/frontier/scenes/main.json");

    let r1: Vec<i32> = (0..5).map(|_| {
        sim1.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)")
    }).collect();

    sim2.step(1000);
    let r2: Vec<i32> = (0..5).map(|_| {
        sim2.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)")
    }).collect();

    assert_eq!(r1, r2);
}

#[test]
fn random_stream_state_in_snapshot() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    sim.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");
    sim.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");

    let snapshot = sim.snapshot();
    let mut restored = TestSim::restore(&snapshot);

    let r1 = sim.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");
    let r2 = restored.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");
    assert_eq!(r1, r2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p vitric-cli --test region random_stream`
Expected: FAIL.

- [ ] **Step 3: Implement Substream in pcg.rs**

```rust
#[derive(Clone, Debug)]
pub struct Substream {
    state: u64,
    increment: u64,
}

impl Substream {
    pub fn new(world_seed: u64, name: &str) -> Self {
        let mut hash = world_seed;
        for byte in name.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let increment = hash | 1;
        let state = Self::init_state(0, increment);
        Self { state, increment }
    }

    fn init_state(seed: u64, increment: u64) -> u64 {
        let mut s = Pcg32::new(seed, increment);
        s.next_u32();
        s.state
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6364136223846793005).wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        (xorshifted >> rot) | (xorshifted << ((!rot).wrapping_add(1) & 31))
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64) / (u32::MAX as f64 + 1.0)
    }
}
```

- [ ] **Step 4: Add substream registry to Sim + integrate into snapshot/hash**

In `crates/vitric-sim/src/sim.rs`, add `substreams: HashMap<String, Substream>` to `Sim`. Expose `random_stream(name) -> &mut Substream`. Include substream state in `snapshot()`, `restore()`, and `state_hash()` (sort keys for deterministic hash).

- [ ] **Step 5: Expose ctx.random_stream to JS**

In `crates/vitric-script/src/lib.rs`, add `ctx.random_stream(name)` returning `{ next(), nextInt(min, max) }` that bridges to `sim.substreams`.

- [ ] **Step 6: Run substream tests**

Run: `cargo test -p vitric-cli --test region random_stream`
Expected: PASS.

- [ ] **Step 7: Run full test suite + frontier gate**

Run: `cargo test --workspace && cargo run --release -- gate games/frontier`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/vitric-sim/src/pcg.rs crates/vitric-sim/src/sim.rs crates/vitric-script/src/lib.rs crates/vitric-cli/tests/region.rs
git commit -m "feat(sim,script): E3 seeded RNG substreams

ctx.random_stream(name) returns a deterministic substream seeded by
(world_seed, name). Independent of call timing — replay-safe even if
region thaw happens at different ticks. State persisted in snapshot
and included in state_hash."
git push origin main
```

---

### Task 4: E4 — View-frustum culling

**Files:**
- Modify: `crates/vitric-render/src/lib.rs`
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `render_world` skips entities outside camera viewport (in addition to dormant skip from Task 1).

- [ ] **Step 1: Write failing perf test for culling**

```rust
#[test]
fn render_time_scales_with_visible_not_total() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    for i in 0..1000 {
        let e = sim.spawn();
        sim.add_component(e, "Position", &format!(r#"{{"x":{0},"y":{1}}}"#, 1000 + i, 1000));
        sim.add_component(e, "Sprite", r#"{"w":1,"h":1,"image":"rock.png"}"#);
    }

    let start = std::time::Instant::now();
    sim.render_frame();
    let elapsed_with_culling = start.elapsed();

    let mut sim2 = TestSim::with_scene("games/frontier/scenes/main.json");
    let start2 = std::time::Instant::now();
    sim2.render_frame();
    let elapsed_baseline = start2.elapsed();

    assert!(elapsed_with_culling < elapsed_baseline * 3,
        "culling should keep render time bounded");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vitric-cli --test region render_time_scales_with_visible_not_total`
Expected: FAIL.

- [ ] **Step 3: Implement view-frustum culling in render_world**

In `crates/vitric-render/src/lib.rs`:

```rust
pub fn render_world(world: &World, camera: &Camera, frame: &mut Frame) {
    let viewport = camera.viewport_bounds();
    let margin = 4.0;

    for &id in world.query(&["Position", "Sprite"]).iter() {
        if !is_renderable(world, id) { continue; } // Dormant skip

        let pos = world.get_component(id, "Position").unwrap();
        let x = pos["x"].as_f64().unwrap();
        let y = pos["y"].as_f64().unwrap();
        let sprite = world.get_component(id, "Sprite").unwrap();
        let w = sprite["w"].as_f64().unwrap_or(1.0);
        let h = sprite["h"].as_f64().unwrap_or(1.0);

        if x + w < viewport.0 - margin { continue; }
        if x > viewport.2 + margin { continue; }
        if y + h < viewport.1 - margin { continue; }
        if y > viewport.3 + margin { continue; }

        // ... existing render logic
    }
}

impl Camera {
    pub fn viewport_bounds(&self) -> (f64, f64, f64, f64) {
        let half_w = self.view_w / 2.0 / self.scale;
        let half_h = self.view_h / 2.0 / self.scale;
        (self.x - half_w, self.y - half_h, self.x + half_w, self.y + half_h)
    }
}
```

- [ ] **Step 4: Run culling test to verify it passes**

Run: `cargo test -p vitric-cli --test region render_time_scales_with_visible_not_total`
Expected: PASS.

- [ ] **Step 5: Apply same culling to wgpu GPU mirror**

Mirror the AABB filter in GPU draw call queuing.

- [ ] **Step 6: Run full test suite + frontier gate**

Run: `cargo test --workspace && cargo run --release -- gate games/frontier`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/vitric-render/src/lib.rs crates/vitric-cli/tests/region.rs
git commit -m "feat(render): E4 view-frustum culling

render_world skips entities outside camera viewport (with margin for
shadow casters). Applies to both CPU rasterizer and wgpu GPU mirror.
Performance scales with visible entities, not total."
git push origin main
```

---

### Task 5: E5 — Snapshot/replay/describe plumbing

**Files:**
- Modify: `crates/vitric-render/src/lib.rs` (describe dormant dim)
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `render/describe` classifies entities as `on_screen` / `off_screen` / `dormant`. Replay with region-thaw events is hash-identical.

- [ ] **Step 1: Write failing test for describe dormant classification**

```rust
#[test]
fn describe_classifies_dormant_entities() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    let e = sim.spawn_named("hidden");
    sim.add_component(e, "Position", r#"{"x":50,"y":50}"#);
    sim.add_component(e, "Sprite", r#"{"w":1,"h":1}"#);
    sim.add_component(e, "Region", r#"{"id":"mountain","state":"dormant","discovered":0,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200}"#);

    let desc = sim.render_describe();
    assert!(desc.dormant.iter().any(|ent| ent.name == "hidden"));
    assert!(!desc.on_screen.iter().any(|ent| ent.name == "hidden"));
    assert!(!desc.off_screen.iter().any(|ent| ent.name == "hidden"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vitric-cli --test region describe_classifies_dormant_entities`
Expected: FAIL.

- [ ] **Step 3: Extend describe_world with dormant classification**

In `crates/vitric-render/src/lib.rs`, modify `describe_world` to add `dormant` field to `DescribeResult`. Iterate all entities (not just non-dormant); classify dormant ones into the new list.

- [ ] **Step 4: Run describe test to verify it passes**

Run: `cargo test -p vitric-cli --test region describe_classifies_dormant_entities`
Expected: PASS.

- [ ] **Step 5: Write snapshot round-trip + replay consistency tests**

```rust
#[test]
fn snapshot_preserves_dormant_state() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    sim.thaw_region("mountain");
    let mountain_e = sim.entity("mountain").unwrap();
    sim.set_component(mountain_e, "Region", r#"{"id":"mountain","state":"frozen","discovered":1,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200}"#);

    let snap = sim.snapshot();
    let mut restored = TestSim::restore(&snap);
    let region = restored.get_component(mountain_e, "Region");
    assert_eq!(region.get("state").unwrap().as_str(), Some("frozen"));
    assert_eq!(region.get("discovered").unwrap().as_i64(), Some(1));
}

#[test]
fn replay_with_region_thaw_is_hash_identical() {
    // Create a minimal recording that thaws a region, then replay it
    // Fixture at tests/fixtures/region_thaw_recording.json
    let bin = env!("CARGO_BIN_EXE_vitric");
    let output = std::process::Command::new(bin)
        .args(&["replay", "tests/fixtures/region_thaw_recording.json"])
        .output()
        .expect("failed to run replay");
    assert!(output.status.success());
}
```

- [ ] **Step 6: Run all E5 tests**

Run: `cargo test -p vitric-cli --test region --`
Expected: PASS.

- [ ] **Step 7: Run full test suite + frontier gate**

Run: `cargo test --workspace && cargo run --release -- gate games/frontier`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/vitric-render/src/lib.rs crates/vitric-cli/tests/region.rs tests/fixtures/region_thaw_recording.json
git commit -m "feat(render,sim): E5 describe dormant dim + snapshot/replay plumbing

render/describe classifies entities as on_screen/off_screen/dormant.
Snapshot preserves dormant state. Replay with region-thaw events is
hash-identical. Closes E1-E5 engine capability set."
git push origin main
```

---

## Phase 2: Independent Game Systems (Tasks 6-9)

### Task 6: Seasons & Weather

**Files:**
- Modify: `games/frontier/schema.json` (add Season, Weather)
- Modify: `games/frontier/scenes/main.json` (Season/Weather on Clock entity)
- Modify: `games/frontier/scripts/clock.js` (season advance)
- Modify: `games/frontier/scripts/flare.js` (refactor to weather system)
- Modify: `games/frontier/scripts/crops.js` (season multiplier)
- Modify: `games/frontier/scripts/colony.js` (weather multiplier)
- Modify: `games/frontier/rules/flare.json`, `rules/hud.json`
- Modify: `games/frontier/tools/test_progression.py`

**Interfaces:**
- Produces: `Season` component (spring/summer/autumn/winter, 12 days each, 48-day year); `Weather` component (clear/cloudy/rain/storm/flare); season multipliers on crop growth + resource yield; weather multipliers on colony production.

- [ ] **Step 1: Add Season and Weather components to schema.json**

```json
"Season": {
  "fields": {
    "current": { "type": "enum", "variants": ["spring", "summer", "autumn", "winter"], "default": "spring" },
    "day_in_season": { "type": "int", "default": 0 },
    "year": { "type": "int", "default": 1 }
  }
},
"Weather": {
  "fields": {
    "current": { "type": "enum", "variants": ["clear", "cloudy", "rain", "storm", "flare"], "default": "clear" },
    "timer": { "type": "number", "default": 30 },
    "next": { "type": "enum", "variants": ["clear", "cloudy", "rain", "storm", "flare"], "default": "clear" }
  }
}
```

- [ ] **Step 2: Add Season/Weather to Clock entity in scenes/main.json**

```json
{
  "name": "clock",
  "components": {
    "Clock": { "day": 1, "time": 0, "tod": "晨", "last_day_emit": 0 },
    "Season": { "current": "spring", "day_in_season": 0, "year": 1 },
    "Weather": { "current": "clear", "timer": 30, "next": "clear" }
  }
}
```

Also add `Weather` to `colony` entity (or keep on Clock — pick one and document).

- [ ] **Step 3: Extend clock.js to advance seasons**

```javascript
const SEASON_DAYS = 12;
const SEASONS = ["spring", "summer", "autumn", "winter"];

// In day-wrap logic:
function advanceSeason(ctx) {
  const clock_e = ctx.entity("clock");
  let day_in_season = ctx.getField(clock_e, "Season.day_in_season") + 1;
  let season = ctx.getField(clock_e, "Season.current");
  let year = ctx.getField(clock_e, "Season.year");

  if (day_in_season >= SEASON_DAYS) {
    day_in_season = 0;
    const idx = SEASONS.indexOf(season);
    const next_idx = (idx + 1) % SEASONS.length;
    season = SEASONS[next_idx];
    if (next_idx === 0) year += 1;
    ctx.emit("season-change", { season, year });
  }

  ctx.setField(clock_e, "Season.day_in_season", day_in_season);
  ctx.setField(clock_e, "Season.current", season);
  ctx.setField(clock_e, "Season.year", year);
}
```

- [ ] **Step 4: Refactor flare.js into weather system**

```javascript
const WEATHER_DURATION = [30, 90];
const SEASONAL_WEIGHTS = {
  spring: { clear: 50, cloudy: 30, rain: 15, storm: 5, flare: 0 },
  summer: { clear: 40, cloudy: 20, rain: 10, storm: 5, flare: 25 },
  autumn: { clear: 45, cloudy: 30, rain: 20, storm: 5, flare: 0 },
  winter: { clear: 30, cloudy: 40, rain: 0, storm: 30, flare: 0 }
};

vitric.system("weather-tick", { query: [], writes: ["Weather"] }, function(entities, ctx) {
  const colony_e = ctx.entity("colony");
  let timer = ctx.getField(colony_e, "Weather.timer") - ctx.dt;
  if (timer <= 0) {
    const clock_e = ctx.entity("clock");
    const season = ctx.getField(clock_e, "Season.current");
    const weights = SEASONAL_WEIGHTS[season];
    const new_weather = weightedPick(ctx, weights);
    ctx.setField(colony_e, "Weather.current", new_weather);
    ctx.setField(colony_e, "Weather.timer", randInRange(ctx, WEATHER_DURATION[0], WEATHER_DURATION[1]));
    ctx.emit("weather-change", { weather: new_weather });
    if (new_weather === "flare") {
      ctx.emit("flare-hit", {});
    }
  } else {
    ctx.setField(colony_e, "Weather.timer", timer);
  }
});

function weightedPick(ctx, weights) {
  const total = Object.values(weights).reduce((a, b) => a + b, 0);
  let r = ctx.random() * total;
  for (const [k, w] of Object.entries(weights)) {
    r -= w;
    if (r <= 0) return k;
  }
  return Object.keys(weights)[0];
}

function randInRange(ctx, min, max) {
  return min + ctx.random() * (max - min);
}
```

Remove old `flare_timer` countdown logic.

- [ ] **Step 5: Apply season multiplier in crops.js**

```javascript
const SEASON_CROP_MULT = {
  spring: 1.2, summer: 1.0, autumn: 1.5, winter: 0.3
};

// In crop-grow system function:
const clock_e = ctx.entity("clock");
const season = ctx.getField(clock_e, "Season.current");
const mult = SEASON_CROP_MULT[season];
const dt = ctx.dt * mult;
// ... use dt for growth
```

Also update the `catch_up` function (Task 2) to apply same multiplier.

- [ ] **Step 6: Apply weather multiplier in colony.js tally**

```javascript
const WEATHER_RATE_MULT = {
  clear:  { power: 1.0, water: 1.0 },
  cloudy: { power: 0.7, water: 1.0 },
  rain:   { power: 0.7, water: 1.5 },
  storm:  { power: 0.3, water: 1.0 },
  flare:  { power: 0.0, water: 1.0 }
};

const SEASON_RESOURCE_MULT = {
  spring: 1.0, summer: 0.8, autumn: 1.2, winter: 0.5
};

// In tally system:
const weather = ctx.getField(colony_e, "Weather.current");
const season = ctx.getField(clock_e, "Season.current");
const wmult = WEATHER_RATE_MULT[weather];
const smult = SEASON_RESOURCE_MULT[season];
// Apply to o2_rate, pow_rate, water_rate, food_rate
```

- [ ] **Step 7: Update flare.json rules for weather-change events**

Replace direct `flare_timer` references with `weather-change` handlers (see spec §4.1 for example).

- [ ] **Step 8: Add season/weather HUD labels (prepare for Task 7)**

Add `season_lbl` and `weather_lbl` entities to scenes/main.json. Add HUD rules to refresh them.

- [ ] **Step 9: Run schema check**

Run: `cargo run --release -- check games/frontier`
Expected: PASS.

- [ ] **Step 10: Write season cycle progression test**

In `games/frontier/tools/test_progression.py`:

```python
def test_season_cycle_completes():
    sim = TestSim("games/frontier")
    for _ in range(48 * 90 * 60):  # 48 days at 90s/day, 60 ticks/sec
        sim.step(1)
    season = sim.get_field("clock", "Season.current")
    year = sim.get_field("clock", "Season.year")
    assert year == 2
    assert season == "spring"
```

- [ ] **Step 11: Run progression test + gate**

Run: `cd games/frontier && python tools/test_progression.py && cd ../.. && cargo run --release -- gate games/frontier`
Expected: PASS (gate may fail due to DAY_SEC change — that's OK, will be fixed in Task 14/15).

- [ ] **Step 12: Commit**

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json games/frontier/scripts/clock.js games/frontier/scripts/flare.js games/frontier/scripts/crops.js games/frontier/scripts/colony.js games/frontier/rules/flare.json games/frontier/rules/hud.json games/frontier/tools/test_progression.py
git commit -m "feat(frontier): seasons & weather system

Add Season (4 seasons × 12 days = 48-day year) and Weather (5 states
weighted by season). Refactor flare.js into weather system (flare is
now a weather variant). Apply season multipliers to crop growth and
weather multipliers to colony production."
git push origin main
```

---

### Task 7: Seasons/Weather HUD

**Files:**
- Modify: `games/frontier/scenes/main.json` (add season/weather labels + forecast bar)
- Modify: `games/frontier/scripts/hud.js`
- Modify: `games/frontier/rules/hud.json`

**Interfaces:**
- Produces: visible season indicator, weather icon, 7-day forecast bar.

- [ ] **Step 1: Add HUD entities for season/weather/forecast**

In `games/frontier/scenes/main.json`, add `season_lbl`, `weather_lbl`, `forecast_lbl` entities positioned in HUD top row.

- [ ] **Step 2: Add forecast bar system in hud.js**

Use `ctx.random_stream("forecast")` for deterministic 7-day forecast (doesn't flicker each tick).

- [ ] **Step 3: Update hud.json rules to refresh labels**

- [ ] **Step 4: Run schema check + gate**

Run: `cargo run --release -- check games/frontier && cargo run --release -- gate games/frontier`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add games/frontier/scenes/main.json games/frontier/scripts/hud.js games/frontier/rules/hud.json
git commit -m "feat(frontier): season/weather HUD with 7-day forecast"
git push origin main
```

---

### Task 8: Tech Tree

**Files:**
- Modify: `games/frontier/schema.json` (add Research, TechPoint; extend Mode, Inventory)
- Create: `games/frontier/scripts/research.js`
- Create: `games/frontier/rules/research.json`
- Modify: `games/frontier/scripts/economy.js` (recipe `requires` field)
- Modify: `games/frontier/rules/affordability.json`
- Modify: `games/frontier/rules/ui.json` (research mode)
- Modify: `games/frontier/scenes/main.json` (tech panel UI)
- Modify: `games/frontier/scripts/poi.js` (TechPoint rewards)

**Interfaces:**
- Produces: `Research` component; `TechPoint` resource; `research` mode; 12-node tech tree (4 branches × 3 tiers); recipe `requires` field checked by affordability.

- [ ] **Step 1: Add Research/TechPoint to schema.json + extend Mode/Inventory**

```json
"Research": {
  "fields": {
    "known": { "type": "text", "default": "[]" },
    "current": { "type": "text", "default": "" },
    "progress": { "type": "number", "default": 0 },
    "cost_total": { "type": "int", "default": 0 }
  }
},
"TechPoint": {
  "fields": {
    "value": { "type": "int", "default": 0 }
  }
}
```

Extend Mode enum: `["build", "craft", "interact", "upgrade", "research", "combat", "trade"]`.
Extend Inventory with `hide`, `crystal_core` fields.

- [ ] **Step 2: Define tech tree data + research system in research.js**

Full 12-node tree as defined in spec §4.3 (survival/agriculture/exploration/industry × 3 tiers). Implement `research-progress` system (advances current research), `start_research` fn (validates prerequisites, deducts TechPoints).

- [ ] **Step 3: Add research rules (unlock recipes/regions on researched event)**

Create `games/frontier/rules/research.json` with handlers for `researched` event calling `unlock_region` / `unlock_recipes` fns.

- [ ] **Step 4: Add `requires` field to recipes in economy.js**

Extend BUILD table with `requires` field on tier-2/3 recipes (well2, recycler, dome, hydroponics, arc_gun, turret, etc.).

- [ ] **Step 5: Extend affordability rule to check `requires`**

In `games/frontier/scripts/economy.js`, implement `update_affordability_with_requires` fn that checks both cost AND tech prerequisites.

- [ ] **Step 6: Add research mode + tech panel UI**

Add tech panel entity with 12 buttons (4×3 grid) to scenes/main.json. Add `T` key binding in ui.json.

- [ ] **Step 7: Add TechPoint earning in poi.js**

Extend `interact_poi` to award TechPoints on POI exploration (+2 per POI).

- [ ] **Step 8: Run schema check + write tech tree test**

Run: `cargo run --release -- check games/frontier`
Test: `cd games/frontier && python tools/test_progression.py test_tech_tree_unlock_flow`

- [ ] **Step 9: Commit**

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json games/frontier/scripts/research.js games/frontier/scripts/economy.js games/frontier/scripts/poi.js games/frontier/rules/research.json games/frontier/rules/affordability.json games/frontier/rules/ui.json games/frontier/tools/test_progression.py
git commit -m "feat(frontier): tech tree with 12 nodes across 4 branches"
git push origin main
```

---

### Task 9: Companions Expansion

**Files:**
- Modify: `games/frontier/schema.json` (add Persona.role)
- Modify: `games/frontier/scripts/companion.js` (DRIFTER_POOL 6→12, role-driven contribution)
- Modify: `games/frontier/scripts/wish.js` (6 templates + collective wish)
- Modify: `games/frontier/rules/companion.json` (spawn cadence, cap 4→8)
- Modify: `games/frontier/scenes/main.json`

**Interfaces:**
- Produces: `Persona.role` enum (6 roles); 12-entry DRIFTER_POOL; role-driven contribution; 6 wish templates + collective wish.

- [ ] **Step 1: Add role field to Persona schema**

```json
"role": { "type": "enum", "variants": ["builder", "farmer", "explorer", "guard", "trader", "scholar"], "default": "builder" }
```

- [ ] **Step 2: Expand DRIFTER_POOL in companion.js**

12 entries, 2 per role, as defined in spec §4.4. Bump `drifters_spawned` cap from 4 to 8 in `drifter-cadence` rule.

- [ ] **Step 3: Implement role-driven contribution**

Extend `companion-contribution` system with switch on `Persona.role`: builder emits build-bonus, farmer emits crop-tend, explorer emits explore-bonus, guard emits guard-patrol (full combat integration in Task 10), trader emits trade-available, scholar generates TechPoints.

- [ ] **Step 4: Expand wish templates to 6 + collective wish**

In `games/frontier/scripts/wish.js`, add `WISH_TEMPLATES` per role (3 wishes each) + `COLLECTIVE_WISH` (colony-level granary-50 goal).

- [ ] **Step 5: Update spawn cadence rule**

In `games/frontier/rules/companion.json`, change `drifters_spawned < 4` to `drifters_spawned < 8`, and modulate cadence by stage.

- [ ] **Step 6: Run schema check + tests + gate**

Run: `cargo run --release -- check games/frontier && cd games/frontier && python tools/test_progression.py && cd ../.. && cargo run --release -- gate games/frontier`

- [ ] **Step 7: Commit**

```bash
git add games/frontier/schema.json games/frontier/scripts/companion.js games/frontier/scripts/wish.js games/frontier/rules/companion.json games/frontier/scenes/main.json
git commit -m "feat(frontier): companions expansion — 6 roles, 12 pool, wish templates"
git push origin main
```

---

## Phase 3: Dependent Game Systems (Tasks 10-13)

### Task 10: Combat System

**Files:**
- Modify: `games/frontier/schema.json` (add Hp, Enemy, Weapon, Guard; extend Mode)
- Modify: `games/frontier/scenes/main.json` (Hp/Weapon on player)
- Create: `games/frontier/scripts/combat.js`
- Create: `games/frontier/rules/combat.json`
- Modify: `games/frontier/scripts/companion.js` (guard auto-defense)
- Modify: `games/frontier/rules/ui.json` (combat mode)
- Add: `games/frontier/assets/enemy.png` (placeholder from rock.png)

**Interfaces:**
- Produces: `Hp` on player/companion/enemy/structure; `Enemy`, `Weapon`, `Guard` components; `combat` mode; enemy spawn on `night-fall{threat}`; damage resolution; structure downgrade; player respawn.

- [ ] **Step 1: Add combat components to schema.json**

```json
"Hp": {
  "fields": {
    "value": { "type": "number", "default": 100 },
    "max": { "type": "number", "default": 100 }
  }
},
"Enemy": {
  "fields": {
    "kind": { "type": "text", "default": "gnawer" },
    "damage": { "type": "number", "default": 5 },
    "aggro_range": { "type": "number", "default": 8 },
    "home_region": { "type": "text", "default": "wild" }
  }
},
"Weapon": {
  "fields": {
    "kind": { "type": "text", "default": "stone_axe" },
    "damage": { "type": "number", "default": 10 },
    "range": { "type": "number", "default": 2 },
    "cooldown": { "type": "number", "default": 1 },
    "_cd_t": { "type": "number", "default": 0 }
  }
},
"Guard": {
  "fields": {
    "post_x": { "type": "number", "default": 0 },
    "post_y": { "type": "number", "default": 0 },
    "patrol_r": { "type": "number", "default": 5 }
  }
}
```

Add `combat` to Mode enum.

- [ ] **Step 2: Add Hp/Weapon to player entity in scenes/main.json**

- [ ] **Step 3: Implement enemy spawn system in combat.js**

`spawn_wave(threat, region_count, day)` fn: wave size = min(8, threat × (1 + region_count × 0.3)). Enemy types: gnawer (day 1+), raider (day 5+, 30% chance), sandbeast (desert only, handled in Task 13).

- [ ] **Step 4: Implement enemy AI system**

`enemy-ai` system: straight-line path to player when in aggro_range, attack when in range 1.5.

- [ ] **Step 5: Implement player combat mode**

`player-combat` system: when Mode=combat, on attack input swing weapon, apply damage to enemies in range, drop loot on kill.

- [ ] **Step 6: Implement structure damage + downgrade**

`enemy-attack-structures` system: enemies attack adjacent structures, Hp=0 → downgrade tier (or despawn if tier 1).

- [ ] **Step 7: Implement player respawn**

`player-respawn-check` system: on Hp=0, teleport to lander, restore Hp, -20% food.

- [ ] **Step 8: Add night-fall → spawn_wave rule + combat mode UI**

In `rules/combat.json`: `on night-fall call spawn_wave`. In `rules/ui.json`: `F` key → combat mode.

- [ ] **Step 9: Add companion guard auto-defense**

In `companion.js`, extend `companion-contribution` guard case to attack nearest enemy in range.

- [ ] **Step 10: Add enemy.png placeholder**

Copy `rock.png` to `enemy.png` (real art out of scope).

- [ ] **Step 11: Run schema check + tests + gate**

Run: `cargo run --release -- check games/frontier && cd games/frontier && python tools/test_progression.py && cd ../.. && cargo run --release -- gate games/frontier`

- [ ] **Step 12: Commit**

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json games/frontier/scripts/combat.js games/frontier/scripts/companion.js games/frontier/rules/combat.json games/frontier/rules/ui.json games/frontier/assets/enemy.png games/frontier/tools/test_progression.py
git commit -m "feat(frontier): combat system

Add Hp/Enemy/Weapon/Guard components, combat mode. Enemies spawn on
night-fall. Straight-line AI. Structure damage causes tier downgrade.
Player respawn on death with -20% food penalty. Guard role auto-defends."
git push origin main
```

---

### Task 11: Trading & Diplomacy

**Files:**
- Modify: `games/frontier/schema.json` (add Faction)
- Create: `games/frontier/scripts/faction.js`
- Create: `games/frontier/rules/faction.json`
- Create: `games/frontier/rules/trade.json`
- Modify: `games/frontier/scenes/main.json` (trade menu UI)
- Modify: `games/frontier/rules/ui.json` (trade mode)

**Interfaces:**
- Produces: `Faction` component; 3 factions; relation tracking; tier derivation; trade menu; barter logic; LLM negotiation with fallback.

- [ ] **Step 1: Add Faction component to schema.json**

```json
"Faction": {
  "fields": {
    "relations": { "type": "text", "default": "{\"nomads\":30,\"caravan\":0,\"remnant\":-10}" },
    "tier_nomads": { "type": "text", "default": "neutral" },
    "tier_caravan": { "type": "text", "default": "wary" },
    "tier_remnant": { "type": "text", "default": "wary" }
  }
}
```

Add `trade` to Mode enum.

- [ ] **Step 2: Implement faction system in faction.js**

`faction-tick` system (derives tier from relation). `change_relation(faction, delta)` fn. `complete_trade(faction, give, receive)` fn with rate_mult per tier. `negotiate(faction, topic)` fn using `ctx.ask("llm")` with deterministic fallback.

- [ ] **Step 3: Add faction rules (relation thresholds unlock regions)**

In `rules/faction.json`: on relation-change, if caravan ≥ 76 → unlock desert region.

- [ ] **Step 4: Add trade menu UI**

Add `trade_menu` entity to scenes/main.json. Add `B` key binding in ui.json.

- [ ] **Step 5: Run schema check + tests + gate**

- [ ] **Step 6: Commit**

```bash
git add games/frontier/schema.json games/frontier/scripts/faction.js games/frontier/rules/faction.json games/frontier/rules/trade.json games/frontier/rules/ui.json games/frontier/scenes/main.json
git commit -m "feat(frontier): trading & diplomacy

Add Faction component with 3 factions. Relation tracking drives tier
(hostile→allied). Trade menu with barter rates. LLM negotiation with
deterministic fallback. Allied unlocks regions."
git push origin main
```

---

### Task 12: Map Expansion & Region Content

**Files:**
- Modify: `games/frontier/scripts/world.js`
- Create: `games/frontier/scripts/region.js`
- Create: `games/frontier/rules/region.json`
- Modify: `games/frontier/scenes/main.json` (region markers, expanded wild)
- Modify: `crates/vitric-cli/src/runtime.rs` (camera world_bounds clamping)
- Modify: `games/frontier/schema.json` (Camera.world_bounds field)

**Interfaces:**
- Produces: 5 regions; region content generator using E3 substreams; region unlock rules; camera world_bounds clamping.

- [ ] **Step 1: Define region specs + content specs in region.js**

5 regions (home/wild/mountain/swamp/desert) with coords + sizes. Per-region content specs (tiles, nodes, POIs) as defined in spec §4.6.

- [ ] **Step 2: Implement region content generator**

`generate_region_content(region_id)` fn using `ctx.random_stream("region:<id>")` for deterministic tile/node/POI placement.

- [ ] **Step 3: Implement region unlock rules**

In `rules/region.json`: on player position at region boundary + unlock condition met → call `thaw_region` + `generate_region_content`. Conditions: mountain needs `exploration_1` tech; swamp needs explorer-role companion in party; desert needs caravan relation ≥ neutral AND `industry_3` tech.

- [ ] **Step 4: Add region marker entities to scenes/main.json**

3 dormant region markers (mountain/swamp/desert) with full Region component.

- [ ] **Step 5: Implement Camera world_bounds clamping**

Add `world_bounds` field to Camera schema. In `crates/vitric-cli/src/runtime.rs` motion integration, clamp player position to discovered region bounds.

- [ ] **Step 6: Run schema check + region generation determinism test**

Test: same world_seed, different thaw timing → identical region content (positions of nodes/POIs).

- [ ] **Step 7: Commit**

```bash
git add games/frontier/scripts/world.js games/frontier/scripts/region.js games/frontier/rules/region.json games/frontier/scenes/main.json games/frontier/schema.json crates/vitric-cli/src/runtime.rs games/frontier/tools/test_progression.py
git commit -m "feat(frontier): map expansion with 5 regions

5 regions (home/wild/mountain/swamp/desert), 3 dormant with unlock
conditions. Region content generated on thaw using E3 seeded substreams.
Camera world_bounds clamps player to discovered regions."
git push origin main
```

---

### Task 13: Region Content Polish

**Files:**
- Modify: `games/frontier/scripts/region.js` (expanded POI tables)
- Modify: `games/frontier/scripts/poi.js` (per-type handlers)
- Modify: `games/frontier/scripts/combat.js` (biome enemies)

**Interfaces:**
- Produces: Per-region POI types with unique rewards; biome-specific enemy spawning (sandbeast in desert).

- [ ] **Step 1: Expand REGION_CONTENT with full POI tables**

Add ancient-ruins, crystal-cave, tomb, oasis, caravan-stop POI types with unique rewards (tech_point, crystal_core, water, seed, trade).

- [ ] **Step 2: Add per-type POI handlers in poi.js**

`POI_HANDLERS` dict mapping kind → handler function. Extend `interact_poi` to dispatch by kind.

- [ ] **Step 3: Add biome-specific enemy spawning**

`desert-spawn` system: if desert active and player in desert, spawn sandbeast every 2 in-game hours using `random_stream("desert_spawn")`.

- [ ] **Step 4: Run schema check + tests**

- [ ] **Step 5: Commit**

```bash
git add games/frontier/scripts/region.js games/frontier/scripts/poi.js games/frontier/scripts/combat.js
git commit -m "feat(frontier): region content polish — POI tables, biome enemies"
git push origin main
```

---

## Phase 4: Polish (Tasks 14-16)

### Task 14: Pacing Rebalance

**Files:**
- Modify: `games/frontier/scripts/clock.js` (DAY_SEC 60→90)
- Modify: `games/frontier/scripts/colony.js` (stage thresholds)
- Modify: `games/frontier/rules/quest.json` (quest thresholds)
- Modify: `games/frontier/GDD.md`

**Interfaces:**
- Produces: 90s/day; compound stage thresholds aligned with seasons/years; sandbox mode after 兴旺.

- [ ] **Step 1: Update DAY_SEC to 90.0**

- [ ] **Step 2: Update stage thresholds in colony.js**

Implement `stage-check` system with compound conditions: 起步 (day 1-3), 立足 (spring end day 12 + survival_1 + struct≥5), 成形 (summer end day 24 + pop≥3 + agriculture_1), 成群 (year 1 end day 48 + pop≥5 + any faction neutral), 兴旺 (year 2 end day 96 + monument + any faction allied + all T2 techs). Emit `settlement-founded` on transition to 兴旺.

- [ ] **Step 3: Update quest thresholds in quest.json**

Align quest step triggers with new stage thresholds.

- [ ] **Step 4: Update GDD.md pacing docs**

- [ ] **Step 5: Run schema check + progression test**

- [ ] **Step 6: Commit**

```bash
git add games/frontier/scripts/clock.js games/frontier/scripts/colony.js games/frontier/rules/quest.json games/frontier/GDD.md
git commit -m "feat(frontier): pacing rebalance for sandbox play

Day length 60s → 90s. Compound stage thresholds aligned with seasons.
settlement-founded fires at 兴旺 (year 2 end, day 96)."
git push origin main
```

---

### Task 15: Gate Recording

**Files:**
- Modify: `games/frontier/tools/record_clear.py`
- Modify: `games/frontier/qa/clear.json` (re-recorded)
- Modify: `games/frontier/vitric.json` (if needed)

**Interfaces:**
- Produces: New `qa/clear.json` that triggers `settlement-founded` at new pacing; `vitric gate games/frontier` passes.

- [ ] **Step 1: Add acceleration mode to record_clear.py**

4x speed for recording. Each in-game day compressed to ~22.5 real seconds.

- [ ] **Step 2: Extend record_clear.py to cover new systems**

Add actions for: research (T key + click tech node), combat (F key + attack during night), trade (B key + complete trade), region exploration (walk to mountain boundary after exploration_1).

- [ ] **Step 3: Generate new recording**

```bash
cargo run --release -- run games/frontier --record games/frontier/qa/clear.json &
sleep 2
python games/frontier/tools/record_clear.py
```

- [ ] **Step 4: Verify gate passes**

Run: `cargo run --release -- gate games/frontier`
Expected: PASS with `settlement-founded` event.

- [ ] **Step 5: Verify replay hash**

Run: `cargo run --release -- replay games/frontier/qa/clear.json`
Expected: hash OK at all checkpoints.

- [ ] **Step 6: Commit**

```bash
git add games/frontier/tools/record_clear.py games/frontier/qa/clear.json games/frontier/vitric.json
git commit -m "feat(frontier): re-record gate with sandbox pacing

New qa/clear.json covers full sandbox progression through 兴旺 stage.
4x acceleration mode. vitric gate passes with hash-identical replay."
git push origin main
```

---

### Task 16: README Upgrade

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Create: `docs/media/sandbox.gif`
- Create: `docs/media/sandbox-grid.png`

**Interfaces:**
- Produces: README with new hero GIF, Frontier featured section, engine capabilities matrix, updated status/roadmap.

- [ ] **Step 1: Capture new hero GIF + screenshot grid**

Run frontier, capture 60-second sandbox playthrough showing season change, combat, tech panel, region transition. Capture individual screenshots and composite into grid.

- [ ] **Step 2: Add Frontier featured section to README**

Insert between Quick Start and Agent API sections. 3-paragraph description + screenshot grid + list of 12 systems.

- [ ] **Step 3: Replace hero GIF**

Change `![glow demo](docs/media/glow.gif)` to `![sandbox demo](docs/media/sandbox.gif)`. Move glow.gif to Features section.

- [ ] **Step 4: Add engine capabilities matrix**

Table mapping each frontier feature to the engine capability it exercises (Region dormant/active → catch_up on thaw, etc.).

- [ ] **Step 5: Update Status + Roadmap**

Mention sandbox completion, 12+ systems, 2-hour playthrough. Mark Cookbook as partially addressed by frontier.

- [ ] **Step 6: Mirror changes to README.zh-CN.md**

- [ ] **Step 7: Commit**

```bash
git add README.md README.zh-CN.md docs/media/sandbox.gif docs/media/sandbox-grid.png
git commit -m "docs: README upgrade with sandbox showcase

New hero GIF, Frontier featured section, engine capabilities matrix.
Updated Status and Roadmap. Bilingual (EN + zh-CN)."
git push origin main
```

---

## Self-Review Notes

**Spec coverage:** All 5 engine capabilities (E1-E5) covered in Tasks 1-5. All 6 game systems covered: seasons/weather (T6+T7), combat (T10), tech tree (T8), companions (T9), trading/diplomacy (T11), map expansion (T12+T13). Pacing rebalance (T14), gate recording (T15), README (T16). Open questions from spec §9 (TechPoint economy, faction deltas, enemy stats) are pinned with concrete numbers in respective tasks but marked as tunable during playtest.

**Type consistency:** `Region` component fields consistent across Task 1 (schema) and Task 12 (region specs). `Persona.role` enum consistent across Task 9. `Mode` enum extended consistently (research/combat/trade added in Tasks 8/10/11). `Inventory` extended with hide/crystal_core in Task 8, used in Task 10 (combat drops).

**Placeholder scan:** No TBD/TODO in task steps. Concrete code provided for engine tasks; game tasks provide key data structures and function signatures with implementation details following existing patterns.

**Scope check:** Plan is large (16 tasks) but appropriate for spec scope. Phased structure allows subagent-driven execution with natural review checkpoints between phases.
