// Companion wish system: each companion has 3 archetype-based wishes.
// Wishes advance via gameplay events (built/harvested/gathered/entered-poi/upgrade/food-high/see-dawn).
// Fulfilling a wish: +30 affinity, Colony.companion_wish_count++, emit wish-fulfilled.
// Task 5 wires wish-fulfilled to LLM memory dialogue; this task only advances + emits.

// Wish advancement is a fn (not a system) because it's triggered by discrete gameplay events
// caught by rules/wish.json. The fn reads Colony.companion_handles (maintained by the
// companion-register system in companion.js), iterates each companion, reads/advances Wish.items.

const AFFINITY_GAIN_PER_WISH = 30;

// Advance all companions' wishes of `kind` by `amount`. Called by rules/wish.json on gameplay events.
vitric.fn("advance_wish", (a, ctx) => {
  const kind = a.kind || "";
  const amount = (a.n | 0) || 0;
  if (!kind || amount <= 0) return;

  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  for (const h of handles) {
    if (!h) continue;
    const raw = ctx.getField(h, "Wish.items") || "[]";
    let items;
    try { items = JSON.parse(raw); } catch (_) { continue; }
    if (!Array.isArray(items)) continue;

    let changed = false;
    for (const it of items) {
      if (!it || it.done) continue;
      if (it.kind !== kind) continue;
      it.progress = (it.progress || 0) + amount;
      if (it.progress >= (it.target || 1)) {
        it.done = true;
        // Bump fulfilled counter on this entity.
        const fulfilled = ctx.getField(h, "Wish.fulfilled") | 0;
        ctx.setField(h, "Wish.fulfilled", fulfilled + 1);
        // Boost affinity (cap 100).
        const aff = ctx.getField(h, "Need.affinity");
        const affNum = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
        const newAff = Math.min(100, affNum + AFFINITY_GAIN_PER_WISH);
        ctx.setField(h, "Need.affinity", newAff);
        ctx.setField(h, "Need.affinity_i", Math.round(newAff));
        // Sync aggregate counter to Colony (quest gating in Task 6 reads this).
        const cnt = ctx.getField("colony", "Colony.companion_wish_count") | 0;
        ctx.setField("colony", "Colony.companion_wish_count", cnt + 1);
        // Emit for Task 5 (LLM memory dialogue) + toast.
        const name = ctx.getField(h, "Persona.name") || "伙伴";
        ctx.emit("wish-fulfilled", { companion: name, wish_desc: it.desc || kind, entity: h });
      }
      changed = true;
    }
    if (changed) ctx.setField(h, "Wish.items", JSON.stringify(items));
  }
});

// Apply a mood-drop penalty to all companions (e.g. cave-injury from POI).
// Rules can't iterate companion_handles, so this fn does it.
vitric.fn("apply_mood_drop", (a, ctx) => {
  const amount = (a.amount | 0) || 0;
  if (amount <= 0) return;
  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  for (const h of handles) {
    if (!h) continue;
    const cur = ctx.getField(h, "Need.comfort");
    const curNum = (typeof cur === "number" && !isNaN(cur)) ? cur : 50;
    const next = Math.max(0, curNum - amount);
    ctx.setField(h, "Need.comfort", next);
    ctx.setField(h, "Need.comfort_i", Math.round(next));
  }
});

// Food-high wish: emit a `food-high` event once per day when Colony.food >= 80.
// A rule in wish.json catches it and calls advance_wish. Guarded by Colony._wish_food_day
// to fire at most once per day (avoids spamming every tick while food stays high).
vitric.system("wish_food_check", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const food = c.Colony.food || 0;
  const day = c.Colony.day || 1;
  const lastDay = ctx.getField("colony", "Colony._wish_food_day") | 0;
  if (food >= 80 && day !== lastDay) {
    ctx.setField("colony", "Colony._wish_food_day", day);
    ctx.emit("food-high", { food: food });
  }
});
