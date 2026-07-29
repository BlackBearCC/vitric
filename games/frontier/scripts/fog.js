// Fog of war: cells/POIs/Nests/Relics are hidden until the player gets close.
// Three states per cell (tracked in Fog.state on each cell entity):
//   hidden  — beyond any sight range; tinted near-black, blocks interaction discovery
//   dim     — was seen but not currently in sight; tinted 50% darker
//   visible — currently within player sight radius; full color
//
// Entities that carry a Fog component (cells, POIs, nests, relics) get their Sprite.color
// scaled by the fog state each tick. Non-cell entities (POI/Nest/Relic) additionally get
// their Text.content blanked while hidden so the player can't read labels through fog.
//
// Sight radius: 5 tiles from player. Memory: once seen, stays dim (never fully hidden again).
// Performance: only writes when state changes (compares current state to new state).

const SIGHT_RADIUS = 5;
const SIGHT_R2 = SIGHT_RADIUS * SIGHT_RADIUS;
const DIM_RADIUS = SIGHT_RADIUS + 2;       // soft edge: 2-tile band beyond sight fades to dim
const DIM_R2 = DIM_RADIUS * DIM_RADIUS;

// Original colors are stored on the entity once (in _orig_color) so we can restore them.
// We stash it on first encounter (when Fog.state is still the default "hidden" and Sprite.color
// is the scene-defined color). This keeps the system idempotent across ticks.

function fogColor(original, state) {
  if (state === "visible") return original;
  if (state === "dim") return "#3a3a3a";     // 50% darker placeholder
  return "#111111";                            // hidden
}

// ---- fog-update: recompute Fog.state for every cell based on player position ----
// Queries all Cell entities with Position + Sprite + Fog. Writes Fog.state and Sprite.color.
vitric.system("fog-update", { query: ["Cell", "Position", "Sprite", "Fog"], writes: ["Fog", "Sprite"] }, (entities, ctx) => {
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  for (const e of entities) {
    // Stash original color on first encounter.
    let orig = e.Fog._orig_color;
    if (!orig) {
      orig = e.Sprite.color || "#888888";
      e.Fog._orig_color = orig;
    }
    const dx = e.Position.x - px, dy = e.Position.y - py;
    const d2 = dx * dx + dy * dy;
    let newState;
    if (d2 <= SIGHT_R2) newState = "visible";
    else if (d2 <= DIM_R2) newState = "dim";
    else if (e.Fog.state === "visible" || e.Fog.state === "dim") newState = "dim"; // memory
    else newState = "hidden";
    if (newState !== e.Fog.state) {
      e.Fog.state = newState;
      e.Sprite.color = fogColor(orig, newState);
    } else if (newState === "visible" && e.Sprite.color !== orig) {
      // Restore original color if we previously dimmed it.
      e.Sprite.color = orig;
    }
  }
});

// ---- fog-reveal-entities: hide POIs/Nests/Relics that are on hidden cells ----
// POIs/Nests/Relics don't move, so we only need to check their Fog.state and blank their
// Text + dim Sprite while hidden. Writes Text.content and Sprite.color.
// NOTE: entities without a Fog component are skipped (query requires Fog).
vitric.system("fog-reveal-entities", { query: ["Fog", "Position", "Sprite", "Text"], writes: ["Fog", "Sprite", "Text"] }, (entities, ctx) => {
  for (const e of entities) {
    // Skip cells — handled by fog-update.
    if (e.Cell) continue;
    let orig = e.Fog._orig_color;
    if (!orig) {
      orig = e.Sprite.color || "#e8d878";
      e.Fog._orig_color = orig;
    }
    const st = e.Fog.state;
    if (st === "hidden") {
      e.Sprite.color = "#111111";
      e.Text.content = "";
    } else if (st === "dim") {
      e.Sprite.color = "#3a3a3a";
      e.Text.content = "";
    } else {
      e.Sprite.color = orig;
      // Text.content is left alone — set by game logic (relic text, POI label, etc.)
    }
  }
});
