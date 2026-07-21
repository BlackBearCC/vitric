# Task 12 — Map Expansion & Region Content — Implementation Report

## Summary

Implemented Task 12 (Frontier sandbox map expansion). The world now has 5 regions
(home / wild / mountain / swamp / desert); 3 are dormant and unlock via gameplay
conditions. Region content (tiles, resource nodes, POIs) is generated on thaw using
E3 seeded substreams — bit-identical regardless of when the thaw happens. Camera
`world_bounds` clamps the player to discovered regions. Also fixes a pre-existing
dead `unlock_region` fn from Task 8 by adding the missing `ctx.thaw_region` JS bridge.

## Files changed

### Engine (3 files)
| File | Lines (+/-) |
|---|---|
| `crates/vitric-script/src/lib.rs` | +13 / -0 |
| `crates/vitric-script/src/prelude.js` | +9 / -0 |
| `crates/vitric-sim/src/sim.rs` | +48 / -0 |

### Game data (7 files)
| File | Lines (+/-) |
|---|---|
| `games/frontier/schema.json` | +4 / -0 |
| `games/frontier/scripts/world.js` | +12 / -2 |
| `games/frontier/scripts/region.js` | +209 / -0 (NEW) |
| `games/frontier/rules/region.json` | +13 / -0 (NEW) |
| `games/frontier/rules/research.json` | +0 / -6 |
| `games/frontier/scenes/main.json` | +1 / -1 (single-line JSON; 4 entities added) |
| `games/frontier/vitric.json` | +2 / -0 |

### Tests (1 file)
| File | Lines (+/-) |
|---|---|
| `crates/vitric-cli/tests/region.rs` | +152 / -0 |

**Total: 11 files, +463 / -9 (excludes the brief markdown which is not committed).**

## Tests run

### `cargo run --release -- check games/frontier`
- **PASS** — schema validation succeeds; all systems registered correctly
  (including the new `region-approach-check` system).

### `cargo test -p vitric-cli --test region`
- **17 passed; 0 failed** (3 new + 14 existing).
  - New: `ctx_thaw_region_bridge_works_from_js` — PASS
  - New: `region_content_deterministic_across_thaw_timing` — PASS
  - New: `camera_world_bounds_clamps_player_position` — PASS
  - Existing 14 tests still pass (no regressions).

### `cargo test --workspace`
- **All test files pass except `crates/vitric-cli/tests/typescript.rs`**.
- The 2 typescript test failures are **pre-existing environment issues** unrelated
  to Task 12: the test panics with `测试需要 esbuild：仓库里跑 cd mcp && npm install，
  或设 ESBUILD_BIN` because `esbuild` is not installed on this machine. Confirmed
  by `which esbuild` (not found) and `ls mcp/node_modules/.bin/esbuild` (no such
  file). My changes do not touch any TypeScript-related code.

## Deviations from brief

### Deviation 1: `region-approach-check` system uses `query: ["Player"]` instead of `query: []`

**Brief specified**: `vitric.system("region-approach-check", { query: [], writes: [] }, …)`
with the design note: "system has `query: []` (no entity batch) — it reads everything via
`ctx.getField`."

**Issue discovered**: The prelude (`crates/vitric-script/src/prelude.js:18`) explicitly
rejects empty query arrays:
```javascript
if (!decl || !Array.isArray(decl.query) || decl.query.length === 0) {
  throw new Error("vitric.system(\"" + name + "\"): 第二个参数必须含非空 query 数组…");
}
```
Also, even if the prelude accepted empty queries, `World::query(&[])` returns ALL
non-dormant entities (because `required.iter().all(...)` on an empty iterator is vacuously
true) — this would iterate 490+ entities per tick doing nothing with them, which is
wasteful.

**Resolution**: Use `query: ["Player"]` — matches only the player entity. The system
body still ignores the entities argument and reads everything via `ctx.getField("player",
…)`. This satisfies the prelude validation, doesn't waste cycles iterating the whole
world, and preserves the brief's intent (deferred ops via `ctx.thaw_region` /
`ctx.emit`, no query-based writes).

**Code change**: in `games/frontier/scripts/region.js`:
```javascript
vitric.system("region-approach-check", { query: ["Player"], writes: [] }, (entities, ctx) => { … });
```
A comment block above the system documents the deviation.

### No other deviations

All other brief specifications were followed faithfully:
- Engine: `__thawRegion` native fn registered exactly as specified (mirrors `__randomStreamNext`).
- Engine: `ctx.thaw_region` JS bridge added to prelude exactly as specified.
- Engine: Camera `world_bounds` clamping added at the end of `integrate_motion` exactly
  as specified (after the main motion loop, before `follow_camera`).
- `schema.json`: `Camera.world_bounds` field added as `text` with `default: ""`.
- `world.js`: `genWild` extended to cover x16..59, y0..29 + 4 new resource nodes.
- `region.js`: `REGION_SPECS`, `REGION_CONTENT`, `gen_region_content`, `update_camera_bounds`,
  `region-approach-check` system, `checkUnlockCondition` helper — all match the brief.
- `rules/region.json`: 2 rules (`region-thaw-content`, `region-thaw-bounds`) exactly as
  specified.
- `rules/research.json`: `research-unlock-desert` removed (kept `research-unlock-mountain`
  + `tp-apply`) — exactly as specified.
- `scenes/main.json`: 4 region markers added (home/wild/swamp/desert; mountain already
  existed) + camera `world_bounds: "[0,0,60,30]"` — exactly as specified. Used Python
  with `separators=(",",":")`, `ensure_ascii=False` to keep the file single-line.
- `vitric.json`: `scripts/region.js` and `rules/region.json` added to the arrays —
  exactly as specified.
- Tests: 3 tests added at the end of `region.rs` exactly as specified in §5.1-5.3.

## Schema field audit

**Result: PASS** — every field read by a rule OR accessed via `ctx.getField` / `ctx.setField`
is declared in `schema.json`.

### New field declared:
- `Camera.world_bounds` (text, default "") — added in §3.2 ✓

### Existing fields read by new code (verified present in schema):
- `Region.state`, `Region.id`, `Region.biome`, `Region.discovered` ✓
- `Region.anchor_x`, `anchor_y`, `w`, `h`, `dormant_ticks`, `spawn_timer` ✓
- `Camera.follow`, `Camera.x`, `Camera.y`, `Camera.world_bounds` ✓
- `Position.x`, `Position.y`, `Velocity.x`, `Velocity.y` ✓
- `Colony.companion_handles` (list of text) ✓
- `Colony.Faction.tier_caravan` (text) ✓
- `Colony.Research.has_exploration_t1`, `has_industry_t3` (int) ✓
- `Companion` + `Persona.role` (enum, includes "explorer") ✓
- `Node.kind`, `Node.left`, `Node.max`, `Node.cooldown` ✓
- `Poi.kind`, `Poi.state`, `Poi.cooldown`, `Poi.reward_table` ✓
- `Cell.kind`, `Sprite.*`, `Text.*` ✓

All fields used by the new code are declared. No undeclared field issues.

## Verification summary

```
cargo run --release -- check games/frontier    PASS
cargo test -p vitric-cli --test region         17 passed / 0 failed
cargo test --workspace                          All pass except pre-existing
                                                typescript.rs failures (missing
                                                esbuild binary; not a Task 12
                                                regression)
```

## Commit

- Hash: `50776c06f47038bb82318e9aa70e75f10a3532b4`
- Branch: `main`
- Pushed to: `git@github.com:BlackBearCC/vitric.git`
- 11 files changed, 468 insertions(+), 14 deletions(-)

```
git commit -m "feat(frontier): map expansion with 5 regions

5 regions (home/wild/mountain/swamp/desert), 3 dormant with unlock
conditions. Region content generated on thaw using E3 seeded substreams
(deterministic regardless of thaw timing). Camera world_bounds clamps
player to discovered regions. Adds ctx.thaw_region JS bridge (fixes
pre-existing dead unlock_region fn from Task 8)."
```

`git push origin main` succeeded: `f4d2ceb..50776c0  main -> main`.
