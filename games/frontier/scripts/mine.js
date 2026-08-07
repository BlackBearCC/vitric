// Underground mine layer: a separate coordinate space (x200+, y200+) reached via the mine
// entrance near the home area. Each entry generates a fresh level with richer ore (left=8 vs
// surface left=5) and more enemies. Infinite depth — no ending (per product direction).
//
// Design:
// - NOT a scene switch — a teleport to a far coordinate. The player's position changes, the
//   camera follows; surface entities stay in memory (just off-screen).
// - Enemies use the EXISTING combat systems (enemy-ai chases via Colony.player_x/y,
//   enemy-attack-player applies on-contact DPS, player_attack lets the player fight back). So
//   mine-crawlers are plain Enemy entities — no new AI/damage code needed.
// - A MineExit entity lets the player return to the surface anytime (interact mode + collision).
// - mine-cleanup system despawns all x>=MINE_OFFSET_X entities once the player is back on the
//   surface, keeping memory bounded. Regeneration uses a per-depth substream → replay-safe.

const MINE_OFFSET_X = 200;
const MINE_OFFSET_Y = 200;
const MINE_W = 14;
const MINE_H = 10;
const MINE_FLOOR_COLOR = "#2a2a2e";
const MINE_WALL_COLOR = "#17171b";
const MINE_ORE_COLOR = "#caa45a";
const ORE_RICH_LEFT = 8;
const SURFACE_CAM_BOUNDS = "[0,0,60,30]";

// Enter mine: depth+1, generate level at x200+, teleport player, lock camera to mine bounds.
// Arg shape: {} (bound via rule's `call`).
vitric.fn("enter_mine", (a, ctx) => {
  const depth = (ctx.getField("colony", "Colony.mine_depth") | 0) + 1;
  ctx.setField("colony", "Colony.mine_depth", depth);
  ctx.setField("colony", "Colony.mine_layer", "underground");

  gen_mine_level(depth, ctx);

  // Teleport player to mine entrance (interior, not on the border wall).
  ctx.setField("player", "Position.x", MINE_OFFSET_X + 1);
  ctx.setField("player", "Position.y", MINE_OFFSET_Y + 1);

  // Lock camera to mine bounds.
  ctx.setField("camera", "Camera.world_bounds",
    JSON.stringify([MINE_OFFSET_X, MINE_OFFSET_Y, MINE_OFFSET_X + MINE_W, MINE_OFFSET_Y + MINE_H]));

  ctx.emit("toast-show", { text: "矿坑第 " + depth + " 层 — 小心矿中生物" });
});

// Level generator: floor + border walls (visual only), rich ore nodes, enemies, and the exit.
// Uses "mine:level_<depth>" substream so the same (seed, depth) reproduces the same layout.
function gen_mine_level(depth, ctx) {
  const stream = ctx.random_stream("mine:level_" + depth);

  // Border walls: visual only (matches surface rock tiles — no Solid component in this game).
  for (let gx = 0; gx < MINE_W; gx++) {
    for (let gy = 0; gy < MINE_H; gy++) {
      const isBorder = (gx === 0 || gy === 0 || gx === MINE_W - 1 || gy === MINE_H - 1);
      ctx.spawn({
        Cell: { kind: isBorder ? "mine-wall" : "mine-floor" },
        Position: { x: MINE_OFFSET_X + gx, y: MINE_OFFSET_Y + gy },
        Sprite: { w: 1, h: 1, image: "", color: isBorder ? MINE_WALL_COLOR : MINE_FLOOR_COLOR },
        Fog: { state: "hidden", _orig_color: "" },
      });
    }
  }

  // Rich ore nodes (6 + depth, capped at 12), placed on interior floor tiles.
  const oreCount = Math.min(12, 6 + depth);
  for (let i = 0; i < oreCount; i++) {
    const ox = MINE_OFFSET_X + 1 + stream.nextInt(0, MINE_W - 3);
    const oy = MINE_OFFSET_Y + 1 + stream.nextInt(0, MINE_H - 3);
    ctx.spawn({
      Node: { kind: "ore", left: ORE_RICH_LEFT, max: ORE_RICH_LEFT, cooldown: 0 },
      Position: { x: ox, y: oy },
      Sprite: { w: 0.9, h: 0.9, image: "", color: MINE_ORE_COLOR },
      Text: { content: "富矿脉", size: 0.3, color: "#ffe070", screen: false },
      Fog: { state: "hidden", _orig_color: "" },
    });
  }

  // Enemies (2 + depth, capped at 6): plain Enemy entities — existing combat systems drive them.
  const enemyCount = Math.min(6, 2 + depth);
  for (let i = 0; i < enemyCount; i++) {
    const ex = MINE_OFFSET_X + 2 + stream.nextInt(0, MINE_W - 5);
    const ey = MINE_OFFSET_Y + 2 + stream.nextInt(0, MINE_H - 5);
    const hp = 20 + depth * 5;
    ctx.spawn({
      Enemy: { kind: "mine-crawler", damage: 8, aggro_range: 9 },
      Hp: { value: hp, max: hp },
      Position: { x: ex, y: ey },
      Velocity: { x: 0, y: 0 },
      Collider: { w: 0.8, h: 0.8 },
      Sprite: { w: 0.8, h: 0.8, image: "", color: "#7a3a3a" },
      Text: { content: "", size: 0.3, color: "#ff6a6a", screen: false },
      Fog: { state: "hidden", _orig_color: "" },
    });
  }

  // Mine exit at the entrance position — player walks onto it + interact to return.
  ctx.spawn({
    MineExit: {},
    Position: { x: MINE_OFFSET_X, y: MINE_OFFSET_Y },
    Collider: { w: 1, h: 1 },
    Sprite: { w: 1, h: 1, image: "", color: "#4a4a6a" },
    Text: { content: "返回地面", size: 0.35, color: "#8a8aff", screen: false },
    Fog: { state: "visible", _orig_color: "" },
  });
}

// Exit mine: mark surface, teleport player back to the mine entrance, restore camera bounds.
// Mine entities are cleaned up next tick by the mine-cleanup system (they block the check below).
// Arg shape: {}.
vitric.fn("exit_mine", (a, ctx) => {
  ctx.setField("colony", "Colony.mine_layer", "surface");
  ctx.setField("player", "Position.x", 3);
  ctx.setField("player", "Position.y", 9);
  ctx.setField("camera", "Camera.world_bounds", SURFACE_CAM_BOUNDS);

  const depth = ctx.getField("colony", "Colony.mine_depth") | 0;
  ctx.emit("toast-show", { text: "返回地面 — 此前下到第 " + depth + " 层" });
});

// Cleanup: when the player is on the surface, despawn any entity in the mine coordinate space.
// Runs every tick but is a cheap x-guard; keeps memory bounded across infinite mine entries.
// A system (not a call fn) because exit_mine can't iterate the world — systems get a batch.
vitric.system("mine-cleanup", { query: ["Position"], writes: [] }, (entities, ctx) => {
  const layer = ctx.getField("colony", "Colony.mine_layer");
  if (layer !== "surface") return; // when underground, keep the mine alive

  for (const e of entities) {
    const x = e.Position.x;
    if (typeof x === "number" && x >= MINE_OFFSET_X) {
      ctx.despawn(e.id);
    }
  }
});