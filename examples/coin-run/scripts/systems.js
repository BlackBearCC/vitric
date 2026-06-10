// coin-run 的脚本层：规则写不动的逻辑落到这里。

// 金币上下漂浮——连续的正弦运动用规则表达很别扭，是脚本系统的典型活。
// 注意 writes 只声明 Position：这个系统碰不了别的组件，引擎盯着。
vitric.system("coin-bob", { query: ["Coin", "Position"], writes: ["Position"] }, (entities, ctx) => {
  for (const e of entities) {
    e.Position.y = e.Coin.home_y + Math.sin(ctx.tick / 10) * 0.5;
  }
});

// 通关时由 win-check 规则 call 进来：撒确定性随机的彩带。
vitric.fn("celebrate", (args, ctx) => {
  for (let i = 0; i < 5; i++) {
    ctx.spawn({
      Position: { x: ctx.random() * 40, y: 10 },
      Velocity: { x: 0, y: -(1 + ctx.random() * 3) },
    });
  }
  ctx.emit("game-won", { score: args.score });
});
