// Companion system: wandering + comfort + departure + day/night routine + multi-drifter/multi-companion target tracking
//
// Systems:
//   cache-player-pos     Player+Position → Colony.player_x / player_y
//   drifter-snapshot     Drifter+Position+Persona → Colony.drifter_snapshot (JSON)
//   companion-snapshot   Companion+Position+Persona → Colony.companion_snapshot (JSON)
//   companion-register   Companion → Colony.companion_handles (handle list, for companion-shelter)
//   companion-shelter    Colony → each handle's Need.quarters (struct_count > 0)
//   target-drifter       Colony → Colony.target_drifter_* (find nearest drifter)
//   target-companion     Colony → Colony.target_companion_* (find nearest companion)
//   companion-wander     Companion+Wander+Position+Velocity → wander
//   companion-need       Companion+Need → comfort / departure
//   talk-reply-apply-*   Colony → Text.content (to nearest drifter/companion); consumes last_talk_reply
//
// Data flow: cache-player-pos writes Colony.player_x/y → snapshot systems pack
// Drifter/Companion data into JSON and write to Colony → target-* / companion-shelter / talk-reply-apply-* only read Colony.
// All cross-system data lives in Colony fields; no module-level `let __` shared variables.

const WANDER_SPEED = 1.2;   // wander speed
const WANDER_RADIUS = 2.5;  // around home_x/y ± radius
const COMP_DAY_SEC = 60.0;
const COMP_TICK_PER_SEC = 60;

// Wish templates per role. Each companion gets 3 wishes based on their Persona.role.
// items is stored as JSON text in Wish.items (schema doesn't support nested list-of-struct).
// Duplicated in wish.js (QuickJS has no ES modules — each file is its own scope; keep both copies in sync).
const WISH_TEMPLATES = {
  builder: [
    { desc: "建造 3 个结构", kind: "build", target: 3, progress: 0, done: false },
    { desc: "建一盏灯",     kind: "build-lamp", target: 1, progress: 0, done: false },
    { desc: "升级 1 个结构", kind: "upgrade", target: 1, progress: 0, done: false },
  ],
  farmer: [
    { desc: "种出 2 茬作物",     kind: "harvest", target: 2, progress: 0, done: false },
    { desc: "收获 8 单位麦子",   kind: "harvest-wheat", target: 8, progress: 0, done: false },
    { desc: "吃饱一次(食≥80)",   kind: "food-high", target: 80, progress: 0, done: false },
  ],
  explorer: [
    { desc: "探索 3 处野外地点", kind: "enter-poi", target: 3, progress: 0, done: false },
    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
    { desc: "看一次日出(凌晨出门)", kind: "see-dawn", target: 1, progress: 0, done: false },
  ],
  guard: [
    { desc: "建造 3 个结构",     kind: "build", target: 3, progress: 0, done: false },
    { desc: "升级 1 个结构",     kind: "upgrade", target: 1, progress: 0, done: false },
    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
  ],
  trader: [
    { desc: "探索 3 处野外地点", kind: "enter-poi", target: 3, progress: 0, done: false },
    { desc: "建造 3 个结构",     kind: "build", target: 3, progress: 0, done: false },
    { desc: "收获 8 单位麦子",   kind: "harvest-wheat", target: 8, progress: 0, done: false },
  ],
  scholar: [
    { desc: "探索 5 处野外地点", kind: "enter-poi", target: 5, progress: 0, done: false },
    { desc: "采集 10 单位矿石",  kind: "gather-ore", target: 10, progress: 0, done: false },
    { desc: "建造 3 个结构",     kind: "build", target: 3, progress: 0, done: false },
  ],
};
// Direct role-keyed lookup (post-Task-9 every companion has Persona.role).
function wishesForRole(role) {
  return WISH_TEMPLATES[role] || WISH_TEMPLATES.builder;
}
// Backwards-compat: derive role from archetype keywords (for any pre-Task-9 companion lacking Persona.role).
function wishesForArchetype(archetype) {
  const a = archetype || "";
  let role = "explorer"; // default
  if (/技|电|匠|build|builder/i.test(a)) role = "builder";
  else if (/厨|医|农|farm|farmer/i.test(a)) role = "farmer";
  else if (/兵|卫|guard/i.test(a)) role = "guard";
  else if (/商|trade|trader/i.test(a)) role = "trader";
  else if (/学|究|scholar/i.test(a)) role = "scholar";
  return wishesForRole(role);
}

function compTodOf(tick) {
  const secOfDay = (tick / COMP_TICK_PER_SEC) % COMP_DAY_SEC;
  const frac = secOfDay / COMP_DAY_SEC;
  if (frac < 0.25) return "晨";
  if (frac < 0.50) return "午";
  if (frac < 0.75) return "昏";
  return "夜";
}

// Pack an entity snapshot array into a JSON string (for Colony.*_snapshot).
// id/Position/Persona have all been verified present via the query.
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

// Read snapshot JSON from Colony; on failure (empty field / corrupt) return an empty array.
function readSnapshot(raw) {
  if (!raw || typeof raw !== "string") return [];
  try { return JSON.parse(raw) || []; } catch (_) { return []; }
}

// ---- Cache player position into Colony (needed by target-* / companion-shelter / talk-reply-apply-*) ----
vitric.system("cache-player-pos", { query: ["Player", "Position"], writes: ["Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if (!e.Player) continue;
    ctx.setField("colony", "Colony.player_x", e.Position.x);
    ctx.setField("colony", "Colony.player_y", e.Position.y);
  }
});

// ---- Drifter/companion snapshot: every frame pack all Drifter / Companion Position+Persona into JSON ----
vitric.system("drifter-snapshot", { query: ["Drifter", "Position", "Persona"], writes: [] }, (entities, ctx) => {
  ctx.setField("colony", "Colony.drifter_snapshot", packSnapshot(entities));
});

vitric.system("companion-snapshot", { query: ["Companion", "Position", "Persona"], writes: [] }, (entities, ctx) => {
  ctx.setField("colony", "Colony.companion_snapshot", packSnapshot(entities));
});

// ---- Maintain Colony.companion_handles handle list (companion-shelter uses handles directly via setField, no JSON parse) ----
vitric.system("companion-register", { query: ["Companion"], writes: [] }, (entities, ctx) => {
  ctx.setField("colony", "Colony.companion_handles", entities.map(e => e.id));
});

// ---- Sync all companions' Need.quarters (based on Colony.struct_count; the rule already maintains this field) ----
vitric.system("companion-shelter", { query: ["Colony"], writes: [] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const q = c.Colony.struct_count > 0 ? 1 : 0;
  for (const handle of (c.Colony.companion_handles || [])) {
    if (typeof handle !== "string" || !handle) continue;
    ctx.setField(handle, "Need.quarters", q);
  }
});

// ---- Maintain nearest drifter: find the one closest to the player from Colony.drifter_snapshot, write to Colony.target_drifter* ----
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

// ---- Maintain nearest companion ----
// Query Colony + Companion + Need + Persona + Position + Mood + Text, find the nearest companion,
// and snapshot it and its live state (id/position/Persona/Need/Mood) into Colony.* fields.
// That way, when a rule calls a fn (talk / gift), it can pass @colony.Colony.target_companion_xxx straight to the fn
// without the fn having to read fields across entities (ctx has no getField).
// query only contains [Colony]: like target-drifter, find the nearest companion from companion_snapshot.
// (Companion must NOT be added to the query — no entity has both Colony and Companion, so the system would match 0 entities and
//  never run → target_companion stays empty forever → gift/talk can never find a target. This was the previous cause of death for iter2 interaction layer.)
// Position/persona fields come straight from the snapshot; Need/Mood are not in the snapshot, so use ctx.getField by handle to read live values.
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

// ---- Interaction discoverability: when the player is near the nearest companion (within
// interaction range), float a "!" marker + key hint above their head. Uses a separate
// companion_marker entity (does not occupy the companion's speech bubble Text). Moves out of view when walking away. ----
vitric.system("companion-hint", { query: ["Colony"], writes: [] }, (ents, ctx) => {
  const c = ents[0];
  if (!c) return;
  const tid = c.Colony.target_companion || "";
  const px = c.Colony.player_x || 0, py = c.Colony.player_y || 0;
  const tx = c.Colony.target_companion_x || 0, ty = c.Colony.target_companion_y || 0;
  const near = tid !== "" && ((tx - px) * (tx - px) + (ty - py) * (ty - py)) <= 16.0; // within interaction range (dist<4)
  if (near) {
    ctx.setField("companion_marker", "Position.x", tx);
    ctx.setField("companion_marker", "Position.y", ty + 0.95);
    ctx.setField("companion_marker", "Text.content", "! G送礼 T对话");
  } else {
    ctx.setField("companion_marker", "Position.y", -999.0);
    ctx.setField("companion_marker", "Text.content", "");
  }
});

// ---- Interaction discoverability: hint above drifter (can be invited to join). At 6 tiles,
// start prompting to approach; at 4 tiles (invitable range), show "press I to invite".
// Distinguished from companion marker: drifter is cyan, press I. ----
vitric.system("drifter-hint", { query: ["Colony"], writes: [] }, (ents, ctx) => {
  const c = ents[0];
  if (!c) return;
  const tid = c.Colony.target_drifter || "";
  const px = c.Colony.player_x || 0, py = c.Colony.player_y || 0;
  const tx = c.Colony.target_drifter_x || 0, ty = c.Colony.target_drifter_y || 0;
  const d2 = (tx - px) * (tx - px) + (ty - py) * (ty - py);
  if (tid !== "" && d2 <= 36.0) {
    ctx.setField("drifter_marker", "Position.x", tx);
    ctx.setField("drifter_marker", "Position.y", ty + 0.95);
    ctx.setField("drifter_marker", "Text.content", d2 <= 16.0 ? "! 按 I 邀请" : "旅人 · 走近按 I");
  } else {
    ctx.setField("drifter_marker", "Position.y", -999.0);
    ctx.setField("drifter_marker", "Text.content", "");
  }
});

// ---- Wander: switch target points on a timer; stop when reached, wait for timer to move again ----
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

// ---- Comfort: day/night rhythm + housing ----
// Day: no shelter → slow decay; with shelter → slow recovery.
// Night: no shelter → fast decay; with shelter → accelerated recovery.
// comfort hitting 0 → leave_timer accrues; at 15 seconds → despawn self
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

// ---- Invite drifter: the rule calls this fn after receiving companion-invited ----
// Despawn the passed drifter_id, then spawn a new entity with Companion near the player.
// The name is passed in via persona.name by the rules. preferred is filled from args or looked up in DRIFTER_POOL by name.
// Different companions get different hues (from name hash → HSL), so visually the colony is populated by distinct people.
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
  // Derive a 0..360 hue from the hash, fixed s/l warm color — same name always yields the same color.
  const hue = ((personaHash(name) % 360) + 360) % 360;
  // Warm filter: compress the hue into [10, 60] (warm orange/yellow/red), avoiding cold blue/green.
  const warm = 10 + ((hue % 50) | 0);
  // hsv → rgb (simplified, pure numbers, deterministic)
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
  const role = args.role || "builder";
  const persona = {
    name: name,
    archetype: args.archetype || "",
    traits: args.traits || "",
    speech: args.speech || "",
    preferred: args.preferred || personaPreferred(name),
    role: role,
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
    Wish: { items: JSON.stringify(args.role ? wishesForRole(args.role) : wishesForArchetype(args.archetype)), fulfilled: 0 },
  });
  ctx.emit("companion-moved-in", { name: persona.name });
});

// ---- Drifter arrival: triggered by the day-start event every game day; pick a persona by index ----
// Fixed persona pool (deterministic → replay/recording consistent). Each spawn is unnamed (avoids collision with existing @drifter).
// Each persona carries "preferred items" (preferred, comma-separated) → used by the iter2 gift system: matching gift +12 affinity, wrong +3 affinity.
// 12 differentiated personas (Task 9: 2 per role × 6 roles). Each entry carries a `role` field consumed by consumeDrifter.
const DRIFTER_POOL = [
  { name: "Mira",  archetype: "山地步兵",   traits: "坚忍,寡言,脚步轻",           speech: "简短、爱用省略号",     preferred: "fiber,wood",    role: "guard" },
  { name: "Kade",  archetype: "电工学徒",   traits: "好奇、爱拆东西、胆大",       speech: "语速快、夹英文",       preferred: "ore,plank",     role: "builder" },
  { name: "Sori",  archetype: "老年医师",   traits: "慈祥、念叨、健忘",           speech: "慢热、爱讲从前",       preferred: "lamp,chair",    role: "scholar" },
  { name: "Vex",   archetype: "游戏少年",   traits: "浮躁、爱吹、爱笑",           speech: "夸张、表情符号感",     preferred: "wheat,seed",    role: "explorer" },
  { name: "Nell",  archetype: "沉默匠人",   traits: "内向、手巧、爱干净",         speech: "句子短、偶尔冒冷笑话", preferred: "wood,ore",      role: "builder" },
  { name: "Orin",  archetype: "退役厨师",   traits: "话多、爱美食、嗓门大",       speech: "嗓门大、爱用感叹号",   preferred: "wheat,lamp",    role: "farmer" },
  { name: "Holt",  archetype: "退役士兵",   traits: "严肃、警觉、责任感强",       speech: "短促、爱用军语",       preferred: "ore,plank",     role: "guard" },
  { name: "Pim",   archetype: "博物学者",   traits: "好奇、博学、爱记录",         speech: "学究气、爱引用",       preferred: "lamp,chair",    role: "scholar" },
  { name: "Dax",   archetype: "徒步旅人",   traits: "机敏、爱冒险、记路",         speech: "简洁、爱用方向词",     preferred: "fiber,wood",    role: "explorer" },
  { name: "Yara",  archetype: "园丁",       traits: "耐心、爱植物、观察入微",     speech: "温柔、爱用比喻",       preferred: "seed,wheat",    role: "farmer" },
  { name: "Rix",   archetype: "商队学徒",   traits: "精明、爱砍价、记帐快",       speech: "快嘴、爱用数字",       preferred: "plank,lamp",    role: "trader" },
  { name: "Lira",  archetype: "游商",       traits: "圆滑、爱讲故事、见多识广",   speech: "热络、爱用感叹",       preferred: "wheat,chair",   role: "trader" },
];
vitric.fn("spawnNewDrifter", (args, ctx) => {
  const idx = (args.idx | 0) || 0;
  const persona = DRIFTER_POOL[idx % DRIFTER_POOL.length];
  // Wild area (x>=16), avoid overlapping existing resource nodes
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

// ---- Talk (press t): if near a target, fire an LLM conversation ----
// Distance check + use LLM to generate dialogue + stash the target id in Colony.last_talk_target;
// onTalkReply gives it +affinity when the reply comes back and records the affinity gain for that day/companion.
vitric.fn("talkNearby", (args, ctx) => {
  const px = args.px | 0, py = args.py | 0;
  const dx = args.dx | 0, dy = args.dy | 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) return;
  const name = args.pname || "旅人";
  const arch = args.parch || "漂泊者";
  const traits = args.ptraits || "沉默";
  const speech = args.pspeech || "寡言";
  const pid = args.pid || ""; // handle of the target entity (companion or drifter)
  const prompt = "你是一个在荒星漂泊的旅人" + name + "(" + arch + "),性格" + traits + ",说话" + speech + "。一个拓荒者走近了你,你主动说句话打个招呼。请用 JSON 格式回复:{\"say\":\"你说的话\",\"mood\":\"情绪\"}";
  ctx.setField("colony", "Colony.last_talk_target", pid);
  ctx.ask("llm", prompt, "onTalkReply");
});

// ---- Invite (press i): if near a target, fire the invite event ----
vitric.fn("inviteAnyNearby", (args, ctx) => {
  const px = +args.px || 0, py = +args.py || 0;
  const dx = +args.dx || 0, dy = +args.dy || 0;
  const dist2 = (px - dx) * (px - dx) + (py - dy) * (py - dy);
  if (dist2 > 16) { ctx.emit("invite-fail", {}); return; } // 太远:给个"走近点"提示
  if (!args.drifter_id) return;
  // Read the drifter's Persona.role and forward it so consumeDrifter can stamp it on the new companion.
  const role = (ctx.getField(args.drifter_id, "Persona.role") || "builder").toString();
  ctx.emit("companion-invited", {
    drifter_id: args.drifter_id,
    name: args.pname || "旅人",
    archetype: args.parch || "",
    traits: args.ptraits || "",
    speech: args.pspeech || "",
    role: role,
  });
});

// ---- Full item set (aligned with the Inventory schema) — for gift selection and write-back ----
const ITEM_KINDS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];

// ---- Gift (press g): when near a companion, pick the first "preferred by this companion" item in the inventory → gift +affinity ----
// Gift-pick strategy: prefer preferred items → if no preferred item, pick the first item with quantity > 0.
// Preferred hit +12 affinity, generic +3 affinity (cap of 2 gifts per day; guarded by gifted_today).
// Write-back: deduct inventory → emit inv-set (uses the same absolute-value write-back path as economy.js to land on @player.Inventory.*).
const GIFT_PREFERRED_GAIN = 12;
const GIFT_GENERIC_GAIN   = 3;
const GIFT_DAILY_CAP      = 2;
function readInvFromArgs(a) {
  const inv = {};
  for (const k of ITEM_KINDS) inv[k] = (a[k] | 0);
  return inv;
}
function pickGiftItem(inv, preferredCsv) {
  // preferred hits first
  const prefs = (preferredCsv || "").split(",").map(s => s.trim()).filter(s => s);
  for (const k of prefs) {
    if ((inv[k] | 0) > 0) return k;
  }
  // no preferred; pick the first >0
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
  // daily cap
  const got = ctx.getField(pid, "Need.gifted_today");
  const cur = (typeof got === "number" && !isNaN(got)) ? got : 0;
  if (cur >= GIFT_DAILY_CAP) {
    ctx.emit("gift-cap", { pid: pid });
    return;
  }
  // pick item
  const inv = readInvFromArgs(args);
  const preferred = args.ppreferred || "";
  const item = pickGiftItem(inv, preferred);
  if (!item) {
    ctx.emit("gift-empty", { pid: pid });
    return;
  }
  const preferredHit = preferred.split(",").map(s => s.trim()).filter(s => s).indexOf(item) >= 0;
  const gain = preferredHit ? GIFT_PREFERRED_GAIN : GIFT_GENERIC_GAIN;
  // deduct 1 item + write back
  inv[item] -= 1;
  const d = {};
  for (const k of ITEM_KINDS) d[k] = inv[k];
  ctx.emit("inv-set", d);
  // +affinity + count + interaction marker
  const aff = ctx.getField(pid, "Need.affinity");
  const a = (typeof aff === "number" && !isNaN(aff)) ? aff : 30;
  const na = a + gain;
  ctx.setField(pid, "Need.affinity", na > 100 ? 100 : na);
  ctx.setField(pid, "Need.gifted_today", cur + 1);
  ctx.setField(pid, "Need.last_interact_day",
    ctx.getField("colony", "Colony.day") | 0);
  ctx.emit("gift-given", { pid: pid, item: item, preferred: preferredHit ? 1 : 0, gain: gain });
});

// ---- Generic LLM reply: store the reply in Colony.last_talk_reply;
// the talk-reply-apply-* systems land it on the target entity's Text.content every frame ----
// Also give +affinity to the companion / drifter pointed to by last_talk_target (when a drifter moves into the colony this value carries over).
// Talk cap of 3 per day per companion → guarded by talked_today (over cap: no more +affinity, but the reply is still displayed).
// fn can't call ctx.getField, so: talkNearby stashes the target handle in Colony.last_talk_target,
// and the rule also writes the target's current talked_today/affinity to Colony.last_talk_* copies;
// this fn reads those copies from Colony, adds to them, then writes back (setField across entities).
const TALK_AFFINITY_GAIN = 3;     // +affinity per conversation
const TALK_DAILY_CAP = 3;          // max +affinity conversations per companion per day
vitric.fn("onTalkReply", (reply, ctx) => {
  const text = reply.text || "（对方点了点头）";
  let display = text;
  try {
    const parsed = JSON.parse(text);
    if (parsed.say) display = parsed.say;
  } catch (_) {}
  ctx.setField("colony", "Colony.last_talk_reply", display);
  // Find the target companion / drifter +affinity
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

// ---- Land last_talk_reply on the nearest drifter/companion's Text.content; whichever of the two apply systems hits first consumes it.
// Whoever is closer gets the reply; the other apply sees last_talk_reply already emptied and just returns.
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
    ctx.setField("colony", "Colony.last_talk_reply", ""); // consume only once
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
// iter2 — companion relationship system (affinity / interactions / contribution / mood)
// =====================================================================
//
// Design: aggregate "interaction count + comfort" into affinity (0..100).
//   1. talk (gain +3, max +9/day = 3 talks)        — added in onTalkReply
//   2. gift (preferred hit +12 / generic +3, max +24/day = 2 gifts) — added in giveGiftNearby
//   3. care: high comfort + housing → +0.6/min, sustained rise;
//          very low comfort → -0.3/min, slow decline;
//          no interaction for a long time (>3 days) → -0.5/day decay.
//
// Then affinity + mood drive contribution: every 12 seconds, automatically
//   - +1 stage to the nearest ripe crop on a Plot (help with farming)
//   - +1 ore/wood/fiber to @player inventory (help with gathering)
//   - +0.5 boost to Colony food rate (extra food output)
//   Pick one of three, distributed via ctx.random.
//
// Mood: derived from comfort + distance from last_interact_day + time-of-day → "happy/calm/down/tired",
//   written to Mood.value; companion-mood floats this as a small line above the Persona's Text.
//   (Reuses the existing Text field; the reply text would be overwritten in the same tick, so we use Text to display affinity/mood instead.)

// ---- Mood: comfort + interaction recency + time-of-day → "happy/calm/tired/down/angry" ----
// Priority: angry (<20 comfort and 5 days without interaction) > down (<35 comfort) > tired (night with no housing)
//       > happy (comfort>70 + recent interaction) > calm.
// Only writes Mood.value; never touches Text.content (reply uses Text; can't conflict).
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

// ---- Care (comfort-driven) + long-neglect decay: every frame ----
// Has quarters + comfort > 70 → +0.6/min (0.01/s)
// comfort < 30 and quarters==0 → -0.3/min (-0.005/s)
// No interaction for >= 3 days → -0.5/day ≈ -0.0083/s
const CARE_GAIN_PER_SEC = 0.01;   // +0.6/min
const CARE_LOSS_PER_SEC = 0.005;  // -0.3/min (no housing, low comfort)
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

// ---- Reset talked_today / gifted_daily each day + decay care ----
// On receiving the day-start event → Colony.day has already been +1 (the clock system runs first).
// Approach: store the day start in Colony.day_anchor; the system compares once per day; on mismatch reset each companion's counts.
// Here we take a simpler approach: the clock system emits day-start every tick; this system queries [Companion,Need],
// every frame checks "this entry into the system is still a new day (compared to last day_anchor)" → reset counts.
// query only contains [Companion,Need] (the comment originally said so; the old code accidentally put Colony here so the system never ran).
// day / _day_anchor live on the colony; read via getField and write via setField across entities.
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

// ---- Contribution: affinity>=50 + mood happy/calm → every 12 seconds auto-help the colony ----
// Role-based dispatch (Task 9):
//   builder  → +1 plank to @player.Inventory
//   farmer   → +0.5 Colony.food_rate (one-tick boost)
//   explorer → +1 fiber to @player.Inventory + explore-bonus forward-compat hook
//   guard    → +1 ore to @player.Inventory + guard-patrol forward-compat hook
//   trader   → +1 wheat to @player.Inventory + trade-available forward-compat hook
//   scholar  → emit tp-set {value: tp+1} → tp-apply rule writes @player.TechPoint.value
// Action (A) helping the nearest crop speed up is split into a separate system companion-tend-crops (queries Crop, doesn't go through ctx.world).
const CONTRIB_INTERVAL_SEC = 12.0;
const CONTRIB_AFFINITY_MIN = 50;
// query doesn't include Colony: read/write colony goes via ctx.getField/ctx.setField across entities; putting Colony in the query would make the system
// match 0 entities (no entity has both Companion and Colony) → the entire contribution loop never runs (previous cause of death for iter2).
// Persona is included so e.Persona.role is readable.
vitric.system("companion-contribution", { query: ["Companion", "Need", "Mood", "Persona", "Position"], writes: ["Need"] }, (entities, ctx) => {
  for (const e of entities) {
    const n = e.Need;
    if ((n.affinity || 0) < CONTRIB_AFFINITY_MIN) continue;
    const mood = (e.Mood && e.Mood.value) || "平静";
    if (mood !== "开心" && mood !== "平静") continue;
    n.contribution_timer = (n.contribution_timer || 0) - ctx.dt;
    if (n.contribution_timer > 0) continue;
    n.contribution_timer = CONTRIB_INTERVAL_SEC + ctx.random() * 4; // 12~16 seconds

    const role = (e.Persona && e.Persona.role) || "builder";
    switch (role) {
      case "builder": {
        const cur = ctx.getField("@player", "Inventory.plank") | 0;
        ctx.setField("@player", "Inventory.plank", cur + 1);
        ctx.emit("companion-contributed", { pid: e.id, kind: "plank", role: "builder" });
        break;
      }
      case "farmer": {
        const fr = ctx.getField("colony", "Colony.food_rate");
        ctx.setField("colony", "Colony.food_rate", (typeof fr === "number" ? fr : 0) + 0.5);
        ctx.emit("companion-boost", { pid: e.id, what: "food", role: "farmer" });
        break;
      }
      case "explorer": {
        const cur = ctx.getField("@player", "Inventory.fiber") | 0;
        ctx.setField("@player", "Inventory.fiber", cur + 1);
        ctx.emit("explore-bonus", { pid: e.id, role: "explorer" }); // forward-compat hook for Task 12
        ctx.emit("companion-contributed", { pid: e.id, kind: "fiber", role: "explorer" });
        break;
      }
      case "guard": {
        const cur = ctx.getField("@player", "Inventory.ore") | 0;
        ctx.setField("@player", "Inventory.ore", cur + 1);
        ctx.emit("guard-patrol", { pid: e.id, role: "guard" }); // forward-compat hook for Task 10
        ctx.emit("companion-contributed", { pid: e.id, kind: "ore", role: "guard" });
        break;
      }
      case "trader": {
        const cur = ctx.getField("@player", "Inventory.wheat") | 0;
        ctx.setField("@player", "Inventory.wheat", cur + 1);
        ctx.emit("trade-available", { pid: e.id, role: "trader" }); // forward-compat hook for Task 11
        ctx.emit("companion-contributed", { pid: e.id, kind: "wheat", role: "trader" });
        break;
      }
      case "scholar": {
        const tp = ctx.getField("@player", "TechPoint.value") | 0;
        ctx.emit("tp-set", { value: tp + 1 }); // tp-apply rule in research.json writes TechPoint.value
        ctx.emit("companion-contributed", { pid: e.id, kind: "techpoint", role: "scholar" });
        break;
      }
      default: {
        // Fallback: existing random pick (shouldn't fire — all companions have a role post-Task-9).
        const pick = (ctx.random() * 2) | 0;
        if (pick === 0) {
          const items = ["ore", "wood", "fiber"];
          const which = items[(ctx.random() * items.length) | 0];
          const cur = ctx.getField("@player", "Inventory." + which) | 0;
          ctx.setField("@player", "Inventory." + which, cur + 1);
          ctx.emit("companion-contributed", { pid: e.id, kind: which });
        } else {
          const fr = ctx.getField("colony", "Colony.food_rate");
          ctx.setField("colony", "Colony.food_rate", (typeof fr === "number" ? fr : 0) + 0.5);
          ctx.emit("companion-boost", { pid: e.id, what: "food" });
        }
      }
    }
  }
});

// ---- Help crops speed up: at least one companion with affinity>=50 in the colony → every ~10 seconds give an unripe wheat +1 stage ----
// Split into a separate system because it needs to query Crop (the companion system's query is Companion); uses colony.companion_happy_count
// (written by the companion-tally system) as the trigger condition, to avoid cross-system mutual queries.
const TEND_INTERVAL_SEC = 10.0;
vitric.system("companion-tend-crops", { query: ["Crop", "Position"], writes: ["Crop"] }, (entities, ctx) => {
  const happy = ctx.getField("colony", "Colony.companion_happy_count") | 0;
  if (happy < 1) return;
  // Simple approach: collect all unripe wheat entities, trigger once per tick by probability (determinism controlled by ctx.random)
  const candidates = [];
  for (const e of entities) {
    if (e.Crop.kind !== "wheat") continue;
    if ((e.Crop.stage | 0) >= 3) continue;
    candidates.push(e);
  }
  if (candidates.length === 0) return;
  // Use the first candidate's id as a stable tick-count anchor (each crop times itself; evenly distributed)
  for (const e of candidates) {
    e.Crop._tend_t = ((e.Crop._tend_t || 0) - ctx.dt);
    if (e.Crop._tend_t > 0) continue;
    e.Crop._tend_t = TEND_INTERVAL_SEC + ctx.random() * 3; // 10~13 seconds
    const st = (e.Crop.stage | 0) + 1;
    e.Crop.stage = st > 3 ? 3 : st;
  }
});

// ---- Colony-level tally: happy count + average affinity — for win tie-in / colony-stage gate / contribution triggers ----
// happy = number of companions with affinity>=50 (consistent with the "will help" line in companion-contribution / companion-tend-crops).
// Note: query only contains [Companion,Need] — writing to Colony goes via ctx.setField across entities; Colony must NOT be put in the query
// (no entity has both Companion and Colony; putting it in would make the system match 0 entities and never run).
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

// ---- HUD: when near a companion, show a small card under the HUD (name + affinity + mood) ----
// Reuses Colony.text (screen space): stores a string like "near: Lio affinity 54 · happy".
// Writes this string to @hud_companion_lbl.UiLabel.content.
// Find the nearest companion → only show when distance <= 5, otherwise empty string (label hidden).
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
    // Pull this companion's affinity + mood
    const aff = ctx.getField(best.id, "Need.affinity_i") | 0;
    const mood = (ctx.getField(best.id, "Mood.value") || "").toString();
    const gifted = ctx.getField(best.id, "Need.gifted_today") | 0;
    const talked = ctx.getField(best.id, "Need.talked_today") | 0;
    label = "♥ " + (best.name || "伙伴") + "  好感 " + aff + "  ·" + mood
          + "    今日 谈 " + talked + "/3  礼 " + gifted + "/2";
  }
  ctx.setField("@hud_companion_lbl", "UiLabel.content", label);
});