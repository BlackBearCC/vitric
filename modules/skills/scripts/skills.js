// Skills module — active abilities with mana cost and cooldowns.
//
// Two components:
//   Abilities — parallel lists encoding the entity's known abilities (same
//     parallel-list pattern as Dialogue/LootTable/Equipment/StatusEffects):
//     known         — ability ids (e.g. ["fireball", "heal", "shield"])
//     cooldowns     — remaining cooldown ticks per ability (0 = ready)
//     costs         — mana cost per ability (static, set at authoring time)
//     cooldown_maxs — cooldown duration per ability (static, set at authoring)
//
//   Mana — resource pool:
//     current — current mana (clamped to [0, max])
//     max     — max mana
//
// Events the module handles (emitted by the game's rules):
//   cast { who, ability, target } — request to cast `ability` from `who` onto `target`
//
// Events the module emits:
//   ability-cast   { who, ability, target } — cast succeeded (game defines effect)
//   cast-rejected  { who, ability, reason } — cast failed
//     reason ∈ {"unknown", "cooldown", "mana"}
//
// Composition: the module manages the LIFECYCLE (validate / pay cost / set
// cooldown / tick cooldowns), the game's rules decide what each ability DOES.
// On `ability-cast`, the game emits damage/heal/apply-status/attack/etc.:
//   on ability-cast where ability=="fireball" → emit damage { who: target, amount: 50 }
//   on ability-cast where ability=="heal"     → emit heal { who, amount: 30 }
//   on ability-cast where ability=="shield"   → emit apply-status { who, effect: "shield", ... }
//
// This keeps the module generic — it doesn't know what "fireball" or "heal"
// means. The game defines ability semantics via rules, same as the equipment
// module doesn't know what "sword" does, and status-effects doesn't know what
// "poison" does.

// ---- cast: validate, pay cost, set cooldown, emit ability-cast ----
vitric.fn("__skills_cast", (args, ctx) => {
  const who = args.who;
  const ability = String(args.ability);
  const target = args.target || "";
  if (!who) throw new Error("__skills_cast: missing who");
  if (!ability) throw new Error("__skills_cast: missing ability");

  // Read the entity's ability lists.
  const known = (ctx.getField(who, "Abilities.known") || []).slice();
  const cooldowns = (ctx.getField(who, "Abilities.cooldowns") || []).slice();
  const costs = (ctx.getField(who, "Abilities.costs") || []).slice();
  const cooldownMaxs = (ctx.getField(who, "Abilities.cooldown_maxs") || []).slice();

  // Find the ability index.
  const idx = known.indexOf(ability);
  if (idx < 0) {
    ctx.emit("cast-rejected", { who, ability, reason: "unknown" });
    return;
  }

  // Check cooldown (0 = ready).
  const currentCd = Number(cooldowns[idx]) || 0;
  if (currentCd > 0) {
    ctx.emit("cast-rejected", { who, ability, reason: "cooldown" });
    return;
  }

  // Check mana.
  const cost = Number(costs[idx]) || 0;
  const mana = Number(ctx.getField(who, "Mana.current")) || 0;
  if (mana < cost) {
    ctx.emit("cast-rejected", { who, ability, reason: "mana" });
    return;
  }

  // Pay mana.
  const newMana = Math.max(0, mana - cost);
  ctx.setField(who, "Mana.current", newMana);

  // Set cooldown.
  const cdMax = Number(cooldownMaxs[idx]) || 0;
  cooldowns[idx] = cdMax;
  ctx.setField(who, "Abilities.cooldowns", cooldowns);

  // Emit success — game rules listen to define the effect.
  ctx.emit("ability-cast", { who, ability, target });
});

// ---- tick system: decrement all non-zero cooldowns ----
vitric.system(
  "skills-cooldown-tick",
  { query: ["Abilities"], writes: ["Abilities"] },
  (entities, ctx) => {
    for (const e of entities) {
      const cooldowns = (e.Abilities.cooldowns || []).slice();
      let changed = false;
      for (let i = 0; i < cooldowns.length; i++) {
        if (cooldowns[i] > 0) {
          cooldowns[i] = cooldowns[i] - 1;
          changed = true;
        }
      }
      if (changed) {
        ctx.setField(e.id, "Abilities.cooldowns", cooldowns);
      }
    }
  },
);
