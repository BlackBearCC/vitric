// shop-demo script — demonstrates the full economic loop:
// kill enemy → loot coins → buy potion → use potion to heal.
// Also supports selling items back to the shop.
// The shop module directly mutates Inventory (atomic), so this script only
// handles the potion-use logic (consume potion + heal) and HUD rendering.

vitric.fn("use_potion", (args, ctx) => {
  const who = args.who;
  if (!who) throw new Error("use_potion: missing who");

  // Check if player has a potion.
  const items = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const counts = ((ctx.getField(who, "Inventory.counts") || [])).slice();
  const idx = items.indexOf("potion");
  if (idx < 0 || counts[idx] < 1) {
    ctx.emit("potion-missing", { who });
    return;
  }

  // Consume the potion.
  counts[idx] -= 1;
  if (counts[idx] <= 0) {
    items.splice(idx, 1);
    counts.splice(idx, 1);
  }
  ctx.setField(who, "Inventory.items", items);
  ctx.setField(who, "Inventory.counts", counts);

  // Heal 30 HP (clamped to max).
  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || hp;
  const newHp = Math.min(maxHp, hp + 30);
  ctx.setField(who, "Health.hp", newHp);
  ctx.emit("healed", { who, amount: newHp - hp, hp_after: newHp });
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !enemy || !hud) throw new Error("render_hud: missing who/enemy/hud");

  const enemyHp = ctx.getField(enemy, "Health.hp");
  const hp = ctx.getField(who, "Health.hp");
  const maxHp = ctx.getField(who, "Health.max");
  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];

  let invText = items.map(function (it, i) { return it + "x" + counts[i]; }).join(", ");
  if (!invText) invText = "empty";

  const text = "HP: " + hp + "/" + maxHp + "  Enemy: " + enemyHp + "  Inv: " + invText +
    "  | X:attack B:buy potion S:sell key H:heal";
  ctx.setField(hud, "Text.content", text);
});
