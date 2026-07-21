# Task 12 Review — Map Expansion & Region Content

**Commit**: 50776c0
**Reviewer**: subagent
**Date**: 2026-07-21

## Verdict: NEEDS CHANGES

One Important gameplay-breaking bug found in `checkUnlockCondition("swamp", ...)`: `JSON.parse` is called on an already-parsed array, causing the swamp region to be permanently un-unlockable. The brief explicitly flagged this scenario as a deviation to investigate (brief §9.1), but the implementer's report states "No other deviations", meaning the investigation was not performed. The fix is trivial (3 lines). Everything else in the implementation is excellent and faithful to the brief.

## Summary

Task 12 expands the world from a single home+wild pocket to 5 regions, adds the missing `ctx.thaw_region` JS bridge (fixing the pre-existing dead `unlock_region` fn from Task 8), implements Camera `world_bounds` clamping in `sim.rs::integrate_motion`, and adds region content generation driven by deterministic E3 substreams. The engine changes, schema, scene, rules, and tests are all correct and well-structured. The only issue is a latent bug in the swamp unlock path that the brief explicitly warned about — `ctx.getField` on a `list`-typed field returns a parsed array, but the code calls `JSON.parse` on it, which throws and is silently swallowed by a `try/catch`.

## Section 1: Schema field audit — PASS

Every field read by a rule or accessed via `ctx.getField`/`ctx.setField` is declared in `schema.json`.

**New field declared:**
- `Camera.world_bounds` (text, default "") — schema.json:249-252 ✓

**Existing fields read by new code (verified present):**
- `Region.state` (enum, schema.json:1049), `Region.id` (1047), `Region.biome` (1048), `Region.discovered` (1050), `Region.anchor_x`/`anchor_y` (1051-1052), `Region.w`/`h` (1053-1054), `Region.dormant_ticks` (1055), `Region.spawn_timer` (1056) ✓
- `Camera.follow` (schema.json:237), `Camera.world_bounds` (249) ✓
- `Position.x`/`y`, `Velocity.x`/`y` ✓
- `Colony.companion_handles` (list of text, schema.json:756-761) ✓
- `Colony.Faction.tier_caravan` (text, schema.json:1088) — accessed via `ctx.getField("colony", "Faction.tier_caravan")` ✓
- `Colony.Research.has_exploration_t1` (int, schema.json:1071), `Colony.Research.has_industry_t3` (int, 1076) — accessed via `ctx.getField("colony", "Research.has_*")` ✓
- `Persona.role` (enum, schema.json:881-885) ✓
- `Node.kind`/`left`/`max`/`cooldown` (schema.json:554+) ✓
- `Poi.kind`/`state`/`cooldown`/`reward_table` (schema.json:992+) ✓
- `Cell.kind`, `Sprite.*`, `Text.*` ✓

**Fields written by new code (verified present):**
- `Camera.world_bounds` (NEW, written by `update_camera_bounds` via `ctx.setField`) ✓
- Spawned components: `Cell.kind`, `Position.x`/`y`, `Sprite.*`, `Text.*`, `Node.*`, `Poi.*`, `Region.*` ✓

**Rule reads in `rules/region.json`:** no `@entity.Comp.field` reads — rules only `call` functions with `event.id` / `{}` args. No schema field audit needed for rules. ✓

No undeclared field issues.

## Section 2: Enum variant audit — PASS

- `Region.state` enum variants: `["dormant", "active", "frozen"]` (schema.json:1049). New code uses `"active"` (in `gen_region_content` spawned Region components) and reads `"dormant"`/`"active"` (in `region-approach-check` + `update_camera_bounds`). All variants present. ✓
- `Poi.state` enum variants: `["fresh", "looted", "depleted"]` (schema.json:1000). New code sets `"fresh"` in `gen_region_content`. Variant present. ✓
- `Persona.role` enum variants: `["builder", "farmer", "explorer", "guard", "trader", "scholar"]` (schema.json:883). New code checks `role === "explorer"` in `checkUnlockCondition`. Variant present. ✓

No missing enum variants.

## Section 3: Scene entity reference audit — PASS

All entities referenced by name in `region.js` exist in `scenes/main.json` (verified by parsing the scene):

**Region markers (5):**
- `mountain` — Region{state:"dormant", anchor:(0,12), 30×28} ✓
- `home` — Region{state:"active", anchor:(0,0), 28×12} ✓ (NEW)
- `wild` — Region{state:"active", anchor:(28,0), 32×30} ✓ (NEW)
- `swamp` — Region{state:"dormant", anchor:(28,12), 32×28} ✓ (NEW)
- `desert` — Region{state:"dormant", anchor:(60,0), 60×60} ✓ (NEW)

**Other referenced entities:**
- `player` — has Player, Position, Velocity components ✓
- `camera` — has Camera component with `follow:"player"`, `world_bounds:"[0,0,60,30]"` ✓
- `colony` — has Colony, Research, Faction components ✓

**Runtime-spawned entities (not in scene, created by `gen_region_content`):**
- `mountain_node_0..5`, `mountain_poi_0` — spawned with `ctx.spawn({...}, id + "_node_" + idx)` / `id + "_poi_" + idx`. The test `region_content_deterministic_across_thaw_timing` reads these by name and they exist after thaw. ✓

No missing entity references.

## Section 4: UI layout overlap audit — PASS

Task 12 adds no UI entities. The 4 new region markers (`home`, `wild`, `swamp`, `desert`) carry only a `Region` component — no `Sprite`, no `Ui`, no `Text`, no `Position`. They are invisible data-only markers. No UI overlap risk.

## Section 5: Standard checks — PASS

- `cargo run --release -- check games/frontier` exits 0 (verified by controller). ✓
- All new `//` comments in region.js, lib.rs, prelude.js, sim.rs are in English (project convention). ✓
- String literals keep their authored language: panic/error messages in prelude.js are Chinese ("ctx.thaw_region: id 必须是非空字符串"); game labels in region.js are Chinese ("矿脉", "纤维丛", "晶核", "古代遗迹", "危险植物", "商队营地", "古墓", "区域生成: "). ✓
- No fake APIs used — only `ctx.random_stream`, `ctx.spawn`, `ctx.emit`, `ctx.getField`, `ctx.setField`, `ctx.thaw_region` (all real, all registered in prelude.js). `Math.random` not used. ✓
- No duplicate `const` declarations across scripts: `REGION_SPECS`, `REGION_CONTENT` are unique to region.js. No collision with other scripts. ✓
- Tests pass: 17/17 region tests (3 new + 14 existing). ✓
- Commit message follows `<type>(<scope>): <summary>` convention: `feat(frontier): map expansion with 5 regions`. ✓
- Only in-scope files modified (11 files, all listed in brief §2). ✓
- No dead code: `REGION_SPECS`, `REGION_CONTENT`, `gen_region_content`, `update_camera_bounds`, `region-approach-check`, `checkUnlockCondition` are all reachable and used. The `spec.state` field on `REGION_SPECS` is a fast-path filter for home/wild (skips the `getField` call for active regions) — slightly redundant with the dynamic `ctx.getField(id, "Region.state")` check but not dead. ✓

## Deviations

### Deviation 1: `region-approach-check` system uses `query: ["Player"]` instead of `query: []` — ACCEPTABLE

**Brief specified** (§4.1): `vitric.system("region-approach-check", { query: [], writes: [] }, ...)` with design note: "system has `query: []` (no entity batch) — it reads everything via `ctx.getField`."

**Issue discovered**: The prelude (`crates/vitric-script/src/prelude.js:18`) explicitly rejects empty query arrays:
```javascript
if (!decl || !Array.isArray(decl.query) || decl.query.length === 0) {
  throw new Error("vitric.system(\"" + name + "\"): 第二个参数必须含非空 query 数组…");
}
```
Additionally, even if the prelude accepted empty queries, `World::query(&[])` returns ALL non-dormant entities (vacuous truth on empty required-components iterator), which would iterate 490+ entities per tick doing nothing — wasteful.

**Resolution**: Use `query: ["Player"]`. `"Player"` is a valid schema component (schema.json:275-277, tag component with empty fields). The system body ignores the `entities` batch and reads everything via `ctx.getField("player", ...)`. There is exactly one player entity, so the system runs once per tick.

**Assessment**: Acceptable. This is actually better than the brief's specification because:
1. It works around a real prelude limitation (empty queries are rejected).
2. It's more efficient than iterating all entities (which `World::query(&[])` would do).
3. It preserves the brief's intent: no query-based writes (`writes: []`), only deferred ops via `ctx.thaw_region` / `ctx.emit`.
4. The system body is identical to what the brief specified — only the query declaration differs.

The implementer documented this deviation clearly with a comment block above the system (region.js:145-151).

### Deviation 2 (NEW, blocker): Swamp unlock broken — `JSON.parse` called on already-parsed array

**Brief warned** (§9.1): "Colony.companion_handles field type: The brief assumes `ctx.getField("colony", "Colony.companion_handles")` returns a JSON array. If the field is stored as a `list` type (not `text`), `getField` might return a parsed array instead of a JSON string. The implementer should test this and adjust the parsing logic in `checkUnlockCondition` accordingly."

**Issue found**: `Colony.companion_handles` is declared as `"type": "list", "of": {"type": "text"}` (schema.json:756-761). The prelude's `getField` calls `__getFieldRaw` (which serializes the stored `Value::Array` to a JSON string via `serde_json::to_string`), then `JSON.parse(raw)` on the JS side — returning a **parsed array**, not a JSON string.

This is confirmed by existing code in `games/frontier/scripts/wish.js:21-22`:
```javascript
const handles = ctx.getField("colony", "Colony.companion_handles") || [];
for (const h of handles) {  // iterates the array directly — would iterate chars if it were a string
```

The implementer's code in `region.js:194-197`:
```javascript
const handlesJson = ctx.getField("colony", "Colony.companion_handles");
if (!handlesJson) return false;
let handles = [];
try { handles = JSON.parse(handlesJson); } catch { return false; }
```

When `companion_handles` is non-empty (e.g. `["e3v0","e4v1"]`):
1. `handlesJson` is the parsed array `["e3v0","e4v1"]` (truthy, so the `!handlesJson` check passes).
2. `JSON.parse(["e3v0","e4v1"])` coerces the array to string `"e3v0,e4v1"`, then attempts to parse — throws `SyntaxError`.
3. The `catch` block returns `false`.

When `companion_handles` is empty (`[]`):
1. `handlesJson` is `[]` (truthy — empty arrays are truthy in JS).
2. `JSON.parse([])` → `JSON.parse("")` → throws `SyntaxError`.
3. `catch` returns `false`.

**Impact**: `checkUnlockCondition("swamp", ctx)` ALWAYS returns `false`. The swamp region can NEVER be unlocked via the `region-approach-check` system. This is a gameplay-breaking bug — one of the three dormant regions is permanently inaccessible.

The bug is not caught by tests because the test suite only exercises the mountain unlock path (via `unlock_region` fn called directly). No test covers the swamp or desert unlock conditions.

**Fix** (3 lines, mirrors the pattern in `wish.js:21-22`):
```javascript
if (id === "swamp") {
  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  if (!Array.isArray(handles)) return false;
  for (const h of handles) {
    if (typeof h !== "string" || !h) continue;
    const role = ctx.getField(h, "Persona.role");
    if (role === "explorer") return true;
  }
  return false;
}
```

**Note**: The brief's own example code (§4.1, lines 386-398) has the same bug — the implementer faithfully copied it. However, the brief explicitly instructed the implementer to test this scenario and adjust accordingly (§9.1). The implementer's report states "No other deviations", indicating the investigation was not performed.

## Issues found

### Important

1. **Swamp region permanently un-unlockable** — `region.js:194-197`. `JSON.parse` is called on an already-parsed array (returned by `ctx.getField` for `list`-typed fields), which throws `SyntaxError` and is silently swallowed by the `try/catch`, causing `checkUnlockCondition("swamp", ...)` to always return `false`. The swamp region can never be thawed by the `region-approach-check` system. See Deviation 2 above for full analysis and fix. **Blocker for approval.**

### Minor

2. **No test coverage for swamp or desert unlock conditions** — `crates/vitric-cli/tests/region.rs`. The new tests verify the `ctx.thaw_region` bridge (mountain), determinism (mountain), and camera clamping. No test exercises `checkUnlockCondition("swamp", ...)` or `checkUnlockCondition("desert", ...)`. This is why the swamp bug above was not caught. Consider adding tests that set up `companion_handles` with an explorer-role companion and verify the swamp unlocks, and tests that set `Faction.tier_caravan` + `Research.has_industry_t3` and verify the desert unlocks. Not a blocker, but the swamp bug would have been caught by such a test.

3. **Spawn position collisions in `gen_region_content`** — `region.js:85-86, 103-104`. Resource nodes and POIs are placed at random positions within the region bounds via `stream.nextInt(0, spec.w - 1)`. With 6 nodes + 1 POI in a 30×28 region, collisions are possible (two entities at the same tile). This is a minor visual issue, not a correctness bug — Task 13 (Region Content Polish) can address it if needed. Not a blocker.

### Nits

4. **`spec.state` field on `REGION_SPECS` is a static fast-path filter** — `region.js:15-21`. The `region-approach-check` system checks `if (spec.state !== "dormant") continue;` before the dynamic `ctx.getField(id, "Region.state")` check. This is a minor optimization (skips the `getField` call for home/wild, which are always active). Slightly redundant with the dynamic check but not dead code. Acceptable.

5. **`region-approach-check` runs every tick** — `region.js:152`. The system checks 3 dormant regions × 1 player × 1 `getField` per region per tick = 3 `getField` calls/tick. This is cheap (60 calls/s). Acceptable. Could be optimized to run every 30 ticks if needed, but not necessary.

## Files reviewed

- `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/briefs/task-12-brief.md` (brief)
- `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/briefs/task-12-report.md` (implementation report)
- `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/review-checklist.md` (checklist)
- `/Users/leolele/Documents/leo/vitric/crates/vitric-script/src/lib.rs` (engine: `__thawRegion` native fn registration, lines 178-188)
- `/Users/leolele/Documents/leo/vitric/crates/vitric-script/src/prelude.js` (engine: `ctx.thaw_region` JS bridge, lines 198-206; prelude query validation, line 18)
- `/Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/sim.rs` (engine: `thaw_region` fn, lines 280-294; camera world_bounds clamping in `integrate_motion`, lines 725-771)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/region.js` (NEW game script, 212 lines)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/world.js` (extended `genWild`, lines 6-38)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/research.js` (existing `unlock_region` fn that calls `ctx.thaw_region`, lines 99-105; existing `companion_handles` writes in companion.js)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/wish.js` (reference: existing `ctx.getField("colony", "Colony.companion_handles")` usage confirming array return type, lines 21-22, 150-151, 191-192)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/companion.js` (reference: existing `companion_handles` write pattern, line 125; read pattern, line 133)
- `/Users/leolele/Documents/leo/vitric/games/frontier/rules/region.json` (NEW rules file, 2 rules)
- `/Users/leolele/Documents/leo/vitric/games/frontier/rules/research.json` (removed `research-unlock-desert`, kept `research-unlock-mountain` + `tp-apply`)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scenes/main.json` (verified 5 region markers + camera world_bounds via Python parse)
- `/Users/leolele/Documents/leo/vitric/games/frontier/schema.json` (Camera.world_bounds, Region, Research, Faction, Persona, Node, Poi fields)
- `/Users/leolele/Documents/leo/vitric/games/frontier/vitric.json` (region.js + region.json registered)
- `/Users/leolele/Documents/leo/vitric/crates/vitric-cli/tests/region.rs` (3 new tests, lines 514-657)
