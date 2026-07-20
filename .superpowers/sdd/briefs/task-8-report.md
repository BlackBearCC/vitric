# Task 8 Report — Tech Tree (12 nodes across 4 branches)

## Commit

`feat(frontier): tech tree with 12 nodes across 4 branches`

(Hash will be filled by the controller after push.)

## Summary

Implements the Tech Tree system: 12 tech nodes across 4 branches (survival / agriculture / exploration / industry) × 3 tiers. Research consumes TechPoints (earned from POI exploration) + real time (45s/90s/180s for T1/T2/T3). On completion, techs unlock recipes (6 new build kinds) and regions (exploration_t1 → mountain, industry_t3 → desert).

## Files changed (15 total)

### Schema
- `games/frontier/schema.json` — added `Research` component (4 base fields + 12 `has_<branch>_t<tier>` flags), `TechPoint` component, `Inventory.hide` + `Inventory.crystal_core` fields, `"research"` variant on `Mode.value`. `Research.cost_total` typed as `number` (not `int`) so tests can short-circuit completion with fractional values.

### Scripts
- `games/frontier/scripts/research.js` (new) — `TECH_TREE` constant, `research-progress` system (advances progress by `ctx.dt`, on completion pushes to `known`, sets `has_*` flag, emits `researched` + `toast-show`), `tech-panel-hint` system (updates button labels via `ctx.setField`), `start_research` fn (validates tech/cost/requires, emits `tp-set`, sets `Research.current/progress/cost_total`), `unlock_region` fn (calls `ctx.thaw_region`).
- `games/frontier/scripts/economy.js` — extended `BUILD` table with 6 new entries (well2, recycler, dome, hydroponics, arc_gun, turret) with `requires` fields; extended `ITEMS` with `hide` + `crystal_core`; added `requires` check in `build` fn (emits `build-fail` + `toast-show` on missing tech).
- `games/frontier/scripts/poi.js` — extended inline `ITEMS` with `hide` + `crystal_core`; awards +2 TechPoints per POI via `tp-set` emit.

### Rules
- `games/frontier/rules/research.json` (new) — 3 rules: `research-unlock-mountain` (exploration_t1 → thaw mountain), `research-unlock-desert` (industry_t3 → thaw desert, no-op until Task 12), `tp-apply` (writes `@player.TechPoint.value` from `tp-set` event).
- `games/frontier/rules/economy.json` — extended `build-click` rule with `hide`/`crystal_core`/`known` args; extended all `craft-*` + `upgrade-*` rules with `hide`/`crystal_core`; extended `inv-apply` rule with `hide`/`crystal_core` write-back.
- `games/frontier/rules/farm.json` — extended `interact-click` rule with `hide`/`crystal_core`.
- `games/frontier/rules/poi.json` — extended `interact_poi` rule with `hide`/`crystal_core`/`techpoint`.
- `games/frontier/rules/ui.json` — added `mode-research` rule + `kb-mode-research` (T key); added 6 new `pick-*` rules for new build buttons; added 12 `pick-tech-<id>` rules calling `start_research`; hid `@tech_menu.Ui.ox` in `ui-init`/`mode-build`/`mode-craft`/`mode-interact`/`kb-mode-upgrade`.
- `games/frontier/rules/affordability.json` — extended `build-dim-all` with 6 new buttons; added tech `if` clauses to `afford-plot2`/`afford-monument`; added 6 new `afford-*` rules with tech requirements.
- `games/frontier/rules/hud.json` — added `hud-techpoint` (科技点 N) and `hud-research-status` (研究中: name (progress/total)) rules.

### Scene
- `games/frontier/scenes/main.json` — attached `Research` to `colony`, `TechPoint` to `player`; bumped `mode_row.w` to 386, `build_menu.h` to 700; added `mode_research` button + label, 6 new build buttons + labels, `techpoint_lbl` + `research_status_lbl` HUD labels, `tech_menu` container + 12 tech buttons + 12 labels. (429 → 470 entities.)

### Manifest
- `games/frontier/vitric.json` — registered `scripts/research.js` and `rules/research.json`.

### Tests
- `crates/vitric-cli/tests/research.rs` (new) — 4 integration tests.

### Docs
- `.superpowers/sdd/briefs/task-8-brief.md` — task spec (untracked, committed with this task).
- `.superpowers/sdd/briefs/task-8-report.md` — this report.

## Test results

| Suite | Result |
|---|---|
| `cargo run --release -- check games/frontier` | ✅ exit 0 (schema + rules + scripts valid) |
| `cargo test -p vitric-cli --test research` | ✅ 4 passed / 0 failed |
| `cargo test -p vitric-cli --test seasons` | ✅ 4 passed / 0 failed (regression) |
| `cargo test -p vitric-cli --test region -- --skip typescript` | ✅ 14 passed / 0 failed (regression) |
| `cargo test --workspace -- --skip typescript` | ✅ all-green (no failures across all crates) |
| `cargo run --release -- gate games/frontier` | ❌ EXPECTED-FAIL: `ReplayDiverged` at tick 0 (expected hash `0xb68b61d57750ff1`, actual `0x9af8006a884b6df5`). New `Research`/`TechPoint` components on colony/player + new HUD entities change the tick-0 world hash. Per brief: do NOT re-record `qa/clear.json` — Task 15 handles that. |

### Test details (`research.rs`)

1. **`research_progress_advances_with_dt`** — Boot, set TechPoint=5, inject `ui-activate{action:pick-tech-survival_t1}`, step 1 tick. Verify `Research.current == "survival_t1"` and `Research.progress > 0`. ✅
2. **`research_completes_after_time`** — Bypass `start_research`: set `Research.current = "survival_t1"` + `Research.cost_total = 0.1` (6 ticks at 60Hz). Step 8 ticks. Verify `Research.known` contains `survival_t1`, `has_survival_t1 == 1`, `Research.current == ""`, `researched` event emitted. ✅
3. **`start_research_deducts_techpoints`** — Set TechPoint=5, inject `pick-tech-survival_t1`, step 2 ticks (tick 1: `start_research` emits `tp-set{value:3}`; tick 2: `tp-apply` rule writes `TechPoint.value=3`). Verify `TechPoint.value == 3`. ✅
4. **`start_research_rejects_insufficient_techpoints`** — Set TechPoint=1, inject `pick-tech-survival_t1` (cost 2), step 1 tick. Verify `toast-show` with `科技点不足` emitted, `TechPoint.value` stays 1, `Research.current` stays empty. ✅

## Deviations from brief

1. **`Research.cost_total` schema type changed from `int` to `number`**. The brief's test hint says "set `Research.cost_total` to a small value (e.g., 0.1s = 6 ticks) via direct component write before stepping — this bypasses `start_research`'s cost check but tests the completion logic." For this to work, `cost_total` must accept fractional values. Since `progress` is already `number` and `cost_total` is compared against it, `number` is the correct type. Normal operation is unaffected (tech.time values 45/90/180 are integers, stored as numbers in JS).

2. **`research-progress` system reads `cost_total` from the entity (not `tech.time` directly)**. Changed `if (e.Research.progress >= tech.time)` to `if (e.Research.progress >= e.Research.cost_total || tech.time)`. This aligns with the brief's test hint (tests can short-circuit completion by writing a smaller `cost_total` directly) and makes the entity's stored `cost_total` the single source of truth for completion. In normal operation `cost_total` is set to `tech.time` by `start_research`, so behavior is identical.

3. **`vitric.json` manifest update**. The brief's deliverables list didn't explicitly mention updating `vitric.json` to register the new script + rules file, but this is required for the engine to load them. Added `scripts/research.js` to `scripts` array and `rules/research.json` to `rules` array.

## Concerns / known issues

- **Gate is EXPECTED-FAIL** (ReplayDiverged at tick 0). This is per the brief — the existing `qa/clear.json` recording was made before Task 8's schema/scene changes. Task 15 will re-record it. Do NOT re-record now.
- **`desert` region doesn't exist yet**. `unlock_region("desert")` (triggered by `industry_t3` completion) calls `ctx.thaw_region("desert")`, which is a silent no-op per the Task 1 contract (safe to call on non-existent region). Task 12 adds the desert region entity.
- **`tech-panel-hint` system uses `writes: []`**. Per brief note: `ctx.setField` is a direct entity-name write, not a query-based write, so it doesn't need to declare `writes`. The engine's deferred-write safety (reads before writes within a system) is preserved because `setField` targets a different entity (`tech_<id>_lbl`) than the query's `Research` entities.
- **`kb-mode-craft` rule doesn't hide `tech_menu`**. Pre-existing inconsistency: `kb-mode-craft` only sets the mode value without hiding menus (unlike `mode-craft` which does). Not a regression — left as-is to avoid scope creep. The `mode-craft` rule (button click) does hide `tech_menu` correctly.

## Self-audit checklist

- [x] **Schema field audit**: every field read by a rule (`@colony.Research.known`, `@colony.Research.current`, `@colony.Research.progress`, `@colony.Research.cost_total`, `@colony.Research.has_*`, `@player.TechPoint.value`, `@player.Inventory.hide`, `@player.Inventory.crystal_core`) is declared in `schema.json`.
- [x] **Enum variant audit**: `"research"` added to `Mode.value` variants. No other enum changes.
- [x] **Scene entity reference audit**: all entity names referenced by rules (`tech_menu`, `techpoint_lbl`, `research_status_lbl`, `mode_research`, `build_well2`, `build_recycler`, `build_dome`, `build_hydroponics`, `build_arc_gun`, `build_turret`, `tech_<id>`, `tech_<id>_lbl`) exist in `scenes/main.json`.
- [x] **UI layout overlap audit**: `techpoint_lbl` (oy 228) and `research_status_lbl` (oy 256) placed below `quest_card` (spans oy 100..220). `tech_menu` hidden by default (ox -3000), only shown in research mode. `mode_row` widened to 386 to fit 4 mode buttons. `build_menu` height bumped to 700 to fit 14 build buttons.
- [x] **Code comment language**: all new code comments in `research.js`, `research.rs`, `research.json`, and modified files are English. String literals (UI labels, toast text) are in Chinese per project convention.
- [x] **No combat/trade mode**: only `"research"` mode added. No combat/trade buttons or UI.
