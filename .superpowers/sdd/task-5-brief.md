# Task 5: LLM Memory Dialogue Wired to Wish Fulfillment

## Context

Vitric frontier deepening, Task 5 of 10. When a companion's wish is fulfilled (Task 4 emits `wish-fulfilled`), trigger an LLM-generated memory dialogue: the companion shares a 1-2 sentence memory about their past. This deepens the companion relationship loop.

Tasks 1-4 are committed on `main` (HEAD = `e01d4c0`).

## LLM API (confirmed from companion.js + prelude.js + companion.json)

The engine's LLM flow:
1. Script calls `ctx.ask("llm", prompt, "callbackFnName")` — engine emits `llm-ask` event with id like `callbackFnName#tick#N`
2. LLM service processes it, emits `llm-reply` event with `{id, text}`
3. Rule `llm-reply-router` in `rules/companion.json` catches `llm-reply` and calls built-in `__onReply` with `{id, text}`
4. `__onReply` (in `crates/vitric-script/src/prelude.js`) extracts callback name from id prefix, calls the registered `vitric.fn("callbackFnName", (reply, ctx) => {...})` where `reply = {id, text}`
5. On LLM error, `llm-error` event fires; rule `llm-error-handler` calls `__onReply` with a generic fallback text `"（旅人沉默片刻,点了点头）"` — this still routes to the callback fn

Existing example (companion.js talkNearby → onTalkReply):
```javascript
vitric.fn("talkNearby", (args, ctx) => {
  // ... build prompt ...
  ctx.setField("colony", "Colony.last_talk_target", pid);  // stash target for callback
  ctx.ask("llm", prompt, "onTalkReply");
});

vitric.fn("onTalkReply", (reply, ctx) => {
  const text = reply.text || "（对方点了点头）";
  // ... apply to target ...
});
```

## Files

- **Modify:** `games/frontier/scripts/wish.js` — add `triggerWishMemory` fn + `onWishMemoryReply` callback fn
- **Modify:** `games/frontier/rules/wish.json` — add rule catching `wish-fulfilled` → calls `triggerWishMemory`

No new files. No schema changes (uses existing `Need.memory_unlocked` from Task 1, and ad-hoc `Colony.last_wish_memory_target` following the `Colony.last_talk_target` pattern).

## Step 1: Add `triggerWishMemory` fn to `games/frontier/scripts/wish.js`

Add this after the existing `advance_wish` fn (before `apply_mood_drop` or after — your choice, but keep fns grouped):

```javascript
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
```

## Step 2: Add `onWishMemoryReply` callback fn to `games/frontier/scripts/wish.js`

Add this after `triggerWishMemory`:

```javascript
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
```

## Step 3: Add rule to `games/frontier/rules/wish.json`

Add this rule to the `rules` array in `wish.json` (after the existing `wish-fulfilled-toast` rule):

```json
    {
      "id": "wish-fulfilled-memory",
      "comment": "Wish fulfilled -> trigger LLM memory dialogue (companion shares a past memory).",
      "on": { "event": "wish-fulfilled" },
      "do": [ { "call": "triggerWishMemory", "with": {
        "entity": "event.entity",
        "wish_desc": "event.wish_desc"
      } } ]
    }
```

**Note:** This rule and the existing `wish-fulfilled-toast` rule both listen for `wish-fulfilled`. Both will fire — the toast shows the wish completion notification, and the memory rule triggers the LLM dialogue. This is intended (two independent effects of the same event).

## Step 4: Verify

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -20`
Expected: exit code 0.

If check fails, read the error output and fix. Common issues:
- Script syntax error in wish.js
- `onWishMemoryReply` fn not registered (verify it's `vitric.fn(...)`)
- Rule references unknown event/fn

## Step 5: Commit + push

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/wish.js games/frontier/rules/wish.json
git commit -m "feat(frontier): wire LLM memory dialogue to wish fulfillment"
git push origin main
```

## Self-Review Checklist

- [ ] `triggerWishMemory` fn registered via `vitric.fn`.
- [ ] `onWishMemoryReply` fn registered via `vitric.fn`.
- [ ] `triggerWishMemory` reads Persona fields via `ctx.getField`, builds prompt, stashes handle in `Colony.last_wish_memory_target`, calls `ctx.ask("llm", prompt, "onWishMemoryReply")`.
- [ ] `onWishMemoryReply` reads stashed handle, detects LLM error fallback, substitutes archetype-specific canned line, increments `Need.memory_unlocked`, sets `Colony.last_talk_reply`, emits `toast-show` + `memory-unlocked`, clears stash.
- [ ] `wish.json` has `wish-fulfilled-memory` rule catching `wish-fulfilled` and calling `triggerWishMemory` with `event.entity` + `event.wish_desc`.
- [ ] No fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`).
- [ ] All comments in wish.js additions are English.
- [ ] String literals (prompt text, fallback memories, toast text) stay Chinese.
- [ ] `vitric check games/frontier` exits 0.
- [ ] Commit pushed to origin/main.

## Report Contract

Write your full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-5-report.md` containing:
1. What was implemented (files + 1-line summary each)
2. Verification output (`vitric check` exit code + last 10 lines)
3. Commit SHA(s) pushed
4. Any concerns or deviations from the brief

Return in your final message: STATUS (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit SHA(s), one-line test summary, and any concerns.
