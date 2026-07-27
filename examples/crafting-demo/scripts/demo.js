// crafting-demo script — demonstrates the crafting module:
// craft sword (3 iron + 1 wood), craft shield (2 iron + 2 wood), equip sword
// for +ATK, then attack to see the damage increase.
//
// Composition: crafting + inventory + equipment + combat. The crafting module
// consumes materials from Inventory and adds the output to Inventory (atomic).
// The equipment module moves items between Inventory and Equipment slots and
// emits equipped/unequipped events. This script handles stat bonuses (the
// bridge between equipment and combat), same as the equipment demo.

function bonusFor(item) {
  switch (item) {
    case "sword":  return { atk: 15, maxHp: 0 };
    case "shield": return { atk: 0,  maxHp: 20 };
    default:       return { atk: 0,  maxHp: 0 };
  }
}

vitric.fn("apply_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const bonus = bonusFor(item);
  if (bonus.atk !== 0) {
    const power = Number(ctx.getField(who, "Attack.power")) || 0;
    ctx.setField(who, "Attack.power", power + bonus.atk);
  }
  if (bonus.maxHp !== 0) {
    const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
    const hp = Number(ctx.getField(who, "Health.hp")) || 0;
    const newMax = maxHp + bonus.maxHp;
    ctx.setField(who, "Health.max", newMax);
    ctx.setField(who, "Health.hp", Math.min(newMax, hp + bonus.maxHp));
  }
});

vitric.fn("remove_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const bonus = bonusFor(item);
  if (bonus.atk !== 0) {
    const power = Number(ctx.getField(who, "Attack.power")) || 0;
    ctx.setField(who, "Attack.power", Math.max(0, power - bonus.atk));
  }
  if (bonus.maxHp !== 0) {
    const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
    const hp = Number(ctx.getField(who, "Health.hp")) || 0;
    const newMax = Math.max(1, maxHp - bonus.maxHp);
    ctx.setField(who, "Health.max", newMax);
    ctx.setField(who, "Health.hp", Math.min(hp, newMax));
  }
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !enemy || !hud) throw new Error("render_hud: missing who/enemy/hud");

  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || 0;
  const power = ctx.getField(who, "Attack.power") || 0;
  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];
  const slots = ctx.getField(who, "Equipment.slots") || [];
  const equipped = ctx.getField(who, "Equipment.items") || [];

  const invText = items.length === 0
    ? "empty"
    : items.map(function (it, i) { return it + "x" + counts[i]; }).join(", ");
  const eqText = slots.map(function (s, i) { return s + ":" + (equipped[i] || "-"); }).join(", ");

  const enemyHp = ctx.getField(enemy, "Health.hp") || 0;
  const text = "HP: " + hp + "/" + maxHp + "  ATK: " + power +
    "  Inv: [" + invText + "]  Equipped: [" + eqText + "]" +
    "  Enemy HP: " + enemyHp +
    "  | 1:craft sword 2:craft shield 3:equip sword 4:unequip X:attack";
  ctx.setField(hud, "Text.content", text);
});
