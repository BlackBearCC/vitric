// 作物生长:种植台(plot)建出来就带一个 Crop 组件(空地 kind=""),互动点击种下后 kind="wheat"。
// 作物就长在种植台这块地上(同一实体的 Crop 组件),靠引擎的 ctx.setField 原地写。
//
// 节奏:3 段 ×各 4 秒(共 12 秒);夜里 timer 暂停 — 一茬一天(白天生长 = 12/60 = 20% 日长,
// 实际"几小时工作量")。stage 0/1/2 生长中,stage 3 熟(可收)。
// 每帧 timer += dt(夜里不动);满 4 秒进下一段、timer 清零;到 stage 3 停住等收。
//
// 时段由 ctx.tick 推(与 clock.js 的 DAY_SEC 同源,二者必须同步)。

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
    if (isNight) continue; // 夜里作物休眠:timer 不推,颜色保留
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
});
