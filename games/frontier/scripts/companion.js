// 活伙伴(P4):走近按 t 说话 → think 系统拼人设 prompt 调 ctx.ask → 回复经 __onReply
// 分发到 companionReply → 发 companion-said 事件 → 规则把心情和话泡落到伙伴身上。
// LLM 回复走录制通道,这段对话离线重放逐位一致。

// wander 系统:伙伴平时在家附近慢慢溜达——走到一个随机目标点,停一会儿,再挑下一个。
// 这是"活"的最便宜来源,不烧 LLM。随机用确定性的 ctx.random()。
const WSPEED = 1.0;
vitric.system(
  "wander",
  { query: ["Companion", "Position", "Velocity", "Wander"], writes: ["Velocity", "Wander"] },
  (entities, ctx) => {
    for (const e of entities) {
      const w = e.Wander;
      const dx = w.tx - e.Position.x;
      const dy = w.ty - e.Position.y;
      const d = Math.sqrt(dx * dx + dy * dy);
      if (d > 0.15) {
        e.Velocity.x = (dx / d) * WSPEED;
        e.Velocity.y = (dy / d) * WSPEED;
      } else {
        e.Velocity.x = 0;
        e.Velocity.y = 0;
        w.timer -= ctx.dt;
        if (w.timer <= 0) {
          w.tx = w.home_x + (ctx.random() * 4 - 2); // 家附近 ±2 格
          w.ty = w.home_y + (ctx.random() * 4 - 2);
          w.timer = 1.5 + ctx.random() * 3; // 到点后停 1.5–4.5 秒
        }
      }
    }
  }
);

// need 系统:舒适需求随时间消长。有住所(quarters,由 feed-quarters 规则落进 Need.quarters)
// 就回舒适、没有就掉;舒适见底后还宽限 LEAVE_GRACE 秒(温和有预警),仍不管才真走(despawn)。
// 建个住所就能把舒适拉回来 → 留住人,体现"不满意就走、但能挽回"。
const COMFORT_UP = 4.0;
const COMFORT_DOWN = 4.0;
const LEAVE_GRACE = 8.0;
const WISH_AT = 22.0; // 舒适跌破这个值,伙伴会开口提一次愿望(走之前的预警)
vitric.system(
  "need",
  { query: ["Companion", "Need", "ThinkReq", "Mood"], writes: ["Need", "ThinkReq", "Mood"] },
  (entities, ctx) => {
    for (const e of entities) {
      const n = e.Need;
      const rate = n.quarters > 0 ? COMFORT_UP : -COMFORT_DOWN;
      n.comfort = Math.max(0, Math.min(100, n.comfort + rate * ctx.dt));
      n.comfort_i = Math.round(n.comfort); // HUD 显示用整数
      // 跌破阈值:开口提一次愿望(voiced 防刷屏;舒适回升后复位,下次还能提)
      if (n.comfort < WISH_AT && n.voiced < 1) {
        n.voiced = 1;
        e.Mood.value = "闷闷的";
        if (e.ThinkReq.pending < 1) e.ThinkReq.pending = 2; // 2 = 提愿望模式
      } else if (n.comfort >= WISH_AT) {
        n.voiced = 0;
      }
      if (n.comfort <= 0) {
        n.leave_timer += ctx.dt;
        // 宽限到了:发离开事件(由 do-leave 规则按名字 despawn,只发一次)。
        if (n.leave_timer >= LEAVE_GRACE && n.voiced < 2) {
          n.voiced = 2; // 复用 voiced 当"已宣告离开"标记,防每帧刷
          ctx.emit("companion-left", { who: e.id });
        }
      } else {
        n.leave_timer = 0;
      }
    }
  }
);

// think 系统:有人来搭话(ThinkReq.pending)就拼提示词问大模型,问完清掉 pending。
vitric.system(
  "companion-think",
  { query: ["Companion", "Persona", "Mood", "ThinkReq"], writes: ["ThinkReq"] },
  (entities, ctx) => {
    for (const e of entities) {
      const mode = e.ThinkReq.pending;
      if (mode < 1) continue;
      const p = e.Persona;
      const who = "你是" + p.name + "(" + p.archetype + ";" + p.traits + ";说话:" + p.speech + ")。";
      const fmt = "只回 JSON:{\"say\":\"一句话\",\"mood\":\"你此刻的心情(两到四个字)\"}";
      let prompt;
      if (mode === 2) {
        // 提愿望模式:住得不踏实,委婉跟玩家说出心愿(走之前的预警)
        prompt = who + "你在这儿住得不踏实——还没个能歇脚的地方,有点想走了。" +
          "用你的口吻,委婉地把这点心愿说给玩家听(别太重)。" + fmt;
      } else {
        prompt = who + "你此刻心情是「" + e.Mood.value + "」。玩家走过来跟你打招呼。用你的口吻回一句。" + fmt;
      }
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
