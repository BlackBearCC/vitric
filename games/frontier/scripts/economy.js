// 经营算账：建造 + 制作。脚本只算账（够不够料、扣完剩多少、摆在哪），
// 真正的写账（改背包数字、生成结构实体）一部分在这里直接做（spawn 结构），
// 背包扣减走 emit "inv-set"{绝对值} → rules/economy.json 写回（spire 的幂等回写法：
// 载荷全是扣完后的绝对值，重复应用/重放都一致）。
//
// 为什么背包不在这里直接改：fn 只能 spawn/despawn 整实体 + 写自己 query 到的组件，
// 够不到 @player.Inventory（它是别的实体）。所以规则把当前背包当参数传进来，
// 这里算出扣完的新值，emit 回去让规则 set。

// 建造表（GDD + 纵深）：kind -> 料 + tier + 配色 + label + 视觉尺寸。
//   尺寸分层（按 _layout_spec.md 第 3 节）：
//   - 普通结构 plot/wall/conduit/extractor/plot2 = 1.0~1.1（在地块之上微凸）
//   - quarters 住所 1.1（比墙高一点）
//   - beacon 信标 1.8 / monument 丰碑 2.0（地标,场上最大最显眼）
const BUILD = {
  plot:      { cost: {},                              tier: 1, color: "#6b8f3a", label: "种植台", size: 1.0 },
  wall:      { cost: { wood: 1 },                     tier: 1, color: "#8a7a5c", label: "墙",     size: 1.0 },
  conduit:   { cost: { ore: 1 },                      tier: 1, color: "#d8a83a", label: "电导管", size: 1.0 },
  extractor: { cost: { ore: 1 },                      tier: 1, color: "#4aa6c8", label: "抽水机", size: 1.0 },
  quarters:  { cost: { plank: 2 },                    tier: 1, color: "#c08a4a", label: "住所",   size: 1.1 },
  beacon:    { cost: { ore: 2, plank: 2 },            tier: 1, color: "#f5b942", label: "信标",   size: 1.8 },
  plot2:     { cost: { plank: 3, chair: 1 },          tier: 2, color: "#a8e85a", label: "良田",   size: 1.05 },
  monument:  { cost: { ore: 4, plank: 4, lamp: 2, wheat: 4 }, tier: 3, color: "#ffe066", label: "丰碑", size: 2.0 },
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
  if (!canPay(inv, def.cost)) { ctx.emit("build-fail", { kind: a.kind, label: def.label }); return; } // 料不够 → 通知
  const gx = Math.round(a.x);
  const gy = Math.round(a.y);
  pay(inv, def.cost);
  // ctx.spawn(组件对象, 可选名字)——组件是扁平第一参数(不是 {components:...})；这里匿名 spawn。
  const comps = {
    Structure: { kind: a.kind, tier: def.tier || 1 },
    Position: { x: gx, y: gy },
    Sprite: { w: def.size, h: def.size, color: def.color },
    Text: { content: def.label, size: 0.34, color: "#ffffff", screen: false }, // 名字标在结构上
  };
  // 种植台建出来就挂一个空 Crop——之后互动点它直接 setField 把作物种在这块地上(原地种,不再 spawn 另一个作物实体)。
  if (a.kind === "plot") comps.Crop = { kind: "", stage: 0, timer: 0 };
  ctx.spawn(comps);
  emitInv(ctx, inv);
  ctx.emit("built", { kind: a.kind, label: def.label, x: gx, y: gy });
});

// ---- 互动点击：规则把"点中的实体(a.entity 句柄/名字)+它的组件(a.comp)+当前背包"传进来 ----
// 点中的若是种植台：空地 + 有种子 → 种(setField Crop.kind=wheat)；熟了(stage>=3) → 收(清空 + 麦子+2)。
// 点中的若是野外资源点(Node)：采集 ore/wood/fiber 进背包。
// 直接对"点中的那块地"用 ctx.setField 写——不再要命令寄存器/格子匹配(引擎已支持点击带命中实体 entity+comp)。
vitric.fn("interact", (a, ctx) => {
  const comp = a.comp || {};
  const st = comp.Structure;
  const crop = comp.Crop;
  const node = comp.Node;
  // ---- 种植台 ----
  if (st && st.kind === "plot" && crop) {
    const inv = readInv(a);
    if (crop.kind === "" && (inv.seed | 0) > 0) {
      // 种：扣 1 种子 + 把麦子作物挂在这块地上(原地)
      inv.seed -= 1;
      ctx.setField(a.entity, "Crop.kind", "wheat");
      ctx.setField(a.entity, "Crop.stage", 0);
      ctx.setField(a.entity, "Crop.timer", 0);
      emitInv(ctx, inv);
      ctx.emit("planted", { x: a.x, y: a.y });
    } else if (crop.kind === "" && (inv.seed | 0) <= 0) {
      ctx.emit("plant-fail", {});
    } else if (crop.kind === "wheat" && (crop.stage | 0) >= 3) {
      // 收：清空作物 + 麦子 +2
      inv.wheat += 2;
      ctx.setField(a.entity, "Crop.kind", "");
      ctx.setField(a.entity, "Crop.stage", 0);
      emitInv(ctx, inv);
      ctx.emit("harvested", { id: "wheat", n: 2 });
    }
    return;
  }
  // ---- 野外资源点采集(场景已预铺 6 个节点,left>0 即可采) ----
  if (node && (node.left | 0) > 0) {
    const inv = readInv(a);
    const nodeKind = node.kind || "ore";
    const ITEM_MAP = { ore: "ore", wood: "wood", fiber: "fiber" };
    const itemId = ITEM_MAP[nodeKind] || "ore";
    inv[itemId] += 1;
    ctx.setField(a.entity, "Node.left", (node.left | 0) - 1);
    emitInv(ctx, inv);
    ctx.emit("gathered", { node: nodeKind, id: itemId, n: 1 });
    return;
  }
});

// ---- 制作：规则在点配方按钮（craft-<id>）时调，把配方 id + 当前背包传进来 ----
// 够料：扣料 + 产物 +1 → emit inv-set（产物的 +1 已并进绝对值）+ emit crafted。不够：no-op。
vitric.fn("craft", (a, ctx) => {
  const rec = CRAFT[a.id];
  if (!rec) return;
  const inv = readInv(a);
  if (!canPay(inv, rec.cost)) { ctx.emit("craft-fail", { id: a.id }); return; }
  pay(inv, rec.cost);
  inv[rec.out] += 1;
  emitInv(ctx, inv);
  ctx.emit("crafted", { id: rec.out, n: 1 });
});
