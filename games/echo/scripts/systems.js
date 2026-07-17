// echo — combat brain (gameplay layer)
//
// Architecture (script does the math + rules apply + command register; the path validated in spire):
// - Input rules only write register fields on the ctl entity (command / click coords / chosen card slot); no logic.
// - The battle-brain system reads the registers each tick to do the math: card settlement, shade AI, lighting checks, win/loss.
//   It writes back to components inside its own query (Shade/Cell/Position/Card); writes outside the query
//   always go via emit sync-* events (payloads are all absolute values, idempotent), which rules apply after receiving them.
// - Scripts keep zero hidden state: all cross-tick state lives in components, snapshot/replay safe.
//
// [Piece profile = Shade component] All logical entities in combat (lantern-bearer / shades / lamps / blockers / registers)
// share one component profile {Shade, Cell, Position, Card}; Shade.kind distinguishes identity:
//   "hero"     lantern-bearer: Shade.hp=hearts Shade.n=lamp oil (Player component is a read-only mirror, see hero-player-sync)
//   "stalker" / "lurker" / "devourer"  shades (three entries in the monster table): hp=HP n=action order stunned=stun
//   "lamp"     lamp: Card.name="point"|"spot" (prismed) Card.slot=spotlight direction (degrees)
//   "blocker"  blocker (also has Solid+Collider; blocking light and routing is handled by the engine)
//   "ctl"      command register (one per battle scene; see the register table below)
//   "fx"       feedback timer register (one per battle scene): Cell.cx=red-flash countdown when oil is insufficient (ticks)
//              Cell.cy=SAVED save-toast countdown (ticks); the brain decrements every tick and emits a restore event at 0
//   "node"     node tag: Shade.n = node number of this battle (1/2/3)
//   "tutor"    tutorial register (only battle-1): Card.slot=reached tutorial step (0..5, only increases, advanced by rules)
//              Cell.cx=displayed step (the brain reconciles every tick; if unequal it emits sync-hint to refresh the HUD; scene initial -1=force first emit)
// This is a requirement of the query mechanism: script systems match by component AND, so only entities sharing a profile can see each other in one system.
// ⚠ Therefore "Shade component != shade"; counting shades must filter by kind — QA assertions must not count Shade entities directly.
//
// [ctl register table] (a battle scene must contain an entity named ctl)
//   Shade.hp   = phase: 0 player turn / 1 shade turn / 2 win / 3 loss
//   Shade.n    = turn count (starts at 1)
//   Shade.stunned = pause flag (true = paused: the whole brain freezes, only command 3 releases it)
//   Card.name  = hand register: comma-separated string of 4 slots, e.g. "lamp,flash,," ; "" = not yet dealt (battle-start sentinel)
//   Card.slot  = currently selected hand slot (0=none, 1..4)
//   Cell.cx    = command: 0 idle / 1 board click / 2 end turn / 3 pause toggle (ESC/R)
//                / 11..14 select the Nth card
//                / 20 right-click empty cell (restore default hint) / 21..24 right-click Nth card (HUD shows that card's description)
//   Cell.cy    = intra-phase progress: shade turn = next acting shade index, win/loss phase = exit countdown
//   Position   = world coords of the most recent click (used for board clicks)
//
// [Lighting check = mirrors the engine formula] isLit() computes cell-center illumination in the script using the same formula as the engine,
// with the known discrepancies vs the engine's rendering (v1; tuning/debugging starts here):
//   1. Only the cell center is sampled (the engine goes pixel-by-pixel);
//   2. Lamp color is approximated by scalar luminance: luma(#ffb060)=0.689 multiplied into the contribution (the engine works channel-by-channel);
//   3. Ambient light is not counted (GDD: "lit by any lamp" only considers lamp contributions);
//   4. When the cell center coincides with the lamp center, spotlight angular attenuation is treated as 1 (engine pixels almost never land exactly on the lamp center);
//   5. The threshold LIT_THRESH=0.05 is a gameplay parameter: placing a lamp (r4.5) lights up about a 3×3 cell area around itself.
//   Blocker hard shadows and self-occlusion rules (a center inside a blocker is not blocked by that blocker itself) match the engine verbatim.
//   ⚠ Therefore the lantern-bearer / art decoration entities must not carry Light — it would cause a "looks bright, judged dark" mismatch.

"use strict";

// ---- Card table (locked by the GDD phase-1 contract; help = right-click card description line; DejaVu has no CJK so it's all ASCII) ----
const CARDS = {
  lamp:    { cost: 1, label: "LAMP", help: "LAMP 1 OIL - LIGHT A CELL" },        // Place lamp: drop a lamp on the target cell, r=4.5 warm light
  move:    { cost: 1, label: "MOVE", help: "MOVE 1 OIL - MOVE NEAREST LAMP" },   // Move lamp: move the nearest friendly lamp to the target cell (v1 simplification: no two-stage lamp selection)
  snuff:   { cost: 0, label: "SNUFF", help: "SNUFF 0 OIL - REMOVE LAMP +1 OIL" },// Snuff lamp: remove the lamp on the target cell, +1 oil
  prism:   { cost: 2, label: "PRISM", help: "PRISM 2 OIL - FOCUS LAMP AT FOE" }, // Prism: the lamp on the target cell becomes a 60° spotlight cone r=7 facing the nearest shade (re-casting rotates it)
  blocker: { cost: 1, label: "BLOCK", help: "BLOCK 1 OIL - WALL STOPS LIGHT" },  // Blocker: place a blocker on the target cell (Solid 2×2; the size table is authoritative)
  flash:   { cost: 2, label: "FLASH", help: "FLASH 2 OIL - STUN SHADES NEAR" },  // Flash: instant intense light r=3 on the target cell; shades hit lose 1 HP and get stunned; the lamp does not stay on the field
};

// ---- Tutorial text (battle-1 guide steps; step number = tutor.Card.slot, only increased by rules) ----
const HINT_DEFAULT = "E - END TURN";
const TUT_TEXT = [
  "CLICK A CARD",                          // 0 opening
  "CLICK A CELL TO PLAY IT",               // 1 a card was selected
  "SHADES FEAR LIGHT - TRAP THEM",         // 2 first card played
  "PRESS E TO END TURN",                   // 3 oil exhausted but turn not ended (turn-state-driven, not wall-clock)
  "STUNNED SHADES LOSE HP - END THEM",     // 4 first stun (≤33 chars: hud-hint is centered on x=10.6; longer overflows the right frame)
  HINT_DEFAULT,                            // 5 first kill; graduate back to the standing hint
];
const DRAW_POOL = ["lamp", "move", "snuff", "prism", "blocker", "flash"]; // uniform draw from an infinite deck
const HAND_SIZE = 4;

// ---- Board (8×6 cells, each 2×2, lower-left cell center at (-7,-5)) ----
const GRID_W = 8, GRID_H = 6, CELL = 2;
const ORIGIN_X = -7, ORIGIN_Y = -5;

// ---- Lighting parameters ----
const LIT_THRESH = 0.05;                // lit-cell threshold (gameplay parameter; tune difficulty here)
const LAMP_R = 4.5, LAMP_INT = 1.2, LAMP_COLOR = "#ffb060";
const LAMP_LUMA = (0xff + 0xb0 + 0x60) / (3 * 255); // 0.689, scalar luminance approximation
const SPOT_R = 7, SPOT_ANGLE = 60;
const FLASH_R = 3, FLASH_INT = 2.0;     // flash treated as white light, luma=1

// ---- Pacing parameters (game-feel table) ----
const ACT_PERIOD = 24;   // shade action interval (ticks; 24 = one shade every 0.4s)
const EXIT_DELAY = 90;   // win/loss banner dwell time (ticks) before switching scenes
const SHAKE_BLOCK = 2;   // blocker size (2×2; the size table is authoritative and overrides the 1×2 on the card face)
const OIL_FLASH_TICKS = 18; // OIL text red-flash duration on an invalid play (0.3s)
const TOAST_TICKS = 90;     // SAVED save-toast dwell time (1.5s)

// ---- Pause text (shipping checklist: pause/resume/exit-to-menu; the engine has no rule-reachable exit-process hook, so we honestly tell the player to close the window) ----
// ESC=pause toggle R=resume M=back to main menu (only effective while paused). M does not drop global progress: progress is on
// Persist and follows to the menu (the menu scene no longer pre-places progress; on startup it's spawned by the start rule, and on victory
// it's reset by the victory-back-to-menu field) — only the current battle is lost; clicking START again resumes the node from the map.
const PAUSE_BANNER = "PAUSED - [R]ESUME [M]ENU";
const PAUSE_HINT = "CLOSE WINDOW TO QUIT";
const OIL_COLOR = "#ffe9a8";      // hud-oil default color (matches the scene initial)
const OIL_FLASH_COLOR = "#ff6055"; // red-flash color

// ---- Pure function utilities ----
function cellCenter(cx, cy) { return [ORIGIN_X + cx * CELL, ORIGIN_Y + cy * CELL]; }
function cellFromPoint(x, y) {
  const cx = Math.floor((x - (ORIGIN_X - CELL / 2)) / CELL);
  const cy = Math.floor((y - (ORIGIN_Y - CELL / 2)) / CELL);
  if (cx < 0 || cx >= GRID_W || cy < 0 || cy >= GRID_H) return null;
  return [cx, cy];
}
function inBounds(cx, cy) { return cx >= 0 && cx < GRID_W && cy >= 0 && cy < GRID_H; }
function manh(ax, ay, bx, by) { return Math.abs(ax - bx) + Math.abs(ay - by); }

// Does segment (x0,y0)-(x1,y1) cross an AABB (slab method; endpoints inside the box also count as crossing)
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

// Contribution of a single lamp to point (px,py) (mirrors the engine: lamp color luma · intensity · (1-d/r)², spotlight multiplies by t², blocker hard shadow)
function lampContrib(lamp, px, py, blockers) {
  const lx = lamp.Position.x, ly = lamp.Position.y;
  const spot = lamp.Card.name === "spot";
  const r = spot ? SPOT_R : LAMP_R;
  const d = Math.hypot(px - lx, py - ly);
  if (d >= r) return 0;
  let c = LAMP_INT * LAMP_LUMA * (1 - d / r) * (1 - d / r);
  if (spot) {
    let t = 1; // d=0 direction undefined, treat as cone center (known discrepancy ④)
    if (d > 0) {
      const dir = lamp.Card.slot; // direction (degrees; 0=+x counterclockwise, same convention as the engine)
      let dth = Math.abs((Math.atan2(py - ly, px - lx) * 180 / Math.PI - dir) % 360);
      if (dth > 180) dth = 360 - dth;
      t = Math.max(0, Math.min(1, 1 - dth / (SPOT_ANGLE / 2)));
    }
    c *= t * t;
  }
  // Blocker hard shadow: if the segment from the point to the lamp center crosses any blocker collider, the contribution is zeroed; if the point is inside a blocker, that blocker doesn't block it (self-occlusion exemption, matching the engine)
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

// ---- Combat brain ----
vitric.system(
  "battle-brain",
  { query: ["Shade", "Cell", "Position", "Card"], writes: ["Shade", "Cell", "Position", "Card"] },
  (entities, ctx) => {
    // 1) Sort pieces by kind
    let ctl = null, hero = null, tag = null, tut = null, fx = null;
    const shades = [], lamps = [], blockers = [];
    for (const e of entities) {
      const k = e.Shade.kind;
      if (k === "ctl") ctl = e;
      else if (k === "hero") hero = e;
      else if (k === "node") tag = e;
      else if (k === "tutor") tut = e;
      else if (k === "fx") fx = e;
      else if (k === "lamp") lamps.push(e);
      else if (k === "blocker") blockers.push(e);
      else if (k === "stalker" || k === "lurker" || k === "devourer") shades.push(e);
    }
    if (!ctl || !hero) return; // non-battle scene (menu/map/victory): no-op
    const node = tag ? tag.Shade.n : 0;
    const alive = () => shades.filter((s) => s.Shade.hp > 0);

    // ---- Tutorial hint reconciliation (only battle-1 has tutor): if the actual step (Card.slot, advanced by rules) != the displayed step (Cell.cx)
    // refresh the HUD once. Register pattern: idempotent, zero hidden state, replay safe.
    if (tut && tut.Cell.cx !== tut.Card.slot) {
      tut.Cell.cx = tut.Card.slot;
      ctx.emit("sync-hint", { text: TUT_TEXT[tut.Card.slot] || HINT_DEFAULT });
    }

    // ---- Utilities (closures are used only within this tick; nothing is stored across ticks) ----
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
      ctx.emit("shade-died", { n: s.Shade.n, x, y }); // event-table name; x/y are extension fields (used by the particle rule)
      ctx.despawn(s.id);
      s.Shade.hp = 0; // local-view sync within this tick (alive() filters on this)
    };
    const win = () => {
      ctl.Shade.hp = 2;
      ctl.Cell.cy = 0; // exit countdown
      ctx.emit("battle-won", { node });
      ctx.emit("node-cleared", { node }); // → the autosave-on-clear rule writes to disk
      if (fx) { fx.Cell.cy = TOAST_TICKS; ctx.emit("sync-toast", { text: "SAVED" }); } // make the save explicit
      syncBattle(); syncHud();
    };
    const lose = () => {
      ctl.Shade.hp = 3;
      ctl.Cell.cy = 0;
      ctx.emit("battle-lost", {});
      syncBattle(); syncHud();
    };
    // Unified exit for invalid ops: reject sound (rule card-rejected-sound) + OIL text red-flash (timer in the fx register)
    const reject = (name, reason) => {
      ctx.emit("card-rejected", { name, reason });
      if (fx) { fx.Cell.cx = OIL_FLASH_TICKS; ctx.emit("sync-oil-flash", { color: OIL_FLASH_COLOR }); }
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
    // Greedy single step: among the 4 neighbors, pick a dark + in-bounds + unoccupied + strictly smaller Manhattan-distance cell (fixed enumeration order guarantees determinism)
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

    // ---- 1.5) Pause (command 3, available in any phase): toggle to freeze/unfreeze the whole brain ----
    // Pause flag lives in ctl.Shade.stunned (in a component, snapshot/replay safe). Frozen = shade actions,
    // turn processing, win/loss countdown, feedback timers all stop; on resume the banner/hint are recomputed from the phase truth (idempotent).
    if (ctl.Cell.cx === 3) {
      ctl.Cell.cx = 0;
      ctl.Shade.stunned = !ctl.Shade.stunned;
      ctx.emit("sync-pause", ctl.Shade.stunned
        ? { banner: PAUSE_BANNER, hint: PAUSE_HINT }
        : { banner: banner(), hint: tut ? (TUT_TEXT[tut.Card.slot] || HINT_DEFAULT) : HINT_DEFAULT });
    }
    if (ctl.Shade.stunned) return; // pause freeze

    // ---- 1.6) Feedback timer register decrement (red-flash / SAVED toast; emit a restore event at 0) ----
    if (fx) {
      if (fx.Cell.cx > 0) {
        fx.Cell.cx -= 1;
        if (fx.Cell.cx === 0) ctx.emit("sync-oil-flash", { color: OIL_COLOR });
      }
      if (fx.Cell.cy > 0) {
        fx.Cell.cy -= 1;
        if (fx.Cell.cy === 0) ctx.emit("sync-toast", { text: "" });
      }
    }

    // ---- 2) Battle-start sentinel: hand register empty = first entry; deal a hand of 4 ----
    if (ctl.Card.name === "") {
      writeHand(refill(["", "", "", ""]));
      ctl.Cell.cy = 1;
      syncHand(); syncHud(); syncBattle();
      return;
    }

    // ---- 3) Win/loss check (every tick; only checked in in-progress phases) ----
    if (ctl.Shade.hp <= 1) {
      if (alive().length === 0) { win(); return; }
      if (hero.Shade.hp <= 0) { lose(); return; }
    }

    // ---- 4) Win/loss phase: after the banner dwells, emit the exit event (timer in a component; on save load it'll still reach this) ----
    if (ctl.Shade.hp >= 2) {
      ctl.Cell.cy += 1;
      if (ctl.Cell.cy === EXIT_DELAY) {
        if (ctl.Shade.hp === 2) ctx.emit("battle-exit", { node });
        else ctx.emit("battle-retreat", {});
      }
      return;
    }

    // ---- 5) Player phase: consume the command register ----
    if (ctl.Shade.hp === 0) {
      const cmd = ctl.Cell.cx;
      if (cmd === 0) return;
      ctl.Cell.cx = 0;

      if (cmd >= 11 && cmd <= 14) { // Click card: toggle selection; clicking an empty slot = invalid op, give feedback
        const slot = cmd - 10;
        if (!parseHand()[slot - 1]) { reject("", "empty"); return; }
        ctl.Card.slot = ctl.Card.slot === slot ? 0 : slot;
        syncHand();
        return;
      }

      if (cmd >= 20 && cmd <= 24) { // Right-click card description: 21..24 show the cost/effect of the Nth card; 20/empty slot restores the default
        let text = tut ? (TUT_TEXT[tut.Card.slot] || HINT_DEFAULT) : HINT_DEFAULT; // default = current tutorial step (battle-1) or the standing hint
        if (cmd >= 21) {
          const name = parseHand()[cmd - 21];
          if (name) text = CARDS[name].help;
        }
        ctx.emit("sync-hint", { text });
        return;
      }

      if (cmd === 2) { // E to end turn → shade-phase opener: lit-cell stun loses 1 HP (checked at turn start)
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
        ctl.Cell.cy = 0; // next acting shade index
        syncBattle(); syncHud();
        return;
      }

      if (cmd === 1) { // Board click: if a card is selected, settle the play
        const cell = cellFromPoint(ctl.Position.x, ctl.Position.y);
        if (!cell) return; // click outside the board (including coords triggered alongside a card click): ignore, keep selection
        const sel = ctl.Card.slot;
        if (sel < 1) return; // no card selected: ignore board clicks
        const hand = parseHand();
        const name = hand[sel - 1];
        if (!name) { ctl.Card.slot = 0; syncHand(); return; } // selected an empty slot: clear selection
        const card = CARDS[name];
        if (card.cost > hero.Shade.n) { reject(name, "oil"); return; }

        const [cx, cy] = cell;
        const [px, py] = cellCenter(cx, cy);
        const ok = (() => {
          switch (name) {
            case "lamp": {
              if (occupiedBy(cx, cy)) return false;
              const seq = lamps.reduce((m, l) => Math.max(m, l.Shade.n), 0) + 1;
              ctx.spawn({
                Shade: { kind: "lamp", hp: 0, n: seq, stunned: false },
                Lamp: {}, // contract marker component; keeps the same profile as scene-preset lamps
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
              let pick = null, pd = Infinity; // nearest lamp to the target cell (ties broken by smaller n, deterministic)
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
              hero.Shade.n += 1; // +1 oil
              return true;
            }
            case "prism": {
              const l = lampAt(cx, cy);
              if (!l) return false;
              const liv = alive();
              let tgt = null, td = Infinity; // face the nearest shade (re-casting the same lamp = rotate)
              for (const s of liv) {
                const d = Math.hypot(s.Position.x - l.Position.x, s.Position.y - l.Position.y);
                if (d < td) { td = d; tgt = s; }
              }
              let deg = tgt ? Math.round(Math.atan2(tgt.Position.y - l.Position.y, tgt.Position.x - l.Position.x) * 180 / Math.PI) : 0;
              deg = ((deg % 360) + 360) % 360;
              l.Card.name = "spot";
              l.Card.slot = deg; // the lamp-light-sync system uses this to update Light
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
              // Instant intense light: shades hit by it (white r=3 formula; blockers apply) lose 1 HP and get stunned
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
              ctx.spawn({ // pure visual afterglow; the lamp doesn't stay on the field
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

        if (!ok) { reject(name, "target"); return; }
        hero.Shade.n -= card.cost;
        hand[sel - 1] = "";
        ctl.Card.slot = 0;
        writeHand(hand);
        ctx.emit("card-played", { name, cell: cx + "," + cy }); // event table: card-played{name,cell}
        syncHand(); syncHud();
        return;
      }
      return;
    }

    // ---- 6) Shade phase: one shade acts every ACT_PERIOD ticks (sorted by Shade.n ascending) ----
    if (ctl.Shade.hp === 1) {
      if (ctx.tick % ACT_PERIOD !== 0) return;
      const order = alive().sort((a, b) => a.Shade.n - b.Shade.n);
      const idx = ctl.Cell.cy;
      if (idx >= order.length) { // all shades acted → new player turn: restore 3 oil, refill hand up to 4
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

      if (s.Shade.stunned) { // stun: skip this turn's action
        s.Shade.stunned = false;
      } else if (s.Shade.kind === "stalker") {
        // Stalker: walk 2 cells along dark cells toward the player; if already adjacent, attack directly (degenerate case of "move into adjacent cell and attack"; v1 convention: adjacency = one bite per turn)
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
        // Lurker: doesn't move; if in a dark cell and adjacent to the player → -2 hearts
        if (!isLit(s.Cell.cx, s.Cell.cy, lamps, blockers) && adjacentToHero(s)) hitHero(2);
      } else if (s.Shade.kind === "devourer") {
        // Devourer (Boss): every 2 turns devours the nearest lamp, walks 1 cell; doesn't devour while stunned (the stunned branch above already handles it)
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
      ctx.emit("shade-acted", { n: s.Shade.n, x: ax, y: ay }); // event table: shade-acted{n}; x/y are extension fields
      return;
    }
  }
);

// ---- Lamp optics sync: the Light component is always derived from the lamp form registered in Card (idempotent; overwritten every tick) ----
// After prism changes Card.name to "spot" and writes Card.slot for direction, this system keeps the engine's Light in sync.
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

// ---- Lantern-bearer mirror: Player{hearts,oil} is the contract component, one-way synced from the Shade truth (QA-friendly) ----
vitric.system("hero-player-sync", { query: ["Shade", "Player"], writes: ["Player"] }, (entities) => {
  for (const e of entities) {
    if (e.Shade.kind !== "hero") continue;
    e.Player.hearts = Math.max(0, e.Shade.hp);
    e.Player.oil = Math.max(0, e.Shade.n);
  }
});

// ---- Victory confetti (called by the battle-won rule) ----
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
