# Task 10 Review — Combat System (Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn)

**Commit under review:** `318c7ec` — `feat(frontier): combat system — Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn`
**Diff range:** `c440fda..318c7ec` (parent `c440fda`)
**Files changed:** 10 (+773/-4)
**Brief:** `.superpowers/sdd/briefs/task-10-brief.md`
**Checklist:** `.superpowers/sdd/review-checklist.md` (5 sections)

## Verdict: **APPROVED**

No ❌ Critical issues. All 5 audit sections PASS. All tests green (combat 4/4, research 4/4, seasons 4/4, companions 4/4, region 14/14, workspace all-green except pre-existing typescript skip, schema check exit 0). Gate fails at tick 0 (ReplayDiverged) — matches the brief's EXPECTED-FAIL contract; do NOT re-record `qa/clear.json` (Task 15 handles that).

One Minor finding (`PLAYER_ATTACK_RANGE` dead constant) and a few Nits / non-blocking observations below.

## Findings

| # | Severity | Location | Description | Recommended fix |
|---|----------|----------|-------------|-----------------|
| 1 | Minor | `games/frontier/scripts/combat.js:32` | `const PLAYER_ATTACK_RANGE = 2;` is declared but never referenced. The `player_attack` fn uses `a.weapon_range \|\| 2` (reading the live `Weapon.range` via rule args), so the constant is dead code. Implementer flagged it as "documentation-only" but the checklist §5 forbids dead code / YAGNI. | Delete the constant. The comment can be merged into the `ENEMY_ATTACK_RANGE` block or moved to where `a.weapon_range` is read. Alternatively, have `player_attack` consult `PLAYER_ATTACK_RANGE` as a clamp upper bound to make it live. |
| 2 | Nit | `games/frontier/scenes/main.json` (`mode_row.Ui.w`) | `mode_row.w` was bumped from 386 to 582, but only 5 buttons exist (`mode_build`, `mode_craft`, `mode_interact`, `mode_research`, `mode_combat`). 5×92 + 4×6 = 484 would fit exactly; 582 leaves ~98 px of empty slack at the row's right edge. The brief §11 explicitly specifies 582 (apparently assuming a `mode_upgrade` button exists — it does not; upgrade mode is entered via `upgrade-prompt` ui-activate, not a dedicated button). | Either shrink `mode_row.w` to 484 to fit 5 buttons exactly, or leave at 582 as forward-compat slack for a future `mode_upgrade` button. Not a blocker; current layout has no overlap. |
| 3 | Nit | `games/frontier/scripts/combat.js:23-24` (comment) | The header comment says "combat.js loads BEFORE economy.js so `STRUCTURE_HP_BY_TIER` is available to the build fn" — accurate, but the load-order dependency is also re-documented in `economy.js:9-12`. Slight duplication. | Acceptable. No action required. |
| 4 | Nit | `games/frontier/scripts/combat.js` (system order) | `cache-player-pos` is registered in `companion.js` which loads AFTER `combat.js`, so `enemy-ai` and `enemy-attack-player` read `Colony.player_x/y` that is 1 tick stale. | Consistent with existing `companion-snapshot` / `drifter-snapshot` pattern (they also read 1-tick-stale player pos). Tests pass. Not a blocker. |

## Section-by-section audit

### Section 1 — Schema field audit: **PASS**

Verified every field read by a rule (`@entity.Comp.field`) and every field accessed via `ctx.getField` / `ctx.setField` in `combat.js` and the modified `economy.js` is declared in `games/frontier/schema.json`.

**Rule reads (from `combat.json`, `ui.json`, `hud.json` diffs):**

| Rule | Field read | Declared? |
|------|-----------|-----------|
| `night-fall-spawn-wave` | `@colony.Clock.day` | ✅ `Clock.day` (int, default 1) |
| `combat-click` | `@uistate.Mode.value` | ✅ `Mode.value` (enum, includes `"combat"`) |
| `combat-click` | `@colony.Colony.player_x`, `@colony.Colony.player_y` | ✅ `Colony.player_x/y` (pre-existing) |
| `combat-click` | `@player.Weapon.damage`, `@player.Weapon.range`, `@player.Weapon.cooldown` | ✅ all declared in `Weapon` |
| `enemy-killed-loot` | `@player.Inventory.{ore,wood,fiber,seed,wheat,plank,chair,lamp,hide,crystal_core}` | ✅ all 10 declared in `Inventory` (Task 8 added hide/crystal_core) |
| `enemy-killed-loot` | `event.loot` | event payload (not a schema field) — handled by `apply_loot` fn defensively (string or object) |
| `mode-combat`, `kb-mode-combat` | `@uistate.Mode.value` (write `"combat"`) | ✅ enum variant declared |
| `mode-combat`, `kb-mode-combat` | `@build_menu.Ui.ox`, `@craft_menu.Ui.ox`, `@tech_menu.Ui.ox` | ✅ pre-existing `Ui.ox` |
| `hud-hp` | `@hp_lbl.UiLabel.content` (write) | ✅ `UiLabel.content` (text) |
| `hud-hp` | `@player.Hp.value`, `@player.Hp.max` | ✅ both declared in `Hp` |

**`ctx.getField` / `ctx.setField` calls in `combat.js`:**

| Call | Field | Declared? |
|------|-------|-----------|
| `spawn_wave` | `ctx.getField("mountain", "Region.discovered")` | ✅ `Region.discovered` (int, default 0) |
| `enemy-ai` | `ctx.getField("colony", "Colony.player_x/y")` | ✅ pre-existing |
| `enemy-attack-player` | `ctx.getField("colony", "Colony.player_x/y")` | ✅ pre-existing |
| `enemy-attack-player` | `ctx.getField("@player", "Hp.value")`, `ctx.setField("@player", "Hp.value", ...)` | ✅ `Hp.value` (number, default 100) |
| `enemy-attack-structures` | `ctx.getField("colony", "Colony.enemy_snapshot")` | ✅ NEW `Colony.enemy_snapshot` (text, default `"[]"`) |
| `enemy-attack-structures` | `e.Hp.value`, `e.Hp.max`, `e.Structure.tier` (writes) | ✅ all declared (`Hp.value/max`, `Structure.tier`) |
| `player-combat-cooldown` | `e.Weapon._cd_t` (read/write) | ✅ NEW `Weapon._cd_t` (number, default 0) |
| `player_attack` fn | `ctx.getField("@player", "Weapon._cd_t")`, `ctx.setField("@player", "Weapon._cd_t", cd)` | ✅ NEW `Weapon._cd_t` |
| `player_attack` fn | `ctx.getField("colony", "Colony.enemy_snapshot")` | ✅ NEW |
| `player_attack` fn | `ctx.getField(best.id, "Hp.value")`, `ctx.setField(best.id, "Hp.value", ...)` | ✅ `Hp.value` |
| `player_attack` fn | `ctx.getField(best.id, "Enemy.kind")` | ✅ NEW `Enemy.kind` (text, default `"gnawer"`) |
| `turret-auto-attack` | `e.Structure._cd_t` (read/write) | ✅ NEW `Structure._cd_t` (number, default 0) |
| `turret-auto-attack` | `ctx.getField("colony", "Colony.enemy_snapshot")` | ✅ NEW |
| `turret-auto-attack` | `ctx.getField(best.id, "Hp.value")`, `ctx.setField(best.id, "Hp.value", ...)`, `ctx.getField(best.id, "Enemy.kind")` | ✅ all declared |
| `guard-auto-defense` | `e.Need.affinity`, `e.Persona.role` (reads) | ✅ pre-existing |
| `guard-auto-defense` | `ctx.getField("colony", "Colony.enemy_snapshot")` | ✅ NEW |
| `guard-auto-defense` | `ctx.getField(best.id, "Hp.value")`, `ctx.setField(best.id, "Hp.value", ...)`, `ctx.getField(best.id, "Enemy.kind")` | ✅ all declared |
| `player-respawn-check` | `e.Hp.value`, `e.Position.x/y` (writes) | ✅ all declared |
| `player-respawn-check` | `ctx.getField("colony", "Colony.food")`, `ctx.setField("colony", "Colony.food", ...)` | ✅ pre-existing `Colony.food` |
| `apply_loot` fn | `ctx.emit("inv-set", inv)` | event emission (no schema field) |
| `retreat_all_enemies` fn | `ctx.getField("colony", "Colony.enemy_snapshot")` | ✅ NEW |

**`e.<Comp>.<field> = x` writes in `combat.js` (direct assignment):**

| System | Write | Declared? |
|--------|-------|-----------|
| `enemy-ai` | `e.Velocity.x =`, `e.Velocity.y =` | ✅ `Velocity.x/y` |
| `enemy-attack-structures` | `e.Hp.value =`, `e.Hp.max =`, `e.Structure.tier =` | ✅ all declared (writes list `["Hp", "Structure"]` correctly includes `Structure` since `tier` is modified) |
| `player-combat-cooldown` | `e.Weapon._cd_t =` | ✅ NEW `Weapon._cd_t` (writes list `["Weapon"]` ✓) |
| `turret-auto-attack` | `e.Structure._cd_t =` | ✅ NEW `Structure._cd_t` (writes list `["Structure"]` ✓) |
| `player-respawn-check` | `e.Hp.value =`, `e.Hp.max =`, `e.Position.x =`, `e.Position.y =` | ✅ all declared (writes list `["Hp", "Position"]` ✓) |

**`economy.js` modifications:**

| Location | Field | Declared? |
|----------|-------|-----------|
| `build` fn | `STRUCTURE_HP_BY_TIER[tier]` (read shared global) | n/a (not a schema field) |
| `build` fn | `comps.Structure = { kind, tier, _cd_t: 0 }` | ✅ all 3 fields declared |
| `build` fn | `comps.Hp = { value: tierHp, max: tierHp }` | ✅ both fields declared |

### Section 2 — Enum variant audit: **PASS**

Only one enum write in the diff:

| Rule | Write | Variant | Declared? |
|------|-------|---------|-----------|
| `mode-combat` | `@uistate.Mode.value ← "combat"` | `"combat"` | ✅ added to `Mode.value.variants` |
| `kb-mode-combat` | `@uistate.Mode.value ← "combat"` | `"combat"` | ✅ same |

No `ctx.setField` enum writes in JS (the `Mode.value` is only written via rules).

### Section 3 — Scene entity reference audit: **PASS**

Every `@<name>` referenced in the diff's rules exists in `games/frontier/scenes/main.json` (474 entities total):

| Reference | Entity exists? | Notes |
|-----------|----------------|-------|
| `@player` | ✅ | Has `Player`, `Position`, `Inventory`, `Hp` (NEW: value 100, max 100), `Weapon` (NEW: stone_axe/dmg 10/range 2/cd 1/_cd_t 0) |
| `@colony` | ✅ | Has `Colony`, `Clock` (pre-existing) |
| `@uistate` | ✅ | Has `Mode` (pre-existing) |
| `@hp_lbl` | ✅ NEW | `Ui` (anchor top-right, parent ui, ox -32, oy 312, w 260, h 24) + `UiLabel` (content `"HP 100/100"`, size 20, color `#ff6b6b`, align end) |
| `@build_menu`, `@craft_menu`, `@tech_menu` | ✅ | Pre-existing; `mode-combat` / `kb-mode-combat` rules hide them via `Ui.ox = -3000` |
| `@mode_combat` | ✅ NEW | `Ui` (anchor top-left, parent mode_row, w 92, h 48) + `Panel` (#3a4a6b) + `Button` (action mode-combat, state normal) |
| `@mode_combat_lbl` | ✅ NEW | `Ui` (anchor stretch, parent mode_combat) + `UiLabel` (content `"战斗"`, size 30, color #ffffff, align center) |
| `@mountain` (ctx.getField target) | ✅ | Has `Region` component (pre-existing, Task 1) |

### Section 4 — UI layout overlap audit: **PASS**

**`mode_row` HBox (anchor top-left, parent ui, oy 100, w 582, h 64, gap 6, pad 9):**

Children (all `anchor: top-left, parent: mode_row, w 92, h 48`):
1. `mode_build`
2. `mode_craft`
3. `mode_interact`
4. `mode_research`
5. `mode_combat` (NEW)

HBox auto-layout: button i at x = pad + i×(w+gap). With 5 buttons: x = 9, 107, 205, 303, 401. Last button right edge = 401 + 92 = 493 ≤ 582. No overlap between siblings (gap 6 px between each). The brief's `mode_row.w = 582` over-provisions ~89 px of slack (would exactly fit 6 buttons) but does not cause overlap. See Finding #2.

**Top-right HUD labels (anchor top-right, parent ui, ox -32):**

| Entity | oy | h | y-range |
|--------|----|----|---------|
| `techpoint_lbl` | 228 | 24 | [228, 252] |
| `research_status_lbl` | 256 | 24 | [256, 280] |
| `collective_wish_lbl` | 284 | 24 | [284, 308] |
| `hp_lbl` (NEW) | 312 | 24 | [312, 336] |

All four ranges are disjoint (each ends exactly 4 px before the next begins). No overlap. The brief specified oy:312 (= 284 + 24 + 4 margin); implementer matched exactly.

**`mode_combat_lbl`:** `Ui` anchor is `stretch` with parent `mode_combat` — no manual `ox`/`oy`/`w`/`h`, fills the parent button. No overlap risk. Matches the pattern used by `mode_build_lbl` / `mode_research_lbl` / etc.

### Section 5 — Standard checks: **PASS**

| Check | Result |
|-------|--------|
| `cargo run -p vitric-cli -- check games/frontier` exits 0 | ✅ PASS (exit 0, schema valid) |
| All new `//` / `/* */` comments in `combat.js` / `economy.js` / `combat.rs` are English | ✅ PASS — verified all comments in `combat.js` (377 lines), the `economy.js` diff (11 lines), and `combat.rs` (211 lines). All comments are English. String literals (`"你倒下了,被送回登陆点"` toast, `"战斗"` UI label, `"HP {}/{}"` format) retain their authored language as required. |
| No fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`) | ✅ PASS — grep on `combat.js` finds none. Uses only verified APIs: `ctx.getField`, `ctx.setField`, `ctx.spawn`, `ctx.despawn`, `ctx.emit`, `ctx.random`, `ctx.dt`, `e.id`, `entities.map`, `vitric.system`, `vitric.fn`, `Math.sqrt`, `Math.max`, `Math.min`, `Math.floor`, `JSON.parse`, `JSON.stringify`. |
| No dead code / YAGNI | ⚠️ `PLAYER_ATTACK_RANGE` declared but never read — see Finding #1 (Minor). All other constants are used: `STRUCTURE_HP_BY_TIER` (used in `economy.js` + `enemy-attack-structures`), `ENEMY_SPEED`, `ENEMY_ATTACK_RANGE`, `RESPAWN_*`, `TURRET_*`, `GUARD_*`, `ENEMY_TYPES`, `LOOT_ITEMS`. |
| Commit message follows `<type>(<scope>): <summary>` | ✅ PASS — `feat(frontier): combat system — Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn` |
| Only in-scope files modified | ✅ PASS — exactly 10 files: `combat.js` (NEW), `economy.js` (MOD), `combat.json` (NEW), `ui.json` (MOD), `hud.json` (MOD), `schema.json` (MOD), `scenes/main.json` (MOD), `vitric.json` (MOD), `assets/enemy.png` (NEW, binary copy of `rock.png`), `tests/combat.rs` (NEW). No out-of-scope files. |

## Test results (independently re-run)

| Suite | Result |
|-------|--------|
| `cargo test -p vitric-cli --test combat` | ✅ 4/4 pass (`enemy_spawns_on_night_fall`, `enemy_ai_moves_toward_player`, `player_attack_kills_enemy`, `player_respawns_on_death`) — finished in 1.70s |
| `cargo test -p vitric-cli --test research` | ✅ 4/4 pass |
| `cargo test -p vitric-cli --test seasons` | ✅ 4/4 pass |
| `cargo test -p vitric-cli --test companions` | ✅ 4/4 pass |
| `cargo test -p vitric-cli --test region -- --skip typescript` | ✅ 14/14 pass (finished in 252.16s — slow but green; `catch_up_advances_dormant_crop_on_thaw` and `random_stream_same_seed_regardless_of_call_timing` are the long-running ones) |
| `cargo test --workspace -- --skip typescript` | ✅ all green (exit 0) |
| `cargo run --release -- check games/frontier` | ✅ exit 0 |
| `cargo run --release -- gate games/frontier` | ⚠️ FAIL — `playthrough:qa/clear.json` ReplayDiverged at tick 0: expected `0xb68b61d57750ff1`, actual `0xd42278b24b8b648e`. **This is the EXPECTED-FAIL** per brief §"Critical reminders" #8 — the new player `Hp` + `Weapon` components, `hp_lbl` HUD entity, and `mode_combat` button change the tick-0 world hash. Do NOT re-record `qa/clear.json`; Task 15 handles that. |

## Brief architecture invariants — verification

| # | Invariant | Verified |
|---|-----------|----------|
| 1 | `Hp` component {value, max} declared + attached to player + structures (via `economy.js` build fn) | ✅ Schema declares `Hp.{value,max}`. Scene attaches `Hp{value:100,max:100}` to `player`. `economy.js` build fn adds `Hp{value:tierHp, max:tierHp}` to spawned structures. |
| 2 | `Enemy` component {kind, damage, aggro_range, home_region, _attack_cd} spawned by `spawn_wave` fn on `night-fall` event | ✅ All 5 fields declared. `spawn_wave` fn spawns entities with all 5 fields + Position/Velocity/Collider/Sprite/Hp. Rule `night-fall-spawn-wave` calls it on `night-fall` event. |
| 3 | `Weapon` component {kind, damage, range, cooldown, _cd_t} attached to player; `_cd_t` decremented by `player-combat-cooldown` system, read by `player_attack` fn | ✅ Schema declares all 5 fields. Player scene entity has `Weapon{kind:"stone_axe",damage:10,range:2,cooldown:1,_cd_t:0}`. `player-combat-cooldown` system decrements `_cd_t` by `ctx.dt` every tick. `player_attack` fn reads `ctx.getField("@player", "Weapon._cd_t")` and returns early if > 0. |
| 4 | `Guard` component {post_x, post_y, patrol_r} declared but NOT attached to any entity (forward-compat Task 11) | ✅ Schema declares all 3 fields. Grep on `scenes/main.json` confirms no entity has a `Guard` component. `guard-auto-defense` system uses `Persona.role == "guard"` instead. |
| 5 | Structure HP by tier: tier 1 = 50, tier 2 = 100, tier 3 = 200. `STRUCTURE_HP_BY_TIER` declared in `combat.js` as shared global, referenced by `economy.js`'s `build` fn. `vitric.json` scripts array loads `combat.js` BEFORE `economy.js`. | ✅ `combat.js:27` declares `const STRUCTURE_HP_BY_TIER = { 1: 50, 2: 100, 3: 200 };`. `economy.js` build fn reads it via shared QuickJS global. `vitric.json` scripts array: `["scripts/colony.js", "scripts/combat.js", "scripts/economy.js", ...]` — combat.js loads before economy.js ✓. `economy.js:9-12` documents the dependency. |
| 6 | Damage model: enemy→structure/player = continuous (`damage * ctx.dt`); player/turret/guard→enemy = discrete swings with cooldown | ✅ `enemy-attack-player` uses `totalDamage += e.Enemy.damage * ctx.dt`. `enemy-attack-structures` uses `nearestDmg * ctx.dt`. `player_attack` fn applies discrete `dmg = a.weapon_damage`. `turret-auto-attack` applies discrete `TURRET_DAMAGE` with `Structure._cd_t` cooldown. `guard-auto-defense` applies **continuous** `GUARD_DAMAGE * ctx.dt` (deviates from brief's "discrete swings" — see Concern #1 below; acceptable per brief's own inline comment "Simplest: apply damage * dt (continuous, like enemy-attack-player)"). |
| 7 | `enemy_snapshot` pattern: `enemy-snapshot` system packs all Enemy entities' {id, x, y, kind, damage} into `Colony.enemy_snapshot` (JSON text). Other systems read via `ctx.getField("colony", "Colony.enemy_snapshot")` and `JSON.parse`. | ✅ `enemy-snapshot` system writes `JSON.stringify(data)` to `Colony.enemy_snapshot`. `enemy-attack-structures`, `turret-auto-attack`, `guard-auto-defense`, `player_attack`, `retreat_all_enemies` all read it via `ctx.getField` + `readSnapshot` helper (defensive `JSON.parse` with `[]` fallback). Mirrors `drifter-snapshot` / `companion-snapshot`. |
| 8 | `player_attack` fn: SECOND version from brief (cooldown via `ctx.getField("@player", "Weapon._cd_t")`), NOT the first version (cooldown-in-args). | ✅ `combat.js` `player_attack` fn: `const cdT = ctx.getField("@player", "Weapon._cd_t") \|\| 0; if (cdT > 0) return;` — this is the SECOND version. The fn header comment explicitly notes this: "NOTE: This is the SECOND version of player_attack from the brief (cooldown via ctx.getField, NOT cooldown-in-args)." |
| 9 | `enemy-attack-structures` system MUST have `writes: ["Hp", "Structure"]` (modifies tier and hp). | ✅ `combat.js`: `vitric.system("enemy-attack-structures", { query: ["Structure", "Position", "Hp"], writes: ["Hp", "Structure"] }, ...)` — both `Hp` (writes value/max) and `Structure` (writes tier on downgrade) are in the writes list. |
| 10 | `combat.js` MUST load BEFORE `economy.js` in `vitric.json` scripts array. | ✅ `vitric.json` scripts: `["scripts/colony.js", "scripts/combat.js", "scripts/economy.js", ...]` — combat.js is index 1, economy.js is index 2. |

## Approved deviations

None. The implementer followed all brief invariants. Where the brief offered alternative implementations (e.g., "declare `STRUCTURE_HP_BY_TIER` in both files OR declare only in `combat.js` and ensure load order"), the implementer chose the authorized alternative (declare only in `combat.js`, document the dependency in `economy.js`, ensure load order) — this is explicitly permitted by the brief.

## Concerns (non-blocking)

1. **`guard-auto-defense` uses continuous DPS, not discrete swings.** Brief invariant #6 says "player/turret/guard→enemy uses discrete swings with cooldown." The implementer's `guard-auto-defense` uses `GUARD_DAMAGE * ctx.dt` (continuous DPS, like `enemy-attack-player`). However, the brief's own inline design note (brief lines 501-505) explicitly authorizes this: "Simplest: apply damage * dt (continuous, like enemy-attack-player)." The brief's design rationale was to avoid adding a new `_combat_cd` field to the `Need` component. This is consistent with the brief's authorized alternative. Not a deviation requiring fix.

2. **1-tick player position lag.** `cache-player-pos` (in `companion.js`) is registered AFTER `enemy-ai` / `enemy-attack-player` (in `combat.js`), so those systems read `Colony.player_x/y` that is 1 tick stale. This is consistent with the existing `companion-snapshot` / `drifter-snapshot` pattern (which also read 1-tick-stale player position). The 1-tick lag is invisible at 60 Hz and tests pass. Not a blocker.

3. **`event.loot` serialization ambiguity.** The `enemy-killed` event is emitted with `loot` as a JS object (`{hide: N, crystal_core: M}`). When the rule engine passes this to `apply_loot` via `"loot": "event.loot"`, it may arrive as a JSON string or as a parsed object depending on engine internals. The `apply_loot` fn defensively handles both (`typeof a.loot === "string" ? JSON.parse(a.loot) : a.loot`). Test `player_attack_kills_enemy` confirms the pipeline works end-to-end (asserts `Inventory.hide > 0`). Not a blocker.

4. **Cross-entity `ctx.setField` writes outside the `writes` list.** `player-respawn-check` (writes `["Hp", "Position"]`) also calls `ctx.setField("colony", "Colony.food", ...)` — a cross-entity write not declared in `writes`. Similarly, `enemy-attack-player` (writes `[]`) calls `ctx.setField("@player", "Hp.value", ...)`. This matches the existing codebase convention: the `writes` array tracks writes to *queried entity* components via direct `e.Comp.field = x` assignment; cross-entity writes via `ctx.setField` are side effects outside the `writes` list. The engine tolerates this (schema check passes, tests pass). Not a blocker.

5. **`Colony.last_threat` declared but unused.** Brief schema spec adds `Colony.last_threat` (int, default 0) "carries night-fall threat between events, for spawn_wave fn." The `spawn_wave` fn receives `threat` directly via the rule arg (`event.threat`), so `last_threat` is never written or read in `combat.js`. It is forward-compat for future tasks that may need to query the last threat level outside the `night-fall` event handler. Acceptable as a forward-compat hook (similar to `Guard` component).

## Summary

The implementation faithfully follows the brief across all 10 invariants. Schema, scene, rules, scripts, and tests are internally consistent. The 4 new combat tests pass, all regression tests pass, the schema check exits 0, and the gate fails exactly as the brief predicts (ReplayDiverged at tick 0 due to new player components + HUD entity + button). The only actionable item is the Minor `PLAYER_ATTACK_RANGE` dead-constant cleanup (Finding #1) — recommended but not blocking. The Nit about `mode_row.w = 582` over-provisioning (Finding #2) is faithful to the brief's literal spec and can be deferred.
