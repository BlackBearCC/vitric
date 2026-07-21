# Task 9 Brief — Companions Expansion (6 roles, 12 pool, wish templates)

## Context

Frontier Sandbox Expansion, Task 9 of 16. Base commit: `888057d` (after Task 8 review artifacts). Plan: `docs/superpowers/plans/2026-07-20-frontier-sandbox-expansion.md` §Task 9 (lines 1101-1145). Spec: `docs/superpowers/specs/2026-07-20-frontier-sandbox-expansion-design.md` §4.4 (lines 246-264).

Phase 2 game systems: Task 6 (Seasons/Weather) ✅, Task 7 (Forecast HUD) ✅, Task 8 (Tech Tree) ✅, Task 9 (Companions Expansion) = this task.

## Goal

Expand the companions system: add `Persona.role` enum (6 roles), expand `DRIFTER_POOL` from 6 to 12 entries (2 per role), implement role-driven contribution (replacing the random pick in `companion-contribution`), expand wish templates from 3 to 6 (one per role), add a colony-level collective wish (granary-50 goal), bump `drifters_spawned` cap from 4 to 8, modulate spawn cadence by stage.

## Plan corrections (fictional APIs / scope clarifications)

1. **`test_progression.py` does NOT exist** — write Rust tests in `crates/vitric-cli/tests/companions.rs` following the pattern of `seasons.rs` / `research.rs`.
2. **Role-driven contribution partial implementability**: spec §4.4 lists 6 role contributions, but 3 depend on future tasks:
   - guard's "auto-defends during combat" → Task 10 (combat not implemented yet)
   - trader's "enables trade menu" → Task 11 (trade not implemented yet)
   - explorer's "unlocks swamp region" → Task 12 (swamp region doesn't exist yet)
   
   For Task 9: implement the role-specific RESOURCE bonus for all 6 roles (testable now), AND emit forward-compat hook events for the 3 unimplemented roles (`guard-patrol`, `trade-available`, `explore-bonus`). Tasks 10/11/12 will consume these events.
3. **No new mode**: Task 9 does NOT add a Mode enum variant. Roles are passive (no UI mode switch).
4. **`scenes/main.json` modification**: add `role` to the existing `companion` (Pip) and `drifter` (Lio) entities' Persona components. Add a `collective_wish_lbl` HUD entity. No other scene changes.

## Schema changes (`games/frontier/schema.json`)

### Extend `Persona` component

Add `role` field:

```json
"Persona": {
  "fields": {
    "name": { "type": "text", "default": "旅人" },
    "archetype": { "type": "text", "default": "" },
    "traits": { "type": "text", "default": "" },
    "speech": { "type": "text", "default": "" },
    "preferred": { "type": "text", "default": "" },
    "role": { "type": "enum", "variants": ["builder", "farmer", "explorer", "guard", "trader", "scholar"], "default": "builder" }
  }
}
```

### Extend `Colony` component

Add `collective_wish_done` field (one-time flag for the collective wish):

```json
"collective_wish_done": { "type": "int", "default": 0 }
```

Add to `Colony.fields` alongside the existing `companion_wish_count` / `_wish_food_day` / `last_wish_memory_target` fields.

## Scene changes (`games/frontier/scenes/main.json`)

### Add `role` to existing Persona entities

- `companion` entity (Pip, 话痨技工): `Persona.role = "builder"` (Pip is a builder-style技工).
- `drifter` entity (Lio, 乐天厨子): `Persona.role = "farmer"` (Lio is a cook = food-related).

### Add `collective_wish_lbl` HUD entity

Position: below `research_status_lbl` (which is at oy:308 + h:24 = 332, so oy:336). Use `anchor: "top-left"`, `parent: "ui"`, `ox: 24`, `oy: 336`, `w: 280`, `h: 24`. `UiLabel: { content: "共识: 粮储 50 (未达成)", size: 18, color: "#ffffff", align: "start" }`.

## Script: `games/frontier/scripts/companion.js` (MODIFY)

### Expand DRIFTER_POOL from 6 to 12 entries

Keep the existing 6 entries (re-assign roles based on archetype), add 6 new entries (2 trader + 1 each of guard/scholar/explorer/farmer to reach 2 per role). Each entry gets a `role` field.

Existing 6 (with roles assigned):
```javascript
{ name: "Mira",  archetype: "山地步兵",   traits: "坚忍,寡言,脚步轻",           speech: "简短、爱用省略号",     preferred: "fiber,wood",     role: "guard" },
{ name: "Kade",  archetype: "电工学徒",   traits: "好奇、爱拆东西、胆大",       speech: "语速快、夹英文",       preferred: "ore,plank",      role: "builder" },
{ name: "Sori",  archetype: "老年医师",   traits: "慈祥、念叨、健忘",           speech: "慢热、爱讲从前",       preferred: "lamp,chair",     role: "scholar" },
{ name: "Vex",   archetype: "游戏少年",   traits: "浮躁、爱吹、爱笑",           speech: "夸张、表情符号感",     preferred: "wheat,seed",     role: "explorer" },
{ name: "Nell",  archetype: "沉默匠人",   traits: "内向、手巧、爱干净",         speech: "句子短、偶尔冒冷笑话", preferred: "wood,ore",       role: "builder" },
{ name: "Orin",  archetype: "退役厨师",   traits: "话多、爱美食、嗓门大",       speech: "嗓门大、爱用感叹号",   preferred: "wheat,lamp",     role: "farmer" },
```

New 6 (to reach 2 per role):
```javascript
{ name: "Holt",  archetype: "退役士兵",   traits: "严肃、警觉、责任感强",       speech: "短促、爱用军语",       preferred: "ore,plank",      role: "guard" },
{ name: "Pim",   archetype: "博物学者",   traits: "好奇、博学、爱记录",         speech: "学究气、爱引用",       preferred: "lamp,chair",     role: "scholar" },
{ name: "Dax",   archetype: "徒步旅人",   traits: "机敏、爱冒险、记路",         speech: "简洁、爱用方向词",     preferred: "fiber,wood",     role: "explorer" },
{ name: "Yara",  archetype: "园丁",       traits: "耐心、爱植物、观察入微",     speech: "温柔、爱用比喻",       preferred: "seed,wheat",     role: "farmer" },
{ name: "Rix",   archetype: "商队学徒",   traits: "精明、爱砍价、记帐快",       speech: "快嘴、爱用数字",       preferred: "plank,lamp",     role: "trader" },
{ name: "Lira",  archetype: "游商",       traits: "圆滑、爱讲故事、见多识广",   speech: "热络、爱用感叹",       preferred: "wheat,chair",    role: "trader" },
```

Final: 12 entries, 2 per role × 6 roles. ✓

### Update `consumeDrifter` fn to pass `role` through

In the `persona` object built in `consumeDrifter`, add `role: args.role || "builder"`. The rule that calls `consumeDrifter` (in `rules/companion.json`, rule `companion-invite-process`) reads `event.role` and passes it. The `companion-invited` event is emitted by `inviteAnyNearby` which must now also include `role` from the drifter's Persona.

Update `inviteAnyNearby` fn to read the drifter's Persona.role via `ctx.getField(args.drifter_id, "Persona.role")` and include it in the `companion-invited` event payload.

### Replace random pick in `companion-contribution` with role-based dispatch

The current `companion-contribution` system (query `["Companion", "Need", "Mood", "Position"]`, writes `["Need"]`) does a random pick between (B) +1 resource and (C) +0.5 food_rate. Replace the random pick with a role-based switch:

```javascript
vitric.system("companion-contribution", { query: ["Companion", "Need", "Mood", "Persona", "Position"], writes: ["Need"] }, (entities, ctx) => {
  for (const e of entities) {
    const n = e.Need;
    if ((n.affinity || 0) < CONTRIB_AFFINITY_MIN) continue;
    const mood = (e.Mood && e.Mood.value) || "平静";
    if (mood !== "开心" && mood !== "平静") continue;
    n.contribution_timer = (n.contribution_timer || 0) - ctx.dt;
    if (n.contribution_timer > 0) continue;
    n.contribution_timer = CONTRIB_INTERVAL_SEC + ctx.random() * 4;

    const role = (e.Persona && e.Persona.role) || "builder";
    switch (role) {
      case "builder": {
        const cur = ctx.getField("@player", "Inventory.plank") | 0;
        ctx.setField("@player", "Inventory.plank", cur + 1);
        ctx.emit("companion-contributed", { pid: e.id, kind: "plank", role: "builder" });
        break;
      }
      case "farmer": {
        const fr = ctx.getField("colony", "Colony.food_rate");
        ctx.setField("colony", "Colony.food_rate", (typeof fr === "number" ? fr : 0) + 0.5);
        ctx.emit("companion-boost", { pid: e.id, what: "food", role: "farmer" });
        break;
      }
      case "explorer": {
        const cur = ctx.getField("@player", "Inventory.fiber") | 0;
        ctx.setField("@player", "Inventory.fiber", cur + 1);
        ctx.emit("explore-bonus", { pid: e.id, role: "explorer" }); // forward-compat hook for Task 12
        ctx.emit("companion-contributed", { pid: e.id, kind: "fiber", role: "explorer" });
        break;
      }
      case "guard": {
        const cur = ctx.getField("@player", "Inventory.ore") | 0;
        ctx.setField("@player", "Inventory.ore", cur + 1);
        ctx.emit("guard-patrol", { pid: e.id, role: "guard" }); // forward-compat hook for Task 10
        ctx.emit("companion-contributed", { pid: e.id, kind: "ore", role: "guard" });
        break;
      }
      case "trader": {
        const cur = ctx.getField("@player", "Inventory.wheat") | 0;
        ctx.setField("@player", "Inventory.wheat", cur + 1);
        ctx.emit("trade-available", { pid: e.id, role: "trader" }); // forward-compat hook for Task 11
        ctx.emit("companion-contributed", { pid: e.id, kind: "wheat", role: "trader" });
        break;
      }
      case "scholar": {
        const tp = ctx.getField("@player", "TechPoint.value") | 0;
        ctx.emit("tp-set", { value: tp + 1 });
        ctx.emit("companion-contributed", { pid: e.id, kind: "techpoint", role: "scholar" });
        break;
      }
      default: {
        // Fallback: existing random pick (shouldn't happen — all companions have a role)
        const pick = (ctx.random() * 2) | 0;
        if (pick === 0) {
          const items = ["ore", "wood", "fiber"];
          const which = items[(ctx.random() * items.length) | 0];
          const cur = ctx.getField("@player", "Inventory." + which) | 0;
          ctx.setField("@player", "Inventory." + which, cur + 1);
          ctx.emit("companion-contributed", { pid: e.id, kind: which });
        } else {
          const fr = ctx.getField("colony", "Colony.food_rate");
          ctx.setField("colony", "Colony.food_rate", (typeof fr === "number" ? fr : 0) + 0.5);
          ctx.emit("companion-boost", { pid: e.id, what: "food" });
        }
      }
    }
  }
});
```

**Notes:**
- Query extended to include `"Persona"` so `e.Persona.role` is readable.
- `ctx.getField("@player", "Inventory.plank")` etc. — `@player` is a valid entity-name reference (used elsewhere in the codebase, e.g., economy.js). Verify this works (if not, use `"player"` without `@` — check existing patterns).
- Scholar's `tp-set` emit reuses the Task 8 write-back channel (rule `tp-apply` in research.json applies it to `@player.TechPoint.value`).
- `explore-bonus` / `guard-patrol` / `trade-available` events have no consumers yet — Tasks 10/11/12 will add rules listening for them. They're forward-compat hooks.
- The `default` fallback preserves the old random behavior for safety (shouldn't fire since all companions have a role post-Task-9).

## Script: `games/frontier/scripts/wish.js` (MODIFY)

### Expand WISH_TEMPLATES from 3 to 6

Add 3 new role keys (guard, trader, scholar). Use existing wish kinds (build, harvest, enter-poi, gather-ore, etc.) so no new rules are needed:

```javascript
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
  guard: [
    { desc: "建造 3 个结构",     kind: "build", target: 3, progress: 0, done: false },
    { desc: "升级 1 个结构",     kind: "upgrade", target: 1, progress: 0, done: false },
    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
  ],
  trader: [
    { desc: "探索 3 处野外地点", kind: "enter-poi", target: 3, progress: 0, done: false },
    { desc: "建造 3 个结构",     kind: "build", target: 3, progress: 0, done: false },
    { desc: "收获 8 单位麦子",   kind: "harvest-wheat", target: 8, progress: 0, done: false },
  ],
  scholar: [
    { desc: "探索 5 处野外地点", kind: "enter-poi", target: 5, progress: 0, done: false },
    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
    { desc: "建造 3 个结构",     kind: "build", target: 3, progress: 0, done: false },
  ],
};
```

### Update `wishesForArchetype` → `wishesForRole`

Replace the archetype-keyword-matching function with a direct role lookup. Keep `wishesForArchetype` as a backwards-compat wrapper that maps archetype → role (for any existing companions without a role field — shouldn't happen post-Task-9, but defensive):

```javascript
function wishesForRole(role) {
  return WISH_TEMPLATES[role] || WISH_TEMPLATES.builder;
}

// Backwards-compat: derive role from archetype keywords (for pre-Task-9 companions without a role field).
function wishesForArchetype(archetype) {
  const a = archetype || "";
  let role = "explorer"; // default
  if (/技|电|匠|build|builder/i.test(a)) role = "builder";
  else if (/厨|医|农|farm|farmer/i.test(a)) role = "farmer";
  else if (/兵|卫|guard/i.test(a)) role = "guard";
  else if (/商|trade|trader/i.test(a)) role = "trader";
  else if (/学|究|scholar/i.test(a)) role = "scholar";
  return wishesForRole(role);
}
```

### Update `consumeDrifter` call site

In `companion.js`'s `consumeDrifter` fn, the `Wish: { items: JSON.stringify(wishesForArchetype(args.archetype)) }` line should now use `wishesForRole(args.role || "builder")`. Update the call to prefer `args.role`, falling back to `wishesForArchetype(args.archetype)` for backwards compat:

```javascript
Wish: { items: JSON.stringify(args.role ? wishesForRole(args.role) : wishesForArchetype(args.archetype)), fulfilled: 0 },
```

### Add collective wish system

Add a new system `collective-wish-check` that fires when Colony.food_i >= 50 (granary-50 goal). One-time (guarded by `collective_wish_done`). On completion: +10 affinity to all companions, emit `collective-wish-fulfilled`, toast.

```javascript
const COLLECTIVE_WISH_THRESHOLD = 50;
const COLLECTIVE_WISH_AFFINITY_GAIN = 10;

vitric.system("collective-wish-check", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  if ((c.Colony.collective_wish_done | 0) !== 0) return;
  if ((c.Colony.food_i | 0) < COLLECTIVE_WISH_THRESHOLD) return;
  // Fulfill: mark done, buff all companions, emit event.
  ctx.setField("colony", "Colony.collective_wish_done", 1);
  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  for (const h of handles) {
    if (!h) continue;
    const aff = ctx.getField(h, "Need.affinity");
    const affNum = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
    const newAff = Math.min(100, affNum + COLLECTIVE_WISH_AFFINITY_GAIN);
    ctx.setField(h, "Need.affinity", newAff);
    ctx.setField(h, "Need.affinity_i", Math.round(newAff));
  }
  ctx.emit("collective-wish-fulfilled", { threshold: COLLECTIVE_WISH_THRESHOLD });
  ctx.emit("toast-show", { text: "共识达成: 粮储达到 " + COLLECTIVE_WISH_THRESHOLD + "!" });
});
```

**Note:** `wishesForRole` and `wishesForArchetype` are defined in `wish.js`, but `consumeDrifter` is in `companion.js`. QuickJS doesn't have ES modules — each script file is its own scope. The current code already calls `wishesForArchetype` from `companion.js`'s `consumeDrifter`, which means these fns must be in the SAME file. Check: are `companion.js` and `wish.js` concatenated by the engine, or are they separate?

Looking at the existing code: `consumeDrifter` (in companion.js) calls `wishesForArchetype` — so either (a) they're concatenated, or (b) `wishesForArchetype` is duplicated in companion.js. Reading companion.js line 26-50: YES, `WISH_TEMPLATES` and `wishesForArchetype` are ALSO defined in companion.js (lines 26-50). So they're duplicated.

**Action**: update BOTH copies (companion.js lines 26-50 AND wish.js) to add the 3 new role templates + `wishesForRole` fn. Keep them in sync. This is an existing DRY violation — don't try to fix it (out of scope).

## Rules: `games/frontier/rules/companion.json` (MODIFY)

### Update `drifter-cadence` rule

Change cap from 4 to 8, and add stage-based cadence modulation. Replace the existing `drifter-cadence` rule with two rules:

```json
{
  "id": "drifter-cadence-normal",
  "comment": "每隔一个游戏日 spawn 一个新旅人(最多 8 个,且场上没人时才 spawn)。stage != 兴旺 时 cadence=2 天。",
  "on": { "event": "day-start" },
  "if": [
    ["event.day", ">=", "@colony.Colony.next_drifter_day"],
    ["@colony.Colony.drifters_spawned", "<", 8],
    ["@colony.Colony.target_drifter", "==", ""],
    ["@colony.Colony.stage", "!=", "兴旺"]
  ],
  "do": [
    { "call": "spawnNewDrifter", "with": {
      "idx": "@colony.Colony.drifters_spawned",
      "arrival_day": "event.day"
    } },
    { "add": "@colony.Colony.drifters_spawned", "by": 1 },
    { "add": "@colony.Colony.next_drifter_day", "by": 2 }
  ]
},
{
  "id": "drifter-cadence-fast",
  "comment": "兴旺阶段:cadence=1 天(更频繁的旅人到访)。",
  "on": { "event": "day-start" },
  "if": [
    ["event.day", ">=", "@colony.Colony.next_drifter_day"],
    ["@colony.Colony.drifters_spawned", "<", 8],
    ["@colony.Colony.target_drifter", "==", ""],
    ["@colony.Colony.stage", "==", "兴旺"]
  ],
  "do": [
    { "call": "spawnNewDrifter", "with": {
      "idx": "@colony.Colony.drifters_spawned",
      "arrival_day": "event.day"
    } },
    { "add": "@colony.Colony.drifters_spawned", "by": 1 },
    { "add": "@colony.Colony.next_drifter_day", "by": 1 }
  ]
}
```

### Update `companion-invite-process` rule to pass `role`

The existing rule calls `consumeDrifter` with fields from `event`. The `companion-invited` event (emitted by `inviteAnyNearby`) now includes `role`. Add `"role": "event.role"` to the `with` object:

```json
{
  "id": "companion-invite-process",
  "comment": "邀请 → consumeDrifter(despawn by id + spawn companion + emit moved-in)。",
  "on": { "event": "companion-invited" },
  "do": [
    { "call": "consumeDrifter", "with": {
      "drifter_id": "event.drifter_id",
      "name": "event.name",
      "archetype": "event.archetype",
      "traits": "event.traits",
      "speech": "event.speech",
      "role": "event.role"
    } }
  ]
}
```

## Rules: `games/frontier/rules/hud.json` (MODIFY)

### Add `hud-collective-wish` rule

```json
{
  "id": "hud-collective-wish",
  "comment": "共识愿望 HUD:粮储 50 (达成/未达成)。每帧刷 @colony.Collective_wish_done + food_i。",
  "on": "tick",
  "do": [
    { "set": "@collective_wish_lbl.UiLabel.content", "to": { "format": "共识: 粮储 50 ({})", "args": [
      { "format": "{}",
        "args": ["@colony.Collective_wish_done"] }
    ] } }
  ]
}
```

Wait — the rule engine may not support nested format strings. Let me simplify: use a single format with a conditional check. Actually rule engines typically don't support if-then-else in `to` values. Let me use TWO rules with `if` clauses:

```json
{
  "id": "hud-collective-wish-pending",
  "comment": "共识愿望未达成:显示 粮储 NN/50。",
  "on": "tick",
  "if": [ ["@colony.Colony.collective_wish_done", "==", 0] ],
  "do": [
    { "set": "@collective_wish_lbl.UiLabel.content", "to": { "format": "共识: 粮储 {}/50 (未达成)", "args": ["@colony.Colony.food_i"] } }
  ]
},
{
  "id": "hud-collective-wish-done",
  "comment": "共识愿望已达成:显示 ✓共识达成。",
  "on": "tick",
  "if": [ ["@colony.Colony.collective_wish_done", "==", 1] ],
  "do": [
    { "set": "@collective_wish_lbl.UiLabel.content", "to": "✓共识达成: 粮储 50" }
  ]
}
```

Note: I corrected the field name — it's `Colony.collective_wish_done` (not `Collective_wish_done` — collective_wish_done is a field ON the Colony component, not a separate component). Audit: this field MUST be declared in schema.json under `components.Colony.fields.collective_wish_done`. (Already specified in the Schema section above.)

## Tests: `crates/vitric-cli/tests/companions.rs` (NEW)

Follow the pattern of `seasons.rs` / `research.rs`. 4 tests:

1. **`drifter_pool_has_12_entries`**: This is a JS-level check, not easily testable from Rust. SKIP this test — verify via code review instead. Replace with:

1. **`companion_contribution_role_builder_grants_plank`**: Boot runtime. Spawn a test companion with Persona.role="builder", Need.affinity=60, Mood.value="开心". Step ~15 ticks (contribution_timer starts at 0, fires immediately, then resets to 12-16s). Verify `@player.Inventory.plank` increased by 1.

   **Simpler approach**: don't spawn a new companion — modify the existing Pip companion (already in scene with role="builder" post-Task-9). Set Need.affinity=60 + Mood.value="开心" via direct component write. Step 1 tick (contribution_timer defaults to 0, fires on first tick). Verify plank increased.

2. **`companion_contribution_role_scholar_grants_techpoint`**: Same setup but role="scholar". Step 1 tick. Verify `@player.TechPoint.value` increased by 1 (via the tp-set event → rule → write-back — may need 2 ticks for the rule to fire).

3. **`collective_wish_fires_at_food_50`**: Boot runtime. Set `Colony.food_i = 50` AND `Colony.collective_wish_done = 0` via direct component write. Step 1 tick. Verify `Colony.collective_wish_done == 1` and `collective-wish-fulfilled` event emitted.

4. **`collective_wish_one_time_only`**: Same setup but `Colony.collective_wish_done = 1` already. Set `Colony.food_i = 80`. Step 1 tick. Verify `Colony.collective_wish_done` stays 1 (no repeat fire) and no second `collective-wish-fulfilled` event.

**Test setup notes** (from research.rs/seasons.rs pattern):
- `Runtime::boot(frontier_dir())` loads the full scene + logic.
- `frontier_dir()` helper: `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")`.
- To set component fields: `rt.world_mut().set_component(rt.world().entity("colony")?, "Colony", json!({"food_i": 50, "collective_wish_done": 0}))?` — check the exact Runtime API.
- To advance time: `rt.step_ticks(n)`.
- To read entity state: `rt.world().get_component(rt.world().entity("colony")?, "Colony")?`.
- To check events: `rt.drain_events()` or similar — check what the research.rs tests use.
- Keep tests fast: 1-2 ticks each.

If the Runtime API doesn't expose direct component writes, use the event-driven approach: emit a `ui-activate` event that triggers a rule which sets the field. OR use `ScriptEngine::call_fn` to call a test-helper fn that sets the field. Whichever is simpler — check existing tests for the pattern.

## Verification

```bash
# Schema check (must exit 0)
~/.cargo/bin/cargo run --release -- check games/frontier

# New companions tests (4 must pass — or 3 if test 1 is skipped)
~/.cargo/bin/cargo test -p vitric-cli --test companions

# Regression: research (4) + seasons (4) + region (14) still pass
~/.cargo/bin/cargo test -p vitric-cli --test research
~/.cargo/bin/cargo test -p vitric-cli --test seasons
~/.cargo/bin/cargo test -p vitric-cli --test region -- --skip typescript

# Workspace all-green
~/.cargo/bin/cargo test --workspace -- --skip typescript

# Gate EXPECTED-FAIL (ReplayDiverged at tick 0 — new collective_wish_done field on Colony changes tick-0 world hash; new collective_wish_lbl HUD entity too)
# DO NOT re-record qa/clear.json — Task 15 handles that.
~/.cargo/bin/cargo run --release -- gate games/frontier 2>&1 | tail -5
```

## Commit

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json \
        games/frontier/scripts/companion.js games/frontier/scripts/wish.js \
        games/frontier/rules/companion.json games/frontier/rules/hud.json \
        crates/vitric-cli/tests/companions.rs
git commit -m "feat(frontier): companions expansion — 6 roles, 12 pool, wish templates"
git push origin main
```

## Critical reminders (project-wide rules)

1. **All code comments must be English** (// and /* */ in JS, // in Rust). String literals (toast text, UI labels, game content) keep their original language.
2. **Every field read by a rule (`@entity.Comp.field`) MUST be declared in `schema.json`**. This task adds: `Persona.role` (enum), `Colony.collective_wish_done` (int). Audit before committing.
3. **Every field accessed via `ctx.getField` / `ctx.setField` MUST be declared**. `companion-contribution` reads `e.Persona.role` (via query — needs Persona in query). `collective-wish-check` reads `c.Colony.collective_wish_done` + `c.Colony.food_i`. Both must be declared.
4. **Rule engine format**: `{call: "fnName", with: {args}}` for fn calls; `{set: "path", to: value}` for field writes.
5. **DRY violation**: `WISH_TEMPLATES` + `wishesForArchetype` are duplicated in BOTH `companion.js` and `wish.js` (existing pattern). Update BOTH copies. Do NOT try to extract to a shared module (QuickJS has no ES modules — out of scope).
6. **`@player` entity-name reference**: verify `ctx.getField("@player", "Inventory.plank")` works. If the engine doesn't accept the `@` prefix in ctx.getField (only in rule `@entity.Comp.field` syntax), use `"player"` instead. Check existing patterns in `companion.js` line 661-667 — they use `ctx.getField("@player", ...)` and `ctx.setField("@player", ...)` — so `@player` IS valid in ctx API. Follow that pattern.
7. **`drifter-cadence` split into two rules**: ensure they're mutually exclusive via the `stage != "兴旺"` vs `stage == "兴旺"` if clauses. Both rules fire on `day-start` — only one will match.
8. **Gate EXPECTED-FAIL is OK** — do NOT re-record `qa/clear.json`. Task 15 handles it.
9. **Don't implement combat/trade/region-expansion systems** — only emit the forward-compat hook events (`guard-patrol`, `trade-available`, `explore-bonus`). Tasks 10/11/12 consume them.

## Deliverable

Return a report at `.superpowers/sdd/briefs/task-9-report.md` with:
- Commit hash
- Files changed (count + list)
- Test results (companions N/N, research 4/4, seasons 4/4, region 14/14, schema check exit 0, workspace all-green, gate failure mode)
- Deviations from this brief (with reasoning)
- Concerns / known issues

Do NOT update `progress.md` — the controller does that after review.
