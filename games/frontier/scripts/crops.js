// 作物生长：种植台(plot)建出来就带一个 Crop 组件(空地 kind="")，互动点击种下后 kind="wheat"。
// 作物就长在种植台这块地上(同一实体的 Crop 组件)，靠引擎新的 ctx.setField 原地写——
// 不再 spawn 单独的作物实体。
//
// 生长(GDD 作物表)：3 段 ×各 6 秒(共 18 秒)。stage 0/1/2 = 生长中，stage 3 = 熟(可收)。
// 每帧 timer += dt；满 6 秒进下一段、timer 清零；到 stage 3 停住等收。每段换地块颜色给反馈；
// 空地(kind="")回到种植台底色。

const STAGE_SECONDS = 6.0; // 每段时长
const RIPE_STAGE = 3;      // 到这段就是熟了
const PLOT_COLOR = "#6b8f3a"; // 空种植台底色(同 economy.js BUILD.plot 色)
// 各段配色：0 嫩芽，1 抽长，2 壮实，3 熟(金黄)。
const STAGE_COLOR = ["#7fbf5a", "#5fa83a", "#3f8f2a", "#e8c83a"];

vitric.system("crop-grow", { query: ["Crop", "Sprite"], writes: ["Crop", "Sprite"] }, (entities, ctx) => {
  for (const e of entities) {
    const c = e.Crop;
    if (c.kind === "") {
      // 空地：回底色，不长。
      if (e.Sprite.color !== PLOT_COLOR) e.Sprite.color = PLOT_COLOR;
      continue;
    }
    if (c.stage < RIPE_STAGE) {
      c.timer += ctx.dt;
      if (c.timer >= STAGE_SECONDS) {
        c.timer = 0;
        c.stage += 1;
        // 刚熟那一刻发一次 crop-ready(GDD 事件表)。
        if (c.stage >= RIPE_STAGE) ctx.emit("crop-ready", {});
      }
    }
    // 颜色跟随当前段(幂等：每帧 set 成该段颜色，重放/快照一致)。
    const color = STAGE_COLOR[c.stage] || STAGE_COLOR[RIPE_STAGE];
    if (e.Sprite.color !== color) e.Sprite.color = color;
  }
});
