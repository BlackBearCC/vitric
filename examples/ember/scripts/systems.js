// ember script layer: animation state machine + brazier counting + particle burst
// Scripts keep no hidden state — cross-tick state lives entirely in components, snapshot/replay safe.

// Animation choice: airborne=jump, moving=walk, still=idle
vitric.system("hero-anim", { query: ["Player", "Body", "Velocity", "Anim"], writes: ["Anim"] }, (entities) => {
  for (const e of entities) {
    const moving = Math.abs(e.Velocity.x) > 0.1;
    const clip = !e.Body.grounded ? "jump" : (moving ? "walk" : "idle");
    if (e.Anim.clip !== clip) e.Anim.clip = clip;
  }
});

// Brazier count: every tick, count lit braziers and emit lit-count{c} to the HUD rule (idempotent; repeatedly setting the text has no side effects).
// When all 4 are lit, emit all-lit; it fires every tick, but the win rule is guarded by ["@door","exists"],
// so once the door is gone the rule no-ops — no need for the script to remember the last count; deterministic and safe.
vitric.system("brazier-counter", { query: ["Brazier"], writes: [] }, (entities, ctx) => {
  let lit = 0;
  for (const e of entities) {
    if (e.Brazier.lit) lit++;
  }
  ctx.emit("lit-count", { c: lit });
  if (lit >= 4) ctx.emit("all-lit", {});
});

// Particle burst: dust / sparks / victory confetti; lifetime reaped by the engine Particle system
vitric.fn("burst", (args, ctx) => {
  const kinds = {
    dust:     { colors: ["#d8c8a0", "#c4b48e"],                     img: "",         up: 2, spread: 3, ttl: 22, s: 0.4, light: 0 },
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
