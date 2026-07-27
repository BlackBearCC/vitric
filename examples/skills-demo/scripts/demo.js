// skills-demo script — demonstrates the skills module:
// fireball (damage), heal (heal), shield (apply-status), and basic attack.
//
// Three composition patterns shown:
// 1. Damage ability (fireball): on ability-cast → emit damage → combat module
// 2. Healing ability (heal): on ability-cast → emit heal → combat module
// 3. Status ability (shield): on ability-cast → emit apply-status → status-effects module
//
// The skills module itself doesn't know what fireball/heal/shield DO — it only
// validates (known / cooldown / mana), pays the cost, sets cooldown, and emits
// ability-cast. The game rules above bridge ability-cast to the right effect.
//
// This script only handles HUD rendering. The ability semantics are entirely in
// rules/game.json — that's the point: the module manages lifecycle, the game
// owns semantics via declarative rules.

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const enemy = args.enemy;
  const hud = args.hud;
  if (!who || !enemy || !hud) throw new Error("render_hud: missing who/enemy/hud");

  const hp = ctx.getField(who, "Health.hp") || 0;
  const maxHp = ctx.getField(who, "Health.max") || 0;
  const mana = ctx.getField(who, "Mana.current") || 0;
  const maxMana = ctx.getField(who, "Mana.max") || 0;

  function fmtAbilities(entityId) {
    const known = ctx.getField(entityId, "Abilities.known") || [];
    const cds = ctx.getField(entityId, "Abilities.cooldowns") || [];
    const costs = ctx.getField(entityId, "Abilities.costs") || [];
    if (known.length === 0) return "none";
    return known.map(function (a, i) {
      return a + "(" + costs[i] + "m,cd" + cds[i] + ")";
    }).join(", ");
  }

  function fmtStatus(entityId) {
    const effs = ctx.getField(entityId, "StatusEffects.effects") || [];
    if (effs.length === 0) return "none";
    return effs.join(", ");
  }

  const enemyHp = ctx.getField(enemy, "Health.hp") || 0;

  const text = "HP: " + hp + "/" + maxHp + "  MP: " + mana + "/" + maxMana +
    "  Abilities: [" + fmtAbilities(who) + "]" +
    "  Status: [" + fmtStatus(who) + "]" +
    "  Enemy HP: " + enemyHp +
    "  | 1:fireball 2:heal 3:shield X:attack";
  ctx.setField(hud, "Text.content", text);
});
