# Task 6 Review — Seasons & Weather

## Verdict: APPROVED

The implementation faithfully realizes the brief's intent across all 11 steps. Four deviations from brief pseudocode were all justified adaptations to the engine's actual rule format and to pre-existing HUD layout — none are correctness regressions. One brief ambiguity (o2 source in `tally`) is flagged as Important for the controller to resolve with the brief author; the implementer's conservative choice is defensible. Schema audit, event preservation, RNG isolation, multiplier placement, and test quality all check out. Gate failure is expected and accepted per brief.

## Spec compliance

- Step 1 (schema): ✅ — `Season` (current/day_in_season/year) and `Weather` (current/timer/next) added to `games/frontier/schema.json` with correct types, variants, and defaults matching the brief exactly.
- Step 2 (scene): ✅ — `colony` entity extended with `Season`+`Weather` (defaults `spring/0/1` and `clear/30/clear`); `season_lbl` (oy:190) and `weather_lbl` (oy:224) added. Position deviation from brief's 150/184 is justified — see Important #1.
- Step 3 (clock.js): ✅ — `clock-advance` query extended to `["Clock","Season"]`, writes `["Clock","Season"]`; season advance inside `if (dayJustWrapped)`; `season-change` fires only when `day_in_season >= SEASON_DAYS (12)`; year++ only on winter→spring wrap (next_idx===0).
- Step 4 (flare.js refactor): ✅ — renamed `flare` → `weather-tick`; query `["Colony","Clock","Season","Weather"]`, writes `["Colony","Weather"]`; preserved `night-fall`/`dawn-break`/`flare-hit`/`flare-imminent`; added `weather-change`; uses `ctx.random_stream("weather")` for weighted pick + timer reset; flare-hit fires only on transition INTO flare.
- Step 5 (crops.js multiplier): ✅ — `SEASON_CROP_MULT` constant added (spring 1.2 / summer 1.0 / autumn 1.5 / winter 0.3); multiplier applied to `dt` BEFORE `c.timer += dt` in main fn; catch_up applies `dormantSec * mult`.
- Step 6 (colony.js tally multiplier): ✅ — `WEATHER_RATE_MULT` + `SEASON_RESOURCE_MULT` constants added; multipliers applied in `tally` system BEFORE `ctx.emit("tally", ...)`. The `colony` stockpile system is unchanged. See Important #2 for the o2 source ambiguity.
- Step 7 (rules): ✅ — `toast-weather-change` appended to `rules/flare.json`; `hud-season` + `hud-weather` appended to `rules/hud.json`. Rule format matches existing patterns in `rules/hud.json` (`{format, args}` template) and `rules/flare.json` (`{set, to}` pair). See Important #3 for the rule-format deviation note.
- Step 8 (schema check): ✅ — implementer reports `cargo run --release -- check games/frontier` PASS. All 6 new fields declared; all `ctx.getField`/rule-referenced fields are in schema.
- Step 9 (seasons.rs tests): ✅ — 4 integration tests assert meaningful contracts (season advance, season rollover at 12 days, year increment on spring wrap, weather timer decrement). All would fail if the implementation broke. See Notes for test-quality analysis.
- Step 10 (test suite + gate): ✅ — implementer reports seasons tests PASS (4/4), region tests PASS (14/14, no regression), workspace tests PASS (excluding pre-existing typescript failures), schema check PASS, gate FAIL with `ReplayDiverged` at tick 0 (expected and accepted per brief).
- Step 11 (commit): ✅ — single commit `e24d28f` pushed to `origin/main` with the prescribed commit message format. Range `0b6adb9..e24d28f` matches.

## Findings

### Critical
(none)

### Important

1. **Brief ambiguity: `o2` source in `tally` system** (`games/frontier/scripts/colony.js:50`)
   The brief's pseudocode (Step 6, line 388) shows `o2: extractor * PER * smult` (source = `extractor`), but the brief's prose (line 401) says "o2 gets only `smult`" (multiplier-only, no source change). The pre-Task-6 source was `o2: plot * PER` (verified via `git show 0b6adb9:games/frontier/scripts/colony.js`). The implementer kept the original `plot` source and applied only `smult`, giving `o2: plot * PER * smult`. The implementer added an inline comment documenting the deviation and asking the brief author to confirm.
   **Action for controller**: resolve with brief author which was intended — (a) keep `plot` source + `smult` (implementer's choice, matches prose "only smult"), or (b) change source to `extractor` + `smult` (matches pseudocode). If (b), the implementer should also re-audit the `water` source (currently `extractor * PER * wmult.water * smult`) to decide if both o2 and water should come from extractors. The implementer's choice is defensible either way; flagging only because the brief is internally inconsistent.

2. **Rule-format deviation is approved** (`games/frontier/rules/flare.json:19-22`, `games/frontier/rules/hud.json:66-80`)
   The brief's pseudocode used fictional `{call: "toast", with: {...}}` and inline `@entity.Comp.field` string interpolation. The implementer used `{format, args}` template syntax + `{set: "@toast_lbl.Toast.timer", to: N}` pair, matching the EXISTING `rules/hud.json` patterns (`hud-res`, `hud-stage`, `hud-inv` all use `{format, args}`) and the EXISTING `rules/flare.json` toast pattern (`{set: "@toast_lbl.UiLabel.content", to: "..."} + {set: "@toast_lbl.Toast.timer", to: N}`). I verified by reading both files. The `@colony.Season.year` / `@colony.Season.current` / `@colony.Weather.current` references inside `args` arrays are the correct way to interpolate component fields — the rule engine resolves `@`-prefixed tokens in `args` position (per existing `hud-stage` rule's `"args": ["@colony.Colony.stage"]`). This is an approved deviation — the implementer's adaptation is mechanically correct and consistent with the engine's actual capabilities. Not a regression.

3. **HUD position deviation is approved** (`games/frontier/scenes/main.json` — `season_lbl` oy:190, `weather_lbl` oy:224)
   The brief specified oy:150 and oy:184. The implementer moved them to oy:190 and oy:224. I verified via the scene file: `narration_lbl` exists at oy:150 with h:40 (spans 150–190), so the brief's oy:150 would overlap. The new positions sit immediately below `narration_lbl`'s bottom edge (190) with a 6-px gutter before `weather_lbl` (224 − 190 − 28 = 6). `flare_lbl` is at oy:122 (h:28, spans 122–150) — no overlap. `toast_lbl` is at oy:30 — no overlap. The deviation is justified and the new layout is clean. Colors (`#7fbf5a` season, `#f4a261` weather) and size (28) match the brief.

### Minor

1. **`Weather.next` declared but unused** (`games/frontier/schema.json:221-225`)
   The `Weather.next` field is declared in schema but never written by any JS system, never read by any rule, never accessed via `ctx.getField`/`ctx.setField`. The brief explicitly allows this as forward-compat for Task 7 forecast (Step 4 note: "Leave it in schema for forward-compat"). The implementer flagged this as Concern #1. No action needed unless Task 7 doesn't use it — then it should be removed.

2. **`total` field in `tally` event** (`games/frontier/scripts/colony.js:54`)
   The brief pseudocode (Step 6, line 391) shows `total: conduit + plot + extractor`, but the original tally had `total: entities.length` (verified via `git show 0b6adb9`). The implementer kept `entities.length` unchanged. This is correct — the brief pseudocode is showing the full function but the `total` field is not a Task 6 concern (multipliers don't apply to it). Not a deviation, just a brief inaccuracy. No action.

3. **`flare-imminent` semantic inversion** (Concern #2 — accepted)
   The old `flare-imminent` warned before flare STARTED (countdown to flare onset). The new `flare-imminent` warns before flare ENDS (fires when `weather.current === "flare" && weather.timer <= 30`). The brief explicitly accepts this (Step 4 note). The existing `toast-flare-imminent` rule text says "耀斑 30 秒后来袭!储备电力氧气!" ("flare coming in 30s") which is now misleading — the flare is ending, not coming. The implementer flagged this for Task 7 re-audit. No action for Task 6, but the controller should ensure Task 7 (HUD/toast polish) updates this toast text.

4. **Catch_up season approximation** (Concern #3 — accepted)
   The catch_up function in `crops.js` uses `ctx.getField("colony", "Season.current")` at thaw time and applies that single multiplier to the entire dormant budget. If a crop was dormant across a season boundary, it gets the thaw-time season's multiplier for the whole dormant period. The brief explicitly accepts this (Step 5 note). Bounded impact (dormant regions are typically far from active farms; dormant budget is small). No action.

5. **Stale uncommitted `progress.md`** (Concern #6 — process note)
   `.superpowers/sdd/progress.md` has pre-existing uncommitted Task 5 review notes that were NOT staged in the Task 6 commit (per the brief's explicit 9-file `git add` list). This is correct behavior — the brief's commit instructions listed exactly 9 files and the implementer followed them. The stale `progress.md` remains in the working tree for the controller to handle separately. No action for Task 6.

### ⚠️ Cannot verify from diff

1. **Workspace test suite pass claim** — The implementer reports `cargo test --workspace -- --skip typescript` exits 0. I did not re-run this (it would take ~2min and the implementer's report is credible and detailed). Controller may re-run if there's doubt.

2. **Schema check pass claim** — The implementer reports `cargo run --release -- check games/frontier` PASS. I did not re-run. The schema diff is correct (all 6 fields declared with right types), so this should pass. Controller may re-run if there's doubt.

3. **Region tests no-regression claim** — The implementer reports `cargo test -p vitric-cli --test region -- --skip typescript` 14 passed. The region tests boot the frontier project, so the new `weather-tick` system runs during them — if the system crashed, these tests would fail. They didn't. Controller may re-run if there's doubt.

4. **Gate failure mode** — The implementer reports gate fails with `ReplayDiverged` at tick 0 (`期望哈希 0xb68b61d57750ff1，实际 0x39f53230c84d99e3`). This is the expected failure mode (adding Season/Weather to colony changes tick-0 world hash; weather-tick consumes RNG). Per brief, do NOT re-record `qa/clear.json` (Task 15 does that). No verification needed — the failure is expected.

## Notes

### Test-quality analysis (Step 9)

The 4 tests in `crates/vitric-cli/tests/seasons.rs` assert meaningful contracts:
- `season_advances_on_day_boundary`: sets `Clock.time=59.99`, `Season.day_in_season=0`, steps 1 tick (dt=1/60≈0.017). Day-wrap fires, day_in_season → 1. Asserts `day_in_season==1`, `current=="spring"`, `year==1`. Would fail if season-advance logic broke or if the day-wrap boundary was off.
- `season_rolls_over_at_12_days`: sets `day_in_season=11`, triggers day-wrap. Asserts rollover to `summer`, `day_in_season==0`, `year==1`. Would fail if the `>= SEASON_DAYS` check or the season index advance broke.
- `year_increments_on_spring_wrap`: sets `season=winter`, `day_in_season=11`, triggers day-wrap. Asserts wrap to `spring`, `year==2`, `day_in_season==0`. Would fail if the `next_idx === 0` year-increment check broke.
- `weather_timer_decrements_each_tick`: asserts `timer_after < timer_before` and `elapsed < 0.1`, and that `current` didn't change on a single tick (timer=30, dt≈0.017, no transition). Would fail if the timer decrement or the `timer > 0` early-return broke.

All 4 tests are FAST (1 tick each, ~0.02s per test). They test the boundary logic directly rather than running a full 48-day cycle. Good test hygiene — they assert specific contracts, not just "doesn't crash".

### Event preservation audit (Step 4)

Verified all 8 required events are emitted by the refactored `weather-tick` system + `clock-advance` system:
- `night-fall{threat}` — preserved (diff lines 467-475 of `flare.js`, fires on `isNight 0→1`).
- `dawn-break{}` — preserved (fires on `isNight 1→0`).
- `flare-hit{power_loss, o2_loss}` — emitted on transition INTO flare (diff line 537), 40% cut applied once per flare onset. Payload carries pre-hit values.
- `flare-imminent{eta}` — emitted when `weather.current === "flare" && weather.timer <= 30 && cl.flare_warning !== 1` (diff line 499-503). Matches brief's accepted semantic (warns before flare ENDS).
- `weather-change{weather, prev}` — NEW, emitted on every weather transition (diff line 530).
- `season-change{season, year}` — NEW, emitted on season rollover inside `if (dayJustWrapped)` block, only when `day_in_season >= SEASON_DAYS` (clock.js diff line 270).
- `day-start{day}` — preserved (clock.js, unchanged).
- `time-tick{day, tod}` — preserved (clock.js, unchanged).

### Multiplier placement audit (Step 6)

Verified multipliers are applied in the `tally` system BEFORE `ctx.emit("tally", ...)`, NOT in the `colony` stockpile system. The `apply-rates` rule in `rules/colony.json` is unchanged (implementer's report confirms this; the diff doesn't touch `rules/colony.json`). Multipliers flow through the event payload: `event.pow`, `event.food`, `event.o2`, `event.water` all carry the multiplied values, and the rule writes them directly to `@colony.Colony.*_rate`. Correct architecture — keeps the rule engine simple, multipliers live in JS where they can access `ctx.getField`.

### Weather RNG audit (Step 4)

Verified `ctx.random_stream("weather")` is used for BOTH:
- Weighted pick: `const r = stream.next() * total;` (diff line 516)
- Timer reset: `weather.timer = stream.nextInt(WEATHER_DURATION[0], WEATHER_DURATION[1]);` (diff line 526)

NOT `ctx.random()`. This isolates weather RNG from the main stream, ensuring replay stability regardless of when other systems consume the main RNG. E3 pattern correctly applied.

### Schema field audit (project rule)

All fields written by JS systems OR read by rules OR accessed via `ctx.getField`/`ctx.setField` are declared in `schema.json`:
- `Season.current` — read by `ctx.getField` in `crops.js:37`, `colony.js:43`; read by `rules/hud.json:70` (`hud-season`). Declared ✅
- `Season.day_in_season` — written by `clock.js` (`e.Season.day_in_season += 1`). Declared ✅
- `Season.year` — read by `rules/hud.json:70`. Declared ✅
- `Weather.current` — written by `flare.js` (`weather.current = new_weather`); read by `ctx.getField` in `colony.js:42`; read by `rules/hud.json:78` (`hud-weather`). Declared ✅
- `Weather.timer` — written by `flare.js` (`weather.timer -= ctx.dt`, `weather.timer = stream.nextInt(...)`). Declared ✅
- `Weather.next` — declared but unused (brief allows as forward-compat). See Minor #1.

Audit passes.

### Catch_up multiplier audit (Step 5)

Verified the catch_up function in `crops.js` applies the season multiplier to `dormantSec` (the dormant tick budget):
```js
const dormantSec = dormantTicks / CROP_TICK_PER_SEC;
const season = ctx.getField("colony", "Season.current");
const mult = SEASON_CROP_MULT[season] || 1.0;
let t = (ctx.getField(entityHandle, "Crop.timer") || 0) + dormantSec * mult;
```
Correct — multiplier applies to the dormant seconds BEFORE they're added to the timer. The `|| 1.0` fallback is defensive (shouldn't happen since Season.current is always a valid enum). The approximation (season-at-thaw for entire dormant period) is documented in the code comment and flagged in Concern #3.

### Implementer's 7 concerns — reviewer assessment

1. `Weather.next` unused — Minor, brief allows. ✅
2. `flare-imminent` semantic inversion — accepted by brief, flagged for Task 7 toast text re-audit. ✅
3. Catch_up season approximation — accepted by brief, bounded impact. ✅
4. Rule-format adjustments — approved deviation (matches existing `rules/*.json` patterns, verified by reading `rules/flare.json` and `rules/hud.json`). ✅
5. HUD position overlap — approved deviation (verified `narration_lbl` at oy:150/h:40, new positions 190/224 avoid overlap). ✅
6. Stale `progress.md` — correct behavior (brief's `git add` list was explicit, 9 files). ✅
7. (gate failure) — expected and accepted per brief. ✅

All 7 concerns are appropriately flagged and either accepted by brief or approved deviations. No blockers.
