// Companion wish system: each companion has 3 role-based wishes.
// Wishes advance via gameplay events (built/harvested/gathered/entered-poi/upgrade/food-high/see-dawn).
// Fulfilling a wish: +30 affinity, Colony.companion_wish_count++, emit wish-fulfilled.
// Task 5 wires wish-fulfilled to LLM memory dialogue; this task only advances + emits.

// Wish advancement is a fn (not a system) because it's triggered by discrete gameplay events
// caught by rules/wish.json. The fn reads Colony.companion_handles (maintained by the
// companion-register system in companion.js), iterates each companion, reads/advances Wish.items.

const AFFINITY_GAIN_PER_WISH = 30;

// Note: WISH_TEMPLATES, wishesForRole(), wishesForArchetype() live in companion.js (loaded first).
// QuickJS scripts share a single global scope, so wish.js can call them directly without re-declaring.

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

// ---- LLM memory dialogue: when a wish is fulfilled, ask the LLM for a memory ----
// The companion shares a 1-2 sentence memory about their past, unlocked by the wish fulfillment.
// Flow: rule catches wish-fulfilled -> calls triggerWishMemory -> ctx.ask("llm", prompt, "onWishMemoryReply")
//   -> onWishMemoryReply applies the reply (increments memory_unlocked, displays via toast + last_talk_reply).
// On LLM error, the engine's llm-error-handler rule routes to __onReply with a generic fallback text;
// onWishMemoryReply detects the fallback and substitutes an archetype-specific canned line.

// Archetype-specific fallback memories (deterministic, used when LLM is unavailable).
// Indexed by [archetypeKey][memoryIndex % list.length].
const MEMORY_FALLBACKS = {
  builder: [
    "我父亲是木匠,他教过我榫卯,说木头是有脾气的。",
    "这双手建过更高的塔,那时候还有脚手架。",
    "砖石会记得建造者,这是我师傅说的。",
  ],
  farmer: [
    "麦浪的声音我永远忘不掉,家乡的秋天全是金的。",
    "母亲做过更好的面包,加了蜂蜜的那种。",
    "雨水总是最好的礼物,特别是播完种之后。",
  ],
  explorer: [
    "我记得第一次看见星空的那晚,那时我还在逃。",
    "从前我也走过更远的路,比这片荒原更远。",
    "家乡的山比这里更高,山顶常年有雪。",
  ],
};

// Detect the LLM error fallback text (set by rules/companion.json llm-error-handler).
const LLM_ERROR_FALLBACK = "（旅人沉默片刻,点了点头）";

vitric.fn("triggerWishMemory", (a, ctx) => {
  const handle = a.entity || "";
  if (!handle) return;
  // Read companion Persona fields for the prompt.
  const name = ctx.getField(handle, "Persona.name") || "伙伴";
  const archetype = ctx.getField(handle, "Persona.archetype") || "";
  const traits = ctx.getField(handle, "Persona.traits") || "";
  const speech = ctx.getField(handle, "Persona.speech") || "";
  const memCount = (ctx.getField(handle, "Need.memory_unlocked") | 0) + 1; // 1-indexed for prompt

  const prompt = [
    "你是一个在荒星生存的伙伴,名叫" + name + "。",
    "性格:" + archetype + "," + traits + "。",
    "说话风格:" + speech + "。",
    "玩家刚刚帮你完成了心愿:\"" + (a.wish_desc || "") + "\"。",
    "这是你解锁的第 " + memCount + " 段记忆。请用 1-2 句话分享一段关于你过去的回忆,语气符合你的性格,不要超过 60 字。直接输出回忆内容,不要加引号或前缀。",
  ].join("\n");

  // Stash target handle so the callback knows who to update.
  ctx.setField("colony", "Colony.last_wish_memory_target", handle);
  ctx.ask("llm", prompt, "onWishMemoryReply");
});

// LLM callback: applies the memory dialogue reply.
// - Increments Need.memory_unlocked on the stashed target.
// - Sets Colony.last_talk_reply so the existing talk-reply-apply-* systems display it above the companion.
// - Emits toast-show with the memory text.
// - Emits memory-unlocked event (for future use / UI).
// On LLM error (detected via fallback text), substitutes an archetype-specific canned line.
vitric.fn("onWishMemoryReply", (reply, ctx) => {
  const handle = ctx.getField("colony", "Colony.last_wish_memory_target") || "";
  if (!handle) return;

  let text = (reply && reply.text) || "";
  // Detect LLM error fallback and substitute archetype-specific canned line.
  if (!text || text === LLM_ERROR_FALLBACK) {
    const archetype = ctx.getField(handle, "Persona.archetype") || "";
    let key = "explorer";
    if (/技|电|匠|build|builder/i.test(archetype)) key = "builder";
    else if (/厨|医|农|farm|farmer/i.test(archetype)) key = "farmer";
    const list = MEMORY_FALLBACKS[key] || MEMORY_FALLBACKS.explorer;
    const memCount = ctx.getField(handle, "Need.memory_unlocked") | 0;
    text = list[memCount % list.length];
  }

  // Apply: increment memory_unlocked, display, notify.
  const memCount = (ctx.getField(handle, "Need.memory_unlocked") | 0) + 1;
  ctx.setField(handle, "Need.memory_unlocked", memCount);
  ctx.setField("colony", "Colony.last_talk_reply", text);
  const name = ctx.getField(handle, "Persona.name") || "伙伴";
  ctx.emit("toast-show", { text: name + ": " + text });
  ctx.emit("memory-unlocked", { name: name, text: text, entity: handle });

  // Clear the stash so a stale target isn't reused.
  ctx.setField("colony", "Colony.last_wish_memory_target", "");
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

// ---- Collective wish (Task 9): granary-50 colony milestone ----
// One-time: when Colony.food_i >= 50 AND collective_wish_done == 0:
//   - mark collective_wish_done = 1
//   - +10 affinity to all companions (via Colony.companion_handles)
//   - emit collective-wish-fulfilled + toast-show
// The HUD rule hud-collective-wish-* (rules/hud.json) reads collective_wish_done to swap the label.
const COLLECTIVE_WISH_THRESHOLD = 50;
const COLLECTIVE_WISH_AFFINITY_GAIN = 10;
vitric.system("collective-wish-check", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  if ((c.Colony.collective_wish_done | 0) !== 0) return;
  if ((c.Colony.food_i | 0) < COLLECTIVE_WISH_THRESHOLD) return;
  // Fulfill: mark done, buff all companions, emit event.
  ctx.setField("colony", "Colony.collective_wish_done", 1);
  const handles = ctx.getField("colony", "Colony.companion_handles") || [];
  for (const h of handles) {
    if (!h) continue;
    const aff = ctx.getField(h, "Need.affinity");
    const affNum = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
    const newAff = Math.min(100, affNum + COLLECTIVE_WISH_AFFINITY_GAIN);
    ctx.setField(h, "Need.affinity", newAff);
    ctx.setField(h, "Need.affinity_i", Math.round(newAff));
  }
  ctx.emit("collective-wish-fulfilled", { threshold: COLLECTIVE_WISH_THRESHOLD });
  ctx.emit("toast-show", { text: "共识达成: 粮储达到 " + COLLECTIVE_WISH_THRESHOLD + "!" });
});
