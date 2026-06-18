// 时间/日夜循环:整个游戏的节奏骨架。
//   @colony 挂 Clock 组件 → day/time/tod 三个字段持续推进,每天 emit day-start。
//   dt 来自引擎,1 单位 = 1 秒游戏时间(60 tick/sec 下 tick 是 1/60 秒)。
//   1 个游戏日 = DAY_SEC 秒;本版 DAY_SEC = 60 — 实测约 1 分钟/天,
//   10–15 分钟一场对 5–7 天的纵切刚好够玩。
//
// 时段:
//   晨  0% – 25%   醒来、种田、采集、建房
//   午 25% – 50%   阳光最强、作物快速生长、旅人活跃
//   昏 50% – 75%   收工、回家、聚居氛围浓
//   夜 75% – 100%  伙伴须在住所休息;作物休眠
//
// 每个 tick 末 emit time-tick{day, time, tod} — 规则/脚本可监听(作物休眠就用这里)。

const CLOCK_DAY_SEC = 60.0;

vitric.system("clock-advance", { query: ["Clock"], writes: ["Clock"] }, (entities, ctx) => {
  for (const e of entities) {
    e.Clock.time += ctx.dt;
    let dayJustWrapped = false;
    while (e.Clock.time >= CLOCK_DAY_SEC) {
      e.Clock.time -= CLOCK_DAY_SEC;
      e.Clock.day += 1;
      dayJustWrapped = true;
    }
    // 时段标签
    const frac = e.Clock.time / CLOCK_DAY_SEC;
    let tod = "晨";
    if (frac >= 0.75) tod = "夜";
    else if (frac >= 0.50) tod = "昏";
    else if (frac >= 0.25) tod = "午";
    if (e.Clock.tod !== tod) e.Clock.tod = tod;
    // 每天 wrap 那一刻发一次 day-start(每个 Clock 实例一份)
    if (dayJustWrapped && e.Clock.last_day_emit !== e.Clock.day) {
      e.Clock.last_day_emit = e.Clock.day;
      ctx.emit("day-start", { day: e.Clock.day });
    }
    ctx.emit("time-tick", { day: e.Clock.day, tod: e.Clock.tod });
  }
});
