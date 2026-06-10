// 2D 仿 3D 翻页:活页宽度=cos 投影(像绕书脊转),前半程在右侧收拢,后半程翻到左侧展开
const FLIP_TICKS = 36;
const PAGE_W = 9.9, HALF = PAGE_W / 2, GAP = 0.1;

vitric.system("page-flip", { query: ["Leaf", "Sprite", "Position"], writes: ["Leaf", "Sprite", "Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if (!e.Leaf.flipping) continue;
    e.Leaf.t += 1;
    const p = e.Leaf.t / FLIP_TICKS;          // 0..1
    const ang = p * Math.PI;                  // 绕书脊转半圈
    const proj = Math.cos(ang);               // 宽度投影 1..0..-1
    const w = Math.abs(proj) * PAGE_W;
    e.Sprite.w = w;
    // 页心位置:贴着书脊,在转动那一侧
    e.Position.x = (proj >= 0 ? 1 : -1) * (GAP + w / 2);
    // 背面比正面深一点,翻过中点换色
    e.Sprite.color = proj >= 0 ? "#f2e8d0" : "#e6d9bc";
    if (e.Leaf.t === Math.ceil(FLIP_TICKS / 2)) {
      // 过书脊:揭示下一张纸的正面
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
