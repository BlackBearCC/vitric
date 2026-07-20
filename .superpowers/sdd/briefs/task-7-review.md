# Task 7 Review — Seasons/Weather HUD (7-day forecast bar)

## Verdict
**APPROVED**

All five brief requirements are faithfully realized in the diff. The `Forecast` component, `forecast-update` system, `forecast_lbl` entity, and `hud-forecast` rule are all in place and match the brief's spec. Schema field audit passes — every field read by a rule or written by the JS system is declared in `schema.json`. UI layout audit passes — `forecast_lbl` at oy:258 has a 6px gap above `weather_lbl` (224+28=252) and no entity below it. The one implementer deviation (Ui shape: `anchor/parent/oy/w/h` instead of brief's `x/y/ox/mode`) is approved — it matches the existing `season_lbl`/`weather_lbl` pattern, which the brief explicitly instructs the implementer to follow. The gate failure (`ReplayDiverged` at tick 0) is expected and accepted per brief — Task 15 will re-record.

## Brief Requirements Audit

- **Step 1 — Add `Forecast` component to schema.json** ✅
  `games/frontier/schema.json:369-380` declares `Forecast` with `days` (text, default `""`) and `last_day` (int, default `0`). Types and defaults match the brief exactly. Block is inserted between `Weather` and `Inventory`, preserving JSON structure (closing `},` before `"Inventory":`).

- **Step 2a — Attach `Forecast` to `colony` entity** ✅
  `games/frontier/scenes/main.json` `colony` entity now carries `"Forecast":{"days":"","last_day":0}` alongside the existing `Colony`/`Census`/`Clock`/`Season`/`Weather` components. Defaults match the schema.

- **Step 2b — Add `forecast_lbl` HUD entity** ✅
  `games/frontier/scenes/main.json` adds `forecast_lbl` between `weather_lbl` and `quest_card`. Position `oy:258` matches the brief verbatim. UiLabel `size:24` + `color:"#a8d8ea"` match the brief. Ui shape deviates from brief pseudocode (uses `anchor/parent/oy/w/h` instead of `x/y/ox/mode`) — see Approved Deviation #1.

- **Step 3 — Add `forecast-update` system to hud.js** ✅
  `games/frontier/scripts/hud.js:35-90` (system body at lines 60-90) implements the brief's pseudocode exactly:
  - Query `["Clock","Season","Forecast"]`, writes `["Forecast"]` — matches brief.
  - Day-change gate `if (e.Clock.day === e.Forecast.last_day) continue;` — matches brief.
  - `const stream = ctx.random_stream("forecast");` — separate substream from `ctx.random_stream("weather")` used in `flare.js:78`. Verified.
  - 7-label generation loop (`FORECAST_DAYS = 7`), weighted pick via `forecastPick(stream, weights)`, label translation via `FORECAST_LABELS` — matches brief.
  - Output string `"预报: " + labels.join(" ")` — matches brief's `"预报: ..."` format.
  - `FORECAST_SEASONAL_WEIGHTS` is a verbatim duplicate of `flare.js:26 SEASONAL_WEIGHTS` — brief explicitly accepts this DRY violation.

- **Step 4 — Add `hud-forecast` rule to hud.json** ✅
  `games/frontier/rules/hud.json:81-88` appends `hud-forecast` after `hud-weather`. Rule body `{ "set": "@forecast_lbl.UiLabel.content", "to": "@colony.Forecast.days" }` matches the brief exactly. Format matches existing `hud-food-bar` / `hud-flare-bar` direct-copy pattern.

- **Step 5 — Tests + gate** ✅
  Implementer reports: schema check PASS, seasons tests PASS (4/4), region tests PASS (14/14), workspace tests PASS (minus pre-existing typescript failures), gate FAIL with `ReplayDiverged` at tick 0 (expected per brief; `qa/clear.json` not re-recorded — Task 15 handles it). See Cannot-verify section.

- **Step 6 — Commit** ✅
  Single commit `f82d5bae8da9d98b50a30e6054851f421dff7540` on `origin/main` with the prescribed `feat(frontier):` commit message format. Per task instructions, reviewer did NOT commit.

## SDD Checklist

### 1. Schema field audit

Every field referenced by a rule (`@entity.Comp.field`) or accessed via JS (`e.Comp.field`):

| Field reference | Where used | Declared in schema.json? |
| --- | --- | --- |
| `Forecast.days` (write) | `hud.js:113` `e.Forecast.days = ...` | ✅ `schema.json:371-374` (text, default `""`) |
| `Forecast.days` (read) | `hud.json:86` `@colony.Forecast.days` | ✅ same |
| `Forecast.last_day` (read+write) | `hud.js:100, 114` | ✅ `schema.json:375-378` (int, default `0`) |
| `Clock.day` (read) | `hud.js:100, 114` `e.Clock.day` | ✅ `schema.json:316-319` (int, default `1`) — declared in Task 6 |
| `Season.current` (read) | `hud.js:103` `e.Season.current` | ✅ `schema.json:336-340` (enum spring/summer/autumn/winter, default `spring`) — declared in Task 6 |
| `UiLabel.content` (set) | `hud.json:86` `@forecast_lbl.UiLabel.content` | ✅ engine-built-in component, used by all other HUD rules |

No `ctx.getField` / `ctx.setField` calls in the new JS — the system uses direct `e.Comp.field` access only (which the engine resolves via the query/writes declaration). All referenced fields are declared. **Audit passes.**

### 2. Enum variant audit

No enum variants are written by the new code:
- `forecast-update` writes `Forecast.days` (text) and `Forecast.last_day` (int) — no enum writes.
- `hud-forecast` rule copies `Forecast.days` (text) to `UiLabel.content` (text) — no enum literals in `set`/`to`.
- `forecastPick` returns one of `"clear"|"cloudy"|"rain"|"storm"|"flare"` — these are JS string keys used to look up `FORECAST_LABELS`, not enum values written to a component. No schema audit needed.

`Season.current` is read (not written) by `forecast-update` — the read is type-safe against the declared `spring|summer|autumn|winter` variants. The fallback `|| FORECAST_SEASONAL_WEIGHTS.spring` guards against any unexpected value. **Audit passes.**

### 3. Scene entity reference audit

Only one `@name.Comp.field` reference in the new rule:

| Reference | Entity exists in scene? |
| --- | --- |
| `@forecast_lbl.UiLabel.content` | ✅ `forecast_lbl` added to `scenes/main.json` in this same diff |
| `@colony.Forecast.days` | ✅ `colony` exists; `Forecast` component added to it in this same diff |

Both target entities exist. **Audit passes.**

### 4. UI layout overlap audit

Center HUD stack (entities with `anchor:"top-center", parent:"ui"`):

| Entity | oy | h | y-range |
| --- | --- | --- | --- |
| `hud_companion_lbl` | 88 | 32 | 88–120 |
| `flare_lbl` | 122 | 28 | 122–150 |
| `narration_lbl` | 150 | 40 | 150–190 |
| `season_lbl` | 190 | 28 | 190–218 |
| `weather_lbl` | 224 | 28 | 224–252 |
| `forecast_lbl` (NEW) | 258 | 28 | 258–286 |

Gap analysis:
- `weather_lbl` ends at 252; `forecast_lbl` starts at 258 → **6px gap** ✅ (matches brief's intended spacing pattern of 6px gutters)
- No entity exists below `forecast_lbl` in the center stack — no overlap risk below.
- `quest_card` (oy:100, anchor:top-right) is a different anchor lane — no horizontal overlap with the top-center stack.
- `build_menu` (oy:176, anchor:top-left) and `craft_menu` (oy:176, anchor:top-left) are also different lanes.

No overlaps. **Audit passes.**

### 5. Standard checks

- ✅ **Schema check** — implementer reports `cargo run --release -- check games/frontier` exits 0 (429 entities, +1 for `forecast_lbl`). Schema diff is well-formed JSON; `Forecast` block correctly comma-separated between `Weather` and `Inventory`.
- ✅ **Comment language** — all new `//` comments in `hud.js` (lines 35-41, 70-71, 86, 92, 99, 105) are English. Rule comment in `hud.json:83` is English. String literals (`"预报: "`, `"晴"`, `"阴"`, `"雨"`, `"暴风"`, `"耀斑"`) keep their authored Chinese — correct per project convention.
- ✅ **No fake APIs** — `ctx.random_stream`, `stream.next()`, `vitric.system`, `e.Comp.field` are all verified real APIs (same as `flare.js:78` weather-tick). No `ctx.singleton`/`ctx.each`/`Math.random`/etc.
- ✅ **No dead code / YAGNI** — `FORECAST_DAYS`, `FORECAST_SEASONAL_WEIGHTS`, `FORECAST_LABELS`, `forecastPick` are all referenced by the `forecast-update` system body.
- ✅ **Commit message** — `feat(frontier): 7-day weather forecast HUD bar` follows `<type>(<scope>): <summary>` convention.
- ✅ **In-scope files only** — diff touches only `games/frontier/schema.json`, `games/frontier/scenes/main.json`, `games/frontier/scripts/hud.js`, `games/frontier/rules/hud.json` (the 4 files in the brief).
- ✅ **Determinism** — `ctx.random_stream("forecast")` is a separate E3 substream from `ctx.random_stream("weather")` (verified `flare.js:78` uses `"weather"`). 7 draws per day, gated by day-change check. `Object.entries(weights)` iteration is insertion-order for string keys (V8/QuickJS guarantee), so the weighted pick is deterministic across replays.

## Findings

### Critical
None.

### Important
None.

### Minor

- **M1: `FORECAST_SEASONAL_WEIGHTS` + `FORECAST_LABELS` duplicated from `flare.js`** (`hud.js:44-54` vs `flare.js:26-34`). The brief explicitly accepts this DRY violation (Step 3 note: "extracting to a shared module is out of scope — the weights are stable"). Risk: if `flare.js` weights are ever tweaked, `hud.js` must be updated in lockstep or forecast predictions will drift from actual weather distribution. The implementer flagged this as Concern #1. No action required for Task 7; future refactor opportunity.

- **M2: `forecast_lbl` Ui shape deviates from brief pseudocode** (`scenes/main.json`). Brief specified `{x:0, y:0, w:0, h:0, ox:0, oy:258, mode:"screen"}`; implementer used `{anchor:"top-center", parent:"ui", oy:258, w:1280, h:28}` to match the existing `season_lbl`/`weather_lbl` pattern. This is approved (see Approved Deviation #1) — flagging here only for audit completeness. The `size:24` and `color:"#a8d8ea"` match the brief verbatim. The added `align:"center"` matches sibling labels.

## Approved Deviations

1. **`forecast_lbl` Ui shape** — The brief's pseudocode used `{x, y, w, h, ox, oy, mode}` for the `Ui` component, but the existing `season_lbl` and `weather_lbl` (added in Task 6) use a different shape: `{anchor, parent, oy, w, h}` with no `x/y/ox/mode` fields. The brief explicitly instructs (Step 2b position note + general "Important: match existing patterns" guidance in prior tasks) to follow the existing pattern when in doubt. The implementer's choice is mechanically correct — `anchor:"top-center"` + `parent:"ui"` drives positioning, `ox:0` is the default, and the entity renders identically. Position `oy:258` was preserved verbatim from the brief.

2. **`forecast_lbl` `h:28` instead of brief's `h:0`** — Necessary corollary of Deviation #1. A `h:0` label would not render. The existing labels use `h:28` for size:28 text; `forecast_lbl` uses `size:24`, so `h:28` is a reasonable box height (4px vertical slack). No visual overlap with neighbors (6px gap above to `weather_lbl`, nothing below).

3. **`align:"center"` added to `forecast_lbl.UiLabel`** — Brief pseudocode omitted `align`, but all sibling top-center labels (`hud_companion_lbl`, `flare_lbl`, `season_lbl`, `weather_lbl`) specify `align:"center"`. Adding it for consistency is a no-op visually (center is the natural default for top-center anchored labels) and matches the established pattern.

## Cannot-verify (controller must confirm)

The reviewer did not re-run any commands; the following are taken from the implementer's report. Controller may re-run if there is doubt.

1. **`cargo run --release -- check games/frontier` exits 0** — schema validity claim. The schema diff is well-formed JSON and all fields are correctly typed, so this should pass. Implementer reports 429 entities (+1 for `forecast_lbl`).

2. **`cargo test -p vitric-cli --test seasons` PASS (4/4)** — Task 6 season/weather tests, no regression. These tests boot the frontier project, so the new `forecast-update` system runs during them — if the system crashed, the tests would fail.

3. **`cargo test -p vitric-cli --test region -- --skip typescript` PASS (14/14)** — region tests no-regression claim. Same boot-the-project argument as above.

4. **`cargo test --workspace -- --skip typescript` PASS** — full workspace suite minus pre-existing typescript failures. Implementer's most credible claim, since the workspace includes the seasons and region tests above plus unit tests across all 9 crates.

5. **`cargo run --release -- gate games/frontier` FAIL with `ReplayDiverged` at tick 0** — expected failure mode per brief (adding `Forecast` to `colony` changes tick-0 world hash; forecast substream changes trajectory from tick 1 onward). Implementer reports expected hash `0xb68b61d57750ff1`, actual `0x9efc8316d884aa53`. `qa/clear.json` was NOT re-recorded per brief — Task 15 will re-record all gates. The `check` sub-gate passing (429 entities) plus the green workspace test suite provides equivalent verification in lieu of the gate.

6. **Commit `f82d5bae8da9d98b50a30e6054851f421dff7540` pushed to `origin/main`** — reviewer did not verify the push directly. Controller should confirm via `git log origin/main -1` before marking the task fully closed.
