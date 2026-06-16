// 活伙伴(P4):走近按 t 说话 → think 系统拼人设 prompt 调 ctx.ask → 回复经 __onReply
// 分发到 companionReply → 发 companion-said 事件 → 规则把心情和话泡落到伙伴身上。
// LLM 回复走录制通道,这段对话离线重放逐位一致。

// think 系统:有人来搭话(ThinkReq.pending)就拼提示词问大模型,问完清掉 pending。
vitric.system(
  "companion-think",
  { query: ["Companion", "Persona", "Mood", "ThinkReq"], writes: ["ThinkReq"] },
  (entities, ctx) => {
    for (const e of entities) {
      if (e.ThinkReq.pending < 1) continue;
      const p = e.Persona;
      const prompt =
        "你是" + p.name + "(" + p.archetype + ";" + p.traits + ";说话:" + p.speech + ")。" +
        "你此刻心情是「" + e.Mood.value + "」。玩家走过来跟你打招呼。" +
        "用你的口吻回一句,只回 JSON:{\"say\":\"一句话\",\"mood\":\"你此刻的心情(两到四个字)\"}";
      ctx.ask("llm", prompt, "companionReply");
      e.ThinkReq.pending = 0;
    }
  }
);

// 回复回来(__onReply 按 id 转到这):解析 {say, mood},发事件让规则去落地。
vitric.fn("companionReply", (reply, ctx) => {
  const t = reply && reply.text;
  if (typeof t !== "string") {
    ctx.emit("companion-said", { say: "(没听清…)", mood: "平静" });
    return;
  }
  let say = "…";
  let mood = "平静";
  try {
    const r = JSON.parse(t);
    if (typeof r.say === "string") say = r.say;
    if (typeof r.mood === "string") mood = r.mood;
  } catch (err) {
    say = t.slice(0, 40); // 模型没回 JSON 也别崩,原样冒出来
  }
  ctx.emit("companion-said", { say: say, mood: mood });
});
