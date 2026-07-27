// game-flow module — the closed-loop backbone: title → playing → won/lost → restart.
//
// A game carries ONE global `GameState` component on a `@game` entity:
//   phase — title | playing | won | lost | paused
//   time  — ticks elapsed in the current playthrough (auto-incremented while playing)
//   score — arbitrary integer the game updates via __game_add_score
//
// State machine:
//   title  --game-start-->   playing
//   playing --game-win-->    won
//   playing --game-lose-->   lost
//   won/lost --game-restart--> title
//
// The module is state-machine-only: it does NOT manage save/load (that needs
// Sim-level snapshot/restore, not yet exposed to scripts) and does NOT define
// win/lose conditions (the game's own rules emit game-win / game-lose when its
// conditions are met). It gives every Vitric game a common beginning/end shape,
// which is the structural difference between "sandbox demo" and "complete game".
//
// Convention: the project must include a named entity `game` with a `GameState`
// component. Rules/scripts reference it as `@game`.
//
// Events emitted: game-started / game-won / game-lost / game-restarted.

const GAME = "@game";

vitric.fn("__game_start", (_args, ctx) => {
  ctx.setField(GAME, "GameState.phase", "playing");
  ctx.setField(GAME, "GameState.time", 0);
  ctx.emit("game-started", {});
});

vitric.fn("__game_win", (_args, ctx) => {
  const phase = ctx.getField(GAME, "GameState.phase");
  if (phase !== "playing") return; // idempotent: only win from playing
  ctx.setField(GAME, "GameState.phase", "won");
  ctx.emit("game-won", {
    score: ctx.getField(GAME, "GameState.score") || 0,
    time: ctx.getField(GAME, "GameState.time") || 0,
  });
});

vitric.fn("__game_lose", (_args, ctx) => {
  const phase = ctx.getField(GAME, "GameState.phase");
  if (phase !== "playing") return; // idempotent: only lose from playing
  ctx.setField(GAME, "GameState.phase", "lost");
  ctx.emit("game-lost", {});
});

vitric.fn("__game_restart", (_args, ctx) => {
  ctx.setField(GAME, "GameState.phase", "title");
  ctx.setField(GAME, "GameState.time", 0);
  ctx.setField(GAME, "GameState.score", 0);
  ctx.emit("game-restarted", {});
});

vitric.fn("__game_tick", (_args, ctx) => {
  const phase = ctx.getField(GAME, "GameState.phase");
  if (phase !== "playing") return;
  const t = ctx.getField(GAME, "GameState.time") || 0;
  ctx.setField(GAME, "GameState.time", t + 1);
});

// Helper for games to bump the score from any rule (e.g. on coin pickup).
vitric.fn("__game_add_score", (args, ctx) => {
  const delta = Number(args.delta) || 0;
  const s = ctx.getField(GAME, "GameState.score") || 0;
  ctx.setField(GAME, "GameState.score", s + delta);
});
