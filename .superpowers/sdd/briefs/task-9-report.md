# Task 9 — Companions Expansion: Report

## Commit
- Hash: `f3a6ef5f95d056531c36f79590a32cd44f12a8d7`
- Message: `feat(frontier): companions expansion — 6 roles, 12 pool, wish templates`
- Remote: pushed to `origin/main` (SSH: `git@github.com:BlackBearCC/vitric.git`)

## Files changed (7 files, +331 / -40)
| File | Change |
|---|---|
| `games/frontier/schema.json` | Add `Persona.role` enum (6 variants) and `Colony.collective_wish_done` int field |
| `games/frontier/scenes/main.json` | Add `role:"builder"` to companion (Pip); `role:"farmer"` to drifter (Lio); add `collective_wish_lbl` HUD entity (parent=ui, oy=336) |
| `games/frontier/scripts/companion.js` | DRIFTER_POOL 6→12 (2 per role, each entry has `role`); `consumeDrifter` passes role to Persona; `inviteAnyNearby` reads role and emits in event; `companion-contribution` system: role-based dispatch replacing random pick (builder/farmer/explorer/guard/trader/scholar + forward-compat hooks `guard-patrol`, `trade-available`, `explore-bonus`); `WISH_TEMPLATES` 3→6 + `wishesForRole()` |
| `games/frontier/scripts/wish.js` | Add `collective-wish-check` system: when `Colony.food_i >= 50` and `collective_wish_done == 0`, mark done, +10 affinity to all companions (via `companion_handles`), emit `collective-wish-fulfilled` + `toast-show` |
| `games/frontier/rules/companion.json` | `companion-invite-process` rule: pass `event.role` to `consumeDrifter`; split `drifter-cadence` into `drifter-cadence-normal` (2-day, stage != 兴旺) + `drifter-cadence-fast` (1-day, stage == 兴旺); cap raised 4→8 |
| `games/frontier/rules/hud.json` | Add `hud-collective-wish-pending` and `hud-collective-wish-done` rules (label swaps based on `collective_wish_done` state) |
| `crates/vitric-cli/tests/companions.rs` | New integration test file: 4 tests covering builder contribution (+plank), scholar contribution (tp-set → tp-apply → +TechPoint.value), collective wish firing at food=50, and one-time-only guard |

## Test results
| Test | Result |
|---|---|
| `cargo run --release -- check games/frontier` (schema) | ✅ exit 0 |
| `cargo test -p vitric-cli --test companions` (4 tests) | ✅ 4 passed |
| `cargo test -p vitric-cli --test research` (regression, 4 tests) | ✅ 4 passed |
| `cargo test -p vitric-cli --test seasons` (regression, 4 tests) | ✅ 4 passed |
| `cargo test -p vitric-cli --test region -- --skip typescript` (regression, 14 tests) | ✅ 14 passed |
| `cargo test --workspace -- --skip typescript` | ✅ all-green (every crate's test suites pass) |
| `cargo run --release -- gate games/frontier` | ⚠️ EXPECTED-FAIL: `check` passes, `playthrough:qa/clear.json` diverges at tick 0 (hash mismatch — expected because scene/scripts/rules changed). Per brief, do NOT re-record `qa/clear.json`; Task 15 handles it. |

## Self-audit (`.superpowers/sdd/review-checklist.md`)

### 1. Schema field audit ✅
All fields read by rules (`@entity.Comp.field`) or accessed via `ctx.getField/setField` in modified scripts are declared in `schema.json`:
- `Persona.role` — declared (enum, 6 variants)
- `Colony.collective_wish_done`, `.food_i`, `.food_rate`, `.companion_handles` — declared
- `Need.affinity`, `.affinity_i`, `.contribution_timer` — declared
- `Inventory.plank`, `.ore`, `.fiber`, `.wheat` — declared
- `TechPoint.value` — declared
- `Mood.value` — declared
- `UiLabel.content` — declared

### 2. Enum variant audit ✅
- All `Persona.role` values used in `companion-contribution` switch (builder/farmer/explorer/guard/trader/scholar) are in `schema.json` `Persona.role.variants`.
- `Mood.value` is text (not enum), no variant check needed.
- No new enum literals in rules.

### 3. Scene entity reference audit ✅
- `@collective_wish_lbl` — exists in `scenes/main.json` (added in this task).
- `@colony`, `@player`, `@research_status_lbl`, `@hud_food_lbl` etc. — all pre-existing.

### 4. UI layout overlap audit ✅
- `collective_wish_lbl`: parent=`ui`, anchor=`top-left`, oy=336, h=24, range=[336, 360].
- Nearest preceding sibling: `forecast_lbl` (parent=`ui`, oy=258, h=28, end=286). Gap = 50px.
- No interior overlap.

### 5. Standard checks ✅
- Schema check exits 0.
- All new `//` comments in English; string literals (toast text, HUD labels) keep Chinese as-authored.
- No fake APIs (`ctx.singleton`, `Math.random`, etc.); only real engine APIs used (`ctx.getField`, `ctx.setField`, `ctx.emit`, `ctx.ask`, `ctx.random`, `ctx.dt`, `vitric.fn`, `vitric.system`).
- No dead code; every new fn/system has a caller.
- Commit message follows `feat(frontier): <summary>` convention.
- Only in-scope files modified.

## Deviations from brief

### Deviation 1: `WISH_TEMPLATES` not duplicated in `wish.js`
**Brief said**: "DRY violation: `WISH_TEMPLATES` + `wishesForArchetype` are duplicated in BOTH `companion.js` and `wish.js` (existing pattern). Update BOTH copies."

**Reality**: The brief's premise was wrong. `WISH_TEMPLATES` was NEVER in `wish.js` prior to this task. When I added the duplicate block per the brief's explicit instruction, the schema check failed:
```
vitric 错误: 脚本 scripts/wish.js 加载失败: Error: redeclaration of 'WISH_TEMPLATES'
```

**Root cause**: QuickJS scripts share a single global scope (verified by reading `crates/vitric-script/src/lib.rs` — `eval_file` calls `ctx.eval(source)` on the shared context). `companion.js` is loaded before `wish.js` (alphabetical), so its `const WISH_TEMPLATES` already lives in the global when `wish.js` tries to declare it again — redeclaration error.

**Fix applied**: Did NOT duplicate `WISH_TEMPLATES` / `wishesForRole` / `wishesForArchetype` in `wish.js`. Added a comment explaining that the symbols are inherited from `companion.js`'s global scope. `wish.js` doesn't call any of these functions anyway (only `companion.js`'s `consumeDrifter` does).

### Deviation 2: `collective_wish_lbl` placement context
**Brief said**: `research_status_lbl` is at oy=308; place `collective_wish_lbl` at oy=336 to avoid overlap.

**Reality**: `research_status_lbl` is at oy=256 (not 308). The actual nearest preceding sibling at parent=`ui` is `forecast_lbl` at oy=258 with end=286. oy=336 still leaves a 50px gap, so no overlap either way. The placement is correct; only the brief's cited context was slightly off.

### Deviation 3: Builder test setup
**Brief said**: "modify the existing Pip companion, step 1 tick, verify plank increased."

**Reality**: The first tick consumes the `start` event, which fires the `seed-start` rule (sets `Inventory.plank = 6`). If I step only once from boot, the contribution fires but plank = 6 + 1 = 7 (not 1 as the test asserts).

**Fix applied**: Step once first to let `seed-start` settle, then `prime_companion` + set `plank=0` + step + verify `plank=1`. This isolates the contribution increment from the seed-start baseline.

## Concerns / risks

1. **`qa/clear.json` is dirty** (gate EXPECTED-FAIL). Per brief, this is expected — Task 15 will re-record after all 15 tasks land. Not a blocker.

2. **Forward-compat hooks have no consumers yet**. The `guard-patrol`, `trade-available`, `explore-bonus` events are emitted but no rule consumes them. This is by design (Tasks 10/11/12 will consume them). No functional impact — emitted events without consumers are silently dropped.

3. **`DRIFTER_POOL` 12 entries, but cap is 8**. Per brief: cap raised from 4 to 8 (not 12). The pool has headroom for future expansion. Drifter idx 0-7 are reachable; idx 8-11 are unreachable in normal play but defined for future cap raises.

4. **`companion-contribution` scholar path uses `tp-set` event**. The `tp-set` event is consumed by the existing `tp-apply` rule in `rules/research.json`, which writes `@player.TechPoint.value`. This reuses the existing techpoint write-back pipeline (same one `start_research` uses). Verified working by `companion_contribution_role_scholar_grants_techpoint` test.

5. **Collective wish buff uses `Need.affinity`** (not `affinity_i`). The system writes both `Need.affinity` (float) and `Need.affinity_i` (rounded int) to keep them in sync — matches the existing pattern in `wish.js`'s `advance_wish` fn.
