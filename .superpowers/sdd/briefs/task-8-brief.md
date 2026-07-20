# Task 8 Brief — Tech Tree (12 nodes, 4 branches × 3 tiers)

## Context

Frontier Sandbox Expansion, Task 8 of 16. Base commit: `0aa546f` (after Task 7 review artifacts). Plan: `docs/superpowers/plans/2026-07-20-frontier-sandbox-expansion.md` §Task 8 (lines 1026-1097). Spec: `docs/superpowers/specs/2026-07-20-frontier-sandbox-expansion-design.md` §4.3 (lines 217-244).

Phase 1 (E1-E5 engine capabilities) is complete. Phase 2 game systems: Task 6 (Seasons/Weather) ✅, Task 7 (Forecast HUD) ✅, Task 8 (Tech Tree) = this task, Task 9 (Companions Expansion) next.

## Goal

Add a 12-node tech tree (4 branches × 3 tiers) with: `Research` + `TechPoint` components, `research` mode + tech panel UI, `requires` field on tier-2/3 recipes, TechPoint earning from POI exploration, region unlocks on exploration/industry tier completion.

## Plan corrections (fictional APIs in plan pseudocode)

The plan's pseudocode contains several fictional APIs/patterns. Use these real equivalents:

1. **`test_progression.py` does NOT exist** — the codebase uses Rust integration tests in `crates/vitric-cli/tests/`. Write Rust tests in `crates/vitric-cli/tests/research.rs` following the pattern of `seasons.rs` / `region.rs`.
2. **Rule engine format**: `{call: "fnName", with: {args}}` for calling JS fns registered via `vitric.fn(name, fn)`. `{set: "@entity.Comp.field", to: value}` for direct field sets, where `value` can be a literal OR `{format: "...", args: [...]}` for templated strings (args can be `@entity.Comp.field` reads). See `rules/companion.json`, `rules/economy.json`, `rules/hud.json` for working examples.
3. **`ctx.thaw_region(id)` is the real API** for region unlock (E1 from Task 1). It transitions Region.state dormant→active + emits `region-thaw` event. Safe no-op on missing region. Use this inside an `unlock_region` JS fn — do NOT invent a separate `unlock_recipes` fn (recipe unlocking is implicit via the `requires` check in the build fn + affordability rule).
4. **`update_affordability_with_requires` fn is NOT needed** — the existing `rules/affordability.json` is rule-based. Extend it directly with extra `if` clauses checking `@colony.Research.has_<branch>_t<tier>` (boolean fields — see Schema below).
5. **`Inventory` extension**: plan says add `hide`, `crystal_core`. `hide` is for Task 10 (combat drops) — declare it now but it's unused until Task 10. `crystal_core` is for tech tree + trade — used as a cost on tier-3 recipes.
6. **Mode enum extension**: plan says add `["build", "craft", "interact", "upgrade", "research", "combat", "trade"]`. For Task 8 only add `"research"` — `"combat"` and `"trade"` are added in Tasks 10/11. Adding them now would be dead code.

## Schema changes (`games/frontier/schema.json`)

### New components

```json
"Research": {
  "fields": {
    "known": { "type": "text", "default": "[]" },
    "current": { "type": "text", "default": "" },
    "progress": { "type": "number", "default": 0 },
    "cost_total": { "type": "int", "default": 0 },
    "has_survival_t1": { "type": "int", "default": 0 },
    "has_survival_t2": { "type": "int", "default": 0 },
    "has_survival_t3": { "type": "int", "default": 0 },
    "has_agriculture_t1": { "type": "int", "default": 0 },
    "has_agriculture_t2": { "type": "int", "default": 0 },
    "has_agriculture_t3": { "type": "int", "default": 0 },
    "has_exploration_t1": { "type": "int", "default": 0 },
    "has_exploration_t2": { "type": "int", "default": 0 },
    "has_exploration_t3": { "type": "int", "default": 0 },
    "has_industry_t1": { "type": "int", "default": 0 },
    "has_industry_t2": { "type": "int", "default": 0 },
    "has_industry_t3": { "type": "int", "default": 0 }
  }
},
"TechPoint": {
  "fields": {
    "value": { "type": "int", "default": 0 }
  }
}
```

**Why 12 boolean `has_*` fields**: rule engine `if` clauses need numeric/enum comparisons, NOT text-substring checks. Each tier completion sets the corresponding `has_*` to 1; affordability rules check `["@colony.Research.has_agriculture_t1", ">=", 1]`. The `known` text field is the source of truth (JSON array of tech ids); the booleans are derived mirrors for rule-engine consumption. The `research-progress` system updates BOTH when a research completes (push id to `known` array + set `has_*` to 1).

### Extend existing components

- `Mode.value` enum: add `"research"` to variants. Final: `["build", "craft", "interact", "upgrade", "research"]`. Do NOT add `"combat"` or `"trade"` (those come in Tasks 10/11).
- `Inventory`: add `hide` (int, default 0) and `crystal_core` (int, default 0). Add to the `ITEMS` array in `economy.js` and `poi.js` so they round-trip through `inv-set` emit. Update `readInv` and `emitInv` in `economy.js` and the inline inventory reads in `poi.js`'s `interact_poi`.

## Scene changes (`games/frontier/scenes/main.json`)

### Attach components

- `colony` entity: add `Research` component (all defaults).
- `player` entity: add `TechPoint` component (value: 0).

### New HUD entities

- `techpoint_lbl`: HUD label showing "科技点 N". Position: top-right area. Use `anchor: "top-right"`, `parent: "ui"`, `ox: 24`, `oy: 100` (same row as `mode_row` but on the right — set `ax` to position from right edge if needed; alternatively place below `forecast_lbl` at `oy: 286`). Choose a non-overlapping position. Recommended: `oy: 286` (below forecast_lbl at oy:258 + h:28 = 286), `anchor: "top-left"`, `parent: "ui"`, `w: 200`, `h: 24`.
- `research_status_lbl`: HUD label showing current research progress "研究中: <name> (NN%)". Position below `techpoint_lbl` at `oy: 314`, `h: 24`.

### New UI: tech panel

- `tech_menu`: container for 12 tech buttons. Use same pattern as `build_menu` (VBox or Grid). Recommended: `anchor: "top-left"`, `parent: "ui"`, `ox: -3000` (hidden by default), `oy: 176`, `w: 348`, `h: 400`. `Container: { kind: "Grid", columns: 3, gap: 8, pad: 12, main: "start", cross: "center" }`. (3 columns × 4 rows = 12 cells; each cell is one branch's 3 tiers stacked, OR just a flat 3×4 grid of all 12 techs — pick whatever reads best. Branch-tier ordering: rows = branches [survival, agriculture, exploration, industry], columns = tiers [T1, T2, T3].)
- 12 `tech_<id>` button entities (one per tech id). Each: `parent: "tech_menu"`, `w: 100`, `h: 80`, `Button.action: "pick-tech-<id>"`, `Panel.color: "#33405e"` (default), `Button.state: "normal"`.
- 12 `tech_<id>_lbl` label entities. Each: `parent: "tech_<id>"`, `UiLabel.size: 16`, `UiLabel.content: "<name>"` (initial — the research-status system updates these to show cost/progress/locked state).

### Mode button

Add a `mode_research` button to `mode_row` (4th button after mode_interact). `mode_row` is currently width 320 holding 3 buttons × 92 + 2 gaps × 6 = 288. Bump `mode_row.w` to 386 (or to 380 with gap 4) so a 4th button fits. `mode_research`: `parent: "mode_row"`, `w: 92`, `h: 48`, `Button.action: "mode-research"`, `Panel.color: "#3a4a6b"`. Plus `mode_research_lbl` with content "科技".

## Script: `games/frontier/scripts/research.js` (NEW)

```javascript
// Tech tree: 12 nodes across 4 branches (survival/agriculture/exploration/industry) × 3 tiers.
// Research consumes TechPoints + real time (T1 = 0.5 day, T3 = 2 days; runs in background).
// On completion: emit `researched{id}` event → rule calls unlock_region fn for region-unlocking techs.
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

vitric.system("research-progress", { query: ["Research"], writes: ["Research"] }, (entities, ctx) => {
  for (const e of entities) {
    const cur = e.Research.current || "";
    if (!cur) continue;
    const tech = TECH_TREE[cur];
    if (!tech) { e.Research.current = ""; e.Research.progress = 0; e.Research.cost_total = 0; continue; }

    e.Research.progress += ctx.dt;
    if (e.Research.progress >= tech.time) {
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
```

**Notes:**
- `TECH_TREE` is the single source of truth for tech metadata. Don't duplicate it.
- `research-progress` system: query only `[Research]` (Research is on colony, single entity). Does NOT need Clock/Season — `ctx.dt` is enough.
- `tech-panel-hint` system: writes via `ctx.setField("tech_<id>_lbl", ...)` using entity-name lookup (the engine resolves entity names). `writes: []` is correct because `ctx.setField` is not a query-based write — it's a direct entity-name write (the declaration `writes: []` is for query-locking purposes; setField bypasses it).
- `start_research` fn: receives `techpoint` and `known` as args from the rule (rule reads `@player.TechPoint.value` and `@colony.Research.known` and passes them in `with`). Returns early with toast on any validation failure.
- `unlock_region` fn: calls `ctx.thaw_region(region_id)` (E1 API from Task 1). Safe no-op if region doesn't exist (Task 1 contract — defensive). Desert region added in Task 12.

## Script: `games/frontier/scripts/economy.js` (MODIFY)

### Extend BUILD table

Add `requires` field to tier-2/3 entries. Add 6 new tier-2/3 recipes:

```javascript
const BUILD = {
  plot:      { cost: {},                              tier: 1, color: "#6b8f3a", label: "种植台", size: 1.0 },
  wall:      { cost: { wood: 1 },                     tier: 1, color: "#8a7a5c", label: "墙",     size: 1.0 },
  conduit:   { cost: { ore: 1 },                      tier: 1, color: "#d8a83a", label: "电导管", size: 1.0 },
  extractor: { cost: { ore: 1 },                      tier: 1, color: "#4aa6c8", label: "抽水机", size: 1.0 },
  quarters:  { cost: { plank: 2 },                    tier: 1, color: "#c08a4a", label: "住所",   size: 1.1 },
  beacon:    { cost: { ore: 2, plank: 2 },            tier: 1, color: "#f5b942", label: "信标",   size: 1.8 },
  // Tier 2+ (requires tech):
  plot2:     { cost: { plank: 3, chair: 1 },          tier: 2, color: "#a8e85a", label: "良田",   size: 1.05, requires: "agriculture_t1" },
  monument:  { cost: { ore: 4, plank: 4, lamp: 2, wheat: 4 }, tier: 3, color: "#ffe066", label: "丰碑", size: 2.0, requires: "industry_t2" },
  // NEW tier-2/3 recipes unlocked by tech tree:
  well2:       { cost: { ore: 2, plank: 1 },            tier: 2, color: "#5fb8d8", label: "改良水井", size: 1.0, requires: "survival_t1" },
  recycler:    { cost: { ore: 3, plank: 2 },            tier: 2, color: "#8a6bc8", label: "回收器",   size: 1.0, requires: "survival_t2" },
  dome:        { cost: { ore: 4, plank: 4, crystal_core: 1 }, tier: 3, color: "#88e0ff", label: "大气穹顶", size: 1.6, requires: "survival_t3" },
  hydroponics: { cost: { ore: 3, plank: 3, crystal_core: 1 }, tier: 3, color: "#88ffaa", label: "水培",     size: 1.2, requires: "agriculture_t3" },
  arc_gun:     { cost: { ore: 3, crystal_core: 1 },     tier: 2, color: "#ff6b6b", label: "电磁炮",   size: 1.0, requires: "industry_t1" },
  turret:      { cost: { ore: 4, plank: 2, crystal_core: 1 }, tier: 2, color: "#ff8a4a", label: "炮塔",   size: 1.0, requires: "industry_t1" },
};
```

### Extend `build` fn with `requires` check

At the top of the `build` fn, after `const def = BUILD[a.kind];` and `if (!def) return;`, add:

```javascript
if (def.requires) {
  let known = [];
  try { known = JSON.parse(a.known || "[]"); } catch { known = []; }
  if (!known.includes(def.requires)) {
    ctx.emit("build-fail", { kind: a.kind, label: def.label });
    ctx.emit("toast-show", { text: "需要科技: " + (TECH_NAMES[def.requires] || def.requires) });
    return;
  }
}
```

(Where `TECH_NAMES` is a small lookup `{ "survival_t1": "改良水井", ... }` — OR just import `TECH_TREE` from research.js. Since QuickJS doesn't have ES modules, duplicate the names as a small const at the top of economy.js, OR just emit the raw tech id in the toast text without name lookup — simpler. Pick simpler: `text: "需要科技: " + def.requires`.)

The rule that calls `build` must now also pass `known` in `with`. See Rules section below.

### Extend ITEMS + readInv + emitInv

```javascript
const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core"];
```

`readInv` and `emitInv` already iterate over `ITEMS`, so they pick up the new fields automatically. The `interact` fn (plot plant/harvest, node gather) doesn't need changes — it doesn't touch hide/crystal_core.

## Script: `games/frontier/scripts/poi.js` (MODIFY)

### Add TechPoint reward to `interact_poi`

In `interact_poi` fn, after the existing reward loop + before `ctx.emit("toast-show", ...)`:

```javascript
// Award TechPoints for POI exploration (+2 per fresh POI).
const tp = (a.techpoint | 0) + 2;
ctx.emit("tp-set", { value: tp });
```

Also update the inline `ITEMS` list in `interact_poi` to match economy.js (add `hide`, `crystal_core`). The rule that calls `interact_poi` must now also pass `techpoint: "@player.TechPoint.value"` in `with` (see Rules section).

## Rules: `games/frontier/rules/research.json` (NEW)

```json
{
  "rules": [
    {
      "id": "research-unlock-mountain",
      "comment": "On exploration_t1 complete: thaw mountain region (E1 API).",
      "on": { "event": "researched", "filter": { "id": "exploration_t1" } },
      "do": [ { "call": "unlock_region", "with": { "region_id": "mountain" } } ]
    },
    {
      "id": "research-unlock-desert",
      "comment": "On industry_t3 complete: thaw desert region (Task 12 adds the region entity — until then thaw_region is a silent no-op per Task 1 contract).",
      "on": { "event": "researched", "filter": { "id": "industry_t3" } },
      "do": [ { "call": "unlock_region", "with": { "region_id": "desert" } } ]
    },
    {
      "id": "tp-apply",
      "comment": "Apply TechPoint write-back from interact_poi / start_research (emit tp-set {value} -> @player.TechPoint.value).",
      "on": { "event": "tp-set" },
      "do": [ { "set": "@player.TechPoint.value", "to": "event.value" } ]
    }
  ]
}
```

**Note on `event.value`**: rule engine supports reading event fields via `event.<field>` syntax (see existing rules using `event.id`, `event.n`, `event.amount` in `rules/companion.json` / `rules/wish.json` / `rules/poi.json`).

## Rules: `games/frontier/rules/economy.json` (MODIFY)

### Pass `known` to `build` fn

Find the existing `build` rule (the one with `"call": "build"`). Add `"known": "@colony.Research.known"` to the `with` object. The build fn reads this to check `requires`.

### Pass `techpoint` to `interact_poi` fn

Find the existing `interact_poi` rule in `rules/poi.json`. Add `"techpoint": "@player.TechPoint.value"` to the `with` object.

## Rules: `games/frontier/rules/ui.json` (MODIFY)

### Add `mode-research` + `kb-mode-research` rules

```json
{
  "id": "mode-research",
  "comment": "切科研模式:显科技菜单,藏建造/制作菜单。",
  "on": { "event": "ui-activate", "filter": { "action": "mode-research" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "research" },
    { "set": "@tech_menu.Ui.ox", "to": 208 },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 }
  ]
},
{
  "id": "kb-mode-research",
  "comment": "T 键切科研模式。",
  "on": { "event": "input", "filter": { "action": "t", "phase": "pressed" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "research" },
    { "set": "@tech_menu.Ui.ox", "to": 208 },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 }
  ]
}
```

Also extend the existing `mode-build` / `mode-craft` / `mode-interact` / `kb-mode-upgrade` rules to hide `tech_menu` (add `{ "set": "@tech_menu.Ui.ox", "to": -3000 }` to each `do` array). Otherwise tech_menu stays visible when switching modes.

### Add 12 `pick-tech-<id>` rules

For each of the 12 tech ids, add a rule:

```json
{
  "id": "pick-tech-survival_t1",
  "on": { "event": "ui-activate", "filter": { "action": "pick-tech-survival_t1" } },
  "do": [ { "call": "start_research", "with": {
    "tech_id": "survival_t1",
    "techpoint": "@player.TechPoint.value",
    "known": "@colony.Research.known"
  } } ]
}
```

Repeat for all 12 ids. (This is verbose but mechanical — 12 nearly-identical rules.)

## Rules: `games/frontier/rules/affordability.json` (MODIFY)

### Add tech-locked affordability rules for tier-2/3 build buttons

For each new tier-2/3 build button (well2, recycler, dome, hydroponics, arc_gun, turret, plot2, monument), add an affordability rule that checks BOTH cost AND tech:

```json
{
  "id": "afford-well2",
  "on": "tick",
  "if": [ ["@player.Inventory.ore", ">=", 2], ["@player.Inventory.plank", ">=", 1], ["@colony.Research.has_survival_t1", ">=", 1] ],
  "do": [ { "set": "@build_well2.Panel.color", "to": "#33405e" } ]
}
```

Also extend `build-dim-all` (the first rule that dims everything) to include the new buttons:

```json
{ "set": "@build_well2.Panel.color", "to": "#262a33" },
{ "set": "@build_recycler.Panel.color", "to": "#262a33" },
{ "set": "@build_dome.Panel.color", "to": "#262a33" },
{ "set": "@build_hydroponics.Panel.color", "to": "#262a33" },
{ "set": "@build_arc_gun.Panel.color", "to": "#262a33" },
{ "set": "@build_turret.Panel.color", "to": "#262a33" }
```

(plot2 and monument are already in `build-dim-all`.)

For the existing `afford-plot2` and `afford-monument` rules, add the tech `if` clause:

```json
{ "id": "afford-plot2", "on": "tick", "if": [ ["@player.Inventory.plank", ">=", 3], ["@player.Inventory.chair", ">=", 1], ["@colony.Research.has_agriculture_t1", ">=", 1] ], "do": [ { "set": "@build_plot2.Panel.color", "to": "#33405e" } ] },
{ "id": "afford-monument", "on": "tick", "if": [ ["@player.Inventory.ore", ">=", 4], ["@player.Inventory.plank", ">=", 4], ["@player.Inventory.lamp", ">=", 2], ["@player.Inventory.wheat", ">=", 4], ["@colony.Research.has_industry_t2", ">=", 1] ], "do": [ { "set": "@build_monument.Panel.color", "to": "#33405e" } ] }
```

### Add 6 new build button entities to scene

In `scenes/main.json`, add `build_well2`, `build_recycler`, `build_dome`, `build_hydroponics`, `build_arc_gun`, `build_turret` entities inside `build_menu` (same pattern as `build_plot2`). Plus their `_lbl` entities. Note: `build_menu` is a VBox with gap 8 — adding 6 more buttons (h:40 each + gap 8) = 288 more vertical space. `build_menu.h` is currently 400 — extend to ~700 to fit 14 buttons (8 original + 6 new). Or use a 2-column Grid layout for build_menu. Pick whichever is simpler — extending h to 700 is fine.

## Rules: `games/frontier/rules/hud.json` (MODIFY)

### Add `hud-techpoint` + `hud-research-status` rules

```json
{
  "id": "hud-techpoint",
  "comment": "科技点 HUD:科技点 N。每帧刷 @player.TechPoint.value。",
  "on": "tick",
  "do": [
    { "set": "@techpoint_lbl.UiLabel.content", "to": { "format": "科技点 {}", "args": ["@player.TechPoint.value"] } }
  ]
},
{
  "id": "hud-research-status",
  "comment": "研发状态 HUD:研究中: <name> (NN%)。每帧刷 @colony.Research。",
  "on": "tick",
  "do": [
    { "set": "@research_status_lbl.UiLabel.content", "to": { "format": "研究中: {} ({}/{}s)", "args": ["@colony.Research.current", "@colony.Research.progress", "@colony.Research.cost_total"] } }
  ]
}
```

(Using `progress/cost_total` format instead of percentage — simpler than computing pct in rule engine. The label shows "研究中: survival_t1 (45.2/90s)" when researching, "研究中:  (0/0s)" when idle — the idle state looks slightly odd but is acceptable. If you want a cleaner idle display, add an `if` clause: when `@colony.Research.current` is empty, set content to "空闲".)

## Tests: `crates/vitric-cli/tests/research.rs` (NEW)

Follow the pattern of `crates/vitric-cli/tests/seasons.rs` (uses `Runtime::boot(frontier_dir())` for full scene + logic). 4 tests:

1. **`research_progress_advances_with_dt`**: Boot runtime, call `start_research` fn directly via `ScriptEngine::call_fn` (or via `Runtime` API — check seasons.rs for the exact pattern) with `tech_id: "survival_t1", techpoint: 5, known: "[]"`. Step 1 tick. Verify `Research.progress > 0`.

2. **`research_completes_after_time`**: Same setup. Step enough ticks to exceed `tech.time` (45s = 2700 ticks at 60Hz). Verify `Research.known` contains "survival_t1", `Research.has_survival_t1 == 1`, `Research.current == ""`, and `researched` event was emitted.

3. **`start_research_deducts_techpoints`**: Setup: player has TechPoint.value = 5. Call `start_research` with survival_t1 (cost 2). Verify TechPoint.value becomes 3 (via the `tp-set` event → rule → @player.TechPoint.value write-back). This may require stepping a tick for the rule to fire.

4. **`start_research_rejects_insufficient_techpoints`**: Setup: player has TechPoint.value = 1. Call `start_research` with survival_t1 (cost 2). Verify TechPoint.value stays 1, Research.current stays empty, `toast-show` event emitted with "科技点不足".

**Test setup notes** (from Task 6's seasons.rs pattern):
- Use `Runtime::boot(frontier_dir())` to load the full Frontier scene + logic.
- `frontier_dir()` helper: `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")`.
- To call a JS fn: check the `Runtime` API — likely `rt.call_fn(name, args_json)` or via `rt.step_with_event(Event::named("ui-activate", json!({...})))` to trigger the rule that calls the fn. Prefer the event-driven approach (sends a `ui-activate` event that matches the `pick-tech-<id>` rule) — this tests the full flow.
- To advance time: `rt.step_ticks(n)`.
- To read entity state: `rt.world().get_component(rt.world().entity("colony")?, "Research")?`.
- Keep tests fast: 1-5 ticks each, except the completion test which needs ~2700 ticks (use release mode if debug is too slow, or reduce `tech.time` to 1s in test setup via direct component write).

If the completion test is too slow in debug, set `Research.cost_total` to a small value (e.g., 0.1s = 6 ticks) via direct component write before stepping — this bypasses `start_research`'s cost check but tests the completion logic. Document this as a test-only shortcut.

## Verification

After implementation:

```bash
# Schema check (must exit 0)
~/.cargo/bin/cargo run --release -- check games/frontier

# New research tests (4 must pass)
~/.cargo/bin/cargo test -p vitric-cli --test research

# Regression: seasons (4) + region (14) still pass
~/.cargo/bin/cargo test -p vitric-cli --test seasons
~/.cargo/bin/cargo test -p vitric-cli --test region -- --skip typescript

# Workspace all-green (skip typescript — pre-existing esbuild missing)
~/.cargo/bin/cargo test --workspace -- --skip typescript

# Gate EXPECTED-FAIL (ReplayDiverged at tick 0 — Research/TechPoint on colony/player changes tick-0 world hash; new HUD entities too)
# DO NOT re-record qa/clear.json — Task 15 handles that.
~/.cargo/bin/cargo run --release -- gate games/frontier 2>&1 | tail -5
```

## Commit

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json \
        games/frontier/scripts/research.js games/frontier/scripts/economy.js \
        games/frontier/scripts/poi.js games/frontier/rules/research.json \
        games/frontier/rules/economy.json games/frontier/rules/ui.json \
        games/frontier/rules/affordability.json games/frontier/rules/hud.json \
        games/frontier/rules/poi.json crates/vitric-cli/tests/research.rs
git commit -m "feat(frontier): tech tree with 12 nodes across 4 branches"
git push origin main
```

## Critical reminders (project-wide rules)

1. **All code comments must be English** (// and /* */ in JS, // in Rust). String literals (panic messages, toast text, UI labels, game content) keep their original language.
2. **Every field read by a rule (`@entity.Comp.field`) MUST be declared in `schema.json`**. This task adds: `Research.known`, `Research.current`, `Research.progress`, `Research.cost_total`, `Research.has_*` (12 fields), `TechPoint.value`. All must be in schema. Audit before committing.
3. **Every field accessed via `ctx.getField` / `ctx.setField` MUST be declared**. `start_research` uses `ctx.setField("colony", "Research.current", ...)`, `ctx.setField("colony", "Research.progress", ...)`, `ctx.setField("colony", "Research.cost_total", ...)`. `tech-panel-hint` uses `ctx.setField("tech_<id>_lbl", "UiLabel.content", ...)`. `UiLabel.content` is already declared. All Research.* fields must be declared.
4. **Rule engine format**: `{call: "fnName", with: {args}}` for fn calls; `{set: "path", to: value}` for field writes (value can be literal or `{format, args}`).
5. **Mode enum**: add ONLY `"research"` — NOT `"combat"` or `"trade"` (those are Tasks 10/11).
6. **Gate EXPECTED-FAIL is OK** — do NOT re-record `qa/clear.json`. Task 15 handles it.
7. **`desert` region doesn't exist yet** — `unlock_region("desert")` is a silent no-op (Task 1 contract). Task 12 adds the desert region entity.
8. **Don't add `combat`/`trade` mode buttons, combat mode UI, or trade mode UI** — those are Tasks 10/11.

## Deliverable

Return a report at `.superpowers/sdd/briefs/task-8-report.md` with:
- Commit hash
- Files changed (count + list)
- Test results (research 4/4, seasons 4/4, region 14/14, schema check exit 0, workspace all-green, gate failure mode)
- Deviations from this brief (with reasoning)
- Concerns / known issues (e.g., DRY violations, expected gate failure, etc.)

Do NOT update `progress.md` — the controller does that after review.
