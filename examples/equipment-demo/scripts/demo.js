// equipment-demo script — demonstrates the equipment module:
// equip items to slots, apply stat bonuses via events, auto-unequip
// when swapping, and unequip back to inventory.
//
// Player starts with 5 items in inventory (sword, armor, ring, gloves, spare_sword)
// and 3 empty equipment slots (weapon, armor, accessory).
//
// Stat bonuses (game-defined, not module-defined):
//   sword        → +10 ATK
//   spare_sword  → +8 ATK
//   armor        → +20 max HP (and heal +20)
//   ring         → +5 ATK
//   gloves       → +3 ATK

// Per-item stat bonus table. The game owns this — the equipment module just
// moves items between inventory and slots and emits events.
function bonusFor(item) {
  switch (item) {
    case "sword":       return { atk: 10, maxHp: 0 };
    case "spare_sword": return { atk: 8,  maxHp: 0 };
    case "armor":       return { atk: 0,  maxHp: 20 };
    case "ring":        return { atk: 5,  maxHp: 0 };
    case "gloves":      return { atk: 3,  maxHp: 0 };
    default:            return { atk: 0,  maxHp: 0 };
  }
}

vitric.fn("apply_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  if (!who) throw new Error("apply_equip_bonus: missing who");

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
    // Full heal to new max — equipping armor should not leave you wounded.
    ctx.setField(who, "Health.hp", Math.min(newMax, hp + bonus.maxHp));
  }
});

vitric.fn("remove_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  if (!who) throw new Error("remove_equip_bonus: missing who");

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
    // Clamp HP to new max (unequipping armor can leave you over-healed otherwise).
    ctx.setField(who, "Health.hp", Math.min(hp, newMax));
  }
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const hud = args.hud;
  if (!who || !hud) throw new Error("render_hud: missing who/hud");

  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || 0;
  const power = ctx.getField(who, "Attack.power") || 0;

  const eqSlots = ctx.getField(who, "Equipment.slots") || [];
  const eqItems = ctx.getField(who, "Equipment.items") || [];
  let eqText = eqSlots.map(function (s, i) { return s + ":" + (eqItems[i] || "-"); }).join(" ");

  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];
  let invText = items.map(function (it, i) { return it + "x" + counts[i]; }).join(", ");
  if (!invText) invText = "empty";

  const text = "HP: " + hp + "/" + maxHp + "  ATK: " + power +
    "  Eq: [" + eqText + "]  Inv: " + invText +
    "  | 1:sword 2:armor 3:ring 4:gloves 5:spare Q/W/E:unequip X:attack";
  ctx.setField(hud, "Text.content", text);
});
