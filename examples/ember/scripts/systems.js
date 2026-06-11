// ember 的脚本层:动画状态机 + 火盆计数 + 粒子迸发
// 脚本无私藏状态——跨 tick 状态全在组件里,快照/回放安全。

// 动画选择:腾空=jump,移动=walk,静止=idle
vitric.system("hero-anim", { query: ["Player", "Body", "Velocity", "Anim"], writes: ["Anim"] }, (entities) => {
  for (const e of entities) {
    const moving = Math.abs(e.Velocity.x) > 0.1;
    const clip = !e.Body.grounded ? "jump" : (moving ? "walk" : "idle");
    if (e.Anim.clip !== clip) e.Anim.clip = clip;
  }
});

// 火盆计数:每 tick 数已点亮的火盆,发 lit-count{c} 给 HUD 规则(幂等,重复 set 文本无副作用)。
// 全 4 座点亮时发 all-lit;它每 tick 都会发,但 win 规则带 ["@door","exists"] 守卫,
// 门一消失规则就空转——无需脚本记忆上一次的计数,确定性安全。
vitric.system("brazier-counter", { query: ["Brazier"], writes: [] }, (entities, ctx) => {
  let lit = 0;
  for (const e of entities) {
    if (e.Brazier.lit) lit++;
  }
  ctx.emit("lit-count", { c: lit });
  if (lit >= 4) ctx.emit("all-lit", {});
});

// 粒子迸发:尘土/火花/通关彩带,寿命交给引擎 Particle 系统收尾
vitric.fn("burst", (args, ctx) => {
  const kinds = {
    dust:     { colors: ["#d8c8a0", "#c4b48e"],                     img: "dust.png", up: 2, spread: 3, ttl: 22, s: 0.4, light: 0 },
    spark:    { colors: ["#ffd75e", "#ffb73e", "#fff2b0"],          img: "",         up: 5, spread: 5, ttl: 28, s: 0.3, light: 1.4 },
    confetti: { colors: ["#ff9a3c", "#ffd75e", "#ffeaa0", "#ff6b3c"], img: "",       up: 9, spread: 8, ttl: 48, s: 0.35, light: 1.0 },
  };
  const k = kinds[args.kind] || kinds.dust;
  for (let i = 0; i < args.n; i++) {
    const c = k.colors[Math.floor(ctx.random() * k.colors.length)];
    const spec = {
      Position: { x: args.x + (ctx.random() - 0.5) * 1.2, y: args.y + (ctx.random() - 0.3) * 0.8 },
      Velocity: { x: (ctx.random() - 0.5) * 2 * k.spread, y: k.up * (0.4 + ctx.random() * 0.9) },
      Sprite:   { w: k.s, h: k.s, color: c, image: k.img },
      Particle: { ttl: k.ttl + Math.floor(ctx.random() * 8) },
    };
    if (k.light > 0) {
      spec.Light = { radius: 2.2, color: c, intensity: k.light };
    }
    ctx.spawn(spec);
  }
});
