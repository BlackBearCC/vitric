# Task 9 (Companions Expansion) — Code Review

**Commit reviewed:** `f3a6ef5f95d056531c36f79590a32cd44f12a8d7`
**Base:** `888057d`
**Diff stat:** 7 files, +331 / -40
**Reviewer independence:** Diff read from git; every modified file re-read in current state; schema/rule/scene audits re-run from scratch; tests re-executed.

---

## Verdict: NEEDS FIXES

One Important finding blocks approval: the UI layout overlap audit (checklist §4) was performed against the wrong sibling group, missing a real interior overlap between `collective_wish_lbl` and `build_menu` that is visible at game start. The fix is a one-line documentation note (or a position tweak) — not a functional bug. Once documented/moved, the task can be re-approved without re-running tests.

All other audit sections (schema, enum, scene entity refs, standard checks) PASS. All tests pass (4/4 new + 22/22 regression). Gate EXPECTED-FAIL mode confirmed.

---

## Critical findings (blocks approval)

None.

---

## Important findings (should fix before approval)

### 1. UI layout overlap with `build_menu` missed in self-audit (checklist §4 ❌)

**Location:** `games/frontier/scenes/main.json` — `collective_wish_lbl` entity (oy=336).

**Description:**
`collective_wish_lbl` (added by Task 9) is at:
- `anchor: "top-left"`, `parent: "ui"`, `ox: 24`, `oy: 336`, `w: 280`, `h: 24`
- y-range = [336, 360], x-range = [24, 304]

The existing `build_menu` (pre-Task-9) is at:
- `anchor: "top-left"`, `parent: "ui"`, `ox: 24` (scene) / `ox: 208` (after `start` event fires `mode-build-initial` rule in `rules/ui.json:6-12`), `oy: 176`, `w: 348`, `h: 700`
- y-range = [176, 876], x-range = [24, 372] (scene initial) or [208, 556] (after `start`)

**Overlap:** Interior overlap in BOTH x and y at game start:
- Scene initial (ox=24): x-overlap [24, 304], y-overlap [336, 360] ⊂ [176, 876].
- After `start` event (ox=208): x-overlap [208, 304], y-overlap [336, 360] ⊂ [176, 876].

**Why it's visible at startup:**
- `uistate.Mode.value = "build"` in scene (default mode).
- `rules/ui.json:6-12` rule `mode-build-initial` fires on the `start` event (tick 0), setting `@build_menu.Ui.ox = 208` — so the build_menu is visible at tick 0.
- Entity render order in `scenes/main.json`: `collective_wish_lbl` at index 360, `build_menu` at index 370. Later entities render on top, so `build_menu` renders OVER `collective_wish_lbl`, hiding it whenever the player is in build mode (the default).

**Why the implementer's self-audit missed it:**
The implementer's report (`.superpowers/sdd/briefs/task-9-report.md` lines 51-54) states:
> `collective_wish_lbl`: parent=`ui`, anchor=`top-left`, oy=336, h=24, range=[336, 360].
> Nearest preceding sibling: `forecast_lbl` (parent=`ui`, oy=258, h=28, end=286). Gap = 50px.

`forecast_lbl` is `anchor: "top-center"`, NOT `top-left`. The checklist §4 requires listing "all UI entities sharing the same `(anchor, parent)` pair" — i.e., the same anchor. The actual `anchor=top-left, parent=ui` siblings are: `mode_row` (oy=100, end=164), `build_menu` (oy=176, end=876), `craft_menu` (off-screen ox=-3000), `tech_menu` (off-screen ox=-3000). The implementer compared across anchor groups, which is incorrect.

**Suggested fix (pick one):**

(a) **Document as intentional overlap** (preferred — minimal change): add a note to the implementer's report and a `comment` on the `collective_wish_lbl` entity's Ui component (or a remark in `rules/hud.json`) stating: "collective_wish_lbl is occluded by build_menu while Mode.value=build (default); acceptable because build_menu is a temporary modal panel. Label is visible in interact/craft/tech/upgrade modes." Per checklist §4: "If overlap is intentional (e.g. a label on top of a panel), document it in the report" — this makes the overlap acceptable.

(b) **Move `collective_wish_lbl`** to a non-overlapping slot. Options:
   - `anchor: "top-right"`, `ox: -32`, `oy: 288` (just below `research_status_lbl` at oy=256, end=280; gap=8px). Matches the right-side HUD column pattern.
   - `anchor: "top-left"`, `ox: 24`, `oy: 884` (just below `build_menu`'s end=876; gap=8px). Keeps the left side but pushes it below the menu.

Either fix is small. Re-running tests is not required (no logic change).

---

## Minor findings (cosmetic, optional)

### 1. Test setup inconsistency between builder and scholar tests

**Location:** `crates/vitric-cli/tests/companions.rs:48-64` (builder) vs `:67-81` (scholar).

**Description:**
The builder test does an extra `sim.step(&mut rt).unwrap()` before priming "to let seed-start settle" (justified by the comment about `seed-start` setting `Inventory.plank = 6`). The scholar test does NOT do this settle step — it primes directly after boot. Both tests pass because each explicitly resets the relevant field (`Inventory.plank = 0` / `TechPoint.value = 0`) before the contribution step, so the settle step is not actually load-bearing.

Verified: `seed-start` rule exists at `rules/economy.json:73-83` and does set `Inventory.plank = 6` (and `ore=6, wood=8, fiber=2, seed=10, lamp=2`). So the implementer's comment is accurate, but the extra step is unnecessary given the explicit `plank=0` reset two lines later.

**Suggested fix:** Either remove the settle step from the builder test (matching the scholar test pattern), or add the same settle step to the scholar test for consistency. Not blocking.

### 2. Implementer's report cites wrong "nearest preceding sibling" for UI audit

**Location:** `.superpowers/sdd/briefs/task-9-report.md:53`.

**Description:**
> Nearest preceding sibling: `forecast_lbl` (parent=`ui`, oy=258, h=28, end=286). Gap = 50px.

`forecast_lbl` is `anchor: "top-center"`, not `top-left`. The actual nearest preceding `anchor=top-left, parent=ui` sibling is `mode_row` (oy=100, end=164), with a gap of 176px. The 50px gap cited in the report is between entities in different anchor groups and is not the relevant comparison. (This is the same root cause as Important Finding #1.)

**Suggested fix:** After applying Important Finding #1's fix, update the report's UI audit section to reference the correct sibling group.

---

## Approved deviations

### Deviation 1: `WISH_TEMPLATES` not duplicated in `wish.js`

**Brief said** (lines 282-286): "`WISH_TEMPLATES` + `wishesForArchetype` are duplicated in BOTH `companion.js` and `wish.js` (existing pattern). Update BOTH copies."

**Implementer's reasoning:** The brief's premise was wrong — `WISH_TEMPLATES` was never in `wish.js` prior to Task 9. When the implementer initially added the duplicate per the brief, schema check failed with `redeclaration of 'WISH_TEMPLATES'` because QuickJS loads all scripts into a single shared global scope. Fix: don't duplicate; add a comment in `wish.js` explaining the shared global.

**Reviewer's verification:**
- (a) **Engine really is a shared global:** `crates/vitric-script/src/lib.rs:107-127` constructs `ScriptEngine` with a single `Context::full(&runtime)` stored in `self.context`. The `load` method (line 181-185) calls `eval_file(file, source)` for every script, and `eval_file` (line 523-532) calls `self.context.with(|ctx| ctx.eval::<(), _>(source...))` on the SAME `self.context`. So all scripts share one global object scope. Re-declaring `const WISH_TEMPLATES` in a second file would raise a redeclaration error. ✅
- (b) **`wish.js` does NOT call `WISH_TEMPLATES` / `wishesForRole` / `wishesForArchetype`:** Read the full file (`games/frontier/scripts/wish.js`). It defines `advance_wish`, `triggerWishMemory`, `onWishMemoryReply`, `apply_mood_drop`, `wish_food_check`, `collective-wish-check`. None of these reference `WISH_TEMPLATES` or its helper fns. ✅
- (c) **Comment in `wish.js` is accurate:** Lines 12-13 read: "Note: WISH_TEMPLATES, wishesForRole(), wishesForArchetype() live in companion.js (loaded first). QuickJS scripts share a single global scope, so wish.js can call them directly without re-declaring." Verified accurate. ✅

**Verdict:** Approved. The implementer correctly identified a factual error in the brief and applied the right fix. The brief's "DRY violation" premise was a misreading of the codebase.

### Deviation 2: Builder test setup with extra "settle" step

**Brief said** (lines 408-410): "modify the existing Pip companion, step 1 tick, verify plank increased."

**Implementer's reasoning:** The first tick consumes the `start` event, which fires the `seed-start` rule (`rules/economy.json:73-83`) setting `Inventory.plank = 6`. If the test stepped only once from boot, the contribution would fire but plank = 6 + 1 = 7, not 1. Fix: step once to settle seed-start, then `prime_companion` + set `plank=0` + step + verify `plank=1`.

**Reviewer's verification:**
- `seed-start` rule at `rules/economy.json:73-83` confirmed — it sets `Inventory.plank = 6` on the `start` event. ✅
- Test logic at `crates/vitric-cli/tests/companions.rs:48-64` matches the description. The `set_field(plank, 0)` after the settle step guarantees a clean baseline regardless of seed-start's effect. ✅
- Test passes (4/4). ✅

**Verdict:** Approved. Test logic is sound and deterministic. The settle step is technically redundant given the explicit `plank=0` reset (see Minor Finding #1), but it doesn't break anything.

### Deviation 3: `research_status_lbl` position cited as oy=308 in brief, actual is oy=256

**Brief said** (line 63): "Position: below `research_status_lbl` (which is at oy:308 + h:24 = 332, so oy:336)."

**Implementer's reasoning:** `research_status_lbl` is at oy=256 (not 308). The actual nearest visible top-center HUD sibling is `forecast_lbl` at oy=258, end=286. Placing `collective_wish_lbl` at oy=336 still leaves a 50px gap from `forecast_lbl`, so the placement is correct; only the brief's cited context was slightly off.

**Reviewer's verification:**
- `research_status_lbl` in `scenes/main.json`: `anchor: "top-right"`, `parent: "ui"`, `ox: -32`, `oy: 256`, `w: 260`, `h: 24`. Confirmed oy=256, not 308. ✅
- `forecast_lbl` in `scenes/main.json`: `anchor: "top-center"`, `parent: "ui"`, `oy: 258`, `h: 28`, end=286. Confirmed. ✅
- `collective_wish_lbl` at oy=336 leaves a 50px gap from `forecast_lbl` (end=286). ✅ (But this comparison is across anchor groups — see Important Finding #1.)

**Verdict:** Approved. The brief's cited position was wrong; the implementer's actual placement at oy=336 matches the brief's specified value. The cross-anchor comparison in the implementer's report is the root cause of Important Finding #1.

---

## Audit results

### Section 1: Schema field audit — **PASS** ✅

Cross-checked every `@entity.Comp.field` reference in new/modified rules and every `ctx.getField`/`ctx.setField` call in new/modified JS against `games/frontier/schema.json`:

**New fields declared:**
| Field | Location in schema | Type |
|---|---|---|
| `Persona.role` | `schema.json:783-787` | enum (6 variants) |
| `Colony.collective_wish_done` | `schema.json:732-735` | int (default 0) |

**Pre-existing fields used by new code (verified declared):**
| Field | Location in schema |
|---|---|
| `Colony.food_i` | `schema.json:530-533` |
| `Colony.companion_handles` | `schema.json:670-675` |
| `Colony.stage` | `schema.json:546-549` |
| `Colony.food_rate` | `schema.json:514-517` |
| `Colony.next_drifter_day` | `schema.json:554-557` |
| `Colony.drifters_spawned` | `schema.json:562-565` |
| `Colony.target_drifter` | `schema.json:582-585` |
| `Inventory.ore/wood/fiber/seed/wheat/plank/chair/lamp` | `schema.json:383-414` |
| `TechPoint.value` | `schema.json:981-984` |
| `Need.affinity/affinity_i/contribution_timer` | `schema.json:828-851` |
| `Mood.value` | `schema.json:792-795` |
| `UiLabel.content` | `schema.json:126-131` |

**New rule field references (hud.json + companion.json):** All OK — see automated grep output in reviewer's working notes (20/20 declared).

**New `ctx.getField/setField` calls in `collective-wish-check` (wish.js):** All OK — 5/5 declared (`Colony.collective_wish_done`, `Colony.companion_handles`, `Need.affinity`, `Need.affinity_i`).

**New `ctx.getField/setField` calls in `companion-contribution` + `inviteAnyNearby` + `consumeDrifter` (companion.js):** All OK — verified `@player.Inventory.plank/ore/fiber/wheat`, `@player.TechPoint.value`, `Colony.food_rate`, `<drifter_id>.Persona.role` all declared. (Two `[MISS]` hits in the automated grep were false positives from string concatenation `"Inventory." + which` in the pre-existing fallback case — the actual fields `Inventory.ore/wood/fiber` are all declared.)

### Section 2: Enum variant audit — **PASS** ✅

- `Persona.role` enum has exactly 6 variants: `["builder", "farmer", "explorer", "guard", "trader", "scholar"]` (`schema.json:785`). Matches brief exactly. ✅
- `Mode.value` enum NOT modified by Task 9 — still 5 variants `["build", "craft", "interact", "upgrade", "research"]` (`schema.json:429-435`). Brief says "No new mode" — confirmed. ✅
- No `set` to an enum field with a literal value in the new rules (the new HUD rules set `UiLabel.content` which is `text`, not `enum`).
- No `ctx.setField` to an enum field with a literal value in the new JS code (the `collective-wish-check` sets `Colony.collective_wish_done` which is `int`, not `enum`; sets `Need.affinity`/`affinity_i` which are `number`/`int`).

### Section 3: Scene entity reference audit — **PASS** ✅

- `companion` entity (Pip) — `Persona.role = "builder"` ✅ (verified in `scenes/main.json`)
- `drifter` entity (Lio) — `Persona.role = "farmer"` ✅ (verified in `scenes/main.json`)
- `collective_wish_lbl` entity exists in `scenes/main.json` ✅
  - `Ui`: `anchor: "top-left"`, `parent: "ui"`, `ox: 24`, `oy: 336`, `w: 280`, `h: 24` ✅ (matches brief)
  - `UiLabel`: `content: "共识: 粮储 50 (未达成)"`, `size: 18`, `color: "#ffffff"`, `align: "start"` ✅ (matches brief)
- Rules referencing `@collective_wish_lbl`: `hud-collective-wish-pending` and `hud-collective-wish-done` in `rules/hud.json:106-122`. Entity exists. ✅
- `@colony`, `@player` — pre-existing entities, still present. ✅

### Section 4: UI layout overlap audit — **FAIL** ❌ (see Important Finding #1)

Listing all UI entities sharing `(anchor="top-left", parent="ui")`:

| Entity | ox | oy | w | h | y-range | x-range | visible? |
|---|---|---|---|---|---|---|---|
| `mode_row` | 24 | 100 | 386 | 64 | [100, 164] | [24, 410] | yes |
| `build_menu` | 24 (scene) / 208 (after `start`) | 176 | 348 | 700 | [176, 876] | [24, 372] or [208, 556] | yes (default mode=build) |
| `craft_menu` | -3000 | 176 | 250 | 260 | [176, 436] | off-screen | no |
| `tech_menu` | -3000 | 176 | 348 | 400 | [176, 576] | off-screen | no |
| `collective_wish_lbl` | 24 | 336 | 280 | 24 | [336, 360] | [24, 304] | yes |

**Overlaps:**
- `collective_wish_lbl` y[336, 360] ⊂ `build_menu` y[176, 876]: INTERIOR OVERLAP. ❌
- `collective_wish_lbl` x[24, 304] ∩ `build_menu` x[24, 372] (scene) = [24, 304]: INTERIOR OVERLAP. ❌
- Same with `build_menu` at ox=208: x ∩ = [208, 304]: INTERIOR OVERLAP. ❌
- `mode_row` y[100, 164] vs `build_menu` y[176, 876]: no overlap (164 < 176). OK.

**Not documented** in implementer's report. Implementer's self-audit compared against `forecast_lbl` (anchor=top-center, wrong group).

### Section 5: Standard checks — **PASS** ✅

- `cargo run --release -- check games/frontier` exits 0 (schema check passes). ✅
- All new JS `//` comments in `companion.js` and `wish.js` are in English. ✅ (Verified each hunk: "Wish templates per role", "Direct role-keyed lookup", "Backwards-compat", "Read the drifter's Persona.role", "12 differentiated personas", "Role-based dispatch (Task 9)", "Persona is included", "forward-compat hook for Task 10/11/12", "Fallback: existing random pick", "Note: WISH_TEMPLATES...", "---- Collective wish (Task 9)", "Fulfill: mark done", etc.)
- All new Rust `//` / `//!` / `///` comments in `crates/vitric-cli/tests/companions.rs` are in English. ✅
- String literals (toast text "共识达成: 粮储达到 50!", HUD label "共识: 粮储 50 (未达成)") keep Chinese as-authored. ✅
- Rule `comment` fields in `rules/hud.json` and `rules/companion.json` keep Chinese — consistent with existing convention (existing rules all use Chinese `comment` fields; the brief's English-comment rule applies to JS `//` and Rust `//` only). ✅
- No fake APIs (`ctx.singleton`, `Math.random`, `vitric.on`, `ctx.llm`, etc.). Uses only verified real APIs: `ctx.getField`, `ctx.setField`, `ctx.emit`, `ctx.ask`, `ctx.random`, `ctx.dt`, `ctx.despawn`, `vitric.fn`, `vitric.system`. ✅
- No dead code in new JS — every new fn/system has a caller or is registered with `vitric.system`/`vitric.fn`. ✅
- Commit message `feat(frontier): companions expansion — 6 roles, 12 pool, wish templates` follows `<type>(<scope>): <summary>` convention. ✅
- Only in-scope files modified (7 files: schema, scene, 2 scripts, 2 rules, 1 test file). No out-of-scope changes. ✅

**Standard check sub-items from the brief:**
- Forward-compat hook events (`guard-patrol`, `trade-available`, `explore-bonus`) emitted with no consumers — by design (Tasks 10/11/12 will consume them). ✅
- `drifter-cadence` split into `drifter-cadence-normal` (stage != "兴旺") and `drifter-cadence-fast` (stage == "兴旺") — mutually exclusive via the `if` clauses. Both fire on `day-start`; only one matches. ✅
- `companion-invite-process` rule passes `role: "event.role"` (`rules/companion.json:89`). ✅
- `companion-contribution` query includes `"Persona"` (`scripts/companion.js` line ~678: `query: ["Companion", "Need", "Mood", "Persona", "Position"]`). ✅
- `consumeDrifter` passes role through: `role: role` in persona object, `args.role ? wishesForRole(args.role) : wishesForArchetype(args.archetype)` in Wish items. ✅
- `inviteAnyNearby` reads drifter `Persona.role` via `ctx.getField(args.drifter_id, "Persona.role")` and includes `role: role` in the `companion-invited` event payload. ✅

---

## Test verification

| Test suite | Result |
|---|---|
| `cargo test -p vitric-cli --test companions` (4 new tests) | ✅ 4/4 passed (0.64s) |
| `cargo test -p vitric-cli --test research` (regression) | ✅ 4/4 passed (1.32s) |
| `cargo test -p vitric-cli --test seasons` (regression) | ✅ 4/4 passed (0.46s) |
| `cargo test -p vitric-cli --test region -- --skip typescript` (regression) | ✅ 14/14 passed (220.41s — slow catch_up test) |
| `cargo run --release -- check games/frontier` | ✅ exit 0 |
| `cargo run --release -- gate games/frontier` | ⚠️ EXPECTED-FAIL: `check` passes, `playthrough:qa/clear.json` diverges at tick 0 (expected hash `0xb68b61d57750ff1`, actual `0x3ee0561b3d5743fc`). Expected because: (a) new `Colony.collective_wish_done` field on Colony changes tick-0 world hash, (b) new `collective_wish_lbl` HUD entity changes tick-0 world hash. Per brief: do NOT re-record `qa/clear.json`; Task 15 handles it. ✅ (expected failure mode confirmed) |

**New tests exercise the new behavior:**
1. `companion_contribution_role_builder_grants_plank` — primes Pip with `role=builder`, `affinity=60`, `mood=开心`, `contribution_timer=0`. Steps once. Verifies `@player.Inventory.plank = 1`. Exercises the `case "builder"` branch of `companion-contribution`. ✅
2. `companion_contribution_role_scholar_grants_techpoint` — primes Pip with `role=scholar`. Steps twice (contribution emits `tp-set` on tick 1; `tp-apply` rule in `rules/research.json` writes `TechPoint.value` on tick 2). Verifies `@player.TechPoint.value = 1`. Exercises the `case "scholar"` branch + cross-tick event pipeline. ✅
3. `collective_wish_fires_at_food_50` — sets `Colony.food_i = 50`, `collective_wish_done = 0`. Steps once. Verifies `collective_wish_done = 1` and `collective-wish-fulfilled` event emitted. Exercises the `collective-wish-check` system's main path. ✅
4. `collective_wish_one_time_only` — sets `collective_wish_done = 1`, `food_i = 80`. Steps once. Verifies `collective_wish_done` stays 1 and no second `collective-wish-fulfilled` event. Exercises the one-time guard. ✅

**Test API verification:** `Runtime::boot(&frontier_dir())`, `sim.step(&mut rt)`, `sim.world.entity(name)`, `sim.world.set_field(id, path, value)`, `sim.world.get_field(id, path)`, `rt.drain_observed()` — all match the existing pattern in `crates/vitric-cli/tests/research.rs`. Tests compile and pass. ✅

---

## Cannot-verify items

None. All audit items independently verified.

---

## Summary

The implementation is technically sound — all schema fields declared, all tests pass, gate failure mode is the expected one, the QuickJS shared-global deviation is correct and well-documented, and the role-based dispatch + collective wish system work as specified. The only blocker is a self-audit miss on the UI layout: the implementer compared `collective_wish_lbl` against `forecast_lbl` (wrong anchor group) and missed a real interior overlap with `build_menu` that is visible at game start (default `Mode.value = "build"`). The fix is a one-line documentation note (preferred) or a small position adjustment — no logic change needed, no test re-run needed.

**Recommended action:** Apply Important Finding #1's fix (a) — add a documentation note to the implementer's report and a remark in `rules/hud.json` or as a `comment` on the `collective_wish_lbl` entity stating the overlap with `build_menu` is intentional (HUD label occluded by modal build panel). Then re-approve.
