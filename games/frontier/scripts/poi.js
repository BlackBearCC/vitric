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
        Enemy: { kind: "gnawer", damage: 5, aggro_range: 6, home_region: "swamp", _attack_cd: 0 },
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

  // Parse reward table: {item: [lo, hi]}.
  let rewards = {};
  try { rewards = JSON.parse(poi.reward_table || "{}"); } catch { return; }

  // Build inventory from args (same pattern as economy.js readInv).
  // `hide` + `crystal_core` round-trip through inv-set alongside the rest of the inventory.
  const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core"];
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
