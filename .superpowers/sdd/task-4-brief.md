# Task 4: Wish System + Toast/Mood Listeners

## Context

Vitric frontier deepening, Task 4 of 10. Adds the **companion wish system**: each companion gets 3 archetype-based wishes (build/harvest/explore goals). Wishes advance via gameplay events; fulfilling a wish boosts affinity +30 and emits `wish-fulfilled` (which Task 5 wires to LLM memory dialogue).

Also resolves two Task 3 deferred Minor findings:
- **F2**: `toast-show` event (emitted by poi.js) has no listener — add generic toast rule.
- **F3**: `companion-mood-drop` event (emitted by poi.js on cave-injury) has no listener — add mood-drop rule.

Tasks 1-3 are committed on `main` (HEAD = `4c357e0`).

## Files

- **Create:** `games/frontier/scripts/wish.js` — `advance_wish` fn + `wish_food_check` system
- **Create:** `games/frontier/rules/wish.json` — wish-advancement rules + toast-show + companion-mood-drop listeners
- **Modify:** `games/frontier/scripts/companion.js` — add `Wish` component to `consumeDrifter` spawn
- **Modify:** `games/frontier/scenes/main.json` — add `Wish` component to initial `companion` entity (Pip)
- **Modify:** `games/frontier/vitric.json` — register wish.js + wish.json

## Real API (confirmed from companion.js, economy.js, flare.js, poi.js)

```javascript
// System: runs each tick. `entities` = array with ALL queried components.
vitric.system("name", { query: ["Comp"], writes: ["Comp"] }, (entities, ctx) => {...});

// Named function: callable from rules via {"call": "name", "with": {...}}.
// `a` = merged `with` object. `ctx` has: dt, random(), emit(name, data),
//   setField(handle, "Comp.field", value), getField(handle, "Comp.field"),
//   spawn(comps), despawn(handle), ask("llm", prompt, callbackFn), tick.
vitric.fn("name", (a, ctx) => {...});

// Cross-entity reads/writes (companion.js uses these extensively):
ctx.getField("colony", "Colony.companion_handles");  // returns list of handles
ctx.getField(handle, "Wish.items");                   // returns string (JSON)
ctx.setField(handle, "Wish.items", newJsonString);
ctx.setField("colony", "Colony.companion_wish_count", newVal);
```

**DO NOT USE** (fake APIs): `ctx.singleton()`, `ctx.each()`, `vitric.on()`, `vitric.expose()`, `vitric.call()`, `ctx.entity()`, `ctx.llm()`, `Math.random()`.

**Use `ctx.random()` for ALL randomness.**

## Step 1: Create `games/frontier/scripts/wish.js`

```javascript
// Companion wish system: each companion has 3 archetype-based wishes.
// Wishes advance via gameplay events (built/harvested/gathered/entered-poi/upgrade/food-high/see-dawn).
// Fulfilling a wish: +30 affinity, Colony.companion_wish_count++, emit wish-fulfilled.
// Task 5 wires wish-fulfilled to LLM memory dialogue; this task only advances + emits.

// Wish advancement is a fn (not a system) because it's triggered by discrete gameplay events
// caught by rules/wish.json. The fn reads Colony.companion_handles (maintained by the
// companion-register system in companion.js), iterates each companion, reads/advances Wish.items.

const AFFINITY_GAIN_PER_WISH = 30;

// Advance all companions' wishes of `kind` by `amount`. Called by rules/wish.json on gameplay events.
vitric.fn("advance_wish", (a, ctx) => {
  const kind = a.kind || "";
  const amount = (a.n | 0) || 0;
  if (!kind || amount <= 0) return;

  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  for (const h of handles) {
    if (!h) continue;
    const raw = ctx.getField(h, "Wish.items") || "[]";
    let items;
    try { items = JSON.parse(raw); } catch (_) { continue; }
    if (!Array.isArray(items)) continue;

    let changed = false;
    for (const it of items) {
      if (!it || it.done) continue;
      if (it.kind !== kind) continue;
      it.progress = (it.progress || 0) + amount;
      if (it.progress >= (it.target || 1)) {
        it.done = true;
        // Bump fulfilled counter on this entity.
        const fulfilled = ctx.getField(h, "Wish.fulfilled") | 0;
        ctx.setField(h, "Wish.fulfilled", fulfilled + 1);
        // Boost affinity (cap 100).
        const aff = ctx.getField(h, "Need.affinity");
        const affNum = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
        const newAff = Math.min(100, affNum + AFFINITY_GAIN_PER_WISH);
        ctx.setField(h, "Need.affinity", newAff);
        ctx.setField(h, "Need.affinity_i", Math.round(newAff));
        // Sync aggregate counter to Colony (quest gating in Task 6 reads this).
        const cnt = ctx.getField("colony", "Colony.companion_wish_count") | 0;
        ctx.setField("colony", "Colony.companion_wish_count", cnt + 1);
        // Emit for Task 5 (LLM memory dialogue) + toast.
        const name = ctx.getField(h, "Persona.name") || "伙伴";
        ctx.emit("wish-fulfilled", { companion: name, wish_desc: it.desc || kind, entity: h });
      }
      changed = true;
    }
    if (changed) ctx.setField(h, "Wish.items", JSON.stringify(items));
  }
});

// Food-high wish: emit a `food-high` event once per day when Colony.food >= 80.
// A rule in wish.json catches it and calls advance_wish. Guarded by Colony._wish_food_day
// to fire at most once per day (avoids spamming every tick while food stays high).
vitric.system("wish_food_check", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const food = c.Colony.food || 0;
  const day = c.Colony.day || 1;
  const lastDay = ctx.getField("colony", "Colony._wish_food_day") | 0;
  if (food >= 80 && day !== lastDay) {
    ctx.setField("colony", "Colony._wish_food_day", day);
    ctx.emit("food-high", { food: food });
  }
});
```

**Note:** `Colony._wish_food_day` is a new ad-hoc field. The schema does not declare it, but the engine allows runtime fields on entities (companion.js uses `_day_anchor` similarly). If `vitric check` rejects undeclared fields, add `_wish_food_day` to the Colony component in schema.json with `{"type": "int", "default": 0}`. Check first; only add if needed.

## Step 2: Create `games/frontier/rules/wish.json`

```json
{
  "rules": [
    {
      "id": "wish-advance-build",
      "comment": "Any structure built -> advance all companions' 'build' wish by 1.",
      "on": { "event": "built" },
      "do": [ { "call": "advance_wish", "with": { "kind": "build", "n": 1 } } ]
    },
    {
      "id": "wish-advance-build-lamp",
      "comment": "Lamp built -> also advance 'build-lamp' wish by 1.",
      "on": { "event": "built", "filter": { "kind": "lamp" } },
      "do": [ { "call": "advance_wish", "with": { "kind": "build-lamp", "n": 1 } } ]
    },
    {
      "id": "wish-advance-harvest",
      "comment": "Wheat harvested -> advance 'harvest' wish by 1.",
      "on": { "event": "harvested", "filter": { "id": "wheat" } },
      "do": [ { "call": "advance_wish", "with": { "kind": "harvest", "n": 1 } } ]
    },
    {
      "id": "wish-advance-harvest-wheat",
      "comment": "Wheat harvested -> advance 'harvest-wheat' wish by event.n.",
      "on": { "event": "harvested", "filter": { "id": "wheat" } },
      "do": [ { "call": "advance_wish", "with": { "kind": "harvest-wheat", "n": "event.n" } } ]
    },
    {
      "id": "wish-advance-gather-ore",
      "comment": "Ore gathered -> advance 'gather-ore' wish by event.n.",
      "on": { "event": "gathered", "filter": { "id": "ore" } },
      "do": [ { "call": "advance_wish", "with": { "kind": "gather-ore", "n": "event.n" } } ]
    },
    {
      "id": "wish-advance-poi",
      "comment": "Player entered a POI -> advance 'enter-poi' wish by 1.",
      "on": { "event": "entered-poi" },
      "do": [ { "call": "advance_wish", "with": { "kind": "enter-poi", "n": 1 } } ]
    },
    {
      "id": "wish-advance-upgrade",
      "comment": "Structure upgraded (Task 7 emits this) -> advance 'upgrade' wish by 1.",
      "on": { "event": "upgrade-structure" },
      "do": [ { "call": "advance_wish", "with": { "kind": "upgrade", "n": 1 } } ]
    },
    {
      "id": "wish-advance-food-high",
      "comment": "Colony food >= 80 (emitted by wish_food_check system) -> advance 'food-high' wish by 80.",
      "on": { "event": "food-high" },
      "do": [ { "call": "advance_wish", "with": { "kind": "food-high", "n": 80 } } ]
    },
    {
      "id": "wish-advance-see-dawn",
      "comment": "Dawn breaks AND player is in wild zone (x>=16) -> advance 'see-dawn' wish by 1.",
      "on": { "event": "dawn-break" },
      "if": [ ["@player.Position.x", ">=", 16] ],
      "do": [ { "call": "advance_wish", "with": { "kind": "see-dawn", "n": 1 } } ]
    },
    {
      "id": "wish-fulfilled-toast",
      "comment": "Wish fulfilled -> toast notification.",
      "on": { "event": "wish-fulfilled" },
      "do": [
        { "set": "@toast_lbl.UiLabel.content", "to": { "format": "{} 心愿达成: {}", "args": ["event.companion", "event.wish_desc"] } },
        { "set": "@toast_lbl.Toast.timer", "to": 3.0 }
      ]
    },
    {
      "id": "toast-show-generic",
      "comment": "Generic toast-show listener (resolves Task 3 F2): any script can emit toast-show{text} and this lands it on the toast label.",
      "on": { "event": "toast-show" },
      "do": [
        { "set": "@toast_lbl.UiLabel.content", "to": "event.text" },
        { "set": "@toast_lbl.Toast.timer", "to": 2.5 }
      ]
    },
    {
      "id": "companion-mood-drop-apply",
      "comment": "companion-mood-drop event (resolves Task 3 F3): decrement all companions' Need.comfort by event.amount. Uses a fn because rules can't iterate companion_handles.",
      "on": { "event": "companion-mood-drop" },
      "do": [ { "call": "apply_mood_drop", "with": { "amount": "event.amount" } } ]
    }
  ]
}
```

**Note:** `apply_mood_drop` fn needs to be added to `wish.js` (Step 1 update). Add this to wish.js after `advance_wish`:

```javascript
// Apply a mood-drop penalty to all companions (e.g. cave-injury from POI).
// Rules can't iterate companion_handles, so this fn does it.
vitric.fn("apply_mood_drop", (a, ctx) => {
  const amount = (a.amount | 0) || 0;
  if (amount <= 0) return;
  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  for (const h of handles) {
    if (!h) continue;
    const cur = ctx.getField(h, "Need.comfort");
    const curNum = (typeof cur === "number" && !isNaN(cur)) ? cur : 50;
    const next = Math.max(0, curNum - amount);
    ctx.setField(h, "Need.comfort", next);
    ctx.setField(h, "Need.comfort_i", Math.round(next));
  }
});
```

## Step 3: Modify `games/frontier/scripts/companion.js`

In `consumeDrifter` (around line 295-324), add `Wish` to the `ctx.spawn({...})` component object. Also add the wish-template helper at the top of the file (after the existing constants).

### 3a. Add wish template helper near the top of companion.js (after `const COMP_TICK_PER_SEC = 60;` around line 22):

```javascript
// Wish templates per archetype family. Each companion gets 3 wishes based on their archetype.
// items is stored as JSON text in Wish.items (schema doesn't support nested list-of-struct).
const WISH_TEMPLATES = {
  builder: [
    { desc: "建造 3 个结构", kind: "build", target: 3, progress: 0, done: false },
    { desc: "建一盏灯",     kind: "build-lamp", target: 1, progress: 0, done: false },
    { desc: "升级 1 个结构", kind: "upgrade", target: 1, progress: 0, done: false },
  ],
  farmer: [
    { desc: "种出 2 茬作物",     kind: "harvest", target: 2, progress: 0, done: false },
    { desc: "收获 8 单位麦子",   kind: "harvest-wheat", target: 8, progress: 0, done: false },
    { desc: "吃饱一次(食≥80)",   kind: "food-high", target: 80, progress: 0, done: false },
  ],
  explorer: [
    { desc: "探索 3 处野外地点", kind: "enter-poi", target: 3, progress: 0, done: false },
    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
    { desc: "看一次日出(凌晨出门)", kind: "see-dawn", target: 1, progress: 0, done: false },
  ],
};
// Map Chinese archetype strings to template keys via keyword match.
function wishesForArchetype(archetype) {
  const a = archetype || "";
  let key = "explorer"; // default
  if (/技|电|匠|build|builder/i.test(a)) key = "builder";
  else if (/厨|医|农|farm|farmer/i.test(a)) key = "farmer";
  return WISH_TEMPLATES[key] || WISH_TEMPLATES.explorer;
}
```

### 3b. In `consumeDrifter`'s `ctx.spawn({...})` call (around line 307-322), add the `Wish` component:

Add this line inside the spawn component object (e.g., after `Census: { count: 0, is_hub: 0 },`):

```javascript
    Wish: { items: JSON.stringify(wishesForArchetype(args.archetype)), fulfilled: 0 },
```

The resulting spawn object should look like:
```javascript
  ctx.spawn({
    Companion: {},
    Persona: persona,
    Mood: { value: "平静" },
    ThinkReq: { pending: 0 },
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
    Wish: { items: JSON.stringify(wishesForArchetype(args.archetype)), fulfilled: 0 },
  });
```

## Step 4: Modify `games/frontier/scenes/main.json` — add Wish to initial companion entity

The initial `companion` entity (Pip, archetype "话痨技工") is in main.json. It needs a `Wish` component with builder-template wishes (since "话痨技工" matches the builder regex `/技/`).

Read `games/frontier/scenes/main.json`, find the entity with `"name": "companion"`, and add `"Wish": {"items": "[{\"desc\":\"建造 3 个结构\",\"kind\":\"build\",\"target\":3,\"progress\":0,\"done\":false},{\"desc\":\"建一盏灯\",\"kind\":\"build-lamp\",\"target\":1,\"progress\":0,\"done\":false},{\"desc\":\"升级 1 个结构\",\"kind\":\"upgrade\",\"target\":1,\"progress\":0,\"done\":false}]", "fulfilled": 0}` to its `components` object.

The JSON string value for `items` (properly escaped for embedding in JSON) is:
```json
"items": "[{\"desc\":\"建造 3 个结构\",\"kind\":\"build\",\"target\":3,\"progress\":0,\"done\":false},{\"desc\":\"建一盏灯\",\"kind\":\"build-lamp\",\"target\":1,\"progress\":0,\"done\":false},{\"desc\":\"升级 1 个结构\",\"kind\":\"upgrade\",\"target\":1,\"progress\":0,\"done\":false}]"
```

**Important:** main.json is currently a single-line compact JSON file. Edit it carefully — use a Python script to load, modify, and re-dump to avoid manual escaping errors. Example:

```python
import json
path = "games/frontier/scenes/main.json"
with open(path) as f:
    scene = json.load(f)
wishes = [
    {"desc": "建造 3 个结构", "kind": "build", "target": 3, "progress": 0, "done": False},
    {"desc": "建一盏灯", "kind": "build-lamp", "target": 1, "progress": 0, "done": False},
    {"desc": "升级 1 个结构", "kind": "upgrade", "target": 1, "progress": 0, "done": False},
]
for e in scene["entities"]:
    if e.get("name") == "companion":
        e["components"]["Wish"] = {"items": json.dumps(wishes, ensure_ascii=False), "fulfilled": 0}
        break
with open(path, "w") as f:
    json.dump(scene, f, ensure_ascii=False, separators=(",", ":"))
```

Run this script from the repo root. Verify the companion entity now has a Wish component and the total entity count is still 424 (this edit doesn't add/remove entities, just adds a component to one).

## Step 5: Register in `games/frontier/vitric.json`

- Add `"rules/wish.json"` to the `rules` array (after `"rules/poi.json"`).
- Add `"scripts/wish.js"` to the `scripts` array (after `"scripts/poi.js"`).

## Step 6: Handle `Colony._wish_food_day` field

Run `cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -20` first. If it passes (exit 0), the engine allows runtime fields and no schema change is needed. If it fails with an error about `_wish_food_day` not existing on Colony, add this field to the Colony component in `games/frontier/schema.json` (after `companion_wish_count`):

```json
        "_wish_food_day": {
          "type": "int",
          "default": 0
        }
```

Re-run `vitric check` after any schema change.

## Step 7: Verify

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -20`
Expected: exit code 0.

If check fails, read the error output and fix. Common issues:
- Script syntax error in wish.js
- Rule references unknown event/component
- Colony._wish_food_day undeclared (fix per Step 6)
- main.json malformed JSON (re-do Step 4 with the Python script)

## Step 8: Commit + push

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/wish.js games/frontier/rules/wish.json games/frontier/scripts/companion.js games/frontier/scenes/main.json games/frontier/vitric.json
# Also add schema.json if Step 6 required the _wish_food_day field
git add games/frontier/schema.json  # only if modified
git commit -m "feat(frontier): add companion wish system + toast-show/mood-drop listeners"
git push origin main
```

## Self-Review Checklist

- [ ] All randomness uses `ctx.random()` — no `Math.random()` in wish.js (grep to confirm; there should be none, wish advancement is deterministic).
- [ ] No fake APIs in wish.js (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`).
- [ ] All comments in wish.js and companion.js additions are English.
- [ ] String literals (wish descriptions, toast text) stay Chinese.
- [ ] `consumeDrifter` spawn includes `Wish` component.
- [ ] Initial `companion` entity in main.json has `Wish` component with builder template (archetype "话痨技工" → builder).
- [ ] `wish.json` has rules for: build, build-lamp, harvest, harvest-wheat, gather-ore, enter-poi, upgrade, food-high, see-dawn, wish-fulfilled-toast, toast-show-generic, companion-mood-drop-apply.
- [ ] `vitric check games/frontier` exits 0.
- [ ] Commit pushed to origin/main.
- [ ] main.json still has 424 entities (verify with `python3 -c "import json; print(len(json.load(open('games/frontier/scenes/main.json'))['entities']))"`).

## Report Contract

Write your full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-4-report.md` containing:
1. What was implemented (files + 1-line summary each)
2. Verification output (`vitric check` exit code + last 10 lines)
3. Commit SHA(s) pushed
4. Confirmation that main.json still has 424 entities
5. Whether `Colony._wish_food_day` needed a schema change (Step 6)
6. Any concerns or deviations from the brief

Return in your final message: STATUS (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit SHA(s), one-line test summary, and any concerns.
