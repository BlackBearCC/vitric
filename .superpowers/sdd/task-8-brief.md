# Task 8 Brief — UI Hooks: Upgrade Mode + Flare Warning Bar

## Where this fits

Tasks 1–7 complete. Task 8 wires up the UI for two of the new mechanics:
1. **Structure upgrade trigger**: a new "upgrade" mode (key "u") + click rule that calls `upgrade_structure` (from Task 7) when the player clicks a structure.
2. **Flare warning bar**: a system that writes a warning string when `Colony.flare_warning == 1` + a rule that displays it.

**POI prompt is NOT in this task** — Task 3 already handles POI fully via direct interact-mode click (`poi-interact-click` rule → `interact_poi` fn). No modal needed; the plan's "auto-pick explore" is already the live behavior.

## Files to modify

- `games/frontier/scripts/hud.js` — add `flare-bar` system (writes `Colony.flare_bar`)
- `games/frontier/rules/hud.json` — add `hud-flare-bar` rule (copies `Colony.flare_bar` → `@flare_lbl`)
- `games/frontier/rules/ui.json` — add `kb-mode-upgrade` rule (key "u" → Mode.value = "upgrade" + hide menus)
- `games/frontier/rules/economy.json` — add `upgrade-click` rule (mouse + Mode=upgrade → call `upgrade_structure`)

That's it. No schema changes (ad-hoc `Colony.flare_bar` field, same pattern as existing `Colony.food_bar`). No vitric.json changes (all files already registered). No scene changes (`@flare_lbl` entity added in Task 9).

## Real API reference (confirmed)

- `vitric.system(name, {query, writes}, (entities, ctx) => {...})` — system. `entities` array, each has `.Comp.field` access.
- `vitric.fn(name, (args, ctx) => {...})` — rule-callable function.
- Rules are declarative JSON: `on` (event), `if` (predicates), `do` (actions: `set`/`emit`/`call`).
- Mouse events carry: `event.x`, `event.y`, `event.entity` (hit entity name/handle), `event.comp` (hit entity's components).
- Input events: `{event: "input", filter: {action: "<key>", phase: "pressed"|"released"}}`.
- `ui-activate` events: `{event: "ui-activate", filter: {action: "<name>"}}`.
- Mode switching: `@uistate.Mode.value` (existing modes: "build", "craft", "interact").
- HUD pattern (from existing `food-bar`): system writes derived string to `Colony.<field>` → rule copies `@colony.Colony.<field>` → `@<label>_lbl.UiLabel.content`.

## Existing code context

### hud.js (full file, 24 lines) — one system `food-bar`

```javascript
vitric.system("food-bar", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const food = Math.round(e.Colony.food_i || 0);
    ...
    e.Colony.food_bar = "食 " + clamped + "/100 [" + bar + "]";
  }
});
```

This is the pattern to follow for `flare-bar`: query `["Colony"]`, write `["Colony"]`, compute a string, write it to `e.Colony.flare_bar`.

### hud.json — `hud-food-bar` rule (the pattern to follow)

```json
{
  "id": "hud-food-bar",
  "on": "tick",
  "do": [
    { "set": "@hud_food_lbl.UiLabel.content", "to": "@colony.Colony.food_bar" }
  ]
}
```

### ui.json — mode-switch rules (the pattern to follow)

```json
{ "id": "kb-mode-build",    "on": { "event": "input", "filter": { "action": "q", "phase": "pressed" } }, "do": [ { "set": "@uistate.Mode.value", "to": "build" } ] },
{ "id": "kb-mode-interact", "on": { "event": "input", "filter": { "action": "r", "phase": "pressed" } }, "do": [ { "set": "@uistate.Mode.value", "to": "interact" } ] },
{ "id": "kb-mode-craft",    "on": { "event": "input", "filter": { "action": "e", "phase": "pressed" } }, "do": [ { "set": "@uistate.Mode.value", "to": "craft" } ] }
```

These are the last 3 entries in ui.json's `rules` array (line 63). The `mode-interact` rule (lines 33-41) shows the full pattern with menu hiding:
```json
{
  "id": "mode-interact",
  "on": { "event": "ui-activate", "filter": { "action": "mode-interact" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "interact" },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 }
  ]
}
```

### economy.json — `build-click` rule (the pattern for mouse + mode + call fn)

```json
{
  "id": "build-click",
  "on": { "event": "mouse" },
  "if": [ ["@uistate.Mode.value", "==", "build"] ],
  "do": [
    { "call": "build", "with": {
      "x": "event.x", "y": "event.y", "entity": "event.entity", "kind": "@uistate.Build.kind",
      "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", ...
    } }
  ]
}
```

### Task 7's `upgrade-button-click` rule (already in economy.json)

```json
{
  "id": "upgrade-button-click",
  "on": { "event": "ui-activate", "filter": { "action": "upgrade-prompt" } },
  "do": [ { "call": "upgrade_structure", "with": { "entity": "event.target_id", ... } } ]
}
```

This rule (from Task 7) stays as-is — it's a programmatic API for a future UI modal. Task 8 adds a DIRECT click rule (`upgrade-click`) that bypasses the `ui-activate` indirection, following the `build-click` / `poi-interact-click` pattern. Both rules call the same `upgrade_structure` fn.

## Exact changes

### Edit 1: hud.js — add `flare-bar` system

Append at the END of hud.js (after the `food-bar` system, line 24):

```javascript

// Flare warning bar: shows a warning string when Colony.flare_warning == 1.
// The rule in hud.json pulls Colony.flare_bar into @flare_lbl.UiLabel.content.
// @flare_lbl entity is added in Task 9 scene polish; until then the rule's set silently no-ops.
vitric.system("flare-bar", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const warn = e.Colony.flare_warning | 0;
    e.Colony.flare_bar = warn ? "\u26a0 \u8000\u6591\u5373\u5c06\u6765\u88ad!" : "";
  }
});
```

**Note on the string**: `\u26a0` = ⚠ (warning sign), `\u8000\u6591\u5373\u5c06\u6765\u88ad` = "耀斑即将来袭". Using Unicode escapes for the warning sign to avoid encoding issues; the Chinese string can be literal or escaped — both work. If you prefer literal Chinese, write: `e.Colony.flare_bar = warn ? "⚠ 耀斑即将来袭!" : "";` — either form is acceptable.

**Rationale**: Follows the `food-bar` pattern exactly. `Colony.flare_warning` is set by `flare_tick` system (Task 2, flare.js) to `1` when a flare is 30 seconds out. The system writes a non-empty warning string when warning is active, empty string otherwise. `Colony.flare_bar` is an ad-hoc field (same as `Colony.food_bar` — engine allows undeclared fields).

### Edit 2: hud.json — add `hud-flare-bar` rule

Append at the END of the `rules` array in hud.json (after `hud-inv`, line 56). Be careful with JSON: add a comma after the `hud-inv` rule's closing `}` before the new rule.

```json
    {
      "id": "hud-flare-bar",
      "comment": "耀斑预警条:flare-bar 系统写 Colony.flare_bar,这里拉到 @flare_lbl。@flare_lbl 实体在 Task 9 场景 polish 里加,届时该规则生效。",
      "on": "tick",
      "do": [
        { "set": "@flare_lbl.UiLabel.content", "to": "@colony.Colony.flare_bar" }
      ]
    }
```

**Rationale**: Same pattern as `hud-food-bar`. The `@flare_lbl` UI entity doesn't exist in the scene yet — the rule's `set` will silently no-op until Task 9 adds the entity. This is the same behavior as the Task 3 main.json regression (quest labels missing → rules silently failed), but intentional and temporary.

### Edit 3: ui.json — add `kb-mode-upgrade` rule

The last line of ui.json (line 63) is:
```json
    { "id": "kb-mode-craft", "on": { "event": "input", "filter": { "action": "e", "phase": "pressed" } }, "do": [ { "set": "@uistate.Mode.value", "to": "craft" } ] }
  ]
}
```

Replace that last entry + closing to add the new rule after it:

```json
    { "id": "kb-mode-craft", "on": { "event": "input", "filter": { "action": "e", "phase": "pressed" } }, "do": [ { "set": "@uistate.Mode.value", "to": "craft" } ] },
    { "id": "kb-mode-upgrade", "comment": "u 键切升级模式:藏两个菜单(同 interact 模式),点击结构即调 upgrade_structure。",
      "on": { "event": "input", "filter": { "action": "u", "phase": "pressed" } },
      "do": [
        { "set": "@uistate.Mode.value", "to": "upgrade" },
        { "set": "@build_menu.Ui.ox", "to": -3000 },
        { "set": "@craft_menu.Ui.ox", "to": -3000 }
      ]
    }
  ]
}
```

**Rationale**: Key "u" enters upgrade mode. Hides both build and craft menus (same as interact mode) since upgrade doesn't need a palette. The player then clicks a structure to attempt upgrade.

### Edit 4: economy.json — add `upgrade-click` rule

Append at the END of the `rules` array in economy.json (after `upgrade-button-click` which was added in Task 7). Be careful with JSON: add a comma after the `upgrade-button-click` rule's closing `}` before the new rule, and no trailing comma after the new rule.

```json
    {
      "id": "upgrade-click",
      "comment": "Upgrade mode + left-click on a structure -> call upgrade_structure. Passes hit entity + full inventory. The fn handles non-structure clicks gracefully (returns early).",
      "on": { "event": "mouse" },
      "if": [ ["@uistate.Mode.value", "==", "upgrade"] ],
      "do": [ { "call": "upgrade_structure", "with": {
        "entity": "event.entity",
        "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
        "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
        "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp"
      } } ]
    }
```

**Rationale**: Same pattern as `build-click` (mouse + mode guard → call fn with entity + inventory). The `upgrade_structure` fn (Task 7) reads `Structure.kind`/`tier` via `ctx.getField` — if the clicked entity isn't a Structure, `getField` returns undefined and the fn emits a "无法升级" toast and returns. No need for a structure-type filter in the rule; the fn is the guard.

## Out of scope (do NOT touch)

- `schema.json` — `Colony.flare_bar` is an ad-hoc field (same as `Colony.food_bar`). No schema change.
- `vitric.json` — hud.js, hud.json, ui.json, economy.json all already registered. No change.
- `scenes/main.json` — `@flare_lbl` entity added in Task 9. No change here.
- `scripts/economy.js` — `upgrade_structure` fn already added in Task 7. No change.
- `scripts/flare.js` — `flare_tick` system already sets `Colony.flare_warning` (Task 2). No change.
- `scripts/poi.js` / `rules/poi.json` — POI fully handled in Task 3. No change.

## Verification

After all 4 edits:

```bash
cd /Users/leolele/Documents/leo/vitric
cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -15
```

Expected: exit code 0 (success).

Do NOT run `vitric gate` — qa/clear.json stale (Task 9 re-records).

## Commit

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/hud.js games/frontier/rules/hud.json games/frontier/rules/ui.json games/frontier/rules/economy.json
git commit -m "feat(frontier): UI hooks for upgrade mode + flare warning bar"
git push origin main
```

Per project memory: auto commit + push to main after edits + verification.

## Self-check checklist (verify in report)

- [ ] hud.js: `flare-bar` system queries `["Colony"]`, writes `["Colony"]`, writes `e.Colony.flare_bar` = warning string or "" (Edit 1)
- [ ] hud.json: `hud-flare-bar` rule on tick sets `@flare_lbl.UiLabel.content` from `@colony.Colony.flare_bar` (Edit 2)
- [ ] ui.json: `kb-mode-upgrade` rule on key "u" sets Mode.value = "upgrade" + hides both menus (Edit 3)
- [ ] economy.json: `upgrade-click` rule on mouse + Mode=upgrade calls `upgrade_structure` with entity + full inventory (Edit 4)
- [ ] JSON valid in all 3 rule files (commas correct, no trailing commas)
- [ ] schema.json NOT modified
- [ ] vitric.json NOT modified
- [ ] `cargo run -p vitric-cli -- check games/frontier` exits 0
- [ ] Committed + pushed to `main`

## Report contract

Write the full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-8-report.md` and return only: status (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit hash(es), one-line check summary, and any concerns.
