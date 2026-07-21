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
// Task 13 will expand POI tables and add biome-specific enemies; Task 12 lays the framework.
const REGION_CONTENT = {
  mountain: {
    tile_color: "#3a3530",
    nodes: [
      { kind: "ore", count: 6, color: "#caa45a", label: "矿脉", left: 5 },
    ],
    pois: [
      // Ancient ruins: TechPoint reward (already in Task 12).
      { kind: "ancient-ruins", reward_table: '{"techpoint":[1,3]}', label: "古代遗迹" },
      // Crystal cave: crystal_core reward + cave-injury risk (handler in poi.js).
      { kind: "crystal-cave", reward_table: '{"crystal_core":[1,2]}', label: "水晶洞" },
    ],
  },
  swamp: {
    tile_color: "#2a3a2a",
    nodes: [
      { kind: "fiber", count: 5, color: "#9aac5a", label: "纤维丛", left: 5 },
    ],
    pois: [
      // Dangerous flora: hide reward + combat trigger (handler spawns a weak enemy).
      { kind: "dangerous-flora", reward_table: '{"hide":[1,2]}', label: "危险植物" },
      // Oasis: seed + fiber reward (fertile ground).
      { kind: "oasis", reward_table: '{"seed":[2,4],"fiber":[1,3]}', label: "绿洲" },
    ],
  },
  desert: {
    tile_color: "#7a6a3a",
    nodes: [
      { kind: "crystal_core", count: 2, color: "#5acaff", label: "晶核", left: 3 },
    ],
    pois: [
      // Caravan stop: no direct reward, but handler emits trade-available (faction hook).
      { kind: "caravan-stop", reward_table: '{}', label: "商队驿站" },
      // Tomb: high-tier reward + curse risk (handler applies mood drop).
      { kind: "tomb", reward_table: '{"crystal_core":[1,2],"techpoint":[2,4]}', label: "古墓" },
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
  for (let gx = spec.anchor_x; gx < spec.anchor_x + spec.w; gx++) {
    for (let gy = spec.anchor_y; gy < spec.anchor_y + spec.h; gy++) {
      ctx.spawn({
        Cell: { kind: spec.biome },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: content.tile_color },
        Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                  anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                  dormant_ticks: 0, spawn_timer: 0 },
      });
    }
  }

  // Spawn resource nodes at deterministic positions within region bounds.
  let nodeIdx = 0;
  for (const nodeSpec of content.nodes) {
    for (let i = 0; i < nodeSpec.count; i++) {
      const nx = spec.anchor_x + stream.nextInt(0, spec.w - 1);
      const ny = spec.anchor_y + stream.nextInt(0, spec.h - 1);
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

  // Spawn POIs at deterministic positions.
  let poiIdx = 0;
  for (const poiSpec of content.pois) {
    const px = spec.anchor_x + stream.nextInt(0, spec.w - 1);
    const py = spec.anchor_y + stream.nextInt(0, spec.h - 1);
    ctx.spawn({
      Poi: { kind: poiSpec.kind, state: "fresh", cooldown: 0, reward_table: poiSpec.reward_table },
      Position: { x: px, y: py },
      Sprite: { w: 1, h: 1, image: "", color: "#e8d878" },
      Text: { content: poiSpec.label, size: 0.34, color: "#ffffff", screen: false },
      Region: { id: id, biome: spec.biome, state: "active", discovered: 1,
                anchor_x: spec.anchor_x, anchor_y: spec.anchor_y, w: spec.w, h: spec.h,
                dormant_ticks: 0, spawn_timer: 0 },
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
