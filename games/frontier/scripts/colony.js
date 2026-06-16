// 生存系统(P2):殖民地资源(氧/电/食)随时间消耗,产出结构续上。
// 单系统不能跨实体聚合,所以分两步:
//   tally 系统统计产出结构 → emit "tally" 速率 → rules/colony.json 落到 @colony.*_rate
//   colony 系统每帧按 (产出 - 基础消耗) 调库存。
// 产出映射:conduit→电,plot(种植/水培)→食 + 氧。

const BASE_USE = 2.0;     // 每秒固定底噪(电/氧/食/水各自)——殖民地活着就在烧
const PER = 3.0;          // 每个产出结构每秒产量
const DRAW_PER_POP = 0.8; // 每个居民每秒额外消耗(呼吸/吃喝/用水电):人越多,胃口越大
const POP_HELP = 0.4;     // 每个居民每秒帮的产出(干活)。比人均消耗小 → 多一个人是净负担,
                          // 逼你扩产能去撑住——这才是心脏 C:伙伴是你建设的目标,养住他们就是挑战。

function clamp(v) {
  return v < 0 ? 0 : (v > 100 ? 100 : v);
}

// 普查:数在场伙伴,写回标了 is_hub 的 @colony。这个系统总在跑(@colony 永远在),
// 所以伙伴走光也能归零——按 [Companion] 查的系统在 0 实体时根本不跑,计数会滞留。
vitric.system("census", { query: ["Census"], writes: ["Census"] }, (entities, ctx) => {
  let companions = 0;
  let hub = null;
  for (const e of entities) {
    if (e.Census.is_hub > 0) hub = e;
    else companions += 1;
  }
  if (hub) hub.Census.count = companions;
});

// 统计产出结构,把"每秒产量"发出去(规则把它落进 @colony 的速率字段)。
vitric.system("tally", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  let conduit = 0;
  let plot = 0;
  let quarters = 0;
  let extractor = 0;
  for (const e of entities) {
    if (e.Structure.kind === "conduit") conduit += 1;
    else if (e.Structure.kind === "plot") plot += 1;
    else if (e.Structure.kind === "quarters") quarters += 1;
    else if (e.Structure.kind === "extractor") extractor += 1;
  }
  // quarters 数给伙伴需求系统用;total 给阶段系统(殖民地发展度)用
  ctx.emit("tally", { pow: conduit * PER, food: plot * PER, o2: plot * PER, water: extractor * PER, quarters: quarters, total: entities.length });
});

// 阶段:殖民地随"结构数 + 在场伙伴数"成长,落脚 → 立足 → 成形 → 兴旺(游戏曲线的进展感)。
// 总在跑(@colony 有 Colony+Census),所以即便没人/没结构也对。
vitric.system("stage", { query: ["Colony", "Census"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const s = e.Colony.struct_count;
    const n = e.Census.count;
    let stage = "落脚";
    if (s >= 6 && n >= 3) stage = "兴旺";
    else if (s >= 3 && n >= 2) stage = "成形";
    else if (s >= 1) stage = "立足";
    e.Colony.stage = stage;
  }
});

// 每帧:库存 += (产出速率 - 基础消耗) * dt,夹在 [0,100]。
vitric.system("colony", { query: ["Colony", "Census"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const c = e.Colony;
    const pop = e.Census.count;            // 普查的实时人数
    const draw = BASE_USE + pop * DRAW_PER_POP; // 总消耗随人口涨(底噪 + 每人一份胃口)
    const work = pop * POP_HELP;                // 居民帮的产出(< 人均消耗,所以人是净负担)
    c.power = clamp(c.power + (c.pow_rate + work - draw) * ctx.dt);
    c.oxygen = clamp(c.oxygen + (c.o2_rate + work - draw) * ctx.dt);
    c.food = clamp(c.food + (c.food_rate + work - draw) * ctx.dt);
    c.water = clamp(c.water + (c.water_rate + work - draw) * ctx.dt);
    // 取整给 HUD 显示用(format 模板直接读这几个,免得屏上是 53.9999)
    c.o2_i = Math.round(c.oxygen);
    c.pow_i = Math.round(c.power);
    c.food_i = Math.round(c.food);
    c.water_i = Math.round(c.water);
  }
});

// 宇宙事件:每隔一阵来一次太阳耀斑,电和氧骤降——给生存加动态张力(你得留缓冲、靠建设扛过去)。
const FLARE_INTERVAL = 25.0;
const FLARE_POWER = 35;
const FLARE_O2 = 20;
const FLASH_SECONDS = 3.0; // 警告亮多久
vitric.system("cosmic", { query: ["Colony", "Event"], writes: ["Colony", "Event"] }, (entities, ctx) => {
  for (const e of entities) {
    const ev = e.Event;
    if (ev.flash > 0) ev.flash = Math.max(0, ev.flash - ctx.dt);
    ev.timer -= ctx.dt;
    if (ev.timer <= 0) {
      ev.timer = FLARE_INTERVAL;
      ev.flash = FLASH_SECONDS;
      e.Colony.power = Math.max(0, e.Colony.power - FLARE_POWER);
      e.Colony.oxygen = Math.max(0, e.Colony.oxygen - FLARE_O2);
      ctx.emit("flare", {});
    }
  }
});
