# Task 4 Review: Wish System + Toast/Mood Listeners

**Reviewer:** task-reviewer sub-agent
**BASE:** `4c357e0` (Task 3) → **HEAD:** `e01d4c0` (Task 4), single commit.
**Files reviewed:** `games/frontier/scripts/wish.js` (new), `games/frontier/rules/wish.json` (new), `games/frontier/scripts/companion.js` (modified), `games/frontier/scenes/main.json` (modified), `games/frontier/vitric.json` (modified). `schema.json` unchanged.

## 1. Spec compliance table

| # | Description | Verdict | Notes |
|---|---|---|---|
| 1 | `advance_wish` fn: registered via `vitric.fn`; reads `Colony.companion_handles` via `ctx.getField`; iterates handles; reads `Wish.items` (JSON), parses, advances matching `kind` by `amount`; on completion: `Wish.fulfilled++`, `Need.affinity += 30` (cap 100), `Need.affinity_i` updated, `Colony.companion_wish_count++`, emits `wish-fulfilled` with `companion`/`wish_desc`/`entity`; writes back `Wish.items` as JSON; skips `done`; defensive on parse failure | ✅ | wish.js:13-53. All behaviors present. Defensive: `try/catch` around `JSON.parse`, `Array.isArray` check, `if (!it || it.done) continue`, `if (!h) continue`. Emit payload `{companion, wish_desc, entity: h}` matches spec. |
| 2 | `apply_mood_drop` fn: registered via `vitric.fn`; reads `Colony.companion_handles`; for each, reads `Need.comfort`, decrements by `amount` (floor 0), updates `Need.comfort_i`; resolves Task 3 F3 | ✅ | wish.js:57-69. `Math.max(0, curNum - amount)` floors at 0. `Need.comfort_i = Math.round(next)` mirror updated. |
| 3 | `wish_food_check` system: `vitric.system` with `query: ["Colony"], writes: []`; emits `food-high` once per day when `Colony.food >= 80`, guarded by `Colony._wish_food_day` | ✅ | wish.js:74-84. Signature `{ query: ["Colony"], writes: [] }` exactly matches. Guard: `food >= 80 && day !== lastDay`; sets `_wish_food_day = day` before emit. |
| 4 | `wish.json` — 12 rules: build, build-lamp (filter kind=lamp), harvest (filter id=wheat), harvest-wheat (filter id=wheat, n=event.n), gather-ore (filter id=ore, n=event.n), poi, upgrade, food-high, see-dawn (guard `@player.Position.x >= 16`), wish-fulfilled-toast (format `"{} 心愿达成: {}"`), toast-show-generic (resolves F2), companion-mood-drop-apply (calls apply_mood_drop) | ✅ | wish.json has exactly 12 rules. All ids, events, filters, and `with` payloads match brief. Toast format string `"{} 心愿达成: {}"` preserved as Chinese. |
| 5 | `companion.js` — `WISH_TEMPLATES` (3 archetypes × 3 wishes) + `wishesForArchetype` (keyword match 技/电/匠→builder, 厨/医/农→farmer, default explorer); placed after `COMP_TICK_PER_SEC` | ✅ | companion.js:24-50. Inserted immediately after `COMP_TICK_PER_SEC` (line 22). Keyword regex `/技\|电\|匠\|build\|builder/i` and `/厨\|医\|农\|farm\|farmer/i` match spec. Default `explorer`. |
| 6 | `consumeDrifter` spawn includes `Wish: { items: JSON.stringify(wishesForArchetype(args.archetype)), fulfilled: 0 }` | ✅ | companion.js:350, inside the `ctx.spawn({...})` component object, after `Census`. |
| 7 | `main.json` — initial companion entity has `Wish` (builder template, archetype "话痨技工" matches /技/ → builder); entity count still 424 | ✅ | Verified via Python: companion entity has `Wish` with 3 builder wishes (build/build-lamp/upgrade). `len(scene['entities']) == 424`. |
| 8 | `vitric.json` — `rules/wish.json` in rules array, `scripts/wish.js` in scripts array | ✅ | vitric.json:24 and :36. Placed after `rules/affordability.json` and `scripts/poi.js` respectively. |
| 9 | `vitric check games/frontier` exits 0 | ✅ | Per implementer report §2 (exit code 0). Not re-run per review instructions. |
| 10 | No fake APIs in wish.js (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`) | ✅ | Grep across `games/frontier` found zero matches in wish.js (only match was a "do not use" comment in poi.js:7). |
| 11 | Comments in English / strings Chinese (wish.js + companion.js additions) | ✅ | All `//` comments in wish.js and the new companion.js block are English. Wish descriptions (建造 3 个结构, 建一盏灯, etc.) and toast format `"{} 心愿达成: {}"` are Chinese. |
| 12 | No dead code / YAGNI in wish.js | ✅ | `AFFINITY_GAIN_PER_WISH` (used at line 39), `WISH_TEMPLATES` and `wishesForArchetype` (used at line 350) all referenced. No unused exports or premature abstractions. |
| 13 | Determinism: wish advancement is deterministic; `wish_food_check` uses day-comparison guard, not randomness | ✅ | `advance_wish` and `apply_mood_drop` contain no randomness. `wish_food_check` uses `day !== lastDay` guard. |
| 14 | JSON round-trip safety: `Wish.items` parse-mutate-stringify preserves structure; no prototype pollution | ✅ | `JSON.parse(raw)` → mutate `it.progress`/`it.done` in place → `JSON.stringify(items)`. All fields (desc/kind/target/progress/done) preserved. `JSON.parse` (not `eval`) prevents prototype pollution. `Array.isArray` check guards against non-array payloads. |
| 15 | Affinity cap: `Math.min(100, affNum + 30)` caps correctly; updates both `Need.affinity` (number) and `Need.affinity_i` (int mirror) | ✅ | wish.js:39-41. `newAff = Math.min(100, affNum + 30)`; `Need.affinity = newAff`; `Need.affinity_i = Math.round(newAff)`. Both fields updated. |
| 16 | `companion_handles` iteration safety: handles empty list, null/undefined (default `[]`), skips falsy handles | ✅ | wish.js:18 `|| []` defaults null/undefined. `for (const h of handles)` is a no-op on empty list. `if (!h) continue` skips falsy entries. Same pattern in `apply_mood_drop` (line 60-62). |
| 17 | `wish-advance-build` fires on EVERY built event; `wish-advance-build-lamp` ALSO fires on lamp builds (both advance on lamp) | ✅ | wish.json:4-14. `wish-advance-build` has no filter (fires on all `built`); `wish-advance-build-lamp` has `filter: { kind: "lamp" }`. Building a lamp triggers both → `build` +1 and `build-lamp` +1. Brief comment "Lamp built -> **also** advance 'build-lamp' wish by 1" confirms intent. Rules match. |
| 18 | `wish-advance-harvest` (advance by 1) and `wish-advance-harvest-wheat` (advance by `event.n`) both fire on wheat harvest | ✅ | wish.json:16-25. Both rules filter `id=wheat`. `harvest` advances by 1, `harvest-wheat` advances by `event.n` (emitted as `n: 2` in economy.js:120). Tracks different templates: farmer's "种出 2 茬作物" (target 2) vs "收获 8 单位麦子" (target 8). Brief explicitly distinguishes them. |
| 19 | `see-dawn` guard `@player.Position.x >= 16` is the correct wild-zone boundary | ✅ | `games/frontier/tools/gen_scene.py:41`: `elif p in ROCK or (gx >= 16 and p in WILD_ROCK)` — wild zone starts at x=16. `WILD_ROCK` includes `(16, 4)`; `WILD_NODES` smallest x is 17. Guard matches engine wild-zone boundary. |
| 20 | `Colony._wish_food_day` as undeclared field — safe? survives save/load? | ⚠️ → ✅ (Minor) | Pattern is established by `Colony._day_anchor` in companion.js:628-630 (same ad-hoc int field on Colony, used the same way). `vitric check` passes, so the engine permits runtime fields. Save/load behavior not directly verifiable from diff, but consistency with the existing `_day_anchor` pattern makes this safe. See Finding M1. |

**Totals:** 19 ✅ / 1 ✅(Minor) out of 20.

## 2. Findings

### M1 — `Colony._wish_food_day` is an undeclared runtime field [Minor]

**Location:** `games/frontier/scripts/wish.js:79, 81`; field absent from `games/frontier/schema.json`.

**Observation:** `Colony._wish_food_day` is read and written via `ctx.getField`/`ctx.setField` but is not declared in `schema.json`'s `Colony` component. The implementer's report §5 notes that `vitric check` passed without it, so the engine allows runtime ad-hoc fields.

**Risk:** Save/load serialization behavior for undeclared fields is not verifiable from the diff alone. If the engine only persists schema-declared fields, `_wish_food_day` would reset to `0` on load — causing `food-high` to fire again on the same day after a reload. The same risk applies to the pre-existing `Colony._day_anchor` field in `companion.js:628`, so this is a project-wide pattern, not a Task 4 regression.

**Suggested fix (optional):** Add `_wish_food_day` to the `Colony` component in `schema.json` with `{"type": "int", "default": 0}`. This makes the field durable across save/load and explicit in the schema. The brief's Step 6 explicitly permits this conditional addition; the implementer chose not to add it because `vitric check` passed without it. If the team prefers strict schema discipline, this should also be retroactively applied to `_day_anchor`.

**No action required for Task 4 acceptance** — the pattern is established and the engine accepts it.

---

No Critical or Important findings.

## 3. Verdict

**APPROVED**

The implementation matches the brief on all 20 review items. The single Minor finding (M1) is a pre-existing pattern (`_day_anchor`) carried forward; it does not block acceptance.

## 4. Summary

Task 4 is implemented faithfully: `wish.js` exposes `advance_wish` + `apply_mood_drop` fns and the `wish_food_check` system with correct defensive handling (JSON parse guards, empty/null handles, affinity cap, int mirror updates); `wish.json` contains all 12 rules with correct events/filters/payloads; `companion.js` adds the `WISH_TEMPLATES`/`wishesForArchetype` helper and the `Wish` spawn component; `main.json`'s companion entity has the builder-template `Wish` and the entity count remains 424. Cross-task concerns (lamp double-fire, wheat double-fire, wild-zone boundary x≥16) all match the brief's intent. The only finding is Minor: `Colony._wish_food_day` is undeclared, consistent with the existing `_day_anchor` pattern, and is flagged for awareness rather than blocking.
