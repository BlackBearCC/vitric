// combat-demo script — renders player + enemy HP to the HUD Text component.

// Move a dead enemy off-screen (keep the entity alive so HUD can still read it).
vitric.fn("stash_enemy", (args, ctx) => {
  const enemy = args.enemy;
  if (!enemy) return;
  ctx.setField(enemy, "Position.x", -100);
  ctx.setField(enemy, "Position.y", -100);
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !hud) throw new Error("render_hud: missing who/hud");

  const php = ctx.getField(who, "Health.hp") || 0;
  const pmax = ctx.getField(who, "Health.max") || 0;

  let text = "Player HP: " + php + "/" + pmax;
  if (enemy) {
    const ehp = ctx.getField(enemy, "Health.hp");
    if (ehp !== undefined && ehp !== null) {
      const emax = ctx.getField(enemy, "Health.max") || 0;
      text += "  Enemy HP: " + ehp + "/" + emax;
    } else {
      text += "  Enemy: dead";
    }
  }
  ctx.setField(hud, "Text.content", text);
});
