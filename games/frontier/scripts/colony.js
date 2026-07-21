// Survival baseline + stage milestones:
//   Colony oxygen/power/food/water decay over time, sustained by structure output; consumption scales with population.
//   Stages upgrade from structure-count to "days + multi-dimensional achievements" driven, making the game a 5-7 day vertical slice.
//
// Systems:
//   tally       count output structures -> emit "tally" rate -> rules/colony.json writes @colony.*_rate
//   census      count companions -> emit "census-tick" -> rules writes @colony.Colony.pop
//   stage       stage milestones: startup -> foothold -> taking shape -> warmth -> crowd -> prosperity
//   colony      each frame adjust stockpile by (output - base consumption), clamped to [0,100], no death
//
// Task 6: tally now applies weather + season multipliers to the EMITTED rates (before the apply-rates
//   rule writes them to @colony.*_rate). This keeps rules/colony.json unchanged — the multipliers
//   flow through the event payload. The colony stockpile system below is unchanged (applies rates as-is).

const BASE_USE = 1.4;
const PER = 3.0;

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

function clamp(v) {
  return v < 0 ? 0 : (v > 100 ? 100 : v);
}

vitric.system("tally", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  let conduit = 0, plot = 0, extractor = 0, monument = 0;
  for (const e of entities) {
    const k = e.Structure.kind;
    if (k === "conduit") conduit += 1;
    else if (k === "plot" || k === "plot2") plot += 1;
    else if (k === "extractor") extractor += 1;
    else if (k === "monument") monument += 1;
  }
  // Fetch weather + season multipliers.
  const weather = ctx.getField("colony", "Weather.current");
  const season = ctx.getField("colony", "Season.current");
  const wmult = WEATHER_RATE_MULT[weather] || WEATHER_RATE_MULT.clear;
  const smult = SEASON_RESOURCE_MULT[season] || 1.0;

  // Apply multipliers to the emitted rates — the apply-rates rule is unchanged.
  // Sources match the original tally (o2 from plots, water from extractors) — only the
  // multipliers are new. The brief's pseudocode showed o2 from extractors, but the brief's
  // text explicitly says "o2 gets only smult" (multiplier only, no source change). Keeping
  // the original plot source to avoid an unintended behavior change.
  ctx.emit("tally", {
    pow: conduit * PER * wmult.power * smult,
    food: plot * PER * smult,
    o2: plot * PER * smult,
    water: extractor * PER * wmult.water * smult,
    total: entities.length,
    monument: monument,
  });
});

vitric.system("census", { query: ["Census"], writes: [] }, (entities, ctx) => {
  let pop = 0;
  for (const e of entities) {
    if (e.Census.is_hub) continue;
    pop += 1;
  }
  ctx.emit("census-tick", { pop: pop });
});

// Stages: compound conditions tied to seasons/years (spec §4.7).
//   起步 (day 1-3)          — default, no requirement
//   立足 (end of spring, day>=12)  — survival_t1 researched AND struct >= 5
//   成形 (end of summer, day>=24)  — pop >= 3 AND agriculture_t1 researched
//   成群 (end of year 1, day>=48)  — pop >= 5 AND any faction tier >= neutral
//   兴旺 (end of year 2, day>=96)  — all 4 branches T2+ AND monument built AND any faction allied
// Sandbox continues after 兴旺 — no ending stage.
// Transitions are monotonic by day-floor: if day >= 96 but 兴旺 conditions not met, stage stays at 成群
// (the highest stage whose day-floor + conditions are both satisfied).
vitric.system("stage", { query: ["Colony", "Clock"], writes: ["Colony"] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const day = c.Clock.day;
  const s = c.Colony.struct_count;
  const pop = c.Colony.pop;
  const monument = c.Colony.monument_built | 0;

  // Read Research fields (on the same colony entity — Colony+Research are both attached to "colony").
  const hasSurvivalT1 = (ctx.getField("colony", "Research.has_survival_t1") | 0) === 1;
  const hasSurvivalT2 = (ctx.getField("colony", "Research.has_survival_t2") | 0) === 1;
  const hasAgriT1 = (ctx.getField("colony", "Research.has_agriculture_t1") | 0) === 1;
  const hasAgriT2 = (ctx.getField("colony", "Research.has_agriculture_t2") | 0) === 1;
  const hasExplT2 = (ctx.getField("colony", "Research.has_exploration_t2") | 0) === 1;
  const hasIndT2 = (ctx.getField("colony", "Research.has_industry_t2") | 0) === 1;

  // Read Faction tiers (on the same colony entity — Faction is attached to "colony").
  const tierNomads = ctx.getField("colony", "Faction.tier_nomads") || "wary";
  const tierCaravan = ctx.getField("colony", "Faction.tier_caravan") || "wary";
  const tierRemnant = ctx.getField("colony", "Faction.tier_remnant") || "wary";
  const anyFactionNeutralOrBetter = ["neutral", "friendly", "allied"].includes(tierNomads)
    || ["neutral", "friendly", "allied"].includes(tierCaravan)
    || ["neutral", "friendly", "allied"].includes(tierRemnant);
  const anyFactionAllied = tierNomads === "allied" || tierCaravan === "allied" || tierRemnant === "allied";

  const allT2 = hasSurvivalT2 && hasAgriT2 && hasExplT2 && hasIndT2;

  // Check from highest stage downward; first match wins.
  let stage = "起步";
  if (day >= 96 && allT2 && monument >= 1 && anyFactionAllied) stage = "兴旺";
  else if (day >= 48 && pop >= 5 && anyFactionNeutralOrBetter) stage = "成群";
  else if (day >= 24 && pop >= 3 && hasAgriT1) stage = "成形";
  else if (day >= 12 && hasSurvivalT1 && s >= 5) stage = "立足";

  if (c.Colony.stage !== stage) c.Colony.stage = stage;
});

// Colony stockpile: each frame adjust stockpile by (output - base consumption).
vitric.system("colony", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const c = e.Colony;
    const draw = BASE_USE + c.pop * 0.5;
    c.power = clamp(c.power + (c.pow_rate - draw) * ctx.dt);
    c.oxygen = clamp(c.oxygen + (c.o2_rate - draw) * ctx.dt);
    c.food = clamp(c.food + (c.food_rate - draw) * ctx.dt);
    c.water = clamp(c.water + (c.water_rate - draw) * ctx.dt);
    c.o2_i = Math.round(c.oxygen);
    c.pow_i = Math.round(c.power);
    c.food_i = Math.round(c.food);
    c.water_i = Math.round(c.water);
  }
});

// Monument flag: at any time if a monument structure exists on the field -> @colony.Colony.monument_built = 1
vitric.system("monument-watch", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  let monument = 0;
  for (const e of entities) if (e.Structure.kind === "monument") monument += 1;
  if (monument > 0) ctx.emit("monument-present", { count: monument });
});
