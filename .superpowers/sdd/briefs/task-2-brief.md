# Task 2: E2 — Catch_up system API

**Files:**
- Modify: `crates/vitric-script/src/lib.rs`
- Modify: `crates/vitric-sim/src/sim.rs`
- Modify: `games/frontier/scripts/crops.js` (declare catch_up)
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `vitric.system(name, opts, fn, catch_up_fn?)` — 4th optional arg; `Sim::invoke_catch_up_for_region(id)` calls each system's catch_up for entities in that region.
- Consumes: Task 1's `thaw_region` stub (`invoke_catch_up_for_region` is currently a no-op stub on `Sim`).

## Step 1: Write failing test for catch_up invocation

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

## Step 2: Run test to verify it fails

Run: `cargo test -p vitric-cli --test region catch_up_advances_dormant_crop_on_thaw`
Expected: FAIL.

## Step 3: Extend vitric.system API to accept catch_up

In `crates/vitric-script/src/prelude.js`, modify `vitric.system` to accept an optional 4th argument (the catch_up function). Store it in the system object as `catch_up` (null if not provided). Also extend `__list` to include `catch_up: true/false` per system.

Add a new global `__runCatchUp(idx, entityHandle, dormantTicks, payloadJson)` that invokes the catch_up fn of system `idx` for a single entity.

In `crates/vitric-script/src/lib.rs`, extend `SystemDecl` with `has_catch_up: bool`. Add a `run_catch_up(&mut self, system_idx, entity, dormant_ticks, world, rng, tick)` method that calls `__runCatchUp` and applies ops.

## Step 4: Implement invoke_catch_up_for_region in sim.rs

In `crates/vitric-sim/src/sim.rs`, replace the stub:

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

## Step 5: Add catch_up to frontier's crop-grow system

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

## Step 6: Run catch_up test to verify it passes

Run: `cargo test -p vitric-cli --test region catch_up_advances_dormant_crop_on_thaw`
Expected: PASS.

## Step 7: Run full test suite for regressions

Run: `cargo test --workspace && cargo run --release -- check games/frontier`
Expected: all pass.

## Step 8: Commit

```bash
git add crates/vitric-script/src/lib.rs crates/vitric-script/src/prelude.js crates/vitric-sim/src/sim.rs crates/vitric-cli/tests/region.rs games/frontier/scripts/crops.js
git commit -m "feat(script,sim): E2 catch_up system API

Systems can declare optional catch_up(entity, ctx, dormant_ticks).
On region thaw, engine invokes catch_up for each entity in the region.
crop-grow declares catch_up as reference implementation."
git push origin main
```
