# Task 14 Review

**Verdict**: APPROVED
**Commit**: 194f0e9

## Findings

### Critical (blocks approval)
- None

### Important (non-blocking)
- **GDD.md §QA arithmetic error (propagated from brief)**: The new line "96 天 ~5760s real-time at 90s/day" is arithmetically wrong. 96 × 90 = **8640 s** (= 144 min ≈ 2 h 24 min), not 5760 s. The figure 5760 s equals 96 × 60 (old DAY_SEC) — i.e. the brief carried over the pre-Task-14 day length. The same inconsistency lives in `clock.js` line 5 header comment ("~96 days to 兴旺 milestone (~90 min real-time)") and in the brief's own §1 ("~90 min real-time"). The implementer faithfully transcribed the brief's text, so this is a brief-side error, not an implementer regression — but future readers will be misled. Suggest fixing in a post-review touch-up or in Task 15 (when the gate recording is re-recorded): replace "~5760s" → "~8640s (≈144 min)" in GDD.md §QA, and replace "~90 min real-time" → "~144 min real-time" in the `clock.js` header. Blocker-free because: (a) does not affect runtime, (b) the implementer followed the brief verbatim.

### Minor (nit-level)
- `Colony.stage` schema default is still `"落脚"` (line 634 of schema.json) but the `stage` system always overrides it to `"起步"` (or higher) on the first tick. Pre-existing — not introduced by Task 14. No action required.
- `clock.js` line 5 comment now reads "12-day seasons, 48-day years, ~96 days to 兴旺 milestone (~90 min real-time)". The "12-day seasons / 48-day years" parts are correct (matches `SEASON_DAYS = 12`); only the "~90 min" is wrong (see Important note above).

## Schema field audit

All 15 fields read by the new `stage` system (and by the modified `quest.json` rules) are declared in `games/frontier/schema.json`. No undeclared fields. No new fields added (consistent with brief §"Out of scope").

| Field | Declared? | Schema line |
|---|---|---|
| `Colony.struct_count` | PASS | 628-631 (int) |
| `Colony.pop` | PASS | 624-627 (int) |
| `Colony.monument_built` | PASS | 652-655 (int) |
| `Colony.stage` | PASS | 632-635 (text, default `"落脚"`) |
| `Colony.day` | PASS | 656-659 (int) — read by quest.json step5/step6 rules |
| `Colony.companion_wish_count` | PASS | 806-809 (int) — read by quest.json step6 rule |
| `Clock.day` | PASS | 400-403 (int) |
| `Clock.time` | PASS | 404-407 (number) — referenced by pacing.rs tests |
| `Research.has_survival_t1` | PASS | 1065 (int) |
| `Research.has_survival_t2` | PASS | 1066 (int) |
| `Research.has_agriculture_t1` | PASS | 1068 (int) |
| `Research.has_agriculture_t2` | PASS | 1069 (int) |
| `Research.has_exploration_t2` | PASS | 1072 (int) |
| `Research.has_industry_t2` | PASS | 1075 (int) |
| `Faction.tier_nomads` | PASS | 1087 (text) |
| `Faction.tier_caravan` | PASS | 1088 (text) |
| `Faction.tier_remnant` | PASS | 1089 (text) |
| `Inventory.wheat` | PASS | 483-486 (int) — read by quest.json step5 rule |
| `QuestLog.step` | PASS | 1015-1018 (int) — read by all quest rules |
| `UiLabel.content` | PASS | 128-130 (text) — written by banner rules |

Scene entity reference audit (`@colony`, `@player`, `@quest`, `@quest_title_lbl`, `@quest_sub_lbl`): all 5 entities verified present in `games/frontier/scenes/main.json` via JSON parse.

## Enum variant audit

- `Colony.stage` is declared `type: "text"` (NOT `enum`), so all 5 stage literals used by the new `stage` system — `"起步"`, `"立足"`, `"成形"`, `"成群"`, `"兴旺"` — are valid string writes. The `quest.json` `settlement-founded` rule's `@colony.Colony.stage == "兴旺"` filter is a text comparison, no enum constraint. PASS.
- `Faction.tier_nomads/tier_caravan/tier_remnant` are `type: "text"` (NOT `enum`). The literals `"neutral"`, `"friendly"`, `"allied"`, `"wary"` used in the new `stage` system's JS arrays are valid string comparisons. PASS.
- No `"set": "@<entity>.<EnumComp>.<value_field>", "to": "<literal>"` rule writes touch an enum-typed field in this diff. PASS.

## Rule condition audit

- `quest-step5-done` (`@colony.Colony.day >= 12`, `@player.Inventory.wheat >= 5`): both fields declared int. PASS.
- `quest-step6-done` (`@colony.Colony.day >= 24`, `@colony.Colony.pop >= 3`, `@colony.Colony.companion_wish_count >= 2`): all int. PASS.
- `settlement-founded` (`@colony.Colony.stage == "兴旺"`): text comparison against text-typed field. PASS.

## Variable name audit (typo check)

The new `stage` system in `colony.js` (lines 76-101) uses these identifiers — all spell-checked against schema:

- `tierNomads` (NOT `tierNomands`) ✓ — matches `Faction.tier_nomads`
- `tierCaravan` ✓ — matches `Faction.tier_caravan`
- `tierRemnant` ✓ — matches `Faction.tier_remnant`
- `hasSurvivalT1`, `hasSurvivalT2`, `hasAgriT1`, `hasAgriT2`, `hasExplT2`, `hasIndT2` ✓ — all match `Research.has_*` schema field names
- `c.Colony.monument_built`, `c.Colony.struct_count`, `c.Colony.pop`, `c.Colony.stage`, `c.Clock.day` ✓ — all match schema

The deliberate `tierNomands` typo from the early brief draft was NOT carried into the implementation. Clean.

## Deviation evaluation

### Deviation A — `seasons.rs` mechanical fix (59.99 → 89.99, 60.0 → 90.0 in comments): APPROVED

**Rationale**: The brief §8 expects "no regression" on seasons tests, but `seasons.rs` hardcodes `Clock.time = 59.99` (just below `CLOCK_DAY_SEC = 60.0`) to test that one tick (dt ≈ 0.0167) pushes time past the day-wrap boundary. After changing `CLOCK_DAY_SEC` to 90.0, `59.99 + 0.0167 = 60.0067 < 90.0` — day-wrap would no longer fire and 3 of 4 seasons tests would fail. The implementer's minimal fix (bump 59.99 → 89.99 so `89.99 + 0.0167 > 90.0` again, plus comment updates 60.0 → 90.0) preserves the original test intent verbatim and is the smallest possible change. Direct verification: `cargo test -p vitric-cli --test seasons` → 4 passed; 0 failed. Legitimate.

### Deviation B — `pacing.rs` spawns 5 `Structure` entities (kind=`"plot"`) instead of writing `Colony.struct_count` directly: APPROVED

**Rationale**: Verified the clobbering pipeline end-to-end:
1. `colony.js` `tally` system queries `["Structure"]`, computes `total = entities.length`, emits `tally { total, ... }` every tick (lines 36-64).
2. `rules/colony.json` `apply-rates` rule subscribes to `tally` event and writes `@colony.Colony.struct_count = event.total` (lines 4-13).

So a direct `set_field(... "Colony.struct_count", json!(5))` would be overwritten on the very next `sim.step()` — Test 1 (day 11 → assert stage=="起步") would still see struct_count=5 at day 11, but the day-12 assertion is the one that matters and a direct write would survive only one tick anyway. The implementer's `spawn_structures` helper spawns 5 real `Structure` entities (kind=`"plot"`, tier=1, `_cd_t=0`), which the `tally` system will count naturally and the `apply-rates` rule will then write `struct_count = 5` itself. This is more correct than the brief's suggested direct write — it actually exercises the production tally pipeline rather than bypassing it. The 5 spawned entities use unique names (`test_struct_plot_0..4`) so they don't collide with scene entities. Direct verification: `cargo test -p vitric-cli --test pacing` → 3 passed; 0 failed. Legitimate.

One small note: the spawned `Structure` entities have no `Position` component, so they exist at `(0,0)`. This doesn't affect the `tally` system (which only reads `Structure.kind`), but if a future system starts spatially querying structures this could become a test-isolation footgun. Non-blocking; the test passes and the helper is clearly scoped.

## GDD.md accuracy

Cross-checked every numeric threshold in `games/frontier/GDD.md` against the actual code:

### §机制 任务步 (GDD lines 46-49)
| Step | GDD text | Code reference | Match? |
|---|---|---|---|
| 4 | `day≥12 (春末) + survival_t1 研究 + 结构≥5` | colony.js: `day >= 12 && hasSurvivalT1 && s >= 5` | PASS |
| 5 | `day≥12 (春末) + 小麦存量≥5` | quest.json step5: `Colony.day >= 12` + `Inventory.wheat >= 5` | PASS |
| 6 | `day≥24 (夏末) + 人手≥3 + companion_wish_count>=2` | quest.json step6: `Colony.day >= 24` + `Colony.pop >= 3` + `companion_wish_count >= 2` | PASS |
| 7 | `Colony.stage==兴旺 (day≥96 + 全 T2 科技 + 丰碑 + 派系结盟)` | quest.json step7: `Colony.stage == "兴旺"`; colony.js: `day >= 96 && allT2 && monument >= 1 && anyFactionAllied` | PASS |

### §深化系统 沙盒节奏 (GDD lines 70-77)
| Stage | GDD text | Code reference | Match? |
|---|---|---|---|
| 起步 | `day 1-3, 无要求` | colony.js: `let stage = "起步"` default | PASS |
| 立足 | `day≥12, survival_t1 + 结构≥5` | colony.js: `day >= 12 && hasSurvivalT1 && s >= 5` | PASS |
| 成形 | `day≥24, pop≥3 + agriculture_t1` | colony.js: `day >= 24 && pop >= 3 && hasAgriT1` | PASS |
| 成群 | `day≥48, pop≥5 + 任一派系 neutral+` | colony.js: `day >= 48 && pop >= 5 && anyFactionNeutralOrBetter` (neutral/friendly/allied) | PASS |
| 兴旺 | `day≥96, 全 T2 科技 + 丰碑 + 任一派系 allied` | colony.js: `day >= 96 && allT2 && monument >= 1 && anyFactionAllied` | PASS |

### §QA (GDD line 124)
- "96 天 ~5760s real-time at 90s/day" — **ARITHMETIC ERROR**: 96 × 90 = 8640 s, not 5760 s. See Important findings above. The implementer transcribed the brief's text verbatim.

## Banner text accuracy

All 4 banner updates in `quest.json` (lines 134, 143, 152, 161) match the new thresholds:

| Banner | Sub-label text | Code reference | Match? |
|---|---|---|---|
| quest-banner-4 | `等到第 12 天(春末),研究 survival_t1 + 凑齐 5 座结构` | colony.js 立足: day≥12 + survival_t1 + struct≥5 | PASS |
| quest-banner-5 | `攒 5 麦子,等过第 12 天(春末)` | quest.json step5: day≥12 + wheat≥5 | PASS |
| quest-banner-6 | `等过第 24 天(夏末),把第 2、3 个伙伴请回家` | quest.json step6: day≥24 + pop≥3 + companion_wish_count≥2 | PASS |
| quest-banner-7 | `等过第 96 天(两年末),研究全部 T2 科技 + 立丰碑 + 任一派系结盟` | colony.js 兴旺: day≥96 + allT2 + monument + allied | PASS |

## Test verification

Ran the three required verification commands:

```
$ cargo test -p vitric-cli --test pacing
running 3 tests
test stage_advances_to_prosperity_at_day_96 ... ok
test stage_does_not_advance_without_tech ... ok
test stage_advances_to_foothold_at_day_12 ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
```

```
$ cargo test -p vitric-cli --test seasons
running 4 tests
test year_increments_on_spring_wrap ... ok
test weather_timer_decrements_each_tick ... ok
test season_rolls_over_at_12_days ... ok
test season_advances_on_day_boundary ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
```

```
$ cargo test -p vitric-cli --test region
test catch_up_advances_dormant_crop_on_thaw ... ok
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 92.33s
```

All 26 tests pass. No regressions.

The 3 pacing tests cover exactly what brief §7.1/§7.2/§7.3 specifies:
- Test 1 (`stage_advances_to_foothold_at_day_12`): day 11 + tech + struct 5 → "起步"; then day 12 → "立足". Correctly tests the day-floor boundary.
- Test 2 (`stage_does_not_advance_without_tech`): day 12 + struct 5 but `has_survival_t1 = 0` → stays "起步". Correctly tests that tech is a hard gate, not just a day-floor.
- Test 3 (`stage_advances_to_prosperity_at_day_96`): day 96 + all 4 T2 techs + monument + faction allied → "兴旺". Correctly tests the highest-stage happy path.

Test 3 also sets `has_survival_t1` and `has_agriculture_t1` (T1) in addition to the T2s — this is belt-and-suspenders but doesn't affect the assertion because 兴旺 is checked first and only depends on T2 + monument + allied. The comment in the test correctly explains this.

## Standard checklist compliance

- [x] Schema field audit (section 1): no undeclared fields. No new fields added (out-of-scope respected).
- [x] Enum variant audit (section 2): N/A — `Colony.stage` and `Faction.tier_*` are `text`, not `enum`.
- [x] Rule condition audit (section 3): all `@<entity>.<Comp>.<field>` reads use declared fields on existing scene entities.
- [x] Scene entity reference audit: `colony`, `player`, `quest`, `quest_title_lbl`, `quest_sub_lbl` all present in `scenes/main.json`.
- [x] N/A — no scene/UI layout edits in this task.
- [x] All new `//` comments in `clock.js`, `colony.js`, `pacing.rs` are in English. String literals (banner text, GDD content) keep their authored Chinese. ✓
- [x] No fake APIs used — only `ctx.getField`, `ctx.emit`, `sim.world.spawn_named`, `sim.world.set_component`, `sim.world.set_field`, `sim.world.get_field`, all of which are verified real APIs used by existing tests (`research.rs`, `seasons.rs`).
- [x] No dead code / YAGNI. The `spawn_structures` helper is used by both Test 1 and Test 2. The `set_field`/`get_field` helpers are used by all 3 tests.
- [x] Commit message follows `<type>(<scope>): <summary>` convention: `feat(frontier): pacing rebalance for sandbox play`. ✓
- [x] In-scope files only: brief listed 5 files (clock.js, colony.js, quest.json, GDD.md, pacing.rs); implementer also touched seasons.rs (Deviation A — justified).

## Recommendation

**APPROVED.** No blocking fixes required.

Optional post-review touch-up (deferrable to Task 15, when the gate recording is re-recorded anyway):
- Fix the arithmetic inconsistency in `clock.js` line 5 header: "~96 days to 兴旺 milestone (~90 min real-time)" → "~96 days to 兴旺 milestone (~144 min real-time at 90s/day)". 96 × 90 = 8640 s = 144 min, not 90 min.
- Fix `GDD.md` §QA: "96 天 ~5760s real-time at 90s/day" → "96 天 ~8640s real-time at 90s/day (≈144 min)". Same arithmetic correction.

Both fixes are documentation-only; neither affects runtime or tests. The implementer faithfully followed the brief's (incorrect) text, so this is a brief-side error caught at review time.
