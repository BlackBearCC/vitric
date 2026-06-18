// 伙伴系统：游荡 + 舒适度 + 离开
// 三个子系统：
//   companion-wander  Wander + Velocity 驱动作息游荡
//   companion-need    Need 舒适衰减 → leave_timer 到时 emit companion-left
// 不依赖跨系统状态，全在组件里。

const WANDER_SPEED = 1.2;   // 散步速度
const WANDER_RADIUS = 2.5;  // 围绕 home_x/y ± 半径

// ---- 游荡：随 timer 换目标点，到了就停、等 timer 再动 ----
vitric.system("companion-wander", { query: ["Companion", "Wander", "Position", "Velocity"], writes: ["Wander", "Velocity"] }, (entities, ctx) => {
  for (const e of entities) {
    const w = e.Wander;
    const pos = e.Position;
    const vel = e.Velocity;
    w.timer -= ctx.dt;
    if (w.timer > 0) continue; // 还在等待（或走在路上——走到才停）
    // 已到目标或 timer 耗尽 → 新目标
    const dx = w.tx - pos.x;
    const dy = w.ty - pos.y;
    if (dx * dx + dy * dy < 0.25) {
      // 到了：停住、等下一次（离家近才动）
      vel.x = 0;
      vel.y = 0;
      // 在家里附近随机新目标
      const hx = w.home_x || pos.x;
      const hy = w.home_y || pos.y;
      w.tx = hx + (ctx.random() * 2 - 1) * WANDER_RADIUS;
      w.ty = hy + (ctx.random() * 2 - 1) * WANDER_RADIUS;
      w.timer = 3 + ctx.random() * 4; // 停 3-7 秒再动
    } else {
      // 朝目标走
      const dist = Math.sqrt(dx * dx + dy * dy) || 0.001;
      vel.x = (dx / dist) * WANDER_SPEED;
      vel.y = (dy / dist) * WANDER_SPEED;
      w.timer = 0.5; // 下次再检查距离
    }
  }
});

// ---- 舒适度：规则层把 Need.quarters 设为 1/0（有 shelter 则 1），这里据此处理 ----
// 有 shelter(quarters>0)：舒适度缓慢恢复。无 shelter：衰减 → leave_timer → 离开。
vitric.system("companion-need", { query: ["Companion", "Need"], writes: ["Need"] }, (entities, ctx) => {
  for (const e of entities) {
    const n = e.Need;
    if (n.quarters > 0) {
      // 有 shelter：舒适度缓慢恢复（上限 100），leave_timer 归零
      n.comfort += 0.05 * ctx.dt;
      n.leave_timer = 0;
    } else {
      // 无 shelter：舒适度衰减
      n.comfort -= 0.08 * ctx.dt;
      if (n.comfort <= 0) {
        n.leave_timer += ctx.dt;
        if (n.leave_timer >= 15) {
          ctx.emit("companion-left", { name: e.Persona ? e.Persona.name : "未知旅人" });
          n.leave_timer = 0;
        }
      } else {
        n.leave_timer = 0;
      }
    }
    n.comfort = n.comfort < 0 ? 0 : (n.comfort > 100 ? 100 : n.comfort);
    n.comfort_i = Math.round(n.comfort);
  }
});

// ---- 邀请旅人：规则收到 companion-invited 后调这个 fn ----
// 把 @drifter despawn 掉（规则层做），这里在家园附近 spawn 一个带 Companion 的新实体
vitric.fn("spawnCompanion", (args, ctx) => {
  const persona = args.persona || {};
  const homeName = persona.name || "旅人";
  // 在家园左侧 spawn
  const sx = 5 + Math.round(ctx.random() * 4);
  const sy = 5 + Math.round(ctx.random() * 4);
  ctx.spawn({
    Companion: {},
    Persona: { name: homeName, archetype: persona.archetype || "", traits: persona.traits || "", speech: persona.speech || "" },
    Mood: { value: "平静" },
    ThinkReq: { pending: 0 },
    Need: { comfort: 60, quarters: 0, leave_timer: 0, voiced: 0, comfort_i: 60 },
    Wander: { home_x: sx, home_y: sy, tx: sx, ty: sy, timer: 2 },
    Position: { x: sx, y: sy },
    Velocity: { x: 0, y: 0 },
    Sprite: { w: 0.9, h: 0.9, color: "#d4a06a" },
    Text: { content: "", size: 0.7, color: "#ffe9b0" },
    Census: { count: 0, is_hub: 0 },
  });
  ctx.emit("companion-moved-in", { name: homeName });
});

// ---- 对话(按 t):规则把玩家和目标的坐标+人设传进来,靠近(<4)则发 LLM 对话 ----
vitric.fn("talkNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return; // 距离 > 4,不触发
  const name = args.pname || "旅人";
  const arch = args.parch || "漂泊者";
  const traits = args.ptraits || "沉默";
  const speech = args.speech || "寡言";
  const prompt = "你是一个在荒星漂泊的旅人" + name + "(" + arch + ")，性格" + traits + "，说话" + speech + "。一个拓荒者走近了你，你主动说句话打个招呼。请用 JSON 格式回复：{\"say\":\"你说的话\",\"mood\":\"情绪\"}";
  const target = args.entity || "drifter";
  ctx.ask("llm", prompt, "on" + target.charAt(0).toUpperCase() + target.slice(1) + "Reply");
});

// ---- 邀请(按 i):靠近旅人则发邀请事件 ----
vitric.fn("inviteNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return;
  ctx.emit("companion-invited", {
    name: args.pname || "旅人",
    archetype: args.parch || "",
    traits: args.ptraits || "",
    speech: args.pspeech || "",
  });
});

// ---- LLM 回复设定,按实体分开(disp + text 写回) ----
vitric.fn("onDrifterReply", (reply, ctx) => {
  const text = reply.text || "（旅人点了点头）";
  // 尝试解析 JSON
  let display = text;
  try {
    const parsed = JSON.parse(text);
    if (parsed.say) display = parsed.say;
  } catch (_) { /* 非 JSON,直接显示原文 */ }
  ctx.setField("drifter", "Text.content", display);
});

vitric.fn("onCompanionReply", (reply, ctx) => {
  const text = reply.text || "（伙伴笑了笑）";
  let display = text;
  try {
    const parsed = JSON.parse(text);
    if (parsed.say) display = parsed.say;
  } catch (_) {}
  ctx.setField("companion", "Text.content", display);
});
