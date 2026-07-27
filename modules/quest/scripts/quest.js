// Quest module — data-driven quest system with state machine + objective tracking.
//
// A quest is an ENTITY carrying three components:
//   QuestDef      — static: id / title / desc / prereq / reward
//   QuestObjective — static: kind(collect|talk) / arg / target
//   QuestState    — mutable: state / progress / assignee
//
// State machine: inactive → offered → active → completed → turned-in
//   (any state may go to `failed` via a future hook; not driven by this module yet)
//
// The player (or any quest-taking entity) carries a QuestLog component:
//   QuestLog.active     — list of quest ids currently accepted
//   QuestLog.completed  — list of quest ids turned in
//
// Objective tracking is a TICK SYSTEM (quest-track). The rule DSL can't iterate
// quest entities on an event trigger, so progress is recomputed each tick from
// world state: collect objectives read the assignee's Inventory (composes with
// the inventory module — soft dependency, no error if absent), talk objectives
// read the NPC's Talked.count.
//
// State transitions (offer / accept / turn-in) are EVENT-driven via rules calling
// these fns. Rewards are granted by emitting `pickup` — which the inventory module
// handles. This is the module-composition seam: quest + inventory = full loop.
//
// Events emitted by this module:
//   quest-offered    { quest, id }           — inactive → offered
//   quest-locked     { quest, id, missing }  — prereqs not met (offer refused)
//   quest-accepted   { quest, id, who }      — offered → active
//   quest-completed  { quest, id, who }      — active → completed (objective done)
//   quest-turned-in  { quest, id, who }      — completed → turned-in (reward granted)

// ---- tick system: advance active quest progress from world state ----
vitric.system(
  "quest-track",
  { query: ["QuestDef", "QuestState", "QuestObjective"], writes: ["QuestState"] },
  (entities, ctx) => {
    for (const e of entities) {
      const st = e.QuestState;
      if (st.state !== "active") continue;
      const def = e.QuestDef;
      const obj = e.QuestObjective;
      const who = st.assignee || "@player";
      let progress = st.progress;

      if (obj.kind === "collect") {
        // Soft-depends on the inventory module: reads Inventory.items/counts.
        // If the inventory module isn't included, these reads return undefined → progress stays 0.
        const items = ctx.getField(who, "Inventory.items") || [];
        const counts = ctx.getField(who, "Inventory.counts") || [];
        const idx = items.indexOf(obj.arg);
        const have = idx >= 0 ? Number(counts[idx]) || 0 : 0;
        progress = Math.min(have, obj.target);
      } else if (obj.kind === "talk") {
        // `arg` is the NPC entity name. Read its Talked.count; if > 0, objective is done.
        const talked = ctx.getField(obj.arg, "Talked.count");
        progress = talked && talked > 0 ? obj.target : 0;
      }

      if (progress !== st.progress) {
        ctx.setField(e.id, "QuestState.progress", progress);
      }
      if (progress >= obj.target) {
        ctx.setField(e.id, "QuestState.state", "completed");
        ctx.emit("quest-completed", { quest: e.id, id: def.id, who });
      }
    }
  },
);

// ---- inactive → offered (checks prereqs) ----
vitric.fn("__quest_offer", (args, ctx) => {
  const quest = args.quest;
  if (!quest) throw new Error("__quest_offer: 缺少 quest（实体名或句柄）");

  const state = ctx.getField(quest, "QuestState.state");
  if (state !== "inactive") return; // idempotent: only offer once

  const id = ctx.getField(quest, "QuestDef.id") || quest;
  const prereq = ctx.getField(quest, "QuestDef.prereq") || [];

  // Prereqs are quest ids that must be in the player's completed list.
  // Single-player default: check @player.QuestLog.completed.
  const completed = ctx.getField("@player", "QuestLog.completed") || [];
  const missing = prereq.filter((p) => !completed.includes(p));
  if (missing.length > 0) {
    ctx.emit("quest-locked", { quest, id, missing });
    return;
  }

  ctx.setField(quest, "QuestState.state", "offered");
  ctx.emit("quest-offered", { quest, id });
});

// ---- offered → active (assigns to a player) ----
vitric.fn("__quest_accept", (args, ctx) => {
  const quest = args.quest;
  const who = args.who || "@player";
  if (!quest) throw new Error("__quest_accept: 缺少 quest");

  const state = ctx.getField(quest, "QuestState.state");
  if (state !== "offered") return; // idempotent: only accept from offered

  const id = ctx.getField(quest, "QuestDef.id") || quest;

  ctx.setField(quest, "QuestState.state", "active");
  ctx.setField(quest, "QuestState.assignee", who);

  // Add quest id to the player's active list.
  const active = (ctx.getField(who, "QuestLog.active") || []).slice();
  if (!active.includes(id)) active.push(id);
  ctx.setField(who, "QuestLog.active", active);

  ctx.emit("quest-accepted", { quest, id, who });
});

// ---- completed → turned-in (grants rewards) ----
vitric.fn("__quest_turn_in", (args, ctx) => {
  const quest = args.quest;
  const who = args.who || "@player";
  if (!quest) throw new Error("__quest_turn_in: 缺少 quest");

  const state = ctx.getField(quest, "QuestState.state");
  if (state !== "completed") return; // idempotent: only turn in from completed

  const id = ctx.getField(quest, "QuestDef.id") || quest;
  const rewardItem = ctx.getField(quest, "QuestDef.reward_item") || "";
  const rewardCount = Number(ctx.getField(quest, "QuestDef.reward_count")) || 0;

  ctx.setField(quest, "QuestState.state", "turned-in");

  // Move quest id from active to completed on the player's log.
  const active = (ctx.getField(who, "QuestLog.active") || []).slice();
  const completed = (ctx.getField(who, "QuestLog.completed") || []).slice();
  const idx = active.indexOf(id);
  if (idx >= 0) active.splice(idx, 1);
  if (!completed.includes(id)) completed.push(id);
  ctx.setField(who, "QuestLog.active", active);
  ctx.setField(who, "QuestLog.completed", completed);

  // Grant item reward by emitting `pickup` — the inventory module handles it.
  // Soft dependency: if inventory module isn't included, the pickup event is simply
  // unhandled (no listener); the game can still observe quest-turned-in to grant
  // rewards its own way.
  if (rewardItem && rewardCount > 0) {
    ctx.emit("pickup", { who, item: rewardItem, count: rewardCount });
  }

  ctx.emit("quest-turned-in", { quest, id, who, reward_item: rewardItem, reward_count: rewardCount });
});
