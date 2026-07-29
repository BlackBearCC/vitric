// P2 environmental narrative: relics scattered in the wild tell a fragmented story
// when the player approaches. Each relic is a non-interactive decoration entity that
// shows its Text.content only when the player is within 2 tiles.
//
// relic-proximity: query all Relic entities with Position + Text. Set Text.content to
// Relic.text when player is near, blank otherwise. Writes Text.content only.

vitric.system("relic-proximity", { query: ["Relic", "Position", "Text"], writes: ["Text"] }, (entities, ctx) => {
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  for (const e of entities) {
    const dx = e.Position.x - px;
    const dy = e.Position.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 <= 4.0) {
      // Within 2 tiles — show the relic's text.
      const t = e.Relic.text || "";
      if (e.Text.content !== t) e.Text.content = t;
    } else {
      if (e.Text.content !== "") e.Text.content = "";
    }
  }
});
