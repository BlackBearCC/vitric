// 生存底盘：殖民地 氧/电/食/水 随时间掉，靠结构产出续上；消耗随人口涨（本增量人口=0）。
// 单系统不能跨实体聚合，所以分两步：
//   tally 系统数产出结构 → emit "tally" 速率 → rules/colony.json 落到 @colony.*_rate
//   colony 系统每帧按 (产出 - 基础消耗) 调库存，夹在 [0,100]，不死人。
// 产出映射（GDD 建造表）：plot→食+氧，conduit→电，extractor→水。
// 本增量砍掉：census/伙伴普查（无伙伴）、cosmic 耀斑（GDD 明确砍）。pop 恒 0。

const BASE_USE = 1.4;     // 每秒固定底噪（电/氧/食/水各自）——殖民地活着就在烧。给宽，别盖过经营节奏。
const PER = 3.0;          // 每个产出结构每秒产量

function clamp(v) {
  return v < 0 ? 0 : (v > 100 ? 100 : v);
}

// 数产出结构，把"每秒产量"发出去（规则落进 @colony 的速率字段 + 结构总数）。
// 总在跑（只要场上有 Structure 就数；没有结构时 tally 不发，速率保持上一次——
// 但初始无结构时速率默认 0，库存只掉底噪，符合"先建设再续命"）。
vitric.system("tally", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  let conduit = 0, plot = 0, extractor = 0;
  for (const e of entities) {
    const k = e.Structure.kind;
    if (k === "conduit") conduit += 1;
    else if (k === "plot") plot += 1;
    else if (k === "extractor") extractor += 1;
  }
  ctx.emit("tally", {
    pow: conduit * PER,
    food: plot * PER,
    o2: plot * PER,
    water: extractor * PER,
    total: entities.length,
  });
});

// 阶段：随结构数成长，落脚 → 立足 → 成形 → 兴旺（进展感）。本增量人手恒 0，
// "兴旺"需人手≥2（伙伴未做），所以本增量最多到"成形"——符合纵切，伙伴一来即可兴旺。
vitric.system("stage", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const s = e.Colony.struct_count;
    const n = e.Colony.pop;
    let stage = "落脚";
    if (s >= 6 && n >= 2) stage = "兴旺";
    else if (s >= 3) stage = "成形";
    else if (s >= 1) stage = "立足";
    e.Colony.stage = stage;
  }
});

// 每帧：库存 += (产出速率 - 基础消耗) * dt，夹 [0,100]；取整给 HUD 读。
// pop 恒 0（无伙伴），所以消耗就是底噪，产出就是结构速率。
vitric.system("colony", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const c = e.Colony;
    const draw = BASE_USE; // 人口 0：无人均消耗
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
