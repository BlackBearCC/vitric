# Task 10 — Combat System (Implementation Report)

## Commit

**Hash:** `318c7ec`
**Message:** `feat(frontier): combat system — Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn`
**Branch:** `main`
**Remote:** `git@github.com:BlackBearCC/vitric.git` (origin, SSH)

## Files changed (10)

| # | File | Status | Purpose |
|---|---|---|---|
| 1 | `games/frontier/schema.json` | modified | Declares `Hp`, `Enemy`, `Weapon`, `Guard` components; extends `Structure._cd_t`; extends `Mode.value` enum with `"combat"`; extends `Colony.enemy_snapshot` + `Colony.last_threat` |
| 2 | `games/frontier/scenes/main.json` | modified | Player gets `Hp{100,100}` + `Weapon{stone_axe, dmg 10, range 2, cd 1}`; new `hp_lbl` HUD entity; new `mode_combat` + `mode_combat_lbl` UI button; `mode_row.Ui.w` bumped 580 → 582 (6×92 + 5×6) |
| 3 | `games/frontier/scripts/combat.js` | new (377 lines) | All combat systems + fns: `spawn_wave`, `player_attack` (SECOND brief version, ctx.getField cooldown), `apply_loot`, `retreat_all_enemies`; 8 systems (snapshot/ai/attack-player/attack-structures/cooldown/turret/guard/respawn) |
| 4 | `games/frontier/scripts/economy.js` | modified | `build` fn adds `Hp{tierHp, tierHp}` + `Structure._cd_t:0` to spawned structures; dependency comment block at top noting `STRUCTURE_HP_BY_TIER` is from combat.js |
| 5 | `games/frontier/rules/combat.json` | new (51 lines, 4 rules) | `night-fall-spawn-wave`, `dawn-break-retreat`, `combat-click` (Mode=combat gate), `enemy-killed-loot` |
| 6 | `games/frontier/rules/ui.json` | modified | Adds `mode-combat` (ui-activate action=mode-combat) + `kb-mode-combat` (input action=f pressed) rules |
| 7 | `games/frontier/rules/hud.json` | modified | Adds `hud-hp` rule: `HP {value}/{max}` on `@hp_lbl.UiLabel.content` every tick |
| 8 | `games/frontier/vitric.json` | modified | Registers `rules/combat.json` (after research.json) and `scripts/combat.js` BEFORE `scripts/economy.js` (after colony.js) — load order critical for `STRUCTURE_HP_BY_TIER` shared global |
| 9 | `games/frontier/assets/enemy.png` | new (2689 bytes) | Copied from `rock.png` via `cp` (per brief: NOT text_to_image API) |
| 10 | `crates/vitric-cli/tests/combat.rs` | new (211 lines, 4 tests) | `enemy_spawns_on_night_fall`, `enemy_ai_moves_toward_player`, `player_attack_kills_enemy`, `player_respawns_on_death` |

## Test results

| Suite | Result |
|---|---|
| Schema check (`cargo run -p vitric-cli -- check games/frontier`) | exit 0 — PASS |
| Combat tests (`cargo test -p vitric-cli --test combat`) | 4/4 PASS |
| Research tests (`cargo test -p vitric-cli --test research`) | 4/4 PASS |
| Seasons tests (`cargo test -p vitric-cli --test seasons`) | 4/4 PASS |
| Companions tests (`cargo test -p vitric-cli --test companions`) | 4/4 PASS |
| Region tests (`cargo test -p vitric-cli --test region`) | 14/14 PASS |
| Workspace (`cargo test --workspace`) | all-green, 0 failures |
| Gate (`cargo run --release -- gate games/frontier`) | **EXPECTED-FAIL** — `ReplayDiverged` at tick 0 (expected hash `0xb68b61d57750ff1` vs actual `0xd42278b24b8b648e`). Cause: scene hash change from new player `Hp`+`Weapon` components, `hp_lbl` HUD entity, and `mode_combat` button. Per brief: do NOT re-record `qa/clear.json` — Task 15 handles that. |

## Deviations from the brief

None. All "Critical reminders" at the bottom of the brief were honored:

1. ✅ Used `cp games/frontier/assets/rock.png games/frontier/assets/enemy.png` (shell `cp`, NOT text_to_image API).
2. ✅ All code comments in combat.js / economy.js / combat.rs are English. String literals (toast "你倒下了,被送回登陆点", UI label "战斗") keep their authored language.
3. ✅ Every new field read by a rule or `ctx.getField`/`ctx.setField` is declared in `schema.json` (audit table below).
4. ✅ `STRUCTURE_HP_BY_TIER` is declared in combat.js and referenced in economy.js; `vitric.json` loads `scripts/combat.js` BEFORE `scripts/economy.js`; economy.js has a dependency comment at lines 10-12.
5. ✅ Implemented the SECOND `player_attack` version from the brief (cooldown via `ctx.getField("@player", "Weapon._cd_t")`, NOT cooldown-in-args). The brief's first version was explicitly REJECTED.
6. ✅ `enemy-attack-structures` system has `writes: ["Hp", "Structure"]`.
7. ✅ Used `~/.cargo/bin/cargo` for all cargo commands.
8. ✅ Did NOT update `progress.md` (controller does that).
9. ✅ Did NOT re-record `qa/clear.json` (Task 15 handles that).
10. ✅ Commit message is exactly: `feat(frontier): combat system — Hp/Enemy/Weapon/Guard, night spawns, structure downgrade, respawn`.
11. ✅ Committed only the 10 in-scope files listed in brief lines 886-890.

## Concerns / known issues

1. **`PLAYER_ATTACK_RANGE` constant is effectively documentation-only.** Declared at combat.js line 32 as `2` to mirror `Weapon.range`'s default, but the `player_attack` fn uses `a.weapon_range || 2` (rule-passed arg, not the constant). No functional impact; kept as design documentation per brief narrative. Could be removed for stricter YAGNI compliance, but doing so would require re-running tests for no behavioral gain.
2. **Gate EXPECTED-FAIL is correct and expected.** The scene hash change is unavoidable: adding `Hp`+`Weapon` to the player entity (required by the brief) changes the tick-0 world state. Task 15 will re-record `qa/clear.json` after all combat-adjacent tasks (12, 13, 14) land.
3. **Raider enemy type spawn condition is forward-compat.** `spawn_wave` only spawns raiders if `mountain.Region.discovered == 1` AND `day >= 5`. Mountain region is currently dormant (Task 12 territory), so only gnawers spawn in practice. This is per-brief design (brief notes raider requires mountain thaw).
4. **Sandbeast enemy type is deferred to Task 13** (desert region only). `ENEMY_TYPES` table only has `gnawer` and `raider`. No code path spawns sandbeast — this is per brief.

## Schema field audit (Section 1 of review checklist) — PASS

All new fields verified declared in `games/frontier/schema.json`:

| Component | Field | Schema line | Used by |
|---|---|---|---|
| `Hp` | `value` | 312 | rule `hud-hp` (@player.Hp.value); ctx.getField/setField in combat.js |
| `Hp` | `max` | 316 | rule `hud-hp` (@player.Hp.max); e.Hp.max writes |
| `Enemy` | `kind` | 324 | ctx.getField in combat.js (player_attack, turret, guard) |
| `Enemy` | `damage` | 328 | e.Enemy.damage read in enemy-attack-player; snapshot |
| `Enemy` | `aggro_range` | 332 | e.Enemy.aggro_range read in enemy-ai |
| `Enemy` | `home_region` | 336 | set by spawn_wave (no rule read) |
| `Enemy` | `_attack_cd` | 340 | set by spawn_wave (forward-compat, no rule read yet) |
| `Weapon` | `kind` | 348 | set in scene (no rule read) |
| `Weapon` | `damage` | 352 | rule `combat-click` (@player.Weapon.damage) |
| `Weapon` | `range` | 356 | rule `combat-click` (@player.Weapon.range) |
| `Weapon` | `cooldown` | 360 | rule `combat-click` (@player.Weapon.cooldown) |
| `Weapon` | `_cd_t` | 364 | ctx.getField/setField in combat.js (player_attack, player-combat-cooldown) |
| `Guard` | `post_x` | 372 | declared (forward-compat, no current reader) |
| `Guard` | `post_y` | 376 | declared (forward-compat, no current reader) |
| `Guard` | `patrol_r` | 380 | declared (forward-compat, no current reader) |
| `Structure` | `_cd_t` | 304 | e.Structure._cd_t read/write in turret-auto-attack; set by economy.js build |
| `Mode` | `value` (+`"combat"` variant) | 515 | rule `combat-click` (@uistate.Mode.value == "combat"); rules `mode-combat`, `kb-mode-combat` set it |
| `Colony` | `enemy_snapshot` | 817 | ctx.setField in enemy-snapshot; ctx.getField in 5 systems + 2 fns |
| `Colony` | `last_threat` | 821 | declared (forward-compat for future threat escalation; no current reader) |

**Pre-existing fields re-used by combat (still declared):** `Colony.player_x`, `Colony.player_y`, `Colony.food`, `Clock.day`, `Region.discovered`, `Position.x/y`, `Velocity.x/y`, `Inventory.{ore,wood,fiber,seed,wheat,plank,chair,lamp,hide,crystal_core}`, `Persona.role`, `Need.affinity`.

No ❌ items.

## Enum variant audit (Section 2) — PASS

| Enum write | Variant | Declared in schema |
|---|---|---|
| `@uistate.Mode.value` ← `"combat"` (rules `mode-combat`, `kb-mode-combat`) | `combat` | line 515 ✅ |

No other enum writes in the diff. No ❌ items.

## Scene entity reference audit (Section 3) — PASS

Every `@<name>` in the diff's rules verified present in `games/frontier/scenes/main.json` (474 entities total):

| Entity name | Referenced by | Present? |
|---|---|---|
| `@player` | combat.json (combat-click, enemy-killed-loot), hud.json (hud-hp) | ✅ (Player, Hp, Weapon, Inventory, Position, …) |
| `@colony` | combat.json (all 4 rules) | ✅ (Colony, Clock, …) |
| `@uistate` | ui.json (mode-combat, kb-mode-combat) | ✅ (Mode, Build) |
| `@hp_lbl` | hud.json (hud-hp) | ✅ (Ui, UiLabel) — newly added |
| `@build_menu`, `@craft_menu`, `@tech_menu` | ui.json (mode-combat, kb-mode-combat hide menus) | ✅ pre-existing |

No ❌ items.

## UI layout overlap audit (Section 4) — PASS

`mode_combat` is added as a child of the `mode_row` HBox container (`anchor: top-left, parent: mode_row, w: 92, h: 48`). The HBox auto-distributes children with `gap: 6`, so no manual oy computation is needed — children cannot overlap by construction.

`mode_row.Ui.w` was bumped from 580 → 582 to fit 6 buttons × 92 + 5 gaps × 6 = 582. Verified via Python parse: `mode_row.Ui.w = 582`. ✅

`mode_combat_lbl` is `anchor: stretch, parent: mode_combat` — it fills its parent button, which is the standard pattern for all existing mode button labels (`mode_build_lbl`, `mode_craft_lbl`, etc.). No overlap concern.

No ❌ items.

## Standard checks (Section 5) — PASS

| Check | Result |
|---|---|
| `cargo run -p vitric-cli -- check games/frontier` exits 0 | ✅ |
| All new `//` / `/* */` comments in combat.js + combat.rs are English | ✅ |
| String literals (toast "你倒下了,被送回登陆点", UI label "战斗") keep authored language | ✅ |
| No fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`) | ✅ — combat.js uses only `vitric.system`, `vitric.fn`, `ctx.dt`, `ctx.random()`, `ctx.spawn`, `ctx.emit`, `ctx.despawn`, `ctx.getField`, `ctx.setField`, `e.id`, `e.<Comp>.<field>` — all verified real |
| No dead code / YAGNI | ⚠️ minor: `PLAYER_ATTACK_RANGE` constant declared but not referenced (see Concerns #1). All other constants, helpers, and fns are actively used. |
| Commit message follows `<type>(<scope>): <summary>` | ✅ `feat(frontier): combat system — …` |
| Only in-scope files modified | ✅ 10 files, all in brief allowlist |

No ❌ items.

## Self-audit summary

| Section | Result |
|---|---|
| 1. Schema field audit | ✅ PASS |
| 2. Enum variant audit | ✅ PASS |
| 3. Scene entity reference audit | ✅ PASS |
| 4. UI layout overlap audit | ✅ PASS |
| 5. Standard checks | ✅ PASS |

**Overall: PASS — ready for review.**
