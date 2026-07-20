// Tech tree: 12 nodes across 4 branches (survival/agriculture/exploration/industry) × 3 tiers.
// Research consumes TechPoints + real time (T1 = 0.5 day, T3 = 2 days; runs in background).
// On completion: emit `researched{id}` event -> rule calls unlock_region fn for region-unlocking techs.
// TechPoints earned from: POI exploration (+2 per POI, added in poi.js), future: trade.

const TECH_TREE = {
  "survival_t1":     { branch: "survival",    tier: 1, name: "改良水井",   cost: 2,  time: 45,   requires: "",                unlocks: { recipes: ["well2"], regions: [] } },
  "survival_t2":     { branch: "survival",    tier: 2, name: "水循环",     cost: 4,  time: 90,   requires: "survival_t1",     unlocks: { recipes: ["recycler"], regions: [] } },
  "survival_t3":     { branch: "survival",    tier: 3, name: "大气穹顶",   cost: 8,  time: 180,  requires: "survival_t2",     unlocks: { recipes: ["dome"], regions: [] } },
  "agriculture_t1":  { branch: "agriculture", tier: 1, name: "温室",       cost: 2,  time: 45,   requires: "",                unlocks: { recipes: ["plot2"], regions: [] } },
  "agriculture_t2":  { branch: "agriculture", tier: 2, name: "滴灌",       cost: 4,  time: 90,   requires: "agriculture_t1",  unlocks: { recipes: [], regions: [] } },
  "agriculture_t3":  { branch: "agriculture", tier: 3, name: "水培",       cost: 8,  time: 180,  requires: "agriculture_t2",  unlocks: { recipes: ["hydroponics"], regions: [] } },
  "exploration_t1":  { branch: "exploration", tier: 1, name: "山区探索",   cost: 2,  time: 45,   requires: "",                unlocks: { recipes: [], regions: ["mountain"] } },
  "exploration_t2":  { branch: "exploration", tier: 2, name: "雷达",       cost: 4,  time: 90,   requires: "exploration_t1",  unlocks: { recipes: [], regions: [] } },
  "exploration_t3":  { branch: "exploration", tier: 3, name: "信标网",     cost: 8,  time: 180,  requires: "exploration_t2",  unlocks: { recipes: [], regions: [] } },
  "industry_t1":     { branch: "industry",    tier: 1, name: "电磁武器",   cost: 2,  time: 45,   requires: "",                unlocks: { recipes: ["arc_gun", "turret"], regions: [] } },
  "industry_t2":     { branch: "industry",    tier: 2, name: "合金结构",   cost: 4,  time: 90,   requires: "industry_t1",     unlocks: { recipes: [], regions: [] } },
  "industry_t3":     { branch: "industry",    tier: 3, name: "沙漠商路",   cost: 8,  time: 180,  requires: "industry_t2",     unlocks: { recipes: [], regions: ["desert"] } },
};

// `time` is in seconds (45s = 0.5 day at DAY_SEC=90; 180s = 2 days).
// `cost` is TechPoints.

// Background research timer: advances Research.progress by ctx.dt each tick.
// On completion: push id into known array, set has_* flag, emit researched + toast-show.
vitric.system("research-progress", { query: ["Research"], writes: ["Research"] }, (entities, ctx) => {
  for (const e of entities) {
    const cur = e.Research.current || "";
    if (!cur) continue;
    const tech = TECH_TREE[cur];
    if (!tech) { e.Research.current = ""; e.Research.progress = 0; e.Research.cost_total = 0; continue; }

    e.Research.progress += ctx.dt;
    // cost_total is set by start_research (= tech.time). Read from the entity so tests can
    // short-circuit completion by writing a smaller cost_total directly.
    const total = e.Research.cost_total || tech.time;
    if (e.Research.progress >= total) {
      // Complete: add to known array, set has_* flag, emit researched event.
      let known = [];
      try { known = JSON.parse(e.Research.known || "[]"); } catch { known = []; }
      if (!known.includes(cur)) known.push(cur);
      e.Research.known = JSON.stringify(known);
      e.Research["has_" + cur] = 1;
      e.Research.current = "";
      e.Research.progress = 0;
      e.Research.cost_total = 0;
      ctx.emit("researched", { id: cur, branch: tech.branch, tier: tech.tier, name: tech.name });
      ctx.emit("toast-show", { text: "研发完成: " + tech.name });
    }
  }
});

// Update tech panel button labels to show current state (locked / researching / done).
// writes: [] because ctx.setField is a direct entity-name write, not a query-based write.
vitric.system("tech-panel-hint", { query: ["Research"], writes: [] }, (entities, ctx) => {
  for (const e of entities) {
    let known = [];
    try { known = JSON.parse(e.Research.known || "[]"); } catch { known = []; }
    for (const id of Object.keys(TECH_TREE)) {
      const tech = TECH_TREE[id];
      let text = tech.name;
      if (known.includes(id)) {
        text = "✓" + tech.name;
      } else if (e.Research.current === id) {
        const pct = Math.floor((e.Research.progress / tech.time) * 100);
        text = tech.name + " " + pct + "%";
      } else {
        text = tech.name + " " + tech.cost + "TP";
      }
      // Label lives on tech_<id>_lbl entity; write via ctx.setField (entity name lookup).
      ctx.setField("tech_" + id + "_lbl", "UiLabel.content", text);
    }
  }
});

// Start research: rule calls this on ui-activate{action:"pick-tech-<id>"}.
// Args: { tech_id, techpoint (current TechPoint.value from @player), known (Research.known from @colony) }
vitric.fn("start_research", (a, ctx) => {
  const id = a.tech_id;
  const tech = TECH_TREE[id];
  if (!tech) return;
  const tp = a.techpoint | 0;
  let known = [];
  try { known = JSON.parse(a.known || "[]"); } catch { known = []; }
  if (known.includes(id)) { ctx.emit("toast-show", { text: "已研发完成" }); return; }
  if (tech.requires && !known.includes(tech.requires)) {
    ctx.emit("toast-show", { text: "需要前置科技" });
    return;
  }
  if (tp < tech.cost) { ctx.emit("toast-show", { text: "科技点不足" }); return; }
  // Deduct TechPoints + set Research.current.
  ctx.emit("tp-set", { value: tp - tech.cost });
  ctx.setField("colony", "Research.current", id);
  ctx.setField("colony", "Research.progress", 0);
  ctx.setField("colony", "Research.cost_total", tech.time);
  ctx.emit("toast-show", { text: "开始研发: " + tech.name });
});

// Unlock region: rule calls this on researched event for techs that unlock regions.
// Args: { region_id }
vitric.fn("unlock_region", (a, ctx) => {
  if (typeof a.region_id !== "string" || !a.region_id) return;
  ctx.thaw_region(a.region_id);
  ctx.emit("toast-show", { text: "区域解锁: " + a.region_id });
});
