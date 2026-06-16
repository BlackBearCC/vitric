// 建造系统(P1):点击格子生成结构、右键拆除。规则把点击的世界坐标 + 选中类型转进来。
// 结构作为新实体盖在地表瓦片上(运行时 spawn 的实体 id 更大 → 画在地表之上)。

const KIND_IMG = {
  floor: "floor.png",
  wall: "wall.png",
  plot: "plot.png",
  conduit: "conduit.png",
};

// 左键:在点中的格子(四舍五入到整格)生成选中类型的结构。
vitric.fn("build", (a, ctx) => {
  const gx = Math.round(a.x);
  const gy = Math.round(a.y);
  const img = KIND_IMG[a.kind] || "floor.png";
  ctx.spawn({
    Structure: { kind: a.kind },
    Position: { x: gx, y: gy },
    Sprite: { w: 1, h: 1, image: img },
  });
});

// 右键:拆掉点中的结构。只拆运行时生成的无名结构(句柄形如 e12v0),
// 命名实体(地表 t_x_y、player、ui、camera)一律不碰,免得右键把地表抠出洞。
vitric.fn("remove", (a, ctx) => {
  if (typeof a.entity !== "string" || !/^e[0-9]+v[0-9]+$/.test(a.entity)) return;
  ctx.despawn(a.entity);
});
