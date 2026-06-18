// 伙伴系统:游荡 + 舒适度 + 离开 + 日夜作息 + 多旅人/多伙伴目标追踪
//
// 系统:
//   cache-player-pos     Player+Position → Colony.player_x / player_y
//   drifter-snapshot     Drifter+Position+Persona → Colony.drifter_snapshot (JSON)
//   companion-snapshot   Companion+Position+Persona → Colony.companion_snapshot (JSON)
//   companion-register   Companion → Colony.companion_handles (句柄列表, 给 companion-shelter 用)
//   companion-shelter    Colony → 每句柄的 Need.quarters (struct_count > 0)
//   target-drifter       Colony → Colony.target_drifter_* (找最近旅人)
//   target-companion     Colony → Colony.target_companion_* (找最近伙伴)
//   companion-wander     Companion+Wander+Position+Velocity → 散步
//   companion-need       Companion+Need → 舒适度/离开
//   talk-reply-apply-*   Colony → Text.content (给最近旅人/伙伴), 消费 last_talk_reply
//
// 数据流:cache-player-pos 写 Colony.player_x/y → snapshot 系统把 Drifter/Companion
// 数据打包成 JSON 写 Colony → target-* / companion-shelter / talk-reply-apply-* 只读 Colony。
// 跨系统数据全部活在 Colony 字段里,无任何模块级 let __ 共享变量。

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

// 把一个实体快照数组打包成 JSON 字符串(给 Colony.*_snapshot 用)。
// id/Position/Persona 都已通过 query 校验存在。
function packSnapshot(entities) {
  const data = entities.map(e => ({
    id: e.id,
    x: e.Position.x, y: e.Position.y,
    name: e.Persona.name || "",
    archetype: e.Persona.archetype || "",
    traits: e.Persona.traits || "",
    speech: e.Persona.speech || "",
  }));
  return JSON.stringify(data);
}

// 从 Colony 读快照 JSON,失败(空字段/损坏)返回空数组。
function readSnapshot(raw) {
  if (!raw || typeof raw !== "string") return [];
  try { return JSON.parse(raw) || []; } catch (_) { return []; }
}

// ---- 缓存玩家位置到 Colony(target-* / companion-shelter / talk-reply-apply-* 需要)----
vitric.system("cache-player-pos", { query: ["Player", "Position"], writes: ["Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if (!e.Player) continue;
    ctx.setField("colony", "Colony.player_x", e.Position.x);
    ctx.setField("colony", "Colony.player_y", e.Position.y);
  }
});

// ---- 旅人/伙伴快照:每帧把所有 Drifter / Companion 的 Position+Persona 打包成 JSON ----
vitric.system("drifter-snapshot", { query: ["Drifter", "Position", "Persona"], writes: [] }, (entities, ctx) => {
  ctx.setField("colony", "Colony.drifter_snapshot", packSnapshot(entities));
});

vitric.system("companion-snapshot", { query: ["Companion", "Position", "Persona"], writes: [] }, (entities, ctx) => {
  ctx.setField("colony", "Colony.companion_snapshot", packSnapshot(entities));
});

// ---- 维护 Colony.companion_handles 句柄列表(companion-shelter 直接拿句柄 setField, 不用 parse JSON)----
vitric.system("companion-register", { query: ["Companion"], writes: [] }, (entities, ctx) => {
  ctx.setField("colony", "Colony.companion_handles", entities.map(e => e.id));
});

// ---- 同步所有伙伴的 Need.quarters(基于 Colony.struct_count; 规则已经维护这个字段)----
vitric.system("companion-shelter", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const q = c.Colony.struct_count > 0 ? 1 : 0;
  for (const handle of (c.Colony.companion_handles || [])) {
    if (typeof handle !== "string" || !handle) continue;
    ctx.setField(handle, "Need.quarters", q);
  }
});

// ---- 维护最近旅人:从 Colony.drifter_snapshot 里找离 player 最近的写入 Colony.target_drifter* ----
vitric.system("target-drifter", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const snapshot = readSnapshot(c.Colony.drifter_snapshot);
  const px = c.Colony.player_x || 0, py = c.Colony.player_y || 0;
  let bestD2 = Infinity, best = null;
  for (const d of snapshot) {
    const dx = d.x - px, dy = d.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = d; }
  }
  if (best) {
    ctx.setField("colony", "Colony.target_drifter", best.id);
    ctx.setField("colony", "Colony.target_drifter_x", best.x);
    ctx.setField("colony", "Colony.target_drifter_y", best.y);
    ctx.setField("colony", "Colony.target_drifter_name", best.name || "");
    ctx.setField("colony", "Colony.target_drifter_archetype", best.archetype || "");
    ctx.setField("colony", "Colony.target_drifter_traits", best.traits || "");
    ctx.setField("colony", "Colony.target_drifter_speech", best.speech || "");
  } else {
    ctx.setField("colony", "Colony.target_drifter", "");
  }
});

// ---- 维护最近伙伴 ----
vitric.system("target-companion", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const snapshot = readSnapshot(c.Colony.companion_snapshot);
  const px = c.Colony.player_x || 0, py = c.Colony.player_y || 0;
  let bestD2 = Infinity, best = null;
  for (const d of snapshot) {
    const dx = d.x - px, dy = d.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = d; }
  }
  if (best) {
    ctx.setField("colony", "Colony.target_companion", best.id);
    ctx.setField("colony", "Colony.target_companion_x", best.x);
    ctx.setField("colony", "Colony.target_companion_y", best.y);
    ctx.setField("colony", "Colony.target_companion_name", best.name || "");
    ctx.setField("colony", "Colony.target_companion_archetype", best.archetype || "");
    ctx.setField("colony", "Colony.target_companion_traits", best.traits || "");
    ctx.setField("colony", "Colony.target_companion_speech", best.speech || "");
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

// ---- 邀请旅人:规则收到 companion-invited 后调这个 fn ----
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

// ---- 旅人到来:每个 game day 由 day-start 事件触发,根据 index 派一个人设 ----
// 固定人设池(确定性 → 重放/录像一致)。最多 4 个。每个 spawn 不命名(避免与已有 @drifter 冲突)。
const DRIFTER_POOL = [
  { name: "Mira",  archetype: "山地步兵",   traits: "坚忍,寡言,脚步轻",       speech: "简短、爱用省略号" },
  { name: "Kade",  archetype: "电工学徒",   traits: "好奇、爱拆东西、胆大",   speech: "语速快、夹英文" },
  { name: "Sori",  archetype: "老年医师",   traits: "慈祥、念叨、健忘",       speech: "慢热、爱讲从前" },
  { name: "Vex",   archetype: "游戏少年",   traits: "浮躁、爱吹、爱笑",       speech: "夸张、表情符号感" },
];
vitric.fn("spawnNewDrifter", (args, ctx) => {
  const idx = (args.idx | 0) || 0;
  const persona = DRIFTER_POOL[idx % DRIFTER_POOL.length];
  // 野外区域(x>=16),避免与已有资源点重合
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
  const speech = args.pspeech || "寡言";
  const prompt = "你是一个在荒星漂泊的旅人" + name + "(" + arch + "),性格" + traits + ",说话" + speech + "。一个拓荒者走近了你,你主动说句话打个招呼。请用 JSON 格式回复:{\"say\":\"你说的话\",\"mood\":\"情绪\"}";
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

// ---- LLM 回复通用:把回复存到 Colony.last_talk_reply,
// talk-reply-apply-* 系统每帧把它落到目标实体的 Text.content 上 ----
vitric.fn("onTalkReply", (reply, ctx) => {
  const text = reply.text || "（对方点了点头）";
  let display = text;
  try {
    const parsed = JSON.parse(text);
    if (parsed.say) display = parsed.say;
  } catch (_) {}
  ctx.setField("colony", "Colony.last_talk_reply", display);
});

// ---- 把 last_talk_reply 落到最近的旅人/伙伴 Text.content;两个 apply 系统第一个命中就消费。
// 谁更近谁拿到回复,另一个 apply 看到 last_talk_reply 已被清空就直接 return。
function applyReplyToNearest(c, snapshotField, ctx) {
  const reply = c.Colony.last_talk_reply || "";
  if (!reply) return;
  const snapshot = readSnapshot(c.Colony[snapshotField]);
  const px = c.Colony.player_x || 0, py = c.Colony.player_y || 0;
  let bestD2 = Infinity, best = null;
  for (const d of snapshot) {
    const dx = d.x - px, dy = d.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = d; }
  }
  if (best) {
    ctx.setField(best.id, "Text.content", reply);
    ctx.setField("colony", "Colony.last_talk_reply", ""); // 只消费一次
  }
}

vitric.system("talk-reply-apply-drifter", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  applyReplyToNearest(c, "drifter_snapshot", ctx);
});

vitric.system("talk-reply-apply-companion", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  applyReplyToNearest(c, "companion_snapshot", ctx);
});