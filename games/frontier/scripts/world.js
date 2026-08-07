// Wild area: to the right of the home area (x0..15) is a stretch of wild terrain (x16..27)
// plus expanded wild (x28..59). Resource nodes, POIs, and relics are scattered at
// seed-determined positions via ctx.random_stream — same seed = same layout (replay-safe),
// different seed = different map.
//
// Gathering logic lives in the interact fn in economy.js. Here we only:
// ① lay wild terrain + place resource nodes; ② scatter POIs; ③ scatter relics;
// ④ apply cooldown to resource nodes after gathering.

vitric.fn("genWild", (a, ctx) => {
  // Wild terrain: x16..59, y0..29 (home transition x16..27 + expanded wild x28..59).
  for (let gx = 16; gx <= 59; gx++) {
    for (let gy = 0; gy <= 29; gy++) {
      ctx.spawn({
        Cell: { kind: "wild" },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: gx === 16 ? "#5a5040" : "#48402f" },
        Fog: { state: "hidden", _orig_color: "" },
      });
    }
  }

  // Resource nodes: seed-driven positions within wild bounds.
  // 10 nodes total: 4 ore, 3 wood, 3 fiber — same counts as before, different positions.
  const nodeStream = ctx.random_stream("wild:nodes");
  const NODE_SPECS = [
    { kind: "ore",   count: 4, color: "#caa45a", label: "矿脉" },
    { kind: "wood",  count: 3, color: "#5f8f3a", label: "林木" },
    { kind: "fiber", count: 3, color: "#9aac5a", label: "纤维丛" },
  ];
  const placed = []; // track positions for min-distance check
  for (const spec of NODE_SPECS) {
    for (let i = 0; i < spec.count; i++) {
      let nx, ny, attempts = 0;
      do {
        nx = 17 + nodeStream.nextInt(0, 42); // x17..59
        ny = nodeStream.nextInt(0, 29);
        attempts++;
      } while (attempts < 20 && placed.some(p => (p.x-nx)**2 + (p.y-ny)**2 < 4));
      placed.push({ x: nx, y: ny });
      ctx.spawn({
        Node: { kind: spec.kind, left: 5, max: 5, cooldown: 0 },
        Position: { x: nx, y: ny },
        Sprite: { w: 0.9, h: 0.9, image: "", color: spec.color },
        Text: { content: spec.label, size: 0.34, color: "#ffffff", screen: false },
        Fog: { state: "hidden", _orig_color: "" },
      });
    }
  }

  // POIs: seed-driven positions, same 3 types as before.
  const poiStream = ctx.random_stream("wild:pois");
  const POI_SPECS = [
    { kind: "abandoned-camp", label: "废弃营地", color: "#8b6f47",
      reward_table: '{"ore":[1,2],"wheat":[2,4],"fiber":[1,3]}', risk_tier: "safe" },
    { kind: "cave-entrance", label: "洞穴入口", color: "#5a4a6a",
      reward_table: '{"ore":[3,5]}', risk_tier: "safe" },
    { kind: "shipwreck", label: "沉船", color: "#4a5a6a",
      reward_table: '{"wheat":[3,5],"plank":[1,2]}', risk_tier: "safe" },
  ];
  for (let i = 0; i < POI_SPECS.length; i++) {
    let px, py, attempts = 0;
    do {
      px = 18 + poiStream.nextInt(0, 40);
      py = poiStream.nextInt(0, 29);
      attempts++;
    } while (attempts < 20 && placed.some(p => (p.x-px)**2 + (p.y-py)**2 < 9));
    placed.push({ x: px, y: py });
    const spec = POI_SPECS[i];
    ctx.spawn({
      Poi: { kind: spec.kind, state: "fresh", cooldown: 0, reward_table: spec.reward_table, risk_tier: spec.risk_tier, rune_solved: 0 },
      Position: { x: px, y: py },
      Sprite: { w: 1.6, h: 1.6, image: "", color: spec.color },
      Collider: { w: 1.6, h: 1.6 },
      Text: { content: spec.label, size: 0.4, color: "#ffe070", screen: false },
      Fog: { state: "hidden", _orig_color: "" },
    });
  }

  // Relics: seed-driven positions, 5 environmental narrative fragments.
  const relicStream = ctx.random_stream("wild:relics");
  const RELIC_TEXTS = [
    "墙上刻着:'别往南走。'",
    "锈蚀的牌子:'星火定居点 — 人口 47。'",
    "日记残页:'...第七次耀斑,食物不够了...'",
    "石碑:'献给那些留下的人。'",
    "废弃的广播塔。屏幕上还亮着:'撤离已启动。'",
  ];
  for (let i = 0; i < RELIC_TEXTS.length; i++) {
    let rx, ry, attempts = 0;
    do {
      rx = 20 + relicStream.nextInt(0, 38);
      ry = relicStream.nextInt(0, 29);
      attempts++;
    } while (attempts < 20 && placed.some(p => (p.x-rx)**2 + (p.y-ry)**2 < 9));
    placed.push({ x: rx, y: ry });
    ctx.spawn({
      Relic: { text: RELIC_TEXTS[i] },
      Position: { x: rx, y: ry },
      Sprite: { w: 0.8, h: 0.8, image: "", color: "#444444" },
      Text: { content: "", size: 0.32, color: "#aaaaaa", screen: false },
      Fog: { state: "hidden", _orig_color: "" },
    });
  }
});

// Resource node: short cooldown after each gather (prevents rapid-fire clicks). Decrease cooldown every frame.
vitric.system("node", { query: ["Node"], writes: ["Node"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Node.cooldown > 0) e.Node.cooldown = Math.max(0, e.Node.cooldown - ctx.dt);
  }
});
