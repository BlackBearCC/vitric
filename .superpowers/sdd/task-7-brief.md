# Task 7 Brief — Node Regrowth + Structure Upgrade Path

## Where this fits

Tasks 1–6 complete. Task 7 adds two economy mechanics:
1. **Node regrowth**: wild resource nodes (ore/wood/fiber) that deplete now regrow after a cooldown, so the world is renewable.
2. **Structure upgrade**: tier-1 structures (plot/conduit/quarters) can be upgraded to tier-2 (greenhouse/solar-array/cabin) by paying resources.

Both reuse existing schema fields (`Node.left/max/cooldown`, `Structure.kind/tier`) — **no schema changes**. Both reuse existing files (`economy.js`, `economy.json`) — already registered in `vitric.json`, **no vitric.json changes**.

## Files to modify

- `games/frontier/scripts/economy.js` — modify `interact` fn (set cooldown on depletion) + add `node_regrow` system + add `upgrade_structure` fn
- `games/frontier/rules/economy.json` — add 1 rule (`upgrade-button-click`)

That's it. Do not touch schema.json, vitric.json, or any other file.

## Real API reference (confirmed from prelude.js + existing economy.js)

- `vitric.system(name, {query: [components], writes: [components]}, (entities, ctx) => {...})` — system. `entities` is an array of entities each having `.Comp.field` access. `writes` must be subset of `query`.
- `vitric.fn(name, (args, ctx) => {...})` — rule-callable function. `args` is the object passed via rule's `with`.
- `ctx.getField(ref, "Comp.field")` — read any entity's field by name or handle. Returns undefined if missing.
- `ctx.setField(ref, "Comp.field", value)` — write any entity's field (deferred commit).
- `ctx.emit(name, data)` — emit event.
- `ctx.dt` — delta time per tick (seconds).
- `ctx.random()` — deterministic RNG (NOT `Math.random`, which is poisoned).
- Inventory write-back pattern: fn emits `inv-set{absolute values}` → rule `inv-apply` writes `@player.Inventory.*`. The fn CANNOT directly write `@player.Inventory` (different entity).
- `readInv(a)` / `canPay(inv, cost)` / `pay(inv, cost)` / `emitInv(ctx, inv)` helpers already exist in economy.js (lines 37-61). Reuse them.

## Existing code context

### economy.js `interact` fn (lines 96-136) — current Node gathering

```javascript
  // ---- Wild resource node gathering (scene pre-spawns 6 nodes, left>0 means harvestable) ----
  if (node && (node.left | 0) > 0) {
    const inv = readInv(a);
    const nodeKind = node.kind || "ore";
    const ITEM_MAP = { ore: "ore", wood: "wood", fiber: "fiber" };
    const itemId = ITEM_MAP[nodeKind] || "ore";
    inv[itemId] += 1;
    ctx.setField(a.entity, "Node.left", (node.left | 0) - 1);
    emitInv(ctx, inv);
    ctx.emit("gathered", { node: nodeKind, id: itemId, n: 1 });
    return;
  }
```

**Problem**: when `left` hits 0, the node stays empty forever. Need to set `cooldown = 90` so the new `node_regrow` system can count it down and regrow.

### economy.js existing systems (lines 140-157) — `plot-hint` and `node-hint`

These show the pattern for a system that queries + writes a component:
```javascript
vitric.system("node-hint", { query: ["Node", "Text"], writes: ["Text"] }, (entities, ctx) => {
  for (const e of entities) {
    const left = e.Node.left | 0;
    ...
    if (e.Text.content !== t) e.Text.content = t;
  }
});
```

### economy.json existing rules — `build-click`, `craft-*`, `inv-apply`

The `build-click` rule (lines 3-16) shows the pattern for calling a fn with entity + inventory:
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

## Exact changes

### Edit 1: economy.js — set cooldown when node depletes

In the `interact` fn, find the Node gathering block (lines 124-135). Replace:

```javascript
  // ---- Wild resource node gathering (scene pre-spawns 6 nodes, left>0 means harvestable) ----
  if (node && (node.left | 0) > 0) {
    const inv = readInv(a);
    const nodeKind = node.kind || "ore";
    const ITEM_MAP = { ore: "ore", wood: "wood", fiber: "fiber" };
    const itemId = ITEM_MAP[nodeKind] || "ore";
    inv[itemId] += 1;
    ctx.setField(a.entity, "Node.left", (node.left | 0) - 1);
    emitInv(ctx, inv);
    ctx.emit("gathered", { node: nodeKind, id: itemId, n: 1 });
    return;
  }
```

With:

```javascript
  // ---- Wild resource node gathering (scene pre-spawns 6 nodes, left>0 means harvestable) ----
  // On depletion (left hits 0), set cooldown so node_regrow system can regrow it after 90s.
  if (node && (node.left | 0) > 0) {
    const inv = readInv(a);
    const nodeKind = node.kind || "ore";
    const ITEM_MAP = { ore: "ore", wood: "wood", fiber: "fiber" };
    const itemId = ITEM_MAP[nodeKind] || "ore";
    inv[itemId] += 1;
    const newLeft = (node.left | 0) - 1;
    ctx.setField(a.entity, "Node.left", newLeft);
    if (newLeft <= 0) {
      ctx.setField(a.entity, "Node.cooldown", 90); // 1.5 min regrow timer
    }
    emitInv(ctx, inv);
    ctx.emit("gathered", { node: nodeKind, id: itemId, n: 1 });
    return;
  }
```

**Rationale**: `Node.cooldown` already exists in schema (default 0). Setting it to 90 (seconds) when the node depletes kicks off the regrow cycle. The `node_regrow` system (Edit 2) decrements it.

### Edit 2: economy.js — add `node_regrow` system

Add this system at the END of economy.js (after the `craft` fn, line 170):

```javascript

// ---- Node regrowth: depleted nodes regrow to max after cooldown elapses ----
vitric.system("node_regrow", { query: ["Node"], writes: ["Node"] }, (entities, ctx) => {
  for (const e of entities) {
    const left = e.Node.left | 0;
    const cd = e.Node.cooldown || 0;
    if (left <= 0 && cd > 0) {
      const newCd = cd - ctx.dt;
      if (newCd <= 0) {
        e.Node.left = e.Node.max | 0;
        e.Node.cooldown = 0;
      } else {
        e.Node.cooldown = newCd;
      }
    }
  }
});
```

**Rationale**: Queries all entities with `Node`, writes `Node`. For each depleted node (`left <= 0`) with an active cooldown, decrements by `ctx.dt`. When cooldown hits 0, regrows `left = max` and clears cooldown. `Node.max` is the per-node capacity (default 3, set in scene).

### Edit 3: economy.js — add `upgrade_structure` fn

Add this fn at the END of economy.js (after the `node_regrow` system from Edit 2):

```javascript

// ---- Structure upgrade: tier-1 -> tier-2, pay resources, change kind ----
// Called by rule on ui-activate{action:"upgrade-prompt"} — passes target entity handle + current inventory.
// Reads Structure.kind/tier via ctx.getField (deferred-write safe: reads happen before any writes).
// UPGRADES table: which tier-1 kinds can upgrade, to what, and the cost.
vitric.fn("upgrade_structure", (a, ctx) => {
  if (typeof a.entity !== "string" || !a.entity) return;
  const kind = ctx.getField(a.entity, "Structure.kind");
  const tier = ctx.getField(a.entity, "Structure.tier") | 0;
  if (!kind || tier >= 2) {
    ctx.emit("toast-show", { text: "已满级或无法升级" });
    return;
  }
  const UPGRADES = {
    plot:     { to: "greenhouse",  cost: { ore: 2, plank: 2 } },
    conduit:  { to: "solar-array", cost: { ore: 3, plank: 1 } },
    quarters: { to: "cabin",       cost: { plank: 4, lamp: 1 } },
  };
  const up = UPGRADES[kind];
  if (!up) {
    ctx.emit("toast-show", { text: "该结构无法升级" });
    return;
  }
  const inv = readInv(a);
  if (!canPay(inv, up.cost)) {
    ctx.emit("toast-show", { text: "资源不足" });
    return;
  }
  pay(inv, up.cost);
  emitInv(ctx, inv);
  ctx.setField(a.entity, "Structure.kind", up.to);
  ctx.setField(a.entity, "Structure.tier", 2);
  ctx.emit("upgrade-structure", { id: a.entity, kind: up.to });
  ctx.emit("toast-show", { text: "升级为" + up.to });
});
```

**Rationale**:
- `ctx.getField(a.entity, "Structure.kind")` reads the live world state at call start (prelude guarantees this is pre-write snapshot). Safe even though we later `setField` on the same entity.
- `readInv(a)` / `canPay` / `pay` / `emitInv` reuse existing helpers (lines 37-61). `emitInv` emits `inv-set` → rule `inv-apply` writes back `@player.Inventory.*`.
- Emits `upgrade-structure` event so the wish system (Task 4, wish.json rule `wish-advance-upgrade`) can advance the "upgrade" wish.
- Emits `toast-show` directly — Task 4's wish.json `toast-show-generic` rule catches it and writes `@toast_lbl.UiLabel.content` + timer.
- Upgrade table: plot→greenhouse (better farming), conduit→solar-array (better power), quarters→cabin (better housing). Costs balanced against the seed-start inventory (ore6/plank6/lamp2).

### Edit 4: economy.json — add `upgrade-button-click` rule

Add this rule at the END of the `rules` array in economy.json (after `seed-start`, line 77):

```json
    {
      "id": "upgrade-button-click",
      "comment": "Build mode right-click on tier-1 structure -> attempt upgrade. UI (hud.js) emits ui-activate{action:\"upgrade-prompt\", target_id:<entity>} — wired in Task 8.",
      "on": { "event": "ui-activate", "filter": { "action": "upgrade-prompt" } },
      "do": [
        { "call": "upgrade_structure", "with": {
          "entity": "event.target_id",
          "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
          "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
          "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp"
        } }
      ]
    }
```

**Rationale**: Same pattern as `build-click` — passes the target entity handle (`event.target_id`) + full inventory to the fn. The UI that emits `ui-activate{action:"upgrade-prompt"}` is Task 8's job; this rule is the receiver placeholder that makes the fn callable.

## Out of scope (do NOT touch)

- `schema.json` — `Node.cooldown` (line 425), `Node.max` (line 421), `Structure.kind` (line 296), `Structure.tier` (line 300) all already exist. No changes.
- `vitric.json` — `economy.js` and `economy.json` already registered. No changes.
- `scripts/hud.js` — the UI that emits `ui-activate{action:"upgrade-prompt"}` is Task 8.
- `scripts/wish.js` — already has a rule `wish-advance-upgrade` catching `upgrade-structure` events (Task 4). No changes.
- `scenes/main.json` — no new entities needed. Existing nodes get regrowth automatically via the system.

## Verification

After all 4 edits:

```bash
cd /Users/leolele/Documents/leo/vitric
cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -15
```

Expected: exit code 0 (success). The CLI emits a JSON report on success.

Do NOT run `vitric gate` — `qa/clear.json` is stale (Task 6 changed `must_emit` to `settlement-founded`; Task 9 re-records).

## Commit

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/economy.js games/frontier/rules/economy.json
git commit -m "feat(frontier): node regrowth + structure upgrade path (tier 1->2)"
git push origin main
```

Per project memory: auto commit + push to main after edits + verification.

## Self-check checklist (verify in report)

- [ ] `interact` fn sets `Node.cooldown = 90` when `newLeft <= 0` (Edit 1)
- [ ] `node_regrow` system queries `["Node"]`, writes `["Node"]`, decrements cooldown by `ctx.dt`, regrows `left = max` when cooldown hits 0 (Edit 2)
- [ ] `upgrade_structure` fn reads kind/tier via `ctx.getField`, checks cost via `canPay`, pays via `pay`+`emitInv`, writes kind/tier via `ctx.setField`, emits `upgrade-structure` + `toast-show` (Edit 3)
- [ ] `upgrade-button-click` rule in economy.json calls `upgrade_structure` with `entity: "event.target_id"` + full inventory (Edit 4)
- [ ] schema.json NOT modified
- [ ] vitric.json NOT modified
- [ ] `cargo run -p vitric-cli -- check games/frontier` exits 0
- [ ] Committed + pushed to `main`

## Report contract

Write the full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-7-report.md` and return only: status (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit hash(es), one-line check summary, and any concerns.
