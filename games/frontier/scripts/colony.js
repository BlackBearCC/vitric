// 生存系统(P2):殖民地资源(氧/电/食)随时间消耗,产出结构续上。
// 单系统不能跨实体聚合,所以分两步:
//   tally 系统统计产出结构 → emit "tally" 速率 → rules/colony.json 落到 @colony.*_rate
//   colony 系统每帧按 (产出 - 基础消耗) 调库存。
// 产出映射:conduit→电,plot(种植/水培)→食 + 氧。

const BASE_USE = 2.0; // 每秒基础消耗(电/氧/食各自)——殖民地活着就在烧
const PER = 3.0;      // 每个产出结构每秒产量
const POP_BONUS = 1.5; // 每个伙伴每秒帮的活(净正:留住人对生存有实利,心脏 C 的一半)

function clamp(v) {
  return v < 0 ? 0 : (v > 100 ? 100 : v);
}

// 统计在场伙伴数,发出去(规则落到 @colony.pop)。伙伴会帮着撑殖民地。
vitric.system("pop-tally", { query: ["Companion"], writes: [] }, (entities, ctx) => {
  ctx.emit("pop", { n: entities.length });
});

// 统计产出结构,把"每秒产量"发出去(规则把它落进 @colony 的速率字段)。
vitric.system("tally", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  let conduit = 0;
  let plot = 0;
  let quarters = 0;
  for (const e of entities) {
    if (e.Structure.kind === "conduit") conduit += 1;
    else if (e.Structure.kind === "plot") plot += 1;
    else if (e.Structure.kind === "quarters") quarters += 1;
  }
  // quarters 数也带上,给伙伴需求系统用(住所满足舒适)
  ctx.emit("tally", { pow: conduit * PER, food: plot * PER, o2: plot * PER, quarters: quarters });
});

// 每帧:库存 += (产出速率 - 基础消耗) * dt,夹在 [0,100]。
vitric.system("colony", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const c = e.Colony;
    const help = c.pop * POP_BONUS; // 伙伴帮的活,摊到每种资源
    c.power = clamp(c.power + (c.pow_rate + help - BASE_USE) * ctx.dt);
    c.oxygen = clamp(c.oxygen + (c.o2_rate + help - BASE_USE) * ctx.dt);
    c.food = clamp(c.food + (c.food_rate + help - BASE_USE) * ctx.dt);
    // 取整给 HUD 显示用(format 模板直接读这几个,免得屏上是 53.9999)
    c.o2_i = Math.round(c.oxygen);
    c.pow_i = Math.round(c.power);
    c.food_i = Math.round(c.food);
  }
});
