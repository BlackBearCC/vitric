// Time / day-night cycle: the pacing backbone of the whole game.
//   @colony carries a Clock component -> day/time/tod three fields keep advancing, emitting day-start each day.
//   dt comes from the engine, 1 unit = 1 second of game time (at 60 tick/sec, one tick is 1/60 second).
//   1 game day = DAY_SEC seconds; in this version DAY_SEC = 60 — measured at about 1 minute/day,
//   10-15 minutes per session fits a 5-7 day vertical slice nicely.
//
// Time of day:
//   morning    0% – 25%   wake up, farm, gather, build
//   noon      25% – 50%   strongest sunlight, crops grow fast, drifters active
//   dusk      50% – 75%   wrap up work, head home, strong community vibe
//   night     75% – 100%  companions must rest in quarters; crops dormant
//
// At the end of each tick emit time-tick{day, time, tod} — rules/scripts can listen (crop dormancy hooks in here).
//
// Season advance (Task 6): @colony also carries a Season component. Each day-wrap increments
//   Season.day_in_season; when it reaches SEASON_DAYS (12) the season rolls over
//   (spring→summer→autumn→winter→spring) and year increments on the winter→spring wrap.

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
    // Time-of-day label
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

    // On the wrap moment of each day emit day-start once (one per Clock instance)
    if (dayJustWrapped && e.Clock.last_day_emit !== e.Clock.day) {
      e.Clock.last_day_emit = e.Clock.day;
      ctx.emit("day-start", { day: e.Clock.day });
    }
    ctx.emit("time-tick", { day: e.Clock.day, tod: e.Clock.tod });
  }
});
