// Wild area (increment 2): to the right of the home area (x0..15) is a stretch of wild terrain (x16..27) scattered with resource nodes (ore veins / woods / fiber clumps).
// All spawned once in the start event (deterministic, fixed positions) — no hand-writing hundreds of lines of scene.
// Gathering logic lives in the interact fn in economy.js (interaction mode clicks a resource node → gather, using the engine's ctx.setField to write directly to that node).
// Here we only: ① lay wild terrain + place resource nodes; ② apply a short cooldown to resource nodes after gathering (prevents spam-clicking).

vitric.fn("genWild", (a, ctx) => {
  // Wild terrain: x16..27, y0..11, dark rocky earth (x16 boundary slightly brighter, hinting the transition from home to wild).
  for (let gx = 16; gx <= 27; gx++) {
    for (let gy = 0; gy <= 11; gy++) {
      ctx.spawn({
        Cell: { kind: "wild" },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: gx === 16 ? "#5a5040" : "#48402f" },
      });
    }
  }
  // Resource nodes: kind maps directly to inventory field (ore/wood/fiber); left = gatherable count. Tagged with names.
  const NODES = [
    ["ore", 19, 3, "矿脉", "#caa45a"], ["ore", 25, 9, "矿脉", "#caa45a"],
    ["wood", 22, 2, "林木", "#5f8f3a"], ["wood", 24, 10, "林木", "#5f8f3a"],
    ["fiber", 20, 7, "纤维丛", "#9aac5a"], ["fiber", 26, 5, "纤维丛", "#9aac5a"],
  ];
  for (const n of NODES) {
    ctx.spawn({
      Node: { kind: n[0], left: 5, max: 5, cooldown: 0 },
      Position: { x: n[1], y: n[2] },
      // Resource node ≈ 0.9: a bit smaller than a tile, like "scatter"; the name tag tells the player what it is
      Sprite: { w: 0.9, h: 0.9, image: "", color: n[4] },
      Text: { content: n[3], size: 0.34, color: "#ffffff", screen: false },
    });
  }
});

// Resource node: short cooldown after each gather (prevents rapid-fire clicks). Decrease cooldown every frame.
// Depleted (left=0) nodes stay (no auto-regen; 6 nodes × 5 gathers = 30 total, enough for this increment's crafting needs).
vitric.system("node", { query: ["Node"], writes: ["Node"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Node.cooldown > 0) e.Node.cooldown = Math.max(0, e.Node.cooldown - ctx.dt);
  }
});
