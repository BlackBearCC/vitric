// cave-gen recipe generator: turns Recipe numbers into a level.
// Uses ctx.random() throughout (the engine's seeded random stream) — same seed always generates the same map;
// change the seed in vitric.json or the Recipe numbers in the scene and the level is entirely regenerated.

vitric.fn("generate", (args, ctx) => {
  const halfW = args.width / 2;
  const halfH = args.height / 2;

  // Keep a safe zone around the spawn; if a random point falls inside, push it outward
  function place() {
    let x = (ctx.random() * 2 - 1) * (halfW - 1);
    let y = (ctx.random() * 2 - 1) * (halfH - 1);
    if (Math.abs(x) < 4 && Math.abs(y) < 4) {
      x += x >= 0 ? 5 : -5;
      y += y >= 0 ? 5 : -5;
    }
    return { x, y };
  }

  for (let i = 0; i < args.gems; i++) {
    const p = place();
    ctx.spawn({
      Gem: {},
      Position: { x: p.x, y: p.y },
      Collider: { w: 1, h: 1 },
      Sprite: { w: 0.8, h: 0.8, color: "#39e6c3" },
    });
  }
  for (let i = 0; i < args.hazards; i++) {
    const p = place();
    ctx.spawn({
      Hazard: {},
      Position: { x: p.x, y: p.y },
      Collider: { w: 1.2, h: 1.2 },
      Sprite: { w: 1.2, h: 1.2, color: "#ff5470" },
    });
  }
  ctx.emit("level-generated", { gems: args.gems, hazards: args.hazards });
});
