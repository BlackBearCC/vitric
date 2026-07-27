// progression-demo script — combat + progression composition.
//
// Player kills the enemy → combat module emits `died` (carrying killer) →
// game rule emits `gain-xp` → progression module adds XP, emits `leveled-up` →
// game rule calls `apply_level_up_bonus` → player gets +20 max HP (full heal)
// and +10 attack. This closes the loop: fight → XP → level up → stronger.

vitric.fn("apply_level_up_bonus", (args, ctx) => {
  const who = args.who;
  if (!who) throw new Error("apply_level_up_bonus: missing who");

  // +20 max HP, full heal to new max.
  const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
  const newMax = maxHp + 20;
  ctx.setField(who, "Health.max", newMax);
  ctx.setField(who, "Health.hp", newMax);

  // +10 attack power.
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", power + 10);
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !hud) throw new Error("render_hud: missing who/hud");

  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || 0;
  const lvl = ctx.getField(who, "Level.value") || 1;
  const xp = ctx.getField(who, "XP.current") || 0;
  const threshold = ctx.getField(who, "XP.threshold") || 100;
  const power = ctx.getField(who, "Attack.power") || 0;

  let text = "Lv" + lvl + "  HP: " + hp + "/" + maxHp + "  ATK: " + power + "  XP: " + xp + "/" + threshold;
  if (enemy) {
    const ehp = ctx.getField(enemy, "Health.hp");
    if (ehp !== undefined && ehp !== null && ehp > 0) {
      text += "  Enemy HP: " + ehp;
    } else {
      text += "  Enemy: dead";
    }
  }
  ctx.setField(hud, "Text.content", text);
});
