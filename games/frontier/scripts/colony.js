// Survival baseline + stage milestones:
//   Colony oxygen/power/food/water decay over time, sustained by structure output; consumption scales with population.
//   Stages upgrade from structure-count to "days + multi-dimensional achievements" driven, making the game a 5-7 day vertical slice.
//
// Systems:
//   tally       count output structures -> emit "tally" rate -> rules/colony.json writes @colony.*_rate
//   census      count companions -> emit "census-tick" -> rules writes @colony.Colony.pop
//   stage       stage milestones: startup -> foothold -> taking shape -> warmth -> crowd -> prosperity
//   colony      each frame adjust stockpile by (output - base consumption), clamped to [0,100], no death

const BASE_USE = 1.4;
const PER = 3.0;

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
  ctx.emit("tally", {
    pow: conduit * PER,
    food: plot * PER,
    o2: plot * PER,
    water: extractor * PER,
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

// Stages: days + multi-dimensional judgment.
//   startup         (default, day 1)
//   foothold        (day>=3 and structures>=3)
//   taking shape    (day>=4 and structures>=5)
//   crowd           (day>=5 and hands>=3)
//   prosperity      (day>=6 and monument built)
vitric.system("stage", { query: ["Colony", "Clock"], writes: ["Colony"] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const day = c.Clock.day;
  const s = c.Colony.struct_count;
  const pop = c.Colony.pop;
  let stage = "起步";
  if (c.Colony.monument_built && day >= 6) stage = "兴旺";
  else if (day >= 5 && pop >= 3) stage = "成群";
  else if (day >= 4 && s >= 5) stage = "成形";
  else if (day >= 3 && s >= 3) stage = "立足";
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
