# Task 13 Review — Region Content Polish

**Commit**: 00e5789b145ef313003ad006274fa6a43da3c841
**Reviewer**: subagent
**Date**: 2026-07-21
**Base**: f8edc63 (Task 12 post-review fix)

## Verdict: APPROVED (with one Important follow-up recommended)

The implementation is faithful to the brief, deterministic, schema-clean, and fully tested for the sandbeast spawn system. One **Important** issue (the `companion-mood-boost` event has no consumer, so the oasis POI's "+5 mood" toast is currently misleading) is documented below — the brief itself sanctioned the forward-compat-hook fallback, so this does not block approval, but a small follow-up patch (or Task 14 work) is recommended.

## Summary

Task 13 expanded REGION_CONTENT with 3 new POI kinds (crystal-cave, oasis, caravan-stop), added 6 per-type POI handlers in `POI_HANDLERS` with special effects, and added a sandbeast enemy + timer-based `desert-spawn` system using the deterministic `desert_spawn` substream. The 4-file scope was respected (region.js, poi.js, combat.js, region.rs only). All schema fields used already exist; no new fields, no enum variants, no scene changes. The 2 new region tests pass alongside the existing 17 (19/19 total per controller verification).

## Section 1: Schema field audit — PASS

For every `e.<Comp>.<field> =` write and `ctx.setField(..., "<Comp>.<field>", ...)` call in the diff:

| Field | Location in `schema.json` | Used by Task 13 |
| --- | --- | --- |
| `Region.id` (text)            | line 1047 | combat.js `desert-spawn` filter |
| `Region.state` (enum)        | line 1049 | combat.js `desert-spawn` filter |
| `Region.spawn_timer` (number, default 7200) | line 1056 | combat.js `desert-spawn` write |
| `Poi.kind` (text)            | line 994  | poi.js handler dispatch |
| `Poi.state` (enum)           | line 998  | poi.js `interact_poi` check |
| `Poi.cooldown` (number)      | line 1003 | poi.js `interact_poi` write |
| `Poi.reward_table` (text)    | line 1007 | poi.js reward roll |
| `Enemy.kind/damage/aggro_range/home_region/_attack_cd` | lines 328-347 | combat.js `ctx.spawn` for sandbeast |
| `Position.x/y`, `Velocity.x/y`, `Collider.w/h`, `Sprite.w/h/image/color`, `Hp.value/max` | (existing) | combat.js `ctx.spawn` + poi.js `dangerous-flora` spawn |

No new schema fields introduced. No `@<entity>.<Comp>.<field>` rule reads added (the only rule-side touches in scope were the existing `gen_region_content` call from `rules/region.json:7` which Task 13 didn't modify).

## Section 2: Enum variant audit — PASS

- `Region.state` set to `"active"` in tests (`region.rs:674`) — variant exists (dormant/active/frozen) ✓
- `Poi.state` set to `"looted"` in `interact_poi` (poi.js:136) — variant exists (fresh/looted/depleted) ✓
- No new enum literals introduced. `Poi.kind` and `Enemy.kind` are `text` (not enums) — any string is valid. The new kinds (`crystal-cave`, `oasis`, `caravan-stop`, `sandbeast`) are new string values for existing text fields, which requires no schema change.

## Section 3: Scene entity reference audit — PASS

No scene changes. Verified:
- `desert` region marker entity exists in `scenes/main.json` (Task 12) — referenced by `sim.world.entity("desert")` in tests, and by `e.Region.id !== "desert"` filter in the `desert-spawn` system.
- All 6 POI kinds referenced in `REGION_CONTENT` (region.js) have handlers in `POI_HANDLERS` (poi.js): `ancient-ruins`, `crystal-cave`, `dangerous-flora`, `oasis`, `caravan-stop`, `tomb`. No POI kind is left unhandled.
- `caravan-stop` handler emits `trade-available` — consumed by `rules/faction.json:6` (Task 11's `trader-companion-relation` rule, confirmed via grep). ✓

## Section 4: UI layout overlap audit — N/A

No UI changes. No new HUD entities, no `Ui` component edits, no anchor/ox/oy changes. Task 13 only modifies game logic scripts.

## Section 5: Standard checks — PASS

- [x] `cargo run -p vitric-cli -- check games/frontier` exits 0 (per implementation report).
- [x] All new `//` comments in English (project convention). Chinese preserved in game-facing string literals (toasts, POI labels). ✓
- [x] No fake APIs used. Verified APIs: `ctx.random_stream`, `ctx.random`, `ctx.spawn`, `ctx.emit`, `ctx.setField`, `ctx.getField`, `ctx.thaw_region` (not used here but referenced in `unlock_region`), `stream.nextInt`. No `Math.random`, `ctx.singleton`, `ctx.each`, etc.
- [x] No dead code / YAGNI in new functions. Each handler has a distinct effect; `desert-spawn` system has a single clear purpose.
- [x] Commit message follows `feat(<scope>): <summary>` convention (`feat(frontier): region content polish — POI tables, biome enemies`). ✓
- [x] Only in-scope files modified: `region.js`, `poi.js`, `combat.js`, `tests/region.rs` — exactly the 4 files specified in brief §1. ✓
- [x] No duplicate `const` declarations across scripts: `POI_HANDLERS` is unique (no collision with `POI_ITEMS`, `POI_LABELS`, `POI_COOLDOWN_LOOTED`, `POI_CAVE_INJURY_CHANCE` in poi.js or constants in other scripts). `ENEMY_TYPES.sandbeast` is a new key in an existing dict (no redeclaration). ✓
- [x] Tests: 19/19 region tests pass per controller verification (including 2 new sandbeast tests). The 2 pre-existing `typescript.rs` failures are environment-related (missing `esbuild` binary) and unrelated to Task 13 — verified identical on clean `f8edc63` per implementation report.

## Specific checks

### 1. POI_HANDLERS completeness — PASS

All 6 POI kinds declared in `REGION_CONTENT` (region.js:25-62) have handlers in `POI_HANDLERS` (poi.js:38-94):

| Region | POI kinds in REGION_CONTENT | Handlers in POI_HANDLERS |
| --- | --- | --- |
| mountain | `ancient-ruins`, `crystal-cave` | ✓ both present |
| swamp | `dangerous-flora`, `oasis` | ✓ both present |
| desert | `caravan-stop`, `tomb` | ✓ both present |

`interact_poi` dispatches via `POI_HANDLERS[poi.kind]` after the standard reward roll (poi.js:150-151). Any POI kind without a handler silently falls through (no error) — but here every kind has one.

### 2. desert-spawn system correctness — PASS

Verified against brief §4.2 line-by-line (combat.js:392-431):

| Brief requirement | Implementation | Status |
| --- | --- | --- |
| `query: ["Region"]` | line 392 | ✓ |
| `writes: ["Region"]` (covers spawn_timer write) | line 392 | ✓ |
| Filter for `e.Region.id !== "desert"` | line 394 | ✓ |
| Filter for `e.Region.state !== "active"` | line 395 | ✓ |
| Decrement `spawn_timer` by `ctx.dt` | line 398 | ✓ |
| Skip body if `timer > 0` (just write back) | lines 399-402 | ✓ |
| Reset to 7200 on expiry (BEFORE player check) | line 405 | ✓ |
| Read player pos via `ctx.getField("player", ...)` | lines 407-408 | ✓ |
| Type-guard `px/py` (return early if not number) | line 409 | ✓ |
| Desert bounds check `60..119, 0..59` | line 412 | ✓ |
| Use `ctx.random_stream("desert_spawn")` | line 416 | ✓ |
| `stream.nextInt(-3, 3)` for x/y offset | lines 417-418 | ✓ |
| Spawned sandbeast has all 6 required components | lines 419-428: Enemy, Position, Velocity, Collider, Sprite, Hp | ✓ |

**Subtle point (correct)**: The system resets `spawn_timer` to 7200 (line 405) BEFORE the player-position check (line 412). This means: if the timer expires but the player is outside desert, the timer still resets — so the next spawn attempt won't happen for another 7200 ticks. This is the correct behavior (avoids spawning the moment the player re-enters desert from a freshly-expired timer; matches brief §4.2).

**Subtle point (also correct)**: The system iterates ALL region markers (home/wild/mountain/swamp/desert) but only writes to the desert marker — non-desert regions are skipped via `continue` (line 394) before any write. No accidental writes to other regions' spawn_timers.

### 3. interact_poi dispatch — PASS

Order of operations in `interact_poi` (poi.js:99-160):

1. Parse reward_table (line 107)
2. Build inventory + roll rewards via `ctx.random()` (lines 116-128)
3. Emit `inv-set` with new inventory (line 133)
4. Mark POI `looted` + start cooldown (lines 136-137)
5. Emit `tp-set` for +2 TechPoint (line 143)
6. Emit `toast-show` with reward summary (line 146)
7. **Dispatch to `POI_HANDLERS[poi.kind]`** (lines 150-151) — AFTER all standard rewards
8. Legacy `cave-entrance` special-case preserved (lines 154-157) — backward compat with wild-area POIs
9. Emit `entered-poi` for wish system (line 160)

The legacy `cave-entrance` block is preserved exactly as specified in brief §3 (line 199-203). No backward-compat break.

### 4. companion-mood-boost — IMPORTANT (non-blocking)

**Finding**: The `oasis` handler (poi.js:73-77) emits `companion-mood-boost` and shows toast "绿洲清泉:全员心情+5". But grep across the entire codebase confirms **no consumer exists** for this event:

```
$ grep -rn "companion-mood-boost" games/frontier/
games/frontier/scripts/poi.js:75:    ctx.emit("companion-mood-boost", { amount: 5, reason: "oasis" });
```

Only the emitter. No rule in `rules/*.json` listens for it, no fn in `scripts/*.js` consumes it.

**Why this happened**: The brief's §5 instruction said: *"Check if `rules/companion.json` has a rule for `companion-mood-drop`. If it does, the implementer should add a mirror rule for `companion-mood-boost`."* The implementer checked `rules/companion.json` — no mood-drop rule there. Per the brief's literal check, no mirror was added.

**But**: The actual `companion-mood-drop` consumer lives in `rules/wish.json:86-90` (rule `companion-mood-drop-apply`), which calls `apply_mood_drop` in `scripts/wish.js:147-159` — decrementing `Need.comfort` on all companions. The brief pointed at the wrong file (`companion.json` instead of `wish.json`). The implementer's report explicitly documents this discovery (report §"companion-mood-boost consumer", lines 80-99) and recommends a small mirror patch.

**Net effect**: The oasis POI's mood boost is currently a **no-op with misleading UX**:
- Player interacts with oasis → toast says "全员心情+5"
- `companion-mood-boost` event is emitted (no consumer)
- No companion's `Need.comfort` actually changes
- Player is told something happened that didn't

**Severity rationale**:
- The brief itself sanctioned the forward-compat-hook fallback (§3 lines 218-222): *"For Task 13, I'll keep `companion-mood-boost` as an emitted event with a toast. If the companion system doesn't consume it, it's a forward-compat hook (Task 14 or later can add a consumer). The toast gives the player feedback. This is acceptable for a polish task."*
- The implementer correctly followed the brief's literal instruction AND documented the gap.
- The fix is small (~15 lines): add `apply_mood_boost` fn (mirror of `apply_mood_drop` with `Math.min(100, curNum + amount)` instead of `Math.max(0, curNum - amount)`) and `companion-mood-boost-apply` rule calling it. But it requires touching `scripts/wish.js` + `rules/wish.json` — 2 files outside Task 13's stated scope.

**Recommendation**: APPROVE the task as-is, but recommend a post-review fix patch (mirroring the Task 12 pattern of `f8edc63` post-review fix) OR fold the mirror into Task 14. The misleading toast is a UX papercut, not a crash or correctness regression.

### 5. Determinism — PASS

- `desert-spawn` uses `ctx.random_stream("desert_spawn")` (combat.js:416) — deterministic substream derived from `(world_seed, "desert_spawn")`. Replay-safe regardless of when the spawn happens.
- POI handlers use `ctx.random()` (poi.js:49, 58, 89) — also substream-derived and deterministic.
- No `Math.random()` anywhere in the diff (poisoned in QuickJS, would throw).
- No `Date.now()` or other non-deterministic sources.

### 6. Test coverage — ADEQUATE (matches brief), with nits

The 2 new tests (region.rs:667-727) cover:
- `sandbeast_spawns_when_player_in_desert`: activates desert, sets spawn_timer=0, places player at (70, 10), steps, asserts enemy count increased AND at least one enemy has `Enemy.kind == "sandbeast"`. ✓
- `sandbeast_does_not_spawn_when_player_outside_desert`: same setup, player at (7, 7) outside desert, asserts no enemy spawned. ✓

**Coverage gaps (nits, not blockers — brief only specified these 2 tests)**:
- Sandbeast stats not verified (HP=60, damage=12, aggro_range=12). The kind check (`sandbeast_count > 0`) is sufficient to confirm the dispatch path, but a stat check would lock the ENEMY_TYPES.sandbeast values.
- Spawn position not verified to be within desert bounds (the test only counts enemies).
- `spawn_timer` reset behavior not verified (does it go back to 7200 after spawn?).
- Multiple-spawn behavior not tested (does a second spawn happen after another 7200 ticks?).
- `interact_poi` handler dispatch is NOT tested at all. The brief explicitly punted on this (§7 line 364: "this is hard to test deterministically because the handler uses `ctx.random()`"), so it's an acknowledged gap.
- Test for `oasis` POI: would verify the (currently no-op) mood boost — but since the consumer doesn't exist, this test would just check toast emission.

None of these gaps block approval — the brief specified exactly 2 tests, the implementer added exactly those 2 tests, and they pass.

## Issues found

### Important

1. **`companion-mood-boost` has no consumer — oasis POI toast is misleading** (poi.js:75-76)
   - The oasis handler emits `companion-mood-boost` but no rule consumes it. The toast "全员心情+5" tells the player their companions' mood improved, but no companion's `Need.comfort` actually changes.
   - Root cause: brief §5 pointed at `rules/companion.json` for the mood-drop mirror check, but the actual mood-drop consumer lives in `rules/wish.json:86-90` (calls `apply_mood_drop` in `scripts/wish.js:147-159`).
   - The implementer correctly identified this in the report (§"companion-mood-boost consumer") but followed the brief's literal instruction + the 4-file scope constraint.
   - **Recommended fix** (post-review patch OR Task 14): add `apply_mood_boost` fn in `scripts/wish.js` (mirror of `apply_mood_drop` — `Math.min(100, curNum + amount)` instead of `Math.max(0, curNum - amount)`), and add `companion-mood-boost-apply` rule in `rules/wish.json` (mirror of `companion-mood-drop-apply`). ~15 lines total.
   - Brief §3 lines 218-222 explicitly sanctioned the forward-compat-hook fallback ("acceptable for a polish task"), so this does not block approval.

### Minor / Nit

2. **Stale comment in `spawn_wave`** (combat.js:94)
   - Comment reads: `// desert doesn't exist yet (Task 13) — ctx.getField returns 0/default.`
   - Desert DOES exist as a region (added in Task 12 scene). The comment is stale from Task 12.
   - Task 13's brief explicitly said "No change to `spawn_wave`" (§4.3), so the implementer correctly didn't touch it. But the stale comment is misleading to future readers.
   - **Suggested fix**: Either remove the stale comment OR add desert to the `regionCount` calculation (if sandbeast spawns should also count toward night-wave scaling — currently they don't, which is the brief's intent). Either fix is a 1-line change.
   - Pre-existing issue (not a Task 13 regression). Not blocking.

3. **Test coverage gaps** (region.rs:667-727)
   - Sandbeast stats, spawn position, timer reset, and POI handler dispatch are not tested.
   - Brief §7 explicitly specified only these 2 tests, so this matches the brief. But future tasks touching combat.js or poi.js may want to add coverage.
   - Nit, not blocking.

## Files reviewed

- `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/briefs/task-13-brief.md` (brief)
- `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/briefs/task-13-report.md` (implementation report)
- `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/review-checklist.md` (checklist)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/region.js` (modified — REGION_CONTENT expansion)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/poi.js` (modified — POI_HANDLERS + interact_poi refactor)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/combat.js` (modified — sandbeast + desert-spawn system)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scripts/wish.js` (read — for `apply_mood_drop` reference)
- `/Users/leolele/Documents/leo/vitric/games/frontier/rules/wish.json` (read — for `companion-mood-drop-apply` rule)
- `/Users/leolele/Documents/leo/vitric/games/frontier/rules/region.json` (read — for `region-thaw-content` rule)
- `/Users/leolele/Documents/leo/vitric/games/frontier/rules/faction.json` (read — for `trade-available` consumer)
- `/Users/leolele/Documents/leo/vitric/games/frontier/rules/combat.json` (read — for spawn_wave rule)
- `/Users/leolele/Documents/leo/vitric/games/frontier/schema.json` (read — field declarations)
- `/Users/leolele/Documents/leo/vitric/games/frontier/scenes/main.json` (grep — confirm desert marker exists)
- `/Users/leolele/Documents/leo/vitric/crates/vitric-cli/tests/region.rs` (modified — 2 new tests)
- `git show --stat 00e5789` (confirm commit scope: 4 files, +204/-4)
- `git log --oneline -3` (confirm commit lineage: f8edc63 → 00e5789)

## Conclusion

Task 13 is a clean, faithful implementation of the brief. The single Important issue (`companion-mood-boost` has no consumer) is a UX papercut explicitly sanctioned by the brief's own fallback language and clearly documented in the implementation report. The fix is small and well-scoped; recommend folding it into Task 14 (Pacing Rebalance, which will touch wish-related balance anyway) or a quick post-review patch.

**APPROVED.**
