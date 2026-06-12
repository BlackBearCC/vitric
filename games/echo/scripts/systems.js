// 回响 echo — 战斗大脑（玩法层）
//
// 架构（脚本算账 + 规则执行 + 指令寄存器，spire 验证过的路子）：
// - 输入规则只写 ctl 实体上的寄存器字段（指令/点击坐标/选牌槽位），不做逻辑。
// - battle-brain 系统每 tick 读寄存器算账：出牌结算、影怪 AI、光照判定、胜负。
//   落写自己 query 内的组件（Shade/Cell/Position/Card），query 外的落写
//   一律 emit sync-* 事件（载荷全是绝对值，幂等），由规则收事件后写回。
// - 脚本零私藏状态：所有跨 tick 状态都在组件里，快照/回放安全。
//
// 【棋子档案 = Shade 组件】战斗里所有逻辑实体（提灯人/影怪/灯/遮板/寄存器）
// 共用一套组件档案 {Shade, Cell, Position, Card}，Shade.kind 区分身份：
//   "hero"     提灯人：Shade.hp=心 Shade.n=灯油（Player 组件是只读镜像，见 hero-player-sync）
//   "stalker" / "lurker" / "devourer"  影怪（怪表三种）：hp=命 n=行动序号 stunned=僵直
//   "lamp"     灯：Card.name="point"|"spot"（棱镜化） Card.slot=聚光朝向（度）
//   "blocker"  遮板（另带 Solid+Collider，挡光挡路由引擎承担）
//   "ctl"      指令寄存器（每个战斗场景一只，详见下面寄存器表）
//   "node"     节点标签：Shade.n = 本战斗的节点号（1/2/3）
// 这是查询机制的要求：脚本系统按组件 AND 匹配，只有同档案才能在一个系统里互见。
// ⚠ 所以「Shade 组件 ≠ 影怪」，数影怪要按 kind 过滤——QA 断言别直接数 Shade 实体。
//
// 【ctl 寄存器表】（战斗场景必须包含名为 ctl 的实体）
//   Shade.hp   = 阶段：0 玩家回合 / 1 影怪回合 / 2 胜 / 3 负
//   Shade.n    = 回合数（从 1 起）
//   Card.name  = 手牌寄存器：4 槽逗号串，如 "lamp,flash,," ；"" = 尚未发牌（开战哨兵）
//   Card.slot  = 当前选中的手牌槽（0=未选，1..4）
//   Cell.cx    = 指令：0 空闲 / 1 棋盘点击 / 2 结束回合 / 11..14 切换选中第 N 张牌
//   Cell.cy    = 阶段内进度：影怪回合=下一个行动的影怪序、胜负阶段=退场倒计时
//   Position   = 最近一次点击的世界坐标（棋盘点击用）
//
// 【光照判定 = 镜像引擎公式】isLit() 在脚本里按引擎同一套公式算格中心照度，
// 与引擎渲染的已知偏差（v1，调参/排查都先看这里）：
//   1. 只采样格中心一点（引擎逐像素）；
//   2. 灯色按标量亮度近似：luma(#ffb060)=0.689 乘进贡献（引擎逐通道）；
//   3. 环境光 Ambient 不计入（GDD：『被任意灯照到』只看灯的贡献）；
//   4. 格中心恰为灯心时聚光角度衰减按 1 处理（引擎像素几乎不会正好踩灯心）；
//   5. 阈值 LIT_THRESH=0.05 是玩法参数：放灯(r4.5)约照亮自身周围 3×3 格。
//   遮板硬影与自遮挡规则（中心在遮板内不被它自己挡）与引擎逐字一致。
//   ⚠ 因此提灯人/美术装饰实体不要挂 Light——会造成「看着亮、判定暗」的错位。

"use strict";

// ---- 牌表（GDD 第1期合同定死） ----
const CARDS = {
  lamp:    { cost: 1, label: "LAMP" },   // 放灯：目标格放一盏灯 r=4.5 暖光
  move:    { cost: 1, label: "MOVE" },   // 移灯：离目标格最近的一盏己灯移过去（v1 简化：不二段选灯）
  snuff:   { cost: 0, label: "SNUFF" },  // 熄灯：收回目标格的灯，+1 油
  prism:   { cost: 2, label: "PRISM" },  // 棱镜：目标格的灯变 60° 聚光锥 r=7，朝向最近影怪（重打可转向）
  blocker: { cost: 1, label: "BLOCK" },  // 遮板：目标格立遮光板（Solid 2×2，尺寸表为准）
  flash:   { cost: 2, label: "FLASH" },  // 闪光：目标格瞬间强光 r=3，照到的影怪 -1 命并僵直，灯不留场
};
const DRAW_POOL = ["lamp", "move", "snuff", "prism", "blocker", "flash"]; // 无限牌库均匀抽
const HAND_SIZE = 4;

// ---- 棋盘（8×6 格，每格 2×2，左下格中心 (-7,-5)） ----
const GRID_W = 8, GRID_H = 6, CELL = 2;
const ORIGIN_X = -7, ORIGIN_Y = -5;

// ---- 光照参数 ----
const LIT_THRESH = 0.05;                // 亮格阈值（玩法参数，调难度动这里）
const LAMP_R = 4.5, LAMP_INT = 1.2, LAMP_COLOR = "#ffb060";
const LAMP_LUMA = (0xff + 0xb0 + 0x60) / (3 * 255); // 0.689，标量亮度近似
const SPOT_R = 7, SPOT_ANGLE = 60;
const FLASH_R = 3, FLASH_INT = 2.0;     // 闪光按白光 luma=1

// ---- 节奏参数（手感表） ----
const ACT_PERIOD = 24;   // 影怪行动间隔（tick，24 = 0.4s 一只）
const EXIT_DELAY = 90;   // 胜/负横幅停留（tick）后切场景
const SHAKE_BLOCK = 2;   // 遮板尺寸（2×2，尺寸表为准；牌面文案的 1×2 以尺寸表覆盖）

// ---- 纯函数工具 ----
function cellCenter(cx, cy) { return [ORIGIN_X + cx * CELL, ORIGIN_Y + cy * CELL]; }
function cellFromPoint(x, y) {
  const cx = Math.floor((x - (ORIGIN_X - CELL / 2)) / CELL);
  const cy = Math.floor((y - (ORIGIN_Y - CELL / 2)) / CELL);
  if (cx < 0 || cx >= GRID_W || cy < 0 || cy >= GRID_H) return null;
  return [cx, cy];
}
function inBounds(cx, cy) { return cx >= 0 && cx < GRID_W && cy >= 0 && cy < GRID_H; }
function manh(ax, ay, bx, by) { return Math.abs(ax - bx) + Math.abs(ay - by); }

// 线段 (x0,y0)-(x1,y1) 是否穿过 AABB（slab 法，端点在框内也算穿过）
function segHitsBox(x0, y0, x1, y1, bx, by, hw, hh) {
  const dx = x1 - x0, dy = y1 - y0;
  let tmin = 0, tmax = 1;
  for (const [p, d, lo, hi] of [[x0, dx, bx - hw, bx + hw], [y0, dy, by - hh, by + hh]]) {
    if (d === 0) {
      if (p < lo || p > hi) return false;
    } else {
      let t1 = (lo - p) / d, t2 = (hi - p) / d;
      if (t1 > t2) { const t = t1; t1 = t2; t2 = t; }
      tmin = Math.max(tmin, t1);
      tmax = Math.min(tmax, t2);
      if (tmin > tmax) return false;
    }
  }
  return true;
}
function pointInBox(x, y, bx, by, hw, hh) {
  return x >= bx - hw && x <= bx + hw && y >= by - hh && y <= by + hh;
}

// 单灯对点 (px,py) 的贡献（镜像引擎：灯色luma·intensity·(1-d/r)²，spot 再乘 t²，遮板硬影）
function lampContrib(lamp, px, py, blockers) {
  const lx = lamp.Position.x, ly = lamp.Position.y;
  const spot = lamp.Card.name === "spot";
  const r = spot ? SPOT_R : LAMP_R;
  const d = Math.hypot(px - lx, py - ly);
  if (d >= r) return 0;
  let c = LAMP_INT * LAMP_LUMA * (1 - d / r) * (1 - d / r);
  if (spot) {
    let t = 1; // d=0 时方向未定义，按锥心处理（已知偏差④）
    if (d > 0) {
      const dir = lamp.Card.slot; // 朝向（度，0=+x 逆时针，与引擎同约定）
      let dth = Math.abs((Math.atan2(py - ly, px - lx) * 180 / Math.PI - dir) % 360);
      if (dth > 180) dth = 360 - dth;
      t = Math.max(0, Math.min(1, 1 - dth / (SPOT_ANGLE / 2)));
    }
    c *= t * t;
  }
  // 遮板硬影：点到灯心的线段穿过任何遮板碰撞盒贡献清零；点在遮板内部时该遮板不挡它（自遮挡豁免，与引擎一致）
  for (const b of blockers) {
    const hw = SHAKE_BLOCK / 2, hh = SHAKE_BLOCK / 2;
    if (pointInBox(px, py, b.Position.x, b.Position.y, hw, hh)) continue;
    if (segHitsBox(px, py, lx, ly, b.Position.x, b.Position.y, hw, hh)) return 0;
  }
  return c;
}
function isLit(cx, cy, lamps, blockers) {
  const [px, py] = cellCenter(cx, cy);
  let total = 0;
  for (const l of lamps) total += lampContrib(l, px, py, blockers);
  return total > LIT_THRESH;
}

// ---- 战斗大脑 ----
vitric.system(
  "battle-brain",
  { query: ["Shade", "Cell", "Position", "Card"], writes: ["Shade", "Cell", "Position", "Card"] },
  (entities, ctx) => {
    // 1) 按 kind 分拣棋子
    let ctl = null, hero = null, tag = null;
    const shades = [], lamps = [], blockers = [];
    for (const e of entities) {
      const k = e.Shade.kind;
      if (k === "ctl") ctl = e;
      else if (k === "hero") hero = e;
      else if (k === "node") tag = e;
      else if (k === "lamp") lamps.push(e);
      else if (k === "blocker") blockers.push(e);
      else if (k === "stalker" || k === "lurker" || k === "devourer") shades.push(e);
    }
    if (!ctl || !hero) return; // 非战斗场景（menu/map/victory）：空转
    const node = tag ? tag.Shade.n : 0;
    const alive = () => shades.filter((s) => s.Shade.hp > 0);

    // ---- 工具（闭包仅在本 tick 内用，不跨 tick 存东西） ----
    const parseHand = () => {
      const parts = ctl.Card.name.split(",");
      while (parts.length < HAND_SIZE) parts.push("");
      return parts.slice(0, HAND_SIZE);
    };
    const writeHand = (hand) => { ctl.Card.name = hand.join(","); };
    const refill = (hand) => {
      for (let i = 0; i < HAND_SIZE; i++) {
        if (!hand[i]) hand[i] = DRAW_POOL[Math.floor(ctx.random() * DRAW_POOL.length)];
      }
      return hand;
    };
    const heroCell = () => [hero.Cell.cx, hero.Cell.cy];
    const occupiedBy = (cx, cy) => {
      const [hx, hy] = heroCell();
      if (cx === hx && cy === hy) return hero;
      for (const s of alive()) if (s.Cell.cx === cx && s.Cell.cy === cy) return s;
      for (const l of lamps) if (l.Cell.cx === cx && l.Cell.cy === cy) return l;
      for (const b of blockers) if (b.Cell.cx === cx && b.Cell.cy === cy) return b;
      return null;
    };
    const lampAt = (cx, cy) => lamps.find((l) => l.Cell.cx === cx && l.Cell.cy === cy) || null;

    const phaseName = ["player", "shade", "won", "lost"];
    const banner = () => ["", "SHADES MOVE", "VICTORY", "DEFEAT"][ctl.Shade.hp];
    const syncHud = () => ctx.emit("sync-hud", {
      oil: "OIL " + hero.Shade.n,
      hearts: "HP " + Math.max(0, hero.Shade.hp),
      turn: "TURN " + ctl.Shade.n,
      banner: banner(),
    });
    const syncBattle = () => ctx.emit("sync-battle", { phase: phaseName[ctl.Shade.hp], turn: ctl.Shade.n });
    const syncHand = () => {
      const hand = parseHand();
      const d = {};
      for (let i = 0; i < HAND_SIZE; i++) {
        const name = hand[i];
        const sel = ctl.Card.slot === i + 1;
        d["t" + (i + 1)] = name ? CARDS[name].label + " " + CARDS[name].cost : "";
        d["c" + (i + 1)] = !name ? "#2e2a3f"
          : sel ? "#9a7fb8"
          : CARDS[name].cost <= hero.Shade.n ? "#6b5d80" : "#453c52";
      }
      ctx.emit("sync-hand", d);
    };
    const puff = (x, y, colors, n, up) => {
      for (let i = 0; i < n; i++) {
        ctx.spawn({
          Position: { x: x + (ctx.random() - 0.5) * 1.2, y: y + (ctx.random() - 0.5) * 0.8 },
          Velocity: { x: (ctx.random() - 0.5) * 6, y: up * (0.4 + ctx.random() * 0.9) },
          Sprite: { w: 0.3, h: 0.3, color: colors[Math.floor(ctx.random() * colors.length)] },
          Particle: { ttl: 20 + Math.floor(ctx.random() * 10) },
        });
      }
    };
    const killShade = (s) => {
      const [x, y] = cellCenter(s.Cell.cx, s.Cell.cy);
      ctx.emit("shade-died", { n: s.Shade.n, x, y }); // 事件表名；x/y 为扩展字段（粒子规则用）
      ctx.despawn(s.id);
      s.Shade.hp = 0; // 本 tick 本地视图同步（alive() 过滤靠它）
    };
    const win = () => {
      ctl.Shade.hp = 2;
      ctl.Cell.cy = 0; // 退场倒计时
      ctx.emit("battle-won", { node });
      ctx.emit("node-cleared", { node });
      syncBattle(); syncHud();
    };
    const lose = () => {
      ctl.Shade.hp = 3;
      ctl.Cell.cy = 0;
      ctx.emit("battle-lost", {});
      syncBattle(); syncHud();
    };
    const hitHero = (dmg) => {
      hero.Shade.hp -= dmg;
      ctx.emit("player-hit", { hearts: Math.max(0, hero.Shade.hp) });
      syncHud();
      if (hero.Shade.hp <= 0) lose();
    };
    const moveShadeTo = (s, cx, cy) => {
      s.Cell.cx = cx; s.Cell.cy = cy;
      const [x, y] = cellCenter(cx, cy);
      s.Position.x = x; s.Position.y = y;
    };
    const adjacentToHero = (s) => manh(s.Cell.cx, s.Cell.cy, hero.Cell.cx, hero.Cell.cy) === 1;
    // 贪心走一步：四邻里选 暗格+界内+无人占+曼哈顿距离严格变小 的格（固定枚举序保证确定性）
    const bestStep = (s) => {
      const cur = manh(s.Cell.cx, s.Cell.cy, hero.Cell.cx, hero.Cell.cy);
      let best = null, bestD = cur;
      for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
        const nx = s.Cell.cx + dx, ny = s.Cell.cy + dy;
        if (!inBounds(nx, ny) || occupiedBy(nx, ny) || isLit(nx, ny, lamps, blockers)) continue;
        const d = manh(nx, ny, hero.Cell.cx, hero.Cell.cy);
        if (d < bestD) { bestD = d; best = [nx, ny]; }
      }
      return best;
    };

    // ---- 2) 开战哨兵：手牌寄存器为空 = 第一次进场，发起手 4 张 ----
    if (ctl.Card.name === "") {
      writeHand(refill(["", "", "", ""]));
      ctl.Cell.cy = 1;
      syncHand(); syncHud(); syncBattle();
      return;
    }

    // ---- 3) 胜负检测（每 tick，进行中阶段才查） ----
    if (ctl.Shade.hp <= 1) {
      if (alive().length === 0) { win(); return; }
      if (hero.Shade.hp <= 0) { lose(); return; }
    }

    // ---- 4) 胜/负阶段：横幅停留后发退场事件（计时器在组件里，读档恢复后照样会走到） ----
    if (ctl.Shade.hp >= 2) {
      ctl.Cell.cy += 1;
      if (ctl.Cell.cy === EXIT_DELAY) {
        if (ctl.Shade.hp === 2) ctx.emit("battle-exit", { node });
        else ctx.emit("battle-retreat", {});
      }
      return;
    }

    // ---- 5) 玩家阶段：消化指令寄存器 ----
    if (ctl.Shade.hp === 0) {
      const cmd = ctl.Cell.cx;
      if (cmd === 0) return;
      ctl.Cell.cx = 0;

      if (cmd >= 11 && cmd <= 14) { // 点牌：切换选中
        const slot = cmd - 10;
        ctl.Card.slot = ctl.Card.slot === slot ? 0 : slot;
        syncHand();
        return;
      }

      if (cmd === 2) { // E 结束回合 → 影怪阶段开场：亮格僵直 -1 命（回合开始判定）
        for (const s of alive()) {
          if (isLit(s.Cell.cx, s.Cell.cy, lamps, blockers)) {
            s.Shade.stunned = true;
            s.Shade.hp -= 1;
            const [x, y] = cellCenter(s.Cell.cx, s.Cell.cy);
            ctx.emit("shade-stunned", { n: s.Shade.n, x, y });
            puff(x, y, ["#fff2b0", "#ffd75e"], 6, 3);
            if (s.Shade.hp <= 0) killShade(s);
          }
        }
        ctl.Shade.hp = 1;
        ctl.Cell.cy = 0; // 下一个行动的影怪序
        syncBattle(); syncHud();
        return;
      }

      if (cmd === 1) { // 棋盘点击：有选牌就结算出牌
        const cell = cellFromPoint(ctl.Position.x, ctl.Position.y);
        if (!cell) return; // 点在棋盘外（含点牌时连带触发的坐标）：忽略，选中保留
        const sel = ctl.Card.slot;
        if (sel < 1) return; // 没选牌：忽略棋盘点击
        const hand = parseHand();
        const name = hand[sel - 1];
        if (!name) { ctl.Card.slot = 0; syncHand(); return; } // 选中了空槽：清选中
        const card = CARDS[name];
        if (card.cost > hero.Shade.n) { ctx.emit("card-rejected", { name, reason: "oil" }); return; }

        const [cx, cy] = cell;
        const [px, py] = cellCenter(cx, cy);
        const ok = (() => {
          switch (name) {
            case "lamp": {
              if (occupiedBy(cx, cy)) return false;
              const seq = lamps.reduce((m, l) => Math.max(m, l.Shade.n), 0) + 1;
              ctx.spawn({
                Shade: { kind: "lamp", hp: 0, n: seq, stunned: false },
                Lamp: {}, // 合同标记组件，和场景预置灯保持同一档案
                Cell: { cx, cy },
                Card: { name: "point", slot: 0 },
                Position: { x: px, y: py },
                Sprite: { w: 0.8, h: 1.4, color: "#ffd75e" },
                Light: { radius: LAMP_R, color: LAMP_COLOR, intensity: LAMP_INT, kind: "point" },
              });
              puff(px, py, ["#ffd75e", "#fff2b0"], 8, 4);
              return true;
            }
            case "move": {
              if (!lamps.length || occupiedBy(cx, cy)) return false;
              let pick = null, pd = Infinity; // 离目标格最近的灯（并列取 n 小，确定性）
              for (const l of lamps) {
                const d = Math.hypot(l.Position.x - px, l.Position.y - py);
                if (d < pd - 1e-9 || (Math.abs(d - pd) <= 1e-9 && pick && l.Shade.n < pick.Shade.n)) { pd = d; pick = l; }
              }
              pick.Cell.cx = cx; pick.Cell.cy = cy;
              pick.Position.x = px; pick.Position.y = py;
              return true;
            }
            case "snuff": {
              const l = lampAt(cx, cy);
              if (!l) return false;
              ctx.despawn(l.id);
              lamps.splice(lamps.indexOf(l), 1);
              hero.Shade.n += 1; // +1 油
              return true;
            }
            case "prism": {
              const l = lampAt(cx, cy);
              if (!l) return false;
              const liv = alive();
              let tgt = null, td = Infinity; // 朝向最近影怪（重打同一盏 = 转向）
              for (const s of liv) {
                const d = Math.hypot(s.Position.x - l.Position.x, s.Position.y - l.Position.y);
                if (d < td) { td = d; tgt = s; }
              }
              let deg = tgt ? Math.round(Math.atan2(tgt.Position.y - l.Position.y, tgt.Position.x - l.Position.x) * 180 / Math.PI) : 0;
              deg = ((deg % 360) + 360) % 360;
              l.Card.name = "spot";
              l.Card.slot = deg; // lamp-light-sync 系统据此改 Light
              return true;
            }
            case "blocker": {
              if (occupiedBy(cx, cy)) return false;
              ctx.spawn({
                Shade: { kind: "blocker", hp: 0, n: 0, stunned: false },
                Cell: { cx, cy },
                Card: { name: "", slot: 0 },
                Position: { x: px, y: py },
                Sprite: { w: SHAKE_BLOCK, h: SHAKE_BLOCK, color: "#3a3142" },
                Solid: {},
                Collider: { w: SHAKE_BLOCK, h: SHAKE_BLOCK },
              });
              return true;
            }
            case "flash": {
              // 瞬间强光：照得到（白光 r=3 公式，遮板有效）的影怪 -1 命并僵直
              for (const s of alive()) {
                const [sx, sy] = cellCenter(s.Cell.cx, s.Cell.cy);
                const d = Math.hypot(sx - px, sy - py);
                if (d >= FLASH_R) continue;
                let c = FLASH_INT * (1 - d / FLASH_R) * (1 - d / FLASH_R);
                let blocked = false;
                for (const b of blockers) {
                  if (pointInBox(sx, sy, b.Position.x, b.Position.y, 1, 1)) continue;
                  if (segHitsBox(sx, sy, px, py, b.Position.x, b.Position.y, 1, 1)) { blocked = true; break; }
                }
                if (blocked || c <= LIT_THRESH) continue;
                s.Shade.stunned = true;
                s.Shade.hp -= 1;
                ctx.emit("shade-stunned", { n: s.Shade.n, x: sx, y: sy });
                if (s.Shade.hp <= 0) killShade(s);
              }
              ctx.spawn({ // 纯视觉残光，灯不留场
                Position: { x: px, y: py },
                Sprite: { w: 0.5, h: 0.5, color: "#fff6dc" },
                Light: { radius: FLASH_R, color: "#fff6dc", intensity: FLASH_INT, kind: "point" },
                Particle: { ttl: 14 },
              });
              return true;
            }
          }
          return false;
        })();

        if (!ok) { ctx.emit("card-rejected", { name, reason: "target" }); return; }
        hero.Shade.n -= card.cost;
        hand[sel - 1] = "";
        ctl.Card.slot = 0;
        writeHand(hand);
        ctx.emit("card-played", { name, cell: cx + "," + cy }); // 事件表：card-played{name,cell}
        syncHand(); syncHud();
        return;
      }
      return;
    }

    // ---- 6) 影怪阶段：每 ACT_PERIOD tick 行动一只（按 Shade.n 升序） ----
    if (ctl.Shade.hp === 1) {
      if (ctx.tick % ACT_PERIOD !== 0) return;
      const order = alive().sort((a, b) => a.Shade.n - b.Shade.n);
      const idx = ctl.Cell.cy;
      if (idx >= order.length) { // 全部行动完 → 新玩家回合：回 3 油、补牌至 4
        ctl.Shade.n += 1;
        hero.Shade.n = 3;
        writeHand(refill(parseHand()));
        ctl.Shade.hp = 0;
        ctl.Cell.cy = 1;
        syncHand(); syncHud(); syncBattle();
        return;
      }
      const s = order[idx];
      ctl.Cell.cy = idx + 1;

      if (s.Shade.stunned) { // 僵直：跳过本回合行动
        s.Shade.stunned = false;
      } else if (s.Shade.kind === "stalker") {
        // 潜行者：沿暗格向玩家走 2 格；已相邻则直接攻击（『进入相邻格攻击』的退化情形，v1 约定：相邻即每回合咬一口）
        if (adjacentToHero(s)) hitHero(1);
        else {
          for (let step = 0; step < 2; step++) {
            const m = bestStep(s);
            if (!m) break;
            moveShadeTo(s, m[0], m[1]);
            if (adjacentToHero(s)) { hitHero(1); break; }
          }
        }
      } else if (s.Shade.kind === "lurker") {
        // 潜伏者：不动；身处暗格且与玩家相邻 → -2 心
        if (!isLit(s.Cell.cx, s.Cell.cy, lamps, blockers) && adjacentToHero(s)) hitHero(2);
      } else if (s.Shade.kind === "devourer") {
        // 吞灯者（Boss）：每 2 回合吞最近一盏灯，走 1 格；僵直时不吞（上面 stunned 分支已拦）
        if (ctl.Shade.n % 2 === 0 && lamps.length) {
          let pick = null, pd = Infinity;
          for (const l of lamps) {
            const d = Math.hypot(l.Position.x - s.Position.x, l.Position.y - s.Position.y);
            if (d < pd - 1e-9 || (Math.abs(d - pd) <= 1e-9 && pick && l.Shade.n < pick.Shade.n)) { pd = d; pick = l; }
          }
          ctx.emit("lamp-devoured", { x: pick.Position.x, y: pick.Position.y });
          puff(pick.Position.x, pick.Position.y, ["#5a4a6a", "#3a3142"], 10, 3);
          ctx.despawn(pick.id);
          lamps.splice(lamps.indexOf(pick), 1);
        }
        if (adjacentToHero(s)) hitHero(1);
        else {
          const m = bestStep(s);
          if (m) {
            moveShadeTo(s, m[0], m[1]);
            if (adjacentToHero(s)) hitHero(1);
          }
        }
      }
      const [ax, ay] = cellCenter(s.Cell.cx, s.Cell.cy);
      ctx.emit("shade-acted", { n: s.Shade.n, x: ax, y: ay }); // 事件表：shade-acted{n}；x/y 扩展字段
      return;
    }
  }
);

// ---- 灯具光学同步：Light 组件永远从 Card 寄存的灯形态推导（幂等，每 tick 覆写） ----
// 棱镜把 Card.name 改成 "spot"、Card.slot 写朝向后，这里负责把引擎的 Light 跟上。
vitric.system("lamp-light-sync", { query: ["Shade", "Card", "Light"], writes: ["Light"] }, (entities) => {
  for (const e of entities) {
    if (e.Shade.kind !== "lamp") continue;
    if (e.Card.name === "spot") {
      e.Light.kind = "spot";
      e.Light.radius = SPOT_R;
      e.Light.angle = SPOT_ANGLE;
      e.Light.dir = e.Card.slot;
    } else {
      e.Light.kind = "point";
      e.Light.radius = LAMP_R;
    }
    e.Light.color = LAMP_COLOR;
    e.Light.intensity = LAMP_INT;
  }
});

// ---- 提灯人镜像：Player{hearts,oil} 是合同组件，从 Shade 真值单向同步（QA 断言友好） ----
vitric.system("hero-player-sync", { query: ["Shade", "Player"], writes: ["Player"] }, (entities) => {
  for (const e of entities) {
    if (e.Shade.kind !== "hero") continue;
    e.Player.hearts = Math.max(0, e.Shade.hp);
    e.Player.oil = Math.max(0, e.Shade.n);
  }
});

// ---- 通关彩带（battle-won 规则 call） ----
vitric.fn("burst", (args, ctx) => {
  const colors = ["#ffd75e", "#ff9a3c", "#fff2b0", "#9a7fb8"];
  for (let i = 0; i < args.n; i++) {
    ctx.spawn({
      Position: { x: args.x + (ctx.random() - 0.5) * 6, y: args.y + (ctx.random() - 0.3) * 2 },
      Velocity: { x: (ctx.random() - 0.5) * 10, y: 8 * (0.4 + ctx.random() * 0.9) },
      Sprite: { w: 0.35, h: 0.35, color: colors[Math.floor(ctx.random() * colors.length)] },
      Particle: { ttl: 45 + Math.floor(ctx.random() * 12) },
    });
  }
});
