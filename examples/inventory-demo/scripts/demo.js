// inventory-demo script — renders the player's inventory into the HUD Text component.
// Called by rules/game.json on item-picked-up / item-dropped events.

vitric.fn("render_inventory", (args, ctx) => {
  const who = args.who;
  const hud = args.hud;
  if (!who) throw new Error("render_inventory: 缺少 who");
  if (!hud) throw new Error("render_inventory: 缺少 hud");

  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];

  let text;
  if (items.length === 0) {
    text = "Inventory: empty";
  } else {
    const parts = [];
    for (let i = 0; i < items.length; i++) {
      parts.push(items[i] + "x" + counts[i]);
    }
    text = "Inventory: " + parts.join(", ");
  }
  ctx.setField(hud, "Text.content", text);
});
