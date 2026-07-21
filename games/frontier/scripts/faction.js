// Faction system (Task 11): 3 factions, relation tracking, tier derivation,
// barter trades, LLM negotiation with deterministic fallback.
//
// Systems:
//   faction-tick    Colony+Faction -> derive tier_<f> from relations JSON each tick
//
// Fns (called by rules in rules/faction.json + rules/trade.json):
//   change_relation    (faction, delta) -> clamp [-100,100], emit relation-change
//   complete_trade     (faction, give_kind, give_n, receive_kind, receive_n) -> barter + +2 relation
//   negotiate          (faction) -> ctx.ask("llm") with onNegotiateReply callback
//   onNegotiateReply   (reply) -> apply +3 relation to stashed target, fallback canned line
//   emit_reinforcement_hook  () -> if any faction allied on night-fall, emit faction-reinforcements-available
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

// LLM_ERROR_FALLBACK is declared in wish.js (shared QuickJS global scope); reused here.
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

// ---- Fn: complete_trade — barter give->receive, +2 relation to faction ----
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

// ---- Fn: emit_reinforcement_hook — forward-compat night-fall hook for allied factions ----
// Emits faction-reinforcements-available if any faction is allied. No game-mechanical effect
// in Task 11; Task 10's spawn_wave or Task 13 can consume this for joint defense flavor.
vitric.fn("emit_reinforcement_hook", (a, ctx) => {
  const tiers = [
    ctx.getField("colony", "Faction.tier_nomads"),
    ctx.getField("colony", "Faction.tier_caravan"),
    ctx.getField("colony", "Faction.tier_remnant"),
  ];
  if (tiers.some(t => t === "allied")) {
    ctx.emit("faction-reinforcements-available", {});
  }
});
