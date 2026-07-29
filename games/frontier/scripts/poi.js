// POI (Point of Interest) system: 3 daily-refreshing wild locations.
//   Poi component (added in Task 1): kind / state (fresh|looted|depleted) / cooldown / reward_table (JSON text).
//
// Two pieces:
//   1) poi_tick system — decrements cooldown on looted/depleted POIs; when cooldown hits 0, refreshes to fresh.
//   2) interact_poi fn — called by rules/poi.json when player in interact mode clicks a POI. Rolls rewards
//      via ctx.random() (NOT Math.random — poisoned in QuickJS), updates inventory through the inv-set
//      emit pattern (same as economy.js), marks the POI looted, and emits entered-poi for the wish system.

const POI_ITEMS = ["ore", "wheat", "fiber", "plank"];
const POI_LABELS = { ore: "矿", wheat: "麦", fiber: "纤维", plank: "板" };
const POI_COOLDOWN_LOOTED = 120;   // 2 min soft cooldown (full refresh also via tick)
const POI_CAVE_INJURY_CHANCE = 0.3; // cave-entrance: 30% chance of mood-drop injury

// ---- Tick: regrow looted/depleted POIs once their cooldown expires ----
vitric.system("poi_tick", { query: ["Poi"], writes: ["Poi"] }, (entities, ctx) => {
  for (const e of entities) {
    const poi = e.Poi;
    if (poi.state === "fresh") continue;
    if ((poi.cooldown | 0) <= 0) {
      // Already ready to refresh.
      poi.state = "fresh";
      poi.cooldown = 0;
      continue;
    }
    poi.cooldown = Math.max(0, poi.cooldown - ctx.dt);
    if (poi.cooldown <= 0) {
      poi.state = "fresh";
      poi.cooldown = 0;
    }
  }
});

// ---- Per-type POI handlers: special effects beyond the standard reward_table roll ----
// Each handler receives (a, ctx, poi, rewardText) and can emit additional events.
// The standard reward roll (from reward_table) happens BEFORE the handler — handlers
// only add extra effects (events, mood changes, combat triggers, etc.).
const POI_HANDLERS = {
  "ancient-ruins": (a, ctx, poi, rewardText) => {
    // Bonus TechPoint for discovering ancient ruins (on top of the standard +2 per POI).
    const tp = (a.techpoint | 0) + 3;
    ctx.emit("tp-set", { value: tp });
    ctx.emit("toast-show", { text: "古代遗迹: 额外+3科技点" });
  },

  "crystal-cave": (a, ctx, poi, rewardText) => {
    // Crystal cave: 30% chance of cave-injury (companion mood drop).
    // Moved from the legacy "cave-entrance" kind — crystal caves are the new cave POI.
    if (ctx.random() < 0.3) {
      ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
      ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
    }
  },

  "dangerous-flora": (a, ctx, poi, rewardText) => {
    // Dangerous flora: 50% chance of spawning a weak enemy (combat trigger).
    // The enemy spawns at the POI's position — the player must deal with it.
    if (ctx.random() < 0.5) {
      const x = a.comp.Position.x;
      const y = a.comp.Position.y;
      ctx.spawn({
        Enemy: { kind: "gnawer", damage: 5, aggro_range: 6, home_region: "swamp", _attack_cd: 0, _hit_flash: 0, _charge_t: 0, _charge_cd: 0, _flank_dir: 0, _nest_id: "" },
        Position: { x: x + 1, y: y },
        Velocity: { x: 0, y: 0 },
        Collider: { w: 0.8, h: 0.8 },
        Sprite: { w: 0.8, h: 0.8, image: "enemy.png", color: "#7a9a3a" },
        Hp: { value: 15, max: 15 },
      });
      ctx.emit("toast-show", { text: "危险植物释放了孢子!出现了敌对生物" });
    }
  },

  "oasis": (a, ctx, poi, rewardText) => {
    // Oasis: full party mood restoration (fertile ground, safe haven).
    ctx.emit("companion-mood-boost", { amount: 5, reason: "oasis" });
    ctx.emit("toast-show", { text: "绿洲清泉:全员心情+5" });
  },

  "caravan-stop": (a, ctx, poi, rewardText) => {
    // Caravan stop: emit trade-available event (faction trade hook).
    // The caravan faction's relation +1 hook (from Task 11's trader-companion-relation rule)
    // also fires on trade-available — so discovering a caravan-stop improves caravan relation.
    ctx.emit("trade-available", { pid: "caravan-stop", role: "trader" });
    ctx.emit("toast-show", { text: "商队驿站:贸易关系+1" });
  },

  "tomb": (a, ctx, poi, rewardText) => {
    // Tomb: 40% chance of curse (mood drop) — the high-tier reward comes with risk.
    if (ctx.random() < 0.4) {
      ctx.emit("companion-mood-drop", { amount: 15, reason: "tomb-curse" });
      ctx.emit("toast-show", { text: "古墓诅咒!全员心情-15" });
    }
  },
};

// ---- Interact click on a POI: rule passes hit entity handle + components snapshot + current inventory ----
// Same shape as economy.js `interact`: a.entity (handle), a.comp (components), a.<inventory fields>.
// Only acts if the hit entity has a Poi component in state "fresh". Rolls rewards, emits inv-set, marks looted.
vitric.fn("interact_poi", (a, ctx) => {
  const comp = a.comp || {};
  const poi = comp.Poi;
  if (!poi) return;                       // Not a POI hit — ignore.
  if (poi.state !== "fresh") return;      // Already looted/depleted — ignore.

  // P2 risk tier enforcement:
  //   safe  → loot immediately
  //   danger → loot immediately BUT handler may spawn enemies (existing behavior)
  //   ruin  → requires rune_solved=1 before looting; otherwise show puzzle hint
  const tier = poi.risk_tier || "safe";
  if (tier === "ruin" && !(poi.rune_solved | 0)) {
    ctx.emit("toast-show", { text: "古代封印:需要按正确顺序激活符文" });
    return;
  }

  // Parse reward table: {item: [lo, hi]}.
  let rewards = {};
  try { rewards = JSON.parse(poi.reward_table || "{}"); } catch { return; }

  // Build inventory from args (same pattern as economy.js readInv).
  // `hide` + `crystal_core` round-trip through inv-set alongside the rest of the inventory.
  const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core", "climbing_gear", "swamp_boots", "heat_suit"];
  const inv = {};
  for (const k of ITEMS) inv[k] = a[k] | 0;

  // Roll rewards deterministically with ctx.random().
  let rewardText = "";
  for (const key of Object.keys(rewards)) {
    const range = rewards[key];
    if (!Array.isArray(range) || range.length < 2) continue;
    const lo = range[0] | 0;
    const hi = range[1] | 0;
    const span = Math.max(0, hi - lo);
    const n = lo + Math.floor(ctx.random() * (span + 1));
    if (n <= 0) continue;
    inv[key] = (inv[key] | 0) + n;
    const label = POI_LABELS[key] || key;
    rewardText += `${label}+${n} `;
  }

  // Emit inventory write-back (rule "inv-apply" in economy.json handles it).
  const d = {};
  for (const k of ITEMS) d[k] = inv[k];
  ctx.emit("inv-set", d);

  // Mark POI looted + start cooldown (writes to the clicked entity's Poi component).
  ctx.setField(a.entity, "Poi.state", "looted");
  ctx.setField(a.entity, "Poi.cooldown", POI_COOLDOWN_LOOTED);

  // Award TechPoints for POI exploration (+2 per fresh POI).
  // The rule passes the current TechPoint.value in as `techpoint`; we emit the new absolute
  // value back via tp-set, the tp-apply rule in research.json writes it to @player.TechPoint.value.
  const tp = (a.techpoint | 0) + 2;
  ctx.emit("tp-set", { value: tp });

  // Toast with reward summary.
  ctx.emit("toast-show", { text: `探索收获: ${rewardText.trim()}` });

  // Per-type handler: special effects beyond the standard reward roll.
  // Handler runs AFTER rewards are applied (inventory + techpoint already emitted).
  const handler = POI_HANDLERS[poi.kind];
  if (handler) handler(a, ctx, poi, rewardText);

  // Keep the legacy cave-entrance special-case for wild-area POIs (backward compat).
  if (poi.kind === "cave-entrance" && ctx.random() < POI_CAVE_INJURY_CHANCE) {
    ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
    ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
  }

  // Notify wish system.
  ctx.emit("entered-poi", { kind: poi.kind });
});

// ---- P2 rune puzzle: auto-activate runes when player approaches in interact mode ----
// Each Rune has a `sequence` field (1, 2, 3...) indicating the correct activation order.
// When player is within 1.5 tiles of a rune in interact mode, the rune auto-activates.
// Progress is tracked via Colony._rune_progress. When all runes are activated in order,
// the nearest ruin POI is unlocked. Wrong order (walking to a higher-sequence rune first)
// resets all runes + spawns a curse enemy.
//
// rune-auto-activate system: runs every tick, checks player proximity to each rune.
vitric.system("rune-auto-activate", { query: ["Rune", "Position"], writes: ["Rune"] }, (entities, ctx) => {
  const mode = ctx.getField("uistate", "Mode.value") || "";
  if (mode !== "interact") return;
  if (entities.length === 0) return;
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  const total = entities.length;
  const progress = ctx.getField("colony", "Colony._rune_progress") | 0;

  // If already solved, do nothing.
  if (progress >= total) return;

  // Find the nearest inactive rune within 1.5 tiles.
  let nearest = null, nearestD2 = Infinity;
  for (const e of entities) {
    if (e.Rune.active | 0) continue; // skip active runes
    const dx = e.Position.x - px, dy = e.Position.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 < nearestD2) { nearestD2 = d2; nearest = e; }
  }
  if (!nearest || nearestD2 > 2.25) return; // 1.5^2 = 2.25

  const seq = nearest.Rune.sequence | 0;
  // Check if this is the next rune in sequence.
  if (seq === progress + 1) {
    // Correct! Light this rune + increment progress.
    nearest.Rune.active = 1;
    ctx.setField("colony", "Colony._rune_progress", progress + 1);
    ctx.emit("toast-show", { text: "符文 " + seq + " 激活" });
    // If all runes lit, solve the puzzle.
    if (progress + 1 >= total) {
      ctx.setField("colony", "Colony._rune_progress", 0);
      ctx.emit("rune-puzzle-solved", {});
      ctx.emit("toast-show", { text: "封印解除!可以探索遗迹了" });
    }
  } else {
    // Wrong order! Reset all runes + spawn enemy.
    for (const e of entities) {
      e.Rune.active = 0;
    }
    ctx.setField("colony", "Colony._rune_progress", 0);
    ctx.spawn({
      Enemy: { kind: "gnawer", damage: 5, aggro_range: 8, home_region: "wild", _attack_cd: 0, _hit_flash: 0, _charge_t: 0, _charge_cd: 0, _flank_dir: 0, _nest_id: "" },
      Hp: { value: 15, max: 15 },
      Position: { x: px + 2, y: py + 2 },
      Velocity: { x: 0, y: 0 },
      Sprite: { w: 0.8, h: 0.8, image: "", color: "#7a3a3a" },
    });
    ctx.emit("toast-show", { text: "符文顺序错误!封印反噬" });
  }
});

// ---- rune-puzzle-solve system: listens for rune-puzzle-solved event, finds nearest ruin POI ----
// We can't query in fns, so this system runs every tick and checks if Colony._rune_solve_pending
// is set (the event handler sets it). When set, find the nearest unsolved ruin POI and solve it.
vitric.system("rune-puzzle-solve", { query: ["Poi", "Position"], writes: ["Poi"] }, (entities, ctx) => {
  const pending = ctx.getField("colony", "Colony._rune_solve_pending") | 0;
  if (!pending) return;
  // Clear the pending flag first (idempotent).
  ctx.setField("colony", "Colony._rune_solve_pending", 0);
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  let bestD2 = Infinity, bestId = null;
  for (const e of entities) {
    if ((e.Poi.risk_tier || "safe") !== "ruin") continue;
    if (e.Poi.rune_solved | 0) continue;
    const dx = e.Position.x - px, dy = e.Position.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; bestId = e.id; }
  }
  if (bestId) {
    ctx.setField(bestId, "Poi.rune_solved", 1);
  }
});
