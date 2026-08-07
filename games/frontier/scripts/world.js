// Wild area: to the right of the home area (x0..15) is a stretch of wild terrain (x16..27)
// plus expanded wild (x28..59). Resource nodes, POIs, and relics are scattered at
// seed-determined positions via ctx.random_stream — same seed = same layout (replay-safe),
// different seed = different map.
//
// Terrain colors are derived from a deterministic Perlin noise (hash-based gradients,
// no RNG stream calls — replay-safe) so adjacent tiles blend into natural biome bands
// (water → beach → grassland → rocky → highland) instead of a flat solid color.
//
// Gathering logic lives in the interact fn in economy.js. Here we only:
// ① lay wild terrain + place resource nodes; ② scatter POIs; ③ scatter relics;
// ④ apply cooldown to resource nodes after gathering.

// ---- Deterministic Perlin noise (2D). Pure hash of integer lattice → gradient vectors,
// classic Ken Perlin fade+lerp interpolation. No ctx.random_stream / Math.random used —
// the same (x,y) always yields the same value, so terrain is reproducible per seed and
// replay-safe. region.js (loaded after this file) reuses these globals.
globalThis.__noiseHash2 = function (ix, iy) {
  let h = (ix * 374761393 + iy * 668265263) | 0;
  h = ((h ^ (h >> 13)) * 1274126177) | 0;
  return (h ^ (h >> 16)) >>> 0;
};
globalThis.__noiseGrad = function (ix, iy) {
  const h = __noiseHash2(ix, iy);
  return [ (h & 0xFF) / 127.5 - 1, ((h >> 8) & 0xFF) / 127.5 - 1 ];
};
globalThis.__perlin2 = function (x, y) {
  const xi = Math.floor(x), yi = Math.floor(y);
  const xf = x - xi, yf = y - yi;
  const u = xf * xf * xf * (xf * (xf * 6 - 15) + 10);
  const v = yf * yf * yf * (yf * (yf * 6 - 15) + 10);
  const g00 = __noiseGrad(xi, yi),     g10 = __noiseGrad(xi + 1, yi);
  const g01 = __noiseGrad(xi, yi + 1), g11 = __noiseGrad(xi + 1, yi + 1);
  const d00 = g00[0] * xf       + g00[1] * yf;
  const d10 = g10[0] * (xf - 1) + g10[1] * yf;
  const d01 = g01[0] * xf       + g01[1] * (yf - 1);
  const d11 = g11[0] * (xf - 1) + g11[1] * (yf - 1);
  const nx = d00 + (d10 - d00) * u;
  const ny = d01 + (d11 - d01) * u;
  return nx + (ny - nx) * v; // ≈ [-1, 1]
};
// Multi-octave: base shape + detail.
globalThis.__noise2D = function (x, y) {
  return 0.6 * __perlin2(x, y) + 0.4 * __perlin2(x * 2.1 + 31.4, y * 2.1 + 17.7);
};
// Terrain palette: noise band → color. Lower = water/sand, mid = grass, high = rock.
globalThis.__terrainColor = function (n) {
  if (n < -0.28) return "#2c3a52"; // water
  if (n < -0.10) return "#7a7a5a"; // beach/sand
  if (n <  0.30) return "#4a5a32"; // grassland
  if (n <  0.55) return "#5a4a38"; // rocky dirt
  return "#3a3a3a";                // highland
};

vitric.fn("genWild", (a, ctx) => {
  // Wild terrain: x16..59, y0..29 — noise-driven colors for natural biome bands.
  // The noise offset is seeded once from ctx.random_stream so different world seeds
  // produce different terrain shapes (same seed = same terrain, replay-safe).
  const terrainStream = ctx.random_stream("wild:terrain");
  const ox = terrainStream.next() * 100;
  const oy = terrainStream.next() * 100;
  for (let gx = 16; gx <= 59; gx++) {
    for (let gy = 0; gy <= 29; gy++) {
      const n = __noise2D(gx * 0.15 + ox, gy * 0.15 + oy);
      ctx.spawn({
        Cell: { kind: "wild" },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: __terrainColor(n) },
        Fog: { state: "hidden", _orig_color: "" },
      });
    }
  }

  // Resource nodes: seed-driven positions, but placed on biome-appropriate terrain.
  // The noise at each candidate position determines which resource type fits:
  //   rocky/highland (n > 0.30) → ore
  //   grassland    (0.00..0.30) → wood
  //   beach/water  (n < 0.00)   → fiber
  // This makes the world feel coherent: ore in the hills, trees in the grass, fiber near water.
  const nodeStream = ctx.random_stream("wild:nodes");
  const placed = [];
  const NODE_TARGETS = [
    { kind: "ore",   count: 4, color: "#caa45a", label: "矿脉",   minN: 0.30 },
    { kind: "wood",  count: 3, color: "#5f8f3a", label: "林木",   minN: 0.00, maxN: 0.30 },
    { kind: "fiber", count: 3, color: "#9aac5a", label: "纤维丛", maxN: 0.00 },
  ];
  for (const spec of NODE_TARGETS) {
    for (let i = 0; i < spec.count; i++) {
      let nx = 0, ny = 0, ok = false, attempts = 0;
      while (!ok && attempts < 30) {
        nx = 17 + nodeStream.nextInt(0, 42);
        ny = nodeStream.nextInt(0, 29);
        attempts++;
        // Min-distance check
        if (placed.some(p => (p.x-nx)**2 + (p.y-ny)**2 < 4)) continue;
        // Terrain match: check noise at this position
        const n = __noise2D(nx * 0.15 + ox, ny * 0.15 + oy);
        if (spec.minN !== undefined && n < spec.minN) continue;
        if (spec.maxN !== undefined && n >= spec.maxN) continue;
        ok = true;
      }
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
