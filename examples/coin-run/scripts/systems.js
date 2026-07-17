// coin-run script layer: logic that rules can't express lands here.

// Coins bob up and down — continuous sine motion is awkward to express in rules, a classic script system job.
// Note that writes only declares Position: this system can't touch other components, the engine enforces it.
vitric.system("coin-bob", { query: ["Coin", "Position"], writes: ["Position"] }, (entities, ctx) => {
  for (const e of entities) {
    e.Position.y = e.Coin.home_y + Math.sin(ctx.tick / 10) * 0.5;
  }
});

// Called by the win-check rule on victory: scatter deterministic-random confetti.
vitric.fn("celebrate", (args, ctx) => {
  for (let i = 0; i < 5; i++) {
    ctx.spawn({
      Position: { x: ctx.random() * 40, y: 10 },
      Velocity: { x: 0, y: -(1 + ctx.random() * 3) },
      Sprite: { w: 0.5, h: 0.5, color: "#ff6bd6" },
    });
  }
  ctx.emit("game-won", { score: args.score });
});
