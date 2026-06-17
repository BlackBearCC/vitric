// 经营算账：建造 + 制作。脚本只算账（够不够料、扣完剩多少、摆在哪），
// 真正的写账（改背包数字、生成结构实体）一部分在这里直接做（spawn 结构），
// 背包扣减走 emit "inv-set"{绝对值} → rules/economy.json 写回（spire 的幂等回写法：
// 载荷全是扣完后的绝对值，重复应用/重放都一致）。
//
// 为什么背包不在这里直接改：fn 只能 spawn/despawn 整实体 + 写自己 query 到的组件，
// 够不到 @player.Inventory（它是别的实体）。所以规则把当前背包当参数传进来，
// 这里算出扣完的新值，emit 回去让规则 set。

// 建造表（GDD）：kind -> 料；plot 免费。灰盒配色（无贴图，避免缺图崩渲染）。
const BUILD = {
  plot:      { cost: {},                       color: "#6b8f3a" }, // 种植台：绿
  wall:      { cost: { wood: 1 },              color: "#8a7a5c" }, // 墙：土黄
  conduit:   { cost: { ore: 1 },               color: "#d8a83a" }, // 电导管：琥珀（电）
  extractor: { cost: { ore: 1 },               color: "#4aa6c8" }, // 抽水机：蓝（水）
  quarters:  { cost: { plank: 2 },             color: "#c08a4a" }, // 住所：暖棕
  beacon:    { cost: { ore: 2, plank: 2 },     color: "#e85a5a" }, // 信标：红（主线目标）
};

// 制作配方（GDD）：产物 -> 料。
const CRAFT = {
  plank: { cost: { wood: 2 },            out: "plank" },
  chair: { cost: { plank: 1, fiber: 1 }, out: "chair" },
  lamp:  { cost: { plank: 1, ore: 1 },   out: "lamp" },
};

// 背包字段全集（与 schema Inventory 对齐）——回写时一律带全集绝对值，规则一条条 set。
const ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"];

// 从传进来的 args 取当前背包（每项缺省 0）。
function readInv(a) {
  const inv = {};
  for (const k of ITEMS) inv[k] = a[k] | 0;
  return inv;
}

// 料够不够。
function canPay(inv, cost) {
  for (const k in cost) {
    if ((inv[k] | 0) < cost[k]) return false;
  }
  return true;
}

// 扣料（就地改 inv 副本）。
function pay(inv, cost) {
  for (const k in cost) inv[k] -= cost[k];
}

// 把背包绝对值 emit 回去（规则写 @player.Inventory.*）。
function emitInv(ctx, inv) {
  const d = {};
  for (const k of ITEMS) d[k] = inv[k];
  ctx.emit("inv-set", d);
}

// ---- 建造：规则在"建造模式 + 左键点地"时调，把点击世界坐标 + 命中实体名 + 选中 kind + 当前背包传进来 ----
// 只在点中地表瓦片（名字形如 t_<x>_<y>）时建——点中 UI/已有结构/空地一律忽略，
// 防止点建造菜单按钮时窗口同时注入的世界点击误建（窗口左键会同时发世界 mouse + ui-click）。
// 够料：扣料 + 在四舍五入到整格的位置 spawn 结构（匿名实体——不用稳定名,避免同格重建撞名崩溃；
// plot/作物的定位靠规则按取整坐标匹配,不依赖结构名）+ emit built。
// 不够 / 没点中瓦片：什么都不做（spec：insufficient → no-op）。
vitric.fn("build", (a, ctx) => {
  const def = BUILD[a.kind];
  if (!def) return; // 未知 kind（没选）——忽略
  if (typeof a.entity !== "string" || !/^t_[0-9]+_[0-9]+$/.test(a.entity)) return; // 没点中地表瓦片
  const inv = readInv(a);
  if (!canPay(inv, def.cost)) return; // 料不够，静默不建
  const gx = Math.round(a.x);
  const gy = Math.round(a.y);
  pay(inv, def.cost);
  // ctx.spawn(组件对象, 可选名字)——组件是扁平第一参数(不是 {components:...})；这里匿名 spawn。
  ctx.spawn({
    Structure: { kind: a.kind },
    Position: { x: gx, y: gy },
    Sprite: { w: 1, h: 1, color: def.color },
  });
  emitInv(ctx, inv);
  ctx.emit("built", { kind: a.kind, x: gx, y: gy });
});

// ---- 互动点击：规则在"互动模式 + 左键点地"时调，把点击世界坐标传进来 ----
// 这里只负责把浮点点击四舍五入到整格,写进命令寄存器 @cmd（emit cmd-set,规则落 @cmd.Cmd）。
// 真正的种地/收割由 rules/farm.json 的 tick-each 规则读 @cmd + 扫 plot/作物来做
// （脚本够不到"点中格子上的那个 plot/作物"实体,只能把格子坐标交给规则去匹配）。
vitric.fn("interact", (a, ctx) => {
  const gx = Math.round(a.x);
  const gy = Math.round(a.y);
  ctx.emit("cmd-set", { kind: "interact", x: gx, y: gy });
});

// ---- 制作：规则在点配方按钮（craft-<id>）时调，把配方 id + 当前背包传进来 ----
// 够料：扣料 + 产物 +1 → emit inv-set（产物的 +1 已并进绝对值）+ emit crafted。不够：no-op。
vitric.fn("craft", (a, ctx) => {
  const rec = CRAFT[a.id];
  if (!rec) return;
  const inv = readInv(a);
  if (!canPay(inv, rec.cost)) return;
  pay(inv, rec.cost);
  inv[rec.out] += 1;
  emitInv(ctx, inv);
  ctx.emit("crafted", { id: rec.out, n: 1 });
});
