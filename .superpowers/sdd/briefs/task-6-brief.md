# Task 6: Seasons & Weather

**Files:**
- Modify: `games/frontier/schema.json` (add Season, Weather components)
- Modify: `games/frontier/scenes/main.json` (add Season/Weather to `colony` entity; add `season_lbl` + `weather_lbl` HUD entities)
- Modify: `games/frontier/scripts/clock.js` (season advance on day-wrap)
- Modify: `games/frontier/scripts/flare.js` (refactor to `weather-tick` system; preserve events)
- Modify: `games/frontier/scripts/crops.js` (season multiplier on growth + catch_up)
- Modify: `games/frontier/scripts/colony.js` (weather/season multiplier in `tally` system)
- Modify: `games/frontier/rules/flare.json` (add `weather-change` toast)
- Modify: `games/frontier/rules/hud.json` (add `hud-season` + `hud-weather` rules)
- Test: `crates/vitric-cli/tests/seasons.rs` (NEW — rust integration test)

**Interfaces:**
- Produces: `Season` component (spring/summer/autumn/winter, 12 days each, 48-day year); `Weather` component (clear/cloudy/rain/storm/flare); season multipliers on crop growth; weather + season multipliers on colony resource rates. The `weather-tick` system replaces the old `flare` system (flare is now a weather variant, not a standalone timer).

## CRITICAL: real API (NOT the plan's pseudocode)

The plan's pseudocode uses fictional APIs. **Do NOT copy it verbatim.** These are the load-bearing corrections:

1. **No `ctx.entity()` API.** Use `ctx.getField("colony", "Season.current")` (string entity name as ref — confirmed live in `wish.js:18`), OR extend the system's query to include the component and use direct mutation `e.Season.day_in_season += 1`.
2. **No separate `clock` entity.** The `Clock` component lives on the `colony` entity alongside `Colony` and `Census`. `Season` and `Weather` go on `colony` too — NOT on a new `clock` entity.
3. **No `TestSim` Python class.** `tools/test_progression.py` is a top-to-bottom script using `urllib.request` RPC. The season-cycle test is written in RUST (see Step 9) — not Python.
4. **Three duplicate `DAY_SEC` constants** exist: `CLOCK_DAY_SEC = 60.0` in `clock.js:15`, `FLARE_DAY_SEC = 60.0` in `flare.js:15`, `CROP_DAY_SEC = 60.0` in `crops.js:14`. Do NOT unify them (Task 14 will). Just use the existing local constants.
5. **Catch_up signature is `(entityHandle, ctx, dormantTicks)`** — camelCase, NOT the plan's `(entity, ctx, dormant_ticks)`. See `crops.js` existing catch_up.

## CRITICAL: gate hash WILL change — do NOT fix it

Adding `Season` + `Weather` to the `colony` entity changes the tick-0 world hash. The `weather-tick` system consumes RNG (changing the trajectory). Season/weather multipliers change production rates. **All of these make the existing `qa/clear.json` recording non-replayable** — the gate will fail with `ReplayDiverged`.

**This is expected and accepted.** The plan re-records `qa/clear.json` in Task 15. For Task 6, verify via:
- `cargo run --release -- check games/frontier` (schema check) — must PASS
- `cargo test --workspace -- --skip typescript` — must PASS (excluding pre-existing typescript failures)
- `cargo run --release -- gate games/frontier` — EXPECTED TO FAIL with `ReplayDiverged`. Do NOT re-record `qa/clear.json`. Do NOT try to "fix" the gate. Note the failure in the report.

## CRITICAL: schema field audit

Per project convention (project_memory.md): **every field written by a JS system OR read by a rule OR accessed via `ctx.getField`/`ctx.setField` MUST be declared in `schema.json`.** No exceptions.

Fields to declare (new in Task 6):
- `Season.current` (enum spring/summer/autumn/winter)
- `Season.day_in_season` (int)
- `Season.year` (int)
- `Weather.current` (enum clear/cloudy/rain/storm/flare)
- `Weather.timer` (number)
- `Weather.next` (enum clear/cloudy/rain/storm/flare)

Existing fields that Task 6 reads/writes (already declared, do NOT re-declare):
- `Colony.flare_timer`, `Colony.flare_warning`, `Colony.flare_bar`, `Colony.is_night`, `Colony.wild_threat`
- `Colony.o2_rate`, `Colony.pow_rate`, `Colony.food_rate`, `Colony.water_rate`
- `Clock.day`, `Clock.time`, `Clock.tod`, `Clock.last_day_emit`

## Step 1: Add Season and Weather components to schema.json

In `games/frontier/schema.json`, add these two component definitions (place them near `Clock` for logical grouping):

```json
"Season": {
  "fields": {
    "current": { "type": "enum", "variants": ["spring", "summer", "autumn", "winter"], "default": "spring" },
    "day_in_season": { "type": "int", "default": 0 },
    "year": { "type": "int", "default": 1 }
  }
},
"Weather": {
  "fields": {
    "current": { "type": "enum", "variants": ["clear", "cloudy", "rain", "storm", "flare"], "default": "clear" },
    "timer": { "type": "number", "default": 30 },
    "next": { "type": "enum", "variants": ["clear", "cloudy", "rain", "storm", "flare"], "default": "clear" }
  }
}
```

## Step 2: Add Season/Weather to colony entity + HUD label entities in scenes/main.json

### 2a. Extend the `colony` entity

Find the `colony` entity in `games/frontier/scenes/main.json`. It currently has `Colony`, `Census`, `Clock` components. Add `Season` and `Weather`:

```json
{
  "name": "colony",
  "components": {
    "Colony": { /* existing fields unchanged */ },
    "Census": { /* existing fields unchanged */ },
    "Clock": { "day": 1, "time": 0, "tod": "晨", "last_day_emit": 1 },
    "Season": { "current": "spring", "day_in_season": 0, "year": 1 },
    "Weather": { "current": "clear", "timer": 30, "next": "clear" }
  }
}
```

### 2b. Add `season_lbl` and `weather_lbl` HUD entities

Add two new entities near the existing `flare_lbl` entity (copy its `Ui` + `UiLabel` structure, adjust position and color):

```json
{
  "name": "season_lbl",
  "components": {
    "Ui": { "x": 0, "y": 0, "w": 0, "h": 0, "ox": 0, "oy": 150, "mode": "screen" },
    "UiLabel": { "content": "", "size": 28, "color": "#7fbf5a" }
  }
},
{
  "name": "weather_lbl",
  "components": {
    "Ui": { "x": 0, "y": 0, "w": 0, "h": 0, "ox": 0, "oy": 184, "mode": "screen" },
    "UiLabel": { "content": "", "size": 28, "color": "#f4a261" }
  }
}
```

**Position note**: look at the existing `flare_lbl` entity's `Ui.oy` value (the search report says 122). Place `season_lbl` at `oy: 150` and `weather_lbl` at `oy: 184` to stack below it without overlap. Verify no overlap with existing HUD elements by reading the scene file.

## Step 3: Extend clock.js to advance seasons

In `games/frontier/scripts/clock.js`:

The current `clock-advance` system queries `["Clock"]` and writes `["Clock"]`. Extend it to also handle `Season`:

```javascript
const CLOCK_DAY_SEC = 60.0;
const SEASON_DAYS = 12;
const SEASONS = ["spring", "summer", "autumn", "winter"];

vitric.system("clock-advance", { query: ["Clock", "Season"], writes: ["Clock", "Season"] }, (entities, ctx) => {
  for (const e of entities) {
    e.Clock.time += ctx.dt;
    let dayJustWrapped = false;
    while (e.Clock.time >= CLOCK_DAY_SEC) {
      e.Clock.time -= CLOCK_DAY_SEC;
      e.Clock.day += 1;
      dayJustWrapped = true;
    }
    const frac = e.Clock.time / CLOCK_DAY_SEC;
    let tod = "晨";
    if (frac >= 0.75) tod = "夜";
    else if (frac >= 0.50) tod = "昏";
    else if (frac >= 0.25) tod = "午";
    if (e.Clock.tod !== tod) e.Clock.tod = tod;

    // Season advance: only on day-wrap. day_in_season increments, and at SEASON_DAYS
    // the season rolls over (spring→summer→autumn→winter→spring, year++ on wrap to spring).
    if (dayJustWrapped) {
      e.Season.day_in_season += 1;
      if (e.Season.day_in_season >= SEASON_DAYS) {
        e.Season.day_in_season = 0;
        const idx = SEASONS.indexOf(e.Season.current);
        const next_idx = (idx + 1) % SEASONS.length;
        e.Season.current = SEASONS[next_idx];
        if (next_idx === 0) {
          e.Season.year += 1;
        }
        ctx.emit("season-change", { season: e.Season.current, year: e.Season.year });
      }
    }

    if (dayJustWrapped && e.Clock.last_day_emit !== e.Clock.day) {
      e.Clock.last_day_emit = e.Clock.day;
      ctx.emit("day-start", { day: e.Clock.day });
    }
    ctx.emit("time-tick", { day: e.Clock.day, tod: e.Clock.tod });
  }
});
```

**Key points:**
- The system query is now `["Clock", "Season"]` — both components must be on the same entity (they are, both on `colony`).
- Season advance is INSIDE the `if (dayJustWrapped)` block — fires once per day-wrap, not every tick.
- `ctx.emit("season-change", ...)` fires only when the season actually changes (every 12 days), not every day-wrap.
- Direct mutation `e.Season.day_in_season += 1` is valid because the system declares `writes: ["Season"]`.

## Step 4: Refactor flare.js into weather-tick system

In `games/frontier/scripts/flare.js`:

**PRESERVE** from the existing system:
- Day/night detection (`is_night`, `wild_threat`, `night-fall` event, `dawn-break` event).
- `flare-hit` event emission (with `power_loss` + `o2_loss` payload).
- `flare-imminent` event emission (30s warning before flare).

**REMOVE**: the `flare_timer` countdown logic (flare is now a Weather variant, not a standalone timer).

**ADD**: weather transition logic using `ctx.random_stream("weather")` for replay-stable weighted picking.

```javascript
const FLARE_DAY_SEC = 60.0;

// Weather duration range in seconds (timer resets to a random value in this range).
const WEATHER_DURATION = [30, 90];

// Seasonal weather weights (must sum to 100 per season). Flare only in summer.
const SEASONAL_WEIGHTS = {
  spring: { clear: 50, cloudy: 30, rain: 15, storm: 5, flare: 0 },
  summer: { clear: 40, cloudy: 20, rain: 10, storm: 5, flare: 25 },
  autumn: { clear: 45, cloudy: 30, rain: 20, storm: 5, flare: 0 },
  winter: { clear: 30, cloudy: 40, rain: 0, storm: 30, flare: 0 }
};

// Weather labels for HUD (Chinese to match existing UI style).
const WEATHER_LABELS = {
  clear: "晴", cloudy: "阴", rain: "雨", storm: "暴风", flare: "耀斑"
};

vitric.system("weather-tick", { query: ["Colony", "Clock", "Season", "Weather"], writes: ["Colony", "Weather"] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const cl = c.Colony;
  const ck = c.Clock;
  const season = c.Season.current;
  const weather = c.Weather;

  // --- Day / night detection (preserved from old flare system) ---
  const frac = ck.time / FLARE_DAY_SEC;
  const isNight = frac >= 0.75 ? 1 : 0;
  if (isNight !== cl.is_night) {
    cl.is_night = isNight;
    if (isNight === 1) {
      cl.wild_threat = 1 + Math.floor(ck.day / 3);
      ctx.emit("night-fall", { threat: cl.wild_threat });
    } else {
      cl.wild_threat = 0;
      ctx.emit("dawn-break", {});
    }
  }

  // --- Weather timer ---
  weather.timer -= ctx.dt;
  if (weather.timer > 0) {
    // Flare imminent warning: if current weather is flare and timer <= 30, emit flare-imminent.
    // (This preserves the 30s warning behavior from the old flare_timer logic.)
    if (weather.current === "flare" && weather.timer <= 30 && cl.flare_warning !== 1) {
      cl.flare_warning = 1;
      ctx.emit("flare-imminent", { eta: weather.timer });
    }
    return;
  }

  // --- Weather transition ---
  // Use ctx.random_stream("weather") for replay-stable weighted picking. The substream is
  // seeded by (world_seed, "weather") so the weather sequence is deterministic regardless of
  // when other systems consume the main RNG stream.
  const stream = ctx.random_stream("weather");
  const weights = SEASONAL_WEIGHTS[season];
  const total = Object.values(weights).reduce((a, b) => a + b, 0);
  const r = stream.next() * total;
  let new_weather = "clear";
  let acc = 0;
  for (const [k, w] of Object.entries(weights)) {
    acc += w;
    if (r < acc) { new_weather = k; break; }
  }

  const old_weather = weather.current;
  weather.current = new_weather;
  weather.timer = stream.nextInt(WEATHER_DURATION[0], WEATHER_DURATION[1]);
  cl.flare_warning = 0;

  // Emit weather-change for every transition (rules/flare.json toast + HUD update).
  ctx.emit("weather-change", { weather: new_weather, prev: old_weather });

  // Flare-hit: when transitioning INTO flare, apply the 40% power/oxygen cut.
  // (Preserves the old flare-hit behavior — payload carries pre-hit values.)
  if (new_weather === "flare") {
    const power_loss = cl.power * 0.4;
    const o2_loss = cl.oxygen * 0.4;
    ctx.emit("flare-hit", { power_loss, o2_loss });
    cl.power = cl.power * 0.6;
    cl.oxygen = cl.oxygen * 0.6;
  }
});
```

**Key points:**
- System name changes from `flare` to `weather-tick` (more descriptive of expanded role).
- Query extended to `["Colony", "Clock", "Season", "Weather"]` — all four components on `colony` entity.
- `ctx.random_stream("weather")` used for weighted picking (E3 pattern — isolates weather RNG from main stream).
- `flare-hit` fires on transition INTO flare (not every tick while flare is active). The 40% cut applies once per flare onset.
- `flare-imminent` fires when current weather IS flare and timer drops below 30s (warning before flare ends). This is slightly different from the old behavior (which warned before flare started) — but since weather transitions are instant (no "next" preview), the imminent warning now means "flare is about to end." Document this in the report.
- The `Weather.next` field is declared in schema but NOT used by this implementation (transitions are instant, no preview). Leave it in schema for forward-compat (Task 7 HUD might show a forecast). Note this as a deviation in the report.

## Step 5: Apply season multiplier in crops.js

In `games/frontier/scripts/crops.js`:

Add the season multiplier constant and apply it in both the main system function and the catch_up function:

```javascript
const STAGE_SECONDS = 4.0;
const RIPE_STAGE = 3;
const CROP_DAY_SEC = 60.0;
const CROP_TICK_PER_SEC = 60;

// Season multipliers on crop growth rate. Spring is lush, autumn is peak harvest,
// summer is normal, winter is near-dormant.
const SEASON_CROP_MULT = {
  spring: 1.2, summer: 1.0, autumn: 1.5, winter: 0.3
};

// ... existing cropTodOf helper unchanged ...

vitric.system("crop-grow", { query: ["Crop", "Sprite"], writes: ["Crop", "Sprite"] }, (entities, ctx) => {
  const isNight = cropTodOf(ctx.tick) === "夜";
  // Fetch the season multiplier once per tick (not per entity).
  const season = ctx.getField("colony", "Season.current");
  const mult = SEASON_CROP_MULT[season] || 1.0;
  const dt = ctx.dt * mult;
  for (const e of entities) {
    const c = e.Crop;
    if (c.kind === "") { /* plot color */ continue; }
    if (isNight) continue;
    if (c.stage < RIPE_STAGE) {
      c.timer += dt;
      if (c.timer >= STAGE_SECONDS) {
        c.timer = 0;
        c.stage += 1;
        if (c.stage >= RIPE_STAGE) ctx.emit("crop-ready", {});
      }
    }
    /* paint sprite color by stage — unchanged */
  }
},
// catch_up: apply the same season multiplier to the dormant tick budget.
// The season at thaw time determines the growth rate for the entire dormant period
// (simplification: doesn't model season transitions during dormancy — that's a known
// approximation, noted in the report).
(entityHandle, ctx, dormantTicks) => {
  const dormantSec = dormantTicks / CROP_TICK_PER_SEC;
  const season = ctx.getField("colony", "Season.current");
  const mult = SEASON_CROP_MULT[season] || 1.0;
  let t = (ctx.getField(entityHandle, "Crop.timer") || 0) + dormantSec * mult;
  let s = ctx.getField(entityHandle, "Crop.stage") || 0;
  while (t >= STAGE_SECONDS && s < RIPE_STAGE) { t -= STAGE_SECONDS; s += 1; }
  ctx.setField(entityHandle, "Crop.timer", t);
  ctx.setField(entityHandle, "Crop.stage", s);
});
```

**Key points:**
- `ctx.getField("colony", "Season.current")` fetches the season by entity name (string ref — valid per `wish.js:18` pattern).
- Multiplier applied to `ctx.dt` BEFORE the `c.timer += dt` line.
- Catch_up applies the same multiplier to `dormantSec`. The season at thaw time is used for the entire dormant period (approximation — noted in report).
- `SEASON_CROP_MULT[season] || 1.0` falls back to 1.0 if season is somehow undefined (defensive — shouldn't happen but prevents a crash).

## Step 6: Apply weather + season multiplier in colony.js tally system

In `games/frontier/scripts/colony.js`:

**APPROACH**: Apply multipliers in the `tally` system BEFORE emitting the `tally` event. This way the existing `apply-rates` rule in `rules/colony.json` (which reads `event.pow` etc. and writes `@colony.Colony.pow_rate`) is UNCHANGED — the multiplier flows through the event payload.

```javascript
// Weather multipliers on colony production rates.
const WEATHER_RATE_MULT = {
  clear:  { power: 1.0, water: 1.0 },
  cloudy: { power: 0.7, water: 1.0 },
  rain:   { power: 0.7, water: 1.5 },
  storm:  { power: 0.3, water: 1.0 },
  flare:  { power: 0.0, water: 1.0 }
};

// Season multipliers on overall resource yield.
const SEASON_RESOURCE_MULT = {
  spring: 1.0, summer: 0.8, autumn: 1.2, winter: 0.5
};

// ... existing BASE_USE, PER constants unchanged ...

vitric.system("tally", { query: ["Structure"] }, (entities, ctx) => {
  let conduit = 0, plot = 0, extractor = 0, monument = 0;
  for (const e of entities) {
    const s = e.Structure;
    if (s.kind === "conduit") conduit += 1;
    else if (s.kind === "plot") plot += 1;
    else if (s.kind === "extractor") extractor += 1;
    else if (s.kind === "monument") monument += 1;
  }
  // Fetch weather + season multipliers.
  const weather = ctx.getField("colony", "Weather.current");
  const season = ctx.getField("colony", "Season.current");
  const wmult = WEATHER_RATE_MULT[weather] || WEATHER_RATE_MULT.clear;
  const smult = SEASON_RESOURCE_MULT[season] || 1.0;

  // Apply multipliers to the emitted rates — the apply-rates rule is unchanged.
  ctx.emit("tally", {
    pow: conduit * PER * wmult.power * smult,
    food: plot * PER * smult,
    o2: extractor * PER * smult,
    water: extractor * PER * wmult.water * smult,
    total: conduit + plot + extractor,
    monument
  });
});
```

**Key points:**
- The `tally` system query stays `["Structure"]` — it doesn't need Colony/Weather/Season because it fetches via `ctx.getField("colony", ...)`.
- Multipliers applied to the EMITTED values, not to the stockpile directly. The `apply-rates` rule (in `rules/colony.json`) writes `@colony.Colony.pow_rate = event.pow` etc. — unchanged.
- `WEATHER_RATE_MULT[weather] || WEATHER_RATE_MULT.clear` falls back to clear-weather rates if weather is undefined (defensive).
- The `colony` stockpile system (lines 67-80) is UNCHANGED — it applies `c.pow_rate` etc. as-is. The multiplier is already baked into the rate.
- `water` gets both `wmult.water` (rain bonus) and `smult` (season). `food` gets only `smult` (no weather effect on food). `o2` gets only `smult`. `pow` gets both `wmult.power` (flare zero) and `smult`.

## Step 7: Update rules

### 7a. Add weather-change toast to `rules/flare.json`

Append a new rule to `games/frontier/rules/flare.json` (keep existing 4 rules unchanged):

```json
{
  "id": "toast-weather-change",
  "on": { "event": "weather-change" },
  "do": [
    { "call": "toast", "with": { "text": "天气变化：{event.weather}", "timer": 2.0 } }
  ]
}
```

**Note**: look at the existing toast rules for the exact `call`/`with` format — the `text` field likely uses a template syntax. Match the existing pattern. If the existing rules use Chinese labels for weather, use them here too. The `WEATHER_LABELS` map in `flare.js` has the Chinese labels — but the toast can use the English enum value if that's what existing toasts do. Check `rules/flare.json` existing rules and match the style.

### 7b. Add HUD rules to `rules/hud.json`

Append two new rules to `games/frontier/rules/hud.json` (keep existing 5 rules unchanged):

```json
{
  "id": "hud-season",
  "on": { "event": "tick" },
  "do": [
    { "set": "@season_lbl.UiLabel.content", "to": "第 {@colony.Season.year} 年 · {@colony.Season.current}" }
  ]
},
{
  "id": "hud-weather",
  "on": { "event": "tick" },
  "do": [
    { "set": "@weather_lbl.UiLabel.content", "to": "天气：{@colony.Weather.current}" }
  ]
}
```

**Note**: look at the existing `hud-stage` rule for the exact `set`/`to` format — it likely uses `@colony.Colony.stage` path syntax. Match it. The `@season_lbl` and `@weather_lbl` entities were added in Step 2b.

## Step 8: Run schema check

Run: `~/.cargo/bin/cargo run --release -- check games/frontier`
Expected: PASS (schema validates, all referenced fields are declared).

If it fails: check the error message for the missing field, add it to `schema.json`, re-run.

## Step 9: Write season advance + weather timer integration test

Create `crates/vitric-cli/tests/seasons.rs`:

```rust
//! Task 6 (Seasons & Weather) integration tests.
//!
//! Verifies: (a) season advances on day-wrap boundary; (b) season rolls over at 12 days;
//! (c) weather timer decrements each tick; (d) weather-tick system runs without crashing.

use std::path::PathBuf;

use serde_json::json;
use vitric_cli::runtime::Runtime;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

#[test]
fn season_advances_on_day_boundary() {
    // Fast test: set Clock.time to just below CLOCK_DAY_SEC (60.0) and Season.day_in_season
    // to 0, then step 1 tick. The day-wrap fires, day_in_season increments to 1.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    // Set time to 59.99 — one tick (dt=1/60≈0.0167) will push it past 60.0, triggering day-wrap.
    let mut clock = sim.world.get_component(colony_e, "Clock").unwrap().clone();
    clock["time"] = json!(59.99);
    clock["day"] = json!(1);
    clock["last_day_emit"] = json!(1); // Suppress day-start emission noise.
    sim.world.set_component(colony_e, "Clock", clock).unwrap();

    let mut season = sim.world.get_component(colony_e, "Season").unwrap().clone();
    season["day_in_season"] = json!(0);
    season["current"] = json!("spring");
    season["year"] = json!(1);
    sim.world.set_component(colony_e, "Season", season).unwrap();

    sim.step(&mut rt).unwrap();

    let season_after = sim.world.get_component(colony_e, "Season").unwrap();
    assert_eq!(season_after["day_in_season"].as_i64(), Some(1),
        "day_in_season should increment to 1 after one day-wrap");
    assert_eq!(season_after["current"].as_str(), Some("spring"),
        "season should still be spring (only 1 day into the season)");
    assert_eq!(season_after["year"].as_i64(), Some(1),
        "year should still be 1");
}

#[test]
fn season_rolls_over_at_12_days() {
    // Set day_in_season to 11 (last day of season), then trigger a day-wrap.
    // The season should roll over from spring to summer, day_in_season reset to 0.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    let mut clock = sim.world.get_component(colony_e, "Clock").unwrap().clone();
    clock["time"] = json!(59.99);
    clock["day"] = json!(12);
    clock["last_day_emit"] = json!(12);
    sim.world.set_component(colony_e, "Clock", clock).unwrap();

    let mut season = sim.world.get_component(colony_e, "Season").unwrap().clone();
    season["day_in_season"] = json!(11);
    season["current"] = json!("spring");
    season["year"] = json!(1);
    sim.world.set_component(colony_e, "Season", season).unwrap();

    sim.step(&mut rt).unwrap();

    let season_after = sim.world.get_component(colony_e, "Season").unwrap();
    assert_eq!(season_after["day_in_season"].as_i64(), Some(0),
        "day_in_season should reset to 0 after season rollover");
    assert_eq!(season_after["current"].as_str(), Some("summer"),
        "season should roll over from spring to summer");
    assert_eq!(season_after["year"].as_i64(), Some(1),
        "year should still be 1 (only rolls over on spring→spring wrap)");
}

#[test]
fn year_increments_on_spring_wrap() {
    // Set season to winter, day_in_season to 11. Trigger day-wrap.
    // Season should roll to spring, year should increment to 2.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    let mut clock = sim.world.get_component(colony_e, "Clock").unwrap().clone();
    clock["time"] = json!(59.99);
    clock["day"] = json!(48);
    clock["last_day_emit"] = json!(48);
    sim.world.set_component(colony_e, "Clock", clock).unwrap();

    let mut season = sim.world.get_component(colony_e, "Season").unwrap().clone();
    season["day_in_season"] = json!(11);
    season["current"] = json!("winter");
    season["year"] = json!(1);
    sim.world.set_component(colony_e, "Season", season).unwrap();

    sim.step(&mut rt).unwrap();

    let season_after = sim.world.get_component(colony_e, "Season").unwrap();
    assert_eq!(season_after["current"].as_str(), Some("spring"),
        "season should wrap from winter to spring");
    assert_eq!(season_after["year"].as_i64(), Some(2),
        "year should increment on winter→spring wrap");
    assert_eq!(season_after["day_in_season"].as_i64(), Some(0),
        "day_in_season should reset to 0");
}

#[test]
fn weather_timer_decrements_each_tick() {
    // Boot sim, step 1 tick, verify Weather.timer decreased by ~dt (1/60).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    let weather_before = sim.world.get_component(colony_e, "Weather").unwrap().clone();
    let timer_before = weather_before["timer"].as_f64().unwrap();

    sim.step(&mut rt).unwrap();

    let weather_after = sim.world.get_component(colony_e, "Weather").unwrap();
    let timer_after = weather_after["timer"].as_f64().unwrap();

    // The weather-tick system decrements timer by ctx.dt (1/60 ≈ 0.0167).
    // Allow small floating-point slack.
    let elapsed = timer_before - timer_after;
    assert!(elapsed > 0.0 && elapsed < 0.1,
        "timer should decrement by ~dt (1/60≈0.0167), got elapsed={elapsed}, before={timer_before}, after={timer_after}");
    assert_eq!(weather_after["current"].as_str(), weather_before["current"].as_str(),
        "weather should not change on a single tick (timer hasn't expired)");
}
```

**Key points:**
- Tests are FAST — they step 1 tick each, not 43200 ticks. They set up the boundary condition directly (Clock.time = 59.99) and verify the logic fires.
- Tests use `Runtime::boot(&frontier_dir())` + `sim.world.entity("colony")` + `sim.world.get_component` / `set_component` to set up state, then `sim.step(&mut rt)` to advance one tick.
- The `weather_timer_decrements_each_tick` test doesn't assert a specific weather outcome (RNG-dependent) — it only verifies the timer decrements and weather doesn't change prematurely.
- Do NOT test the full season cycle (48 days × 3600 ticks = 172800 ticks — too slow). The boundary tests verify the logic; the full cycle is validated by the gate recording in Task 15.

## Step 10: Run tests + schema check + gate (expected to fail)

Run:
1. `~/.cargo/bin/cargo test -p vitric-cli --test seasons --` — must PASS (4 new tests)
2. `~/.cargo/bin/cargo test -p vitric-cli --test region -- --skip typescript` — must PASS (existing region tests, no regression)
3. `~/.cargo/bin/cargo test --workspace -- --skip typescript` — must PASS (full suite minus pre-existing typescript failures)
4. `~/.cargo/bin/cargo run --release -- check games/frontier` — must PASS (schema validates)
5. `~/.cargo/bin/cargo run --release -- gate games/frontier` — EXPECTED TO FAIL with `ReplayDiverged` (the recording's hash doesn't match because tick-0 world state changed and trajectory diverged). Do NOT fix. Note the failure in the report.

## Step 11: Commit

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json games/frontier/scripts/clock.js games/frontier/scripts/flare.js games/frontier/scripts/crops.js games/frontier/scripts/colony.js games/frontier/rules/flare.json games/frontier/rules/hud.json crates/vitric-cli/tests/seasons.rs
git commit -m "feat(frontier): seasons & weather system

Add Season (4 seasons × 12 days = 48-day year) and Weather (5 states
weighted by season) components. Refactor flare.js into weather-tick
system (flare is now a weather variant, not a standalone timer).
Preserve night-fall/dawn-break/flare-hit/flare-imminent events.

Apply season multipliers to crop growth (spring 1.2, summer 1.0,
autumn 1.5, winter 0.3) and weather+season multipliers to colony
resource rates (multipliers applied in tally system before emit,
so apply-rates rule is unchanged).

Weather weighted picking uses ctx.random_stream('weather') for
replay-stable RNG isolation (E3 pattern).

Gate hash changes (expected — recording re-recorded in Task 15).
Schema check + workspace tests pass."
git push origin main
```

## Report contract

Write your full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/briefs/task-6-report.md` with sections:
1. **Status**: DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED
2. **Commits**: list commit SHA(s) pushed to main
3. **Test results**: table of commands and pass/fail (include the gate failure — note it as EXPECTED)
4. **Files touched**: table of file + line count changes
5. **Deviations from brief**: any deviations (e.g., if you found a simpler way, or if a rule format didn't match the existing pattern)
6. **Concerns**: any doubts or observations the reviewer should know. Specifically flag:
   - The `Weather.next` field is declared but unused (forward-compat for Task 7 forecast).
   - The `flare-imminent` semantic change (now warns before flare ENDS, not before it STARTS).
   - The catch_up season approximation (uses season-at-thaw for entire dormant period).
   - Any rule format adjustments you made to match existing `rules/*.json` patterns.

Return in your response: status, commit SHAs, one-line test summary, and concerns (if any).
