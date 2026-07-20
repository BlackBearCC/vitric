# Task 3: POI System (poi.js + poi.json + scene entities)

## Context

Vitric is a deterministic 2D game engine. `games/frontier/` is a survival demo being deepened from a linear 8-step quest into four free-running loops. Task 3 adds the **wild exploration loop**: 3 daily-refreshing Points of Interest (POIs) in the wild zone that the player can interact with for rewards.

Task 1 (schema) and Task 2 (flare + night) are already committed on `main`. This task stands alone: it adds the POI system end-to-end.

## Files

- **Modify:** `games/frontier/tools/gen_scene.py` — add 3 POI entities to the scene generator
- **Regenerate:** `games/frontier/scenes/main.json` — by running the updated gen_scene.py
- **Create:** `games/frontier/scripts/poi.js` — POI tick system + `interact_poi` fn
- **Create:** `games/frontier/rules/poi.json` — rule that captures interact-mode click on a POI and calls the fn
- **Modify:** `games/frontier/vitric.json` — register the new script + rules

## Real API (CRITICAL — the plan's pseudocode used fake APIs; ignore them)

The engine's real scripting API, as confirmed from `scripts/flare.js`, `scripts/economy.js`, `rules/economy.json`, `rules/farm.json`:

```javascript
// System: runs each tick. `entities` is an array of entities that have ALL queried components.
// Each entity exposes its components as properties (e.Poi, e.Text, e.id for the handle).
// `writes` lists components this system will mutate.
vitric.system("name", { query: ["CompA", "CompB"], writes: ["CompA"] }, (entities, ctx) => {
  for (const e of entities) {
    e.CompA.field = newValue;          // direct write on queried+writes components
  }
});

// Named function: callable from rules via {"call": "name", "with": {...}}.
// `a` is the merged `with` object (rule can interpolate event fields + @entity.Component.field).
// `ctx` has: ctx.dt, ctx.random() (deterministic RNG — Math.random is poisoned), ctx.emit(name, data),
//            ctx.setField(entityHandle, "Comp.field", value), ctx.spawn(comps).
vitric.fn("name", (a, ctx) => {
  // a.entity = hit entity handle (from "event.entity")
  // a.comp = hit entity's components snapshot (from "event.comp")
  // a.ore, a.wood, ... = inventory fields passed in by the rule
});

// Inventory write-back pattern (from economy.js): emit "inv-set" with absolute values;
// rules/economy.json's "inv-apply" rule writes them back to @player.Inventory.*.
const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];
function emitInv(ctx, inv) {
  const d = {};
  for (const k of ITEMS) d[k] = inv[k] | 0;
  ctx.emit("inv-set", d);
}
```

**DO NOT USE** (these do not exist): `ctx.singleton()`, `ctx.each()`, `vitric.on()`, `vitric.expose()`, `vitric.call()`, `ctx.entity()`, `ctx.llm()`, `Math.random()`.

**Use `ctx.random()` for ALL randomness** — `Math.random()` is poisoned and throws in the QuickJS runtime (Task 2 fixed exactly this bug).

## Step 1: Modify `games/frontier/tools/gen_scene.py`

Read the file first. The wild zone is `WILD_NODES` at coordinates like (18,3)-(26,4). The scene builds entities as `{"name": ..., "components": {...}}` (named) or `{"components": {...}}` (anonymous).

After the existing `drifter` entity block (around line 104) and BEFORE the `# ---- UI shell ----` section, add 3 POI entities. Use named entities so the implementer can verify them in the generated scene:

```python
# ---- 3 POIs in wild zone (daily-refreshing wild points of interest) ----
POI_SPECS = [
    # (name, kind, x, y, reward_table) — reward_table: {item: [lo, hi]} rolled on explore.
    ("poi_camp",  "abandoned-camp", 19, 9, {"ore": [1, 2], "wheat": [2, 4], "fiber": [1, 3]}),
    ("poi_cave",  "cave-entrance",  22, 2, {"ore": [3, 5]}),                 # high-risk high-reward
    ("poi_wreck", "shipwreck",      27, 6, {"wheat": [3, 5], "plank": [1, 2]}),
]
POI_LABELS = {"abandoned-camp": "废弃营地", "cave-entrance": "洞穴入口", "shipwreck": "沉船"}
POI_COLORS = {"abandoned-camp": "#8b6f47", "cave-entrance": "#5a4a6a", "shipwreck": "#4a5a6a"}
for name, kind, x, y, rewards in POI_SPECS:
    entities.append({"name": name, "components": {
        "Position": {"x": x, "y": y},
        "Sprite": {"w": 1.6, "h": 1.6, "color": POI_COLORS[kind], "image": ""},
        "Collider": {"w": 1.6, "h": 1.6},
        "Poi": {
            "kind": kind,
            "state": "fresh",
            "cooldown": 0,
            "reward_table": json.dumps(rewards),
        },
        "Text": {"content": POI_LABELS[kind], "size": 0.4, "color": "#ffe070", "screen": False},
    }})
```

Place POIs on tiles that are NOT already in `WILD_ROCK`, `ORE`, `LANDER`, `WILD_NODES`. The coordinates above (19,9)/(22,2)/(27,6) are chosen to avoid existing occupant sets — verify against the sets at the top of the file and adjust if any collide. Coordinates (19,9) is in `WILD_ROCK` — move `poi_camp` to (18, 10) instead. Recheck (22,2) — it is in `WILD_ROCK`, move `poi_cave` to (23, 2). Recheck (27,6) — it is in `WILD_ROCK`, move `poi_wreck` to (26, 5). Final coordinates to use:
- `poi_camp`  at (18, 10)
- `poi_cave`  at (23, 2)
- `poi_wreck` at (26, 5)

Confirm each is not in any existing set (`LANDER`, `ROCK`, `ORE`, `ICE`, `WILD_ROCK`, `WILD_NODES`) before writing.

## Step 2: Regenerate the scene

Run: `cd /Users/leolele/Documents/leo/vitric/games/frontier && python3 tools/gen_scene.py`
Expected: prints `wrote .../scenes/main.json | entities: N` where N is previous count + 3.

## Step 3: Create `games/frontier/scripts/poi.js`

```javascript
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
  const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];
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
```

## Step 4: Create `games/frontier/rules/poi.json`

Use the same shape as `rules/farm.json` (mouse event + Mode=interact guard → call fn with hit entity + inventory). The `inv-apply` rule in `rules/economy.json` already handles the `inv-set` event globally, so we don't duplicate it here.

```json
{
  "rules": [
    {
      "id": "poi-interact-click",
      "comment": "Interact mode + left-click on a POI -> call interact_poi (rolls rewards, marks looted, emits inv-set + entered-poi). Inventory is passed in full; the existing inv-apply rule in economy.json writes it back.",
      "on": { "event": "mouse" },
      "if": [ ["@uistate.Mode.value", "==", "interact"] ],
      "do": [ { "call": "interact_poi", "with": {
        "entity": "event.entity", "comp": "event.comp", "x": "event.x", "y": "event.y",
        "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
        "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
        "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp"
      } } ]
    }
  ]
}
```

## Step 5: Register in `games/frontier/vitric.json`

- Add `"rules/poi.json"` to the `rules` array (after `"rules/flare.json"`).
- Add `"scripts/poi.js"` to the `scripts` array (after `"scripts/flare.js"`).

## Step 6: Verify

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -20`
Expected: exit code 0 (success). The engine prints a JSON diagnostic report; exit 0 = pass. Watch for any errors mentioning poi.js, poi.json, or Poi component.

If check fails, read the error output and fix. Common issues:
- Script syntax error (missing brace, bad export)
- Rule references unknown component/field
- POI entity in scene references unknown component

## Step 7: Commit + push

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/poi.js games/frontier/rules/poi.json games/frontier/tools/gen_scene.py games/frontier/scenes/main.json games/frontier/vitric.json
git commit -m "feat(frontier): add POI system with 3 wild points of interest"
git push origin main
```

## Self-Review Checklist (before reporting DONE)

- [ ] All randomness uses `ctx.random()` — no `Math.random()` anywhere in poi.js (grep to confirm).
- [ ] No use of fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`).
- [ ] POI coordinates in gen_scene.py do not overlap any existing tile occupant set.
- [ ] `vitric check games/frontier` exits 0.
- [ ] Commit pushed to origin/main.

## Report Contract

Write your full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-3-report.md` containing:
1. What was implemented (files + 1-line summary each)
2. Verification output (`vitric check` exit code + last 10 lines)
3. Commit SHA(s) pushed
4. Any concerns or deviations from the brief

Return in your final message: STATUS (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit SHA(s), one-line test summary, and any concerns.
