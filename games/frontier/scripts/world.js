// 野外区(增量2):家园区(x0..15)右边接一片野外地表(x16..27),散布资源点(矿脉/林木/纤维丛)。
// 在 start 事件里一次性 spawn(确定性、固定位置)——不手写几百行场景。
// 采集逻辑在 economy.js 的 interact fn(互动模式点中资源点 → 采,用引擎的 ctx.setField 直接写那个点)。
// 这里只负责:① 铺野外地 + 放资源点;② 资源点采后的短冷却(防连点)。

vitric.fn("genWild", (a, ctx) => {
  // 野外地表:x16..27, y0..11,深色岩土(x16 边界稍亮,提示从家园过渡到野外)。
  for (let gx = 16; gx <= 27; gx++) {
    for (let gy = 0; gy <= 11; gy++) {
      ctx.spawn({
        Cell: { kind: "wild" },
        Position: { x: gx, y: gy },
        Sprite: { w: 1, h: 1, image: "", color: gx === 16 ? "#5a5040" : "#48402f" },
      });
    }
  }
  // 资源点:kind 直接对应背包字段(ore/wood/fiber),left=可采次数。带名字标签。
  const NODES = [
    ["ore", 19, 3, "矿脉", "#caa45a"], ["ore", 25, 9, "矿脉", "#caa45a"],
    ["wood", 22, 2, "林木", "#5f8f3a"], ["wood", 24, 10, "林木", "#5f8f3a"],
    ["fiber", 20, 7, "纤维丛", "#9aac5a"], ["fiber", 26, 5, "纤维丛", "#9aac5a"],
  ];
  for (const n of NODES) {
    ctx.spawn({
      Node: { kind: n[0], left: 5, max: 5, cooldown: 0 },
      Position: { x: n[1], y: n[2] },
      // 资源点 ≈ 0.9：比地块小一点,像"散落物",名字标签让玩家知道这是什么
      Sprite: { w: 0.9, h: 0.9, image: "", color: n[4] },
      Text: { content: n[3], size: 0.34, color: "#ffffff", screen: false },
    });
  }
});

// 资源点:采一下后短冷却(防同一下连发)。每帧把 cooldown 往下减。
// 采空(left=0)的点就留着(不自动再生;6 个点共 30 次采集,够本增量的制作用料)。
vitric.system("node", { query: ["Node"], writes: ["Node"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Node.cooldown > 0) e.Node.cooldown = Math.max(0, e.Node.cooldown - ctx.dt);
  }
});
