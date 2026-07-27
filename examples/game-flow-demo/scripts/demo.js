// game-flow-demo script — a complete game loop: title → play → win/lose → restart.
//
// `move` handles 2D input (the rule DSL can't negate a path, so direction sign
// is applied here). `collect_coin` moves a coin off-screen (kept for restart),
// bumps score, decrements the counter, emits game-win when all coins collected.
// `reset_game` restores player + coin positions for a fresh run, then emits
// game-restart (the game-flow module catches it → phase back to title).
// `render_hud` shows phase-appropriate text.

// Coin home positions (demo-specific; a real game would read these from a Home component).
const COIN_HOMES = {
  "coin-1": [3, 0],
  "coin-2": [6, 2],
  "coin-3": [3, 4],
};

vitric.fn("move", (args, ctx) => {
  const axis = args.axis || "x";
  const dir = Number(args.dir) || 0;
  const speed = ctx.getField("@player", "Speed.value") || 60;
  ctx.setField("@player", "Velocity." + axis, dir * speed);
});

vitric.fn("collect_coin", (args, ctx) => {
  const coin = args.coin;
  if (!coin) return;

  // Move the coin off-screen (don't despawn — reset_game brings it back).
  ctx.setField(coin, "Position.x", -100);
  ctx.setField(coin, "Position.y", -100);

  // +1 score.
  const s = ctx.getField("@game", "GameState.score") || 0;
  ctx.setField("@game", "GameState.score", s + 1);

  // Decrement the remaining-coin counter; win when it hits 0.
  const left = (ctx.getField("@game", "Coins.remaining") || 0) - 1;
  ctx.setField("@game", "Coins.remaining", Math.max(0, left));
  if (left <= 0) {
    ctx.emit("game-win", {});
  }
});

vitric.fn("reset_game", (_args, ctx) => {
  // Reset player to start.
  ctx.setField("@player", "Position.x", 0);
  ctx.setField("@player", "Position.y", 0);
  ctx.setField("@player", "Velocity.x", 0);
  ctx.setField("@player", "Velocity.y", 0);

  // Restore coins to their home positions.
  for (const name in COIN_HOMES) {
    ctx.setField(name, "Position.x", COIN_HOMES[name][0]);
    ctx.setField(name, "Position.y", COIN_HOMES[name][1]);
  }

  // Reset the coin counter.
  ctx.setField("@game", "Coins.remaining", 3);

  // Emit game-restart — the game-flow module catches it and resets phase/time/score.
  ctx.emit("game-restart", {});
});

vitric.fn("render_hud", (args, ctx) => {
  const game = args.game;
  const hud = args.hud;
  if (!game || !hud) throw new Error("render_hud: 缺少 game/hud");

  const phase = ctx.getField(game, "GameState.phase") || "title";
  const score = ctx.getField(game, "GameState.score") || 0;
  const time = ctx.getField(game, "GameState.time") || 0;
  const total = 3;

  let text;
  if (phase === "title") {
    text = "TITLE — Press SPACE to start";
  } else if (phase === "playing") {
    text = "Score " + score + "/" + total + "  Time " + time + "  (avoid red, collect yellow)";
  } else if (phase === "won") {
    text = "YOU WIN! Score " + score + " in " + time + " ticks. Press R to restart.";
  } else if (phase === "lost") {
    text = "GAME OVER! Press R to restart.";
  } else {
    text = phase;
  }
  ctx.setField(hud, "Text.content", text);
});
