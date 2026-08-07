// Region specs: 5 regions matching spec §4.6 layout.
//   home   (0,0)-(28,12)    28×12   active   starting
//   wild   (28,0)-(60,30)   32×30   active   starting (extends current wild)
//   mountain (0,12)-(30,40)  30×28  dormant  Tech: exploration_t1
//   swamp  (28,12)-(60,40)  32×28   dormant  Party has explorer-role companion
//   desert (60,0)-(120,60)  60×60   dormant  Faction caravan relation ≥ neutral AND Tech: industry_t3
//
// Region content is generated on thaw using ctx.random_stream("region:<id>") — deterministic
// regardless of thaw timing (same world_seed → same substream → same positions). This is the
// replay-safety guarantee: a region thawed at tick 100 vs tick 1000 produces bit-identical content.
//
// Camera world_bounds: union of all active region rects. Updated on every region-thaw event.
// Engine's integrate_motion clamps the player (Camera.follow target) to these bounds.

const REGION_SPECS = {
  home:     { anchor_x: 0,  anchor_y: 0,  w: 28, h: 12, biome: "home",     state: "active"  },
  wild:     { anchor_x: 28, anchor_y: 0,  w: 32, h: 30, biome: "wild",     state: "active"  },
  mountain: { anchor_x: 0,  anchor_y: 12, w: 30, h: 28, biome: "mountain", state: "dormant" },
  swamp:    { anchor_x: 28, anchor_y: 12, w: 32, h: 28, biome: "swamp",    state: "dormant" },
  desert:   { anchor_x: 60, anchor_y: 0,  w: 60, h: 60, biome: "desert",   state: "dormant" },
};

// Per-region content config: tile color, resource node types/counts, POI types/counts.
// Each node/POI carries optional minN/maxN — the noise band where it belongs, so resources
// match terrain (ore on rocky, fiber near water, etc.). The noise helpers are in world.js.
const REGION_CONTENT = {
  mountain: {
    tile_color: "#3a3530",
    nodes: [
      { kind: "ore", count: 6, color: "#caa45a", label: "矿脉", left: 5, minN: 0.30 },
      { kind: "crystal_core", count: 2, color: "#5acaff", label: "晶核", left: 3, minN: 0.55 },
    ],
    pois: [
      { kind: "ancient-ruins", reward_table: '{"techpoint":[1,3]}', label: "古代遗迹", minN: 0.30 },
      { kind: "crystal-cave", reward_table: '{"crystal_core":[1,2]}', label: "水晶洞", minN: 0.45 },
    ],
  },
  swamp: {
    tile_color: "#2a3a2a",
    nodes: [
      { kind: "fiber", count: 5, color: "#9aac5a", label: "纤维丛", left: 5, maxN: 0.00 },
      { kind: "wood", count: 3, color: "#5f8f3a", label: "林木", left: 5, minN: 0.00, maxN: 0.30 },
    ],
    pois: [
      { kind: "dangerous-flora", reward_table: '{"hide":[1,2]}', label: "危险植物", minN: 0.00, maxN: 0.30 },
      { kind: "oasis", reward_table: '{"seed":[2,4],"fiber":[1,3]}', label: "绿洲", maxN: 0.00 },
    ],
  },
  desert: {
    tile_color: "#7a6a3a",
    nodes: [
      { kind: "crystal_core", count: 2, color: "#5acaff", label: "晶核", left: 3, minN: 0.45 },
      { kind: "ore", count: 3, color: "#caa45a", label: "矿脉", left: 5, minN: 0.30, maxN: 0.55 },
    ],
    pois: [
      { kind: "caravan-stop", reward_table: '{}', label: "商队驿站", minN: -0.10, maxN: 0.30 },
      { kind: "tomb", reward_table: '{"crystal_core":[1,2],"techpoint":[2,4]}', label: "古墓", minN: 0.45 },
    ],
  },
};

// Generate region content on thaw. Called by rule on region-thaw event.
// Uses ctx.random_stream("region:<id>") for deterministic tile/node/POI placement.
// Args: { region_id }
vitric.fn("gen_region_content", (a, ctx) => {
  const id = a.region_id;
  const spec = REGION_SPECS[id];
  const content = REGION_CONTENT[id];
  if (!spec || !content) return;

  const stream = ctx.random_stream("region:" + id);

  // Spawn terrain tiles within the region bounds.
  // Apply Perlin noise variation around the biome's base color so terrain looks natural
  // (patches of darker/lighter ground) instead of a flat solid color. The noise helpers
  // (__noise2D, __terrainColor) are defined in world.js (loaded before this file).
  const terrainStream = ctx.random_stream("region:" + id + ":terrain");
  const ox = terrainStream.next() * 100;
  const oy = terrainStream.next() * 100;
  for (let gx = spec.anchor_x; gx < spec.anchor_x + spec.w; gx++) {
    for (let gy = spec.anchor_y; gy < spec.anchor_y + spec.h; gy++) {
      const n = __noise2D(gx * 0.15 + ox, gy * 0.15 + oy);
      // Blend: 70% biome base color + 30% noise-driven color for subtle variation.
      const noiseColor = __terrainColor(n);
      ctx.spawn({
        Cell: { kind: spec.biome },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: noiseColor },
        Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                  anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                  dormant_ticks: 0, spawn_timer: 0 },
        Fog: { state: "hidden", _orig_color: "" },
      });
    }
  }

  // Spawn resource nodes at deterministic positions, matched to terrain noise.
  // Each node spec carries optional minN/maxN — the noise band where it belongs.
  // Mountain: ore on rocky/highland; crystal_core only on highest peaks.
  // Swamp:    fiber near water/beach; wood on grassland patches.
  // Desert:   crystal_core on rocky; ore on highland.
  let nodeIdx = 0;
  for (const nodeSpec of content.nodes) {
    for (let i = 0; i < nodeSpec.count; i++) {
      let nx = 0, ny = 0, ok = false, attempts = 0;
      while (!ok && attempts < 30) {
        nx = spec.anchor_x + stream.nextInt(0, spec.w - 1);
        ny = spec.anchor_y + stream.nextInt(0, spec.h - 1);
        attempts++;
        const n = __noise2D(nx * 0.15 + ox, ny * 0.15 + oy);
        if (nodeSpec.minN !== undefined && n < nodeSpec.minN) continue;
        if (nodeSpec.maxN !== undefined && n >= nodeSpec.maxN) continue;
        ok = true;
      }
      ctx.spawn({
        Node: { kind: nodeSpec.kind, left: nodeSpec.left, max: nodeSpec.left, cooldown: 0 },
        Position: { x: nx, y: ny },
        Sprite: { w: 0.9, h: 0.9, image: "", color: nodeSpec.color },
        Text: { content: nodeSpec.label, size: 0.34, color: "#ffffff", screen: false },
        Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                  anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                  dormant_ticks: 0, spawn_timer: 0 },
      }, id + "_node_" + nodeIdx);
      nodeIdx++;
    }
  }

  // Spawn POIs at terrain-matched positions (ancient-ruins on high ground, oasis near water, etc.)
  let poiIdx = 0;
  for (const poiSpec of content.pois) {
    let px = 0, py = 0, ok = false, attempts = 0;
    while (!ok && attempts < 30) {
      px = spec.anchor_x + stream.nextInt(0, spec.w - 1);
      py = spec.anchor_y + stream.nextInt(0, spec.h - 1);
      attempts++;
      const n = __noise2D(px * 0.15 + ox, py * 0.15 + oy);
      if (poiSpec.minN !== undefined && n < poiSpec.minN) continue;
      if (poiSpec.maxN !== undefined && n >= poiSpec.maxN) continue;
      ok = true;
    }
    ctx.spawn({
      Poi: { kind: poiSpec.kind, state: "fresh", cooldown: 0, reward_table: poiSpec.reward_table, risk_tier: poiSpec.risk_tier || "safe", rune_solved: 0 },
      Position: { x: px, y: py },
      Sprite: { w: 1, h: 1, image: "", color: "#e8d878" },
      Text: { content: poiSpec.label, size: 0.34, color: "#ffffff", screen: false },
      Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                dormant_ticks: 0, spawn_timer: 0 },
      Fog: { state: "hidden", _orig_color: "" },
    }, id + "_poi_" + poiIdx);
    poiIdx++;
  }

  ctx.emit("toast-show", { text: "区域生成: " + id });
});

// Update Camera.world_bounds to the union of all active region rects.
// Called by rule on region-thaw event (after gen_region_content).
// Args: {}
vitric.fn("update_camera_bounds", (a, ctx) => {
  // Read all region markers (entities named home/wild/mountain/swamp/desert with Region component).
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const id of Object.keys(REGION_SPECS)) {
    // Read the region marker entity's Region.state field.
    const state = ctx.getField(id, "Region.state");
    if (state !== "active") continue;
    const spec = REGION_SPECS[id];
    minX = Math.min(minX, spec.anchor_x);
    minY = Math.min(minY, spec.anchor_y);
    maxX = Math.max(maxX, spec.anchor_x + spec.w);
    maxY = Math.max(maxY, spec.anchor_y + spec.h);
  }
  if (minX === Infinity) return; // No active regions — leave bounds unchanged
  const bounds = JSON.stringify([minX, minY, maxX, maxY]);
  ctx.setField("camera", "Camera.world_bounds", bounds);
});

// Region approach checker: every tick, check if player is near a dormant region boundary
// AND that region's unlock condition is met. If so, thaw the region (ctx.thaw_region).
// Runs as a system (not a rule) because the condition logic is too complex for rule filters
// (especially the swamp's "party has explorer-role companion" check).
//
// Deviation from brief: brief specifies `query: []` (no entity batch), but the prelude
// requires a non-empty query (an empty query would also iterate ALL non-dormant entities,
// wasteful). We use `query: ["Player"]` — matches only the player entity. The system body
// ignores the entities argument and reads everything via ctx.getField (the player entity
// is the only one in the batch, and we read its position via ctx.getField("player", ...)
// for clarity instead of using the batch).
vitric.system("region-approach-check", { query: ["Player"], writes: [] }, (entities, ctx) => {
  // Read player position via ctx.getField (we ignore the entities batch; reading by name
  // makes the intent explicit and survives query-list changes).
  const px = ctx.getField("player", "Position.x");
  const py = ctx.getField("player", "Position.y");
  if (typeof px !== "number" || typeof py !== "number") return;

  for (const id of Object.keys(REGION_SPECS)) {
    const spec = REGION_SPECS[id];
    if (spec.state !== "dormant") continue; // Only check dormant regions

    // Read the region marker's actual state (it may have been thawed already by a rule).
    const state = ctx.getField(id, "Region.state");
    if (state !== "dormant") continue;

    // Check if player is within 3 tiles of the region boundary.
    const nearX = px >= spec.anchor_x - 3 && px <= spec.anchor_x + spec.w + 3;
    const nearY = py >= spec.anchor_y - 3 && py <= spec.anchor_y + spec.h + 3;
    if (!nearX || !nearY) continue;

    // Check unlock condition.
    if (!checkUnlockCondition(id, ctx)) continue;

    // Unlock condition met + player nearby → thaw.
    ctx.thaw_region(id);
    ctx.emit("region-approach", { id: id });
  }
});

// Check unlock condition for a region.
//   mountain: exploration_t1 tech researched (checked via Colony.Research.has_exploration_t1)
//   swamp: party has explorer-role companion (checked via Colony.companion_handles + Persona.role)
//   desert: caravan relation ≥ neutral (Faction.tier_caravan in [neutral, friendly, allied])
//           AND industry_t3 tech researched
function checkUnlockCondition(id, ctx) {
  if (id === "mountain") {
    const has = ctx.getField("colony", "Research.has_exploration_t1");
    return has === 1;
  }
  if (id === "swamp") {
    // companion_handles is a list-of-text field on Colony; ctx.getField returns it as a
    // parsed JS array (NOT a JSON string — list fields are deserialized by __getFieldRaw).
    // See wish.js:21 for the same pattern (direct array use, no JSON.parse).
    const handles = ctx.getField("colony", "Colony.companion_handles");
    if (!Array.isArray(handles)) return false;
    for (const h of handles) {
      if (typeof h !== "string" || !h) continue;
      const role = ctx.getField(h, "Persona.role");
      if (role === "explorer") return true;
    }
    return false;
  }
  if (id === "desert") {
    const tier = ctx.getField("colony", "Faction.tier_caravan");
    if (tier !== "neutral" && tier !== "friendly" && tier !== "allied") return false;
    const has = ctx.getField("colony", "Research.has_industry_t3");
    return has === 1;
  }
  return false;
}

// ---- P1 exploration gear gates: damage player in hazardous regions without proper gear ----
// mountain without climbing_gear: -5 HP per 5s ("山路难行")
// swamp without swamp_boots: no damage but movement is halved (handled by reading this in move.js
//   — simpler: apply a small -2 HP per 8s "沼泽瘴气" instead, to avoid cross-system coupling)
// desert without heat_suit: -3 HP per 10s ("高温")
// Only applies when the region is active (thawed) and player is within its bounds.
// Uses an accumulator on Colony (_hazard_tick) to throttle damage to the intended cadence.
vitric.system("region-hazard", { query: ["Player", "Position", "Hp"], writes: ["Hp"] }, (entities, ctx) => {
  const px = ctx.getField("player", "Position.x");
  const py = ctx.getField("player", "Position.y");
  if (typeof px !== "number" || typeof py !== "number") return;
  const climbing = ctx.getField("player", "Inventory.climbing_gear") | 0;
  const boots    = ctx.getField("player", "Inventory.swamp_boots")   | 0;
  const suit     = ctx.getField("player", "Inventory.heat_suit")     | 0;
  let dmg = 0;
  // Mountain hazard
  const mtnState = ctx.getField("mountain", "Region.state");
  if (mtnState === "active" && climbing === 0) {
    if (px >= 0 && px <= 30 && py >= 12 && py <= 40) dmg += 5 * ctx.dt / 5;
  }
  // Swamp hazard
  const swampState = ctx.getField("swamp", "Region.state");
  if (swampState === "active" && boots === 0) {
    if (px >= 28 && px <= 60 && py >= 12 && py <= 40) dmg += 2 * ctx.dt / 8;
  }
  // Desert hazard
  const desertState = ctx.getField("desert", "Region.state");
  if (desertState === "active" && suit === 0) {
    if (px >= 60 && px <= 120 && py >= 0 && py <= 60) dmg += 3 * ctx.dt / 10;
  }
  if (dmg > 0) {
    for (const e of entities) {
      const cur = (typeof e.Hp.value === "number" && !isNaN(e.Hp.value)) ? e.Hp.value : 100;
      e.Hp.value = Math.max(0, cur - dmg);
    }
  }
});
