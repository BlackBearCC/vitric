# Task 5 Review Package

## Commit range

BASE = e01d4c0 (Task 4 complete)
HEAD = eb0ac67 (Task 5)

## Commits

```
eb0ac67 feat(frontier): wire LLM memory dialogue to wish fulfillment
```

## Diff stat

```
 games/frontier/rules/wish.json |  9 +++++
 games/frontier/scripts/wish.js | 87 ++++++++++++++++++++++++++++++++++++++++++
 2 files changed, 96 insertions(+)
```

## Full diff (with 10 lines of context)

```diff
diff --git a/games/frontier/rules/wish.json b/games/frontier/rules/wish.json
index 6e604c8..6475fef 100644
--- a/games/frontier/rules/wish.json
+++ b/games/frontier/rules/wish.json
@@ -57,20 +57,29 @@
     },
     {
       "id": "wish-fulfilled-toast",
       "comment": "Wish fulfilled -> toast notification.",
       "on": { "event": "wish-fulfilled" },
       "do": [
         { "set": "@toast_lbl.UiLabel.content", "to": { "format": "{} 心愿达成: {}", "args": ["event.companion", "event.wish_desc"] } },
         { "set": "@toast_lbl.Toast.timer", "to": 3.0 }
       ]
     },
+    {
+      "id": "wish-fulfilled-memory",
+      "comment": "Wish fulfilled -> trigger LLM memory dialogue (companion shares a past memory).",
+      "on": { "event": "wish-fulfilled" },
+      "do": [ { "call": "triggerWishMemory", "with": {
+        "entity": "event.entity",
+        "wish_desc": "event.wish_desc"
+      } } ]
+    },
     {
       "id": "toast-show-generic",
       "comment": "Generic toast-show listener (resolves Task 3 F2): any script can emit toast-show{text} and this lands it on the toast label.",
       "on": { "event": "toast-show" },
       "do": [
         { "set": "@toast_lbl.UiLabel.content", "to": "event.text" },
         { "set": "@toast_lbl.Toast.timer", "to": 2.5 }
       ]
     },
     {
diff --git a/games/frontier/scripts/wish.js b/games/frontier/scripts/wish.js
index f279ff4..62a9460 100644
--- a/games/frontier/scripts/wish.js
+++ b/games/frontier/scripts/wish.js
@@ -45,20 +45,107 @@ vitric.fn("advance_wish", (a, ctx) => {
         // Emit for Task 5 (LLM memory dialogue) + toast.
         const name = ctx.getField(h, "Persona.name") || "伙伴";
         ctx.emit("wish-fulfilled", { companion: name, wish_desc: it.desc || kind, entity: h });
       }
       changed = true;
     }
     if (changed) ctx.setField(h, "Wish.items", JSON.stringify(items));
   }
 });
 
+// ---- LLM memory dialogue: when a wish is fulfilled, ask the LLM for a memory ----
+// The companion shares a 1-2 sentence memory about their past, unlocked by the wish fulfillment.
+// Flow: rule catches wish-fulfilled -> calls triggerWishMemory -> ctx.ask("llm", prompt, "onWishMemoryReply")
+//   -> onWishMemoryReply applies the reply (increments memory_unlocked, displays via toast + last_talk_reply).
+// On LLM error, the engine's llm-error-handler rule routes to __onReply with a generic fallback text;
+// onWishMemoryReply detects the fallback and substitutes an archetype-specific canned line.
+
+// Archetype-specific fallback memories (deterministic, used when LLM is unavailable).
+// Indexed by [archetypeKey][memoryIndex % list.length].
+const MEMORY_FALLBACKS = {
+  builder: [
+    "我父亲是木匠,他教过我榫卯,说木头是有脾气的。",
+    "这双手建过更高的塔,那时候还有脚手架。",
+    "砖石会记得建造者,这是我师傅说的。",
+  ],
+  farmer: [
+    "麦浪的声音我永远忘不掉,家乡的秋天全是金的。",
+    "母亲做过更好的面包,加了蜂蜜的那种。",
+    "雨水总是最好的礼物,特别是播完种之后。",
+  ],
+  explorer: [
+    "我记得第一次看见星空的那晚,那时我还在逃。",
+    "从前我也走过更远的路,比这片荒原更远。",
+    "家乡的山比这里更高,山顶常年有雪。",
+  ],
+};
+
+// Detect the LLM error fallback text (set by rules/companion.json llm-error-handler).
+const LLM_ERROR_FALLBACK = "（旅人沉默片刻,点了点头）";
+
+vitric.fn("triggerWishMemory", (a, ctx) => {
+  const handle = a.entity || "";
+  if (!handle) return;
+  // Read companion Persona fields for the prompt.
+  const name = ctx.getField(handle, "Persona.name") || "伙伴";
+  const archetype = ctx.getField(handle, "Persona.archetype") || "";
+  const traits = ctx.getField(handle, "Persona.traits") || "";
+  const speech = ctx.getField(handle, "Persona.speech") || "";
+  const memCount = (ctx.getField(handle, "Need.memory_unlocked") | 0) + 1; // 1-indexed for prompt
+
+  const prompt = [
+    "你是一个在荒星生存的伙伴,名叫" + name + "。",
+    "性格:" + archetype + "," + traits + "。",
+    "说话风格:" + speech + "。",
+    "玩家刚刚帮你完成了心愿:\"" + (a.wish_desc || "") + "\"。",
+    "这是你解锁的第 " + memCount + " 段记忆。请用 1-2 句话分享一段关于你过去的回忆,语气符合你的性格,不要超过 60 字。直接输出回忆内容,不要加引号或前缀。",
+  ].join("\n");
+
+  // Stash target handle so the callback knows who to update.
+  ctx.setField("colony", "Colony.last_wish_memory_target", handle);
+  ctx.ask("llm", prompt, "onWishMemoryReply");
+});
+
+// LLM callback: applies the memory dialogue reply.
+// - Increments Need.memory_unlocked on the stashed target.
+// - Sets Colony.last_talk_reply so the existing talk-reply-apply-* systems display it above the companion.
+// - Emits toast-show with the memory text.
+// - Emits memory-unlocked event (for future use / UI).
+// On LLM error (detected via fallback text), substitutes an archetype-specific canned line.
+vitric.fn("onWishMemoryReply", (reply, ctx) => {
+  const handle = ctx.getField("colony", "Colony.last_wish_memory_target") || "";
+  if (!handle) return;
+
+  let text = (reply && reply.text) || "";
+  // Detect LLM error fallback and substitute archetype-specific canned line.
+  if (!text || text === LLM_ERROR_FALLBACK) {
+    const archetype = ctx.getField(handle, "Persona.archetype") || "";
+    let key = "explorer";
+    if (/技|电|匠|build|builder/i.test(archetype)) key = "builder";
+    else if (/厨|医|农|farm|farmer/i.test(archetype)) key = "farmer";
+    const list = MEMORY_FALLBACKS[key] || MEMORY_FALLBACKS.explorer;
+    const memCount = ctx.getField(handle, "Need.memory_unlocked") | 0;
+    text = list[memCount % list.length];
+  }
+
+  // Apply: increment memory_unlocked, display, notify.
+  const memCount = (ctx.getField(handle, "Need.memory_unlocked") | 0) + 1;
+  ctx.setField(handle, "Need.memory_unlocked", memCount);
+  ctx.setField("colony", "Colony.last_talk_reply", text);
+  const name = ctx.getField(handle, "Persona.name") || "伙伴";
+  ctx.emit("toast-show", { text: name + ": " + text });
+  ctx.emit("memory-unlocked", { name: name, text: text, entity: handle });
+
+  // Clear the stash so a stale target isn't reused.
+  ctx.setField("colony", "Colony.last_wish_memory_target", "");
+});
+
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
```
