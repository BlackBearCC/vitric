// Combat module — HP / damage / death / healing.
//
// Two components:
//   Health — hp (current) / max (maximum)
//   Attack — power (damage dealt per attack)
//
// Events the module handles (emitted by the game's rules):
//   attack  { attacker, target }             — attacker hits target for Attack.power damage
//   damage  { who, amount, killer? }         — amount (positive) is subtracted from hp
//   heal    { who, amount }                   — amount (positive) is added to hp
//
// Events the module emits:
//   damaged { who, amount, hp_after }         — damage applied
//   healed  { who, amount, hp_after }         — healing applied
//   died    { who, killer }                   — hp reached 0 (game decides despawn/respawn)
//
// The module does NOT despawn on death — the game's rules decide (despawn enemy,
// emit game-lose for player, etc.). This keeps the module flexible for respawn
// scenarios (see cookbook recipe 3: checkpoint).

vitric.fn("__combat_attack", (args, ctx) => {
  const attacker = args.attacker;
  const target = args.target;
  if (!attacker) throw new Error("__combat_attack: missing attacker");
  if (!target) throw new Error("__combat_attack: missing target");
  const power = ctx.getField(attacker, "Attack.power") || 0;
  if (power <= 0) return;
  // Emit damage event — combat-on-damage rule picks it up next tick.
  // Carrying killer through so died event knows who landed the killing blow.
  ctx.emit("damage", { who: target, amount: power, killer: attacker });
});

vitric.fn("__combat_damage", (args, ctx) => {
  const who = args.who;
  const amount = Number(args.amount) || 0;
  if (!who) throw new Error("__combat_damage: missing who");
  if (amount <= 0) return; // no-op for zero/negative damage
  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || hp;
  const newHp = Math.max(0, Math.min(maxHp, hp - amount));
  ctx.setField(who, "Health.hp", newHp);
  ctx.emit("damaged", { who, amount, hp_after: newHp });
  if (newHp <= 0) {
    ctx.emit("died", { who, killer: args.killer || "" });
  }
});

vitric.fn("__combat_heal", (args, ctx) => {
  const who = args.who;
  const amount = Number(args.amount) || 0;
  if (!who) throw new Error("__combat_heal: missing who");
  if (amount <= 0) return;
  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || hp;
  const newHp = Math.max(0, Math.min(maxHp, hp + amount));
  ctx.setField(who, "Health.hp", newHp);
  ctx.emit("healed", { who, amount, hp_after: newHp });
});
