// Achievement module — data-driven achievement tracking with counter-based progress.
//
// One component:
//   Achievements — on a tracker entity (e.g. @achievements):
//     defs           JSON array of {id, name, desc, target} — achievement definitions
//     unlocked       JSON array of unlocked achievement IDs
//     progress       JSON object mapping id → current count (for counter-based)
//     count          total defined achievements
//     unlocked_count number unlocked
//
// Events the module handles (emitted by the game's rules):
//   achievement-define   { tracker, defs }          — register definitions (JSON array or string)
//   achievement-unlock   { tracker, id }            — instant unlock
//   achievement-progress { tracker, id, amount }    — increment counter (amount defaults to 1)
//
// Events the module emits:
//   achievement-unlocked        { id, name, desc }              — just unlocked
//   achievement-progress-updated { id, progress, target, name } — counter changed
//
// Design notes:
//   - target=0 means instant unlock (no counter needed); achievement-progress on a
//     target=0 achievement is a no-op.
//   - Unlocking an already-unlocked achievement is a no-op (idempotent).
//   - Progress on an undefined achievement is a no-op (silent ignore).
//   - defs can be set in the scene file directly (Achievements.defs field) or via
//     the achievement-define event at runtime. Both paths merge by id.
//   - All state is JSON text in component fields — deterministic, serializable,
//     save/load friendly.

function parseJSON(raw, fallback) {
  if (!raw || typeof raw !== "string") return fallback;
  try { return JSON.parse(raw); } catch (_) { return fallback; }
}

// ---- Boot-time init: sync count from defs length (scene may set defs but not count) ----
vitric.system("achievement-init", { query: ["Achievements"], writes: ["Achievements"] }, (entities, ctx) => {
  for (const e of entities) {
    const defs = parseJSON(e.Achievements.defs, []);
    const unlocked = parseJSON(e.Achievements.unlocked, []);
    if (e.Achievements.count !== defs.length) {
      e.Achievements.count = defs.length;
    }
    if (e.Achievements.unlocked_count !== unlocked.length) {
      e.Achievements.unlocked_count = unlocked.length;
    }
  }
});

// ---- achievement-define: merge new defs into existing (dedup by id) ----
vitric.fn("__achievement_define", (args, ctx) => {
  const tracker = args.tracker;
  if (!tracker) throw new Error("__achievement_define: missing tracker");

  let newDefs = args.defs;
  if (typeof newDefs === "string") {
    try { newDefs = JSON.parse(newDefs); } catch (_) { newDefs = []; }
  }
  if (!Array.isArray(newDefs)) newDefs = [];

  const existing = parseJSON(ctx.getField(tracker, "Achievements.defs"), []);
  const existingIds = new Set(existing.map(d => d.id));

  for (const d of newDefs) {
    if (!d || !d.id) continue;
    if (existingIds.has(d.id)) continue; // no duplicates
    existing.push({
      id: d.id,
      name: d.name || d.id,
      desc: d.desc || "",
      target: Number(d.target) || 0,
    });
    existingIds.add(d.id);
  }

  ctx.setField(tracker, "Achievements.defs", JSON.stringify(existing));
  ctx.setField(tracker, "Achievements.count", existing.length);
});

// ---- achievement-unlock: instant unlock by id ----
vitric.fn("__achievement_unlock", (args, ctx) => {
  const tracker = args.tracker;
  const id = args.id;
  if (!tracker || !id) return;

  const defs = parseJSON(ctx.getField(tracker, "Achievements.defs"), []);
  const def = defs.find(d => d.id === id);
  if (!def) return; // undefined achievement — silent no-op

  const unlocked = parseJSON(ctx.getField(tracker, "Achievements.unlocked"), []);
  if (unlocked.includes(id)) return; // already unlocked — idempotent

  unlocked.push(id);
  ctx.setField(tracker, "Achievements.unlocked", JSON.stringify(unlocked));
  ctx.setField(tracker, "Achievements.unlocked_count", unlocked.length);

  // Set progress to target for counter-based achievements (consistency)
  if (def.target > 0) {
    const progress = parseJSON(ctx.getField(tracker, "Achievements.progress"), {});
    progress[id] = def.target;
    ctx.setField(tracker, "Achievements.progress", JSON.stringify(progress));
  }

  ctx.emit("achievement-unlocked", { id, name: def.name, desc: def.desc });
});

// ---- achievement-progress: increment counter, auto-unlock at target ----
vitric.fn("__achievement_progress", (args, ctx) => {
  const tracker = args.tracker;
  const id = args.id;
  if (!tracker || !id) return;

  const defs = parseJSON(ctx.getField(tracker, "Achievements.defs"), []);
  const def = defs.find(d => d.id === id);
  if (!def) return; // undefined — silent no-op
  if (def.target <= 0) return; // instant-unlock achievement, progress is meaningless

  const unlocked = parseJSON(ctx.getField(tracker, "Achievements.unlocked"), []);
  if (unlocked.includes(id)) return; // already unlocked — no-op

  const amount = Number(args.amount) || 1;
  const progress = parseJSON(ctx.getField(tracker, "Achievements.progress"), {});
  const cur = (progress[id] || 0) + amount;
  progress[id] = cur;
  ctx.setField(tracker, "Achievements.progress", JSON.stringify(progress));

  if (cur >= def.target) {
    // Auto-unlock
    unlocked.push(id);
    ctx.setField(tracker, "Achievements.unlocked", JSON.stringify(unlocked));
    ctx.setField(tracker, "Achievements.unlocked_count", unlocked.length);
    ctx.emit("achievement-unlocked", { id, name: def.name, desc: def.desc });
  } else {
    ctx.emit("achievement-progress-updated", {
      id, progress: cur, target: def.target, name: def.name,
    });
  }
});