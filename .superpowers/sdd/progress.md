# Frontier Sandbox Expansion — SDD Progress Ledger

Plan: docs/superpowers/plans/2026-07-20-frontier-sandbox-expansion.md
Base commit: c0c7af5
Branch: main (per user preference: auto-commit + push to main)

## Tasks

- Task 7: complete (commit f82d5ba, APPROVED with 0 Critical/Important, 2 Minor)
  - Seasons/Weather HUD — 7-day forecast bar: `Forecast` component (days text + last_day int) added to schema. Attached to `colony` entity. `forecast_lbl` HUD entity added to scene (oy:258, h:28, anchor/parent pattern matching season_lbl/weather_lbl).
  - `forecast-update` system in hud.js: query `["Clock","Season","Forecast"]`, writes `["Forecast"]`. Daily update gate (`if (e.Clock.day === e.Forecast.last_day) continue`). 7-label weighted-pick loop via `ctx.random_stream("forecast")` — isolated substream from flare.js's `"weather"` stream (7 draws/day don't perturb weather trajectory). `FORECAST_SEASONAL_WEIGHTS` duplicated from flare.js (brief-accepted DRY violation — extracting to shared module out of scope). `FORECAST_LABELS` maps weather keys to 晴/阴/雨/暴风/耀斑.
  - `hud-forecast` rule in hud.json: direct field copy `"to": "@colony.Forecast.days"` → `@forecast_lbl.UiLabel.content`. Uses the `{format, args}` rule engine API (NOT the brief pseudocode's `{call, with}` which was fictional — same correction as Task 6).
  - **Schema field audit PASS**: `Forecast.days`, `Forecast.last_day` declared; `Clock.day`, `Season.current` already declared in Task 6. No silent schema regressions (Task 8 lesson applied).
  - **UI layout PASS**: forecast_lbl at oy:258 has clean 6px gap above weather_lbl (oy:224 + h:24 = 248, then 10px gap to 258). No overlap.
  - **Tests**: seasons 4/4 PASS, region 14/14 PASS (no regression), schema check exit 0. Controller verified.
  - **Gate EXPECTED-FAIL**: `ReplayDiverged at tick 0` — Forecast on colony changes tick-0 world hash. Accepted per brief — `qa/clear.json` re-recorded in Task 15.
  - **Approved deviations**: (1) forecast_lbl Ui shape `anchor/parent/oy/w/h` (matches season_lbl/weather_lbl pattern) instead of brief's `x/y/ox/mode`. (2) `h:28` instead of brief's `h:0` (cosmetic). (3) Added `align:"center"` to match siblings.
  - Minor: M1 `FORECAST_SEASONAL_WEIGHTS` duplication from flare.js (brief-accepted); M2 forecast_lbl Ui shape deviation (approved per existing pattern).
  - Cannot-verify items resolved by controller: seasons/region tests PASS verified; schema check exit 0 verified; gate failure mode confirmed (`ReplayDiverged` at tick 0 — expected).

- Task 6: complete (commits 0b6adb9..e24d28f, APPROVED with 0 Critical, 1 Important resolved, 4 Minor)
  - Seasons & Weather: `Season` component (spring/summer/autumn/winter, 12 days each, 48-day year) + `Weather` component (clear/cloudy/rain/storm/flare) added to schema. Both components attached to `colony` entity (NOT a separate `clock` entity — the plan's pseudocode was fictional). `season_lbl` + `weather_lbl` HUD entities added to scene.
  - `clock.js` extended: query `["Clock","Season"]`, writes `["Clock","Season"]`. Season advance inlines inside `if (dayJustWrapped)` — `day_in_season += 1`, rolls over at 12 days, `season-change` event on rollover, `year += 1` on winter→spring wrap.
  - `flare.js` refactored into `weather-tick` system: query `["Colony","Clock","Season","Weather"]`, writes `["Colony","Weather"]`. Preserves `night-fall`/`dawn-break`/`flare-hit`/`flare-imminent` events. `flare-hit` fires on transition INTO flare (40% power/oxygen cut, once per onset). Weather weighted picking uses `ctx.random_stream("weather")` (E3 pattern — isolates weather RNG from main stream). Seasonal weights: flare only in summer (25%).
  - `crops.js`: `SEASON_CROP_MULT` (spring 1.2, summer 1.0, autumn 1.5, winter 0.3) applied to `ctx.dt` in main fn AND to `dormantSec` in catch_up fn. Season fetched via `ctx.getField("colony","Season.current")`.
  - `colony.js` tally: `WEATHER_RATE_MULT` (clear 1.0, cloudy 0.7, rain 0.7, storm 0.3, flare 0.0 for power; rain 1.5 for water) + `SEASON_RESOURCE_MULT` (spring 1.0, summer 0.8, autumn 1.2, winter 0.5) applied to emitted rates BEFORE `ctx.emit("tally",...)`. `apply-rates` rule unchanged.
  - Rules: `toast-weather-change` added to `flare.json`. `hud-season` + `hud-weather` added to `hud.json`.
  - Tests: `crates/vitric-cli/tests/seasons.rs` (NEW, 4 tests): `season_advances_on_day_boundary`, `season_rolls_over_at_12_days`, `year_increments_on_spring_wrap`, `weather_timer_decrements_each_tick`. All fast (1 tick each, boundary-condition setup).
  - **Gate EXPECTED-FAIL**: `ReplayDiverged` at tick 0 (adding Season/Weather to colony changes tick-0 world hash; weather RNG changes trajectory). Accepted — `qa/clear.json` re-recorded in Task 15. Schema check PASS, workspace tests PASS (skip typescript), seasons 4/4 PASS, region 14/14 PASS (no regression).
  - **Important (resolved by controller)**: brief pseudocode said `o2: extractor * PER * smult` but prose said "o2 gets only smult". Original `o2` source was `plot * PER` (plots produce both food AND oxygen — confirmed via `git show 0b6adb9:games/frontier/scripts/colony.js`). Brief pseudocode had a bug (changing source to extractor). Implementer correctly followed prose intent: `o2: plot * PER * smult` (kept original source, applied only season multiplier). Approved deviation.
  - **Approved deviations**: (1) Rule format `{format,args}` instead of brief's `{call:"toast",...}`/`{set,to}` — matches existing rules in `flare.json`/`hud.json` (brief pseudocode was wrong about rule engine API). (2) HUD positions moved from oy:150/184 to oy:190/224 to avoid overlap with `narration_lbl` (oy:150, h:40). (3) `Weather.next` declared but unused (forward-compat for Task 7 forecast — brief allows). (4) `flare-imminent` semantic inverted (now warns before flare ENDS, not STARTS — brief accepts).
  - Minor: M1 `total` field brief inaccuracy (cosmetic); M2 `flare-imminent` toast text may be misleading for Task 7 (cosmetic, fix in Task 7); M3 stale `progress.md` in working tree (correct — not staged per brief's `git add` list); M4 `Weather.next` unused (forward-compat).
  - Cannot-verify items resolved by controller: gate failure mode confirmed (`ReplayDiverged` at tick 0 — expected); workspace test pass trusted from implementer report (diff is consistent); schema check PASS verified by controller; region test no-regression verified by controller.

- Task 5: complete (commits 1537603..98fd213, APPROVED with 0 Critical/Important, 2 Minor)
  - E5 Snapshot/replay/describe plumbing: `describe_world_with_assets` gains a `dormant` classification pass — iterates `world.entities()` (bypassing `world.query`'s dormant filter), includes only entities with Position+Sprite that are `world.is_dormant(id)`. Each entry carries `id`, `name?`, `world` {x,y}, `sprite` {w,h,color,image?,rot?}, `region` {id, state}. Sort applied (named first, then id) when focal point exists. `"dormant"` key placed after `"offscreen"` in JSON output.
  - Snapshot preserves dormant state: locked by `snapshot_preserves_dormant_state` test (Region.state frozen + discovered=1 round-trips through `Sim::snapshot` + `Sim::restore`). Already works — Region is a World component.
  - Replay with dormant region hash-identical: locked by `replay_with_dormant_region_is_hash_identical` (60-tick recording with dormant mountain, replay matches final hash). Pre-recording thaw contract: locked by `replay_after_pre_recording_thaw_is_hash_identical` (thaw_region before both recording and replay — host-API-re-run contract per sim.rs line 362-365).
  - **Architectural note**: `thaw_region` DURING recording is NOT replayable via `Sim::replay` (by design — host API calls not recorded; `replay_observed` has no between-tick host hook). Replaced plan's impossible `replay_with_region_thaw_is_hash_identical` test with two correct tests. Adding a between-tick host hook to replay is out of scope for E5.
  - Gate hash unchanged: `0xab58ec29d99275df` (describe_world is read-only, snapshot/replay tests don't change production code path). Controller re-ran gate to verify.
  - Minor: M1 sort-closure reuse (improvement over brief pseudocode — DRY); M2 cosmetic test-comment nit. `mut rt` → `rt` in snapshot test (approved deviation — `sim.snapshot(&rt)` takes `&Runtime`, `rt` never used mutably).
  - Cannot-verify items resolved by controller: commit already pushed (98fd213, implementer's report was stale about cancellation); gate hash verified by controller rerun (pass:true); read-only signatures verified by reviewer via Grep.
  - **E1-E5 engine capability set COMPLETE.** Phase 1 done.

- Task 4: complete (commits c286ca5..58ab058, APPROVED with 0 Critical/Important, 3 Minor)
  - E4 View-frustum culling: `render_with` computes world-space viewport bounds from `(cam_x, cam_y, scale)` + framebuffer dimensions, then per-entity AABB check (rotation-aware `ext_x`/`ext_y` for `rot != 0`) before the pixel loop. Skipped entities are exactly those whose rotated AABB is entirely outside the viewport. Culling uses the shaken camera (same as the picture) so shake-panned entities are never lost.
  - **Dropped `margin = 4.0`**: brief pseudocode used margin "for shadow casters" but `collect_occluders` queries `["Solid","Position","Collider"]` via a separate path — sprite cull doesn't affect shadows. Exact AABB is the right boundary; any margin would be arbitrary over-conservatism. Approved deviation.
  - **Replaced flaky perf test with 3 deterministic correctness tests** (brief explicitly allows): `offscreen_entities_not_rendered` (regression guard — passes even without culling due to existing clamp, but locks contract), `onscreen_entities_rendered` (would fail if culling math has a sign flip), `culling_preserves_byte_identical_output_for_onscreen_entities` (asserts `Vec<u8>` equality + 32×32 magenta footprint). Screenshot-hash tests (frames/glow/particles) + gate hash lock the determinism contract end-to-end.
  - **GPU mirror skipped** (brief Step 5 optional): documented in `gpu.rs:1887-1894` pointing to CPU logic. CPU path is source of truth for screenshots/gate.
  - **`describe_world` not culled** (intentional): already classifies visible/offscreen with its own boundary check; off-screen entities appear in `offscreen` array for agent navigation. Culling would lose semantic info.
  - **Lights/Emitters not culled** (intentional): off-screen Light can illuminate on-screen pixels (radius extends into viewport); lighting formula already does per-pixel radius culling. Emitters draw via separate `draw_particles` path.
  - Gate hash unchanged: `0xab58ec29d99275df` (controller re-ran gate to verify — pass:true, verified:true, 37249 ticks).
  - Minor: M1 test header undercount ("two" → "three", fixed in post-review commit); M2 `offscreen_entities_not_rendered` is a weak TDD driver (useful regression guard, implementer honest); M3 test name `culling_preserves_byte_identical_output_for_onscreen_entities` slightly misleading (actual byte-lock comes from existing screenshot-hash tests + gate hash).
  - Cannot-verify items resolved by controller: typescript failures pre-existing (esbuild missing, verified via git stash in Task 1); gate hash verified by controller rerun (pass:true, hash matches); leftover uncommitted files committed in post-review docs commit.

- Task 3: complete (commits 53da162..c286ca5, APPROVED with 0 Critical/Important, 3 Minor)
  - E3 Seeded RNG substreams: `Substream` struct in `pcg.rs` (FNV-like hash of `(world_seed, name)` → increment); `Sim::substreams: HashMap<String, Substream>`; `Sim::random_stream(name) -> &mut Substream` inserts-if-absent; native `__randomStreamNext(name)` via `SIM_PTR` thread-local (mirrors `WORLD_PTR`); `ctx.random_stream(name)` in prelude returns `{ next(), nextInt(min, max) }`; substream state in `Sim::snapshot`/`restore` (serde_json BTreeMap-backed → byte-stable).
  - Substream state NOT in recording checkpoint hash (by design — recording hashes World only); determinism via `(world_seed, name)` seed contract + call-order replay.
  - Gate hash unchanged: `0xab58ec29d99275df` (substreams are Sim state, not World state).
  - Minor: M1 per-call FFI cost (revisit in Task 12); M2 test helper RAII (harmless); M3 safety comment imprecise (mirrors existing pattern).
  - Cannot-verify (Task 12 follow-ups): thaw_region not recorded by Recording — verify Task 12 region-thaw triggers are deterministic-given-recording; substream divergence only detected via world-state hash — verify Task 12 substream consumers write results into entity components.

- Task 2: complete (commits 202504a..53da162, APPROVED with 0 findings)
  - E2 Catch_up system API: `vitric.system(name, decl, fn, catch_up_fn?)` accepts optional 4th arg; `__runCatchUp` global in prelude; `SystemDecl.has_catch_up: bool`; `Sim::pending_catch_ups: Vec<String>` queue flushed in `step()` before `on_tick`; `GameLogic::catch_up_region` trait method (default no-op); `Runtime::catch_up_region` bridges to `ScriptEngine::run_catch_up_for_region`; `crop-grow` declares simplified catch_up (timer + stage only, no emit/side effects).
  - **Deviation approved**: queueing condition changed from `was_discovered` (brief pseudocode) to `was_dormant` — brief's `was_discovered` would skip catch_up on first thaw of never-discovered region, but test explicitly verifies catch_up runs on first thaw when entities exist in dormant region. `was_dormant` is semantically correct: dormant regions have un-simulated entities needing reconciliation; active regions have dormant_ticks=0 so catch_up is no-op anyway.
  - Cannot-verify items resolved by controller: region tests 5/5 PASS (catch_up test takes ~65s in debug due to 3600-tick step); gate PASS with same hash `0xab58ec29d99275df` (Task 2 didn't perturb deterministic trajectory — catch_up only fires on thaw, which existing recording doesn't do).
  - Pre-existing typescript failures (esbuild missing) unchanged.

- Task 1: complete (commits c0c7af5..a9e1167, APPROVED after I1 fix)
  - E1 Region dormant/active/frozen: `world.query`/`render_world`/`describe_world` skip dormant; `Sim::thaw_region(id)` transitions state + emits `region-thaw`; `accumulate_dormant_ticks` runs each step; `invoke_catch_up_for_region` stub added for Task 2 integration.
  - `Region` component added to `games/frontier/schema.json` (10 fields, state enum dormant/active/frozen). Mountain marker entity added to `scenes/main.json` (dormant, anchor 0,12, 30×28).
  - `pending_events: Vec<Event>` field added to `Sim` for host-side event emission (flushed at start of `step()`, not recorded by `Recording` — same determinism model as `pending_inputs`/`pending_replies`).
  - `qa/clear.json` re-recorded (37249 ticks, final_hash `0xab58ec29d99275df`, settlement-founded emitted) — necessary because mountain marker changed tick-0 hash.
  - Review I1 fix (commit a9e1167): added `was_discovered` conditional + `invoke_catch_up_for_region` stub.
  - Minor findings (non-blocking): M1 silent no-op on missing region (safer than brief's `.expect()`); M2 `thaw_region` not idempotent on already-active (rules can dedupe via `discovered`); M3 no positive-case test for `dormant_ticks` increment (brief didn't require).
  - Cannot-verify items resolved by controller: typescript failures pre-existing (esbuild missing); region tests 4/4 PASS verified; gate PASS with re-recorded qa/clear.json verified.

---

# Legacy: Frontier Deepening — SDD Progress Ledger

Plan: docs/superpowers/plans/2026-07-17-frontier-deepening.md
Base commit: af83c61
Branch: main (per user preference: auto-commit + push to main)

## Tasks

- Task 1: complete (commits af83c61..35004ad, review clean)
- Task 2: complete (commits 35004ad..300cf73, review clean after Math.random→ctx.random fix)
- Task 3: complete (commits 300cf73..4c357e0, APPROVED with 4 Minor, F2/F3 resolved in T4)
- Task 4: complete (commits 4c357e0..e01d4c0, APPROVED with 1 Minor)
- Task 5: complete (commits e01d4c0..eb0ac67, APPROVED with 3 Minor)
  - LLM memory dialogue: triggerWishMemory fn + onWishMemoryReply callback + MEMORY_FALLBACKS. Rule wish-fulfilled-memory catches wish-fulfilled → calls triggerWishMemory. Fallback path uses archetype-specific canned lines when LLM unavailable.
  - Minor findings: stash clobber on concurrent fulfillments (low probability), memory-unlocked event has no listener (intentional forward-looking), last_talk_reply shared with talk system (low conflict probability).
- Task 6: complete (commits eb0ac67..06464fb, APPROVED with 0 Critical/Important, 3 expected Minor)
  - Quest system converted to milestone: step3 gate on wish-fulfilled + affinity>=60; step6 gate on companion_wish_count>=2; game-won rule → settlement-founded (emits only settlement-founded, no settlement-thrived/game-won); quest-banner-8 → "自由探索中"; vitric.json gates.must_emit → settlement-founded.
  - Minor findings (all expected, non-blocking): dead ending-show rule in narrative.json left in place (YAGNI); qa/clear.json stale (Task 9 re-records); brief said check output "OK" but actual is JSON report with exit 0 (cosmetic).
- Task 7: complete (commits 06464fb..5a9870f, APPROVED with 0 findings)
  - Node regrowth: interact fn sets Node.cooldown=90 on depletion; node_regrow system decrements by ctx.dt, regrows left=max when cooldown hits 0.
  - Structure upgrade: upgrade_structure fn (plot→greenhouse, conduit→solar-array, quarters→cabin) + upgrade-button-click rule on ui-activate{action:"upgrade-prompt"}.
- Task 8: complete (commits 5a9870f..62c45fc, APPROVED with 1 cosmetic Minor)
  - UI hooks: flare-bar system (writes Colony.flare_bar) + hud-flare-bar rule; kb-mode-upgrade (key "u" → upgrade mode); upgrade-click rule (mouse + Mode=upgrade → upgrade_structure fn).
  - Minor: hud.js missing trailing newline (cosmetic, non-blocking).
  - **Post-review schema regressions fixed**: Task 8 added Colony.flare_bar writes + Mode.value="upgrade" sets + hud-flare-bar rule (reads @colony.Colony.flare_bar), but never added the schema field, the enum variant, or the @flare_lbl scene entity. Fixed in commit ee8cc74 (added Colony.flare_bar text field, "upgrade" to Mode.value enum, flare_lbl entity in main.json). Crashed engine at tick 0.
- Task 9: complete (commits 854c6ff, gate PASS — replay 37247 ticks verified, settlement-founded emitted)
  - Re-recorded 9-day playthrough with new wish-based quest gates.
  - Script bugs found & fixed in record_clear.py: (1) giveGiftNearby consumes ore (ore is first in ITEM_KINDS) → gather 4 ore not 2; (2) inp("e") only sets Mode.value, doesn't show craft_menu (only the mode-craft ui-activate rule shows it) → click mode_craft button before craft_plank.
  - Recording actually runs to day=11 (day-labels in script are aspirational, not literal); gate only checks settlement-founded emission.
  - **Pre-task schema regressions fixed**: Task 4 (commit e01d4c0) introduced Colony._wish_food_day and Colony.last_wish_memory_target writes in wish.js but never added them to schema.json. Fixed in commit 30e6ae7. Crashed engine at tick ~1191 (food reaches 80 after building any plot).
- Task 10: docs update — IN PROGRESS

## Notes

- Task 9 done via RPC-driven record_clear.py (no manual play). Controller rebuilt release binary, ran script, verified replay + gate.
- API: ctx.getField/setField/spawn/despawn/ask/emit/random/dt/tick all real. __onReply is prelude built-in. ctx.ask("llm", prompt, "callbackFnName") routes via llm-reply event → __onReply → callbackFn.
- Engine allows runtime ad-hoc fields on entities BUT rule/system reads via @entity.Comp.field require the field to be declared in schema.json. JS system writes (e.Colony.foo = ...) tolerate undeclared fields, but rule reads (@colony.Colony.foo) do NOT — they crash. Lesson: always declare fields in schema.json if any rule reads them.
- Task 6 changes: quest.json step3/6 gates → wish-based, step7→8 rename game-won→settlement-founded, quest-banner-8 text → "自由探索中", vitric.json gates.must_emit → settlement-founded. Colony.companion_wish_count already in schema (Task 1).
- Schema audit (Task 9): all Colony.* field references in scripts/*.js cross-checked against schema.json. All fields now declared.
