// cave-gen 的配方生成器：把 Recipe 数字变成一个关卡。
// 全程用 ctx.random()（引擎的种子随机流）——同种子永远生成同一张图，
// 改 vitric.json 的 seed 或场景里的 Recipe 数字，关卡就完全换一张。

vitric.fn("generate", (args, ctx) => {
  const halfW = args.width / 2;
  const halfH = args.height / 2;

  // 出生点周围留安全区，随机点落在里面就往外推
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
