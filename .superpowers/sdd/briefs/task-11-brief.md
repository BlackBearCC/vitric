# Task 11 Brief — Trading & Diplomacy (Faction component, 3 factions, trade menu, LLM negotiation)

## Context

Frontier Sandbox Expansion, Task 11 of 16. Base commit: `568dc73` (after Task 10 review artifacts). Plan: `docs/superpowers/plans/2026-07-20-frontier-sandbox-expansion.md` §Task 11 (lines 1254-1306). Spec: `docs/superpowers/specs/2026-07-20-frontier-sandbox-expansion-design.md` §4.5 (lines 266-294).

Phase 3 game systems. Dependencies already in place from prior tasks:
- **Task 9 (Companions)**: `Persona.role` enum has a `trader` variant; `companion-contribution` emits `trade-available` event when a trader companion contributes (forward-compat hook). `inviteAnyNearby` reads drifter role.
- **Task 10 (Combat)**: `Mode.value` enum has `combat` variant; `hp_lbl` at top-right oy:312; `mode_row` at top-left oy:100 w:582.
- **Task 8 (Tech Tree)**: `Industry T3` tech unlocks things (already declared in `TECH_TREE`).
- **Task 1 (E1 Region)**: `Region` component with dormant/active/frozen states; `ctx.thaw_region` available.

Task 12 (Map Expansion) comes AFTER this task. Task 12 will:
- Add the `desert` region marker (dormant) and define the unlock condition `Faction caravan relation >= neutral AND Tech: Industry T3`.
- So Task 11 must expose a way for Task 12 to check Faction.tier_caravan — via `ctx.getField("colony", "Faction.tier_caravan")` (string field, value in {"hostile","wary","neutral","friendly","allied"}).

## Goal

Add a complete trading & diplomacy loop:
1. **Faction component** on `colony` entity: `relations` (JSON text `{"nomads":30,"caravan":0,"remnant":-10}`), three `tier_<faction>` derived fields.
2. **3 factions** (Nomads 荒原游民 / Caravan 商队 / Remnant 遗民) with starting relations per spec.
3. **`faction-tick` system**: derives `tier_<faction>` from `relations` each tick (hostile/wary/neutral/friendly/allied).
4. **`change_relation` fn**: applies delta with clamping to [-100, 100]. Emits `relation-change` event.
5. **`complete_trade` fn**: barter — gives player `receive` items, deducts `give` items, applies +2 relation to that faction. Uses rate multiplier by tier.
6. **Trade menu UI** (`trade_menu` entity, mirrors `craft_menu` layout): 3 trade offers (one per faction). Barter only, no currency.
7. **Trade mode** (`Mode.value = "trade"`, `B` key + button click). Hides other menus.
8. **`negotiate` fn** using `ctx.ask("llm")` with deterministic fallback (mirrors `triggerWishMemory` / `onWishMemoryReply` pattern in `wish.js`).
9. **Forward-compat hooks** for Task 12: `Faction.tier_caravan` is readable (Task 12 will check `>= "neutral"`). `relation-change` event is emitted (Task 12 can hook `caravan >= neutral` to unlock desert).
10. **Tests** in `crates/vitric-cli/tests/faction.rs`: 4 tests covering relation change, tier derivation, trade completion, negotiation fallback.

## Plan corrections (fictional APIs / scope clarifications)

1. **`tools/test_progression.py` does NOT exist** — write Rust tests in `crates/vitric-cli/tests/faction.rs` following the pattern of `combat.rs` / `companions.rs` / `research.rs`.
2. **`Faction` is a single component on the `colony` entity** (NOT multiple Faction components across multiple entities). The spec says "JSON field on colony entity". So `relations` is one JSON text field; `tier_nomads` / `tier_caravan` / `tier_remnant` are three derived text fields.
3. **Barter only (no currency)**: trades are item-for-item. Example: give 3 wheat → receive 1 plank (rate depends on faction + tier). Rate multiplier: hostile 0.5 / wary 0.8 / neutral 1.0 / friendly 1.2 / allied 1.5. Rate affects RECEIVE quantity (round to int, min 1).
4. **Faction-specific recipes (friendly tier)**: spec says "friendly: unlock faction-specific recipes". This is forward-compat — Task 11 only declares the hook (e.g., `friendly-recipe-unlocked` event); the actual recipes are deferred (Task 13 Region Content Polish or a future task). Don't add new build recipes.
5. **Allied region unlock**: spec says "allied: unlock faction's region (desert for caravan, mountain深处 for remnant)". Task 12 will implement actual region unlock. Task 11 only emits `faction-allied` event (consumed by Task 12).
6. **Joint defense (allied reinforcement)**: spec says "joint defense (faction reinforcements during night raids)". This is a forward-compat hook — Task 11 emits `faction-reinforcements-available` event when an allied faction exists during `night-fall`. Task 10's `spawn_wave` fn or Task 13 can consume this. Don't add new enemy spawning logic.
7. **Trader companion integration**: Task 9's `companion-contribution` already emits `trade-available` event. Task 11 adds a rule that catches `trade-available` → `change_relation("nomads", +1)` (trader companion maintains nomad relation). This is a small +1, not +2 (the +2 is for player-initiated trade).
8. **LLM negotiation**: reuse `ctx.ask("llm", prompt, "onNegotiateReply")` pattern (mirrors `triggerWishMemory`/`onWishMemoryReply` in `wish.js`). Stash target faction in `Colony._negotiate_target` (NEW field, must be declared). On reply: apply fixed `+3` relation (deterministic — LLM doesn't control mechanics, only flavor text). Fallback: 3 canned lines per faction. `LLM_ERROR_FALLBACK` detection mirrors `wish.js` pattern (text equals `（旅人沉默片刻,点了点头）`).
9. **Trade offers are static data** (not dynamic): 3 fixed trades (one per faction). Don't generate trades procedurally — keep deterministic and testable.
10. **No new `Region` unlocks in this task** — Task 12 handles region markers + unlock rules. Task 11 only exposes `Faction.tier_<f>` fields + `faction-allied` event.
11. **`Inv-apply` rule already covers all inventory fields** (Task 8 extended it to include `hide` + `crystal_core`). Task 11's `complete_trade` fn emits `inv-set` with the full inventory — the existing rule writes it back. Verify this by reading `rules/economy.json` `inv-apply` rule.
12. **QuickJS shared global scope**: `faction.js` declares `FACTION_IDS` / `FACTION_INFO` / `TRADE_OFFERS` / `RATE_MULT_BY_TIER` / `NEGOTIATION_FALLBACKS` as `const` at top level. These are accessible to any subsequent script. `vitric.json` scripts array loads `faction.js` AFTER `economy.js` (economy doesn't need faction symbols, but faction may need inventory constants — declare inline to be safe, don't cross-reference).
13. **Mode enum**: extend with `"trade"` variant. The `mode_row` HBox already has 5 buttons (build/craft/interact/research/combat); add a 6th `mode_trade` button. `mode_row.Ui.w` bumps 582 → 678 (6×92 + 6×6 = 588, round up to 678 to match brief convention of 6 slots — actually 582 currently fits 5 buttons, so 582 + 92 + 6 = 680; use 680).
14. **HUD label**: `relation_lbl` (top-right, below `hp_lbl` at oy:312 + h:24 = 336, so place at oy:340). Shows "关系 游民 N 商队 N 遗民 N" each tick.

## Schema changes (`games/frontier/schema.json`)

### Add new component `Faction`

```json
"Faction": {
  "fields": {
    "relations": { "type": "text", "default": "{\"nomads\":30,\"caravan\":0,\"remnant\":-10}" },
    "tier_nomads": { "type": "text", "default": "neutral" },
    "tier_caravan": { "type": "text", "default": "wary" },
    "tier_remnant": { "type": "text", "default": "wary" }
  }
}
```

### Extend `Colony` component

Add `_negotiate_target` field (for stashing LLM negotiation target faction id):

```json
"_negotiate_target": { "type": "text", "default": "" }
```

### Extend `Mode` enum

Add `"trade"` variant. Final enum: `["build","craft","interact","upgrade","research","combat","trade"]`.

### Attach `Faction` to `colony` entity in `scenes/main.json`

Add `"Faction": {}` to `colony` entity's components (uses default `relations` JSON + default tiers). The `faction-tick` system will derive tiers on tick 1.

## New script `games/frontier/scripts/faction.js`

```javascript
// Faction system (Task 11): 3 factions, relation tracking, tier derivation,
// barter trades, LLM negotiation with deterministic fallback.
//
// Systems:
//   faction-tick    Colony+Faction → derive tier_<f> from relations JSON each tick
//
// Fns (called by rules in rules/faction.json + rules/trade.json):
//   change_relation    (faction, delta) → clamp [-100,100], emit relation-change
//   complete_trade     (faction, give_kind, give_n, receive_kind, receive_n) → barter + +2 relation
//   negotiate          (faction) → ctx.ask("llm") with onNegotiateReply callback
//   onNegotiateReply   (reply) → apply +3 relation to stashed target, fallback canned line
//
// Data flow: Faction.relations is JSON text. faction-tick parses it, computes tier_<f>
// from relation value, writes tier_<f> back. Other systems/rules read tier_<f> directly.
// LLM negotiation stash: Colony._negotiate_target (text field, mirrors last_wish_memory_target pattern).

// Faction ids + starting relations (mirrors schema default).
const FACTION_IDS = ["nomads", "caravan", "remnant"];

const FACTION_INFO = {
  nomads:  { name: "荒原游民", home: "wild" },
  caravan: { name: "商队",     home: "desert" },
  remnant: { name: "遗民",     home: "mountain" },
};

// Tier thresholds (spec §4.5 line 273).
// hostile: -100..-50, wary: -49..10, neutral: 11..40, friendly: 41..75, allied: 76..100.
function tierFromRelation(v) {
  if (v >= 76) return "allied";
  if (v >= 41) return "friendly";
  if (v >= 11) return "neutral";
  if (v >= -49) return "wary";
  return "hostile";
}

// Rate multiplier by tier (affects receive quantity).
const RATE_MULT_BY_TIER = {
  hostile: 0.5,
  wary: 0.8,
  neutral: 1.0,
  friendly: 1.2,
  allied: 1.5,
};

// Static trade offers (3 — one per faction). Barter only.
// give: player gives; receive: player receives. Base rate; actual receive_n is computed
// at trade time as max(1, round(base_receive_n * rate_mult)).
const TRADE_OFFERS = {
  nomads:  { give_kind: "wheat",  give_n: 3, receive_kind: "fiber",  base_receive_n: 2 },
  caravan: { give_kind: "plank",  give_n: 2, receive_kind: "hide",   base_receive_n: 1 },
  remnant: { give_kind: "hide",   give_n: 2, receive_kind: "crystal_core", base_receive_n: 1 },
};

// LLM negotiation fallback lines (deterministic — used when LLM unavailable).
const NEGOTIATION_FALLBACKS = {
  nomads: [
    "游民长老眯眼看了看你,挥了挥手:「这点心意够了,以后多走动。」",
    "游民似乎对你的诚意感到满意,关系近了些。",
    "游民递给你一把干粮:「荒原上互相帮衬才能活下去。」",
  ],
  caravan: [
    "商队领队翻了翻账本,点头:「记下了,下次有货先给你留。」",
    "商队似乎认可了你的诚意,关系近了些。",
    "商队送你一小袋香料:「做生意讲的是长远。」",
  ],
  remnant: [
    "遗民代表沉吟片刻,低声:「我们记住了你的善意。」",
    "遗民似乎对你的诚意感到满意,关系近了些。",
    "遗民递给你一枚旧徽章:「这是旧时代的信物。」",
  ],
};

// Detect LLM error fallback text (same value as wish.js, set by llm-error-handler rule).
const LLM_ERROR_FALLBACK = "（旅人沉默片刻,点了点头）";

// ---- System: faction-tick — derive tier_<f> from relations JSON each tick ----
vitric.system("faction-tick", { query: ["Colony", "Faction"], writes: ["Faction"] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  let rel;
  try { rel = JSON.parse(c.Faction.relations || "{}"); } catch { rel = {}; }
  for (const f of FACTION_IDS) {
    const v = (rel[f] | 0);
    const tier = tierFromRelation(v);
    const field = "tier_" + f;
    if (c.Faction[field] !== tier) {
      c.Faction[field] = tier;
    }
  }
});

// ---- Fn: change_relation — apply delta to a faction, clamp, emit relation-change ----
vitric.fn("change_relation", (a, ctx) => {
  const f = (a.faction || "").toString();
  if (!FACTION_IDS.includes(f)) return;
  const delta = +a.delta || 0;
  if (delta === 0) return;
  // Read current relations JSON from colony.
  const curJson = ctx.getField("colony", "Faction.relations") || "{}";
  let rel;
  try { rel = JSON.parse(curJson); } catch { rel = {}; }
  const cur = (rel[f] | 0);
  const next = Math.max(-100, Math.min(100, cur + delta));
  if (next === cur) return; // no change (clamped)
  rel[f] = next;
  ctx.setField("colony", "Faction.relations", JSON.stringify(rel));
  ctx.emit("relation-change", { faction: f, old: cur, new: next, delta: delta });
});

// ---- Fn: complete_trade — barter give→receive, +2 relation to faction ----
// Inventory is passed via args (rule reads @player.Inventory.* and passes full set).
// Emits inv-set{...absolute values...} which the existing inv-apply rule writes back.
vitric.fn("complete_trade", (a, ctx) => {
  const f = (a.faction || "").toString();
  if (!FACTION_IDS.includes(f)) return;
  const offer = TRADE_OFFERS[f];
  if (!offer) return;
  // Read inventory from args (rule passes full inventory).
  const inv = {
    ore: a.ore|0, wood: a.wood|0, fiber: a.fiber|0, seed: a.seed|0,
    wheat: a.wheat|0, plank: a.plank|0, chair: a.chair|0, lamp: a.lamp|0,
    hide: a.hide|0, crystal_core: a.crystal_core|0,
  };
  // Verify player has enough give_kind.
  if (inv[offer.give_kind] < offer.give_n) {
    ctx.emit("toast-show", { text: "材料不足,无法交易" });
    return;
  }
  // Compute receive quantity with rate multiplier.
  const tier = (ctx.getField("colony", "Faction.tier_" + f) || "wary").toString();
  const mult = RATE_MULT_BY_TIER[tier] || 1.0;
  const receiveN = Math.max(1, Math.round(offer.base_receive_n * mult));
  // Apply: deduct give, add receive.
  inv[offer.give_kind] -= offer.give_n;
  inv[offer.receive_kind] += receiveN;
  // Emit inv-set with new absolute values (inv-apply rule writes back).
  ctx.emit("inv-set", {
    ore: inv.ore, wood: inv.wood, fiber: inv.fiber, seed: inv.seed,
    wheat: inv.wheat, plank: inv.plank, chair: inv.chair, lamp: inv.lamp,
    hide: inv.hide, crystal_core: inv.crystal_core,
  });
  // +2 relation to this faction (spec §4.5 line 281).
  // Inline the relation change (don't recursively call change_relation — keep it one fn).
  const curJson = ctx.getField("colony", "Faction.relations") || "{}";
  let rel;
  try { rel = JSON.parse(curJson); } catch { rel = {}; }
  const cur = (rel[f] | 0);
  const next = Math.max(-100, Math.min(100, cur + 2));
  rel[f] = next;
  ctx.setField("colony", "Faction.relations", JSON.stringify(rel));
  ctx.emit("relation-change", { faction: f, old: cur, new: next, delta: 2 });
  ctx.emit("toast-show", { text: "交易完成: " + offer.give_n + " " + offer.give_kind + " → " + receiveN + " " + offer.receive_kind });
});

// ---- Fn: negotiate — LLM negotiation with deterministic fallback ----
// Mirrors triggerWishMemory / onWishMemoryReply pattern in wish.js.
vitric.fn("negotiate", (a, ctx) => {
  const f = (a.faction || "").toString();
  if (!FACTION_IDS.includes(f)) return;
  const info = FACTION_INFO[f];
  const prompt = [
    "你是荒星上一个派系的代表:" + info.name + "。",
    "玩家前来谈判,希望改善关系。",
    "请用 1-2 句话回应玩家的诚意,语气符合派系特点,不要超过 50 字。",
    "直接输出对话内容,不要加引号或前缀。",
  ].join("\n");
  // Stash target faction so the callback knows who to update.
  ctx.setField("colony", "Colony._negotiate_target", f);
  ctx.ask("llm", prompt, "onNegotiateReply");
});

// LLM callback: applies +3 relation to stashed target faction + toast.
vitric.fn("onNegotiateReply", (reply, ctx) => {
  const f = ctx.getField("colony", "Colony._negotiate_target") || "";
  if (!f) return;
  let text = (reply && reply.text) || "";
  if (!text || text === LLM_ERROR_FALLBACK) {
    const list = NEGOTIATION_FALLBACKS[f] || NEGOTIATION_FALLBACKS.nomads;
    // Pick line based on current relation (deterministic — not random).
    const curJson = ctx.getField("colony", "Faction.relations") || "{}";
    let rel;
    try { rel = JSON.parse(curJson); } catch { rel = {}; }
    const cur = (rel[f] | 0);
    text = list[((cur % list.length) + list.length) % list.length];
  }
  // Apply +3 relation (fixed — LLM only controls flavor text, not mechanics).
  const curJson = ctx.getField("colony", "Faction.relations") || "{}";
  let rel;
  try { rel = JSON.parse(curJson); } catch { rel = {}; }
  const cur = (rel[f] | 0);
  const next = Math.max(-100, Math.min(100, cur + 3));
  rel[f] = next;
  ctx.setField("colony", "Faction.relations", JSON.stringify(rel));
  ctx.emit("relation-change", { faction: f, old: cur, new: next, delta: 3 });
  ctx.emit("toast-show", { text: text });
  // Clear stash.
  ctx.setField("colony", "Colony._negotiate_target", "");
});
```

## New rules `games/frontier/rules/faction.json`

```json
{
  "rules": [
    {
      "id": "trader-companion-relation",
      "comment": "Task 9 trader companion emits trade-available → +1 nomad relation (trader maintains nomad ties).",
      "on": { "event": "trade-available" },
      "do": [ { "call": "change_relation", "with": { "faction": "nomads", "delta": 1 } } ]
    },
    {
      "id": "negotiate-nomads",
      "comment": "Trade menu 谈判按钮 → negotiate with nomads.",
      "on": { "event": "ui-activate", "filter": { "action": "negotiate-nomads" } },
      "do": [ { "call": "negotiate", "with": { "faction": "nomads" } } ]
    },
    {
      "id": "negotiate-caravan",
      "on": { "event": "ui-activate", "filter": { "action": "negotiate-caravan" } },
      "do": [ { "call": "negotiate", "with": { "faction": "caravan" } } ]
    },
    {
      "id": "negotiate-remnant",
      "on": { "event": "ui-activate", "filter": { "action": "negotiate-remnant" } },
      "do": [ { "call": "negotiate", "with": { "faction": "remnant" } } ]
    },
    {
      "id": "faction-allied-notify",
      "comment": "When relation transitions INTO allied (76+), emit faction-allied (forward-compat hook for Task 12 region unlock).",
      "on": { "event": "relation-change" },
      "if": [ ["event.new", ">=", 76], ["event.old", "<", 76] ],
      "do": [ { "set": "@toast_lbl.UiLabel.content", "to": { "format": "{} 派系达成同盟!", "args": ["event.faction"] } }, { "set": "@toast_lbl.Toast.timer", "to": 3.0 } ]
    },
    {
      "id": "faction-reinforcements-hook",
      "comment": "Forward-compat: on night-fall, if any faction is allied, emit faction-reinforcements-available (Task 10/13 can consume for joint defense). Deterministic — no logic, just a hook.",
      "on": { "event": "night-fall" },
      "do": [ { "call": "emit_reinforcement_hook", "with": {} } ]
    }
  ]
}
```

The `emit_reinforcement_hook` fn (small helper in faction.js, ADD to the script above):

```javascript
vitric.fn("emit_reinforcement_hook", (a, ctx) => {
  // Check if any faction is allied; emit hook event (no game-mechanical effect in Task 11).
  const tiers = [
    ctx.getField("colony", "Faction.tier_nomads"),
    ctx.getField("colony", "Faction.tier_caravan"),
    ctx.getField("colony", "Faction.tier_remnant"),
  ];
  if (tiers.some(t => t === "allied")) {
    ctx.emit("faction-reinforcements-available", {});
  }
});
```

## New rules `games/frontier/rules/trade.json`

```json
{
  "rules": [
    {
      "id": "trade-nomads",
      "comment": "Trade menu: trade with nomads (give 3 wheat → receive 2 fiber base). Passes full inventory.",
      "on": { "event": "ui-activate", "filter": { "action": "trade-nomads" } },
      "do": [ { "call": "complete_trade", "with": {
        "faction": "nomads",
        "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
        "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
        "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp",
        "hide": "@player.Inventory.hide", "crystal_core": "@player.Inventory.crystal_core"
      } } ]
    },
    {
      "id": "trade-caravan",
      "on": { "event": "ui-activate", "filter": { "action": "trade-caravan" } },
      "do": [ { "call": "complete_trade", "with": {
        "faction": "caravan",
        "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
        "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
        "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp",
        "hide": "@player.Inventory.hide", "crystal_core": "@player.Inventory.crystal_core"
      } } ]
    },
    {
      "id": "trade-remnant",
      "on": { "event": "ui-activate", "filter": { "action": "trade-remnant" } },
      "do": [ { "call": "complete_trade", "with": {
        "faction": "remnant",
        "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
        "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
        "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp",
        "hide": "@player.Inventory.hide", "crystal_core": "@player.Inventory.crystal_core"
      } } ]
    }
  ]
}
```

## Modify `games/frontier/rules/ui.json`

Add `mode-trade` (ui-activate action=mode-trade) + `kb-mode-trade` (input action=b pressed). Mirror the existing `mode-combat` / `kb-mode-combat` pattern.

```json
{
  "id": "mode-trade",
  "comment": "切交易模式:显 trade_menu,藏 build/craft/tech 菜单。",
  "on": { "event": "ui-activate", "filter": { "action": "mode-trade" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "trade" },
    { "set": "@trade_menu.Ui.ox", "to": 208 },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 },
    { "set": "@tech_menu.Ui.ox", "to": -3000 }
  ]
},
{
  "id": "kb-mode-trade",
  "comment": "B 键切交易模式:显 trade_menu,藏其他菜单。",
  "on": { "event": "input", "filter": { "action": "b", "phase": "pressed" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "trade" },
    { "set": "@trade_menu.Ui.ox", "to": 208 },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 },
    { "set": "@tech_menu.Ui.ox", "to": -3000 }
  ]
}
```

ALSO extend the existing `mode-build` / `mode-craft` / `mode-interact` / `mode-research` / `mode-combat` / `kb-mode-*` rules to hide `trade_menu` (add `{ "set": "@trade_menu.Ui.ox", "to": -3000 }` to each). This is mechanical — copy the pattern. Same as Task 10 did when adding combat (it added `@tech_menu.Ui.ox` hide to existing mode rules).

## Modify `games/frontier/rules/hud.json`

Add `hud-faction-relation` rule:

```json
{
  "id": "hud-faction-relation",
  "comment": "派系关系 HUD:游民 N 商队 N 遗民 N。每帧刷 @colony.Faction.relations (JSON text, parse in fn — but rule engine can't parse JSON, so we read derived tier_<f> fields instead for compact display).",
  "on": "tick",
  "do": [
    { "set": "@relation_lbl.UiLabel.content", "to": { "format": "游民[{}] 商队[{}] 遗民[{}]",
      "args": [ "@colony.Faction.tier_nomads", "@colony.Faction.tier_caravan", "@colony.Faction.tier_remnant" ] } }
  ]
}
```

## Scene changes `games/frontier/scenes/main.json`

### 1. Attach `Faction` component to `colony` entity

In the `colony` entity's `components` object, add:
```json
"Faction": {}
```
(Uses default relations JSON + default tiers. The `faction-tick` system will derive tiers on tick 1.)

### 2. Add `trade_menu` entity (mirrors `craft_menu` layout)

```json
{
  "name": "trade_menu",
  "components": {
    "Ui": { "anchor": "top-left", "parent": "ui", "ox": -3000, "oy": 176, "w": 280, "h": 360 },
    "UiLabel": { "content": "交易", "size": 24, "color": "#ffe9b0", "align": "center" }
  }
},
```

### 3. Add 3 trade buttons + 3 labels inside `trade_menu` (mirror `craft_plank` pattern)

For each faction `f` in {nomads, caravan, remnant}:

```json
{
  "name": "trade_<f>",
  "components": {
    "Ui": { "anchor": "top-left", "parent": "trade_menu", "w": 252, "h": 42 },
    "Button": { "action": "trade-<f>", "state": "normal" }
  }
},
{
  "name": "trade_<f>_lbl",
  "components": {
    "Ui": { "anchor": "stretch", "parent": "trade_<f>" },
    "UiLabel": { "content": "<faction_name>: <give_n> <give_kind> → <receive_n> <receive_kind>", "size": 22, "color": "#ffffff", "align": "center" }
  }
}
```

Specific labels:
- `trade_nomads_lbl`: "游民: 3 麦 → 2 纤维"
- `trade_caravan_lbl`: "商队: 2 板 → 1 皮"
- `trade_remnant_lbl`: "遗民: 2 皮 → 1 晶核"

### 4. Add 3 negotiate buttons + 3 labels inside `trade_menu` (below trade buttons)

For each faction `f`:

```json
{
  "name": "negotiate_<f>",
  "components": {
    "Ui": { "anchor": "top-left", "parent": "trade_menu", "w": 252, "h": 36 },
    "Button": { "action": "negotiate-<f>", "state": "normal" }
  }
},
{
  "name": "negotiate_<f>_lbl",
  "components": {
    "Ui": { "anchor": "stretch", "parent": "negotiate_<f>" },
    "UiLabel": { "content": "谈判-<faction_name>", "size": 20, "color": "#cfe6ff", "align": "center" }
  }
}
```

### 5. Add `mode_trade` button + label to `mode_row` HBox

Mirror `mode_combat` pattern:

```json
{
  "name": "mode_trade",
  "components": {
    "Ui": { "anchor": "top-left", "parent": "mode_row", "w": 92, "h": 48 },
    "Button": { "action": "mode-trade", "state": "normal" }
  }
},
{
  "name": "mode_trade_lbl",
  "components": {
    "Ui": { "anchor": "stretch", "parent": "mode_trade" },
    "UiLabel": { "content": "交易", "size": 24, "color": "#ffffff", "align": "center" }
  }
}
```

### 6. Bump `mode_row.Ui.w` from 582 → 680

Currently 582 fits 5 buttons (5×92 + 4×6 = 484, plus padding). Adding a 6th button: 6×92 + 5×6 = 582. Wait, that's exactly 582 — so 582 already fits 6. Actually let me recheck: the reviewer noted Task 10 set `mode_row.w=582` for 6 buttons but only 5 exist. So adding the 6th button now means 582 is exactly right. **Do NOT change `mode_row.Ui.w`** — it's already correct for 6 buttons. (Reviewer's Nit #2 in Task 10 review is resolved by this task adding the 6th button.)

### 7. Add `relation_lbl` HUD entity (top-right, below `hp_lbl`)

`hp_lbl` is at top-right oy:312, h:24. So `relation_lbl` at top-right oy:340 (312+24+4 gap = 340):

```json
{
  "name": "relation_lbl",
  "components": {
    "Ui": { "anchor": "top-right", "parent": "ui", "ox": -32, "oy": 340, "w": 360, "h": 24 },
    "UiLabel": { "content": "游民[wary] 商队[wary] 遗民[wary]", "size": 18, "color": "#cfe6ff", "align": "end" }
  }
}
```

## Modify `games/frontier/vitric.json`

Add `scripts/faction.js` to scripts array (after `combat.js`, before `economy.js` — actually doesn't matter for faction.js since it doesn't share globals with economy, but place it logically after `research.js` at the end). Add `rules/faction.json` + `rules/trade.json` to rules array.

Final scripts array:
```json
"scripts": [
  "scripts/colony.js",
  "scripts/combat.js",
  "scripts/economy.js",
  "scripts/crops.js",
  "scripts/companion.js",
  "scripts/clock.js",
  "scripts/hud.js",
  "scripts/toast.js",
  "scripts/flare.js",
  "scripts/poi.js",
  "scripts/wish.js",
  "scripts/research.js",
  "scripts/faction.js"
]
```

Final rules array:
```json
"rules": [
  "rules/move.json",
  "rules/ui.json",
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
  "rules/poi.json",
  "rules/affordability.json",
  "rules/wish.json",
  "rules/research.json",
  "rules/combat.json",
  "rules/faction.json",
  "rules/trade.json"
]
```

## Tests `crates/vitric-cli/tests/faction.rs` (NEW, 4 tests)

Follow the pattern of `combat.rs` / `companions.rs` / `research.rs`:

```rust
//! Task 11 (Trading & Diplomacy) integration tests.
//!
//! Verifies: (a) faction-tick derives tier_<f> from relations JSON;
//! (b) change_relation applies delta with clamping;
//! (c) complete_trade executes barter (deducts give, adds receive, +2 relation);
//! (d) onNegotiateReply applies +3 relation with fallback canned line.

use std::path::PathBuf;
use serde_json::json;
use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

fn set_field(sim: &mut vitric_sim::Sim, name: &str, path: &str, value: serde_json::Value) {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.set_field(id, path, value).expect("set_field ok");
}

fn get_field(sim: &vitric_sim::Sim, name: &str, path: &str) -> serde_json::Value {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.get_field(id, path).expect("get_field ok").clone()
}

#[test]
fn faction_tick_derives_tier_from_relations() {
    // Set Faction.relations to {"nomads":80,"caravan":50,"remnant":-60} →
    // tier_nomads="allied", tier_caravan="friendly", tier_remnant="hostile".
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Faction.relations", json!({"nomads":80,"caravan":50,"remnant":-60}));
    sim.step(&mut rt).unwrap(); // faction-tick runs

    assert_eq!(get_field(&sim, "colony", "Faction.tier_nomads").as_str(), Some("allied"));
    assert_eq!(get_field(&sim, "colony", "Faction.tier_caravan").as_str(), Some("friendly"));
    assert_eq!(get_field(&sim, "colony", "Faction.tier_remnant").as_str(), Some("hostile"));
}

#[test]
fn change_relation_clamps_to_100() {
    // Set nomads to 95, call change_relation(+10) → should clamp to 100.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Faction.relations", json!({"nomads":95,"caravan":0,"remnant":-10}));
    sim.inject_reply("ui-activate", json!({ "action": "negotiate-nomads" })); // placeholder
    // Actually call change_relation directly via a different mechanism.
    // Since change_relation is a fn, we need a rule to trigger it. Use a custom test rule? No —
    // the negotiate-nomads rule calls negotiate, not change_relation. Use a different approach:
    // step once to let faction-tick run, then check tier.
    // Actually to test change_relation, we need to trigger it. The trader-companion-relation rule
    // calls change_relation on trade-available event. Inject that event.
    sim.inject_reply("trade-available", json!({ "pid": "test", "role": "trader" }));
    sim.step(&mut rt).unwrap(); // trader-companion-relation fires → +1 nomads

    let rel = get_field(&sim, "colony", "Faction.relations").as_str().unwrap();
    // 95 + 1 = 96 (not clamped yet).
    assert!(rel.contains("\"nomads\":96"), "expected nomads=96, got {}", rel);
}
```

Actually, for the clamp test, set nomads to 99 and inject trade-available → +1 → 100. Then inject again → +1 → clamped to 100 (no change). Test:
```rust
#[test]
fn change_relation_clamps_to_100() {
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Faction.relations", json!({"nomads":99,"caravan":0,"remnant":-10}));
    sim.inject_reply("trade-available", json!({ "pid": "test", "role": "trader" }));
    sim.step(&mut rt).unwrap(); // +1 → 100
    let rel = get_field(&sim, "colony", "Faction.relations").as_str().unwrap();
    assert!(rel.contains("\"nomads\":100"), "expected nomads=100, got {}", rel);

    // Inject again — should clamp (no change, no event).
    sim.inject_reply("trade-available", json!({ "pid": "test", "role": "trader" }));
    sim.step(&mut rt).unwrap();
    let rel2 = get_field(&sim, "colony", "Faction.relations").as_str().unwrap();
    assert!(rel2.contains("\"nomads\":100"), "expected nomads still 100 (clamped), got {}", rel2);
}
```

```rust
#[test]
fn complete_trade_barters_items_and_adds_relation() {
    // Player has 3 wheat. Trigger trade-nomads (give 3 wheat → receive 2 fiber base, neutral mult 1.0).
    // After: player.Inventory.wheat=0, player.Inventory.fiber=2, nomads relation +2.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "player", "Inventory.wheat", json!(3));
    set_field(&mut sim, "player", "Inventory.fiber", json!(0));
    set_field(&mut sim, "colony", "Faction.relations", json!({"nomads":30,"caravan":0,"remnant":-10}));
    sim.step(&mut rt).unwrap(); // let faction-tick derive tier_nomads=neutral

    sim.inject_reply("ui-activate", json!({ "action": "trade-nomads" }));
    sim.step(&mut rt).unwrap(); // complete_trade runs, emits inv-set
    sim.step(&mut rt).unwrap(); // inv-apply rule writes back inventory

    let wheat = get_field(&sim, "player", "Inventory.wheat").as_i64().unwrap();
    let fiber = get_field(&sim, "player", "Inventory.fiber").as_i64().unwrap();
    assert_eq!(wheat, 0, "wheat should be 0 after trade");
    assert_eq!(fiber, 2, "fiber should be 2 after trade");

    let rel = get_field(&sim, "colony", "Faction.relations").as_str().unwrap();
    assert!(rel.contains("\"nomads\":32"), "nomads relation should be 30+2=32, got {}", rel);
}
```

```rust
#[test]
fn on_negotiate_reply_applies_plus_3_relation() {
    // Call negotiate(nomads) → ctx.ask("llm") stashes target="nomads".
    // Engine routes llm-reply → __onReply → onNegotiateReply.
    // For test: directly inject llm-reply with fallback text → onNegotiateReply runs → +3 relation.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Faction.relations", json!({"nomads":30,"caravan":0,"remnant":-10}));

    sim.inject_reply("ui-activate", json!({ "action": "negotiate-nomads" }));
    sim.step(&mut rt).unwrap(); // negotiate fn runs, stashes target="nomads", emits ctx.ask("llm")

    // Inject LLM reply with fallback text → triggers onNegotiateReply.
    sim.inject_reply("llm-reply", json!({ "text": "（旅人沉默片刻,点了点头）" }));
    sim.step(&mut rt).unwrap(); // onNegotiateReply runs, detects fallback, substitutes canned line, +3 relation

    let rel = get_field(&sim, "colony", "Faction.relations").as_str().unwrap();
    assert!(rel.contains("\"nomads\":33"), "nomads relation should be 30+3=33, got {}", rel);

    // Stash should be cleared.
    let stash = get_field(&sim, "colony", "Colony._negotiate_target").as_str().unwrap();
    assert_eq!(stash, "", "negotiate target stash should be cleared after reply");
}
```

**Important test notes:**
- The `trade-available` event injection requires the rule `trader-companion-relation` to be registered. Verify `rules/faction.json` is loaded by `vitric.json`.
- The `llm-reply` event name: verify by checking how `wish.js`'s `onWishMemoryReply` is wired. Look at `rules/companion.json` for the `llm-error-handler` rule (or wherever llm-reply → __onReply routing happens). If the event name is different, adjust the test.
- All tests use `Runtime::boot(&frontier_dir())` which loads the full game — pattern from `research.rs`.

## Critical reminders

1. **All code comments MUST be English** (`//`, `/* */`). String literals (toast text, UI labels, fallback memories in Chinese) keep their authored language.
2. **Every field read by a rule OR accessed via `ctx.getField`/`ctx.setField` MUST be declared in `schema.json`**:
   - `Faction.relations`, `Faction.tier_nomads`, `Faction.tier_caravan`, `Faction.tier_remnant` (NEW component)
   - `Colony._negotiate_target` (NEW field on Colony)
   - `Mode.value` enum extended with `"trade"` variant
   - Pre-existing fields re-used: `Inventory.{ore,wood,fiber,seed,wheat,plank,chair,lamp,hide,crystal_core}` (already declared Task 8), `Colony.companion_handles` (already declared)
3. **No fake APIs**: use only `vitric.system`, `vitric.fn`, `ctx.dt`, `ctx.random()`, `ctx.spawn`, `ctx.emit`, `ctx.getField`, `ctx.setField`, `ctx.ask`, `e.id`, `e.<Comp>.<field>`. No `ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`.
4. **Use `~/.cargo/bin/cargo` for all cargo commands** (project convention).
5. **Do NOT update `progress.md`** — controller does that after review.
6. **Do NOT re-record `qa/clear.json`** — Task 15 handles that. Gate is EXPECTED-FAIL (ReplayDiverged at tick 0) because adding `Faction` to `colony` + new HUD entities changes tick-0 world hash.
7. **Commit message format**: `feat(frontier): trading & diplomacy — Faction component, 3 factions, barter trades, LLM negotiation`
8. **Commit ONLY the in-scope files** (list below). Do NOT commit `progress.md` or `qa/clear.json`.
9. **`faction.js` load order**: doesn't matter for shared globals (faction.js is self-contained — declares its own constants). Place at END of scripts array (after `research.js`).
10. **Verify `inv-apply` rule in `rules/economy.json` already covers all 10 inventory fields** (ore/wood/fiber/seed/wheat/plank/chair/lamp/hide/crystal_core). If yes, the `complete_trade` fn's `inv-set` emit will be written back correctly. If not, EXTEND `inv-apply` — but Task 8 should have already done this.
11. **UI layout audit**: `relation_lbl` at top-right oy:340 must NOT overlap with `hp_lbl` (oy:312, h:24, ends at 336) — 4px gap. `mode_trade` button in `mode_row` HBox — HBox auto-distributes, no manual positioning. `trade_menu` at top-left oy:176, w:280, h:360 — overlaps with `build_menu` (oy:176, w:348, h:700) BUT they're never shown simultaneously (mode rules hide one when showing the other). Same pattern as `craft_menu` / `tech_menu`.
12. **`relation_lbl` width**: 360px to fit "游民[neutral] 商队[friendly] 遗民[wary]" comfortably. Align "end" (right-aligned) to match `hp_lbl` / `collective_wish_lbl` pattern.

## Files to create/modify (in scope)

| # | File | Status | Purpose |
|---|---|---|---|
| 1 | `games/frontier/schema.json` | modified | Add `Faction` component (4 fields); extend `Colony._negotiate_target`; extend `Mode.value` enum with `"trade"` variant |
| 2 | `games/frontier/scenes/main.json` | modified | Attach `Faction` to `colony`; add `trade_menu` + 6 buttons + 6 labels + `mode_trade` + `mode_trade_lbl` + `relation_lbl` HUD |
| 3 | `games/frontier/scripts/faction.js` | new | All faction systems + fns (faction-tick, change_relation, complete_trade, negotiate, onNegotiateReply, emit_reinforcement_hook) |
| 4 | `games/frontier/rules/faction.json` | new | 6 rules (trader-companion-relation, 3× negotiate-*, faction-allied-notify, faction-reinforcements-hook) |
| 5 | `games/frontier/rules/trade.json` | new | 3 rules (trade-nomads, trade-caravan, trade-remnant) |
| 6 | `games/frontier/rules/ui.json` | modified | Add `mode-trade` + `kb-mode-trade`; extend existing mode rules to hide `trade_menu` |
| 7 | `games/frontier/rules/hud.json` | modified | Add `hud-faction-relation` rule |
| 8 | `games/frontier/vitric.json` | modified | Register `scripts/faction.js` + `rules/faction.json` + `rules/trade.json` |
| 9 | `crates/vitric-cli/tests/faction.rs` | new | 4 integration tests |

## Verification commands (run all before committing)

```bash
~/.cargo/bin/cargo test -p vitric-cli --test faction
~/.cargo/bin/cargo test -p vitric-cli --test combat
~/.cargo/bin/cargo test -p vitric-cli --test research
~/.cargo/bin/cargo test -p vitric-cli --test seasons
~/.cargo/bin/cargo test -p vitric-cli --test companions
~/.cargo/bin/cargo test -p vitric-cli --test region
~/.cargo/bin/cargo run --release -- check games/frontier
~/.cargo/bin/cargo run --release -- gate games/frontier  # EXPECTED-FAIL at tick 0
```

## Commit

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json games/frontier/scripts/faction.js games/frontier/rules/faction.json games/frontier/rules/trade.json games/frontier/rules/ui.json games/frontier/rules/hud.json games/frontier/vitric.json crates/vitric-cli/tests/faction.rs

git commit -m "feat(frontier): trading & diplomacy — Faction component, 3 factions, barter trades, LLM negotiation

Add Faction component (relations JSON + 3 derived tier fields). 3 factions
(nomads/caravan/remnant) with starting relations. faction-tick derives tier
each tick. complete_trade barters items + +2 relation. negotiate uses
ctx.ask(llm) with deterministic fallback (+3 relation). Trade menu UI with
6 buttons. B key + mode_trade button. Forward-compat: faction-allied event
(Task 12 region unlock), faction-reinforcements-available (Task 13 joint defense)."

git push origin main
```

## Self-audit checklist (apply before reporting complete)

Use `.superpowers/sdd/review-checklist.md` sections 1-5:

1. **Schema field audit**: every field read by a rule or `ctx.getField`/`ctx.setField` is declared. Pay special attention to `Faction.*`, `Colony._negotiate_target`, `Mode.value` ("trade" variant).
2. **Enum variant audit**: `Mode.value ← "trade"` declared in schema.
3. **Scene entity reference audit**: every `@<name>` in new rules corresponds to an entity in `scenes/main.json`. Verify: `@trade_menu`, `@relation_lbl`, `@mode_trade` (NEW); `@player`, `@colony`, `@uistate`, `@toast_lbl`, `@build_menu`, `@craft_menu`, `@tech_menu` (pre-existing).
4. **UI layout overlap audit**: `relation_lbl` (top-right oy:340, h:24) doesn't overlap `hp_lbl` (oy:312, h:24, ends 336). `mode_trade` in `mode_row` HBox (auto-distributed). `trade_menu` (top-left oy:176, w:280, h:360) overlaps `build_menu` geometrically but never shown simultaneously (mode rules).
5. **Standard checks**: schema check exit 0; comments English; no fake APIs; commit message format; only in-scope files modified.

## Report

After commit + push, write `.superpowers/sdd/briefs/task-11-report.md` with:
- Commit hash
- Files changed (count + list)
- Test results (all suites)
- Deviations from brief (if any)
- Schema field audit result
- Concerns / known issues
