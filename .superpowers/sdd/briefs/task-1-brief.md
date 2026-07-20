# Task 1: E1 — Region dormant/active

**Files:**
- Modify: `crates/vitric-ecs/src/world.rs`
- Modify: `crates/vitric-sim/src/sim.rs`
- Modify: `crates/vitric-render/src/lib.rs`
- Modify: `games/frontier/schema.json` (add Region component)
- Test: `crates/vitric-cli/tests/region.rs` (new)

**Interfaces:**
- Produces: `world::query()` filters entities with dormant `Region` component; `Sim::thaw_region(id)` transitions state + emits `region-thaw{id}` event; renderer skips dormant entities.
- Consumes: none (foundation task).

## Step 1: Write failing test for dormant query filtering

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

## Step 2: Run test to verify it fails

Run: `cargo test -p vitric-cli --test region dormant_entities_excluded_from_query`
Expected: FAIL — query returns both entities (no Region filtering yet).

## Step 3: Add Region component to frontier schema.json

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

## Step 4: Implement dormant filtering in world.query

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

## Step 5: Run test to verify it passes

Run: `cargo test -p vitric-cli --test region dormant_entities_excluded_from_query`
Expected: PASS.

## Step 6: Write failing test for render skip

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

## Step 7: Run render test to verify it fails

Run: `cargo test -p vitric-cli --test region dormant_entities_skipped_in_render`
Expected: FAIL.

## Step 8: Implement render skip in render_world and describe_world

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

## Step 9: Run render test to verify it passes

Run: `cargo test -p vitric-cli --test region dormant_entities_skipped_in_render`
Expected: PASS.

## Step 10: Write failing test for sim skip + thaw

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

## Step 11: Run sim tests to verify they fail

Run: `cargo test -p vitric-cli --test region --`
Expected: FAIL — `thaw_region` method doesn't exist.

## Step 12: Implement sim skip + thaw_region

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

## Step 13: Run sim tests to verify they pass

Run: `cargo test -p vitric-cli --test region --`
Expected: PASS.

## Step 14: Run full engine test suite for regressions

Run: `cargo test --workspace`
Expected: all existing tests pass.

## Step 15: Commit

```bash
git add crates/vitric-ecs/src/world.rs crates/vitric-sim/src/sim.rs crates/vitric-render/src/lib.rs crates/vitric-cli/tests/region.rs games/frontier/schema.json
git commit -m "feat(ecs,sim,render): E1 Region dormant/active

Add Region component with dormant/active/frozen states. world.query,
render_world, and sim logic dispatch all skip dormant entities.
state_hash still covers dormant entities. Sim::thaw_region() transitions
state and emits region-thaw event."
git push origin main
```
