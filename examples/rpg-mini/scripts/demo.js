// rpg-mini script — composes inventory + quest + dialogue + game-flow into a
// complete game loop: title → talk to elder → collect herbs → turn in → win.
//
// `move` applies input direction to Velocity (rule DSL can't negate paths).
// `reset_game` restores player/herbs/quest for a fresh run, then emits
// game-restart (game-flow module catches it → phase back to title, time/score reset).
// `render_hud` shows phase-appropriate text including quest progress.

// Herb home positions (for restart).
const HERB_HOMES = {
  "herb-1": [3, 0],
  "herb-2": [3, 3],
  "herb-3": [0, 3],
};

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

vitric.fn("reset_game", (_args, ctx) => {
  // Reset player position and velocity.
  ctx.setField("@player", "Position.x", 0);
  ctx.setField("@player", "Position.y", 0);
  ctx.setField("@player", "Velocity.x", 0);
  ctx.setField("@player", "Velocity.y", 0);

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

  let text;
  if (phase === "title") {
    text = "TITLE — Press SPACE to start";
  } else if (phase === "playing") {
    const inv = ctx.getField(who, "Inventory.items") || [];
    const herbCount = inv.filter(function (it) { return it === "herb"; }).length;
    text = "Quest: " + qState + " " + qProgress + "/3  Herbs: " + herbCount + "  Time: " + time;
  } else if (phase === "won") {
    text = "YOU WIN! Cleared in " + time + " ticks. Press R to restart.";
  } else if (phase === "lost") {
    text = "GAME OVER! The wolf got you. Press R to restart.";
  } else {
    text = phase;
  }
  ctx.setField(hud, "Text.content", text);
});
