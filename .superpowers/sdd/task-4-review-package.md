# Task 4 Review Package

## Commit range

BASE = 4c357e0 (Task 3 complete)
HEAD = e01d4c0 (Task 4)

## Commits

```
e01d4c0 feat(frontier): add companion wish system + toast-show/mood-drop listeners
```

## Diff stat

```
 games/frontier/rules/wish.json      | 83 ++++++++++++++++++++++++++++++++++++
 games/frontier/scenes/main.json     |  2 +-
 games/frontier/scripts/companion.js | 29 +++++++++++++
 games/frontier/scripts/wish.js      | 84 +++++++++++++++++++++++++++++++++++++
 games/frontier/vitric.json          |  6 ++-
 5 files changed, 201 insertions(+), 3 deletions(-)
```

## Full diff (with 10 lines of context)

```diff
diff --git a/games/frontier/rules/wish.json b/games/frontier/rules/wish.json
new file mode 100644
index 0000000..6e604c8
--- /dev/null
+++ b/games/frontier/rules/wish.json
@@ -0,0 +1,83 @@
+{
+  "rules": [
+    {
+      "id": "wish-advance-build",
+      "comment": "Any structure built -> advance all companions' 'build' wish by 1.",
+      "on": { "event": "built" },
+      "do": [ { "call": "advance_wish", "with": { "kind": "build", "n": 1 } } ]
+    },
+    {
+      "id": "wish-advance-build-lamp",
+      "comment": "Lamp built -> also advance 'build-lamp' wish by 1.",
+      "on": { "event": "built", "filter": { "kind": "lamp" } },
+      "do": [ { "call": "advance_wish", "with": { "kind": "build-lamp", "n": 1 } } ]
+    },
+    {
+      "id": "wish-advance-harvest",
+      "comment": "Wheat harvested -> advance 'harvest' wish by 1.",
+      "on": { "event": "harvested", "filter": { "id": "wheat" } },
+      "do": [ { "call": "advance_wish", "with": { "kind": "harvest", "n": 1 } } ]
+    },
+    {
+      "id": "wish-advance-harvest-wheat",
+      "comment": "Wheat harvested -> advance 'harvest-wheat' wish by event.n.",
+      "on": { "event": "harvested", "filter": { "id": "wheat" } },
+      "do": [ { "call": "advance_wish", "with": { "kind": "harvest-wheat", "n": "event.n" } } ]
+    },
+    {
+      "id": "wish-advance-gather-ore",
+      "comment": "Ore gathered -> advance 'gather-ore' wish by event.n.",
+      "on": { "event": "gathered", "filter": { "id": "ore" } },
+      "do": [ { "call": "advance_wish", "with": { "kind": "gather-ore", "n": "event.n" } } ]
+    },
+    {
+      "id": "wish-advance-poi",
+      "comment": "Player entered a POI -> advance 'enter-poi' wish by 1.",
+      "on": { "event": "entered-poi" },
+      "do": [ { "call": "advance_wish", "with": { "kind": "enter-poi", "n": 1 } } ]
+    },
+    {
+      "id": "wish-advance-upgrade",
+      "comment": "Structure upgraded (Task 7 emits this) -> advance 'upgrade' wish by 1.",
+      "on": { "event": "upgrade-structure" },
+      "do": [ { "call": "advance_wish", "with": { "kind": "upgrade", "n": 1 } } ]
+    },
+    {
+      "id": "wish-advance-food-high",
+      "comment": "Colony food >= 80 (emitted by wish_food_check system) -> advance 'food-high' wish by 80.",
+      "on": { "event": "food-high" },
+      "do": [ { "call": "advance_wish", "with": { "kind": "food-high", "n": 80 } } ]
+    },
+    {
+      "id": "wish-advance-see-dawn",
+      "comment": "Dawn breaks AND player is in wild zone (x>=16) -> advance 'see-dawn' wish by 1.",
+      "on": { "event": "dawn-break" },
+      "if": [ ["@player.Position.x", ">=", 16] ],
+      "do": [ { "call": "advance_wish", "with": { "kind": "see-dawn", "n": 1 } } ]
+    },
+    {
+      "id": "wish-fulfilled-toast",
+      "comment": "Wish fulfilled -> toast notification.",
+      "on": { "event": "wish-fulfilled" },
+      "do": [
+        { "set": "@toast_lbl.UiLabel.content", "to": { "format": "{} 心愿达成: {}", "args": ["event.companion", "event.wish_desc"] } },
+        { "set": "@toast_lbl.Toast.timer", "to": 3.0 }
+      ]
+    },
+    {
+      "id": "toast-show-generic",
+      "comment": "Generic toast-show listener (resolves Task 3 F2): any script can emit toast-show{text} and this lands it on the toast label.",
+      "on": { "event": "toast-show" },
+      "do": [
+        { "set": "@toast_lbl.UiLabel.content", "to": "event.text" },
+        { "set": "@toast_lbl.Toast.timer", "to": 2.5 }
+      ]
+    },
+    {
+      "id": "companion-mood-drop-apply",
+      "comment": "companion-mood-drop event (resolves Task 3 F3): decrement all companions' Need.comfort by event.amount. Uses a fn because rules can't iterate companion_handles.",
+      "on": { "event": "companion-mood-drop" },
+      "do": [ { "call": "apply_mood_drop", "with": { "amount": "event.amount" } } ]
+    }
+  ]
+}
diff --git a/games/frontier/scripts/companion.js b/games/frontier/scripts/companion.js
index 555108b..4955ff0 100644
--- a/games/frontier/scripts/companion.js
+++ b/games/frontier/scripts/companion.js
@@ -14,20 +14,48 @@
 //
 // Data flow: cache-player-pos writes Colony.player_x/y → snapshot systems pack
 // Drifter/Companion data into JSON and write to Colony → target-* / companion-shelter / talk-reply-apply-* only read Colony.
 // All cross-system data lives in Colony fields; no module-level `let __` shared variables.
 
 const WANDER_SPEED = 1.2;   // wander speed
 const WANDER_RADIUS = 2.5;  // around home_x/y ± radius
 const COMP_DAY_SEC = 60.0;
 const COMP_TICK_PER_SEC = 60;
 
+// Wish templates per archetype family. Each companion gets 3 wishes based on their archetype.
+// items is stored as JSON text in Wish.items (schema doesn't support nested list-of-struct).
+const WISH_TEMPLATES = {
+  builder: [
+    { desc: "建造 3 个结构", kind: "build", target: 3, progress: 0, done: false },
+    { desc: "建一盏灯",     kind: "build-lamp", target: 1, progress: 0, done: false },
+    { desc: "升级 1 个结构", kind: "upgrade", target: 1, progress: 0, done: false },
+  ],
+  farmer: [
+    { desc: "种出 2 茬作物",     kind: "harvest", target: 2, progress: 0, done: false },
+    { desc: "收获 8 单位麦子",   kind: "harvest-wheat", target: 8, progress: 0, done: false },
+    { desc: "吃饱一次(食≥80)",   kind: "food-high", target: 80, progress: 0, done: false },
+  ],
+  explorer: [
+    { desc: "探索 3 处野外地点", kind: "enter-poi", target: 3, progress: 0, done: false },
+    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
+    { desc: "看一次日出(凌晨出门)", kind: "see-dawn", target: 1, progress: 0, done: false },
+  ],
+};
+// Map Chinese archetype strings to template keys via keyword match.
+function wishesForArchetype(archetype) {
+  const a = archetype || "";
+  let key = "explorer"; // default
+  if (/技|电|匠|build|builder/i.test(a)) key = "builder";
+  else if (/厨|医|农|farm|farmer/i.test(a)) key = "farmer";
+  return WISH_TEMPLATES[key] || WISH_TEMPLATES.explorer;
+}
+
 function compTodOf(tick) {
   const secOfDay = (tick / COMP_TICK_PER_SEC) % COMP_DAY_SEC;
   const frac = secOfDay / COMP_DAY_SEC;
   if (frac < 0.25) return "晨";
   if (frac < 0.50) return "午";
   if (frac < 0.75) return "昏";
   return "夜";
 }
 
 // Pack an entity snapshot array into a JSON string (for Colony.*_snapshot).
@@ -312,20 +340,21 @@ vitric.fn("consumeDrifter", (args, ctx) => {
     Need: { comfort: 60, quarters: 0, leave_timer: 0, voiced: 0, comfort_i: 60,
             affinity: 25, affinity_i: 25,
             talked_today: 0, gifted_today: 0,
             last_interact_day: 0, contribution_timer: 0 },
     Wander: { home_x: sx, home_y: sy, tx: sx, ty: sy, timer: 2 },
     Position: { x: sx, y: sy },
     Velocity: { x: 0, y: 0 },
     Sprite: { w: 0.9, h: 0.9, color: personaColor(name) },
     Text: { content: "", size: 0.7, color: "#ffe9b0" },
     Census: { count: 0, is_hub: 0 },
+    Wish: { items: JSON.stringify(wishesForArchetype(args.archetype)), fulfilled: 0 },
   });
   ctx.emit("companion-moved-in", { name: persona.name });
 });
 
 // ---- Drifter arrival: triggered by the day-start event every game day; pick a persona by index ----
 // Fixed persona pool (deterministic → replay/recording consistent). Each spawn is unnamed (avoids collision with existing @drifter).
 // Each persona carries "preferred items" (preferred, comma-separated) → used by the iter2 gift system: matching gift +12 affinity, wrong +3 affinity.
 // 6 differentiated personas: distinct name/archetype/personality/speech style/preferred, not a single cookie-cutter template.
 const DRIFTER_POOL = [
   { name: "Mira",  archetype: "山地步兵",   traits: "坚忍,寡言,脚步轻",           speech: "简短、爱用省略号",     preferred: "fiber,wood" },
diff --git a/games/frontier/scripts/wish.js b/games/frontier/scripts/wish.js
new file mode 100644
index 0000000..f279ff4
--- /dev/null
+++ b/games/frontier/scripts/wish.js
@@ -0,0 +1,84 @@
+// Companion wish system: each companion has 3 archetype-based wishes.
+// Wishes advance via gameplay events (built/harvested/gathered/entered-poi/upgrade/food-high/see-dawn).
+// Fulfilling a wish: +30 affinity, Colony.companion_wish_count++, emit wish-fulfilled.
+// Task 5 wires wish-fulfilled to LLM memory dialogue; this task only advances + emits.
+
+// Wish advancement is a fn (not a system) because it's triggered by discrete gameplay events
+// caught by rules/wish.json. The fn reads Colony.companion_handles (maintained by the
+// companion-register system in companion.js), iterates each companion, reads/advances Wish.items.
+
+const AFFINITY_GAIN_PER_WISH = 30;
+
+// Advance all companions' wishes of `kind` by `amount`. Called by rules/wish.json on gameplay events.
+vitric.fn("advance_wish", (a, ctx) => {
+  const kind = a.kind || "";
+  const amount = (a.n | 0) || 0;
+  if (!kind || amount <= 0) return;
+
+  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
+  for (const h of handles) {
+    if (!h) continue;
+    const raw = ctx.getField(h, "Wish.items") || "[]";
+    let items;
+    try { items = JSON.parse(raw); } catch (_) { continue; }
+    if (!Array.isArray(items)) continue;
+
+    let changed = false;
+    for (const it of items) {
+      if (!it || it.done) continue;
+      if (it.kind !== kind) continue;
+      it.progress = (it.progress || 0) + amount;
+      if (it.progress >= (it.target || 1)) {
+        it.done = true;
+        // Bump fulfilled counter on this entity.
+        const fulfilled = ctx.getField(h, "Wish.fulfilled") | 0;
+        ctx.setField(h, "Wish.fulfilled", fulfilled + 1);
+        // Boost affinity (cap 100).
+        const aff = ctx.getField(h, "Need.affinity");
+        const affNum = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
+        const newAff = Math.min(100, affNum + AFFINITY_GAIN_PER_WISH);
+        ctx.setField(h, "Need.affinity", newAff);
+        ctx.setField(h, "Need.affinity_i", Math.round(newAff));
+        // Sync aggregate counter to Colony (quest gating in Task 6 reads this).
+        const cnt = ctx.getField("colony", "Colony.companion_wish_count") | 0;
+        ctx.setField("colony", "Colony.companion_wish_count", cnt + 1);
+        // Emit for Task 5 (LLM memory dialogue) + toast.
+        const name = ctx.getField(h, "Persona.name") || "伙伴";
+        ctx.emit("wish-fulfilled", { companion: name, wish_desc: it.desc || kind, entity: h });
+      }
+      changed = true;
+    }
+    if (changed) ctx.setField(h, "Wish.items", JSON.stringify(items));
+  }
+});
+
+// Apply a mood-drop penalty to all companions (e.g. cave-injury from POI).
+// Rules can't iterate companion_handles, so this fn does it.
+vitric.fn("apply_mood_drop", (a, ctx) => {
+  const amount = (a.amount | 0) || 0;
+  if (amount <= 0) return;
+  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
+  for (const h of handles) {
+    if (!h) continue;
+    const cur = ctx.getField(h, "Need.comfort");
+    const curNum = (typeof cur === "number" && !isNaN(cur)) ? cur : 50;
+    const next = Math.max(0, curNum - amount);
+    ctx.setField(h, "Need.comfort", next);
+    ctx.setField(h, "Need.comfort_i", Math.round(next));
+  }
+});
+
+// Food-high wish: emit a `food-high` event once per day when Colony.food >= 80.
+// A rule in wish.json catches it and calls advance_wish. Guarded by Colony._wish_food_day
+// to fire at most once per day (avoids spamming every tick while food stays high).
+vitric.system("wish_food_check", { query: ["Colony"], writes: [] }, (entities, ctx) => {
+  const c = entities[0];
+  if (!c) return;
+  const food = c.Colony.food || 0;
+  const day = c.Colony.day || 1;
+  const lastDay = ctx.getField("colony", "Colony._wish_food_day") | 0;
+  if (food >= 80 && day !== lastDay) {
+    ctx.setField("colony", "Colony._wish_food_day", day);
+    ctx.emit("food-high", { food: food });
+  }
+});
diff --git a/games/frontier/vitric.json b/games/frontier/vitric.json
index b4b9eee..1362ca2 100644
--- a/games/frontier/vitric.json
+++ b/games/frontier/vitric.json
@@ -13,32 +13,34 @@
     "rules/colony.json",
     "rules/hud.json",
     "rules/quest.json",
     "rules/farm.json",
     "rules/companion.json",
     "rules/time.json",
     "rules/narrative.json",
     "rules/toast.json",
     "rules/flare.json",
     "rules/poi.json",
-    "rules/affordability.json"
+    "rules/affordability.json",
+    "rules/wish.json"
   ],
   "scripts": [
     "scripts/colony.js",
     "scripts/economy.js",
     "scripts/crops.js",
     "scripts/companion.js",
     "scripts/clock.js",
     "scripts/hud.js",
     "scripts/toast.js",
     "scripts/flare.js",
-    "scripts/poi.js"
+    "scripts/poi.js",
+    "scripts/wish.js"
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

scenes/main.json diff not shown inline (compact single-line JSON; +Wish component on companion entity).
Verification: main.json still has 424 entities (no entities added/removed; only the companion entity gained a Wish component).
