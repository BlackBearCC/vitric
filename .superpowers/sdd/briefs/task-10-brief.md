# Task 10 Brief — Combat System (Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn)

## Context

Frontier Sandbox Expansion, Task 10 of 16. Base commit: `c440fda` (after Task 9 review artifacts). Plan: `docs/superpowers/plans/2026-07-20-frontier-sandbox-expansion.md` §Task 10 (lines 1151-1250). Spec: `docs/superpowers/specs/2026-07-20-frontier-sandbox-expansion-design.md` §4.2 (lines 183-216).

Phase 3 game systems: Task 10 (Combat) = this task. Task 9 emitted `guard-patrol` as a forward-compat hook — Task 10 consumes it by implementing guard auto-defense. Task 11 (Trading) and Task 12 (Map Expansion) come after.

## Goal

Add a complete combat loop: enemies spawn on `night-fall{threat}`, walk toward the player (straight-line AI), attack player and structures. Player switches to `combat` mode (F key), left-clicks to swing weapon, kills enemies, loot drops (`hide` / `crystal_core`). Companion guards (role=guard + affinity>=60) auto-defend. Turret structure (tech-locked) auto-attacks. Structure Hp=0 → tier downgrade. Player Hp=0 → respawn at lander (7,7) with -20% food.

## Plan corrections (fictional APIs / scope clarifications)

1. **`tools/test_progression.py` does NOT exist** — write Rust tests in `crates/vitric-cli/tests/combat.rs` following the pattern of `research.rs` / `companions.rs`.
2. **Enemy types in scope**: gnawer (day 1+, melee, prioritizes structures) + raider (after mountain unlocked, ranged, targets player). **Sandbeast is deferred to Task 13** (desert region only). Raider spawn gate: only if `mountain` region is thawed (i.e., `ctx.getField("mountain", "Region.discovered") == 1`). Until Task 12 thaws mountain, only gnawers spawn — forward-compatible.
3. **Weapon switching deferred**: spec mentions 3 weapons (stone-axe / spear / arc-gun, keys 1-3). Keys 1-3 are already used for build shortcuts (pick-plot/wall/conduit). Implementing weapon switching would require new UI + key rebinding — out of scope for Task 10. Player has a single default weapon (stone_axe, damage 10, range 2, cooldown 1s). Spear/arc-gun weapon variants deferred to a future task.
4. **`assets/enemy.png` placeholder**: the engine's `text_to_image` is for web pages, not game assets. Copy `games/frontier/assets/rock.png` to `games/frontier/assets/enemy.png` via `cp` shell command. Real art is out of scope (per spec non-goals).
5. **Damage application model**: continuous damage-per-second (not discrete swings) for enemy→structure and enemy→player attacks. This avoids per-attack cooldown management. Player→enemy and turret→enemy and guard→enemy attacks use discrete swings with weapon cooldown (`_cd_t` field).
6. **`Colony.enemy_snapshot`**: new text field (JSON array of `{id, x, y, kind, damage}`) maintained by an `enemy-snapshot` system, mirroring the existing `drifter-snapshot` / `companion-snapshot` pattern. Consumed by `enemy-attack-structures` (so structures can find nearest enemy without querying Enemy directly — engine doesn't support cross-entity queries in one system).
7. **Structure Hp by tier**: tier 1 = 50, tier 2 = 100, tier 3 = 200. The `build` fn in `economy.js` adds `Hp: { value: <tier_hp>, max: <tier_hp> }` to spawned structure components.
8. **No new `Guard` component on companions**: the spec mentions `Guard { post_x, post_y, patrol_r }`, but companion guards already have `Persona.role == "guard"`. Adding a separate Guard component is redundant. Use `Persona.role` for guard detection. The `Guard` component is declared in schema (forward-compat, in case future tasks add patrol posts), but not attached to any entity in Task 10.
9. **Player respawn position**: player starts at (7, 7) per `scenes/main.json`. Use this as the respawn point. No separate "lander" entity.

## Schema changes (`games/frontier/schema.json`)

### Add new components

```json
"Hp": {
  "fields": {
    "value": { "type": "number", "default": 100 },
    "max":   { "type": "number", "default": 100 }
  }
},
"Enemy": {
  "fields": {
    "kind":         { "type": "text",   "default": "gnawer" },
    "damage":       { "type": "number", "default": 5 },
    "aggro_range":  { "type": "number", "default": 8 },
    "home_region":  { "type": "text",   "default": "wild" },
    "_attack_cd":   { "type": "number", "default": 0 }
  }
},
"Weapon": {
  "fields": {
    "kind":     { "type": "text",   "default": "stone_axe" },
    "damage":   { "type": "number", "default": 10 },
    "range":    { "type": "number", "default": 2 },
    "cooldown": { "type": "number", "default": 1 },
    "_cd_t":    { "type": "number", "default": 0 }
  }
},
"Guard": {
  "fields": {
    "post_x":   { "type": "number", "default": 0 },
    "post_y":   { "type": "number", "default": 0 },
    "patrol_r": { "type": "number", "default": 5 }
  }
}
```

### Extend `Structure` component

Add `_cd_t` field (for turret auto-attack cooldown):

```json
"Structure": {
  "fields": {
    "kind":  { "type": "text", "default": "floor" },
    "tier":  { "type": "int",  "default": 1 },
    "_cd_t": { "type": "number", "default": 0 }
  }
}
```

### Extend `Mode` enum

Add `"combat"` variant:

```json
"Mode": {
  "fields": {
    "value": {
      "type": "enum",
      "variants": ["build", "craft", "interact", "upgrade", "research", "combat"],
      "default": "build"
    }
  }
}
```

### Extend `Colony` component

Add `enemy_snapshot` field (JSON text, mirrors `drifter_snapshot` pattern):

```json
"enemy_snapshot": { "type": "text", "default": "[]" }
```

Add `last_threat` field (carries night-fall threat between events, for spawn_wave fn):

```json
"last_threat": { "type": "int", "default": 0 }
```

## Scene changes (`games/frontier/scenes/main.json`)

### Add `Hp` + `Weapon` to `player` entity

```json
"player": {
  "components": {
    "Player": {},
    "Position": { "x": 7, "y": 7 },
    "Velocity": { "x": 0, "y": 0 },
    "Collider": { "w": 0.8, "h": 0.8 },
    "Inventory": {},
    "Sprite": { "w": 0.8, "h": 0.8, "image": "player.png", "color": "#ffffff" },
    "TechPoint": {},
    "Hp": { "value": 100, "max": 100 },
    "Weapon": { "kind": "stone_axe", "damage": 10, "range": 2, "cooldown": 1, "_cd_t": 0 }
  }
}
```

### Add `hp_lbl` HUD entity

Position: top-right, below `collective_wish_lbl` (which is at oy:284 + h:24 = 308, so oy:312). Use `anchor: "top-right"`, `parent: "ui"`, `ox: -32`, `oy: 312`, `w: 260`, `h: 24`. `UiLabel: { content: "HP 100/100", size: 20, color: "#ff6b6b", align: "end" }`.

### Add `mode_combat` button to `mode_row`

Add a 6th button to the existing `mode_row` HBox (after `mode_research`):

```json
"mode_combat": {
  "components": {
    "Ui": { "anchor": "top-left", "parent": "mode_row", "w": 92, "h": 48 },
    "Panel": { "color": "#3a4a6b" },
    "Button": { "action": "mode-combat", "state": "normal" }
  }
},
"mode_combat_lbl": {
  "components": {
    "Ui": { "anchor": "stretch", "parent": "mode_combat" },
    "UiLabel": { "content": "战斗", "size": 30, "color": "#ffffff", "align": "center" }
  }
}
```

Note: `mode_row` width may need to be bumped from 386 to 484 to accommodate the 6th button (92 + 6 gap). Verify via the layout — if buttons overflow, bump width.

## Script: `games/frontier/scripts/combat.js` (NEW)

### Constants

```javascript
const ENEMY_SPEED = 0.8;       // tiles per second
const ENEMY_ATTACK_RANGE = 1.5; // distance at which enemy attacks
const PLAYER_ATTACK_RANGE = 2;  // weapon range
const STRUCTURE_HP_BY_TIER = { 1: 50, 2: 100, 3: 200 };
const RESPAWN_X = 7;
const RESPAWN_Y = 7;
const RESPAWN_HP = 100;
const RESPAWN_FOOD_PENALTY = 0.2; // -20% food

const ENEMY_TYPES = {
  gnawer: { damage: 5,  aggro_range: 8,  hp: 20, drops: { hide: [1, 2] } },
  raider: { damage: 8,  aggro_range: 10, hp: 35, drops: { hide: [1, 1], crystal_core: [0, 1] } }, // crystal_core 50% chance
};

// Loot drop table: returns { hide: N, crystal_core: M } based on enemy kind.
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
```

### `spawn_wave` fn

Called by rule on `night-fall{threat}`. Wave size = `min(8, threat × (1 + region_count × 0.3))` where `region_count` = number of thawed regions (1 + mountain_thawed + desert_thawed). Each enemy spawned as anonymous entity with Enemy + Position + Velocity + Collider + Sprite + Hp components.

```javascript
vitric.fn("spawn_wave", (a, ctx) => {
  const threat = (a.threat | 0) || 1;
  const day = (a.day | 0) || 1;
  // Count thawed regions (home is always active; mountain/desert may be dormant).
  let regionCount = 1; // home
  const mountainDisc = ctx.getField("mountain", "Region.discovered") | 0;
  if (mountainDisc === 1) regionCount += 1;
  // desert doesn't exist yet (Task 12) — ctx.getField returns 0 / default.
  const waveSize = Math.min(8, Math.floor(threat * (1 + regionCount * 0.3)));
  // Spawn position: just outside home region boundary (x=30, y=random 5-15).
  for (let i = 0; i < waveSize; i++) {
    // 70% gnawer, 30% raider IF mountain unlocked (raider requires mountain).
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
```

### `enemy-snapshot` system

Mirrors `drifter-snapshot` / `companion-snapshot`. Pack all Enemy entities' id/position/kind/damage into JSON, write to `Colony.enemy_snapshot`.

```javascript
vitric.system("enemy-snapshot", { query: ["Enemy", "Position"], writes: [] }, (entities, ctx) => {
  const data = entities.map(e => ({
    id: e.id,
    x: e.Position.x, y: e.Position.y,
    kind: e.Enemy.kind || "gnawer",
    damage: e.Enemy.damage || 5
  }));
  ctx.setField("colony", "Colony.enemy_snapshot", JSON.stringify(data));
});
```

### `enemy-ai` system

Straight-line path to player when within `aggro_range`. No A* (deterministic, sufficient per spec). When close enough to attack (within `ENEMY_ATTACK_RANGE`), stop moving.

```javascript
vitric.system("enemy-ai", { query: ["Enemy", "Position", "Velocity"], writes: ["Velocity"] }, (entities, ctx) => {
  // Read player position from Colony (cached by cache-player-pos system in companion.js).
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
```

### `enemy-attack-player` system

Continuous damage-per-second when adjacent. Queries [Enemy, Position], reads player Hp via ctx.getField, applies `damage * dt`.

```javascript
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
```

### `enemy-attack-structures` system

Queries [Structure, Position, Hp], reads `Colony.enemy_snapshot`, finds nearest enemy within `ENEMY_ATTACK_RANGE`, applies continuous damage. Structure Hp <= 0 → downgrade tier (or despawn if tier 1).

```javascript
vitric.system("enemy-attack-structures", { query: ["Structure", "Position", "Hp"], writes: ["Hp"] }, (entities, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  let snapshot;
  try { snapshot = JSON.parse(raw); } catch (_) { snapshot = []; }
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
        if ((e.Structure.tier | 0) > 1) {
          e.Structure.tier = (e.Structure.tier | 0) - 1;
          const newMax = STRUCTURE_HP_BY_TIER[e.Structure.tier] || 50;
          e.Hp.value = newMax;
          e.Hp.max = newMax;
          ctx.emit("structure-downgraded", { entity: e.id, tier: e.Structure.tier });
        } else {
          ctx.despawn(e.id);
          ctx.emit("structure-destroyed", { entity: e.id });
        }
      }
    }
  }
});
```

**Note**: writing `e.Structure.tier` requires `Structure` to be in the `writes` list. Update `writes` to `["Hp", "Structure"]`.

### `player-combat` system

When `Mode.value == "combat"`, on mouse click, swing weapon at nearest enemy in range. Discrete swings with weapon cooldown (`_cd_t`).

Actually — mouse clicks are events, not per-tick. The swing should be triggered by a rule (on `mouse` event, Mode=combat → call `player_attack` fn), not a system. Let me use a fn instead:

```javascript
vitric.fn("player_attack", (a, ctx) => {
  // a.px, a.py: player position; a.weapon_damage, a.weapon_range, a.weapon_cd, a.weapon_cd_t: weapon state
  // Find nearest enemy via Colony.enemy_snapshot, apply damage if in range.
  const cdT = (a.weapon_cd_t || 0) - ctx.dt;
  if (cdT > 0) {
    // Still on cooldown — emit cooldown event for UX (optional).
    return;
  }
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  let snapshot;
  try { snapshot = JSON.parse(raw); } catch (_) { snapshot = []; }
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
  const newHp = ctx.getField(best.id, "Hp.value");
  const hpNum = (typeof newHp === "number" && !isNaN(newHp)) ? newHp : 100;
  const dmg = a.weapon_damage || 10;
  const finalHp = Math.max(0, hpNum - dmg);
  ctx.setField(best.id, "Hp.value", finalHp);
  // Reset weapon cooldown.
  ctx.emit("weapon-cd-reset", { cd: a.weapon_cd || 1 });
  if (finalHp <= 0) {
    // Kill: roll loot, despawn, emit.
    const kind = ctx.getField(best.id, "Enemy.kind") || "gnawer";
    const loot = rollLoot(kind, ctx);
    ctx.despawn(best.id);
    ctx.emit("enemy-killed", { id: best.id, kind, loot });
  } else {
    ctx.emit("enemy-hit", { id: best.id, damage: dmg });
  }
});
```

Wait — there's a problem with the cooldown. The fn approach reads `weapon_cd_t` from the rule args, but the cooldown needs to decrement every tick (not just on click). The cleanest approach is a SYSTEM that decrements `_cd_t` every tick, and the `player_attack` fn checks if `_cd_t <= 0` before swinging.

Actually, let me reconsider. The simplest model:
- `player-combat-cooldown` system: query [Player, Weapon], write [Weapon], decrement `_cd_t` by dt every tick.
- `player-attack` fn: called by rule on mouse click when Mode=combat. Reads `Weapon._cd_t` via ctx.getField. If > 0, no-op. If <= 0, swing + set `Weapon._cd_t = cooldown` via ctx.setField.

This separates cooldown decrement (system, every tick) from swing trigger (fn, on click). Both are deterministic.

```javascript
vitric.system("player-combat-cooldown", { query: ["Player", "Weapon"], writes: ["Weapon"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Weapon._cd_t > 0) {
      e.Weapon._cd_t = Math.max(0, e.Weapon._cd_t - ctx.dt);
    }
  }
});

vitric.fn("player_attack", (a, ctx) => {
  // a.px, a.py: player position; a.weapon_damage, a.weapon_range, a.weapon_cd: weapon stats
  // Read Weapon._cd_t via ctx.getField (the rule passes player position + weapon stats).
  const cdT = ctx.getField("@player", "Weapon._cd_t") || 0;
  if (cdT > 0) return; // still on cooldown
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  let snapshot;
  try { snapshot = JSON.parse(raw); } catch (_) { snapshot = []; }
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  let bestD2 = Infinity, best = null;
  for (const enemy of snapshot) {
    const dx = enemy.x - a.px, dy = enemy.y - a.py;
    const d2 = dx * dx + dy * dy;
    if (d2 < bestD2) { bestD2 = d2; best = enemy; }
  }
  if (!best) return;
  const range = a.weapon_range || 2;
  if (bestD2 > range * range) return;
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
```

### `turret-auto-attack` system

Query [Structure, Position] where kind=="turret". For each turret, find nearest enemy in range, apply damage on cooldown (Structure._cd_t).

```javascript
const TURRET_RANGE = 5;
const TURRET_DAMAGE = 8;
const TURRET_COOLDOWN = 1.5;

vitric.system("turret-auto-attack", { query: ["Structure", "Position"], writes: ["Structure"] }, (entities, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  let snapshot;
  try { snapshot = JSON.parse(raw); } catch (_) { snapshot = []; }
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  for (const e of entities) {
    if (e.Structure.kind !== "turret") continue;
    // Decrement cooldown.
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
```

### `guard-auto-defense` system

Query [Companion, Persona, Need, Position]. For each companion with role=guard + affinity>=60, find nearest enemy in range 2, apply damage on Need.contribution_timer cooldown (reuse existing field).

```javascript
const GUARD_RANGE = 2;
const GUARD_DAMAGE = 6;

vitric.system("guard-auto-defense", { query: ["Companion", "Persona", "Need", "Position"], writes: ["Need"] }, (entities, ctx) => {
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  let snapshot;
  try { snapshot = JSON.parse(raw); } catch (_) { snapshot = []; }
  if (!Array.isArray(snapshot) || snapshot.length === 0) return;
  for (const e of entities) {
    if (e.Persona.role !== "guard") continue;
    if ((e.Need.affinity || 0) < 60) continue;
    // Reuse contribution_timer as attack cooldown (it's already declared + decremented elsewhere).
    // Actually — contribution_timer is for resource contribution, not combat. Use a separate cooldown.
    // Simpler: apply damage every tick (no cooldown for guard — guards are melee, dmg 6/tick = 360/s is too fast).
    // Use ctx.tick modulo: attack every 30 ticks (0.5s).
    // Actually that breaks determinism if ctx.tick skips. Use Need.leave_timer? No, that's for departure.
    // Add a new field _combat_cd to Need? That's a schema change.
    // Simplest: apply damage * dt (continuous, like enemy-attack-player).
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
```

### `player-respawn-check` system

Query [Player, Hp, Position]. If Hp.value <= 0 → teleport to (7,7), restore Hp, apply -20% food penalty.

```javascript
vitric.system("player-respawn-check", { query: ["Player", "Hp", "Position"], writes: ["Hp", "Position"] }, (entities, ctx) => {
  for (const e of entities) {
    if ((e.Hp.value || 0) > 0) continue;
    // Respawn.
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
```

### `enemy-cleanup` system

At `dawn-break`, despawn all remaining enemies (they retreat). Query [Enemy], despawn each.

```javascript
// Dawn-break despawns all enemies (they retreat to wild).
// Implemented as a fn called by rule on dawn-break, since systems can't listen to events.
vitric.fn("retreat_all_enemies", (a, ctx) => {
  // Iterate Colony.enemy_snapshot to get enemy IDs, despawn each.
  const raw = ctx.getField("colony", "Colony.enemy_snapshot") || "[]";
  let snapshot;
  try { snapshot = JSON.parse(raw); } catch (_) { snapshot = []; }
  if (!Array.isArray(snapshot)) return;
  for (const enemy of snapshot) {
    if (enemy && enemy.id) ctx.despawn(enemy.id);
  }
  ctx.emit("enemies-retreated", { count: snapshot.length });
});
```

## Script: `games/frontier/scripts/economy.js` (MODIFY)

### Add Hp to spawned structures

In the `build` fn, add `Hp` to the `comps` object based on tier:

```javascript
const tierHp = STRUCTURE_HP_BY_TIER[def.tier] || 50;
const comps = {
  Structure: { kind: a.kind, tier: def.tier || 1, _cd_t: 0 },
  // ... existing components ...
  Hp: { value: tierHp, max: tierHp }
};
```

Add `STRUCTURE_HP_BY_TIER` constant at the top of `economy.js`:

```javascript
const STRUCTURE_HP_BY_TIER = { 1: 50, 2: 100, 3: 200 };
```

(Or import from combat.js — but QuickJS shared global means it's accessible if combat.js is loaded first. To be safe, declare in both files OR declare only in combat.js and ensure load order. The vitric.json `scripts` array determines load order — combat.js should come BEFORE economy.js. But to avoid coupling, declare in both — small DRY violation, acceptable.)

Actually — simpler: declare `STRUCTURE_HP_BY_TIER` only in `combat.js`, and have `economy.js` reference it via the shared global. Add a comment in economy.js noting the dependency. Update `vitric.json` to load `combat.js` before `economy.js`.

## Script: `games/frontier/scripts/companion.js` (MODIFY)

### Consume `guard-patrol` events (optional)

Task 9's `companion-contribution` guard case emits `guard-patrol`. The new `guard-auto-defense` system in `combat.js` handles the actual combat. The `guard-patrol` event is currently a no-op hook — Task 10 doesn't need to consume it (the system reads enemy positions directly). Leave the event emission as-is (forward-compat for future tasks that might want to track guard activity).

**No changes to companion.js needed** — the `guard-auto-defense` system in `combat.js` handles guard combat independently.

## Rules: `games/frontier/rules/combat.json` (NEW)

### `night-fall-spawn-wave` rule

```json
{
  "id": "night-fall-spawn-wave",
  "comment": "night-fall{threat} → spawn_wave fn (waves scale with threat + region count).",
  "on": { "event": "night-fall" },
  "do": [
    { "call": "spawn_wave", "with": {
      "threat": "event.threat",
      "day": "@colony.Clock.day"
    } }
  ]
}
```

Wait — `@colony.Clock.day` is wrong. Clock is on the `colony` entity? Let me check. Looking at schema: `Clock` is a component with `day` field. The colony entity has Clock. So `@colony.Clock.day` should work.

Actually, let me verify — looking at existing rules. Let me grep for `@colony.Clock`.

(Check the existing rules — the brief can specify `@colony.Clock.day` and the implementer verifies it works.)

### `dawn-break-retreat` rule

```json
{
  "id": "dawn-break-retreat",
  "comment": "dawn-break → retreat_all_enemies fn (despawn all enemies).",
  "on": { "event": "dawn-break" },
  "do": [
    { "call": "retreat_all_enemies", "with": {} }
  ]
}
```

### `combat-click` rule

```json
{
  "id": "combat-click",
  "comment": "战斗模式下左键 → player_attack fn(找最近敌人,在武器范围内则挥击)。",
  "on": { "event": "mouse" },
  "if": [ ["@uistate.Mode.value", "==", "combat"] ],
  "do": [
    { "call": "player_attack", "with": {
      "px": "@colony.Colony.player_x", "py": "@colony.Colony.player_y",
      "weapon_damage": "@player.Weapon.damage",
      "weapon_range": "@player.Weapon.range",
      "weapon_cd": "@player.Weapon.cooldown"
    } }
  ]
}
```

### `enemy-killed-loot` rule

```json
{
  "id": "enemy-killed-loot",
  "comment": "enemy-killed{loot} → add loot to Inventory via inv-set emit.",
  "on": { "event": "enemy-killed" },
  "do": [
    { "call": "apply_loot", "with": {
      "loot": "event.loot",
      "ore": "@player.Inventory.ore", "wood": "@player.Inventory.wood", "fiber": "@player.Inventory.fiber",
      "seed": "@player.Inventory.seed", "wheat": "@player.Inventory.wheat", "plank": "@player.Inventory.plank",
      "chair": "@player.Inventory.chair", "lamp": "@player.Inventory.lamp",
      "hide": "@player.Inventory.hide", "crystal_core": "@player.Inventory.crystal_core"
    } }
  ]
}
```

This requires an `apply_loot` fn in `combat.js`:

```javascript
vitric.fn("apply_loot", (a, ctx) => {
  let loot;
  try { loot = typeof a.loot === "string" ? JSON.parse(a.loot) : a.loot; } catch (_) { loot = {}; }
  if (!loot || typeof loot !== "object") return;
  const inv = {};
  const items = ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp", "hide", "crystal_core"];
  for (const k of items) inv[k] = a[k] | 0;
  for (const k in loot) {
    if (items.indexOf(k) >= 0) inv[k] = (inv[k] || 0) + (loot[k] | 0);
  }
  ctx.emit("inv-set", inv);
});
```

**Note**: the `inv-set` event is already consumed by rules in `economy.json` (the existing inventory write-back rules). No new rule needed for the write-back — just emit `inv-set` with the full inventory dict.

Wait — let me re-check. Looking at `economy.json`, the `inv-apply` rule consumes `inv-set`:

(Grep for `inv-set` in rules — if there's an existing write-back rule, `apply_loot` just emits `inv-set` and the existing rule applies it. If not, need to add one.)

The implementer should verify this. If no `inv-apply` rule exists, add one to `combat.json`:

```json
{
  "id": "inv-apply",
  "comment": "inv-set{...} → 写回 @player.Inventory.*(全量绝对值,幂等)。",
  "on": { "event": "inv-set" },
  "do": [
    { "set": "@player.Inventory.ore",        "to": "event.ore" },
    { "set": "@player.Inventory.wood",       "to": "event.wood" },
    { "set": "@player.Inventory.fiber",      "to": "event.fiber" },
    { "set": "@player.Inventory.seed",       "to": "event.seed" },
    { "set": "@player.Inventory.wheat",      "to": "event.wheat" },
    { "set": "@player.Inventory.plank",      "to": "event.plank" },
    { "set": "@player.Inventory.chair",      "to": "event.chair" },
    { "set": "@player.Inventory.lamp",       "to": "event.lamp" },
    { "set": "@player.Inventory.hide",       "to": "event.hide" },
    { "set": "@player.Inventory.crystal_core", "to": "event.crystal_core" }
  ]
}
```

But this rule likely already exists in `economy.json` (Task 8 added hide/crystal_core to it). The implementer should check — if it exists, DON'T duplicate. If it doesn't exist, add to `combat.json`.

## Rules: `games/frontier/rules/ui.json` (MODIFY)

### Add `mode-combat` rule

```json
{
  "id": "mode-combat",
  "comment": "切战斗模式:藏所有菜单(同 interact 模式)。",
  "on": { "event": "ui-activate", "filter": { "action": "mode-combat" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "combat" },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 },
    { "set": "@tech_menu.Ui.ox", "to": -3000 }
  ]
}
```

### Add `kb-mode-combat` rule

```json
{
  "id": "kb-mode-combat",
  "comment": "F 键切战斗模式。",
  "on": { "event": "input", "filter": { "action": "f", "phase": "pressed" } },
  "do": [
    { "set": "@uistate.Mode.value", "to": "combat" },
    { "set": "@build_menu.Ui.ox", "to": -3000 },
    { "set": "@craft_menu.Ui.ox", "to": -3000 },
    { "set": "@tech_menu.Ui.ox", "to": -3000 }
  ]
}
```

### Extend existing mode rules to hide `mode_combat` (NOT needed)

`mode_combat` is a button, not a menu — it stays visible in all modes (part of `mode_row`). No need to hide it.

## Rules: `games/frontier/rules/hud.json` (MODIFY)

### Add `hud-hp` rule

```json
{
  "id": "hud-hp",
  "comment": "HP HUD:显示玩家当前 HP/MaxHp。",
  "on": "tick",
  "do": [
    { "set": "@hp_lbl.UiLabel.content", "to": { "format": "HP {}/{}", "args": ["@player.Hp.value", "@player.Hp.max"] } }
  ]
}
```

## Rules: `games/frontier/rules/affordability.json` (MODIFY)

### Extend `build-dim-all` to include `mode_combat` button

The existing `build-dim-all` rule dims all build buttons when materials are insufficient. The `mode_combat` button doesn't need affordability dimming (it's a mode switch, not a build action). **Skip this modification** — `mode_combat` should always be enabled.

## Manifest: `games/frontier/vitric.json` (MODIFY)

Add `combat.js` to `scripts` array (BEFORE `economy.js` so `STRUCTURE_HP_BY_TIER` is available). Add `combat.json` to `rules` array.

```json
"rules": [
  "rules/move.json",
  "rules/ui.json",
  "rules/economy.json",
  "rules/colony.json",
  "rules/hud.json",
  "rules/quest.json",
  "rules/farm.json",
  "rules/companion.json",
  "rules/time.json",
  "rules/narrative.json",
  "rules/toast.json",
  "rules/flare.json",
  "rules/poi.json",
  "rules/affordability.json",
  "rules/wish.json",
  "rules/research.json",
  "rules/combat.json"
],
"scripts": [
  "scripts/colony.js",
  "scripts/combat.js",
  "scripts/economy.js",
  "scripts/crops.js",
  "scripts/companion.js",
  "scripts/clock.js",
  "scripts/hud.js",
  "scripts/toast.js",
  "scripts/flare.js",
  "scripts/poi.js",
  "scripts/wish.js",
  "scripts/research.js"
]
```

## Asset: `games/frontier/assets/enemy.png` (NEW)

Copy `rock.png` to `enemy.png`:

```bash
cp games/frontier/assets/rock.png games/frontier/assets/enemy.png
```

## Tests: `crates/vitric-cli/tests/combat.rs` (NEW)

Follow the pattern of `research.rs` / `companions.rs`. 4 tests:

1. **`enemy_spawns_on_night_fall`**: Boot runtime. Inject `night-fall{threat: 2}` reply. Step 1 tick. Verify `Colony.enemy_snapshot` is non-empty JSON (wave spawned). Or count Enemy entities in world.

2. **`enemy_ai_moves_toward_player`**: Boot runtime. Spawn a test enemy at (15, 15) via `ctx.spawn` (use `Runtime::spawn` or set component on a fresh entity). Step ~30 ticks (0.5s). Verify enemy Position moved closer to player (7, 7).

   **Simpler approach**: Inject `night-fall{threat: 1}`, step 1 tick (spawns wave), then step 30 more ticks (AI moves). Read `Colony.enemy_snapshot`, parse JSON, verify enemies' positions are closer to (7,7) than their spawn positions.

3. **`player_attack_kills_enemy`**: Boot runtime. Inject `night-fall{threat: 1}`, step 1 tick (spawn wave). Read enemy position from snapshot. Set player Position to adjacent to enemy (within weapon range 2). Set Mode.value="combat". Inject mouse click. Step until enemy dies (or 60 ticks = 1s, given weapon damage 10 + cooldown 1s, enemy hp 20 → 2 swings = 2s). Verify enemy no longer in snapshot, verify `enemy-killed` event emitted, verify `Inventory.hide` increased.

   **Simpler approach**: spawn a single enemy adjacent to player via direct component writes, set Mode=combat, inject mouse click, step until dead. Verify loot.

4. **`player_respawns_on_death`**: Boot runtime. Set `player.Hp.value = 0`. Step 1 tick. Verify `player.Hp.value == 100`, `player.Position.x == 7`, `player.Position.y == 7`. Verify `player-respawned` event emitted.

**Test setup notes** (from research.rs/companions.rs pattern):
- `Runtime::boot(frontier_dir())` loads the full scene + logic.
- `frontier_dir()` helper: `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")`.
- To set component fields: `sim.world.set_field(id, "Hp.value", json!(0))`.
- To advance time: `sim.step(&mut rt)` (1 tick per call).
- To inject events: `sim.inject_reply("night-fall", json!({"threat": 2}))`.
- To read entity state: `sim.world.get_field(id, "Hp.value")`.
- To drain events: `rt.drain_observed()`.
- Keep tests fast: 1-60 ticks each.

For counting Enemy entities: check if `sim.world` has an iterator, or use `Colony.enemy_snapshot` (JSON text) parsed in the test.

## Verification

```bash
# Schema check (must exit 0)
~/.cargo/bin/cargo run --release -- check games/frontier

# New combat tests (4 must pass)
~/.cargo/bin/cargo test -p vitric-cli --test combat

# Regression: research (4) + seasons (4) + companions (4) + region (14) still pass
~/.cargo/bin/cargo test -p vitric-cli --test research --test seasons --test companions
~/.cargo/bin/cargo test -p vitric-cli --test region -- --skip typescript

# Workspace all-green
~/.cargo/bin/cargo test --workspace -- --skip typescript

# Gate EXPECTED-FAIL (ReplayDiverged at tick 0 — Hp/Weapon on player + hp_lbl HUD entity + mode_combat button change tick-0 world hash)
# DO NOT re-record qa/clear.json — Task 15 handles that.
~/.cargo/bin/cargo run --release -- gate games/frontier 2>&1 | tail -5
```

## Commit

```bash
git add games/frontier/schema.json games/frontier/scenes/main.json \
        games/frontier/scripts/combat.js games/frontier/scripts/economy.js \
        games/frontier/rules/combat.json games/frontier/rules/ui.json games/frontier/rules/hud.json \
        games/frontier/vitric.json games/frontier/assets/enemy.png \
        crates/vitric-cli/tests/combat.rs
git commit -m "feat(frontier): combat system — Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn"
git push origin main
```

## Critical reminders (project-wide rules)

1. **All code comments must be English** (// and /* */ in JS, // in Rust). String literals (toast text, UI labels, game content) keep their original language (Chinese is OK).
2. **Every field read by a rule (`@entity.Comp.field`) MUST be declared in `schema.json`**. This task adds: `Hp.value/max`, `Enemy.kind/damage/aggro_range/home_region/_attack_cd`, `Weapon.kind/damage/range/cooldown/_cd_t`, `Guard.post_x/post_y/patrol_r`, `Structure._cd_t`, `Mode.value` (combat variant), `Colony.enemy_snapshot`, `Colony.last_threat`. Audit before committing.
3. **Every field accessed via `ctx.getField` / `ctx.setField` MUST be declared**. Audit all `ctx.getField` / `ctx.setField` calls in `combat.js` and the modified `economy.js`:
   - `ctx.getField("colony", "Colony.player_x/y")` — pre-existing
   - `ctx.getField("colony", "Colony.enemy_snapshot")` — NEW, must declare
   - `ctx.getField("mountain", "Region.discovered")` — pre-existing (Task 1)
   - `ctx.getField("@player", "Hp.value")` — NEW, must declare
   - `ctx.getField("@player", "Weapon._cd_t")` — NEW, must declare
   - `ctx.setField("@player", "Hp.value")` — NEW, must declare
   - `ctx.setField("@player", "Weapon._cd_t")` — NEW, must declare
   - `ctx.setField(best.id, "Hp.value")` — NEW, must declare
   - `ctx.getField(best.id, "Enemy.kind")` — NEW, must declare
   - `ctx.getField(best.id, "Hp.value")` — NEW, must declare
   - `ctx.setField("colony", "Colony.food")` — pre-existing
4. **Rule engine format**: `{call: "fnName", with: {args}}` for fn calls; `{set: "path", to: value}` for field writes; `{emit: "eventName", data: {...}}` for events.
5. **`@player` entity-name reference is valid in ctx.getField/setField** — verified in Task 9 (existing pattern).
6. **`STRUCTURE_HP_BY_TIER` shared global**: declared in `combat.js`, referenced in `economy.js`. Ensure `combat.js` loads BEFORE `economy.js` in `vitric.json`. Add a comment in `economy.js` noting the dependency.
7. **`inv-set` event**: check if `economy.json` already has an `inv-apply` rule that consumes `inv-set` and writes back to `@player.Inventory.*`. If yes, `apply_loot` just emits `inv-set` and the existing rule applies it. If no, add `inv-apply` to `combat.json`.
8. **Gate EXPECTED-FAIL is OK** — do NOT re-record `qa/clear.json`. Task 15 handles it.
9. **Don't implement sandbeast** — Task 13 handles desert region enemies. Only gnawer + raider in Task 10.
10. **Don't implement weapon switching UI** — single weapon (stone_axe) only. Spear/arc-gun deferred.
11. **`mode_row` width**: if adding `mode_combat` button causes overflow (6 buttons × 92 + 5 gaps × 6 = 582 > current 386), bump `mode_row.Ui.w` to 582. Verify via scene check.

## Deliverable

Return a report at `.superpowers/sdd/briefs/task-10-report.md` with:
- Commit hash
- Files changed (count + list)
- Test results (combat N/N, research 4/4, seasons 4/4, companions 4/4, region 14/14, schema check exit 0, workspace all-green, gate failure mode)
- Deviations from this brief (with reasoning)
- Concerns / known issues
- Schema field audit result (PASS/FAIL with list of new fields verified declared)
- Self-audit checklist result (from `.superpowers/sdd/review-checklist.md`)

Do NOT update `progress.md` — the controller does that after review.
