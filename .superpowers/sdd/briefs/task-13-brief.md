# Task 13 — Region Content Polish

> Spec §4.6 (lines 314-317), Plan §Task 13 (lines 1360-1390).
> Predecessor: Task 12 (Map Expansion). Successor: Task 14 (Pacing Rebalance).

## 0. Goal

Polish the region content framework from Task 12 into a rich, varied gameplay experience. Three deliverables:
1. **Expanded POI tables** — more POI types per region with unique rewards.
2. **Per-type POI handlers** — `interact_poi` dispatches by kind, each with special effects beyond the standard reward roll.
3. **Biome-specific enemy spawning** — sandbeast in desert, spawned on a timer when the player is in desert.

This is a CONTENT + POLISH task. No engine changes, no schema changes, no new files. Only 3 existing scripts are modified.

## 1. Files to touch

- **MODIFY** `games/frontier/scripts/region.js` — expand `REGION_CONTENT` with more POI types per region
- **MODIFY** `games/frontier/scripts/poi.js` — add `POI_HANDLERS` dict, refactor `interact_poi` to dispatch by kind
- **MODIFY** `games/frontier/scripts/combat.js` — add `sandbeast` to `ENEMY_TYPES`, add `desert-spawn` system

No new files. No schema changes. No engine changes. No scene changes. No vitric.json changes.

## 2. region.js — Expand REGION_CONTENT

### Current state (Task 12)

```javascript
const REGION_CONTENT = {
  mountain: {
    tile_color: "#3a3530",
    nodes: [{ kind: "ore", count: 6, ... }],
    pois: [{ kind: "ancient-ruins", reward_table: '{"techpoint":[1,3]}', label: "古代遗迹" }],
  },
  swamp: {
    tile_color: "#2a3a2a",
    nodes: [{ kind: "fiber", count: 5, ... }],
    pois: [{ kind: "dangerous-flora", reward_table: '{"hide":[1,2]}', label: "危险植物" }],
  },
  desert: {
    tile_color: "#7a6a3a",
    nodes: [{ kind: "crystal_core", count: 2, ... }],
    pois: [
      { kind: "caravan", reward_table: '{}', label: "商队营地" },
      { kind: "tomb", reward_table: '{"crystal_core":[1,2],"techpoint":[2,4]}', label: "古墓" },
    ],
  },
};
```

### Target state (Task 13)

Expand each region to 2-3 POI types with varied rewards. The `gen_region_content` fn already iterates `content.pois` and spawns each — no change needed to the generator logic, only to the data.

```javascript
const REGION_CONTENT = {
  mountain: {
    tile_color: "#3a3530",
    nodes: [
      { kind: "ore", count: 6, color: "#caa45a", label: "矿脉", left: 5 },
    ],
    pois: [
      // Ancient ruins: TechPoint reward (already in Task 12).
      { kind: "ancient-ruins", reward_table: '{"techpoint":[1,3]}', label: "古代遗迹" },
      // Crystal cave: crystal_core reward + cave-injury risk (handler in poi.js).
      { kind: "crystal-cave", reward_table: '{"crystal_core":[1,2]}', label: "水晶洞" },
    ],
  },
  swamp: {
    tile_color: "#2a3a2a",
    nodes: [
      { kind: "fiber", count: 5, color: "#9aac5a", label: "纤维丛", left: 5 },
    ],
    pois: [
      // Dangerous flora: hide reward + combat trigger (handler spawns a weak enemy).
      { kind: "dangerous-flora", reward_table: '{"hide":[1,2]}', label: "危险植物" },
      // Oasis: seed + fiber reward (fertile ground).
      { kind: "oasis", reward_table: '{"seed":[2,4],"fiber":[1,3]}', label: "绿洲" },
    ],
  },
  desert: {
    tile_color: "#7a6a3a",
    nodes: [
      { kind: "crystal_core", count: 2, color: "#5acaff", label: "晶核", left: 3 },
    ],
    pois: [
      // Caravan stop: no direct reward, but handler emits trade-available (faction hook).
      { kind: "caravan-stop", reward_table: '{}', label: "商队驿站" },
      // Tomb: high-tier reward + curse risk (handler applies mood drop).
      { kind: "tomb", reward_table: '{"crystal_core":[1,2],"techpoint":[2,4]}', label: "古墓" },
    ],
  },
};
```

**Changes**:
- mountain: +1 POI (crystal-cave)
- swamp: +1 POI (oasis)
- desert: renamed "caravan" → "caravan-stop" (clearer name), tomb unchanged

**Note**: The `gen_region_content` fn names POIs as `<region_id>_poi_<idx>`. With 2 POIs per region, the test `region_content_deterministic_across_thaw_timing` (which reads `mountain_poi_0`) still works — it reads the first POI. No test change needed.

## 3. poi.js — Per-type POI handlers

### Current state (Task 12)

`interact_poi` fn rolls rewards from `reward_table` uniformly for all POI kinds. The only special-case is `cave-entrance` (30% mood drop) — but `cave-entrance` is a legacy kind from the wild area, not used by region POIs.

### Target state (Task 13)

Add a `POI_HANDLERS` dict mapping kind → handler function. Each handler receives `(a, ctx, poi, rewards)` and can add special effects. The main `interact_poi` fn dispatches to the handler after the standard reward roll.

```javascript
// ---- Per-type POI handlers: special effects beyond the standard reward_table roll ----
// Each handler receives (a, ctx, poi, rewardText) and can emit additional events.
// The standard reward roll (from reward_table) happens BEFORE the handler — handlers
// only add extra effects (events, mood changes, combat triggers, etc.).
const POI_HANDLERS = {
  "ancient-ruins": (a, ctx, poi, rewardText) => {
    // Bonus TechPoint for discovering ancient ruins (on top of the standard +2 per POI).
    const tp = (a.techpoint | 0) + 3;
    ctx.emit("tp-set", { value: tp });
    ctx.emit("toast-show", { text: "古代遗迹: 额外+3科技点" });
  },

  "crystal-cave": (a, ctx, poi, rewardText) => {
    // Crystal cave: 30% chance of cave-injury (companion mood drop).
    // Moved from the legacy "cave-entrance" kind — crystal caves are the new cave POI.
    if (ctx.random() < 0.3) {
      ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
      ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
    }
  },

  "dangerous-flora": (a, ctx, poi, rewardText) => {
    // Dangerous flora: 50% chance of spawning a weak enemy (combat trigger).
    // The enemy spawns at the POI's position — the player must deal with it.
    if (ctx.random() < 0.5) {
      const x = a.comp.Position.x;
      const y = a.comp.Position.y;
      ctx.spawn({
        Enemy: { kind: "gnawer", damage: 5, aggro_range: 6, home_region: "swamp", _attack_cd: 0 },
        Position: { x: x + 1, y: y },
        Velocity: { x: 0, y: 0 },
        Collider: { w: 0.8, h: 0.8 },
        Sprite: { w: 0.8, h: 0.8, image: "enemy.png", color: "#7a9a3a" },
        Hp: { value: 15, max: 15 },
      });
      ctx.emit("toast-show", { text: "危险植物释放了孢子!出现了敌对生物" });
    }
  },

  "oasis": (a, ctx, poi, rewardText) => {
    // Oasis: full party mood restoration (fertile ground, safe haven).
    ctx.emit("companion-mood-boost", { amount: 5, reason: "oasis" });
    ctx.emit("toast-show", { text: "绿洲清泉:全员心情+5" });
  },

  "caravan-stop": (a, ctx, poi, rewardText) => {
    // Caravan stop: emit trade-available event (faction trade hook).
    // The caravan faction's relation +1 hook (from Task 11's trader-companion-relation rule)
    // also fires on trade-available — so discovering a caravan-stop improves caravan relation.
    ctx.emit("trade-available", { pid: "caravan-stop", role: "trader" });
    ctx.emit("toast-show", { text: "商队驿站:贸易关系+1" });
  },

  "tomb": (a, ctx, poi, rewardText) => {
    // Tomb: 40% chance of curse (mood drop) — the high-tier reward comes with risk.
    if (ctx.random() < 0.4) {
      ctx.emit("companion-mood-drop", { amount: 15, reason: "tomb-curse" });
      ctx.emit("toast-show", { text: "古墓诅咒!全员心情-15" });
    }
  },
};
```

### Refactor interact_poi

Modify the existing `interact_poi` fn to dispatch to `POI_HANDLERS[poi.kind]` after the standard reward roll. Replace the existing `cave-entrance` special-case (lines 87-90) with the handler dispatch.

**Current code** (lines 86-93):
```javascript
  // Cave-entrance risk: 30% chance of companion mood drop (cave-injury).
  if (poi.kind === "cave-entrance" && ctx.random() < POI_CAVE_INJURY_CHANCE) {
    ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
    ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
  }

  // Notify wish system (Task 4 will add a rule listening for this event).
  ctx.emit("entered-poi", { kind: poi.kind });
```

**New code**:
```javascript
  // Per-type handler: special effects beyond the standard reward roll.
  // Handler runs AFTER rewards are applied (inventory + techpoint already emitted).
  const handler = POI_HANDLERS[poi.kind];
  if (handler) handler(a, ctx, poi, rewardText);

  // Keep the legacy cave-entrance special-case for wild-area POIs (backward compat).
  if (poi.kind === "cave-entrance" && ctx.random() < POI_CAVE_INJURY_CHANCE) {
    ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
    ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
  }

  // Notify wish system.
  ctx.emit("entered-poi", { kind: poi.kind });
```

**Key design points**:
1. The handler runs AFTER the standard reward roll (inventory + techpoint already emitted). Handlers only add extra effects.
2. The legacy `cave-entrance` special-case stays (wild-area POIs still use the old kind). No backward-compat break.
3. `companion-mood-boost` is a NEW event — check if a rule handles it. If not, add a rule in `rules/companion.json` or handle it inline. Actually, let me check the existing mood-drop pattern...

**Check**: Does `companion-mood-boost` exist as an event? Let me check the companion rules. If not, I'll use `companion-mood-drop` with negative amount, or emit a different event. Actually, the simplest approach: emit `companion-mood-drop` with a negative amount (which would be a boost). But that's semantically confusing. Let me check what events the companion system listens for.

Actually, for the brief, I'll specify: if `companion-mood-boost` has no consumer, the handler should instead directly set the mood via `ctx.setField` on each companion. But that's complex (need to iterate companions). Simpler: just emit the event and let a future task add the consumer. For now, the toast is the main feedback.

Wait — let me reconsider. The `oasis` handler emits `companion-mood-boost`. If no rule consumes this event, it's a no-op (the toast still shows). That's acceptable for Task 13 — the mood boost is a forward-compat hook. But it's better to make it work. Let me check if there's a simple way to apply mood boost.

Looking at the companion system, mood is stored on the `Mood` component (value: text like "平静"). Changing mood requires the companion system's logic (mood levels, thresholds). A simple `+5` boost would need to go through the companion system's mood-update logic.

For Task 13, I'll keep `companion-mood-boost` as an emitted event with a toast. If the companion system doesn't consume it, it's a forward-compat hook (Task 14 or later can add a consumer). The toast gives the player feedback. This is acceptable for a polish task.

Actually, let me check if `companion-mood-drop` has a consumer — if it does, I can mirror that for boost. Let me check the rules.

Actually, I already know from earlier context that `companion-mood-drop` is emitted by poi.js (line 88) and presumably consumed by a companion rule. Let me check. But for the brief, I'll note that `companion-mood-boost` may need a consumer — if `companion-mood-drop` has one, the implementer should add a mirror rule for boost.

## 4. combat.js — Sandbeast + desert-spawn system

### 4.1 Add sandbeast to ENEMY_TYPES

**Current** (line 52-55):
```javascript
const ENEMY_TYPES = {
  gnawer: { damage: 5,  aggro_range: 8,  hp: 20, drops: { hide: [1, 2] } },
  raider: { damage: 8,  aggro_range: 10, hp: 35, drops: { hide: [1, 1], crystal_core: [0, 1] } },
};
```

**New**:
```javascript
const ENEMY_TYPES = {
  gnawer: { damage: 5,  aggro_range: 8,  hp: 20, drops: { hide: [1, 2] } },
  raider: { damage: 8,  aggro_range: 10, hp: 35, drops: { hide: [1, 1], crystal_core: [0, 1] } },
  // Sandbeast: desert-only enemy, spawned by desert-spawn system. High HP, high damage,
  // drops crystal_core. Only spawns when desert is active AND player is in desert.
  sandbeast: { damage: 12, aggro_range: 12, hp: 60, drops: { hide: [2, 3], crystal_core: [1, 2] } },
};
```

### 4.2 Add desert-spawn system

Add a new system at the end of combat.js. This system:
1. Queries the `desert` region marker entity (by reading its Region component via ctx.getField — NOT by query, since the system's query is for something else).
2. If desert is active, decrements `Region.spawn_timer`.
3. When spawn_timer hits 0, checks if player is in desert (player Position within desert bounds).
4. If yes, spawns a sandbeast near the player using `ctx.random_stream("desert_spawn")` for deterministic position.
5. Resets spawn_timer to 7200 (2 minutes real time = 2 in-game hours at DAY_SEC=60).

Wait — the system needs to write to `Region.spawn_timer` on the desert marker. But systems declare `writes` for components they modify via the entity batch. The desert marker is NOT in the system's query batch (the system queries something else). So the system must use `ctx.setField("desert", "Region.spawn_timer", ...)` to write to it.

Actually, let me reconsider the system design. The system can query `["Region"]` (all region markers) and filter for desert in the body. Then `writes: ["Region"]` covers the spawn_timer write.

```javascript
// Desert spawn: every 2 in-game hours (7200 ticks = 2 min real time), if the desert region
// is active AND the player is inside it, spawn a sandbeast near the player. Uses
// ctx.random_stream("desert_spawn") for deterministic spawn position — replay-safe regardless
// of when the spawn happens.
//
// The spawn_timer field on Region (schema line 1041) tracks the cooldown. It's decremented
// each tick; when it hits 0, the spawn check fires and the timer resets.
vitric.system("desert-spawn", { query: ["Region"], writes: ["Region"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Region.id !== "desert") continue;
    if (e.Region.state !== "active") continue;

    // Decrement spawn timer.
    let timer = e.Region.spawn_timer - ctx.dt;
    if (timer > 0) {
      e.Region.spawn_timer = timer;
      continue;
    }

    // Timer expired — reset and check if player is in desert.
    e.Region.spawn_timer = 7200; // 2 minutes real time (7200 ticks at 60 tick/s)

    const px = ctx.getField("player", "Position.x");
    const py = ctx.getField("player", "Position.y");
    if (typeof px !== "number" || typeof py !== "number") continue;

    // Desert bounds: anchor (60,0), size 60×60 → x:60..119, y:0..59.
    const inDesert = px >= 60 && px <= 119 && py >= 0 && py <= 59;
    if (!inDesert) continue;

    // Spawn sandbeast near player using desert_spawn substream (deterministic).
    const stream = ctx.random_stream("desert_spawn");
    const ox = stream.nextInt(-3, 3);
    const oy = stream.nextInt(-3, 3);
    const def = ENEMY_TYPES.sandbeast;
    ctx.spawn({
      Enemy: { kind: "sandbeast", damage: def.damage, aggro_range: def.aggro_range,
               home_region: "desert", _attack_cd: 0 },
      Position: { x: px + ox, y: py + oy },
      Velocity: { x: 0, y: 0 },
      Collider: { w: 1.0, h: 1.0 },
      Sprite: { w: 1.0, h: 1.0, image: "enemy.png", color: "#d4a84a" },
      Hp: { value: def.hp, max: def.hp },
    });
    ctx.emit("toast-show", { text: "沙兽出现!" });
  }
});
```

**Key design points**:
1. The system queries `["Region"]` — matches all 5 region markers. Filters for desert + active in the body.
2. `writes: ["Region"]` — the system modifies `Region.spawn_timer` via the entity batch (e.Region.spawn_timer = ...). This is the standard system-write pattern.
3. Uses `ctx.random_stream("desert_spawn")` for deterministic spawn offset — replay-safe.
4. The spawn_timer starts at 7200 (set in the scene's desert marker, Task 12). First spawn check happens after 2 minutes of real time.
5. Player position check: desert bounds are (60,0)-(119,59) per REGION_SPECS.

**Note**: The system reads `ctx.getField("player", "Position.x/y")` — these are NOT in the system's query batch. This is the same pattern as `region-approach-check` (reading player pos by name). The `writes` declaration only covers Region (the queried component being modified), not the player reads (which are via `getField`, a deferred-op channel that doesn't require `writes` declaration).

### 4.3 Update spawn_wave to include sandbeast

The existing `spawn_wave` fn (line 84) spawns gnawers and raiders on night-fall. Sandbeasts are NOT part of night waves — they're desert-only and spawn via the `desert-spawn` system. No change to `spawn_wave`.

But the `enemy-attack-player` and `enemy-attack-structures` systems already handle all enemies with `Enemy` component — sandbeasts will be automatically included (they have `Enemy` + `Position` + `Hp`). No change needed to the combat systems.

## 5. Schema field audit (MANDATORY)

### New fields read/written:
None. All fields used by Task 13 already exist:
- `Poi.kind` (text) ✓ — new kinds are just new string values
- `Poi.state`, `Poi.cooldown`, `Poi.reward_table` ✓
- `Region.spawn_timer` (int, schema line 1041) ✓ — already declared in Task 1
- `Region.id`, `Region.state` ✓
- `Enemy.kind`, `Enemy.damage`, `Enemy.aggro_range`, `Enemy.home_region`, `Enemy._attack_cd` ✓
- `Position.x/y`, `Velocity.x/y`, `Collider.w/h`, `Sprite.w/h/image/color`, `Hp.value/max` ✓

### New event names emitted:
- `companion-mood-boost` (NEW — from oasis handler)
- `trade-available` (existing — from caravan-stop handler, already has a consumer in Task 11's `trader-companion-relation` rule)
- `companion-mood-drop` (existing — from crystal-cave and tomb handlers)
- `toast-show` (existing)
- `tp-set` (existing)
- `entered-poi` (existing)

**`companion-mood-boost` consumer**: Check if `rules/companion.json` has a rule for `companion-mood-drop`. If it does, the implementer should add a mirror rule for `companion-mood-boost` (same logic, opposite direction). If not, the event is a forward-compat hook (no consumer, toast is the only feedback). Either way, no schema change needed — events are not schema-declared.

## 6. UI layout audit

No UI changes. No new HUD entities, no new buttons, no new menus. Task 13 only modifies game logic scripts.

**No UI overlap risk.**

## 7. Tests

Task 13 is a content/polish task. The existing tests should continue to pass. Add 2 focused tests:

### 7.1 Test: POI handler dispatches by kind

In `crates/vitric-cli/tests/` — add a new test file `poi.rs` OR add to an existing test file. The test verifies that `interact_poi` dispatches to the correct handler.

Actually, let me reconsider. The existing `interact_poi` fn is already tested implicitly (the gate recording exercises it). Adding a dedicated test for handler dispatch is good but may be complex (need to set up a POI entity, call interact_poi, verify the handler's effect).

For simplicity, I'll add 2 tests to the existing `region.rs` test file (since region content is the context):

1. **`poi_handlers_dispatch_by_kind`**: Set up a crystal-cave POI, call interact_poi, verify cave-injury event fires (or doesn't, based on RNG). Actually, this is hard to test deterministically because the handler uses `ctx.random()`. Let me design a simpler test.

2. **`sandbeast_spawns_in_desert_on_timer`**: Set desert to active, set spawn_timer to 0, place player in desert, step, verify sandbeast spawns.

Let me make the tests simple and focused:

### Test 1: `sandbeast_spawns_when_player_in_desert`

```rust
#[test]
fn sandbeast_spawns_when_player_in_desert() {
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    // Activate desert region and set spawn_timer to 0 (trigger spawn this tick).
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
```

### Test 2: `sandbeast_does_not_spawn_when_player_outside_desert`

```rust
#[test]
fn sandbeast_does_not_spawn_when_player_outside_desert() {
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
```

## 8. Verification steps

```bash
cargo run --release -- check games/frontier
cargo test -p vitric-cli --test region
cargo test --workspace
```

All must pass.

## 9. Implementation report

After implementation, write a report at `.superpowers/sdd/briefs/task-13-report.md` containing:
- Files changed (with line counts)
- Tests run (with pass/fail counts)
- Deviations from brief (if any)
- Schema field audit result
- Whether `companion-mood-boost` has a consumer (and if one was added)
- Commit hash

## 10. Commit

```bash
git add games/frontier/scripts/region.js games/frontier/scripts/poi.js games/frontier/scripts/combat.js \
  crates/vitric-cli/tests/region.rs

git commit -m "feat(frontier): region content polish — POI tables, biome enemies

Expanded POI types per region (crystal-cave, oasis, caravan-stop).
Per-type POI handlers with special effects (cave-injury, combat trigger,
mood boost, trade hook, curse). Sandbeast enemy in desert with
timer-based spawning using desert_spawn substream."
git push origin main
```
