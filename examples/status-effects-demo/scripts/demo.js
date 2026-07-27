// status-effects-demo script — demonstrates the status-effects module:
// poison (DoT), regen (HoT), haste (stat modifier), and clear (antidote).
//
// Two composition patterns shown:
// 1. Tick-based effects (poison, regen): game rules listen to `status-ticked`
//    and emit combat `damage`/`heal` events. The status-effects module manages
//    the lifecycle, the game defines the semantics.
// 2. Stat-modifier effects (haste): game rules listen to `status-applied`/
//    `status-expired` and modify Attack.power. The module doesn't know what
//    "haste" does — the game decides.

// Haste magnitude is stored on the entity so remove_haste_bonus knows how much
// to subtract. We use a field on StatusEffects — but since haste is already
// removed from the lists by the time status-expired fires, we stash the last
// known magnitude in a script-side map keyed by entity id.
//
// Actually, simpler: haste always uses a fixed magnitude in this demo (+10 ATK),
// so we just subtract 10. In a real game with variable haste strength, you'd
// track the bonus per entity (e.g. a HasteBonus component or a script-side map).
// For demo clarity, we use a fixed bonus.

const HASTE_BONUS = 10;

vitric.fn("apply_haste_bonus", (args, ctx) => {
  const who = args.who;
  const magnitude = Number(args.magnitude) || HASTE_BONUS;
  if (!who) throw new Error("apply_haste_bonus: missing who");
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", power + magnitude);
});

vitric.fn("remove_haste_bonus", (args, ctx) => {
  const who = args.who;
  if (!who) throw new Error("remove_haste_bonus: missing who");
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", Math.max(0, power - HASTE_BONUS));
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !enemy || !hud) throw new Error("render_hud: missing who/enemy/hud");

  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || 0;
  const power = ctx.getField(who, "Attack.power") || 0;

  function fmtStatus(entityId) {
    const effs = ctx.getField(entityId, "StatusEffects.effects") || [];
    const durs = ctx.getField(entityId, "StatusEffects.durations") || [];
    const mags = ctx.getField(entityId, "StatusEffects.magnitudes") || [];
    if (effs.length === 0) return "none";
    return effs.map(function (e, i) {
      return e + "(" + durs[i] + "t," + mags[i] + ")";
    }).join(", ");
  }

  const enemyHp = ctx.getField(enemy, "Health.hp") || 0;

  const text = "HP: " + hp + "/" + maxHp + "  ATK: " + power +
    "  Status: [" + fmtStatus(who) + "]" +
    "  Enemy HP: " + enemyHp + "  Enemy status: [" + fmtStatus(enemy) + "]" +
    "  | 1:poison 2:regen 3:haste 4:antidote X:attack";
  ctx.setField(hud, "Text.content", text);
});
