// loot-demo script — render HUD showing player inventory and enemy HP.
// The loot module auto-pickups dropped items to the killer's inventory on death;
// this script just renders the result. No loot-handling glue code needed.

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !enemy || !hud) throw new Error("render_hud: missing who/enemy/hud");

  const enemyHp = ctx.getField(enemy, "Health.hp");
  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];

  let invText;
  if (items.length === 0) {
    invText = "empty";
  } else {
    invText = items.map(function (it, i) { return it + "x" + counts[i]; }).join(", ");
  }

  const text = "Enemy HP: " + enemyHp + "  |  Inventory: " + invText + "  |  Press X to attack";
  ctx.setField(hud, "Text.content", text);
});
