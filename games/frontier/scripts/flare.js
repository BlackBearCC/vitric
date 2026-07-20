// Weather + day/night cycle (Task 6 refactor): drives survival pressure on top of the clock.
//   Colony carries flare_warning / is_night / wild_threat (Colony component) — preserved from old flare system.
//   This system queries the same @colony entity for Colony, Clock, Season, Weather.
//
// Day/night (preserved from old flare system):
//   Night begins at 75% of a day (matches clock.js time-of-day bands). On a 0->1 flip we arm
//   wild_threat (scales with day count) and emit night-fall; on 1->0 we clear it and emit dawn-break.
//
// Weather (new in Task 6):
//   Replaces the old standalone flare_timer. Weather.timer counts down by dt; when it hits 0 a new
//   weather state is picked via ctx.random_stream("weather") with weights drawn from the current
//   season. Flare is one of 5 weather variants (clear/cloudy/rain/storm/flare), only rolls in summer.
//
//   flare-hit fires on transition INTO flare (preserves the 40% power/oxygen cut, payload carries
//     pre-hit values). flare-imminent fires when current weather IS flare and timer drops below 30s
//     (warning before flare ENDS — semantic change from the old "warning before flare STARTS"; see
//     task-6-report.md).

// Mirrors CLOCK_DAY_SEC in clock.js (60s/day). Renamed to avoid redeclaring the shared global.
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
      // Wild threat scales loosely with day count (1 early, ~3+ in late sessions).
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
    // (Semantic change from old behavior: now warns before flare ENDS, not before it STARTS —
    // since weather transitions are instant, there is no "approaching flare" state to warn about.)
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
