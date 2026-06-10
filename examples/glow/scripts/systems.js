// glow 的脚本层:动画状态机 + 粒子迸发
// 动画选择:腾空=jump,跑动=walk,静止=idle——状态全由组件推导,无私藏状态
vitric.system("hero-anim", { query: ["Player","Body","Velocity","Anim"], writes: ["Anim"] }, (entities) => {
  for (const e of entities) {
    const moving = Math.abs(e.Velocity.x) > 0.1;
    const clip = !e.Body.grounded ? "jump" : (moving ? "walk" : "idle");
    if (e.Anim.clip !== clip) e.Anim.clip = clip;
  }
});

// 粒子迸发:尘土/宝石火花/通关彩带,寿命交给引擎 Particle 系统收尾
vitric.fn("burst", (args, ctx) => {
  const kinds = {
    dust:     { colors: ["#d8c8a0","#c4b48e"],            img: "dust.png", up: 2,  spread: 3, ttl: 22, s: 0.4 },
    spark:    { colors: ["#ffd75e","#ffb73e","#fff2b0"],  img: "",         up: 5,  spread: 5, ttl: 26, s: 0.3 },
    confetti: { colors: ["#ff6bd6","#7dff8a","#5ec8ff","#ffd75e"], img: "", up: 9, spread: 8, ttl: 48, s: 0.35 },
  };
  const k = kinds[args.kind] || kinds.dust;
  for (let i = 0; i < args.n; i++) {
    const c = k.colors[Math.floor(ctx.random() * k.colors.length)];
    ctx.spawn({
      Position: { x: args.x + (ctx.random()-0.5)*1.2, y: args.y + (ctx.random()-0.3)*0.8 },
      Velocity: { x: (ctx.random()-0.5)*2*k.spread, y: k.up*(0.4+ctx.random()*0.9) },
      Sprite:   { w: k.s, h: k.s, color: c, image: k.img },
      Particle: { ttl: k.ttl + Math.floor(ctx.random()*8) },
    });
  }
});

// 萤火虫氛围:每 24 tick 在英雄附近放一只慢慢上飘的暖光点,寿命交给 Particle
vitric.system("fireflies", { query: ["Player", "Position"], writes: [] }, (entities, ctx) => {
  if (ctx.tick % 24 !== 0) return;
  for (const e of entities) {
    ctx.spawn({
      Position: { x: e.Position.x + (ctx.random() - 0.3) * 36, y: ctx.random() * 9 + 0.5 },
      Velocity: { x: (ctx.random() - 0.5) * 0.8, y: 0.5 + ctx.random() * 0.8 },
      Sprite:   { w: 0.18, h: 0.18, color: ctx.random() < 0.5 ? "#ffd75e" : "#ffeda8" },
      Particle: { ttl: 120 + Math.floor(ctx.random() * 60) },
    });
  }
});
