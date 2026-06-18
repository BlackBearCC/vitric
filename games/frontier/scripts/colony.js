// 生存底盘 + 阶段里程碑:
//   殖民地 氧/电/食/水 随时间掉,靠结构产出续上;消耗随人口涨。
//   阶段从结构数升级为"靠天数 + 多维度达成"驱动,让游戏是 5-7 天的纵切。
//
// 系统:
//   tally       数产出结构 → emit "tally" 速率 → rules/colony.json 落 @colony.*_rate
//   census      数伙伴 → emit "census-tick" → rules 落 @colony.Colony.pop
//   stage       阶段里程碑:起步 → 立足 → 成形 → 温饱 → 成群 → 兴旺
//   colony      每帧按 (产出 - 基础消耗) 调库存,夹在 [0,100],不死人

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

// 阶段:天数 + 多维度判定。
//   起步    (默认,day 1)
//   立足    (day≥3 且 结构≥3)
//   成形    (day≥4 且 结构≥5)
//   成群    (day≥5 且 人手≥3)
//   兴旺    (day≥6 且 丰碑已立)
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

// 殖民地库存：每帧按 (产出 - 基础消耗) 调库存。
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

// 丰碑标记:任何时候场上存在 monument 结构 → @colony.Colony.monument_built = 1
vitric.system("monument-watch", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  let monument = 0;
  for (const e of entities) if (e.Structure.kind === "monument") monument += 1;
  if (monument > 0) ctx.emit("monument-present", { count: monument });
});
