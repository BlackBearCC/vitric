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
    preferred: e.Persona.preferred || "",
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
// 查询 Colony + Companion + Need + Persona + Position + Mood + Text,找最近伙伴,
// 把它和它的实时状态(id/位置/Persona/Need/Mood)都快照到 Colony.* 字段。
// 这样规则调 fn 时(talk / gift)能直接拿 @colony.Colony.target_companion_xxx 传给 fn,
// 不用在 fn 里跨实体读字段(ctx 没有 getField)。
// query 只放 [Colony]:仿 target-drifter,从 companion_snapshot 找最近伙伴。
// (绝不能把 Companion 塞进 query —— 没有实体同时具备 Colony 和 Companion,会让本系统匹配 0 个、
//  从不运行 → target_companion 永远为空 → gift/talk 永远找不到目标。这是 iter2 互动层此前的死因。)
// 位置/人设字段直接取自快照;Need/Mood 不在快照里,用 ctx.getField 按句柄读 live 值。
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
    const id = best.id;
    ctx.setField("colony", "Colony.target_companion", id);
    ctx.setField("colony", "Colony.target_companion_x", best.x);
    ctx.setField("colony", "Colony.target_companion_y", best.y);
    ctx.setField("colony", "Colony.target_companion_name", best.name || "");
    ctx.setField("colony", "Colony.target_companion_archetype", best.archetype || "");
    ctx.setField("colony", "Colony.target_companion_traits", best.traits || "");
    ctx.setField("colony", "Colony.target_companion_speech", best.speech || "");
    ctx.setField("colony", "Colony.target_companion_preferred", best.preferred || "");
    ctx.setField("colony", "Colony.target_companion_affinity", ctx.getField(id, "Need.affinity") | 0);
    ctx.setField("colony", "Colony.target_companion_affinity_i", ctx.getField(id, "Need.affinity_i") | 0);
    ctx.setField("colony", "Colony.target_companion_talked_today", ctx.getField(id, "Need.talked_today") | 0);
    ctx.setField("colony", "Colony.target_companion_gifted_today", ctx.getField(id, "Need.gifted_today") | 0);
    ctx.setField("colony", "Colony.target_companion_mood", (ctx.getField(id, "Mood.value") || "").toString());
  } else {
    ctx.setField("colony", "Colony.target_companion", "");
    ctx.setField("colony", "Colony.target_companion_preferred", "");
    ctx.setField("colony", "Colony.target_companion_affinity", 0);
    ctx.setField("colony", "Colony.target_companion_affinity_i", 0);
    ctx.setField("colony", "Colony.target_companion_talked_today", 0);
    ctx.setField("colony", "Colony.target_companion_gifted_today", 0);
    ctx.setField("colony", "Colony.target_companion_mood", "");
  }
});

// ---- 交互可发现性:玩家靠近最近伙伴(互动范围内)时,头顶浮一个"!"标记 + 提示按键。
// 用独立的 companion_marker 实体(不占用伙伴说话气泡的 Text)。走开就移出视野。----
vitric.system("companion-hint", { query: ["Colony"], writes: [] }, (ents, ctx) => {
  const c = ents[0];
  if (!c) return;
  const tid = c.Colony.target_companion || "";
  const px = c.Colony.player_x || 0, py = c.Colony.player_y || 0;
  const tx = c.Colony.target_companion_x || 0, ty = c.Colony.target_companion_y || 0;
  const near = tid !== "" && ((tx - px) * (tx - px) + (ty - py) * (ty - py)) <= 16.0; // 互动范围内(dist<4)
  if (near) {
    ctx.setField("companion_marker", "Position.x", tx);
    ctx.setField("companion_marker", "Position.y", ty + 0.95);
    ctx.setField("companion_marker", "Text.content", "! G送礼 T对话");
  } else {
    ctx.setField("companion_marker", "Position.y", -999.0);
    ctx.setField("companion_marker", "Text.content", "");
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
// 名字由 rules 通过 persona.name 传入。preferred 由 args 或按名字查 DRIFTER_POOL 补全。
// 不同伙伴不同色相(从名字 hash → HSL),让聚落视觉上住着不同人。
function personaPreferred(name) {
  for (let i = 0; i < DRIFTER_POOL.length; i++) {
    if (DRIFTER_POOL[i].name === name) return DRIFTER_POOL[i].preferred || "";
  }
  return "";
}
function personaHash(name) {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = ((h * 31) + name.charCodeAt(i)) | 0;
  return h;
}
function personaColor(name) {
  // 用 hash 派一个 0..360 的色相,固定 s/l 暖色 — 同一名字永远同色。
  const hue = ((personaHash(name) % 360) + 360) % 360;
  // 暖色过滤:把色相压到 [10, 60] 区间(暖橙/黄/红),避免冷蓝/绿。
  const warm = 10 + ((hue % 50) | 0);
  // hsv → rgb(简版,纯数字,确定性)
  const s = 0.55, v = 0.78;
  const c = v * s;
  const x = c * (1 - Math.abs(((warm / 60) % 2) - 1));
  const m = v - c;
  let r = 0, g = 0, b = 0;
  if (warm < 60)      { r = c; g = x; b = 0; }
  else if (warm < 120){ r = 0; g = c; b = x; }
  else if (warm < 180){ r = 0; g = x; b = c; }
  else if (warm < 240){ r = x; g = 0; b = c; }
  else if (warm < 300){ r = c; g = 0; b = x; }
  else                { r = c; g = x; b = 0; }
  const toHex = (v) => {
    const n = Math.max(0, Math.min(255, Math.round((v + m) * 255)));
    return n.toString(16).padStart(2, "0");
  };
  return "#" + toHex(r) + toHex(g) + toHex(b);
}

vitric.fn("consumeDrifter", (args, ctx) => {
  if (args.drifter_id) ctx.despawn(args.drifter_id);
  const name = args.name || "旅人";
  const persona = {
    name: name,
    archetype: args.archetype || "",
    traits: args.traits || "",
    speech: args.speech || "",
    preferred: args.preferred || personaPreferred(name),
  };
  const sx = 5 + Math.round(ctx.random() * 4);
  const sy = 5 + Math.round(ctx.random() * 4);
  ctx.spawn({
    Companion: {},
    Persona: persona,
    Mood: { value: "平静" },
    ThinkReq: { pending: 0 },
    Need: { comfort: 60, quarters: 0, leave_timer: 0, voiced: 0, comfort_i: 60,
            affinity: 25, affinity_i: 25,
            talked_today: 0, gifted_today: 0,
            last_interact_day: 0, contribution_timer: 0 },
    Wander: { home_x: sx, home_y: sy, tx: sx, ty: sy, timer: 2 },
    Position: { x: sx, y: sy },
    Velocity: { x: 0, y: 0 },
    Sprite: { w: 0.9, h: 0.9, color: personaColor(name) },
    Text: { content: "", size: 0.7, color: "#ffe9b0" },
    Census: { count: 0, is_hub: 0 },
  });
  ctx.emit("companion-moved-in", { name: persona.name });
});

// ---- 旅人到来:每个 game day 由 day-start 事件触发,根据 index 派一个人设 ----
// 固定人设池(确定性 → 重放/录像一致)。每个 spawn 不命名(避免与已有 @drifter 冲突)。
// 每人设带"偏好物品"(preferred,逗号分隔)→ iter2 送礼系统用:送对其 +12 好感、送错 +3 好感。
// 6 个差异化人设:不同名字/原型/性格/说话风格/偏好,不是千篇一律的同一模板。
const DRIFTER_POOL = [
  { name: "Mira",  archetype: "山地步兵",   traits: "坚忍,寡言,脚步轻",           speech: "简短、爱用省略号",     preferred: "fiber,wood" },
  { name: "Kade",  archetype: "电工学徒",   traits: "好奇、爱拆东西、胆大",       speech: "语速快、夹英文",       preferred: "ore,plank" },
  { name: "Sori",  archetype: "老年医师",   traits: "慈祥、念叨、健忘",           speech: "慢热、爱讲从前",       preferred: "lamp,chair" },
  { name: "Vex",   archetype: "游戏少年",   traits: "浮躁、爱吹、爱笑",           speech: "夸张、表情符号感",     preferred: "wheat,seed" },
  { name: "Nell",  archetype: "沉默匠人",   traits: "内向、手巧、爱干净",         speech: "句子短、偶尔冒冷笑话", preferred: "wood,ore" },
  { name: "Orin",  archetype: "退役厨师",   traits: "话多、爱美食、嗓门大",       speech: "嗓门大、爱用感叹号",   preferred: "wheat,lamp" },
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
// 距离检查 + 用 LLM 生成对白 + 把目标 id 暂存到 Colony.last_talk_target,
// onTalkReply 收到回复时给它 +affinity 并把亲和力增长记到该天/该伙伴。
vitric.fn("talkNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return;
  const name = args.pname || "旅人";
  const arch = args.parch || "漂泊者";
  const traits = args.ptraits || "沉默";
  const speech = args.pspeech || "寡言";
  const pid = args.pid || ""; // 目标实体的句柄(伙伴或旅人)
  const prompt = "你是一个在荒星漂泊的旅人" + name + "(" + arch + "),性格" + traits + ",说话" + speech + "。一个拓荒者走近了你,你主动说句话打个招呼。请用 JSON 格式回复:{\"say\":\"你说的话\",\"mood\":\"情绪\"}";
  ctx.setField("colony", "Colony.last_talk_target", pid);
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

// ---- 物品全集(与 schema Inventory 对齐)——礼物选择 + 写回用 ----
const ITEM_KINDS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];

// ---- 送礼(按 g):靠近伙伴时,选背包里第一件"该伙伴偏好的"物品 → 送礼 +affinity ----
// 选礼策略:优先选偏好物品 → 没有偏好物品则选背包里数量 > 0 的第一件。
// 偏好命中 +12 好感,普通 +3 好感(每天都送上限 2 次,gifted_today 守卫)。
// 写回:扣背包 → emit inv-set(由 economy.js 同一套绝对值回写法落到 @player.Inventory.*)。
const GIFT_PREFERRED_GAIN = 12;
const GIFT_GENERIC_GAIN   = 3;
const GIFT_DAILY_CAP      = 2;
function readInvFromArgs(a) {
  const inv = {};
  for (const k of ITEM_KINDS) inv[k] = (a[k] | 0);
  return inv;
}
function pickGiftItem(inv, preferredCsv) {
  // preferred 命中优先
  const prefs = (preferredCsv || "").split(",").map(s => s.trim()).filter(s => s);
  for (const k of prefs) {
    if ((inv[k] | 0) > 0) return k;
  }
  // 没有偏好,选第一个 >0 的
  for (const k of ITEM_KINDS) {
    if ((inv[k] | 0) > 0) return k;
  }
  return "";
}
vitric.fn("giveGiftNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return;
  const pid = args.pid || "";
  if (!pid) return;
  // 每日上限
  const got = ctx.getField(pid, "Need.gifted_today");
  const cur = (typeof got === "number" && !isNaN(got)) ? got : 0;
  if (cur >= GIFT_DAILY_CAP) {
    ctx.emit("gift-cap", { pid: pid });
    return;
  }
  // 选物品
  const inv = readInvFromArgs(args);
  const preferred = args.ppreferred || "";
  const item = pickGiftItem(inv, preferred);
  if (!item) {
    ctx.emit("gift-empty", { pid: pid });
    return;
  }
  const preferredHit = preferred.split(",").map(s => s.trim()).filter(s => s).indexOf(item) >= 0;
  const gain = preferredHit ? GIFT_PREFERRED_GAIN : GIFT_GENERIC_GAIN;
  // 扣 1 件 + 回写
  inv[item] -= 1;
  const d = {};
  for (const k of ITEM_KINDS) d[k] = inv[k];
  ctx.emit("inv-set", d);
  // +affinity + 计数 + 互动标记
  const aff = ctx.getField(pid, "Need.affinity");
  const a = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
  const na = a + gain;
  ctx.setField(pid, "Need.affinity", na > 100 ? 100 : na);
  ctx.setField(pid, "Need.gifted_today", cur + 1);
  ctx.setField(pid, "Need.last_interact_day",
    ctx.getField("colony", "Colony.day") | 0);
  ctx.emit("gift-given", { pid: pid, item: item, preferred: preferredHit ? 1 : 0, gain: gain });
});

// ---- LLM 回复通用:把回复存到 Colony.last_talk_reply,
// talk-reply-apply-* 系统每帧把它落到目标实体的 Text.content 上 ----
// 同时给 last_talk_target 那位伙伴 / 旅人 +affinity(旅人搬到聚落后此值会被带过来)。
// 每天每伙伴 talk 上限 3 次 → talked_today 守卫(超了不再 +affinity,但仍展示回复)。
// fn 拿不到 ctx.getField,所以:talkNearby 把 target 句柄存到 Colony.last_talk_target,
// 此外规则再把 target 当前的 talked_today/affinity 写到 Colony.last_talk_* 副本,
// 本 fn 从 Colony 读这些副本,加完后再回写(setField 跨实体)。
const TALK_AFFINITY_GAIN = 3;     // 一次对话的 +affinity
const TALK_DAILY_CAP = 3;          // 每天每伙伴最多 +affinity 的对话次数
vitric.fn("onTalkReply", (reply, ctx) => {
  const text = reply.text || "（对方点了点头）";
  let display = text;
  try {
    const parsed = JSON.parse(text);
    if (parsed.say) display = parsed.say;
  } catch (_) {}
  ctx.setField("colony", "Colony.last_talk_reply", display);
  // 找目标伙伴/旅人 +affinity
  const target = (ctx.getField("colony", "Colony.last_talk_target") || "").toString();
  if (target) {
    const cur = ctx.getField(target, "Need.talked_today") | 0;
    if (cur < TALK_DAILY_CAP) {
      const aff = ctx.getField(target, "Need.affinity") | 0;
      const na = aff + TALK_AFFINITY_GAIN;
      ctx.setField(target, "Need.affinity", na > 100 ? 100 : na);
      ctx.setField(target, "Need.talked_today", cur + 1);
      ctx.setField(target, "Need.affinity_i", na > 100 ? 100 : na);
      ctx.setField(target, "Need.last_interact_day",
        ctx.getField("colony", "Colony.day") | 0);
    }
  }
  ctx.setField("colony", "Colony.last_talk_target", "");
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

// =====================================================================
// iter2 — 伙伴关系系统(affinity / interactions / contribution / mood)
// =====================================================================
//
// 设计:把"interact 次数 + comfort"汇总到 affinity(0..100)。
//   1. talk(gain +3, 每天最多 +9 = 3 次)        — onTalkReply 里加
//   2. gift(命中偏好 +12 / 普通 +3, 每天最多 +24 = 2 次) — giveGiftNearby 里加
//   3. care:comfort 高 + 有住所 → +0.6/min,持续涨;
//          comfort 极低 → -0.3/min,缓慢掉;
//          长期(>3 天)无任何互动 → -0.5/day 衰减。
//
// 然后 affinity + mood 驱动 contribution:每 12 秒一次,自动
//   - 给最近 Plot 种熟作物 +1 stage(帮忙种田)
//   - 给 @player 背包 +1 ore/wood/fiber(帮忙采集)
//   - 给 Colony 食物 rate +0.5 boost(出菜多一截)
//   三选一,按 ctx.random 派。
//
// 心情:由 comfort + last_interact_day 距离 + 时段算出"开心/平静/低落/疲倦",
//   写入 Mood.value,companion-mood 把这个值塞进 Persona's Text 上方浮一行小字。
//   (复用现有 Text 字段;reply 文本会被同一 tick 覆盖,所以我们改用 Text 显示亲和力/心情)。

// ---- 心情:comfort + 互动新旧 + 时段 → "开心/平静/疲倦/低落/恼火" ----
// 优先级:恼火(<20 comfort 且 5 天没互动) > 低落(<35 comfort) > 疲倦(夜里无住所)
//       > 开心(comfort>70 + 最近互动过) > 平静。
// 只写 Mood.value,不碰 Text.content(reply 用 Text,不能打架)。
vitric.system("companion-mood", { query: ["Companion", "Need", "Persona", "Mood"], writes: ["Mood"] }, (entities, ctx) => {
  const tod = compTodOf(ctx.tick);
  const day = (ctx.getField("colony", "Colony.day") | 0) || 1;
  for (const e of entities) {
    const n = e.Need;
    const m = e.Mood;
    let mood;
    const sinceInteract = day - (n.last_interact_day | 0);
    if (n.comfort < 20 && sinceInteract >= 5)        mood = "恼火";
    else if (n.comfort < 35)                          mood = "低落";
    else if (tod === "夜" && (n.quarters | 0) === 0)  mood = "疲倦";
    else if (n.comfort > 70 && sinceInteract <= 2)    mood = "开心";
    else                                               mood = "平静";
    if (m.value !== mood) m.value = mood;
  }
});

// ---- 照顾(comfort-driven) + 长期疏远衰减:每帧 ----
// 有 quarters + comfort > 70 → +0.6/min(0.01/s)
// comfort < 30 且 quarters==0 → -0.3/min(-0.005/s)
// 自上次互动 >= 3 天 → -0.5/day ≈ -0.0083/s
const CARE_GAIN_PER_SEC = 0.01;   // +0.6/min
const CARE_LOSS_PER_SEC = 0.005;  // -0.3/min(无住所低 comfort)
const NEGLECT_PER_SEC   = 0.0083; // -0.5/day
vitric.system("companion-affinity-care", { query: ["Companion", "Need"], writes: ["Need"] }, (entities, ctx) => {
  const day = (ctx.getField("colony", "Colony.day") | 0) || 1;
  for (const e of entities) {
    const n = e.Need;
    let delta = 0;
    if ((n.quarters | 0) > 0 && n.comfort > 70) delta += CARE_GAIN_PER_SEC * ctx.dt;
    if ((n.quarters | 0) === 0 && n.comfort < 30) delta -= CARE_LOSS_PER_SEC * ctx.dt;
    const since = day - (n.last_interact_day | 0);
    if (since >= 3) delta -= NEGLECT_PER_SEC * ctx.dt;
    if (delta !== 0) {
      n.affinity = n.affinity + delta;
      if (n.affinity < 0) n.affinity = 0;
      if (n.affinity > 100) n.affinity = 100;
      n.affinity_i = Math.round(n.affinity);
    }
  }
});

// ---- 每天重置 talked_today / gifted_today + 衰减照顾 ----
// 收到 day-start 事件 → Colony.day 已 +1(由 clock 系统先跑)。
// 做法:把 day 起点存到 Colony.day_anchor,系统每天对比一次;差异则重置每伙伴计数。
// 这里用更简单的方式:clock 系统每 tick emit day-start;本系统 query [Companion,Need],
// 每帧检测 "本次进系统时还在新一天(对比上次 day_anchor)" → 重置计数。
// query 只放 [Companion,Need](注释本就写的是这个,原代码手滑塞了 Colony 致本系统从不运行)。
// day / _day_anchor 在 colony 上,用 getField 读、setField 写跨实体。
vitric.system("companion-day-reset", { query: ["Companion", "Need"], writes: ["Need"] }, (entities, ctx) => {
  const day = ctx.getField("colony", "Colony.day") | 0;
  const last = ctx.getField("colony", "Colony._day_anchor") | 0;
  if (day === last) return;
  ctx.setField("colony", "Colony._day_anchor", day);
  for (const e of entities) {
    e.Need.talked_today = 0;
    e.Need.gifted_today = 0;
  }
});

// ---- 贡献:affinity>=50 + 心情 开心/平静 → 每 12 秒自动帮聚落干活 ----
// 候选动作:
//   (B) 给 @player 背包 +1 ore/wood/fiber(随机一种)
//   (C) 给 Colony food_rate 加 0.5(限一次 tick 内有效,速率系统会自然衰减走)
// 动作 (A) 帮最近 crop 加速 拆到独立系统 companion-tend-crops(query Crop,不走 ctx.world)。
const CONTRIB_INTERVAL_SEC = 12.0;
const CONTRIB_AFFINITY_MIN = 50;
// query 不放 Colony:读/写 colony 走 ctx.getField/ctx.setField 跨实体;塞 Colony 进 query 会让本系统
// 匹配 0 个实体(没有实体同时具备 Companion 和 Colony)→ 整个贡献闭环从不运行(iter2 此前的死因)。
vitric.system("companion-contribution", { query: ["Companion", "Need", "Mood", "Position"], writes: ["Need"] }, (entities, ctx) => {
  for (const e of entities) {
    const n = e.Need;
    if ((n.affinity || 0) < CONTRIB_AFFINITY_MIN) continue;
    const mood = (e.Mood && e.Mood.value) || "平静";
    if (mood !== "开心" && mood !== "平静") continue;
    n.contribution_timer = (n.contribution_timer || 0) - ctx.dt;
    if (n.contribution_timer > 0) continue;
    n.contribution_timer = CONTRIB_INTERVAL_SEC + ctx.random() * 4; // 12~16 秒
    // 二选一(不再做 crop-tend,那个由独立系统按全 colony 状态统一处理)
    const pick = (ctx.random() * 2) | 0;
    if (pick === 0) {
      // (B) 给 @player 背包 +1 资源
      const items = ["ore", "wood", "fiber"];
      const which = items[(ctx.random() * items.length) | 0];
      const cur = ctx.getField("@player", "Inventory." + which) | 0;
      ctx.setField("@player", "Inventory." + which, cur + 1);
      ctx.emit("companion-contributed", { pid: e.id, kind: which });
    } else {
      // (C) Colony food_rate 加 0.5(持续 1 tick,后面 colony 系统会自己衰减走)
      const fr = ctx.getField("colony", "Colony.food_rate");
      ctx.setField("colony", "Colony.food_rate", (typeof fr === "number" ? fr : 0) + 0.5);
      ctx.emit("companion-boost", { pid: e.id, what: "food" });
    }
  }
});

// ---- 帮 crop 加速:聚落里至少一名 affinity>=50 的伙伴 → 每 ~10 秒给一个未熟的 wheat +1 stage ----
// 独立成系统是因为它需要 query Crop(伙伴系统 query 是 Companion);靠 colony.companion_happy_count
// (companion-tally 系统已写入)作为触发条件,避免跨系统互相 query。
const TEND_INTERVAL_SEC = 10.0;
vitric.system("companion-tend-crops", { query: ["Crop", "Position"], writes: ["Crop"] }, (entities, ctx) => {
  const happy = ctx.getField("colony", "Colony.companion_happy_count") | 0;
  if (happy < 1) return;
  // 简单做法:把所有未熟 wheat 的 entity 收集,每 tick 按概率触发一次(确定性由 ctx.random 控)
  const candidates = [];
  for (const e of entities) {
    if (e.Crop.kind !== "wheat") continue;
    if ((e.Crop.stage | 0) >= 3) continue;
    candidates.push(e);
  }
  if (candidates.length === 0) return;
  // 用第一只候选的 id 作为稳定 tick 计数锚(每个 crop 自己计时,均匀分布)
  for (const e of candidates) {
    e.Crop._tend_t = ((e.Crop._tend_t || 0) - ctx.dt);
    if (e.Crop._tend_t > 0) continue;
    e.Crop._tend_t = TEND_INTERVAL_SEC + ctx.random() * 3; // 10~13 秒
    const st = (e.Crop.stage | 0) + 1;
    e.Crop.stage = st > 3 ? 3 : st;
  }
});

// ---- 聚落级汇总:happy 数 + 平均 affinity — 给 win tie-in / 成群门 / 贡献触发用 ----
// happy = affinity>=50 的伙伴数(与 companion-contribution / companion-tend-crops 的"会帮忙"线一致)。
// 注意:query 只放 [Companion,Need] —— 写 Colony 走 ctx.setField 跨实体,不能把 Colony 塞进 query
// (没有实体同时具备 Companion 和 Colony,塞进去会让本系统匹配 0 个实体、从不运行)。
vitric.system("companion-tally", { query: ["Companion", "Need"], writes: [] }, (entities, ctx) => {
  let happy = 0, total = 0, sumAff = 0;
  for (const e of entities) {
    total++;
    const aff = e.Need.affinity || 0;
    sumAff += aff;
    if (aff >= 50) happy++;
  }
  ctx.setField("colony", "Colony.companion_happy_count", happy);
  ctx.setField("colony", "Colony.companion_affinity_avg", total > 0 ? sumAff / total : 0);
});

// ---- HUD:近的伙伴时,在 HUD 下方显示一行小卡(name + 好感 + 心情) ----
// 用现成的 Colont.text(屏幕空间):存 "近:Lio 好感 54 · 开心" 之类的串。
// 把这个串作为 @hud_companion_lbl.UiLabel.content 写入。
// 找最近的伙伴 → 距离 <= 5 才显示,否则空串(标签隐藏)。
vitric.system("companion-hud", { query: ["Colony"], writes: [] }, (entities, ctx) => {
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
  let label = "";
  if (best && bestD2 <= 25) {
    // 拉该伙伴的 affinity + mood
    const aff = ctx.getField(best.id, "Need.affinity_i") | 0;
    const mood = (ctx.getField(best.id, "Mood.value") || "").toString();
    const gifted = ctx.getField(best.id, "Need.gifted_today") | 0;
    const talked = ctx.getField(best.id, "Need.talked_today") | 0;
    label = "♥ " + (best.name || "伙伴") + "  好感 " + aff + "  ·" + mood
          + "    今日 谈 " + talked + "/3  礼 " + gifted + "/2";
  }
  ctx.setField("@hud_companion_lbl", "UiLabel.content", label);
});