// quest-demo script — movement helper + HUD renderer.
// The rule DSL can't negate a path (no `-@player.Speed.value`), so directional input
// is routed through this tiny `move` fn which reads Speed and applies a sign.
// `render_hud` reads quest state + inventory and writes a one-line status to the HUD.

vitric.fn("move", (args, ctx) => {
  const dir = Number(args.dir) || 0;
  const speed = ctx.getField("@player", "Speed.value") || 60;
  ctx.setField("@player", "Velocity.x", dir * speed);
});

vitric.fn("render_hud", (args, ctx) => {
  const who = args.who;
  const quest = args.quest;
  const hud = args.hud;
  if (!who || !quest || !hud) throw new Error("render_hud: 缺少 who/quest/hud");

  const state = ctx.getField(quest, "QuestState.state") || "inactive";
  const progress = ctx.getField(quest, "QuestState.progress") || 0;
  const target = ctx.getField(quest, "QuestObjective.target") || 0;
  const title = ctx.getField(quest, "QuestDef.title") || "?";

  const items = ctx.getField(who, "Inventory.items") || [];
  const counts = ctx.getField(who, "Inventory.counts") || [];
  const invText = items.length === 0
    ? "empty"
    : items.map((it, i) => it + "x" + counts[i]).join(", ");

  const stateLabel = {
    inactive: "none",
    offered: "available",
    active: title + " (" + progress + "/" + target + ")",
    completed: "ready to turn in",
    "turned-in": "done",
    failed: "failed",
  }[state] || state;

  ctx.setField(hud, "Text.content", "Quest: " + stateLabel + " | Inventory: " + invText);
});
