// Progression module — XP / level-up / stat points.
//
// Two components:
//   XP    — current (accumulated XP) / threshold (XP needed for next level)
//   Level — value (current level, starts at 1) / points (unspent stat points)
//
// Events the module handles (emitted by the game's rules):
//   gain-xp { who, amount } — add `amount` XP to `who`; auto-level-up if threshold reached
//
// Events the module emits:
//   xp-gained   { who, amount, total }   — XP added (total = new current XP)
//   leveled-up  { who, level, points }   — level increased (points = total unspent stat points)
//
// The module does NOT decide what a level-up grants (HP? attack? skill tree?)
// — different games have different stat systems. The game's rules listen to
// `leveled-up` and apply bonuses (e.g. add 20 to Health.max, add 10 to Attack.power).
// This keeps progression decoupled from combat/inventory/any other module.
//
// Threshold grows by 1.5x (floored) on each level-up. Starting threshold is
// game-configurable (set XP.threshold in the scene file). With threshold=100:
//   L1→L2 needs 100 XP, L2→L3 needs 150, L3→L4 needs 225, L4→L5 needs 337, ...

vitric.fn("__progression_gain_xp", (args, ctx) => {
  const who = args.who;
  const amount = Number(args.amount) || 0;
  if (!who) throw new Error("__progression_gain_xp: missing who");
  if (amount <= 0) return; // no-op for zero/negative XP

  let current = Number(ctx.getField(who, "XP.current")) || 0;
  let threshold = Number(ctx.getField(who, "XP.threshold")) || 100;
  let level = Number(ctx.getField(who, "Level.value")) || 1;
  let points = Number(ctx.getField(who, "Level.points")) || 0;

  current += amount;

  // Level-up loop: one big XP gain can cross multiple thresholds.
  while (current >= threshold) {
    current -= threshold;
    level += 1;
    points += 1;
    // Threshold grows 1.5x (floored). Minimum growth of 1 so it never stalls.
    threshold = Math.max(threshold + 1, Math.floor(threshold * 3 / 2));
    ctx.emit("leveled-up", { who, level, points });
  }

  ctx.setField(who, "XP.current", current);
  ctx.setField(who, "XP.threshold", threshold);
  ctx.setField(who, "Level.value", level);
  ctx.setField(who, "Level.points", points);
  ctx.emit("xp-gained", { who, amount, total: current });
});
