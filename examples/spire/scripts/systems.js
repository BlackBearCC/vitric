// spire 战斗大脑 — 脚本算账，规则写账。
//
// 数据流（每条命令一个 tick 往返）：
//   输入规则把当前世界状态用 @路径 当参数传进 fn（play-card / end-turn / ...），
//   fn 纯计算出整个结算结果，emit 一组 sync-* 事件（载荷全是【绝对值】，幂等），
//   下一 tick 的 apply-* 规则把这些值写回组件。
//   @battle.Card.slot 当命令寄存器/互锁：输入规则置位（非 0 = 处理中拒新输入），
//   每个 fn 收尾都会 emit sync-battle{slot:0} 复位。
// 敌人回合是事件链：end-turn → enemy-act-req{n:0} → enemy-act-req{n:1} → player-turn-req，
// 每跳一个 tick，sync 事件总在链事件之前 emit，所以下一跳读到的世界永远是最新的。
// 脚本零私藏状态：所有跨 tick 状态都在组件里，快照/回放安全。

// ---- 卡牌表（GDD 定死） ----
const CARDS = {
  "STRIKE":    { cost: 1, dmg: 6 },
  "DEFEND":    { cost: 1, blk: 5 },
  "BASH":      { cost: 2, dmg: 8, vuln: 2 },
  "CLEAVE":    { cost: 1, dmg: 4, aoe: true },
  "IRON WAVE": { cost: 2, dmg: 5, blk: 5 },
};
// 牌库 12 张：STRIKE×5 DEFEND×4 BASH×1 CLEAVE×1 IRON WAVE×1
const DECKLIST = [
  "STRIKE", "STRIKE", "STRIKE", "STRIKE", "STRIKE",
  "DEFEND", "DEFEND", "DEFEND", "DEFEND",
  "BASH", "CLEAVE", "IRON WAVE",
];

// ---- 纯函数工具 ----
function shuffle(arr, ctx) { // Fisher–Yates，ctx.random 保证确定性
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(ctx.random() * (i + 1));
    const t = arr[i]; arr[i] = arr[j]; arr[j] = t;
  }
  return arr;
}

function drawCards(draw, discard, hand, n, ctx) {
  for (let k = 0; k < n && hand.length < 5; k++) {
    if (draw.length === 0) {
      if (discard.length === 0) break; // 牌全在手上/场上，没得抽
      const pile = discard.splice(0, discard.length);
      shuffle(pile, ctx);
      for (const c of pile) draw.push(c);
    }
    hand.push(draw.shift());
  }
}

// 敌人意图（GDD：敌 A 斩击者凶、敌 B 守卫攻守参半）
// 调参 2026-06-11：守卫 DEF 50%→40%；第 9 回合起守卫狂怒，ATK 每回合 +2（龟壳节奏收紧，防御流不再拖 27 回合）
function rollIntent(n, turn, ctx) {
  if (n === 0) return ctx.random() < 0.65 ? { kind: "ATK", value: 8 } : { kind: "ATK", value: 11 };
  const atk = 6 + Math.max(0, turn - 8) * 2; // 狂怒计时钟：意图值如实显示，玩家看得见压力上来
  return ctx.random() < 0.4 ? { kind: "DEF", value: 6 } : { kind: "ATK", value: atk };
}

// 攻击结算：易伤 ×1.5 向下取整，先破甲再扣血；emit blocked / damaged（GDD 事件表）
function attack(ctx, targetName, u, base) {
  const eff = u.vuln > 0 ? Math.floor(base * 1.5) : base;
  const absorbed = Math.min(u.block, eff);
  const hpLoss = eff - absorbed;
  u.block -= absorbed;
  u.hp = Math.max(0, u.hp - hpLoss);
  if (absorbed > 0) ctx.emit("blocked", { target: targetName, amount: absorbed });
  if (hpLoss > 0) ctx.emit("damaged", { target: targetName, amount: hpLoss });
}

// ---- 显示文案（fn 直接拼好字符串，规则只搬运） ----
function unitText(u) {
  if (u.hp <= 0) return "DEAD";
  let s = "HP " + u.hp + "/" + u.maxhp;
  if (u.block > 0) s += " BLK " + u.block;
  if (u.vuln > 0) s += " VULN " + u.vuln;
  return s;
}

// ---- sync 事件（全部绝对值，重放/重复应用幂等） ----
function emitBattle(ctx, phase, energy, turn) {
  ctx.emit("sync-battle", {
    phase, energy, turn, slot: 0, // slot:0 = 复位命令寄存器
    etext: "ENERGY " + energy + "/3",
    ttext: "TURN " + turn,
  });
}
function emitPlayer(ctx, p) {
  ctx.emit("sync-player", { hp: p.hp, block: p.block, hptext: unitText(p) });
}
function emitEnemy(ctx, n, e, intent) {
  ctx.emit("sync-enemy-" + n, {
    hp: e.hp, block: e.block, vuln: e.vuln,
    ikind: e.hp > 0 ? intent.kind : "", ivalue: e.hp > 0 ? intent.value : 0,
    hptext: unitText(e),
    itext: e.hp > 0 && intent.kind ? intent.kind + " " + intent.value : "",
  });
}
function emitDeck(ctx, draw, discard, hand) {
  ctx.emit("sync-deck", { draw, discard, hand });
}
function emitHand(ctx, hand, energy) {
  const d = {};
  for (let i = 0; i < 5; i++) {
    const name = hand[i] || "";
    d["n" + (i + 1)] = name;                                  // Card.name（原始牌名）
    d["s" + (i + 1)] = name;                                  // slot-N-text 牌名标签
    d["o" + (i + 1)] = name ? String(CARDS[name].cost) : ""; // slot-N-cost 费用标签
    // 卡面着色（场景空槽底色 #3a3442）：有牌提亮、付不起半暗
    d["c" + (i + 1)] = !name ? "#3a3442" : (CARDS[name].cost <= energy ? "#6b5d80" : "#4a4255");
  }
  ctx.emit("hand-sync", d);
}

// ---- 战斗入口：start 事件触发，洗牌、开局抽 5、定初始意图 ----
vitric.fn("start-battle", (a, ctx) => {
  const draw = shuffle(DECKLIST.slice(), ctx);
  const discard = [];
  const hand = [];
  drawCards(draw, discard, hand, 5, ctx);
  const p = { hp: a.php, maxhp: a.pmax, block: 0, vuln: 0 };
  const e0 = { hp: a.e0hp, maxhp: a.e0max, block: 0, vuln: 0 };
  const e1 = { hp: a.e1hp, maxhp: a.e1max, block: 0, vuln: 0 };
  emitDeck(ctx, draw, discard, hand);
  emitHand(ctx, hand, 3);
  emitPlayer(ctx, p);
  emitEnemy(ctx, 0, e0, rollIntent(0, 1, ctx));
  emitEnemy(ctx, 1, e1, rollIntent(1, 1, ctx));
  emitBattle(ctx, "player", 3, 1);
});

// ---- 出牌：按 1-5 触发，自动指向第一个活着的敌人（CLEAVE 打全体） ----
vitric.fn("play-card", (a, ctx) => {
  const hand = a.hand.slice();
  const name = hand[a.slot - 1];
  if (!name) {
    ctx.emit("card-rejected", { slot: a.slot, reason: "empty" });
    emitBattle(ctx, "player", a.energy, a.turn); // 只为复位命令寄存器
    return;
  }
  const card = CARDS[name];
  if (card.cost > a.energy) {
    ctx.emit("card-rejected", { slot: a.slot, reason: "energy" });
    emitBattle(ctx, "player", a.energy, a.turn);
    return;
  }

  const discard = a.discard.slice();
  const p = { hp: a.php, maxhp: a.pmax, block: a.pblock, vuln: 0 };
  const e0 = { hp: a.e0hp, maxhp: a.e0max, block: a.e0block, vuln: a.e0vuln };
  const e1 = { hp: a.e1hp, maxhp: a.e1max, block: a.e1block, vuln: a.e1vuln };
  const i0 = { kind: a.e0ikind, value: a.e0ivalue };
  const i1 = { kind: a.e1ikind, value: a.e1ivalue };

  hand.splice(a.slot - 1, 1);
  discard.push(name);
  const energy = a.energy - card.cost;
  ctx.emit("card-played", { name, cost: card.cost });

  if (card.dmg) {
    const living = [[0, e0], [1, e1]].filter((t) => t[1].hp > 0);
    const targets = card.aoe ? living : living.slice(0, 1);
    for (const t of targets) {
      attack(ctx, "enemy-" + t[0], t[1], card.dmg);
      if (card.vuln) t[1].vuln += card.vuln;
    }
  }
  if (card.blk) p.block += card.blk;

  emitDeck(ctx, a.draw, discard, hand);
  emitHand(ctx, hand, energy);
  emitPlayer(ctx, p);
  emitEnemy(ctx, 0, e0, i0);
  emitEnemy(ctx, 1, e1, i1);
  const won = e0.hp <= 0 && e1.hp <= 0;
  emitBattle(ctx, won ? "won" : "player", energy, a.turn);
  if (won) ctx.emit("battle-won", {});
});

// ---- 结束回合：手牌入弃牌堆，敌人旧甲过期，开敌人行动链 ----
vitric.fn("end-turn", (a, ctx) => {
  const discard = a.discard.concat(a.hand);
  const e0 = { hp: a.e0hp, maxhp: a.e0max, block: 0, vuln: a.e0vuln };
  const e1 = { hp: a.e1hp, maxhp: a.e1max, block: 0, vuln: a.e1vuln };
  ctx.emit("turn-ended", {});
  emitDeck(ctx, a.draw, discard, []);
  emitHand(ctx, [], 0);
  emitEnemy(ctx, 0, e0, { kind: a.e0ikind, value: a.e0ivalue });
  emitEnemy(ctx, 1, e1, { kind: a.e1ikind, value: a.e1ivalue });
  emitBattle(ctx, "enemy", 0, a.turn);
  ctx.emit("enemy-act-req", { n: 0 }); // 链事件永远最后 emit，保证下一跳读到最新状态
});

// ---- 单个敌人按意图行动（链上一跳一个 tick） ----
vitric.fn("enemy-act", (a, ctx) => {
  const p = { hp: a.php, maxhp: a.pmax, block: a.pblock, vuln: 0 };
  const e = { hp: a.ehp, maxhp: a.emax, block: a.eblock, vuln: a.evuln };
  if (a.kind === "ATK") {
    attack(ctx, "player", p, a.value);
  } else if (a.kind === "DEF") {
    e.block += a.value;
  }
  ctx.emit("enemy-acted", { n: a.n });
  emitPlayer(ctx, p);
  emitEnemy(ctx, a.n, e, { kind: a.kind, value: a.value });
  if (p.hp <= 0) {
    emitBattle(ctx, "lost", 0, a.turn);
    ctx.emit("battle-lost", {});
    return; // 链到此为止
  }
  emitBattle(ctx, "enemy", 0, a.turn);
  if (a.n === 0) ctx.emit("enemy-act-req", { n: 1 });
  else ctx.emit("player-turn-req", {});
});

// ---- 新回合：易伤衰减、玩家甲过期、新意图、回能量、抽满 5 ----
vitric.fn("begin-turn", (a, ctx) => {
  const draw = a.draw.slice();
  const discard = a.discard.slice();
  const hand = [];
  drawCards(draw, discard, hand, 5, ctx);
  const p = { hp: a.php, maxhp: a.pmax, block: 0, vuln: 0 };
  const e0 = { hp: a.e0hp, maxhp: a.e0max, block: a.e0block, vuln: Math.max(0, a.e0vuln - 1) };
  const e1 = { hp: a.e1hp, maxhp: a.e1max, block: a.e1block, vuln: Math.max(0, a.e1vuln - 1) };
  const turn = a.turn + 1;
  const i0 = e0.hp > 0 ? rollIntent(0, turn, ctx) : { kind: "", value: 0 };
  const i1 = e1.hp > 0 ? rollIntent(1, turn, ctx) : { kind: "", value: 0 };
  emitDeck(ctx, draw, discard, hand);
  emitHand(ctx, hand, 3);
  emitPlayer(ctx, p);
  emitEnemy(ctx, 0, e0, i0);
  emitEnemy(ctx, 1, e1, i1);
  emitBattle(ctx, "player", 3, turn);
});

// ---- 打击感：粒子迸发（寿命交给引擎 Particle 系统收尾） ----
vitric.fn("burst", (args, ctx) => {
  const kinds = {
    hit:      { colors: ["#ff5a3c", "#ffb03c", "#ffe08a"], up: 4, spread: 6, ttl: 22, s: 0.32 },
    block:    { colors: ["#7ec8ff", "#b8e0ff", "#e8f4ff"], up: 3, spread: 4, ttl: 20, s: 0.28 },
    confetti: { colors: ["#ff6bd6", "#7dff8a", "#5ec8ff", "#ffd75e"], up: 9, spread: 8, ttl: 50, s: 0.35 },
  };
  const k = kinds[args.kind] || kinds.hit;
  for (let i = 0; i < args.n; i++) {
    const c = k.colors[Math.floor(ctx.random() * k.colors.length)];
    ctx.spawn({
      Position: { x: args.x + (ctx.random() - 0.5) * 1.2, y: args.y + (ctx.random() - 0.3) * 1.2 },
      Velocity: { x: (ctx.random() - 0.5) * 2 * k.spread, y: k.up * (0.4 + ctx.random() * 0.9) },
      Sprite:   { w: k.s, h: k.s, color: c },
      Particle: { ttl: k.ttl + Math.floor(ctx.random() * 8) },
    });
  }
});

// ---- 火把摇曳：intensity 是 (tick, 实体id) 的纯函数，零私藏状态 ----
// 注意：本系统接管所有 Light+Position 实体的 intensity（围绕 ~1.05 摆动），
// 美术调火把亮度请动 radius / color。
vitric.system("torch-flicker", { query: ["Light", "Position"], writes: ["Light"] }, (entities, ctx) => {
  for (const e of entities) {
    let h = 0;
    for (let i = 0; i < e.id.length; i++) h = (h * 31 + e.id.charCodeAt(i)) % 997;
    const t = ctx.tick * 0.13 + h;
    e.Light.intensity = 1.05 + 0.14 * Math.sin(t) + 0.06 * Math.sin(t * 2.7 + 1.3);
  }
});
