// Crop growth: when a plot is built it carries an empty Crop component (kind=""); after the interaction click plants, kind="wheat".
// The crop grows on the plot entity itself (its Crop component), written in place via the engine's ctx.setField.
//
// Pacing: 3 stages × 4s each (12s total); at night the timer pauses — one cycle per day (12/60 = 20% of a day length of growth,
// "a few hours of work"). stage 0/1/2 growing, stage 3 ripe (harvestable).
// Each frame timer += dt (frozen at night); at 4s it advances a stage and resets timer; at stage 3 it stops and waits for harvest.
//
// Time of day is derived from ctx.tick (same source as DAY_SEC in clock.js; the two must stay in sync).

const STAGE_SECONDS = 4.0;
const RIPE_STAGE = 3;
const PLOT_COLOR = "#6b8f3a";
const STAGE_COLOR = ["#7fbf5a", "#5fa83a", "#3f8f2a", "#e8c83a"];
const CROP_DAY_SEC = 60.0;
const CROP_TICK_PER_SEC = 60;

function cropTodOf(tick) {
  const secOfDay = (tick / CROP_TICK_PER_SEC) % CROP_DAY_SEC;
  const frac = secOfDay / CROP_DAY_SEC;
  if (frac < 0.25) return "晨";
  if (frac < 0.50) return "午";
  if (frac < 0.75) return "昏";
  return "夜";
}

vitric.system("crop-grow", { query: ["Crop", "Sprite"], writes: ["Crop", "Sprite"] }, (entities, ctx) => {
  const isNight = cropTodOf(ctx.tick) === "夜";
  for (const e of entities) {
    const c = e.Crop;
    if (c.kind === "") {
      if (e.Sprite.color !== PLOT_COLOR) e.Sprite.color = PLOT_COLOR;
      continue;
    }
    if (isNight) continue; // night: crop dormant, timer frozen, color preserved
    if (c.stage < RIPE_STAGE) {
      c.timer += ctx.dt;
      if (c.timer >= STAGE_SECONDS) {
        c.timer = 0;
        c.stage += 1;
        if (c.stage >= RIPE_STAGE) ctx.emit("crop-ready", {});
      }
    }
    const color = STAGE_COLOR[c.stage] || STAGE_COLOR[RIPE_STAGE];
    if (e.Sprite.color !== color) e.Sprite.color = color;
  }
},
// catch_up: fast-forward Crop.timer/stage by the dormant tick budget when a region thaws.
// Simplified reconciliation — ONLY advances timer and stage, NOT other side effects:
// no emit (crop-ready will fire on the next regular tick if the crop reached ripe),
// no Sprite.color update (the main fn paints that next tick), no night check (dormant
// time is treated as continuous growth, since the regular fn already paused at night).
// dormant_ticks is in ticks (60 ticks = 1 second); convert to seconds and roll stages.
(entityHandle, ctx, dormantTicks) => {
  const dormantSec = dormantTicks / CROP_TICK_PER_SEC;
  let t = (ctx.getField(entityHandle, "Crop.timer") || 0) + dormantSec;
  let s = ctx.getField(entityHandle, "Crop.stage") || 0;
  while (t >= STAGE_SECONDS && s < RIPE_STAGE) {
    t -= STAGE_SECONDS;
    s += 1;
  }
  ctx.setField(entityHandle, "Crop.timer", t);
  ctx.setField(entityHandle, "Crop.stage", s);
});
