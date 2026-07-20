# Task 3 Review — POI System

**Reviewer:** task-reviewer sub-agent
**Commit range reviewed:** `300cf73..4c357e0` (implementer commit `0687c6a` + fix commit `4c357e0`)
**Method:** diff inspection + targeted file reads + grep verification + coordinate overlap computation. Did NOT re-run `vitric check` (per reviewer brief — implementer's report carries exit-0 evidence).

---

## 1. Spec compliance table

| # | Item | Verdict | Notes |
|---|------|---------|-------|
| 1 | `poi_tick` system signature + behavior | ✅ | `vitric.system("poi_tick", { query: ["Poi"], writes: ["Poi"] }, (entities, ctx) => {...})` matches real API (verified against `scripts/flare.js:17`). Skips `fresh`, decrements `cooldown` by `ctx.dt`, refreshes to `fresh` when cooldown hits 0. Correct. |
| 2 | `interact_poi` fn — all spec requirements | ✅ | Registered via `vitric.fn` (`poi.js:37`). Guards on `comp.Poi` + `state === "fresh"` (`:38-41`). Parses `reward_table` JSON (`:45`). Rolls with `ctx.random()` (`:60`). Emits `inv-set` with absolute inventory (`:67-70`). `ctx.setField(a.entity, "Poi.state", "looted")` (`:73`). `ctx.setField(a.entity, "Poi.cooldown", 120)` via `POI_COOLDOWN_LOOTED` (`:74`). Emits `toast-show` (`:77`), `entered-poi` (`:86`), conditional `companion-mood-drop` for `cave-entrance` (`:80-83`). All match spec. |
| 3 | `poi.json` rule shape | ✅ | `on: mouse`, guard `@uistate.Mode.value == "interact"`, calls `interact_poi` with `event.entity`/`event.comp`/`event.x`/`event.y` + 8 inventory fields. Identical shape to `rules/farm.json`'s `interact-click` (verified line-by-line). |
| 4 | 3 POI entities in `gen_scene.py` + `main.json` | ✅ | `poi_camp` (18,10), `poi_cave` (23,2), `poi_wreck` (26,5). Coordinates verified NOT in `LANDER`/`ROCK`/`ORE`/`ICE`/`WILD_ROCK`/`WILD_NODES`/`PLAYER`. Each entity has Position + Sprite + Collider + Poi + Text (verified in `main.json` by extracting the 3 POI objects). Reward tables match brief. |
| 5 | `main.json` 424 entities + UI entities present | ✅ | Verified via `python3 -c json.load`: `len(entities) == 424`. All 7 required UI entities present: `quest_title_lbl`, `quest_sub_lbl`, `quest_card`, `narration_lbl`, `intro_panel`, `ending_panel`, `hud_companion_lbl`. Fix commit `4c357e0` correctly restored the 19 dropped UI entities from `300cf73` and re-injected the 3 POIs. |
| 6 | `vitric.json` registration | ✅ | `"rules/poi.json"` at line 22 (after `rules/flare.json`). `"scripts/poi.js"` at line 34 (after `scripts/flare.js`). |
| 7 | `vitric check games/frontier` exits 0 | ✅ | Per implementer's report (not re-run per reviewer brief). Both commits in range report `CARGO_EXIT=0`. |
| 8 | No fake APIs in `poi.js` | ✅ | Grep for `ctx\.singleton|ctx\.each|vitric\.on|vitric\.expose|vitric\.call|ctx\.entity|ctx\.llm|Math\.random` in `poi.js` returns only the comment on line 7 warning against `Math.random`. No fake API calls. |
| 9 | Comments English / strings Chinese | ✅ | All `//` comments in `poi.js` are English. String literals (`"探索收获: ..."`, `"洞穴坍塌!全员心情-10"`, `"矿"`, `"麦"`, etc.) are Chinese. Project convention upheld. |
| 10 | Determinism — `ctx.random()` only | ✅ | No `Math.random()` calls in `poi.js` (only the warning comment on line 7). All randomness via `ctx.random()` at `:60` and `:80`. |
| 11 | No dead code / YAGNI | ⚠️ | `POI_ITEMS` constant at `poi.js:10` is declared but never referenced anywhere in the file. Only `POI_LABELS` (line 11) and `ITEMS` (line 48) are used. See Finding F1. |
| 12 | Reward range bounds | ✅ | `lo + Math.floor(ctx.random() * (span + 1))` where `span = Math.max(0, hi - lo)`. Analysis: `ctx.random() ∈ [0,1)` → `ctx.random() * (span+1) ∈ [0, span+1)` → `Math.floor(...) ∈ [0, span]` → `n ∈ [lo, lo+span] = [lo, hi]` when `hi ≥ lo`. Inclusive on both ends. Correct. Edge cases: if `lo > hi`, `span = 0`, `n = lo` always (defensive, no crash). If `lo = hi = 0`, `n = 0`, skipped by `if (n <= 0) continue`. All defensive. |
| 13 | Inv write-back consistency | ✅ | `poi.js:67-70` emits `inv-set` with `{item: inv[item]}` for all 8 `ITEMS`. Identical shape to `economy.js:57-61` `emitInv`. The existing `inv-apply` rule in `rules/economy.json:50-63` listens for `inv-set` and writes each field to `@player.Inventory.*`. Confirmed compatible. |
| 14 | Event listeners for emitted events | ⚠️ | `entered-poi`: no listener (intentional — Task 4 wires it). `toast-show`: NO listener found in any `rules/*.json` (grep confirmed). `companion-mood-drop`: NO listener found (grep confirmed). The existing `rules/toast.json` listens for specific game events (`built`, `planted`, `harvested`, etc.), NOT for a generic `toast-show` event. See Findings F2 and F3. |
| 15 | POI collision vs. player movement | ✅ | POIs have `Collider` (w:1.6, h:1.6), player has `Collider` (w:0.8, h:0.8). POIs are solid — player must click from adjacent tiles, matching the farm.json click-to-interact model. Computed adjacency: `poi_camp` (18,10) has 7 walkable adjacent tiles; `poi_cave` (23,2) has 7 walkable adjacent tiles; `poi_wreck` (26,5) has 6 walkable adjacent tiles. All reachable. No finding. |

**Summary:** 13 ✅ / 2 ⚠️ / 0 ❌. No Critical or Important issues. The 2 ⚠️ items produce Minor findings only.

---

## 2. Findings

### F1 — Minor: dead `POI_ITEMS` constant
- **File:** `games/frontier/scripts/poi.js:10`
- **Detail:** `const POI_ITEMS = ["ore", "wheat", "fiber", "plank"];` is declared but never referenced anywhere in the file. The reward loop iterates `Object.keys(rewards)` (line 54) and the inventory loop uses the local `ITEMS` (line 48). `POI_ITEMS` was copied verbatim from the brief's pseudocode (brief line 110), where it was also unused. This is a faithful-copy artifact, not an implementer invention.
- **Fix:** delete line 10. One-line removal, no behavior change.

### F2 — Minor: `toast-show` emitted with no listener (cross-task gap)
- **File:** `games/frontier/scripts/poi.js:77` and `:82`
- **Detail:** `ctx.emit("toast-show", { text: ... })` is emitted on every successful POI loot and on cave-injury. Grep across `games/frontier/rules/` for `toast-show` returns zero matches. The existing `rules/toast.json` only listens for specific named events (`built`, `build-fail`, `planted`, `plant-fail`, `harvested`, `gathered`, `crafted`, `craft-fail`, `companion-invited`, `invite-fail`) and writes to `@toast_lbl.UiLabel.content` directly — there is no generic `toast-show` listener. Consequence: the player sees NO toast when looting a POI. The reward summary (`探索收获: 矿+2 麦+3`) and the cave-injury warning (`洞穴坍塌!全员心情-10`) are silently dropped.
- **Note:** The reviewer brief expected `toast-show` to "likely already be handled by an existing toast rule" — it is not. This is a brief-level gap, not an implementer defect (poi.js faithfully followed the brief's spec). The engine treats unhandled events as no-ops, so `vitric check` stays green.
- **Fix:** add a rule to `rules/toast.json` (or a new `rules/toast_show.json`):
  ```json
  { "id": "toast-show", "on": { "event": "toast-show" }, "do": [
    { "set": "@toast_lbl.UiLabel.content", "to": "event.text" },
    { "set": "@toast_lbl.Toast.timer", "to": 2.5 }
  ] }
  ```
  This is a likely Task 4 candidate; flag here for visibility.

### F3 — Minor: `companion-mood-drop` emitted with no listener (cross-task gap)
- **File:** `games/frontier/scripts/poi.js:81`
- **Detail:** `ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" })` is emitted on cave-entrance injury. Grep across `games/frontier/rules/` and `games/frontier/scripts/` returns zero matches for `companion-mood-drop` (only the emit site in poi.js). Consequence: the companion's mood does not actually drop when the cave-injury triggers. The 30% injury roll still consumes a `ctx.random()` draw (affecting determinism of subsequent rolls) but has no gameplay effect.
- **Note:** The reviewer brief explicitly anticipated this: "flag as Minor if you confirm no listener exists." Confirmed: no listener. Engine treats as no-op, `vitric check` stays green.
- **Fix:** add a `companion-mood-drop` listener rule (likely Task 4) that decrements `@companion.Need.comfort` (or the appropriate mood field) by `event.amount`. Out of scope for Task 3.

### F4 — Minor (pre-existing, latent footgun): `gen_scene.py` does not regenerate committed `main.json`
- **File:** `games/frontier/tools/gen_scene.py` (whole file)
- **Detail:** Running `python3 tools/gen_scene.py` produces a 402-entity scene (399 from the generator's existing logic + 3 POIs from the new `POI_SPECS` block). The committed `scenes/main.json` has 424 entities. The 22-entity gap consists of hand-added UI/narrative entities that exist in `main.json` but are NOT emitted by `gen_scene.py`: `intro_l0`–`intro_l6`, `intro_panel`, `ending_l0`–`ending_l5`, `ending_panel`, `narration_lbl`, `quest_card`, `quest_sub_lbl`, `quest_title_lbl`, `hud_companion_lbl`, `hud_day_lbl`, `hud_food_lbl`, `hud_stage_lbl`, `companion_marker`, `drifter_marker`, `inv_row`, `mode_row`. Several of these are referenced by name in `rules/narrative.json` (lines 15-180) and `rules/quest.json` (lines 105-106) via `@<name>.UiLabel.content` / `@<name>.Ui.ox` setters. Re-running `gen_scene.py` in a future task will silently drop these and break the quest/narrative UI (the same regression that `0687c6a` introduced and `4c357e0` fixed).
- **Note:** This is a PRE-EXISTING issue — `gen_scene.py` was already out of sync with `main.json` at base commit `300cf73` (it produced 399 entities vs. the committed 421). Task 3 did not introduce the discrepancy; it only exposed it. The implementer's fix report (section 5) explicitly documents the decision: "`gen_scene.py` is left as the implementer left it (with `POI_SPECS`) — it is now forward-looking documentation; `main.json` is the authoritative scene." This is an acceptable workaround for Task 3's scope, but the footgun remains for future tasks.
- **Fix (out of Task 3 scope):** either (a) add the missing 22 UI entities to `gen_scene.py` so it faithfully regenerates `main.json`, or (b) add a comment at the top of `gen_scene.py` warning that it produces a subset of `main.json` and must not be re-run without manual re-injection of the hand-added UI entities.

---

## 3. Verdict

**APPROVED** — no Critical or Important findings. The implementation meets the brief's spec on all 7 spec-compliance items and all 8 code-quality items, with 2 ⚠️ items producing only Minor findings (dead constant + two emitted-but-unhandled events that are explicitly Task 4's responsibility, plus one pre-existing latent footgun documented by the implementer).

The critical regression introduced in `0687c6a` (dropping 19 UI entities by regenerating `main.json` from an out-of-sync `gen_scene.py`) was correctly identified and repaired in `4c357e0`. Final `main.json` state verified: 424 entities, all 7 required UI entities present, all 3 POIs present with correct components and coordinates.

---

## 4. Summary

Task 3 is a clean implementation of the POI system: `poi.js` and `poi.json` faithfully follow the brief's real-API guidance (no fake APIs, all randomness via `ctx.random()`, English comments, Chinese strings), the 3 POI entities are correctly placed in non-overlapping wild-zone tiles with all 5 required components, and `vitric.json` registration is correct. The implementer self-caught and fixed a critical scene-regression in a follow-up commit, leaving the final scene at 424 entities with all previously-referenced UI entities intact. Four Minor findings remain (one dead constant, two cross-task event-listener gaps deferred to Task 4, one pre-existing `gen_scene.py`/`main.json` sync footgun) — none block merge.
