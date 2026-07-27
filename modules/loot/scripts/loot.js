// Loot module — death-triggered item drops with deterministic RNG.
//
// One component:
//   LootTable — parallel lists encoding drop entries (same pattern as Dialogue's node_*):
//     items       — item ids to drop (e.g. ["coin", "herb"])
//     count_mins  — min count per entry (inclusive)
//     count_maxs  — max count per entry (inclusive); if absent or < min, defaults to min
//     chances     — drop probability per entry, 0.0-1.0; if absent, defaults to 1.0 (always)
//
// Events the module handles (emitted by the combat module or game rules):
//   died { who, killer } — roll who's LootTable; each successful entry auto-pickups to killer
//
// Events the module emits:
//   pickup      { who, item, count } — auto-pickup to killer's inventory (inventory module receives)
//   loot-dropped { who, killer, item, count } — per dropped entry, for game feedback (HUD/sound)
//
// Composition seam: combat → died → loot → pickup → inventory. The loot module bridges
// combat and inventory without glue code: combat emits died, loot rolls the table and emits
// pickup, inventory auto-adds. Games that want spawn-pickup (drop a Pickup entity on the ground
// instead of auto-inventory) can ignore the pickup event and handle loot-dropped manually.
//
// Determinism: uses ctx.random_stream("loot") — a named substream seeded by (world_seed, "loot"),
// independent of the main RNG stream. This means loot rolls don't shift the main stream and
// won't cause divergence in other systems' random draws. Multiple kills in the same tick draw
// from the substream in event order (FIFO), which is deterministic.

vitric.fn("__loot_roll", (args, ctx) => {
  const who = args.who;
  const killer = args.killer;
  if (!who) throw new Error("__loot_roll: missing who");
  // No killer (e.g. environmental death) → no auto-pickup. Skip the roll entirely
  // since there's no one to receive the loot. Games that want floor-loot on no-killer
  // deaths can emit a custom event and handle it themselves.
  if (!killer) return;

  // Soft dependency: if who has no LootTable or an empty one, silently skip.
  const items = ctx.getField(who, "LootTable.items");
  if (!items || items.length === 0) return;

  const countMins = ctx.getField(who, "LootTable.count_mins") || [];
  const countMaxs = ctx.getField(who, "LootTable.count_maxs") || [];
  const chances = ctx.getField(who, "LootTable.chances") || [];

  // Single substream draw point — all entries in this roll share the "loot" stream,
  // advancing it in entry order. Deterministic across replays.
  const rng = ctx.random_stream("loot");

  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    if (!item) continue;

    // Chance roll: default 1.0 (always drop) if not specified.
    const chance = (i < chances.length && chances[i] !== undefined) ? chances[i] : 1.0;
    if (chance < 1.0) {
      const roll = rng.next(); // [0, 1)
      if (roll >= chance) continue;
    }

    // Count roll: uniform int in [min, max]. Default min=1, max=min (fixed count).
    const min = (i < countMins.length && countMins[i] !== undefined) ? countMins[i] : 1;
    const max = (i < countMaxs.length && countMaxs[i] !== undefined) ? countMaxs[i] : min;
    let count = min;
    if (max > min) {
      count = rng.nextInt(min, max); // [min, max] inclusive
    }
    if (count <= 0) continue;

    // Auto-pickup to killer — inventory module's inv-pickup rule receives this and adds to inventory.
    ctx.emit("pickup", { who: killer, item: item, count: count });
    // Feedback event — game rules can listen to show floating text, play sound, etc.
    ctx.emit("loot-dropped", { who: who, killer: killer, item: item, count: count });
  }
});
