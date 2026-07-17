// spire combat brain — the script does the math, rules write the results.
//
// Data flow (one round trip per command per tick):
//   Input rules pass the current world state into fns via @path args (play-card / end-turn / ...).
//   Each fn is a pure function that computes the full settlement result and emits a batch of sync-* events (payloads are all absolute values, idempotent);
//   the apply-* rules on the next tick write these values back to components.
//   @battle.Card.slot acts as a command register / interlock: input rules set it (non-0 = processing, refuse new input),
//   and every fn emits sync-battle{slot:0} at the end to reset it.
// The enemy turn is an event chain: end-turn → enemy-act-req{n:0} → enemy-act-req{n:1} → player-turn-req,
// one tick per hop; sync events are always emitted before chain events so the next hop always reads the freshest world.
// Scripts keep zero hidden state: all cross-tick state lives in components, snapshot/replay safe.

// ---- Card table (locked by GDD) ----
const CARDS = {
  "STRIKE":    { cost: 1, dmg: 6 },
  "DEFEND":    { cost: 1, blk: 5 },
  "BASH":      { cost: 2, dmg: 8, vuln: 2 },
  "CLEAVE":    { cost: 1, dmg: 4, aoe: true },
  "IRON WAVE": { cost: 2, dmg: 5, blk: 5 },
};
// Deck of 12: STRIKE×5 DEFEND×4 BASH×1 CLEAVE×1 IRON WAVE×1
const DECKLIST = [
  "STRIKE", "STRIKE", "STRIKE", "STRIKE", "STRIKE",
  "DEFEND", "DEFEND", "DEFEND", "DEFEND",
  "BASH", "CLEAVE", "IRON WAVE",
];

// ---- Pure function utilities ----
function shuffle(arr, ctx) { // Fisher–Yates; ctx.random keeps it deterministic
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(ctx.random() * (i + 1));
    const t = arr[i]; arr[i] = arr[j]; arr[j] = t;
  }
  return arr;
}

function drawCards(draw, discard, hand, n, ctx) {
  for (let k = 0; k < n && hand.length < 5; k++) {
    if (draw.length === 0) {
      if (discard.length === 0) break; // all cards in hand/field, nothing to draw
      const pile = discard.splice(0, discard.length);
      shuffle(pile, ctx);
      for (const c of pile) draw.push(c);
    }
    hand.push(draw.shift());
  }
}

// Enemy intent (GDD: enemy A slasher is aggressive; enemy B guard splits attack/defense)
// Tuning 2026-06-11: guard DEF 50%→40%; from turn 9 the guard enrages, ATK +2 per turn (tightens the turtle-shell pace; defense stalls no longer drag to turn 27)
function rollIntent(n, turn, ctx) {
  if (n === 0) return ctx.random() < 0.65 ? { kind: "ATK", value: 8 } : { kind: "ATK", value: 11 };
  const atk = 6 + Math.max(0, turn - 8) * 2; // enrage clock: intent value is shown as-is so the player sees the pressure ramp up
  return ctx.random() < 0.4 ? { kind: "DEF", value: 6 } : { kind: "ATK", value: atk };
}

// Attack settlement: vulnerable ×1.5 floored, block breaks before HP loss; emit blocked / damaged (GDD event table)
function attack(ctx, targetName, u, base) {
  const eff = u.vuln > 0 ? Math.floor(base * 1.5) : base;
  const absorbed = Math.min(u.block, eff);
  const hpLoss = eff - absorbed;
  u.block -= absorbed;
  u.hp = Math.max(0, u.hp - hpLoss);
  if (absorbed > 0) ctx.emit("blocked", { target: targetName, amount: absorbed });
  if (hpLoss > 0) ctx.emit("damaged", { target: targetName, amount: hpLoss });
}

// ---- Display text (fn builds the strings directly; rules just ferry them) ----
function unitText(u) {
  if (u.hp <= 0) return "DEAD";
  let s = "HP " + u.hp + "/" + u.maxhp;
  if (u.block > 0) s += " BLK " + u.block;
  if (u.vuln > 0) s += " VULN " + u.vuln;
  return s;
}

// ---- sync events (all absolute values; replay/repeat application idempotent) ----
function emitBattle(ctx, phase, energy, turn) {
  ctx.emit("sync-battle", {
    phase, energy, turn, slot: 0, // slot:0 = reset command register
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
    d["n" + (i + 1)] = name;                                  // Card.name (raw card name)
    d["s" + (i + 1)] = name;                                  // slot-N-text card-name label
    d["o" + (i + 1)] = name ? String(CARDS[name].cost) : ""; // slot-N-cost cost label
    // Card face coloring (scene empty-slot base color #3a3442): highlight when a card is present, dim when unaffordable
    d["c" + (i + 1)] = !name ? "#3a3442" : (CARDS[name].cost <= energy ? "#6b5d80" : "#4a4255");
  }
  ctx.emit("hand-sync", d);
}

// ---- Battle entry: triggered by the start event; shuffle, opening draw of 5, set initial intents ----
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

// ---- Play card: triggered by 1-5; auto-targets the first living enemy (CLEAVE hits all) ----
vitric.fn("play-card", (a, ctx) => {
  const hand = a.hand.slice();
  const name = hand[a.slot - 1];
  if (!name) {
    ctx.emit("card-rejected", { slot: a.slot, reason: "empty" });
    emitBattle(ctx, "player", a.energy, a.turn); // only to reset the command register
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

// ---- End turn: hand goes to discard pile, enemy old block expires, start enemy action chain ----
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
  ctx.emit("enemy-act-req", { n: 0 }); // chain events are always emitted last so the next hop reads the freshest state
});

// ---- Single enemy acts on its intent (one chain hop per tick) ----
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
    return; // chain ends here
  }
  emitBattle(ctx, "enemy", 0, a.turn);
  if (a.n === 0) ctx.emit("enemy-act-req", { n: 1 });
  else ctx.emit("player-turn-req", {});
});

// ---- New turn: vulnerable decays, player block expires, new intents, energy refills, draw up to 5 ----
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

// ---- Impact feel: particle burst (lifetime is reaped by the engine Particle system) ----
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

// ---- Torch flicker: intensity is a pure function of (tick, entity id); zero hidden state ----
// Note: this system owns the intensity of every Light+Position entity (oscillating around ~1.05);
// for art-side torch brightness tweaks, adjust radius / color instead.
vitric.system("torch-flicker", { query: ["Light", "Position"], writes: ["Light"] }, (entities, ctx) => {
  for (const e of entities) {
    let h = 0;
    for (let i = 0; i < e.id.length; i++) h = (h * 31 + e.id.charCodeAt(i)) % 997;
    const t = ctx.tick * 0.13 + h;
    e.Light.intensity = 1.05 + 0.14 * Math.sin(t) + 0.06 * Math.sin(t * 2.7 + 1.3);
  }
});
