// dialogue-demo script — movement helper + dialogue HUD renderer.
// The `move` fn exists because the rule DSL can't negate a path (no `-@player.Speed.value`).
// `render_dialogue_hud` shows the active dialogue node's text + choices, or a prompt.

vitric.fn("move", (args, ctx) => {
  const dir = Number(args.dir) || 0;
  const speed = ctx.getField("@player", "Speed.value") || 60;
  ctx.setField("@player", "Velocity.x", dir * speed);
});

vitric.fn("render_dialogue_hud", (args, ctx) => {
  const who = args.who;
  const npc = args.npc;
  const hud = args.hud;
  if (!who || !npc || !hud) throw new Error("render_dialogue_hud: 缺少 who/npc/hud");

  const current = ctx.getField(who, "DialogueRunner.current");
  if (current === undefined || current === null || current < 0) {
    // Not in dialogue — show a prompt.
    const talked = ctx.getField(npc, "Talked.count") || 0;
    const prompt = talked > 0
      ? "Talked to elder. Walk right to talk again."
      : "Walk right to talk to the elder. Press 1 to choose.";
    ctx.setField(hud, "Text.content", prompt);
    return;
  }

  // In dialogue — show the NPC's current node text + choices.
  const texts = ctx.getField(npc, "Dialogue.node_text") || [];
  const choicesRaw = ctx.getField(npc, "Dialogue.node_choices") || [];
  const text = texts[current] || "...";
  const choices = (choicesRaw[current] || "").split(";").filter((s) => s.trim() !== "");
  const choiceText = choices.length === 0
    ? "(end)"
    : choices.map((c, i) => (i + 1) + ". " + c).join("  ");
  ctx.setField(hud, "Text.content", "Elder: " + text + "  [" + choiceText + "]");
});
