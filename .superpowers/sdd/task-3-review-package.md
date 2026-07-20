# Task 3 Review Package

## Commit range

BASE = 300cf73 (Task 2 complete)
HEAD = 4c357e0 (Task 3 + fix)

## Commits

```
4c357e0 fix(frontier): restore UI entities dropped by scene regeneration, keep 3 POIs
0687c6a feat(frontier): add POI system with 3 wild points of interest
```

## Diff stat

```
 games/frontier/rules/poi.json     | 16 +++++++
 games/frontier/scenes/main.json   |  2 +-
 games/frontier/scripts/poi.js     | 87 +++++++++++++++++++++++++++++++++++++++
 games/frontier/tools/gen_scene.py | 23 +++++++++++
 games/frontier/vitric.json        |  4 +-
 5 files changed, 130 insertions(+), 2 deletions(-)
```

## Full diff (with 10 lines of context)

```diff
diff --git a/games/frontier/rules/poi.json b/games/frontier/rules/poi.json
new file mode 100644
index 0000000..82cb6fc
--- /dev/null
+++ b/games/frontier/rules/poi.json
@@ -0,0 +1,16 @@
+{
+  "rules": [
+    {
+      "id": "poi-interact-click",
+      "comment": "Interact mode + left-click on a POI -> call interact_poi (rolls rewards, marks looted, emits inv-set + entered-poi). Inventory is passed in full; the existing inv-apply rule in economy.json writes it back.",
+      "on": { "event": "mouse" },
+      "if": [ ["@uistate.Mode.value", "==", "interact"] ],
+      "do": [ { "call": "interact_poi", "with": {
+        "entity": "event.entity", "comp": "event.comp", "x": "event.x", "y": "event.y",
+        "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
+        "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
+        "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp"
+      } } ]
+    }
+  ]
+}
diff --git a/games/frontier/scripts/poi.js b/games/frontier/scripts/poi.js
new file mode 100644
index 0000000..3b5b0b7
--- /dev/null
+++ b/games/frontier/scripts/poi.js
@@ -0,0 +1,87 @@
+// POI (Point of Interest) system: 3 daily-refreshing wild locations.
+//   Poi component (added in Task 1): kind / state (fresh|looted|depleted) / cooldown / reward_table (JSON text).
+//
+// Two pieces:
+//   1) poi_tick system — decrements cooldown on looted/depleted POIs; when cooldown hits 0, refreshes to fresh.
+//   2) interact_poi fn — called by rules/poi.json when player in interact mode clicks a POI. Rolls rewards
+//      via ctx.random() (NOT Math.random — poisoned in QuickJS), updates inventory through the inv-set
+//      emit pattern (same as economy.js), marks the POI looted, and emits entered-poi for the wish system.
+
+const POI_ITEMS = ["ore", "wheat", "fiber", "plank"];
+const POI_LABELS = { ore: "矿", wheat: "麦", fiber: "纤维", plank: "板" };
+const POI_COOLDOWN_LOOTED = 120;   // 2 min soft cooldown (full refresh also via tick)
+const POI_CAVE_INJURY_CHANCE = 0.3; // cave-entrance: 30% chance of mood-drop injury
+
+// ---- Tick: regrow looted/depleted POIs once their cooldown expires ----
+vitric.system("poi_tick", { query: ["Poi"], writes: ["Poi"] }, (entities, ctx) => {
+  for (const e of entities) {
+    const poi = e.Poi;
+    if (poi.state === "fresh") continue;
+    if ((poi.cooldown | 0) <= 0) {
+      // Already ready to refresh.
+      poi.state = "fresh";
+      poi.cooldown = 0;
+      continue;
+    }
+    poi.cooldown = Math.max(0, poi.cooldown - ctx.dt);
+    if (poi.cooldown <= 0) {
+      poi.state = "fresh";
+      poi.cooldown = 0;
+    }
+  }
+});
+
+// ---- Interact click on a POI: rule passes hit entity handle + components snapshot + current inventory ----
+// Same shape as economy.js `interact`: a.entity (handle), a.comp (components), a.<inventory fields>.
+// Only acts if the hit entity has a Poi component in state "fresh". Rolls rewards, emits inv-set, marks looted.
+vitric.fn("interact_poi", (a, ctx) => {
+  const comp = a.comp || {};
+  const poi = comp.Poi;
+  if (!poi) return;                       // Not a POI hit — ignore.
+  if (poi.state !== "fresh") return;      // Already looted/depleted — ignore.
+
+  // Parse reward table: {item: [lo, hi]}.
+  let rewards = {};
+  try { rewards = JSON.parse(poi.reward_table || "{}"); } catch { return; }
+
+  // Build inventory from args (same pattern as economy.js readInv).
+  const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];
+  const inv = {};
+  for (const k of ITEMS) inv[k] = a[k] | 0;
+
+  // Roll rewards deterministically with ctx.random().
+  let rewardText = "";
+  for (const key of Object.keys(rewards)) {
+    const range = rewards[key];
+    if (!Array.isArray(range) || range.length < 2) continue;
+    const lo = range[0] | 0;
+    const hi = range[1] | 0;
+    const span = Math.max(0, hi - lo);
+    const n = lo + Math.floor(ctx.random() * (span + 1));
+    if (n <= 0) continue;
+    inv[key] = (inv[key] | 0) + n;
+    const label = POI_LABELS[key] || key;
+    rewardText += `${label}+${n} `;
+  }
+
+  // Emit inventory write-back (rule "inv-apply" in economy.json handles it).
+  const d = {};
+  for (const k of ITEMS) d[k] = inv[k];
+  ctx.emit("inv-set", d);
+
+  // Mark POI looted + start cooldown (writes to the clicked entity's Poi component).
+  ctx.setField(a.entity, "Poi.state", "looted");
+  ctx.setField(a.entity, "Poi.cooldown", POI_COOLDOWN_LOOTED);
+
+  // Toast with reward summary.
+  ctx.emit("toast-show", { text: `探索收获: ${rewardText.trim()}` });
+
+  // Cave-entrance risk: 30% chance of companion mood drop (cave-injury).
+  if (poi.kind === "cave-entrance" && ctx.random() < POI_CAVE_INJURY_CHANCE) {
+    ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
+    ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
+  }
+
+  // Notify wish system (Task 4 will add a rule listening for this event).
+  ctx.emit("entered-poi", { kind: poi.kind });
+});
diff --git a/games/frontier/tools/gen_scene.py b/games/frontier/tools/gen_scene.py
index 109f6f5..d3211ba 100644
--- a/games/frontier/tools/gen_scene.py
+++ b/games/frontier/tools/gen_scene.py
@@ -96,20 +96,43 @@ entities.append({"name": "companion", "components": {
 entities.append({"name": "drifter", "components": {
     "Drifter": {"arrival_day": 1},
     "Persona": {"name": "Lio", "archetype": "乐天厨子", "traits": "贪吃,爱张罗,记仇又健忘",
                 "speech": "热络,爱用感叹号"},
     "Mood": {"value": "好奇"}, "ThinkReq": {"pending": 0},
     "Position": {"x": 23, "y": 7}, "Collider": {"w": 0.9, "h": 0.9},
     "Sprite": {"w": 0.9, "h": 0.9, "image": "", "color": "#d4a06a"},
     "Text": {"content": "", "size": 0.7, "color": "#ffe9b0"},
 }})
 
+# ---- 3 POIs in wild zone (daily-refreshing wild points of interest) ----
+POI_SPECS = [
+    # (name, kind, x, y, reward_table) — reward_table: {item: [lo, hi]} rolled on explore.
+    ("poi_camp",  "abandoned-camp", 18, 10, {"ore": [1, 2], "wheat": [2, 4], "fiber": [1, 3]}),
+    ("poi_cave",  "cave-entrance",  23, 2,  {"ore": [3, 5]}),                 # high-risk high-reward
+    ("poi_wreck", "shipwreck",      26, 5,  {"wheat": [3, 5], "plank": [1, 2]}),
+]
+POI_LABELS = {"abandoned-camp": "废弃营地", "cave-entrance": "洞穴入口", "shipwreck": "沉船"}
+POI_COLORS = {"abandoned-camp": "#8b6f47", "cave-entrance": "#5a4a6a", "shipwreck": "#4a5a6a"}
+for name, kind, x, y, rewards in POI_SPECS:
+    entities.append({"name": name, "components": {
+        "Position": {"x": x, "y": y},
+        "Sprite": {"w": 1.6, "h": 1.6, "color": POI_COLORS[kind], "image": ""},
+        "Collider": {"w": 1.6, "h": 1.6},
+        "Poi": {
+            "kind": kind,
+            "state": "fresh",
+            "cooldown": 0,
+            "reward_table": json.dumps(rewards),
+        },
+        "Text": {"content": POI_LABELS[kind], "size": 0.4, "color": "#ffe070", "screen": False},
+    }})
+
 # ---- UI shell ----
 def ui_entity(name, ui, extra=None):
     comps = {"Ui": ui}
     if extra:
         comps.update(extra)
     entities.append({"name": name, "components": comps})
 
 ui_entity("hud_bar", {"anchor": "top-center", "parent": "ui", "oy": 12, "w": 1180, "h": 48},
           {"Panel": {"color": "#161a24"}})
 ui_entity("hud_res", {"anchor": "stretch", "parent": "hud_bar"},
diff --git a/games/frontier/vitric.json b/games/frontier/vitric.json
index cb2309f..b4b9eee 100644
--- a/games/frontier/vitric.json
+++ b/games/frontier/vitric.json
@@ -12,31 +12,33 @@
     "rules/economy.json",
     "rules/colony.json",
     "rules/hud.json",
     "rules/quest.json",
     "rules/farm.json",
     "rules/companion.json",
     "rules/time.json",
     "rules/narrative.json",
     "rules/toast.json",
     "rules/flare.json",
+    "rules/poi.json",
     "rules/affordability.json"
   ],
   "scripts": [
     "scripts/colony.js",
     "scripts/economy.js",
     "scripts/crops.js",
     "scripts/companion.js",
     "scripts/clock.js",
     "scripts/hud.js",
     "scripts/toast.js",
-    "scripts/flare.js"
+    "scripts/flare.js",
+    "scripts/poi.js"
   ],
   "seed": 1,
   "render": {
     "width": 1920,
     "height": 1080,
     "note": "渲染分辨率锁 1920x1080,与引擎 UI_REFERENCE_VIEWPORT 一致,所有 UI 尺寸/字号按此设计"
   },
   "gates": {
     "playthroughs": [
       {
```

## Scene file note

scenes/main.json diff is not shown inline (compact single-line JSON, +3 entity objects).
Verification: main.json has 424 entities (421 from 300cf73 + 3 POIs: poi_camp, poi_cave, poi_wreck).
All previously-referenced UI entities (quest_title_lbl, quest_sub_lbl, quest_card, narration_lbl, intro_panel, ending_panel, hud_companion_lbl) are present.
