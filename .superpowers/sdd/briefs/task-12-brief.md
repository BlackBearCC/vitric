# Task 12 — Map Expansion & Region Content

> Spec §4.6 (lines 296-320), Plan §Task 12 (lines 1310-1357).
> Predecessor: Task 11 (Faction). Successor: Task 13 (Region Content Polish).

## 0. Goal

Expand the world from a single home+wild pocket to 5 regions (home / wild / mountain / swamp / desert). Three dormant regions unlock via gameplay conditions. Region content (tiles, resource nodes, POIs) is generated on thaw using E3 seeded substreams — bit-identical regardless of when the thaw happens. Camera gains `world_bounds` clamping so the player can't walk into undiscovered void.

## 1. Plan corrections (5)

The plan §Task 12 has 5 inaccuracies discovered during brief research. The implementer MUST follow the brief, not the plan.

### Correction 1: Camera clamping is in `sim.rs`, NOT `runtime.rs`

**Plan says**: "Modify: `crates/vitric-cli/src/runtime.rs` (camera world_bounds clamping)".

**Reality**: `runtime.rs` has zero camera/motion/clamp code. Camera follow lives in `crates/vitric-sim/src/sim.rs:757` (`follow_camera` fn). Motion integration lives in `sim.rs:642` (`integrate_motion` fn). The step order (`sim.rs:370-377`) is: `apply_gravity` → `integrate_motion` → `follow_camera` → … .

**Brief directs**: Add the world_bounds clamping at the END of `integrate_motion` in `sim.rs` (after all entities have moved, before `follow_camera` runs). Do NOT touch `runtime.rs`.

### Correction 2: `ctx.thaw_region` bridge is missing (pre-existing bug from Task 8)

**Plan assumes**: Region unlock rules can call `unlock_region` fn which calls `ctx.thaw_region`.

**Reality**: `ctx.thaw_region` is NOT exposed in the prelude (`crates/vitric-script/src/prelude.js`). The `ctx` object only has: `dt`, `tick`, `random`, `random_stream`, `emit`, `spawn`, `despawn`, `setField`, `getField`, `ask`. The `unlock_region` fn in `research.js:101-105` calls `ctx.thaw_region(a.region_id)` — this throws `TypeError: ctx.thaw_region is not a function` at runtime. The `research-unlock-mountain` rule (which fires when `exploration_t1` completes) would crash the game. No existing test exercises this path, so the bug went unnoticed.

**Brief directs**: Add a `ctx.thaw_region` native bridge (mirrors the `ctx.random_stream` pattern). Engine changes:
- `crates/vitric-script/src/lib.rs`: register native `__thawRegion(id: String)` that calls `vitric_sim::with_sim_ptr(|sim| sim.thaw_region(&id))`.
- `crates/vitric-script/src/prelude.js`: add `thaw_region: (id) => { … __thawRegion(id) }` to the `ctx` object.

### Correction 3: Wild region marker doesn't exist yet

**Plan says**: "3 dormant region markers (mountain/swamp/desert)".

**Reality**: Only `mountain` exists in `scenes/main.json`. There is no `home`, `wild`, `swamp`, or `desert` region marker entity. The spec lists 5 regions; 2 are active from start (home, wild), 3 are dormant (mountain, swamp, desert).

**Brief directs**: Add 4 region marker entities to `scenes/main.json` (home active, wild active, swamp dormant, desert dormant). Mountain already exists — keep it. Total 5 region markers.

### Correction 4: Wild terrain is smaller than spec

**Plan says**: "expanded wild".

**Reality**: Current `genWild` fn (`world.js:6-32`) spawns wild terrain at x16..27, y0..11 (12×12 area). Spec §4.6 says wild is (28,0)-(60,30) = 32×30. The current wild is a transition zone inside the home region's spec bounds (home is 0,0, 28×12).

**Brief directs**: Keep the existing x16..27 wild terrain as a home-region transition zone. Extend `genWild` to also spawn wild terrain at x28..59, y0..29 (matching spec's wild region). Add 4 more resource nodes in the expanded area. The `wild` region marker (added to scene) covers (28,0)-(60,30).

### Correction 5: Desert unlock has 2 conditions, not 1

**Plan says**: "desert needs caravan relation ≥ neutral AND `industry_3` tech".

**Reality**: The existing `research-unlock-desert` rule (`rules/research.json:10-14`) fires on `researched{ id: "industry_t3" }` and calls `unlock_region("desert")`. It does NOT check caravan relation. This means the desert would unlock on tech completion alone, ignoring the faction condition.

**Brief directs**: Remove the `research-unlock-desert` rule from `rules/research.json` (the `region-approach-check` system in `region.js` will handle desert unlock with BOTH conditions). The `research-unlock-mountain` rule stays (mountain unlock is purely tech-gated, no extra condition — the system approach also works but the existing rule is harmless and tested implicitly). Actually, for CONSISTENCY: remove BOTH `research-unlock-mountain` AND `research-unlock-desert` from `research.json`, and let the `region-approach-check` system handle all 3 dormant regions uniformly. This avoids double-unlock attempts (rule + system both calling `thaw_region` for the same region).

Wait — `thaw_region` is idempotent on already-active regions (sets state to "active" again, still emits `region-thaw` event). So double-unlock is harmless but wasteful (re-generates content). To avoid this, the `region-approach-check` system checks `Region.state == "dormant"` before attempting unlock. So even if both the rule and the system fire, only the first one thaws; the second is skipped (state is already "active"). But the `research-unlock-mountain` rule fires on `researched{ id: "exploration_t1" }` — this fires ONCE when tech completes. The system checks every tick. So the rule would fire first (immediately on tech completion), and the system would skip (already active). No double-unlock.

**Final decision**: Keep `research-unlock-mountain` (it works once the `ctx.thaw_region` bridge is added — it's the existing Task 8 wiring). Remove `research-unlock-desert` (incomplete condition). The `region-approach-check` system handles swamp + desert (both have extra conditions beyond tech). Mountain is handled by the existing rule (pure tech gate). This is the minimal-change approach.

## 2. Files to touch

### Engine (2 files)
- **MODIFY** `crates/vitric-script/src/lib.rs` — register `__thawRegion` native fn
- **MODIFY** `crates/vitric-script/src/prelude.js` — add `ctx.thaw_region`
- **MODIFY** `crates/vitric-sim/src/sim.rs` — camera world_bounds clamping in `integrate_motion`
- **MODIFY** `crates/vitric-sim/src/sim.rs` — (if needed) expose `thaw_region` for the bridge (already `pub fn thaw_region` at line 280 — no change needed)

### Game data (5 files)
- **MODIFY** `games/frontier/schema.json` — add `Camera.world_bounds` field
- **MODIFY** `games/frontier/scripts/world.js` — extend `genWild` to cover expanded wild
- **CREATE** `games/frontier/scripts/region.js` — region specs + content generator + approach checker + bounds updater
- **CREATE** `games/frontier/rules/region.json` — region-thaw rules (content gen + bounds update)
- **MODIFY** `games/frontier/rules/research.json` — remove `research-unlock-desert` (incomplete condition; replaced by region-approach-check system)
- **MODIFY** `games/frontier/scenes/main.json` — add 4 region markers (home/wild/swamp/desert) + Camera.world_bounds initial value
- **MODIFY** `games/frontier/vitric.json` — add `scripts/region.js` + `rules/region.json` to arrays

### Tests (1 file)
- **MODIFY** `crates/vitric-cli/tests/region.rs` — add 3 tests

## 3. Engine changes — DETAILED CODE

### 3.1 `ctx.thaw_region` bridge

#### `crates/vitric-script/src/lib.rs`

Find the existing `__randomStreamNext` registration (around line 167-176) and add `__thawRegion` right after it. The pattern is identical: a native fn that crosses into `Sim` via `with_sim_ptr`.

```rust
// Right after the __randomStreamNext registration block:
{
    // ctx.thaw_region bridge: JS calls ctx.thaw_region(id) → __thawRegion(id) →
    // with_sim_ptr(|sim| sim.thaw_region(id)). Mirrors __randomStreamNext's SIM_PTR pattern.
    // Panics if SIM_PTR is null (ctx.thaw_region called outside a Sim::step window).
    let f_thaw = Function::new(ctx.clone(), |id: String| {
        vitric_sim::with_sim_ptr(|sim| {
            sim.thaw_region(&id);
        });
    });
    ctx.globals().set("__thawRegion", f_thaw).map_err(make_err)?;
}
```

**Note**: `vitric_sim::with_sim_ptr` is already `pub` (sim.rs:51). `Sim::thaw_region` is already `pub` (sim.rs:280). No new `pub` changes needed in sim.rs for this bridge.

The `Function::new` callback returns `()` — `thaw_region` has no return value. The JS side doesn't need a return value either (the thaw is queued, the effect manifests next tick).

#### `crates/vitric-script/src/prelude.js`

In the `__makeCtx` function's returned object, add `thaw_region` after `ask` (or after `random_stream` — placement doesn't matter, but grouping with `random_stream` makes sense as both are Sim bridges). Add this inside the ctx object (around line 197, after the `ask` property):

```javascript
// Thaw a dormant region: transitions Region.state → "active", sets discovered=1, queues
// region-thaw event for the next step. Same host-API semantics as Sim::thaw_region (Rust).
// Called by the region-approach-check system and by unlock_region fn (research.js).
// Idempotent on already-active regions (re-sets state, re-emits event — rules decide whether
// to dedupe based on discovered flag).
thaw_region: (id) => {
  if (typeof id !== "string" || !id) throw new Error("ctx.thaw_region: id 必须是非空字符串");
  __thawRegion(id);
},
```

### 3.2 Camera world_bounds clamping

#### `games/frontier/schema.json` — add `world_bounds` to Camera

Find the `Camera` component (around line 225-250) and add the `world_bounds` field:

```json
"world_bounds": {
  "type": "text",
  "default": ""
}
```

Format: JSON string `"[min_x,min_y,max_x,max_y]"`. Empty string = no clamping (default, backward-compatible with existing scenes that don't set it).

#### `crates/vitric-sim/src/sim.rs` — clamp in `integrate_motion`

At the END of `integrate_motion` (after the main `for id in self.world.query(&["Position", "Velocity"])` loop, right before `Ok(())`), add the camera bounds clamping block. This runs AFTER all entities have moved, BEFORE `follow_camera` (so the camera follows the clamped position).

```rust
// Camera world_bounds clamping: if a Camera has a non-empty world_bounds (JSON "[min_x,min_y,max_x,max_y]")
// and names a follow target, clamp that target's Position to the bounds and zero the clamped axis velocity.
// Only the follow target is clamped (other entities — enemies, particles, NPCs — roam freely).
// Empty world_bounds = no clamping (backward-compatible with scenes that don't set it).
for cam_id in self.world.query(&["Camera"]) {
    let bounds_str = match self.world.get_field(cam_id, "Camera.world_bounds") {
        Ok(v) => match v.as_str() { Some(s) if !s.is_empty() => s, _ => continue },
        Err(_) => continue,
    };
    let bounds: Vec<f64> = match serde_json::from_str::<Vec<f64>>(bounds_str) {
        Ok(b) if b.len() == 4 => b,
        _ => continue, // Malformed bounds — silently skip (defensive; schema doesn't validate JSON shape)
    };
    let (min_x, min_y, max_x, max_y) = (bounds[0], bounds[1], bounds[2], bounds[3]);
    let follow = match self.world.get_field(cam_id, "Camera.follow") {
        Ok(v) => match v.as_str() { Some(s) if !s.is_empty() => s.to_string(), _ => continue },
        Err(_) => continue,
    };
    let target = match self.world.entity(&follow) {
        Ok(id) => id,
        Err(_) => continue, // Follow target doesn't exist — follow_camera will error on its own
    };
    let px = self.num_field(target, "Position", "x")?;
    let py = self.num_field(target, "Position", "y")?;
    let nx = px.clamp(min_x, max_x);
    let ny = py.clamp(min_y, max_y);
    if nx != px {
        self.world.set_field(target, "Position.x", json!(nx))
            .expect("字段刚读过必然存在");
        if let Ok(vx) = self.num_field(target, "Velocity", "x") {
            if vx != 0.0 {
                self.world.set_field(target, "Velocity.x", json!(0.0))
                    .expect("字段刚读过必然存在");
            }
        }
    }
    if ny != py {
        self.world.set_field(target, "Position.y", json!(ny))
            .expect("字段刚读过必然存在");
        if let Ok(vy) = self.num_field(target, "Velocity", "y") {
            if vy != 0.0 {
                self.world.set_field(target, "Velocity.y", json!(0.0))
                    .expect("字段刚读过必然存在");
            }
        }
    }
}
```

**Key design points**:
- Only the camera's `follow` target is clamped (player). Other entities roam freely.
- Velocity on the clamped axis is zeroed (prevents "pushing" against the boundary).
- Malformed `world_bounds` (not JSON, wrong length) is silently skipped — defensive, doesn't crash.
- `num_field` errors propagate as `SimError::BadComponent` (same as existing motion code).

## 4. Game data changes — DETAILED CODE

### 4.1 `games/frontier/scripts/region.js` (NEW)

```javascript
// Region specs: 5 regions matching spec §4.6 layout.
//   home   (0,0)-(28,12)    28×12   active   starting
//   wild   (28,0)-(60,30)   32×30   active   starting (extends current wild)
//   mountain (0,12)-(30,40)  30×28  dormant  Tech: exploration_t1
//   swamp  (28,12)-(60,40)  32×28   dormant  Party has explorer-role companion
//   desert (60,0)-(120,60)  60×60   dormant  Faction caravan relation ≥ neutral AND Tech: industry_t3
//
// Region content is generated on thaw using ctx.random_stream("region:<id>") — deterministic
// regardless of thaw timing (same world_seed → same substream → same positions). This is the
// replay-safety guarantee: a region thawed at tick 100 vs tick 1000 produces bit-identical content.
//
// Camera world_bounds: union of all active region rects. Updated on every region-thaw event.
// Engine's integrate_motion clamps the player (Camera.follow target) to these bounds.

const REGION_SPECS = {
  home:     { anchor_x: 0,  anchor_y: 0,  w: 28, h: 12, biome: "home",     state: "active"  },
  wild:     { anchor_x: 28, anchor_y: 0,  w: 32, h: 30, biome: "wild",     state: "active"  },
  mountain: { anchor_x: 0,  anchor_y: 12, w: 30, h: 28, biome: "mountain", state: "dormant" },
  swamp:    { anchor_x: 28, anchor_y: 12, w: 32, h: 28, biome: "swamp",    state: "dormant" },
  desert:   { anchor_x: 60, anchor_y: 0,  w: 60, h: 60, biome: "desert",   state: "dormant" },
};

// Per-region content config: tile color, resource node types/counts, POI types/counts.
// Task 13 will expand POI tables and add biome-specific enemies; Task 12 lays the framework.
const REGION_CONTENT = {
  mountain: {
    tile_color: "#3a3530",
    nodes: [
      { kind: "ore", count: 6, color: "#caa45a", label: "矿脉", left: 5 },
    ],
    pois: [
      { kind: "ancient-ruins", reward_table: '{"techpoint":[1,3]}', label: "古代遗迹" },
    ],
  },
  swamp: {
    tile_color: "#2a3a2a",
    nodes: [
      { kind: "fiber", count: 5, color: "#9aac5a", label: "纤维丛", left: 5 },
    ],
    pois: [
      { kind: "dangerous-flora", reward_table: '{"hide":[1,2]}', label: "危险植物" },
    ],
  },
  desert: {
    tile_color: "#7a6a3a",
    nodes: [
      { kind: "crystal_core", count: 2, color: "#5acaff", label: "晶核", left: 3 },
    ],
    pois: [
      { kind: "caravan", reward_table: '{}', label: "商队营地" },
      { kind: "tomb", reward_table: '{"crystal_core":[1,2],"techpoint":[2,4]}', label: "古墓" },
    ],
  },
};

// Generate region content on thaw. Called by rule on region-thaw event.
// Uses ctx.random_stream("region:<id>") for deterministic tile/node/POI placement.
// Args: { region_id }
vitric.fn("gen_region_content", (a, ctx) => {
  const id = a.region_id;
  const spec = REGION_SPECS[id];
  const content = REGION_CONTENT[id];
  if (!spec || !content) return;

  const stream = ctx.random_stream("region:" + id);

  // Spawn terrain tiles within the region bounds.
  for (let gx = spec.anchor_x; gx < spec.anchor_x + spec.w; gx++) {
    for (let gy = spec.anchor_y; gy < spec.anchor_y + spec.h; gy++) {
      ctx.spawn({
        Cell: { kind: spec.biome },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: content.tile_color },
        Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                  anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                  dormant_ticks: 0, spawn_timer: 0 },
      });
    }
  }

  // Spawn resource nodes at deterministic positions within region bounds.
  let nodeIdx = 0;
  for (const nodeSpec of content.nodes) {
    for (let i = 0; i < nodeSpec.count; i++) {
      const nx = spec.anchor_x + stream.nextInt(0, spec.w - 1);
      const ny = spec.anchor_y + stream.nextInt(0, spec.h - 1);
      ctx.spawn({
        Node: { kind: nodeSpec.kind, left: nodeSpec.left, max: nodeSpec.left, cooldown: 0 },
        Position: { x: nx, y: ny },
        Sprite: { w: 0.9, h: 0.9, image: "", color: nodeSpec.color },
        Text: { content: nodeSpec.label, size: 0.34, color: "#ffffff", screen: false },
        Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                  anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                  dormant_ticks: 0, spawn_timer: 0 },
      }, id + "_node_" + nodeIdx);
      nodeIdx++;
    }
  }

  // Spawn POIs at deterministic positions.
  let poiIdx = 0;
  for (const poiSpec of content.pois) {
    const px = spec.anchor_x + stream.nextInt(0, spec.w - 1);
    const py = spec.anchor_y + stream.nextInt(0, spec.h - 1);
    ctx.spawn({
      Poi: { kind: poiSpec.kind, state: "fresh", cooldown: 0, reward_table: poiSpec.reward_table },
      Position: { x: px, y: py },
      Sprite: { w: 1, h: 1, image: "", color: "#e8d878" },
      Text: { content: poiSpec.label, size: 0.34, color: "#ffffff", screen: false },
      Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                dormant_ticks: 0, spawn_timer: 0 },
    }, id + "_poi_" + poiIdx);
    poiIdx++;
  }

  ctx.emit("toast-show", { text: "区域生成: " + id });
});

// Update Camera.world_bounds to the union of all active region rects.
// Called by rule on region-thaw event (after gen_region_content).
// Args: {}
vitric.fn("update_camera_bounds", (a, ctx) => {
  // Read all region markers (entities named home/wild/mountain/swamp/desert with Region component).
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const id of Object.keys(REGION_SPECS)) {
    // Read the region marker entity's Region.state field.
    const state = ctx.getField(id, "Region.state");
    if (state !== "active") continue;
    const spec = REGION_SPECS[id];
    minX = Math.min(minX, spec.anchor_x);
    minY = Math.min(minY, spec.anchor_y);
    maxX = Math.max(maxX, spec.anchor_x + spec.w);
    maxY = Math.max(maxY, spec.anchor_y + spec.h);
  }
  if (minX === Infinity) return; // No active regions — leave bounds unchanged
  const bounds = JSON.stringify([minX, minY, maxX, maxY]);
  ctx.setField("camera", "Camera.world_bounds", bounds);
});

// Region approach checker: every tick, check if player is near a dormant region boundary
// AND that region's unlock condition is met. If so, thaw the region (ctx.thaw_region).
// Runs as a system (not a rule) because the condition logic is too complex for rule filters
// (especially the swamp's "party has explorer-role companion" check).
vitric.system("region-approach-check", { query: [], writes: [] }, (entities, ctx) => {
  // Read player position via ctx.getField (the system has no query entities; we read by name).
  const px = ctx.getField("player", "Position.x");
  const py = ctx.getField("player", "Position.y");
  if (typeof px !== "number" || typeof py !== "number") return;

  for (const id of Object.keys(REGION_SPECS)) {
    const spec = REGION_SPECS[id];
    if (spec.state !== "dormant") continue; // Only check dormant regions

    // Read the region marker's actual state (it may have been thawed already by a rule).
    const state = ctx.getField(id, "Region.state");
    if (state !== "dormant") continue;

    // Check if player is within 3 tiles of the region boundary.
    const nearX = px >= spec.anchor_x - 3 && px <= spec.anchor_x + spec.w + 3;
    const nearY = py >= spec.anchor_y - 3 && py <= spec.anchor_y + spec.h + 3;
    if (!nearX || !nearY) continue;

    // Check unlock condition.
    if (!checkUnlockCondition(id, ctx)) continue;

    // Unlock condition met + player nearby → thaw.
    ctx.thaw_region(id);
    ctx.emit("region-approach", { id: id });
  }
});

// Check unlock condition for a region.
//   mountain: exploration_t1 tech researched (checked via Colony.Research.has_exploration_t1)
//   swamp: party has explorer-role companion (checked via Colony.companion_handles + Persona.role)
//   desert: caravan relation ≥ neutral (Faction.tier_caravan in [neutral, friendly, allied])
//           AND industry_t3 tech researched
function checkUnlockCondition(id, ctx) {
  if (id === "mountain") {
    const has = ctx.getField("colony", "Research.has_exploration_t1");
    return has === 1;
  }
  if (id === "swamp") {
    // Read companion_handles list (JSON text array of entity handles) and check if any has
    // Persona.role === "explorer". companion_handles is a list-of-text field on Colony.
    const handlesJson = ctx.getField("colony", "Colony.companion_handles");
    if (!handlesJson) return false;
    let handles = [];
    try { handles = JSON.parse(handlesJson); } catch { return false; }
    if (!Array.isArray(handles)) return false;
    for (const h of handles) {
      const role = ctx.getField(h, "Persona.role");
      if (role === "explorer") return true;
    }
    return false;
  }
  if (id === "desert") {
    const tier = ctx.getField("colony", "Faction.tier_caravan");
    if (tier !== "neutral" && tier !== "friendly" && tier !== "allied") return false;
    const has = ctx.getField("colony", "Research.has_industry_t3");
    return has === 1;
  }
  return false;
}
```

**Key design notes**:
1. `REGION_SPECS` is a const at module scope. QuickJS shared global — must not collide with other scripts' consts. Names are unique (`REGION_SPECS`, `REGION_CONTENT`).
2. `gen_region_content` spawns entities with `Region` component carrying the region id. This tags them for the dormant filter (engine's `query` skips dormant entities; active entities are queried normally).
3. `gen_region_content` names POIs/nodes with `<region_id>_node_<i>` / `<region_id>_poi_<i>` — the test reads these by name.
4. `region-approach-check` system has `query: []` (no entity batch) — it reads everything via `ctx.getField`. `writes: []` — it only calls `ctx.thaw_region` and `ctx.emit` (both are deferred ops, not query-based writes).
5. `checkUnlockCondition` is a plain function (not a `vitric.fn` or system) — only called by the system. It reads `Colony.Research.has_*`, `Colony.companion_handles`, `Colony.Faction.tier_caravan`, and companion `Persona.role` via `ctx.getField`.
6. `Colony.companion_handles` is a `list` of `text` field (schema line 752-756). `ctx.getField` returns the JSON-serialized list — we parse it and iterate.

### 4.2 `games/frontier/rules/region.json` (NEW)

```json
{
  "rules": [
    {
      "id": "region-thaw-content",
      "comment": "On region-thaw: generate region content (tiles, nodes, POIs) using E3 substream.",
      "on": { "event": "region-thaw" },
      "do": [ { "call": "gen_region_content", "with": { "region_id": "event.id" } } ]
    },
    {
      "id": "region-thaw-bounds",
      "comment": "On region-thaw: update Camera.world_bounds to union of all active regions.",
      "on": { "event": "region-thaw" },
      "do": [ { "call": "update_camera_bounds", "with": {} } ]
    }
  ]
}
```

**Note**: Both rules fire on the same `region-thaw` event. Order doesn't matter (they write to different things: content vs camera bounds). The `region-thaw` event carries `{ id: <region_id> }` (set by `Sim::thaw_region` at sim.rs:280).

### 4.3 `games/frontier/scripts/world.js` — extend `genWild`

Modify the existing `genWild` fn to also spawn wild terrain in the expanded wild region (x28..59, y0..29). Keep the existing x16..27 terrain as a transition zone.

**Current genWild** (lines 6-32): spawns x16..27, y0..11 terrain + 6 resource nodes.

**Modified genWild**: extend the terrain loop to x28..59, y0..29, and add 4 more resource nodes in the expanded area.

```javascript
vitric.fn("genWild", (a, ctx) => {
  // Wild terrain: x16..59, y0..29 (home transition x16..27 + expanded wild x28..59).
  // x16 boundary slightly brighter (transition hint from home to wild).
  for (let gx = 16; gx <= 59; gx++) {
    for (let gy = 0; gy <= 29; gy++) {
      ctx.spawn({
        Cell: { kind: "wild" },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: gx === 16 ? "#5a5040" : "#48402f" },
      });
    }
  }
  // Resource nodes: original 6 + 4 new in expanded wild.
  const NODES = [
    ["ore", 19, 3, "矿脉", "#caa45a"], ["ore", 25, 9, "矿脉", "#caa45a"],
    ["wood", 22, 2, "林木", "#5f8f3a"], ["wood", 24, 10, "林木", "#5f8f3a"],
    ["fiber", 20, 7, "纤维丛", "#9aac5a"], ["fiber", 26, 5, "纤维丛", "#9aac5a"],
    // Expanded wild nodes (x28..59, y0..29):
    ["ore", 45, 18, "矿脉", "#caa45a"],
    ["wood", 38, 22, "林木", "#5f8f3a"],
    ["fiber", 52, 8, "纤维丛", "#9aac5a"],
    ["wood", 33, 15, "林木", "#5f8f3a"],
  ];
  for (const n of NODES) {
    ctx.spawn({
      Node: { kind: n[0], left: 5, max: 5, cooldown: 0 },
      Position: { x: n[1], y: n[2] },
      Sprite: { w: 0.9, h: 0.9, image: "", color: n[4] },
      Text: { content: n[3], size: 0.34, color: "#ffffff", screen: false },
    });
  }
});
```

### 4.4 `games/frontier/rules/research.json` — remove `research-unlock-desert`

Delete the `research-unlock-desert` rule (lines 10-14). The `region-approach-check` system in `region.js` handles desert unlock with BOTH conditions (caravan relation ≥ neutral AND industry_t3). Keep `research-unlock-mountain` (pure tech gate, already wired and works once the `ctx.thaw_region` bridge is added).

**Resulting `rules/research.json`**:
```json
{
  "rules": [
    {
      "id": "research-unlock-mountain",
      "comment": "On exploration_t1 complete: thaw mountain region (E1 API).",
      "on": { "event": "researched", "filter": { "id": "exploration_t1" } },
      "do": [ { "call": "unlock_region", "with": { "region_id": "mountain" } } ]
    },
    {
      "id": "tp-apply",
      "comment": "Apply TechPoint write-back from interact_poi / start_research (emit tp-set {value} -> @player.TechPoint.value).",
      "on": { "event": "tp-set" },
      "do": [ { "set": "@player.TechPoint.value", "to": "event.value" } ]
    }
  ]
}
```

### 4.5 `games/frontier/scenes/main.json` — region markers + camera bounds

Add 4 region marker entities (home active, wild active, swamp dormant, desert dormant). Mountain already exists. Add `world_bounds` to the camera entity.

Use Python to edit `scenes/main.json` (single-line JSON; the Edit tool fails on it). The implementer should write a Python script that loads the JSON, adds the entities, and dumps it back with `separators=(",",":")`, `ensure_ascii=False`.

**Region marker entities to add**:

```python
# home region marker (active)
{"name": "home", "components": {"Region": {"id": "home", "biome": "home", "state": "active", "discovered": 1, "anchor_x": 0, "anchor_y": 0, "w": 28, "h": 12, "dormant_ticks": 0, "spawn_timer": 0}}}

# wild region marker (active)
{"name": "wild", "components": {"Region": {"id": "wild", "biome": "wild", "state": "active", "discovered": 1, "anchor_x": 28, "anchor_y": 0, "w": 32, "h": 30, "dormant_ticks": 0, "spawn_timer": 0}}}

# swamp region marker (dormant)
{"name": "swamp", "components": {"Region": {"id": "swamp", "biome": "swamp", "state": "dormant", "discovered": 0, "anchor_x": 28, "anchor_y": 12, "w": 32, "h": 28, "dormant_ticks": 0, "spawn_timer": 7200}}}

# desert region marker (dormant)
{"name": "desert", "components": {"Region": {"id": "desert", "biome": "desert", "state": "dormant", "discovered": 0, "anchor_x": 60, "anchor_y": 0, "w": 60, "h": 60, "dormant_ticks": 0, "spawn_timer": 7200}}}
```

**Camera entity update**: add `world_bounds: "[0,0,60,30]"` to the existing camera entity's Camera component. This is the initial bounds (home ∪ wild = (0,0)-(60,30)). As regions thaw, `update_camera_bounds` expands it.

```python
# In the camera entity's Camera component, add:
camera_entity["components"]["Camera"]["world_bounds"] = "[0,0,60,30]"
```

### 4.6 `games/frontier/vitric.json` — register new files

Add `scripts/region.js` to the `scripts` array (after `scripts/faction.js`). Add `rules/region.json` to the `rules` array (after `rules/trade.json`).

```json
"scripts": [
  "scripts/colony.js",
  "scripts/combat.js",
  "scripts/economy.js",
  "scripts/crops.js",
  "scripts/companion.js",
  "scripts/clock.js",
  "scripts/hud.js",
  "scripts/toast.js",
  "scripts/flare.js",
  "scripts/poi.js",
  "scripts/wish.js",
  "scripts/research.js",
  "scripts/faction.js",
  "scripts/region.js"
],
"rules": [
  "rules/move.json",
  "rules/ui.json",
  "rules/economy.json",
  "rules/colony.json",
  "rules/hud.json",
  "rules/quest.json",
  "rules/farm.json",
  "rules/companion.json",
  "rules/time.json",
  "rules/narrative.json",
  "rules/toast.json",
  "rules/flare.json",
  "rules/poi.json",
  "rules/affordability.json",
  "rules/wish.json",
  "rules/research.json",
  "rules/combat.json",
  "rules/faction.json",
  "rules/trade.json",
  "rules/region.json"
]
```

## 5. Tests — `crates/vitric-cli/tests/region.rs`

Add 3 tests at the end of the file.

### 5.1 `ctx_thaw_region_bridge_works_from_js`

Verify the `ctx.thaw_region` bridge works: call `unlock_region("mountain")` fn (which calls `ctx.thaw_region`), step, verify mountain region is now active.

```rust
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
```

### 5.2 `region_content_deterministic_across_thaw_timing`

Verify E3 substream determinism: same world_seed, different thaw timing → identical region content (node/POI positions).

```rust
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
```

### 5.3 `camera_world_bounds_clamps_player_position`

Verify Camera.world_bounds clamps the player's position.

```rust
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
```

## 6. Schema field audit (MANDATORY)

Every field read by a rule OR accessed via `ctx.getField`/`ctx.setField` MUST be declared in `schema.json`.

### New fields to declare:
- `Camera.world_bounds` (text, default "") — §3.2

### Existing fields read by new code (verify they exist):
- `Region.state` (enum, line 1041) ✓
- `Region.id` (text, line 1041) ✓
- `Region.biome` (text, line 1041) ✓
- `Region.discovered` (int, line 1041) ✓
- `Region.anchor_x`, `anchor_y`, `w`, `h` (int, line 1041) ✓
- `Region.dormant_ticks` (int, line 1041) ✓
- `Region.spawn_timer` (int, line 1041) ✓
- `Camera.follow` (text, line 237) ✓
- `Camera.x`, `Camera.y` (number) ✓
- `Position.x`, `Position.y` (number) ✓
- `Velocity.x`, `Velocity.y` (number) ✓
- `Colony.Research.has_exploration_t1` (int) ✓ (set by research.js:43 `e.Research["has_" + cur] = 1`)
- `Colony.Research.has_industry_t3` (int) ✓ (same pattern)
- `Colony.companion_handles` (list of text, line 752) ✓
- `Colony.Faction.tier_caravan` (text, line 1084) ✓
- `Companion` + `Persona.role` (enum, line 877) ✓
- `Node.kind`, `Node.left`, `Node.max`, `Node.cooldown` ✓
- `Poi.kind`, `Poi.state`, `Poi.cooldown`, `Poi.reward_table` ✓

### Fields written by new code (verify they exist):
- `Camera.world_bounds` — NEW (§3.2)
- `Cell.kind` ✓
- `Sprite.w`, `Sprite.h`, `Sprite.image`, `Sprite.color` ✓
- `Text.content`, `Text.size`, `Text.color`, `Text.screen` ✓

All fields are declared. No undeclared field issues.

## 7. UI layout audit

No new UI entities in Task 12 (no HUD elements, no buttons, no menus). Region markers are invisible (no Sprite, no Ui component). Region content (tiles, nodes, POIs) are world-space entities (Sprite + Text + Position), not UI entities.

**No UI overlap risk.**

## 8. Verification steps

The implementer MUST run these commands and verify they pass:

```bash
# 1. Schema check (catches undeclared fields, missing components, etc.)
cargo run --release -- check games/frontier

# 2. Region tests (new + existing)
cargo test -p vitric-cli --test region

# 3. Full workspace tests (catches regressions)
cargo test --workspace

# 4. Git diff stat (verify file count)
git diff --stat
```

## 9. Expected deviations

The implementer may discover minor issues during implementation. Document any deviations from this brief in the implementation report. Known potential deviations:

1. **`Colony.companion_handles` field type**: The brief assumes `ctx.getField("colony", "Colony.companion_handles")` returns a JSON array. If the field is stored as a `list` type (not `text`), `getField` might return a parsed array instead of a JSON string. The implementer should test this and adjust the parsing logic in `checkUnlockCondition` accordingly.

2. **`Region` component on spawned entities**: The brief spawns tiles/nodes/POIs with a `Region` component to tag them. If this causes issues with the dormant filter (engine's `query` skips dormant entities), the implementer may need to use a different tagging mechanism (e.g., a `RegionTag` component or a naming convention). Test and adjust.

3. **Camera clamping edge cases**: If the player spawns outside the initial world_bounds (e.g., player at x=7, bounds [0,0,60,30] — fine), no issue. But if bounds are set too small, the player could be clamped on load. The initial bounds [0,0,60,30] cover the home+wild area where the player starts (x=7, y=7), so no issue.

## 10. Implementation report

After implementation, the implementer MUST write a report at `.superpowers/sdd/briefs/task-12-report.md` containing:
- Files changed (with line counts)
- Tests run (with pass/fail counts)
- Deviations from brief (if any)
- Schema field audit result
- Commit hash

## 11. Commit

```bash
git add games/frontier/scripts/region.js games/frontier/scripts/world.js \
  games/frontier/rules/region.json games/frontier/rules/research.json \
  games/frontier/scenes/main.json games/frontier/schema.json games/frontier/vitric.json \
  crates/vitric-script/src/lib.rs crates/vitric-script/src/prelude.js \
  crates/vitric-sim/src/sim.rs crates/vitric-cli/tests/region.rs

git commit -m "feat(frontier): map expansion with 5 regions

5 regions (home/wild/mountain/swamp/desert), 3 dormant with unlock
conditions. Region content generated on thaw using E3 seeded substreams
(deterministic regardless of thaw timing). Camera world_bounds clamps
player to discovered regions. Adds ctx.thaw_region JS bridge (fixes
pre-existing dead unlock_region fn from Task 8)."
git push origin main
```
