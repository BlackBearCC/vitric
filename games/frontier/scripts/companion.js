// 伙伴系统:游荡 + 舒适度 + 离开 + 日夜作息 + 多旅人/多伙伴目标追踪
// 三个子系统:
//   companion-wander     Wander + Velocity 驱动作息游荡
//   companion-need       Need 舒适衰减/恢复 → comfort 跌到 0 → leave_timer 到时 emit companion-left + 自己 despawn
//   target-track         每帧维护 @colony.Colony.target_drifter / target_companion(就近的旅人/伙伴)
//   companion-shelter    每帧按 Colony.struct_count 给所有 Companion.Need.quarters 同步
//   companion-register   每帧维护 Colony.companion_handles 列表(增/减),让其他系统能遍历所有伙伴
// 不依赖跨系统状态,全在组件里 + Colony 共享字段。

const WANDER_SPEED = 1.2;   // 散步速度
const WANDER_RADIUS = 2.5;  // 围绕 home_x/y ± 半径
const COMP_DAY_SEC = 60.0;
const COMP_TICK_PER_SEC = 60;

function compTodOf(tick) {
  const secOfDay = (tick / COMP_TICK_PER_SEC) % COMP_DAY_SEC;
  const frac = secOfDay / COMP_DAY_SEC;
  if (frac < 0.25) return "晨";
  if (frac < 0.50) return "午";
  if (frac < 0.75) return "昏";
  return "夜";
}

// 闭包共享的玩家位置(由 cache-player-pos 写,由 target-track 读)
let __playerX = 0;
let __playerY = 0;

// ---- 缓存玩家位置到 Colony(target-track 需要)----
vitric.system("cache-player-pos", { query: ["Player", "Position"], writes: ["Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if (!e.Player) continue;
    __playerX = e.Position.x;
    __playerY = e.Position.y;
    ctx.setField("colony", "Colony.player_x", e.Position.x);
    ctx.setField("colony", "Colony.player_y", e.Position.y);
  }
});

// ---- 维护 Colony.companion_handles 列表 ----
// 扫描所有 Companion 实体,出现在列表里的去掉,新出现的加上,被 despawn 的从列表里清掉。
// 同时去重,避免重复。
vitric.system("companion-register", { query: ["Companion"], writes: [] }, (entities, ctx) => {
  // 当前在场的伙伴句柄
  const alive = entities.map(e => e.id);
  // 读 Colony.companion_handles — 但这里不能读 Colony。改用闭包。
  // 闭包：每 tick 重置 alive set,重算 diff
  const prev = __companionRegistry;
  __companionRegistry = alive.slice();
  // 把当前 alive 写回 Colony(整列替换)
  ctx.setField("colony", "Colony.companion_handles", alive);
});
let __companionRegistry = [];

// ---- 同步所有伙伴的 Need.quarters(基于 Colony.struct_count) ----
// 这里不能直接读 Colony,所以用 @colony.* 的 setField 也不行（写不进读）。
// 解决办法：用 companion-shelter-collect 系统先收集 struct_count 到闭包，
//         companion-shelter 系统再迭代 Colony.companion_handles 设置每个的 Need.quarters。
let __structCount = 0;
vitric.system("companion-shelter-collect", { query: ["Structure"], writes: [] }, (entities, ctx) => {
  __structCount = entities.length;
});

// 现在 iterate companion_handles 这个 list 给每个 setField Need.quarters
// 但 setField 是延迟的,需要 fn 来执行循环。这里用一个系统,它 query Companion,
// 从闭包读 __structCount,直接写 Need.quarters。
vitric.system("companion-shelter", { query: ["Companion", "Need"], writes: ["Need"] }, (entities, ctx) => {
  const q = __structCount > 0 ? 1 : 0;
  for (const e of entities) {
    e.Need.quarters = q;
  }
});

// ---- 维护最近旅人:扫描所有 Drifter,找离玩家最近的写入 Colony.target_drifter* ----
vitric.system("target-drifter", { query: ["Drifter", "Position", "Persona"], writes: [] }, (entities, ctx) => {
  let bestD2 = Infinity;
  let best = null;
  for (const e of entities) {
    const dx = e.Position.x - __playerX;
    const dy = e.Position.y - __playerY;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = e; }
  }
  if (best) {
    ctx.setField("colony", "Colony.target_drifter", best.id);
    ctx.setField("colony", "Colony.target_drifter_x", best.Position.x);
    ctx.setField("colony", "Colony.target_drifter_y", best.Position.y);
    ctx.setField("colony", "Colony.target_drifter_name", best.Persona.name || "");
    ctx.setField("colony", "Colony.target_drifter_archetype", best.Persona.archetype || "");
    ctx.setField("colony", "Colony.target_drifter_traits", best.Persona.traits || "");
    ctx.setField("colony", "Colony.target_drifter_speech", best.Persona.speech || "");
  } else {
    ctx.setField("colony", "Colony.target_drifter", "");
  }
});

// ---- 维护最近伙伴 ----
vitric.system("target-companion", { query: ["Companion", "Position", "Persona"], writes: [] }, (entities, ctx) => {
  let bestD2 = Infinity;
  let best = null;
  for (const e of entities) {
    const dx = e.Position.x - __playerX;
    const dy = e.Position.y - __playerY;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = e; }
  }
  if (best) {
    ctx.setField("colony", "Colony.target_companion", best.id);
    ctx.setField("colony", "Colony.target_companion_x", best.Position.x);
    ctx.setField("colony", "Colony.target_companion_y", best.Position.y);
    ctx.setField("colony", "Colony.target_companion_name", best.Persona.name || "");
    ctx.setField("colony", "Colony.target_companion_archetype", best.Persona.archetype || "");
    ctx.setField("colony", "Colony.target_companion_traits", best.Persona.traits || "");
    ctx.setField("colony", "Colony.target_companion_speech", best.Persona.speech || "");
  } else {
    ctx.setField("colony", "Colony.target_companion", "");
  }
});

// ---- 游荡:随 timer 换目标点,到了就停、等 timer 再动 ----
vitric.system("companion-wander", { query: ["Companion", "Wander", "Position", "Velocity"], writes: ["Wander", "Velocity"] }, (entities, ctx) => {
  for (const e of entities) {
    const w = e.Wander;
    const pos = e.Position;
    const vel = e.Velocity;
    w.timer -= ctx.dt;
    if (w.timer > 0) continue;
    const dx = w.tx - pos.x;
    const dy = w.ty - pos.y;
    if (dx * dx + dy * dy < 0.25) {
      vel.x = 0;
      vel.y = 0;
      const hx = w.home_x || pos.x;
      const hy = w.home_y || pos.y;
      w.tx = hx + (ctx.random() * 2 - 1) * WANDER_RADIUS;
      w.ty = hy + (ctx.random() * 2 - 1) * WANDER_RADIUS;
      w.timer = 3 + ctx.random() * 4;
    } else {
      const dist = Math.sqrt(dx * dx + dy * dy) || 0.001;
      vel.x = (dx / dist) * WANDER_SPEED;
      vel.y = (dy / dist) * WANDER_SPEED;
      w.timer = 0.5;
    }
  }
});

// ---- 舒适度:日夜节奏 + 住所 ----
// 白天:无 shelter 缓慢衰减;有 shelter 缓慢恢复。
// 夜里:无 shelter 快速衰减;有 shelter 加速恢复。
// comfort 跌到 0 → leave_timer 累计,15 秒 → 直接 despawn 自己
vitric.system("companion-need", { query: ["Companion", "Need"], writes: ["Need"] }, (entities, ctx) => {
  const tod = compTodOf(ctx.tick);
  const isNight = tod === "夜";
  for (const e of entities) {
    const n = e.Need;
    if (n.quarters > 0) {
      n.comfort += (isNight ? 0.20 : 0.05) * ctx.dt;
      n.leave_timer = 0;
    } else {
      n.comfort -= (isNight ? 0.20 : 0.08) * ctx.dt;
      if (n.comfort <= 0) {
        n.leave_timer += ctx.dt;
        if (n.leave_timer >= 15) {
          ctx.emit("companion-left", { name: e.Persona ? e.Persona.name : "未知旅人" });
          ctx.despawn(e.id);
          continue;
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
// 把传入的 drifter_id despawn 掉,然后在玩家附近 spawn 一个带 Companion 的新实体。
// 名字由 rules 通过 persona.name 传入。
vitric.fn("consumeDrifter", (args, ctx) => {
  if (args.drifter_id) ctx.despawn(args.drifter_id);
  const persona = {
    name: args.name || "旅人",
    archetype: args.archetype || "",
    traits: args.traits || "",
    speech: args.speech || "",
  };
  const sx = 5 + Math.round(ctx.random() * 4);
  const sy = 5 + Math.round(ctx.random() * 4);
  ctx.spawn({
    Companion: {},
    Persona: persona,
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
  ctx.emit("companion-moved-in", { name: persona.name });
});

// ---- 旅人到来：每个 game day 由 day-start 事件触发，根据 index 派一个人设 ----
// 固定人设池（确定性 → 重放/录像一致）。最多 4 个。每个 spawn 不命名（避免与已有 @drifter 冲突）。
const DRIFTER_POOL = [
  { name: "Mira",  archetype: "山地步兵",   traits: "坚忍,寡言,脚步轻",       speech: "简短、爱用省略号" },
  { name: "Kade",  archetype: "电工学徒",   traits: "好奇、爱拆东西、胆大",   speech: "语速快、夹英文" },
  { name: "Sori",  archetype: "老年医师",   traits: "慈祥、念叨、健忘",       speech: "慢热、爱讲从前" },
  { name: "Vex",   archetype: "游戏少年",   traits: "浮躁、爱吹、爱笑",       speech: "夸张、表情符号感" },
];
vitric.fn("spawnNewDrifter", (args, ctx) => {
  const idx = (args.idx | 0) || 0;
  const persona = DRIFTER_POOL[idx % DRIFTER_POOL.length];
  // 野外区域（x>=16），避免与已有资源点重合
  const sx = 17 + Math.round(ctx.random() * 9); // 17..26
  const sy = 2 + Math.round(ctx.random() * 8);  // 2..10
  ctx.spawn({
    Drifter: { arrival_day: args.arrival_day | 0 },
    Persona: persona,
    Mood: { value: "好奇" },
    ThinkReq: { pending: 0 },
    Position: { x: sx, y: sy },
    Collider: { w: 0.9, h: 0.9 },
    Sprite: { w: 0.9, h: 0.9, color: "#d4a06a" },
    Text: { content: "", size: 0.7, color: "#ffe9b0" },
  });
  ctx.emit("drifter-arrived", { name: persona.name, x: sx, y: sy });
});

// ---- 对话(按 t):靠近目标则发 LLM 对话 ----
// 距离检查 + 用 LLM 生成对白。
vitric.fn("talkNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return;
  const name = args.pname || "旅人";
  const arch = args.parch || "漂泊者";
  const traits = args.ptraits || "沉默";
  const speech = args.speech || "寡言";
  const prompt = "你是一个在荒星漂泊的旅人" + name + "(" + arch + ")，性格" + traits + "，说话" + speech + "。一个拓荒者走近了你，你主动说句话打个招呼。请用 JSON 格式回复：{\"say\":\"你说的话\",\"mood\":\"情绪\"}";
  ctx.ask("llm", prompt, "onTalkReply");
});

// ---- 邀请(按 i):靠近目标则发邀请事件 ----
vitric.fn("inviteAnyNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return;
  if (!args.drifter_id) return;
  ctx.emit("companion-invited", {
    drifter_id: args.drifter_id,
    name: args.pname || "旅人",
    archetype: args.parch || "",
    traits: args.ptraits || "",
    speech: args.pspeech || "",
  });
});

// ---- LLM 回复通用：把回复存到 Colony.last_talk_reply，
// talk-reply-apply 系统每帧把它落到目标实体的 Text.content 上 ----
vitric.fn("onTalkReply", (reply, ctx) => {
  const text = reply.text || "（对方点了点头）";
  let display = text;
  try {
    const parsed = JSON.parse(text);
    if (parsed.say) display = parsed.say;
  } catch (_) {}
  ctx.setField("colony", "Colony.last_talk_reply", display);
});
let __lastTalkReply = "";
vitric.system("talk-reply-apply-drifter", { query: ["Drifter", "Text", "Position"], writes: ["Text"] }, (entities, ctx) => {
  const reply = __lastTalkReply;
  if (!reply) return;
  // 写回距离玩家最近的 drifter
  let bestD2 = Infinity, best = null;
  for (const e of entities) {
    const dx = e.Position.x - __playerX;
    const dy = e.Position.y - __playerY;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = e; }
  }
  if (best) {
    best.Text.content = reply;
    __lastTalkReply = ""; // 只消费一次
  }
});
vitric.system("talk-reply-apply-companion", { query: ["Companion", "Text", "Position"], writes: ["Text"] }, (entities, ctx) => {
  const reply = __lastTalkReply;
  if (!reply) return;
  let bestD2 = Infinity, best = null;
  for (const e of entities) {
    const dx = e.Position.x - __playerX;
    const dy = e.Position.y - __playerY;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = e; }
  }
  if (best) {
    best.Text.content = reply;
    __lastTalkReply = "";
  }
});