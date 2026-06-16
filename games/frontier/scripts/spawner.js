// 伙伴到来(P6 雏形):每隔一阵,没满员就向大模型现生成一个旅人的人设并落地。
// 到来的伙伴设计成"无名、永久的背景帮手"——会溜达、被普查计入帮工,但不参与对话/需求/离开。
// 这绕开两个白模坑:① 从系统 despawn 无名实体会留尸 ② 多伙伴对话回复路由。原版 @companion 仍是全功能活伙伴。

const SPAWN_INTERVAL = 8.0; // 秒;到点且没满员就招一个新旅人
const ARRIVE_X = 2; // 落脚点(区域一角)
const ARRIVE_Y = 2;

// 计时 + 上限判定都在 @colony(它有 Census 计数和 Spawn 计时)。
vitric.system("spawner", { query: ["Census", "Spawn"], writes: ["Spawn"] }, (entities, ctx) => {
  for (const e of entities) {
    const s = e.Spawn;
    s.timer -= ctx.dt;
    if (s.timer <= 0) {
      s.timer = SPAWN_INTERVAL;
      if (e.Census.count < s.cap) {
        // prompt 含"来历",好让(假)大模型按人设格式回
        ctx.ask(
          "llm",
          "生成一个漂泊到这处荒星定居点的旅人,各有来历脾气。只回 JSON:" +
            "{\"name\":\"名字\",\"archetype\":\"类型\",\"traits\":\"性格(逗号分隔)\",\"speech\":\"说话习惯\"}",
          "spawnCompanion"
        );
      }
    }
  }
});

// 人设回来,在落脚点生成一个新伙伴(无名背景帮手:有 Companion/Persona/Census/Wander,
// 没有 Need/Mood/ThinkReq/Text,所以不闹需求、不离开、不抢对话)。
vitric.fn("spawnCompanion", (reply, ctx) => {
  let p = { name: "旅人", archetype: "", traits: "", speech: "" };
  try {
    const r = JSON.parse(reply.text);
    if (r && typeof r.name === "string") p = r;
  } catch (err) {
    // 没回合法人设也别崩,用默认
  }
  ctx.spawn({
    Companion: {},
    Persona: p,
    Census: { is_hub: 0, count: 0 },
    Wander: { home_x: ARRIVE_X, home_y: ARRIVE_Y, tx: ARRIVE_X, ty: ARRIVE_Y, timer: 1 },
    Position: { x: ARRIVE_X, y: ARRIVE_Y },
    Velocity: { x: 0, y: 0 },
    Sprite: { w: 0.9, h: 0.9, image: "companion.png" },
  });
});
