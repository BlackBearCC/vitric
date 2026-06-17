// 作物生长：作物是独立实体 crop_<gx>_<gy>（Crop+Position+Sprite），盖在 plot 上。
// 为什么不挂在 plot 上当组件：脚本只能 spawn/despawn 整实体、改自己 query 到的组件，
// 没法给已存在的 plot "加一个 Crop 组件"。所以种地=spawn 一个作物实体，收割=despawn 它。
//
// 生长（GDD 作物表）：3 段 ×各 6 秒（共 18 秒）。stage 0/1/2 = 生长中，stage 3 = 熟（可收）。
// 每帧 timer += dt；满 6 秒进下一段、timer 清零；到 stage 3 停住（熟了等收割）。
// 每段换精灵颜色（苗→长→壮→熟金黄），给玩家看得见的反馈。

const STAGE_SECONDS = 6.0; // 每段时长
const RIPE_STAGE = 3;      // 到这段就是熟了

// 各段配色：0 嫩芽，1 抽长，2 壮实，3 熟（金黄）。
const STAGE_COLOR = ["#7fbf5a", "#5fa83a", "#3f8f2a", "#e8c83a"];

vitric.system("crop-grow", { query: ["Crop", "Position", "Sprite"], writes: ["Crop", "Sprite"] }, (entities, ctx) => {
  for (const e of entities) {
    const c = e.Crop;
    if (c.stage < RIPE_STAGE) {
      c.timer += ctx.dt;
      if (c.timer >= STAGE_SECONDS) {
        c.timer = 0;
        c.stage += 1;
        // 刚熟这一刻发 crop-ready{plot 坐标}（GDD 事件表）。只在跨入熟那帧发一次。
        if (c.stage >= RIPE_STAGE) {
          ctx.emit("crop-ready", { x: e.Position.x, y: e.Position.y });
        }
      }
    }
    // 颜色跟随当前段（幂等：每帧 set 成该段颜色，重放/快照一致）。
    const color = STAGE_COLOR[c.stage] || STAGE_COLOR[RIPE_STAGE];
    if (e.Sprite.color !== color) e.Sprite.color = color;
  }
});
