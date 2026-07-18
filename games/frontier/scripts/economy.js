// Economy accounting: building + crafting. Scripts only do accounting (enough materials? how much left after deduction? where to place it?).
// The actual write-back (modifying inventory numbers, spawning structure entities) is partly done here directly (spawn structure),
// inventory deduction goes through emit "inv-set"{absolute value} -> rules/economy.json writes back (spire's idempotent write-back:
// payload is always the absolute value after deduction, identical on repeat application/replay).
//
// Why inventory isn't modified directly here: fn can only spawn/despawn whole entities + write to components it queried,
// it can't reach @player.Inventory (that's another entity). So the rule passes the current inventory in as an argument,
// here we compute the new value after deduction, emit it back and let the rule set it.

// Build table (GDD + depth): kind -> materials + tier + color + label + visual size.
//   Size tiers (per _layout_spec.md section 3):
//   - common structures plot/wall/conduit/extractor/plot2 = 1.0~1.1 (slightly raised above the tile)
//   - quarters 1.1 (a bit taller than walls)
//   - beacon 1.8 / monument 2.0 (landmarks, the largest and most visible on the field)
const BUILD = {
  plot:      { cost: {},                              tier: 1, color: "#6b8f3a", label: "种植台", size: 1.0 },
  wall:      { cost: { wood: 1 },                     tier: 1, color: "#8a7a5c", label: "墙",     size: 1.0 },
  conduit:   { cost: { ore: 1 },                      tier: 1, color: "#d8a83a", label: "电导管", size: 1.0 },
  extractor: { cost: { ore: 1 },                      tier: 1, color: "#4aa6c8", label: "抽水机", size: 1.0 },
  quarters:  { cost: { plank: 2 },                    tier: 1, color: "#c08a4a", label: "住所",   size: 1.1 },
  beacon:    { cost: { ore: 2, plank: 2 },            tier: 1, color: "#f5b942", label: "信标",   size: 1.8 },
  plot2:     { cost: { plank: 3, chair: 1 },          tier: 2, color: "#a8e85a", label: "良田",   size: 1.05 },
  monument:  { cost: { ore: 4, plank: 4, lamp: 2, wheat: 4 }, tier: 3, color: "#ffe066", label: "丰碑", size: 2.0 },
};

// Crafting recipes (GDD): output -> materials.
const CRAFT = {
  plank: { cost: { wood: 2 },            out: "plank" },
  chair: { cost: { plank: 1, fiber: 1 }, out: "chair" },
  lamp:  { cost: { plank: 1, ore: 1 },   out: "lamp" },
};

// Full inventory field set (aligned with schema Inventory) — write-back always carries the full set of absolute values, the rule sets them one by one.
const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];

// Read the current inventory from the incoming args (each item defaults to 0).
function readInv(a) {
  const inv = {};
  for (const k of ITEMS) inv[k] = a[k] | 0;
  return inv;
}

// Are materials sufficient?
function canPay(inv, cost) {
  for (const k in cost) {
    if ((inv[k] | 0) < cost[k]) return false;
  }
  return true;
}

// Deduct materials (modifies the inv copy in place).
function pay(inv, cost) {
  for (const k in cost) inv[k] -= cost[k];
}

// Emit the inventory absolute values back (the rule writes @player.Inventory.*).
function emitInv(ctx, inv) {
  const d = {};
  for (const k of ITEMS) d[k] = inv[k];
  ctx.emit("inv-set", d);
}

// ---- Build: the rule calls this on "build mode + left-click on ground", passing in the clicked world coords + hit entity name + selected kind + current inventory ----
// Only builds when a surface tile is clicked (name like t_<x>_<y>) — clicks on UI / existing structures / empty space are all ignored,
// to prevent accidental builds when the world click injected alongside clicking the build menu button (window left-click fires both world mouse + ui-click).
// Materials sufficient: deduct materials + spawn the structure at the rounded-to-grid position (anonymous entity — no stable name, to avoid same-grid rebuild name collisions crashing;
// plot/crop positioning is handled by the rule matching rounded coords, not relying on the structure name) + emit built.
// Insufficient / didn't click a tile: do nothing (spec: insufficient -> no-op).
vitric.fn("build", (a, ctx) => {
  const def = BUILD[a.kind];
  if (!def) return; // Unknown kind (nothing selected) — ignore
  if (typeof a.entity !== "string" || !/^t_[0-9]+_[0-9]+$/.test(a.entity)) return; // Didn't click a surface tile
  const inv = readInv(a);
  if (!canPay(inv, def.cost)) { ctx.emit("build-fail", { kind: a.kind, label: def.label }); return; } // insufficient materials → notify
  const gx = Math.round(a.x);
  const gy = Math.round(a.y);
  pay(inv, def.cost);
  // ctx.spawn(component object, optional name) — the components are the flat first parameter (not {components:...}); here we spawn anonymously.
  const comps = {
    Structure: { kind: a.kind, tier: def.tier || 1 },
    Position: { x: gx, y: gy },
    Sprite: { w: def.size, h: def.size, color: def.color },
    Text: { content: def.label, size: 0.34, color: "#ffffff", screen: false }, // Name label on the structure
  };
  // A plot carries an empty Crop once built — later interaction clicks directly setField to plant the crop on this tile (in-place, no separate crop entity spawned).
  if (a.kind === "plot") comps.Crop = { kind: "", stage: 0, timer: 0 };
  ctx.spawn(comps);
  emitInv(ctx, inv);
  ctx.emit("built", { kind: a.kind, label: def.label, x: gx, y: gy });
});

// ---- Interaction click: the rule passes in the "hit entity (a.entity handle/name) + its components (a.comp) + current inventory" ----
// If the hit is a plot: empty tile + has seed -> plant (setField Crop.kind=wheat); ripe (stage>=3) -> harvest (clear + wheat+2).
// If the hit is a wild resource node (Node): gather ore/wood/fiber into inventory.
// Writes directly to "the clicked tile" via ctx.setField — no more command register / tile matching (engine already supports click with hit entity entity+comp).
vitric.fn("interact", (a, ctx) => {
  const comp = a.comp || {};
  const st = comp.Structure;
  const crop = comp.Crop;
  const node = comp.Node;
  // ---- Plot ----
  if (st && st.kind === "plot" && crop) {
    const inv = readInv(a);
    if (crop.kind === "" && (inv.seed | 0) > 0) {
      // Plant: deduct 1 seed + attach the wheat crop to this tile (in place)
      inv.seed -= 1;
      ctx.setField(a.entity, "Crop.kind", "wheat");
      ctx.setField(a.entity, "Crop.stage", 0);
      ctx.setField(a.entity, "Crop.timer", 0);
      emitInv(ctx, inv);
      ctx.emit("planted", { x: a.x, y: a.y });
    } else if (crop.kind === "" && (inv.seed | 0) <= 0) {
      ctx.emit("plant-fail", {});
    } else if (crop.kind === "wheat" && (crop.stage | 0) >= 3) {
      // Harvest: clear the crop + wheat +2
      inv.wheat += 2;
      ctx.setField(a.entity, "Crop.kind", "");
      ctx.setField(a.entity, "Crop.stage", 0);
      emitInv(ctx, inv);
      ctx.emit("harvested", { id: "wheat", n: 2 });
    }
    return;
  }
  // ---- Wild resource node gathering (scene pre-spawns 6 nodes, left>0 means harvestable) ----
  // On depletion (left hits 0), set cooldown so node_regrow system can regrow it after 90s.
  if (node && (node.left | 0) > 0) {
    const inv = readInv(a);
    const nodeKind = node.kind || "ore";
    const ITEM_MAP = { ore: "ore", wood: "wood", fiber: "fiber" };
    const itemId = ITEM_MAP[nodeKind] || "ore";
    inv[itemId] += 1;
    const newLeft = (node.left | 0) - 1;
    ctx.setField(a.entity, "Node.left", newLeft);
    if (newLeft <= 0) {
      ctx.setField(a.entity, "Node.cooldown", 90); // 1.5 min regrow timer
    }
    emitInv(ctx, inv);
    ctx.emit("gathered", { node: nodeKind, id: itemId, n: 1 });
    return;
  }
});

// ---- Interaction discoverability: the plot's overhead label changes with state, so the player can see at a glance "this is clickable, and what I can do now" ----
// (Previously it always showed "plot", so the player didn't know it was interactive. Now: empty -> plantable / growing -> growing / ripe -> harvestable)
vitric.system("plot-hint", { query: ["Crop", "Text"], writes: ["Text"] }, (entities, ctx) => {
  for (const e of entities) {
    const k = e.Crop.kind || "";
    const stage = e.Crop.stage | 0;
    const t = k === "" ? "▸可种植" : (stage >= 3 ? "✓可收获" : "…生长中");
    if (e.Text.content !== t) e.Text.content = t;
  }
});

// ---- Interaction discoverability: wild resource node labels append "gatherable" to hint they can be clicked to gather ----
vitric.system("node-hint", { query: ["Node", "Text"], writes: ["Text"] }, (entities, ctx) => {
  for (const e of entities) {
    const left = e.Node.left | 0;
    const base = e.Node.kind === "wood" ? "林木" : (e.Node.kind === "fiber" ? "纤维" : "矿脉");
    const t = left > 0 ? base + "·可采" : base + "·空";
    if (e.Text.content !== t) e.Text.content = t;
  }
});

// ---- Craft: the rule is called when the recipe button (craft-<id>) is clicked, passing the recipe id + current inventory ----
// Enough materials: deduct materials + product +1 -> emit inv-set (the product's +1 is already merged into the absolute value) + emit crafted. Not enough: no-op.
vitric.fn("craft", (a, ctx) => {
  const rec = CRAFT[a.id];
  if (!rec) return;
  const inv = readInv(a);
  if (!canPay(inv, rec.cost)) { ctx.emit("craft-fail", { id: a.id }); return; }
  pay(inv, rec.cost);
  inv[rec.out] += 1;
  emitInv(ctx, inv);
  ctx.emit("crafted", { id: rec.out, n: 1 });
});

// ---- Node regrowth: depleted nodes regrow to max after cooldown elapses ----
vitric.system("node_regrow", { query: ["Node"], writes: ["Node"] }, (entities, ctx) => {
  for (const e of entities) {
    const left = e.Node.left | 0;
    const cd = e.Node.cooldown || 0;
    if (left <= 0 && cd > 0) {
      const newCd = cd - ctx.dt;
      if (newCd <= 0) {
        e.Node.left = e.Node.max | 0;
        e.Node.cooldown = 0;
      } else {
        e.Node.cooldown = newCd;
      }
    }
  }
});

// ---- Structure upgrade: tier-1 -> tier-2, pay resources, change kind ----
// Called by rule on ui-activate{action:"upgrade-prompt"} — passes target entity handle + current inventory.
// Reads Structure.kind/tier via ctx.getField (deferred-write safe: reads happen before any writes).
// UPGRADES table: which tier-1 kinds can upgrade, to what, and the cost.
vitric.fn("upgrade_structure", (a, ctx) => {
  if (typeof a.entity !== "string" || !a.entity) return;
  const kind = ctx.getField(a.entity, "Structure.kind");
  const tier = ctx.getField(a.entity, "Structure.tier") | 0;
  if (!kind || tier >= 2) {
    ctx.emit("toast-show", { text: "已满级或无法升级" });
    return;
  }
  const UPGRADES = {
    plot:     { to: "greenhouse",  cost: { ore: 2, plank: 2 } },
    conduit:  { to: "solar-array", cost: { ore: 3, plank: 1 } },
    quarters: { to: "cabin",       cost: { plank: 4, lamp: 1 } },
  };
  const up = UPGRADES[kind];
  if (!up) {
    ctx.emit("toast-show", { text: "该结构无法升级" });
    return;
  }
  const inv = readInv(a);
  if (!canPay(inv, up.cost)) {
    ctx.emit("toast-show", { text: "资源不足" });
    return;
  }
  pay(inv, up.cost);
  emitInv(ctx, inv);
  ctx.setField(a.entity, "Structure.kind", up.to);
  ctx.setField(a.entity, "Structure.tier", 2);
  ctx.emit("upgrade-structure", { id: a.entity, kind: up.to });
  ctx.emit("toast-show", { text: "升级为" + up.to });
});
