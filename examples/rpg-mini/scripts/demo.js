// rpg-mini script — composes inventory + quest + dialogue + game-flow + combat
// + progression into a complete game loop: title → talk to elder → collect herbs
// → fight or avoid the wolf → turn in quest → win. Player has HP, can attack the
// wolf, and levels up on kill (more HP/attack). Either dying triggers game-end.
//
// `move` applies input direction to Velocity (rule DSL can't negate paths).
// `reset_game` restores player/herbs/wolf/quest/XP/Level for a fresh run, then
// emits game-restart (game-flow module catches it → phase back to title, time/score reset).
// `render_hud` shows phase-appropriate text including quest progress, player HP, level.

// Herb home positions (for restart).
const HERB_HOMES = {
  "herb-1": [3, 0],
  "herb-2": [3, 3],
  "herb-3": [0, 3],
};

// Wolf home position and stats (for restart after a fight).
const WOLF_HOME = [1, 2];
const WOLF_MAX_HP = 60;
const PLAYER_MAX_HP = 100;
const PLAYER_XP_THRESHOLD = 100;

vitric.fn("move", (args, ctx) => {
  const axis = args.axis || "x";
  const dir = Number(args.dir) || 0;
  const speed = ctx.getField("@player", "Speed.value") || 60;
  ctx.setField("@player", "Velocity." + axis, dir * speed);
});

// Move a collected herb off-screen (don't despawn — reset_game brings it back).
vitric.fn("stash_herb", (args, ctx) => {
  const herb = args.herb;
  if (!herb) return;
  ctx.setField(herb, "Position.x", -100);
  ctx.setField(herb, "Position.y", -100);
});

// Move a dead wolf off-screen (keep entity alive for restart). Mirrors stash_herb:
// despawning would break @wolf references in rules and reset_game.
vitric.fn("stash_wolf", (args, ctx) => {
  const wolf = args.wolf;
  if (!wolf) return;
  ctx.setField(wolf, "Position.x", -100);
  ctx.setField(wolf, "Position.y", -100);
});

// Apply level-up bonus: +20 max HP (full heal to new max), +10 attack power.
// The progression module emits `leveled-up` but doesn't know about Health/Attack
// (those are combat module components). This fn bridges the two: game decides the bonus.
vitric.fn("apply_level_up_bonus", (args, ctx) => {
  const who = args.who;
  if (!who) throw new Error("apply_level_up_bonus: missing who");
  const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
  const newMax = maxHp + 20;
  ctx.setField(who, "Health.max", newMax);
  ctx.setField(who, "Health.hp", newMax); // full heal on level-up
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", power + 10);
});

vitric.fn("reset_game", (_args, ctx) => {
  // Reset player position, velocity, HP, and progression stats.
  ctx.setField("@player", "Position.x", 0);
  ctx.setField("@player", "Position.y", 0);
  ctx.setField("@player", "Velocity.x", 0);
  ctx.setField("@player", "Velocity.y", 0);
  ctx.setField("@player", "Health.hp", PLAYER_MAX_HP);
  ctx.setField("@player", "Health.max", PLAYER_MAX_HP);
  ctx.setField("@player", "Attack.power", 40);
  ctx.setField("@player", "XP.current", 0);
  ctx.setField("@player", "XP.threshold", PLAYER_XP_THRESHOLD);
  ctx.setField("@player", "Level.value", 1);
  ctx.setField("@player", "Level.points", 0);

  // Clear inventory.
  ctx.setField("@player", "Inventory.items", []);
  ctx.setField("@player", "Inventory.counts", []);

  // Clear quest log.
  ctx.setField("@player", "QuestLog.active", []);
  ctx.setField("@player", "QuestLog.completed", []);

  // Reset quest state.
  ctx.setField("@herb-quest", "QuestState.state", "inactive");
  ctx.setField("@herb-quest", "QuestState.progress", 0);
  ctx.setField("@herb-quest", "QuestState.assignee", "");

  // Reset elder talk counter.
  ctx.setField("@elder", "Talked.count", 0);

  // Reset dialogue runner.
  ctx.setField("@player", "DialogueRunner.active_npc", "");
  ctx.setField("@player", "DialogueRunner.current", -1);

  // Restore herbs to their home positions (they were stashed off-screen on pickup).
  for (const name in HERB_HOMES) {
    ctx.setField(name, "Position.x", HERB_HOMES[name][0]);
    ctx.setField(name, "Position.y", HERB_HOMES[name][1]);
  }

  // Revive the wolf: restore HP and move it back home (it was stashed off-screen if killed).
  ctx.setField("@wolf", "Health.hp", WOLF_MAX_HP);
  ctx.setField("@wolf", "Position.x", WOLF_HOME[0]);
  ctx.setField("@wolf", "Position.y", WOLF_HOME[1]);

  // Emit game-restart — game-flow module resets phase/time/score.
  ctx.emit("game-restart", {});
});

vitric.fn("render_hud", (args, ctx) => {
  const game = args.game;
  const quest = args.quest;
  const who = args.who;
  const hud = args.hud;
  if (!game || !hud) throw new Error("render_hud: missing game/hud");

  const phase = ctx.getField(game, "GameState.phase") || "title";
  const time = ctx.getField(game, "GameState.time") || 0;
  const qState = ctx.getField(quest, "QuestState.state") || "inactive";
  const qProgress = ctx.getField(quest, "QuestState.progress") || 0;
  const php = ctx.getField(who, "Health.hp");
  const lvl = ctx.getField(who, "Level.value");
  const lvlText = (lvl !== undefined && lvl !== null) ? (" Lv" + lvl) : "";
  const phpText = (php !== undefined && php !== null) ? ("  HP: " + php) : "";

  let text;
  if (phase === "title") {
    text = "TITLE — Press SPACE to start";
  } else if (phase === "playing") {
    const inv = ctx.getField(who, "Inventory.items") || [];
    const invCounts = ctx.getField(who, "Inventory.counts") || [];
    const herbCount = inv.filter(function (it) { return it === "herb"; }).length;
    let coinCount = 0;
    for (let i = 0; i < inv.length; i++) {
      if (inv[i] === "coin") coinCount += (invCounts[i] || 0);
    }
    text = lvlText + "  Quest: " + qState + " " + qProgress + "/3  Herbs: " + herbCount + "  Coins: " + coinCount + phpText + "  Time: " + time;
  } else if (phase === "won") {
    text = "YOU WIN! Cleared in " + time + " ticks. Press R to restart.";
  } else if (phase === "lost") {
    text = "GAME OVER! The wolf got you. Press R to restart.";
  } else {
    text = phase;
  }
  ctx.setField(hud, "Text.content", text);
});
