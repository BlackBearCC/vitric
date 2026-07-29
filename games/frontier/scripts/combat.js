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
// Sandbeast is desert-only, spawned by the desert-spawn system on a timer.
const ENEMY_TYPES = {
  gnawer: { damage: 5,  aggro_range: 8,  hp: 20, drops: { hide: [1, 2] } },
  raider: { damage: 8,  aggro_range: 10, hp: 35, drops: { hide: [1, 1], crystal_core: [0, 1] } }, // crystal_core 50% chance
  // Sandbeast: desert-only enemy, spawned by desert-spawn system. High HP, high damage,
  // drops crystal_core. Only spawns when desert is active AND player is in desert.
  sandbeast: { damage: 12, aggro_range: 12, hp: 60, drops: { hide: [2, 3], crystal_core: [1, 2] } },
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
// P1 exploration gear also round-trips (so craft deductions don't zero them on loot apply).
const LOOT_ITEMS = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core", "climbing_gear", "swamp_boots", "heat_suit"];

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
      Enemy: { kind, damage: def.damage, aggro_range: def.aggro_range, home_region: "wild", _attack_cd: 0, _hit_flash: 0, _charge_t: 0, _charge_cd: 0, _flank_dir: 0, _nest_id: "" },
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

// ---- enemy-ai: behavior-differentiated movement toward player ----
// P1: three behavior modes based on Enemy.kind:
//   gnawer   → chase: straight-line to player (original behavior)
//   raider   → flank: approach a point offset 2 tiles perpendicular to player, then cut in
//   sandbeast→ charge: slow walk; every 3s windup 0.5s (stop+flash), then 3x speed charge
// All modes share aggro_range gating; within ENEMY_ATTACK_RANGE they stop (attack handled elsewhere).
vitric.system("enemy-ai", { query: ["Enemy", "Position", "Velocity"], writes: ["Velocity", "Enemy"] }, (entities, ctx) => {
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  for (const e of entities) {
    const dx = px - e.Position.x;
    const dy = py - e.Position.y;
    const dist = Math.sqrt(dx * dx + dy * dy) || 0.001;
    const aggro = e.Enemy.aggro_range || 8;
    const kind = e.Enemy.kind || "gnawer";

    if (dist <= ENEMY_ATTACK_RANGE) {
      e.Velocity.x = 0;
      e.Velocity.y = 0;
      continue;
    }

    if (kind === "raider") {
      // Flank: target a point 2 tiles perpendicular to the player-relative direction.
      // _flank_dir is set once on spawn (+1 or -1); if 0, pick deterministically from position.
      let flank = e.Enemy._flank_dir || 0;
      if (flank === 0) {
        flank = ((((e.Position.x * 7) | 0) % 2) * 2 - 1) || 1;
        e.Enemy._flank_dir = flank;
      }
      // Perpendicular vector (rotate (dx,dy) by 90°): (-dy, dx) * flank
      const perpX = -dy / dist * flank;
      const perpY =  dx / dist * flank;
      const targetX = px + perpX * 2;
      const targetY = py + perpY * 2;
      const fdx = targetX - e.Position.x;
      const fdy = targetY - e.Position.y;
      const fdist = Math.sqrt(fdx * fdx + fdy * fdy) || 0.001;
      // If close to flank point, cut in toward player.
      if (fdist < 1.0) {
        e.Velocity.x = (dx / dist) * 1.2;
        e.Velocity.y = (dy / dist) * 1.2;
      } else {
        e.Velocity.x = (fdx / fdist) * 1.2;
        e.Velocity.y = (fdy / fdist) * 1.2;
      }
    } else if (kind === "sandbeast") {
      // Charge: slow walk by default; every 3s, windup 0.5s (stop + flash white),
      // then charge at 3x speed for 0.5s in the current player direction.
      let chargeCd = (e.Enemy._charge_cd || 0) - ctx.dt;
      let chargeT  = (e.Enemy._charge_t || 0) - ctx.dt;
      if (chargeT > 0) {
        // Currently charging: 3x speed toward player.
        e.Velocity.x = (dx / dist) * 3.0;
        e.Velocity.y = (dy / dist) * 3.0;
        e.Enemy._hit_flash = 0.1; // flash during charge for visual cue
      } else if (chargeCd > 0) {
        // Walking slowly, waiting for next charge.
        e.Velocity.x = (dx / dist) * 0.4;
        e.Velocity.y = (dy / dist) * 0.4;
      } else {
        // Cooldown expired → start windup (0.5s stop + flash), then charge.
        // We model windup as chargeT going negative for 0.5s, then positive for 0.5s.
        // Simpler: set chargeT = 0.5 (charge phase), chargeCd = 3.0 (next cooldown).
        // But we want a windup pause first. Use chargeT = -0.5 as windup signal.
        // For simplicity here: immediately enter charge phase.
        e.Velocity.x = 0;
        e.Velocity.y = 0;
        e.Enemy._hit_flash = 0.3; // bright windup flash
        if (chargeCd < -0.5) {
          // Windup done → charge.
          e.Enemy._charge_t = 0.5;
          e.Enemy._charge_cd = 3.0;
        }
      }
      e.Enemy._charge_cd = chargeCd;
      e.Enemy._charge_t = chargeT;
    } else {
      // gnawer (default): straight-line chase.
      if (dist <= aggro) {
        e.Velocity.x = (dx / dist) * ENEMY_SPEED;
        e.Velocity.y = (dy / dist) * ENEMY_SPEED;
      } else {
        // Out of aggro range: idle (slow drift toward player to avoid stuck).
        e.Velocity.x = (dx / dist) * ENEMY_SPEED * 0.1;
        e.Velocity.y = (dy / dist) * ENEMY_SPEED * 0.1;
      }
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
  // P0 combat feedback: knockback (push enemy 0.5 tile along attack vector)
  const dist = Math.sqrt(bestD2) || 0.001;
  const kb = 0.5;
  const knockX = best.x + (best.x - a.px) / dist * kb;
  const knockY = best.y + (best.y - a.py) / dist * kb;
  ctx.setField(best.id, "Position.x", knockX);
  ctx.setField(best.id, "Position.y", knockY);
  // P0 combat feedback: hit flash (white tint for 0.15s, decay system clears it)
  ctx.setField(best.id, "Enemy._hit_flash", 0.15);
  if (finalHp <= 0) {
    const kind = ctx.getField(best.id, "Enemy.kind") || "gnawer";
    const loot = rollLoot(kind, ctx);
    // P0 combat feedback: kill particles (3 red shards fly out and decay)
    // Particle.ttl is integer ticks (engine age_particles decrements by 1/tick). 30 ticks = 0.5s @ 60 TPS.
    for (let i = 0; i < 3; i++) {
      ctx.spawn({
        Particle: { decay: 0.5, ttl: 30 },
        Position: { x: best.x, y: best.y },
        Velocity: { x: (ctx.random() - 0.5) * 3, y: (ctx.random() - 0.5) * 3 },
        Sprite: { w: 0.2, h: 0.2, image: "", color: "#ff6644" },
      });
    }
    ctx.despawn(best.id);
    ctx.emit("enemy-killed", { id: best.id, kind, loot });
  } else {
    ctx.emit("enemy-hit", { id: best.id, damage: dmg });
  }
});

// ---- P0 combat feedback systems ----
// particle-decay: decrement Particle.decay by dt; despawn at 0. Writes Particle.decay only.
vitric.system("particle-decay", { query: ["Particle"], writes: ["Particle"] }, (entities, ctx) => {
  for (const e of entities) {
    const d = (e.Particle.decay || 0) - ctx.dt;
    if (d <= 0) {
      ctx.despawn(e.id);
    } else {
      e.Particle.decay = d;
    }
  }
});

// ---- enemy-hit-flash-decay: decrement _hit_flash, restore normal color when expired ----
// Writes Enemy._hit_flash and Sprite.color (so the tint reflects the flash state each tick).
vitric.system("enemy-hit-flash-decay", { query: ["Enemy", "Sprite"], writes: ["Enemy", "Sprite"] }, (entities, ctx) => {
  for (const e of entities) {
    let flash = (e.Enemy._hit_flash || 0) - ctx.dt;
    if (flash < 0) flash = 0;
    e.Enemy._hit_flash = flash;
    // While flashing, tint white. When not flashing, restore by-kind base color.
    if (flash > 0) {
      e.Sprite.color = "#ffffff";
    } else {
      const k = e.Enemy.kind || "gnawer";
      const base = (k === "sandbeast") ? "#d4a84a"
        : (k === "raider") ? "#aa3333"
        : "#aa3333";
      e.Sprite.color = base;
    }
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

// ---- player-respawn-check: teleport to (7,7) + restore Hp + drop 30% resources on death ----
// P3 death penalty rework:
//   - Drop 30% of all inventory resources at the death location (player can go back to retrieve)
//   - Increment Colony._death_count; 3 consecutive deaths → suppress night waves for the rest of the day
//     (the night-fall-spawn-wave rule checks _death_count >= 3)
vitric.system("player-respawn-check", { query: ["Player", "Hp", "Position"], writes: ["Hp", "Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if ((e.Hp.value || 0) > 0) continue;
    // Record death location for drop pile.
    const deathX = e.Position.x;
    const deathY = e.Position.y;
    ctx.setField("colony", "Colony._death_drop_x", deathX);
    ctx.setField("colony", "Colony._death_drop_y", deathY);
    // Drop 30% of all inventory resources (emit inv-set with reduced values).
    const invKeys = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core", "climbing_gear", "swamp_boots", "heat_suit"];
    const inv = {};
    for (const k of invKeys) inv[k] = Math.floor((ctx.getField("player", "Inventory." + k) | 0) * 0.7);
    ctx.emit("inv-set", inv);
    // Spawn a "drop pile" entity at the death location so the player can visually find it.
    ctx.spawn({
      Position: { x: deathX, y: deathY },
      Sprite: { w: 0.7, h: 0.7, image: "", color: "#8a6a3a" },
      Text: { content: "遗落物资", size: 0.3, color: "#ffaa44", screen: false },
      Fog: { state: "dim", _orig_color: "" },
    });
    // Increment death count.
    const dc = (ctx.getField("colony", "Colony._death_count") | 0) + 1;
    ctx.setField("colony", "Colony._death_count", dc);
    // Restore Hp + teleport.
    e.Hp.value = RESPAWN_HP;
    e.Hp.max = RESPAWN_HP;
    e.Position.x = RESPAWN_X;
    e.Position.y = RESPAWN_Y;
    // Apply -20% food penalty (kept from original).
    const food = ctx.getField("colony", "Colony.food") || 0;
    const foodNum = (typeof food === "number" && !isNaN(food)) ? food : 0;
    ctx.setField("colony", "Colony.food", Math.max(0, foodNum * (1 - RESPAWN_FOOD_PENALTY)));
    ctx.emit("player-respawned", { x: RESPAWN_X, y: RESPAWN_Y });
    if (dc >= 3) {
      ctx.emit("toast-show", { text: "你倒下了 (连续" + dc + "次)。今夜不再刷怪。" });
    } else {
      ctx.emit("toast-show", { text: "你倒下了,物资遗落在原地" });
    }
  }
});

// ---- P2 enemy nests: active combat targets the player can seek out ----
// Nests spawn enemies periodically (up to max_alive). Player attacks nests via combat-click
// when Mode=combat and the click hits a nest entity (handled by nest-attack rule).
// On destroy: drop loot + emit nest-destroyed.
//
// nest-spawn: every spawn_cd seconds, if alive_count < max_alive, spawn an enemy of nest.kind
// at a position near the nest. We track alive_count by counting children — simpler to just
// increment on spawn and decrement when the enemy is despawned (enemy-killed event does this).
vitric.system("nest-spawn", { query: ["EnemyNest", "Position", "Hp"], writes: ["EnemyNest"] }, (entities, ctx) => {
  for (const e of entities) {
    let cd = (e.EnemyNest.spawn_cd || 0) - ctx.dt;
    const alive = e.EnemyNest.alive_count | 0;
    const max = e.EnemyNest.max_alive | 3;
    if (cd <= 0 && alive < max) {
      // Spawn an enemy near the nest.
      const ang = ctx.random() * Math.PI * 2;
      const r = 1.5 + ctx.random() * 1.5;
      const ex = e.Position.x + Math.cos(ang) * r;
      const ey = e.Position.y + Math.sin(ang) * r;
      const kind = e.EnemyNest.kind || "gnawer";
      const hp = kind === "sandbeast" ? 40 : (kind === "raider" ? 20 : 15);
      ctx.spawn({
        Enemy: { kind, damage: 5, aggro_range: 10, home_region: "wild", _attack_cd: 0, _hit_flash: 0, _charge_t: 0, _charge_cd: 0, _flank_dir: 0, _nest_id: e.id },
        Hp: { value: hp, max: hp },
        Position: { x: ex, y: ey },
        Velocity: { x: 0, y: 0 },
        Sprite: { w: 0.9, h: 0.9, image: "", color: kind === "sandbeast" ? "#d4a84a" : "#aa3333" },
        Text: { content: "", size: 0.3, color: "#ffffff", screen: false },
      });
      e.EnemyNest.alive_count = alive + 1;
      cd = 20; // next spawn in 20 seconds
    }
    e.EnemyNest.spawn_cd = cd;
  }
});

// ---- nest-auto-attack: system that attacks nearby nests when player is in combat mode ----
// Replaces the nest_attack fn + nest-attack-click rule (mouse events don't carry entity handle).
// Runs every tick: if Mode=combat and player is within weapon range of a nest, attack it
// (respecting weapon cooldown). On destroy: drop loot + emit nest-destroyed.
vitric.system("nest-auto-attack", { query: ["EnemyNest", "Position", "Hp"], writes: ["Hp"] }, (entities, ctx) => {
  const mode = ctx.getField("uistate", "Mode.value") || "";
  if (mode !== "combat") return;
  const cdT = ctx.getField("@player", "Weapon._cd_t") || 0;
  if (cdT > 0) return; // weapon on cooldown
  const px = ctx.getField("colony", "Colony.player_x") || 0;
  const py = ctx.getField("colony", "Colony.player_y") || 0;
  const range = ctx.getField("@player", "Weapon.range") || 2;
  const dmg = ctx.getField("@player", "Weapon.damage") || 10;
  const cd = ctx.getField("@player", "Weapon.cooldown") || 1;
  for (const e of entities) {
    const dx = e.Position.x - px, dy = e.Position.y - py;
    const d2 = dx * dx + dy * dy;
    if (d2 > range * range) continue;
    // In range — attack!
    const curHp = (typeof e.Hp.value === "number" && !isNaN(e.Hp.value)) ? e.Hp.value : 30;
    const finalHp = Math.max(0, curHp - dmg);
    e.Hp.value = finalHp;
    ctx.setField("@player", "Weapon._cd_t", cd);
    if (finalHp <= 0) {
      const kind = e.EnemyNest.kind || "gnawer";
      const loot = rollLoot(kind, ctx);
      loot.crystal_core = (loot.crystal_core || 0) + 1;
      loot.ore = (loot.ore || 0) + 3;
      loot.hide = (loot.hide || 0) + 2;
      ctx.emit("enemy-killed", { id: e.id, kind: "nest", loot });
      ctx.emit("nest-destroyed", { id: e.id, kind });
      ctx.emit("toast-show", { text: "巢穴摧毁!" });
      ctx.despawn(e.id);
    }
    break; // only attack one nest per tick
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

// ---- P1 weapon upgrade recipes ----
// Each recipe: cost (materials) + new weapon stats. Player must have the previous weapon
// (checked via Weapon.kind) to upgrade. On success: emit inv-set (materials deducted) +
// emit weapon-upgrade (new stats), the rule writes Weapon.* back.
const WEAPON_RECIPES = {
  bone_blade:    { cost: { hide: 3, ore: 2 },   kind: "bone_blade",    damage: 18, range: 2.0, cooldown: 0.9, requires: "bone_blade_prev", prev: "stone_axe" },
  crystal_edge:  { cost: { crystal_core: 2, plank: 2 }, kind: "crystal_edge", damage: 30, range: 2.5, cooldown: 0.8, prev: "bone_blade" },
};

vitric.fn("craft_weapon", (a, ctx) => {
  const rec = WEAPON_RECIPES[a.id];
  if (!rec) return;
  // Check previous weapon equipped.
  const curKind = ctx.getField("@player", "Weapon.kind") || "stone_axe";
  if (curKind !== rec.prev) {
    ctx.emit("toast-show", { text: "需要先装备 " + rec.prev });
    return;
  }
  // Check materials.
  const inv = {};
  for (const k of LOOT_ITEMS) inv[k] = a[k] | 0;
  for (const k in rec.cost) {
    if ((inv[k] || 0) < rec.cost[k]) {
      ctx.emit("toast-show", { text: "材料不足" });
      return;
    }
  }
  // Deduct materials.
  for (const k in rec.cost) inv[k] -= rec.cost[k];
  ctx.emit("inv-set", inv);
  // Apply weapon upgrade.
  ctx.emit("weapon-upgrade", { kind: rec.kind, damage: rec.damage, range: rec.range, cooldown: rec.cooldown });
  ctx.emit("toast-show", { text: "武器升级: " + rec.kind });
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

// ---- desert-spawn: every 2 in-game hours (7200 ticks), if the desert region is active AND
// the player is inside it, spawn a sandbeast near the player. Uses ctx.random_stream("desert_spawn")
// for deterministic spawn position — replay-safe regardless of when the spawn happens.
//
// The spawn_timer field on Region (schema line 1041) tracks the cooldown. It's decremented
// each tick; when it hits 0, the spawn check fires and the timer resets.
//
// The system queries ["Region"] (matches all region markers), filters for the desert marker
// in the body. writes: ["Region"] covers the spawn_timer write on the desert marker entity.
// Player position is read via ctx.getField (deferred-op channel — same pattern as
// region-approach-check; doesn't require a writes declaration).
vitric.system("desert-spawn", { query: ["Region"], writes: ["Region"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Region.id !== "desert") continue;
    if (e.Region.state !== "active") continue;

    // Decrement spawn timer.
    let timer = e.Region.spawn_timer - ctx.dt;
    if (timer > 0) {
      e.Region.spawn_timer = timer;
      continue;
    }

    // Timer expired — reset and check if player is in desert.
    e.Region.spawn_timer = 7200; // 2 minutes real time (7200 ticks at 60 tick/s)

    const px = ctx.getField("player", "Position.x");
    const py = ctx.getField("player", "Position.y");
    if (typeof px !== "number" || typeof py !== "number") continue;

    // Desert bounds: anchor (60,0), size 60×60 → x:60..119, y:0..59.
    const inDesert = px >= 60 && px <= 119 && py >= 0 && py <= 59;
    if (!inDesert) continue;

    // Spawn sandbeast near player using desert_spawn substream (deterministic).
    const stream = ctx.random_stream("desert_spawn");
    const ox = stream.nextInt(-3, 3);
    const oy = stream.nextInt(-3, 3);
    const def = ENEMY_TYPES.sandbeast;
    ctx.spawn({
      Enemy: { kind: "sandbeast", damage: def.damage, aggro_range: def.aggro_range,
               home_region: "desert", _attack_cd: 0, _hit_flash: 0, _charge_t: 0, _charge_cd: 0, _flank_dir: 0, _nest_id: "" },
      Position: { x: px + ox, y: py + oy },
      Velocity: { x: 0, y: 0 },
      Collider: { w: 1.0, h: 1.0 },
      Sprite: { w: 1.0, h: 1.0, image: "enemy.png", color: "#d4a84a" },
      Hp: { value: def.hp, max: def.hp },
    });
    ctx.emit("toast-show", { text: "沙兽出现!" });
  }
});
