# Task 14 — Pacing Rebalance

**Plan ref**: `docs/superpowers/plans/2026-07-20-frontier-sandbox-expansion.md` §Task 14 (lines 1396-1430)
**Spec ref**: `docs/superpowers/specs/2026-07-20-frontier-sandbox-expansion-design.md` §4.7 (lines 322-336)
**Prior task**: Task 13 (Region Content Polish) — complete, commit `a7cfee6`
**Scope**: 4 files modified + 1 new test file. NO new schema fields. NO new scene entities.

## 1. Goal

Rebalance pacing from a 9-day vertical slice (~9 min real-time) to a sandbox-grade 96-day playthrough (~90 min real-time at 90s/day). Compound stage thresholds tied to seasons/years. `settlement-founded` milestone fires at 兴旺 (end of year 2, day 96). Sandbox continues indefinitely after.

## 2. Files to Modify

| File | Change |
|---|---|
| `games/frontier/scripts/clock.js` | `CLOCK_DAY_SEC` 60 → 90 |
| `games/frontier/scripts/colony.js` | Rewrite `stage` system with compound conditions |
| `games/frontier/rules/quest.json` | Update day thresholds in step5/6/7 + banner text |
| `games/frontier/GDD.md` | Update §机制 任务步 + §深化系统 + §QA day count |
| `crates/vitric-cli/tests/pacing.rs` | NEW — 3 tests for stage transitions |

**Out of scope**: schema.json (no new fields), scenes/main.json (no new entities), any engine crate.

## 3. Step 1 — clock.js: DAY_SEC 60→90

Change line 19:
```javascript
const CLOCK_DAY_SEC = 60.0;
```
to:
```javascript
const CLOCK_DAY_SEC = 90.0;
```

Also update the file header comment (line 4-5) from "60 — measured at about 1 minute/day, 10-15 minutes per session fits a 5-7 day vertical slice" to "90 — sandbox pacing, 90s/day, 12-day seasons, 48-day years, ~96 days to 兴旺 milestone (~90 min real-time)".

**Rationale**: 90s/day gives players time to actually explore regions, do combat, research tech, trade with factions — all the new systems from Tasks 6-13. At 60s/day the sandbox feels rushed.

## 4. Step 2 — colony.js: Compound Stage Thresholds

Replace the entire `stage` system (lines 81-93) with the new compound-condition version per spec §4.7.

### 4.1 New stage thresholds

| Stage | Condition |
|---|---|
| 起步 | day 1-3 (default, no requirement) |
| 立足 | day >= 12 (end of spring) AND `Research.has_survival_t1 == 1` AND `Colony.struct_count >= 5` |
| 成形 | day >= 24 (end of summer) AND `Colony.pop >= 3` AND `Research.has_agriculture_t1 == 1` |
| 成群 | day >= 48 (end of year 1) AND `Colony.pop >= 5` AND any faction tier in {neutral, friendly, allied} |
| 兴旺 | day >= 96 (end of year 2) AND all 4 branches T2+ (`has_survival_t2`, `has_agriculture_t2`, `has_exploration_t2`, `has_industry_t2` all == 1) AND `Colony.monument_built == 1` AND any faction tier == "allied" |

**Stage transitions are monotonic**: once a higher stage is reached, it doesn't regress if conditions later fail (e.g. monument destroyed). Implement by checking stages from highest to lowest and stopping at the first match — but ALSO gate by `day >= threshold` so the day-floor is hard.

### 4.2 New `stage` system code

```javascript
// Stages: compound conditions tied to seasons/years (spec §4.7).
//   起步 (day 1-3)          — default, no requirement
//   立足 (end of spring, day>=12)  — survival_t1 researched AND struct >= 5
//   成形 (end of summer, day>=24)  — pop >= 3 AND agriculture_t1 researched
//   成群 (end of year 1, day>=48)  — pop >= 5 AND any faction tier >= neutral
//   兴旺 (end of year 2, day>=96)  — all 4 branches T2+ AND monument built AND any faction allied
// Sandbox continues after 兴旺 — no ending stage.
// Transitions are monotonic by day-floor: if day >= 96 but 兴旺 conditions not met, stage stays at 成群
// (the highest stage whose day-floor + conditions are both satisfied).
vitric.system("stage", { query: ["Colony", "Clock"], writes: ["Colony"] }, (entities, ctx) => {
  const c = entities[0];
  if (!c) return;
  const day = c.Clock.day;
  const s = c.Colony.struct_count;
  const pop = c.Colony.pop;
  const monument = c.Colony.monument_built | 0;

  // Read Research fields (on the same colony entity — Colony+Research are both attached to "colony").
  const hasSurvivalT1 = (ctx.getField("colony", "Research.has_survival_t1") | 0) === 1;
  const hasSurvivalT2 = (ctx.getField("colony", "Research.has_survival_t2") | 0) === 1;
  const hasAgriT1 = (ctx.getField("colony", "Research.has_agriculture_t1") | 0) === 1;
  const hasAgriT2 = (ctx.getField("colony", "Research.has_agriculture_t2") | 0) === 1;
  const hasExplT2 = (ctx.getField("colony", "Research.has_exploration_t2") | 0) === 1;
  const hasIndT2 = (ctx.getField("colony", "Research.has_industry_t2") | 0) === 1;

  // Read Faction tiers (on the same colony entity — Faction is attached to "colony").
  const tierNomads = ctx.getField("colony", "Faction.tier_nomads") || "wary";
  const tierCaravan = ctx.getField("colony", "Faction.tier_caravan") || "wary";
  const tierRemnant = ctx.getField("colony", "Faction.tier_remnant") || "wary";
  const anyFactionNeutralOrBetter = ["neutral", "friendly", "allied"].includes(tierNomads)
    || ["neutral", "friendly", "allied"].includes(tierCaravan)
    || ["neutral", "friendly", "allied"].includes(tierRemnant);
  const anyFactionAllied = tierNomads === "allied" || tierCaravan === "allied" || tierRemnant === "allied";

  const allT2 = hasSurvivalT2 && hasAgriT2 && hasExplT2 && hasIndT2;

  // Check from highest stage downward; first match wins.
  let stage = "起步";
  if (day >= 96 && allT2 && monument >= 1 && anyFactionAllied) stage = "兴旺";
  else if (day >= 48 && pop >= 5 && anyFactionNeutralOrBetter) stage = "成群";
  else if (day >= 24 && pop >= 3 && hasAgriT1) stage = "成形";
  else if (day >= 12 && hasSurvivalT1 && s >= 5) stage = "立足";

  if (c.Colony.stage !== stage) c.Colony.stage = stage;
});
```

### 4.3 Why `ctx.getField` instead of `c.Research.X`

The `stage` system queries `["Colony", "Clock"]`. Research and Faction are NOT in the query list, so `c.Research` / `c.Faction` would be `undefined`. Use `ctx.getField("colony", "Research.X")` / `ctx.getField("colony", "Faction.X")` to read these fields — same pattern as `flare.js`'s `weather-tick` system reading `Weather.current` via `ctx.getField`.

## 5. Step 3 — quest.json: Update Day Thresholds + Banner Text

### 5.1 step4 (立足)

Already auto-adapts via `@colony.Colony.stage == "立足"`. **No code change needed.** But update the banner text — see §5.5.

### 5.2 step5 (温饱)

Current:
```json
"if": [
  ["@quest.QuestLog.step", "==", 5],
  ["@colony.Colony.day", ">=", 4],
  ["@player.Inventory.wheat", ">=", 5]
]
```

New: change day threshold from 4 to 12 (end of spring):
```json
"if": [
  ["@quest.QuestLog.step", "==", 5],
  ["@colony.Colony.day", ">=", 12],
  ["@player.Inventory.wheat", ">=", 5]
]
```

### 5.3 step6 (多人聚居)

Current:
```json
"if": [
  ["@quest.QuestLog.step", "==", 6],
  ["@colony.Colony.day", ">=", 5],
  ["@colony.Colony.pop", ">=", 3],
  ["@colony.Colony.companion_wish_count", ">=", 2]
]
```

New: change day threshold from 5 to 24 (end of summer):
```json
"if": [
  ["@quest.QuestLog.step", "==", 6],
  ["@colony.Colony.day", ">=", 24],
  ["@colony.Colony.pop", ">=", 3],
  ["@colony.Colony.companion_wish_count", ">=", 2]
]
```

### 5.4 step7 (settlement-founded)

Current:
```json
"if": [
  ["@quest.QuestLog.step", "==", 7],
  ["@colony.Colony.day", ">=", 6],
  ["@colony.Colony.monument_built", ">=", 1]
],
"do": [
  { "emit": "settlement-founded", "data": {} },
  { "set": "@quest.QuestLog.step", "to": 8 }
]
```

New: gate on `Colony.stage == "兴旺"` instead of compound day+monument (the stage system already checks day>=96 + all T2 techs + monument + allied faction):
```json
"if": [
  ["@quest.QuestLog.step", "==", 7],
  ["@colony.Colony.stage", "==", "兴旺"]
],
"do": [
  { "emit": "settlement-founded", "data": {} },
  { "set": "@quest.QuestLog.step", "to": 8 }
]
```

Update the `comment` field to reflect the new gating:
```
"comment": "step7 -> stage==兴旺 (day>=96 + all T2 techs + monument + allied faction) -> settlement-founded. Sandbox continues."
```

### 5.5 Banner text updates

Update the `quest_sub_lbl` text in banners 4-7 to reflect new day thresholds:

- `quest-banner-4` (step 4): change sub from "等到第 3 天,凑齐 3 座结构" to "等到第 12 天(春末),研究 survival_t1 + 凑齐 5 座结构"
- `quest-banner-5` (step 5): change sub from "攒 5 麦子(收几茬即可),等过第 4 天" to "攒 5 麦子,等过第 12 天(春末)"
- `quest-banner-6` (step 6): change sub from "等过第 5 天,把第 2、3 个伙伴请回家" to "等过第 24 天(夏末),把第 2、3 个伙伴请回家"
- `quest-banner-7` (step 7): change sub from "等过第 6 天,攒 ore×4+plank×4+lamp×2+wheat×4 立丰碑" to "等过第 96 天(两年末),研究全部 T2 科技 + 立丰碑 + 任一派系结盟"

## 6. Step 4 — GDD.md Updates

### 6.1 §机制 任务步 (line 42-50)

Update the day thresholds in the quest description:
- Step 4: "day≥3 + 结构≥3" → "day≥12 (春末) + survival_t1 研究 + 结构≥5"
- Step 5: "day≥4 且 小麦存量≥5" → "day≥12 (春末) + 小麦存量≥5"
- Step 6: "day≥5 + 人手≥3 + companion_wish_count>=2" → "day≥24 (夏末) + 人手≥3 + companion_wish_count>=2"
- Step 7: "day≥6 + monument_built>=1" → "Colony.stage==兴旺 (day≥96 + 全 T2 科技 + 丰碑 + 派系结盟)"

### 6.2 §深化系统 — add a new "沙盒节奏" subsection

After the existing 深化系统 entries (line 68ish), add:

```markdown
**沙盒节奏(Task 14)**:DAY_SEC 60→90s,12 天/季,48 天/年,兴旺里程碑在第 96 天(两年末)。阶段条件从单维度(day+结构)改为复合维度(day+科技+人口+派系):
- 起步(day 1-3):无要求
- 立足(春末 day≥12):survival_t1 + 结构≥5
- 成形(夏末 day≥24):pop≥3 + agriculture_t1
- 成群(第一年末 day≥48):pop≥5 + 任一派系 neutral+
- 兴旺(第二年末 day≥96):全 T2 科技 + 丰碑 + 任一派系 allied → emit `settlement-founded`
兴旺后无限沙盒,四循环自驱。
```

### 6.3 §QA (line 116)

Update "9 天 37247 tick" to reflect new pacing: "96 天 ~5760s real-time at 90s/day (Task 15 re-records)".

## 7. Step 5 — Tests (NEW file `crates/vitric-cli/tests/pacing.rs`)

Follow the `research.rs` test pattern: `Runtime::boot(&frontier_dir())`, `set_field` / `get_field` helpers, `sim.step(&mut rt)`, `rt.drain_observed()`.

### 7.1 Test 1: `stage_advances_to_foothold_at_day_12`

Setup:
- Set `Clock.day = 11`, `Clock.time = 0` (just before day 12 boundary)
- Set `Research.has_survival_t1 = 1`
- Set `Colony.struct_count = 5` (via direct field write — struct_count is on Colony)
- Step 1 tick → day still 11, stage should be "起步"

Then:
- Set `Clock.day = 12`
- Step 1 tick
- Assert `Colony.stage == "立足"`

### 7.2 Test 2: `stage_does_not_advance_without_tech`

Setup:
- Set `Clock.day = 12`
- Set `Research.has_survival_t1 = 0` (tech NOT researched)
- Set `Colony.struct_count = 5`
- Step 1 tick
- Assert `Colony.stage == "起步"` (NOT 立足, because tech missing)

### 7.3 Test 3: `stage_advances_to_prosperity_at_day_96`

Setup:
- Set `Clock.day = 96`
- Set `Research.has_survival_t2 = 1`, `has_agriculture_t2 = 1`, `has_exploration_t2 = 1`, `has_industry_t2 = 1`
- Set `Colony.monument_built = 1`
- Set `Faction.tier_caravan = "allied"` (one faction allied)
- Set `Colony.pop = 5`, `Colony.struct_count = 10` (to satisfy earlier stages if needed)
- Step 1 tick
- Assert `Colony.stage == "兴旺"`

**Note**: use `sim.world.set_field(id, "Clock.day", json!(12))` directly — do NOT step 12*90*60 = 64800 ticks to naturally advance the day. The stage system reads `Clock.day` from the entity, so direct field writes work.

**Test runtime budget**: each test ≤ 2 ticks. Total file runtime < 1s.

## 8. Verification

Run before reporting done:
```bash
cargo test -p vitric-cli --test pacing 2>&1 | tail -20
cargo test -p vitric-cli --test region 2>&1 | tail -5   # no regression
cargo test -p vitric-cli --test seasons 2>&1 | tail -5  # no regression
cargo run --release -- gate games/frontier 2>&1 | tail -10  # EXPECTED FAIL — hash changes, re-recorded in Task 15
```

All tests except the gate must PASS. The gate will fail with `ReplayDiverged at tick 0` (DAY_SEC change perturbs the entire trajectory) — this is expected and will be fixed in Task 15 by re-recording `qa/clear.json`.

## 9. Review Checklist (for reviewer)

1. **Schema field audit**: no new fields. All fields read by the new `stage` system must already exist:
   - `Colony.struct_count`, `Colony.pop`, `Colony.monument_built`, `Colony.stage` — pre-existing
   - `Clock.day` — pre-existing
   - `Research.has_survival_t1`, `has_survival_t2`, `has_agriculture_t1`, `has_agriculture_t2`, `has_exploration_t2`, `has_industry_t2` — declared in schema (Task 8)
   - `Faction.tier_nomads`, `tier_caravan`, `tier_remnant` — declared in schema (Task 11)
2. **No typos in variable names**: verify all identifiers compile. Run `cargo build` to catch any syntax errors.
3. **Test coverage**: 3 tests covering 立足 positive + negative + 兴旺 positive.
4. **GDD.md accuracy**: day thresholds in §机制 and §深化系统 match the actual code.
5. **Banner text**: quest-banner-4/5/6/7 sub labels match new thresholds.

## 10. Commit

```bash
git add games/frontier/scripts/clock.js games/frontier/scripts/colony.js games/frontier/rules/quest.json games/frontier/GDD.md crates/vitric-cli/tests/pacing.rs
git commit -m "feat(frontier): pacing rebalance for sandbox play

DAY_SEC 60→90s. Compound stage thresholds tied to seasons/years:
立足 day12+spring+survival_t1+struct5, 成形 day24+pop3+agri_t1,
成群 day48+pop5+faction neutral+, 兴旺 day96+all T2+monument+allied.
settlement-founded fires at 兴旺. 3 pacing tests added."
git push origin main
```

## 11. Report Format

Implementer's final report must include:
1. Commit hash
2. Diff stat (files changed, +/- lines)
3. Test output (pass/fail counts)
4. Gate output (expected fail)
5. Any deviations from this brief (with rationale)
