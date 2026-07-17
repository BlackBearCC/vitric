// Solar flare + day/night cycle: drives survival pressure on top of the clock.
//   Colony carries flare_timer / flare_warning / is_night / wild_threat (added in Task 1).
//   This system queries the same @colony entity for both Colony and Clock, like the `stage` system does.
//
// Day/night:
//   Night begins at 75% of a day (matches clock.js time-of-day bands). On a 0->1 flip we arm
//   wild_threat (scales with day count) and emit night-fall; on 1->0 we clear it and emit dawn-break.
//
// Flare:
//   flare_timer counts down by dt. Within 30s of impact we raise flare_warning and emit flare-imminent.
//   At 0 we emit flare-hit (carrying the pre-hit power/oxygen losses), shave 40% off both, then
//   reset the timer to a 180-300s cooldown.

// Mirrors CLOCK_DAY_SEC in clock.js (60s/day). Renamed to avoid redeclaring the shared global.
const FLARE_DAY_SEC = 60.0;

vitric.system("flare", { query: ["Colony", "Clock"], writes: ["Colony"] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const cl = c.Colony;
  const ck = c.Clock;

  // --- Day / night detection ---
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

  // --- Flare timer ---
  const timer = cl.flare_timer - ctx.dt;
  if (timer <= 0) {
    // Flare strikes: emit losses computed from pre-hit stockpile, then apply the 40% cut.
    ctx.emit("flare-hit", {
      power_loss: cl.power * 0.4,
      o2_loss: cl.oxygen * 0.4,
    });
    cl.power = cl.power * 0.6;
    cl.oxygen = cl.oxygen * 0.6;
    cl.flare_warning = 0;
    cl.flare_timer = 180 + Math.floor(Math.random() * 120);
    return;
  }
  // Warning band: arm at <=30s, clear if it drifts back above 30s.
  if (timer <= 30) {
    if (cl.flare_warning !== 1) {
      cl.flare_warning = 1;
      ctx.emit("flare-imminent", { eta: timer });
    }
  } else if (cl.flare_warning !== 0) {
    cl.flare_warning = 0;
  }
  cl.flare_timer = timer;
});
