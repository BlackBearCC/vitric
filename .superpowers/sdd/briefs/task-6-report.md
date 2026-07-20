# Task 6 Report — Seasons & Weather

## 1. Status

**DONE_WITH_CONCERNS**

All 12 brief steps completed. Schema check passes, 4 new seasons integration tests pass, full workspace test suite passes (excluding pre-existing typescript failures), gate fails with `ReplayDiverged` at tick 0 as expected and accepted (recording re-recorded in Task 15). Commit pushed to `origin/main`. See Concerns (section 6) for the four flagged items the reviewer should be aware of.

## 2. Commits

| SHA | Subject | Remote |
|---|---|---|
| `e24d28f509a8e9317ff0531f038ee3fdea6d030c` | `feat(frontier): seasons & weather system` | `origin/main` (`0b6adb9..e24d28f`) |

9 files changed, 337 insertions(+), 45 deletions(-).

## 3. Test results

| # | Command | Expected | Actual |
|---|---|---|---|
| 1 | `~/.cargo/bin/cargo test -p vitric-cli --test seasons` | PASS | **PASS** (4 passed, 0 failed, 0.08s) |
| 2 | `~/.cargo/bin/cargo test -p vitric-cli --test region -- --skip typescript` | PASS | **PASS** (14 passed, 0 failed, 111.68s — the long-running `catch_up_advances_dormant_crop_on_thaw` accounts for ~60s) |
| 3 | `~/.cargo/bin/cargo test --workspace -- --skip typescript` | PASS | **PASS** (exit 0; all crate unit + integration + doc-tests pass, no regressions) |
| 4 | `~/.cargo/bin/cargo run --release -- check games/frontier` | PASS | **PASS** (schema validates, all referenced fields declared) |
| 5 | `~/.cargo/bin/cargo run --release -- gate games/frontier` | **EXPECTED FAIL** | **FAIL** as expected — `"重放在 tick 0 跑偏：期望哈希 0xb68b61d57750ff1，实际 0x39f53230c84d99e3"`. The replay diverges at tick 0 because adding `Season`+`Weather` to the `colony` entity changes the tick-0 world hash, and `weather-tick` consumes RNG changing the trajectory. Per brief: do NOT re-record `qa/clear.json` (Task 15 does that). |

## 4. Files touched

| File | Change | Lines (+/-) |
|---|---|---|
| `crates/vitric-cli/tests/seasons.rs` | NEW — 4 integration tests | +127 |
| `games/frontier/schema.json` | Added `Season` (3 fields) + `Weather` (3 fields) components | +35 |
| `games/frontier/scenes/main.json` | Extended `colony` entity with `Season`+`Weather`; added `season_lbl` (oy:190) + `weather_lbl` (oy:224) HUD entities | +1 / -1 |
| `games/frontier/scripts/clock.js` | Extended `clock-advance` system: query `["Clock","Season"]`, writes `["Clock","Season"]`; season advance on day-wrap (12 days/season, year++ on winter→spring) | +24 / -1 |
| `games/frontier/scripts/flare.js` | Renamed `flare` → `weather-tick`; query `["Colony","Clock","Season","Weather"]`, writes `["Colony","Weather"]`; weather transitions via `ctx.random_stream("weather")` weighted pick; preserved `night-fall`/`dawn-break`/`flare-hit`/`flare-imminent`; added `weather-change` event | +86 / -20 |
| `games/frontier/scripts/crops.js` | Added `SEASON_CROP_MULT`; multiplier applied to `ctx.dt` in main fn and to `dormantSec` in catch_up | +25 / -5 |
| `games/frontier/scripts/colony.js` | Added `WEATHER_RATE_MULT` + `SEASON_RESOURCE_MULT`; `tally` system applies multipliers to emitted rates before `apply-rates` rule | +33 / -4 |
| `games/frontier/rules/flare.json` | Appended `toast-weather-change` rule (4 existing rules unchanged) | +4 |
| `games/frontier/rules/hud.json` | Appended `hud-season` + `hud-weather` rules (5 existing rules unchanged) | +16 |

## 5. Deviations from brief

1. **`season_lbl`/`weather_lbl` HUD positions adjusted (oy:190/224 instead of brief's 150/184).**
   The existing `narration_lbl` entity occupies `oy:150` with `h:40`, spanning the vertical band 150–190. Placing `season_lbl` at the brief's `oy:150` would overlap. Moved to `oy:190` (immediately below `narration_lbl`'s bottom edge) and `weather_lbl` to `oy:224` (28-pt label height + 6-px gutter below `season_lbl`). Verified no overlap with `flare_lbl` (oy:122) or `toast_lbl` (oy:30). Same colors (`#7fbf5a` for season, `#f4a261` for weather) and size (28) as the brief specifies.

2. **`toast-weather-change` rule uses template format instead of `{call: "toast", with: {text: "天气变化：{event.weather}"}}`.**
   The brief's pseudocode `{call: "toast", with: {text: "天气变化：{event.weather}"}}` doesn't match the existing toast-callable pattern in `rules/wish.json` (which uses `{set: "@toast_lbl.UiLabel.content", "to": {format: "...", args: [...]}}` plus a `{set: "@toast_lbl.Toast.timer", "to": N}` pair). I matched the existing `wish-fulfilled-toast` pattern: `{format: "天气变化：{}", args: ["event.weather"]}` for content + `{set: "@toast_lbl.Toast.timer", "to": 2.0}` for timer. This is the rule-format adjustment flagged in the brief's Concerns section.

3. **`hud-season`/`hud-weather` rules use template `{format, args}` syntax instead of inline string interpolation `"第 {@colony.Season.year} 年 · {@colony.Season.current}"`.**
   The brief's inline `@colony.Season.year` inside a plain string literal isn't supported by the rule engine (verified in `engine.rs` — `resolve()` is only invoked on the *value* side of `set`/`to`, `add`/`by`, etc., not on bare substrings inside string literals). Used `{"format": "第 {} 年 · {}", "args": ["@colony.Season.year", "@colony.Season.current"]}` instead, matching the existing `hud-stage` rule's `format`/`args` pattern. Same deviation applied to `hud-weather` rule.

4. **`o2` source in `tally` system kept as `extractor * PER` (original source) with only `smult` multiplier added.**
   The brief's pseudocode shows `o2: extractor * PER * smult` but the brief's prose (line 401) says "o2 gets only `smult`" (no source change). The pre-Task-6 source was `o2: plot * PER`. I kept the original `plot * PER` source and applied only `smult` (so final is `o2: plot * PER * smult`). Inline comment in `colony.js` notes this deviation. Brief author should confirm which was intended.

5. **`flare-imminent` semantic preserved but inverted.**
   See Concerns (section 6) — the warning now fires when current weather IS flare and timer drops below 30s (warning before flare ENDS), not before flare STARTS. The brief explicitly accepts this in step 4 notes.

## 6. Concerns

1. **`Weather.next` field is declared in `schema.json` but unused by the implementation.**
   The `weather-tick` system performs instant transitions (current weather changes immediately when timer hits 0; no preview of next weather). `Weather.next` was added to the schema per the brief's Step 1 spec for forward-compat with Task 7 (potential HUD forecast). No rule, no JS system, no `ctx.getField`/`ctx.setField` reads or writes it. Schema audit (per project convention) confirms: no rule references `Weather.next`, no script reads `Weather.next`. Left in schema intentionally for forward-compat; if Task 7 doesn't end up using it, it can be removed.

2. **`flare-imminent` semantic change: now warns before flare ENDS, not before it STARTS.**
   Old behavior: `flare_timer` counted down to 0, and when it crossed the 30s threshold, `flare-imminent` fired warning the player that a flare was about to *start*. New behavior: weather transitions are instant (no countdown to next weather). When `Weather.current === "flare"` and `Weather.timer <= 30`, the system emits `flare-imminent` warning the player that the flare is about to *end*. This is a 180° semantic flip of the event. Per brief step 4 notes, this is accepted and flagged here for reviewer awareness. Downstream consumers (UI, save-data snapshots, the existing toast rule for `flare-imminent` if any) should be re-audited in Task 7. Specifically: if the toast text says "耀斑即将来袭!" (flare imminent), it's now misleading — the flare is ending, not coming.

3. **Catch-up season approximation: uses season-at-thaw for entire dormant period.**
   The `crop-grow` catch_up function reads `ctx.getField("colony", "Season.current")` at thaw time and applies that single multiplier to the entire dormant budget (`dormantSec * mult`). If a crop was dormant across a season boundary (e.g., dormant in summer, thawed in autumn), it gets the autumn multiplier (1.5) applied to the entire summer+autumn dormant period — over-counting growth. The accurate approach would be to integrate the season multiplier over time, partitioning the dormant period by season transitions. Per brief step 5 notes, this approximation is accepted (simplification) and flagged here. Impact is bounded because dormant regions are typically in the mountain biome away from active farms, and the dormant budget is small relative to active growth time.

4. **Rule-format adjustments to match existing `rules/*.json` patterns.**
   Two rule-format deviations from brief pseudocode (see Deviations 2 & 3 above): the `toast-weather-change` rule uses `{format, args}` template syntax matching `wish-fulfilled-toast` instead of a `{call: "toast", ...}` invocation (no `toast` fn is registered); the `hud-season`/`hud-weather` rules use `{format, args}` template syntax instead of inline string interpolation (rule engine doesn't interpolate `@entity.Comp.field` inside plain string literals). Both are mechanical adjustments to fit the engine's actual capabilities; semantic intent preserved.

5. **Gate failure (expected, accepted).**
   `vitric gate games/frontier` fails with `ReplayDiverged` at tick 0 (`期望哈希 0xb68b61d57750ff1，实际 0x39f53230c84d99e3`). This is expected and accepted per brief CRITICAL callout: adding Season/Weather to the colony entity changes the tick-0 world hash, weather-tick consumes RNG, season/weather multipliers change production rates. The recording is NOT re-recorded in Task 6 (Task 15 handles that). All other gates pass.

6. **Stale uncommitted `progress.md` left out of commit.**
   `.superpowers/sdd/progress.md` had pre-existing uncommitted edits (Task 5 review notes from a prior session) at the moment of Task 6 commit. Per the brief's explicit `git add` file list (9 files), `progress.md` was NOT staged in the Task 6 commit. The Task 5 review notes remain in the working tree as an unstaged change for the controller/reviewer to handle separately.
