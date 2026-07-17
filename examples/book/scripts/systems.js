// 2D fake-3D page flip: leaf width = cos projection (as if rotating around the spine); first half folds in on the right, second half unfolds on the left
const FLIP_TICKS = 36;
const PAGE_W = 9.9, HALF = PAGE_W / 2, GAP = 0.1;

vitric.system("page-flip", { query: ["Leaf", "Sprite", "Position"], writes: ["Leaf", "Sprite", "Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if (!e.Leaf.flipping) continue;
    e.Leaf.t += 1;
    const p = e.Leaf.t / FLIP_TICKS;          // 0..1
    const ang = p * Math.PI;                  // rotate half a turn around the spine
    const proj = Math.cos(ang);               // width projection 1..0..-1
    const w = Math.abs(proj) * PAGE_W;
    e.Sprite.w = w;
    // Page center position: hugging the spine, on the rotating side
    e.Position.x = (proj >= 0 ? 1 : -1) * (GAP + w / 2);
    // Back is slightly darker than front; swap color past the midpoint
    e.Sprite.color = proj >= 0 ? "#f2e8d0" : "#e6d9bc";
    if (e.Leaf.t === Math.ceil(FLIP_TICKS / 2)) {
      // Crossing the spine: reveal the front of the next sheet
      ctx.emit("page-mid", { n: e.Leaf.page + 2 });
    }
    if (e.Leaf.t >= FLIP_TICKS) {
      e.Leaf.flipping = false;
      e.Leaf.t = 0;
      ctx.emit("page-turned", { n: e.Leaf.page + 1 });
      e.Leaf.page += 2;
      e.Sprite.w = 0.0;
    }
  }
});
