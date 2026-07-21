# Task 13 — Region Content Polish — Implementation Report

## Files changed

| File | Lines |
| --- | --- |
| `games/frontier/scripts/region.js`  | +9 / -1 |
| `games/frontier/scripts/poi.js`     | +69 / -2 |
| `games/frontier/scripts/combat.js`  | +56 / -1 |
| `crates/vitric-cli/tests/region.rs` | +70 / 0  |
| **Total**                           | **+204 / -4** |

(`git diff --stat` reports 204 insertions, 4 deletions across the 4 files.)

## What was implemented

### 1. `region.js` — Expanded REGION_CONTENT
- mountain: +1 POI (`crystal-cave`, crystal_core reward)
- swamp: +1 POI (`oasis`, seed + fiber reward)
- desert: renamed `caravan` → `caravan-stop` (clearer name); `tomb` unchanged
- Each region now has exactly 2 POI types (was 1-2)

### 2. `poi.js` — POI_HANDLERS + interact_poi refactor
- Added `POI_HANDLERS` dict (top-level `const`, unique name — no QuickJS global-scope collision)
- 6 handlers: `ancient-ruins` (bonus +3 TechPoint), `crystal-cave` (30% cave-injury mood drop), `dangerous-flora` (50% spawn gnawer), `oasis` (mood boost event), `caravan-stop` (trade-available event), `tomb` (40% curse mood drop)
- Refactored `interact_poi` to dispatch to `POI_HANDLERS[poi.kind]` AFTER the standard reward roll
- Kept the legacy `cave-entrance` special-case for backward compat with wild-area POIs (per brief)

### 3. `combat.js` — Sandbeast + desert-spawn system
- Added `sandbeast` to `ENEMY_TYPES` (damage 12, hp 60, drops hide + crystal_core)
- Added `desert-spawn` system: queries `["Region"]`, writes `["Region"]`
  - Filters for desert marker + `state === "active"`
  - Decrements `Region.spawn_timer` by `ctx.dt` each tick
  - When timer ≤ 0: resets to 7200, checks if player is inside desert bounds (60..119, 0..59)
  - If yes: spawns sandbeast near player using `ctx.random_stream("desert_spawn")` for deterministic offset
  - Reads player position via `ctx.getField("player", "Position.x/y")` (deferred-op channel — no extra `writes` declaration needed, same pattern as `region-approach-check`)

### 4. `crates/vitric-cli/tests/region.rs` — 2 new tests
- `sandbeast_spawns_when_player_in_desert`: activates desert, sets spawn_timer=0, places player at (70, 10), steps, verifies sandbeast spawned
- `sandbeast_does_not_spawn_when_player_outside_desert`: same setup, player at (7, 7) outside desert, verifies no spawn

## Tests run

| Suite | Result |
| --- | --- |
| `cargo run --release -- check games/frontier` | ✅ exit 0 (desert-spawn system registered, no warnings) |
| `cargo test -p vitric-cli --test region`      | ✅ 19 passed / 0 failed (incl. 2 new sandbeast tests) |
| `cargo test --workspace`                      | ✅ all pass **except** 2 pre-existing `typescript.rs` failures |

### Pre-existing `typescript.rs` failures (NOT a regression)

The 2 `typescript.rs` tests (`typescript_syntax_error_names_the_file`, `typescript_system_runs_after_transpile`) fail with:

> 测试需要 esbuild：仓库里跑 `cd mcp && npm install`，或设 ESBUILD_BIN

(Test requires `esbuild` binary — install via `cd mcp && npm install`, or set `ESBUILD_BIN`.)

This is an **environment issue** (missing external `esbuild` binary), not a regression from Task 13. Verified by `git stash && cargo test -p vitric-cli --test typescript` — the same 2 tests fail identically on the clean main branch (commit `f8edc63`) before any Task 13 changes. The tests are unrelated to game scripts (they test TypeScript transpilation infrastructure).

## Deviations from brief

**None.** All code copied faithfully from the brief. The brief's section 5 (companion-mood-boost consumer check) is handled as documented below — no deviation, just a documented finding.

## Schema field audit: **PASS**

All fields used by Task 13 already exist in the schema — no new fields, no schema changes:
- `Poi.kind` / `Poi.state` / `Poi.cooldown` / `Poi.reward_table` ✓ (new kinds are just new string values for the existing `kind` text field)
- `Region.id` / `Region.state` / `Region.spawn_timer` ✓ (`spawn_timer` was declared in Task 1, schema line 1041)
- `Enemy.kind` / `Enemy.damage` / `Enemy.aggro_range` / `Enemy.home_region` / `Enemy._attack_cd` ✓
- `Position.x/y` / `Velocity.x/y` / `Collider.w/h` / `Sprite.w/h/image/color` / `Hp.value/max` ✓

### New event names emitted (events are not schema-declared)
- `companion-mood-boost` (NEW — from `oasis` handler; see consumer note below)
- `trade-available` (existing — consumed by Task 11's `trader-companion-relation` rule)
- `companion-mood-drop` (existing — consumed by `companion-mood-drop-apply` rule in `rules/wish.json`)
- `tp-set`, `toast-show`, `entered-poi` (existing)

## `companion-mood-boost` consumer

**No consumer was added. The event is currently a forward-compat hook (toast is the only feedback).**

### What the brief asked for
> Check if `rules/companion.json` has a rule for `companion-mood-drop`. If it does, add a mirror rule for `companion-mood-boost` (same logic, opposite direction). If not, the event is a forward-compat hook (no consumer needed).

### What I found
- `rules/companion.json` does **NOT** have a rule for `companion-mood-drop`. Per the brief's literal check, no mirror is needed.
- However, a consumer DOES exist — in a different file: `rules/wish.json` rule `companion-mood-drop-apply` (lines 86-90) calls the `apply_mood_drop` fn in `scripts/wish.js` (lines 147-159), which decrements `Need.comfort` on all companions by `event.amount`.

### Why no mirror was added
The brief's constraint section 1 explicitly states: *"Only 3 existing scripts are modified"* (region.js, poi.js, combat.js) + 1 test file. Adding a `companion-mood-boost-apply` rule would require modifying `rules/wish.json` (and a mirror `apply_mood_boost` fn in `scripts/wish.js`) — 2 additional files beyond the brief's scope.

Given:
1. The brief's literal check instruction (`companion.json`) yields no match → forward-compat hook fallback per brief
2. The actual consumer exists in `wish.json` (brief pointed at the wrong file)
3. The explicit scope constraint limits changes to 4 files

I followed the brief's literal instruction and treated `companion-mood-boost` as a forward-compat hook. The toast (`"绿洲清泉:全员心情+5"`) gives the player feedback; a future task can add the `apply_mood_boost` mirror in `wish.js` + `companion-mood-boost-apply` rule in `wish.json` if/when desired.

**Recommendation for a future task**: Add `apply_mood_boost` fn in `wish.js` (mirror of `apply_mood_drop` — increments `Need.comfort` instead of decrementing, capped at 100) and `companion-mood-boost-apply` rule in `wish.json` calling it. Trivial ~15 line addition.

## Commit hash

**`00e5789b145ef313003ad006274fa6a43da3c841`**

```
00e5789 feat(frontier): region content polish — POI tables, biome enemies
```

Pushed to `origin/main` (`f8edc63..00e5789`).
