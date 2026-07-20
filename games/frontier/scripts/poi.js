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

  // Cave-entrance risk: 30% chance of companion mood drop (cave-injury).
  if (poi.kind === "cave-entrance" && ctx.random() < POI_CAVE_INJURY_CHANCE) {
    ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
    ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
  }

  // Notify wish system (Task 4 will add a rule listening for this event).
  ctx.emit("entered-poi", { kind: poi.kind });
});
