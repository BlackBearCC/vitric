// Combat system (Task 10): night-triggered enemy waves, enemy AI, structure degradation, player weapon swings,
// guard/turret auto-defense, loot drops, and player respawn.
//
// Systems (registration order = script load order; combat.js loads BEFORE economy.js so STRUCTURE_HP_BY_TIER
// is available to the build fn):
//   enemy-snapshot          Enemy+Position → Colony.enemy_snapshot (JSON, mirrors drifter-snapshot pattern)
//   enemy-ai                Enemy+Position+Velocity → straight-line path to player (cached in Colony.player_x/y)
//   enemy-attack-player     Enemy+Position → continuous DPS to @player.Hp.value when adjacent
//   enemy-attack-structures Structure+Position+Hp → continuous DPS from nearest snapshot enemy; tier downgrade on Hp<=0
//   player-combat-cooldown  Player+Weapon → decrement Weapon._cd_t by dt every tick
//   turret-auto-attack      Structure+Position → discrete swings at nearest enemy (Structure._cd_t cooldown)
//   guard-auto-defense      Companion+Persona+Need+Position → continuous DPS to nearest enemy (role=guard, affinity>=60)
//   player-respawn-check    Player+Hp+Position → teleport to (7,7) + Hp restore + -20% food on Hp<=0
//
// Fns (called by rules in rules/combat.json):
//   spawn_wave           night-fall{threat} → spawn N enemies at region edge
//   player_attack        combat mode + mouse click → swing weapon at nearest enemy (discrete, cooldown-gated)
//   apply_loot           enemy-killed{loot} → emit inv-set with new inventory (existing inv-apply rule writes back)
//   retreat_all_enemies  dawn-break → despawn all enemies
//
// Data flow: Colony.enemy_snapshot (JSON text) is the cross-entity bridge — systems that need to find enemies
// (enemy-attack-structures, turret, guard, player_attack) read the snapshot instead of querying Enemy directly
// (the engine doesn't support cross-entity queries within one system). Colony.player_x/y (written by
// cache-player-pos in companion.js) is the player position bridge.

// Shared global: structure HP by tier. Referenced by economy.js build fn — combat.js MUST load before economy.js.
const STRUCTURE_HP_BY_TIER = { 1: 50, 2: 100, 3: 200 };

// Enemy constants.
const ENEMY_SPEED = 0.8;        // tiles per second
const ENEMY_ATTACK_RANGE = 1.5; // distance at which enemy attacks (player or structure)

// Player respawn constants.
const RESPAWN_X = 7;
const RESPAWN_Y = 7;
const RESPAWN_HP = 100;
const RESPAWN_FOOD_PENALTY = 0.2; // -20% food on death

// Turret constants.
const TURRET_RANGE = 5;
const TURRET_DAMAGE = 8;
const TURRET_COOLDOWN = 1.5;

// Guard constants.
const GUARD_RANGE = 2;
const GUARD_DAMAGE = 6;
const GUARD_AFFINITY_MIN = 60;

// Enemy type table: hp / damage / aggro_range / loot drops.
// Raider only spawns if mountain region is thawed (forward-compat for Task 12).
// Sandbeast is deferred to Task 13 (desert region only).
const ENEMY_TYPES = {
  gnawer: { damage: 5,  aggro_range: 8,  hp: 20, drops: { hide: [1, 2] } },
  raider: { damage: 8,  aggro_range: 10, hp: 35, drops: { hide: [1, 1], crystal_core: [0, 1] } }, // crystal_core 50% chance
};

// Roll loot for a killed enemy. Returns { hide: N, crystal_core: M } based on kind's drop table.
// Uses ctx.random() for determinism (substream-derived, replay-safe).
function rollLoot(kind, ctx) {
  const def = ENEMY_TYPES[kind] || ENEMY_TYPES.gnawer;
  const loot = {};
  for (const k in def.drops) {
    const [min, max] = def.drops[k];
    const range = max - min + 1;
    loot[k] = min + ((ctx.random() * range) | 0);
  }
  return loot;
}

// Full inventory field set for loot application (must match economy.js ITEMS + schema Inventory).
// Renamed to LOOT_ITEMS to avoid redeclaration conflict with economy.js in the shared QuickJS global.
const LOOT_ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core"];

// Read snapshot JSON from Colony; on failure return empty array.
function readSnapshot(raw) {
  if (!raw || typeof raw !== "string") return [];
  try { return JSON.parse(raw) || []; } catch (_) { return []; }
}

// ---- spawn_wave fn: called by night-fall-spawn-wave rule on night-fall{threat} ----
// Wave size = min(8, threat * (1 + regionCount * 0.3)) where regionCount = thawed regions (home + mountain + desert).
// Each enemy spawned anonymously with Enemy + Position + Velocity + Collider + Sprite + Hp.
// 70% gnawer / 30% raider IF mountain thawed AND day >= 5 (raider requires mountain).
vitric.fn("spawn_wave", (a, ctx) => {
  const threat = (a.threat | 0) || 1;
  const day = (a.day | 0) || 1;
  // Count thawed regions. Home is always active. Mountain/desert may be dormant (Task 12/13).
  let regionCount = 1; // home
  const mountainDisc = ctx.getField("mountain", "Region.discovered") | 0;
  if (mountainDisc === 1) regionCount += 1;
  // desert doesn't exist yet (Task 13) — ctx.getField returns 0/default.
  const waveSize = Math.min(8, Math.floor(threat * (1 + regionCount * 0.3)));
  // Spawn at region boundary (x=30 + jitter, y=5..15).
  for (let i = 0; i < waveSize; i++) {
    let kind = "gnawer";
    if (mountainDisc === 1 && day >= 5 && ctx.random() < 0.3) kind = "raider";
    const def = ENEMY_TYPES[kind];
    const spawnX = 30 + ctx.random() * 4;
    const spawnY = 5 + ctx.random() * 10;
    ctx.spawn({
      Enemy: { kind, damage: def.damage, aggro_range: def.aggro_range, home_region: "wild", _attack_cd: 0 },
      Position: { x: spawnX, y: spawnY },
      Velocity: { x: 0, y: 0 },
      Collider: { w: 0.8, h: 0.8 },
      Sprite: { w: 0.8, h: 0.8, image: "enemy.png", color: "#aa3333" },
      Hp: { value: def.hp, max: def.hp }
    });
  }
  ctx.emit("wave-spawned", { count: waveSize, threat });
});

// ---- enemy-snapshot: pack all Enemy entities' id/position/kind/damage into Colony.enemy_snapshot ----
// Mirrors drifter-snapshot / companion-snapshot pattern. Consumed by systems that need cross-entity enemy lookup.
vitric.system("enemy-snapshot", { query: ["Enemy", "Position"], writes: [] }, (entities, ctx) => {
  const data = entities.map(e => ({
    id: e.id,
    x: e.Position.x, y: e.Position.y,
    kind: e.Enemy.kind || "gnawer",
    damage: e.Enemy.damage || 5
  }));
  ctx.setField("colony", "Colony.enemy_snapshot", JSON.stringify(data));
});

// ---- enemy-ai: straight-line path to player when within aggro_range ----
// No A* (deterministic, sufficient per spec). When within ENEMY_ATTACK_RANGE, stop moving (will attack via
// enemy-attack-player / enemy-attack-structures). Out of aggro: slow drift to avoid stuck.
vitric.system("enemy-ai", { query: ["Enemy", "Position", "Velocity"], writes: ["Velocity"] }, (entities, ctx) => {
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  for (const e of entities) {
    const dx = px - e.Position.x;
    const dy = py - e.Position.y;
    const dist = Math.sqrt(dx * dx + dy * dy) || 0.001;
    const aggro = e.Enemy.aggro_range || 8;
    if (dist <= ENEMY_ATTACK_RANGE) {
      e.Velocity.x = 0;
      e.Velocity.y = 0;
    } else if (dist <= aggro) {
      e.Velocity.x = (dx / dist) * ENEMY_SPEED;
      e.Velocity.y = (dy / dist) * ENEMY_SPEED;
    } else {
      // Out of aggro range: idle (slow drift toward player to avoid stuck).
      e.Velocity.x = (dx / dist) * ENEMY_SPEED * 0.1;
      e.Velocity.y = (dy / dist) * ENEMY_SPEED * 0.1;
    }
  }
});

// ---- enemy-attack-player: continuous DPS to @player.Hp when enemy is adjacent ----
// Reads player position from Colony cache; applies sum(damage * dt) for all enemies within ENEMY_ATTACK_RANGE.
vitric.system("enemy-attack-player", { query: ["Enemy", "Position"], writes: [] }, (entities, ctx) => {
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  let totalDamage = 0;
  for (const e of entities) {
    const dx = px - e.Position.x, dy = py - e.Position.y;
    const d2 = dx * dx + dy * dy;
    if (d2 <= ENEMY_ATTACK_RANGE * ENEMY_ATTACK_RANGE) {
      totalDamage += (e.Enemy.damage || 5) * ctx.dt;
    }
  }
  if (totalDamage > 0) {
    const curHp = ctx.getField("@player", "Hp.value");
    const hpNum = (typeof curHp === "number" && !isNaN(curHp)) ? curHp : 100;
    ctx.setField("@player", "Hp.value", Math.max(0, hpNum - totalDamage));
  }
});

// ---- enemy-attack-structures: continuous DPS from nearest snapshot enemy; tier downgrade on Hp<=0 ----
// Reads Colony.enemy_snapshot to find nearest enemy within ENEMY_ATTACK_RANGE (engine doesn't support
// cross-entity queries in one system). Structure Hp<=0 → tier-1 downgrade (reset Hp) or despawn (tier 1).
// writes: ["Hp", "Structure"] — modifies Hp.value/max AND Structure.tier.
vitric.system("enemy-attack-structures", { query: ["Structure", "Position", "Hp"], writes: ["Hp", "Structure"] }, (entities, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  const snapshot = readSnapshot(raw);
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  for (const e of entities) {
    const sx = e.Position.x, sy = e.Position.y;
    let nearestDmg = 0;
    for (const enemy of snapshot) {
      const dx = enemy.x - sx, dy = enemy.y - sy;
      const d2 = dx * dx + dy * dy;
      if (d2 <= ENEMY_ATTACK_RANGE * ENEMY_ATTACK_RANGE) {
        const dmg = enemy.damage || 5;
        if (dmg > nearestDmg) nearestDmg = dmg;
      }
    }
    if (nearestDmg > 0) {
      const newHp = Math.max(0, (e.Hp.value || 0) - nearestDmg * ctx.dt);
      e.Hp.value = newHp;
      if (newHp <= 0) {
        // Downgrade tier or despawn.
        const curTier = e.Structure.tier | 0;
        if (curTier > 1) {
          const newTier = curTier - 1;
          e.Structure.tier = newTier;
          const newMax = STRUCTURE_HP_BY_TIER[newTier] || 50;
          e.Hp.value = newMax;
          e.Hp.max = newMax;
          ctx.emit("structure-downgraded", { entity: e.id, tier: newTier });
        } else {
          ctx.emit("structure-destroyed", { entity: e.id });
          ctx.despawn(e.id);
        }
      }
    }
  }
});

// ---- player-combat-cooldown: decrement Weapon._cd_t by dt every tick ----
// Separates cooldown decrement (system, every tick) from swing trigger (fn, on click). Both deterministic.
vitric.system("player-combat-cooldown", { query: ["Player", "Weapon"], writes: ["Weapon"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Weapon._cd_t > 0) {
      e.Weapon._cd_t = Math.max(0, e.Weapon._cd_t - ctx.dt);
    }
  }
});

// ---- player_attack fn: called by combat-click rule on mouse click when Mode=combat ----
// Reads Weapon._cd_t via ctx.getField (the rule passes player position + weapon stats).
// If _cd_t > 0, no-op (on cooldown). Else: find nearest enemy from snapshot, apply weapon damage if in range,
// reset _cd_t to cooldown. On kill: roll loot, despawn, emit enemy-killed{loot}.
//
// NOTE: This is the SECOND version of player_attack from the brief (cooldown via ctx.getField, NOT cooldown-in-args).
// The first version (lines 340-377 of the brief) was REJECTED — it passed cooldown as an arg, which doesn't
// decrement per-tick. This version reads the live Weapon._cd_t that player-combat-cooldown decrements every tick.
vitric.fn("player_attack", (a, ctx) => {
  // a.px, a.py: player position; a.weapon_damage, a.weapon_range, a.weapon_cd: weapon stats
  const cdT = ctx.getField("@player", "Weapon._cd_t") || 0;
  if (cdT > 0) return; // still on cooldown
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  const snapshot = readSnapshot(raw);
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  let bestD2 = Infinity, best = null;
  for (const enemy of snapshot) {
    const dx = enemy.x - a.px, dy = enemy.y - a.py;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = enemy; }
  }
  if (!best) return;
  const range = a.weapon_range || 2;
  if (bestD2 > range * range) return; // out of range
  // Apply damage.
  const curHp = ctx.getField(best.id, "Hp.value");
  const hpNum = (typeof curHp === "number" && !isNaN(curHp)) ? curHp : 100;
  const dmg = a.weapon_damage || 10;
  const finalHp = Math.max(0, hpNum - dmg);
  ctx.setField(best.id, "Hp.value", finalHp);
  // Reset cooldown.
  const cd = a.weapon_cd || 1;
  ctx.setField("@player", "Weapon._cd_t", cd);
  if (finalHp <= 0) {
    const kind = ctx.getField(best.id, "Enemy.kind") || "gnawer";
    const loot = rollLoot(kind, ctx);
    ctx.despawn(best.id);
    ctx.emit("enemy-killed", { id: best.id, kind, loot });
  } else {
    ctx.emit("enemy-hit", { id: best.id, damage: dmg });
  }
});

// ---- turret-auto-attack: Structure kind=="turret" auto-attacks nearest enemy ----
// Discrete swings on Structure._cd_t cooldown. On kill: roll loot, despawn, emit enemy-killed{by:"turret"}.
vitric.system("turret-auto-attack", { query: ["Structure", "Position"], writes: ["Structure"] }, (entities, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  const snapshot = readSnapshot(raw);
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  for (const e of entities) {
    if (e.Structure.kind !== "turret") continue;
    // Decrement cooldown first.
    if ((e.Structure._cd_t || 0) > 0) {
      e.Structure._cd_t = Math.max(0, e.Structure._cd_t - ctx.dt);
      continue;
    }
    // Find nearest enemy in range.
    const sx = e.Position.x, sy = e.Position.y;
    let bestD2 = Infinity, best = null;
    for (const enemy of snapshot) {
      const dx = enemy.x - sx, dy = enemy.y - sy;
      const d2 = dx * dx + dy * dy;
      if (d2 < bestD2) { bestD2 = d2; best = enemy; }
    }
    if (!best || bestD2 > TURRET_RANGE * TURRET_RANGE) continue;
    // Fire.
    const curHp = ctx.getField(best.id, "Hp.value");
    const hpNum = (typeof curHp === "number" && !isNaN(curHp)) ? curHp : 100;
    const finalHp = Math.max(0, hpNum - TURRET_DAMAGE);
    ctx.setField(best.id, "Hp.value", finalHp);
    e.Structure._cd_t = TURRET_COOLDOWN;
    if (finalHp <= 0) {
      const kind = ctx.getField(best.id, "Enemy.kind") || "gnawer";
      const loot = rollLoot(kind, ctx);
      ctx.despawn(best.id);
      ctx.emit("enemy-killed", { id: best.id, kind, loot, by: "turret" });
    } else {
      ctx.emit("enemy-hit", { id: best.id, damage: TURRET_DAMAGE, by: "turret" });
    }
  }
});

// ---- guard-auto-defense: Companion role=guard + affinity>=60 auto-attacks nearest enemy ----
// Continuous DPS (like enemy-attack-player) to avoid adding a new cooldown field to Need.
// On kill: roll loot, despawn, emit enemy-killed{by:"guard"}.
vitric.system("guard-auto-defense", { query: ["Companion", "Persona", "Need", "Position"], writes: ["Need"] }, (entities, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  const snapshot = readSnapshot(raw);
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  for (const e of entities) {
    if (e.Persona.role !== "guard") continue;
    if ((e.Need.affinity || 0) < GUARD_AFFINITY_MIN) continue;
    const sx = e.Position.x, sy = e.Position.y;
    let bestD2 = Infinity, best = null;
    for (const enemy of snapshot) {
      const dx = enemy.x - sx, dy = enemy.y - sy;
      const d2 = dx * dx + dy * dy;
      if (d2 < bestD2) { bestD2 = d2; best = enemy; }
    }
    if (!best || bestD2 > GUARD_RANGE * GUARD_RANGE) continue;
    // Continuous damage-per-second.
    const curHp = ctx.getField(best.id, "Hp.value");
    const hpNum = (typeof curHp === "number" && !isNaN(curHp)) ? curHp : 100;
    const finalHp = Math.max(0, hpNum - GUARD_DAMAGE * ctx.dt);
    ctx.setField(best.id, "Hp.value", finalHp);
    if (finalHp <= 0) {
      const kind = ctx.getField(best.id, "Enemy.kind") || "gnawer";
      const loot = rollLoot(kind, ctx);
      ctx.despawn(best.id);
      ctx.emit("enemy-killed", { id: best.id, kind, loot, by: "guard" });
    }
  }
});

// ---- player-respawn-check: teleport to (7,7) + restore Hp + -20% food on Hp<=0 ----
vitric.system("player-respawn-check", { query: ["Player", "Hp", "Position"], writes: ["Hp", "Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if ((e.Hp.value || 0) > 0) continue;
    e.Hp.value = RESPAWN_HP;
    e.Hp.max = RESPAWN_HP;
    e.Position.x = RESPAWN_X;
    e.Position.y = RESPAWN_Y;
    // Apply -20% food penalty.
    const food = ctx.getField("colony", "Colony.food") || 0;
    const foodNum = (typeof food === "number" && !isNaN(food)) ? food : 0;
    ctx.setField("colony", "Colony.food", Math.max(0, foodNum * (1 - RESPAWN_FOOD_PENALTY)));
    ctx.emit("player-respawned", { x: RESPAWN_X, y: RESPAWN_Y });
    ctx.emit("toast-show", { text: "你倒下了,被送回登陆点" });
  }
});

// ---- apply_loot fn: called by enemy-killed-loot rule on enemy-killed{loot} ----
// Merges loot into current inventory (passed in full by the rule), emits inv-set with new absolute values.
// The existing inv-apply rule in economy.json consumes inv-set and writes back to @player.Inventory.*.
vitric.fn("apply_loot", (a, ctx) => {
  let loot;
  try { loot = typeof a.loot === "string" ? JSON.parse(a.loot) : a.loot; } catch (_) { loot = {}; }
  if (!loot || typeof loot !== "object") return;
  const inv = {};
  for (const k of LOOT_ITEMS) inv[k] = a[k] | 0;
  for (const k in loot) {
    if (LOOT_ITEMS.indexOf(k) >= 0) inv[k] = (inv[k] || 0) + (loot[k] | 0);
  }
  ctx.emit("inv-set", inv);
});

// ---- retreat_all_enemies fn: called by dawn-break-retreat rule on dawn-break ----
// Despawns all enemies (they retreat to wild). Reads Colony.enemy_snapshot to get enemy IDs.
vitric.fn("retreat_all_enemies", (a, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  const snapshot = readSnapshot(raw);
  if (!Array.isArray(snapshot)) return;
  for (const enemy of snapshot) {
    if (enemy && enemy.id) ctx.despawn(enemy.id);
  }
  ctx.emit("enemies-retreated", { count: snapshot.length });
});
