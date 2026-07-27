// Status-effects module — timed effect lifecycle (apply / tick / expire / clear).
//
// One component:
//   StatusEffects — parallel lists encoding active effects (same pattern as
//   Dialogue/LootTable/Shop/Equipment):
//     effects    — effect name list (e.g. ["poison", "haste", "regen"])
//     durations  — ticks remaining per effect (e.g. [5, 3, 2])
//     magnitudes — effect strength per effect (e.g. [10, 1, 20])
//
// Events the module handles (emitted by the game's rules):
//   apply-status { who, effect, duration, magnitude } — apply effect to who
//   clear-status { who, effect }                       — remove effect from who
//
// Events the module emits:
//   status-applied  { who, effect, duration, magnitude } — effect added/refreshed
//   status-ticked   { who, effect, magnitude, ticks_remaining } — effect ticked (game decides what it does)
//   status-expired  { who, effect }                      — effect duration ran out
//   status-cleared  { who, effect }                      — effect removed by clear-status
//
// Composition: the module manages the LIFECYCLE (track durations, tick, expire),
// the game's rules decide what each effect DOES. Two composition patterns:
//
// 1. Tick-based effects (poison, regen): game listens to `status-ticked` and emits
//    combat events:
//      on status-ticked where effect=="poison" → emit damage { who, amount: magnitude }
//      on status-ticked where effect=="regen"  → emit heal { who, amount: magnitude }
//
// 2. Stat-modifier effects (haste, shield): game listens to `status-applied`/
//    `status-expired` and modifies stats:
//      on status-applied where effect=="haste"  → +ATK
//      on status-expired  where effect=="haste"  → -ATK
//
// This keeps the module generic — it doesn't know what "poison" or "haste" means.
// The game defines the effect semantics via rules, same as equipment module doesn't
// know what "sword" does (game defines +ATK in apply_equip_bonus).

// ---- apply: add or refresh an effect ----
vitric.fn("__status_apply", (args, ctx) => {
  const who = args.who;
  const effect = String(args.effect);
  const duration = Math.max(1, Number(args.duration) || 1);
  const magnitude = Number(args.magnitude) || 0;
  if (!who) throw new Error("__status_apply: missing who");
  if (!effect) throw new Error("__status_apply: missing effect");

  const effects = ((ctx.getField(who, "StatusEffects.effects") || [])).slice();
  const durations = ((ctx.getField(who, "StatusEffects.durations") || [])).slice();
  const magnitudes = ((ctx.getField(who, "StatusEffects.magnitudes") || [])).slice();

  const idx = effects.indexOf(effect);
  if (idx >= 0) {
    // Refresh: take max duration and max magnitude (RPG-standard stacking rule).
    durations[idx] = Math.max(durations[idx], duration);
    magnitudes[idx] = Math.max(magnitudes[idx], magnitude);
  } else {
    // New effect: append to all three lists.
    effects.push(effect);
    durations.push(duration);
    magnitudes.push(magnitude);
  }

  ctx.setField(who, "StatusEffects.effects", effects);
  ctx.setField(who, "StatusEffects.durations", durations);
  ctx.setField(who, "StatusEffects.magnitudes", magnitudes);

  ctx.emit("status-applied", { who, effect, duration, magnitude });
});

// ---- clear: remove an effect by name ----
vitric.fn("__status_clear", (args, ctx) => {
  const who = args.who;
  const effect = String(args.effect);
  if (!who) throw new Error("__status_clear: missing who");
  if (!effect) throw new Error("__status_clear: missing effect");

  const effects = ((ctx.getField(who, "StatusEffects.effects") || [])).slice();
  const durations = ((ctx.getField(who, "StatusEffects.durations") || [])).slice();
  const magnitudes = ((ctx.getField(who, "StatusEffects.magnitudes") || [])).slice();

  const idx = effects.indexOf(effect);
  if (idx < 0) return; // not affected → no-op

  effects.splice(idx, 1);
  durations.splice(idx, 1);
  magnitudes.splice(idx, 1);

  ctx.setField(who, "StatusEffects.effects", effects);
  ctx.setField(who, "StatusEffects.durations", durations);
  ctx.setField(who, "StatusEffects.magnitudes", magnitudes);

  ctx.emit("status-cleared", { who, effect });
});

// ---- tick system: decrement durations, emit ticked/expired ----
vitric.system(
  "status-tick",
  { query: ["StatusEffects"], writes: ["StatusEffects"] },
  (entities, ctx) => {
    for (const e of entities) {
      const effects = (e.StatusEffects.effects || []).slice();
      const durations = (e.StatusEffects.durations || []).slice();
      const magnitudes = (e.StatusEffects.magnitudes || []).slice();

      if (effects.length === 0) continue;

      const remaining = [];
      const remainingDur = [];
      const remainingMag = [];

      for (let i = 0; i < effects.length; i++) {
        const effect = effects[i];
        const magnitude = magnitudes[i];
        const ticksLeft = durations[i] - 1;

        // Emit ticked for this tick (the effect is still active right now).
        ctx.emit("status-ticked", {
          who: e.id,
          effect,
          magnitude,
          ticks_remaining: Math.max(0, ticksLeft),
        });

        if (ticksLeft <= 0) {
          // Duration expired after this tick — emit expired, don't keep.
          ctx.emit("status-expired", { who: e.id, effect });
        } else {
          // Still active: keep with decremented duration.
          remaining.push(effect);
          remainingDur.push(ticksLeft);
          remainingMag.push(magnitude);
        }
      }

      // Write back only if lists changed.
      if (remaining.length !== effects.length) {
        ctx.setField(e.id, "StatusEffects.effects", remaining);
        ctx.setField(e.id, "StatusEffects.durations", remainingDur);
        ctx.setField(e.id, "StatusEffects.magnitudes", remainingMag);
      } else {
        // Durations changed even if count didn't.
        ctx.setField(e.id, "StatusEffects.durations", remainingDur);
      }
    }
  },
);
