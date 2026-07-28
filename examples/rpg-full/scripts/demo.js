// rpg-full script — composes all 12 modules into a complete RPG:
// inventory + quest + dialogue + game-flow + combat + progression + loot +
// shop + equipment + status-effects + skills + crafting.
//
// Game loop: title → talk to elder → accept wolf quest → craft sword →
// equip sword → fight wolf (wolf attacks + poisons player, player attacks +
// casts fireball/heal) → wolf dies → loot drops wolf_pelt → quest auto-
// completes → turn in to elder → win. Player can also buy potions from shop.
//
// This script handles: movement, stash/reset, equip bonus, level-up bonus,
// potion consumption, and HUD rendering. The 12 modules handle their own
// lifecycle — this script only bridges modules (e.g. equip bonus bridges
// equipment → combat by modifying Attack.power).

const PLAYER_START = { x: 0, y: 0 };
const PLAYER_MAX_HP = 100;
const PLAYER_BASE_ATK = 10;
const PLAYER_MAX_MANA = 100;
const PLAYER_XP_THRESHOLD = 100;

const WOLF_HOME = { x: 1, y: 2 };
const WOLF_MAX_HP = 80;

const START_INVENTORY = {
  items: ["iron", "wood", "coin"],
  counts: [3, 1, 5],
};

// ---- movement: apply input direction to Velocity ----
vitric.fn("move", (args, ctx) => {
  const axis = args.axis || "x";
  const dir = Number(args.dir) || 0;
  const speed = ctx.getField("@player", "Speed.value") || 60;
  ctx.setField("@player", "Velocity." + axis, dir * speed);
});

// ---- stash dead wolf off-screen (keep entity for restart) ----
vitric.fn("stash_wolf", (args, ctx) => {
  const wolf = args.wolf;
  if (!wolf) return;
  ctx.setField(wolf, "Position.x", -100);
  ctx.setField(wolf, "Position.y", -100);
});

// ---- equip bonus: +15 ATK for sword (bridges equipment → combat) ----
function bonusFor(item) {
  switch (item) {
    case "sword": return 15;
    default: return 0;
  }
}

vitric.fn("apply_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const bonus = bonusFor(item);
  if (bonus !== 0) {
    const power = Number(ctx.getField(who, "Attack.power")) || 0;
    ctx.setField(who, "Attack.power", power + bonus);
  }
});

vitric.fn("remove_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const bonus = bonusFor(item);
  if (bonus !== 0) {
    const power = Number(ctx.getField(who, "Attack.power")) || 0;
    ctx.setField(who, "Attack.power", Math.max(0, power - bonus));
  }
});

// ---- level-up bonus: +20 max HP (full heal), +10 ATK (bridges progression → combat) ----
vitric.fn("apply_level_up_bonus", (args, ctx) => {
  const who = args.who;
  if (!who) throw new Error("apply_level_up_bonus: missing who");
  const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
  const newMax = maxHp + 20;
  ctx.setField(who, "Health.max", newMax);
  ctx.setField(who, "Health.hp", newMax);
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", power + 10);
});

// ---- use potion: consume 1 potion from inventory, emit heal ----
vitric.fn("use_potion", (args, ctx) => {
  const who = args.who;
  if (!who) throw new Error("use_potion: missing who");
  const items = (ctx.getField(who, "Inventory.items") || []).slice();
  const counts = (ctx.getField(who, "Inventory.counts") || []).slice();
  const idx = items.indexOf("potion");
  if (idx < 0) return; // no potion — silent no-op

  counts[idx] = (Number(counts[idx]) || 0) - 1;
  if (counts[idx] <= 0) {
    items.splice(idx, 1);
    counts.splice(idx, 1);
  }
  ctx.setField(who, "Inventory.items", items);
  ctx.setField(who, "Inventory.counts", counts);
  ctx.emit("heal", { who, amount: 30 });
});

// ---- reset game: restore all state for a fresh run ----
vitric.fn("reset_game", (_args, ctx) => {
  // Player position and velocity.
  ctx.setField("@player", "Position.x", PLAYER_START.x);
  ctx.setField("@player", "Position.y", PLAYER_START.y);
  ctx.setField("@player", "Velocity.x", 0);
  ctx.setField("@player", "Velocity.y", 0);

  // Combat stats.
  ctx.setField("@player", "Health.hp", PLAYER_MAX_HP);
  ctx.setField("@player", "Health.max", PLAYER_MAX_HP);
  ctx.setField("@player", "Attack.power", PLAYER_BASE_ATK);

  // Mana.
  ctx.setField("@player", "Mana.current", PLAYER_MAX_MANA);
  ctx.setField("@player", "Mana.max", PLAYER_MAX_MANA);

  // Inventory.
  ctx.setField("@player", "Inventory.items", START_INVENTORY.items.slice());
  ctx.setField("@player", "Inventory.counts", START_INVENTORY.counts.slice());

  // Equipment.
  ctx.setField("@player", "Equipment.slots", ["weapon"]);
  ctx.setField("@player", "Equipment.items", [""]);

  // Abilities (cooldowns reset to 0).
  ctx.setField("@player", "Abilities.cooldowns", [0, 0]);

  // Status effects cleared.
  ctx.setField("@player", "StatusEffects.effects", []);
  ctx.setField("@player", "StatusEffects.durations", []);
  ctx.setField("@player", "StatusEffects.magnitudes", []);

  // Progression.
  ctx.setField("@player", "XP.current", 0);
  ctx.setField("@player", "XP.threshold", PLAYER_XP_THRESHOLD);
  ctx.setField("@player", "Level.value", 1);
  ctx.setField("@player", "Level.points", 0);

  // Quest log.
  ctx.setField("@player", "QuestLog.active", []);
  ctx.setField("@player", "QuestLog.completed", []);

  // Dialogue.
  ctx.setField("@player", "DialogueRunner.active_npc", "");
  ctx.setField("@player", "DialogueRunner.current", -1);

  // Wolf: revive and move home.
  ctx.setField("@wolf", "Health.hp", WOLF_MAX_HP);
  ctx.setField("@wolf", "Position.x", WOLF_HOME.x);
  ctx.setField("@wolf", "Position.y", WOLF_HOME.y);
  ctx.setField("@wolf", "StatusEffects.effects", []);
  ctx.setField("@wolf", "StatusEffects.durations", []);
  ctx.setField("@wolf", "StatusEffects.magnitudes", []);

  // Quest state.
  ctx.setField("@wolf-quest", "QuestState.state", "inactive");
  ctx.setField("@wolf-quest", "QuestState.progress", 0);
  ctx.setField("@wolf-quest", "QuestState.assignee", "");

  // Elder talk counter.
  ctx.setField("@elder", "Talked.count", 0);

  // Emit game-restart — game-flow module resets phase/time/score.
  ctx.emit("game-restart", {});
});

// ---- HUD: show all game state ----
vitric.fn("render_hud", (args, ctx) => {
  const game = args.game;
  const quest = args.quest;
  const who = args.who;
  const wolf = args.wolf;
  const hud = args.hud;
  if (!game || !hud) throw new Error("render_hud: missing game/hud");

  const phase = ctx.getField(game, "GameState.phase") || "title";
  const time = ctx.getField(game, "GameState.time") || 0;

  if (phase === "title") {
    ctx.setField(hud, "Text.content", "RPG FULL — Press SPACE to start | 12 modules composed");
    return;
  }

  if (phase === "won") {
    ctx.setField(hud, "Text.content", "YOU WIN! Cleared in " + time + " ticks. Press R to restart.");
    return;
  }

  if (phase === "lost") {
    ctx.setField(hud, "Text.content", "GAME OVER! Press R to restart.");
    return;
  }

  // Playing phase — show full state.
  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || 0;
  const mana = ctx.getField(who, "Mana.current") || 0;
  const atk = ctx.getField(who, "Attack.power") || 0;
  const lvl = ctx.getField(who, "Level.value") || 1;
  const qState = ctx.getField(quest, "QuestState.state") || "inactive";

  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];
  let invText = items.length === 0
    ? "empty"
    : items.map(function (it, i) { return it + "x" + counts[i]; }).join(", ");

  const slots = ctx.getField(who, "Equipment.slots") || [];
  const equipped = ctx.getField(who, "Equipment.items") || [];
  const eqText = slots.map(function (s, i) { return s + ":" + (equipped[i] || "-"); }).join(", ");

  const effs = ctx.getField(who, "StatusEffects.effects") || [];
  const statusText = effs.length === 0 ? "none" : effs.join(",");

  const wolfHp = ctx.getField(wolf, "Health.hp") || 0;

  const text = "Lv" + lvl + " HP:" + hp + "/" + maxHp + " MP:" + mana + " ATK:" + atk +
    " Quest:" + qState + " Wolf:" + wolfHp + "hp" +
    " Inv:[" + invText + "] Eq:[" + eqText + "] Status:[" + statusText + "]" +
    " | F:fireball G:heal C:craft E:equip Q:unequip B:buy H:potion X:attack";
  ctx.setField(hud, "Text.content", text);
});
