# Task 6 Brief — Quest System: Convert to Milestone (no game-won, gate on settlement-founded)

## Where this fits

Tasks 1–5 are complete (schema extended, flare/night system, POI system, wish system, LLM memory dialogue all live on `main`). Task 6 converts the linear 8-step quest into a guided curve that ends in a **milestone** (`settlement-founded`) instead of a hard ending (`game-won`). After the milestone, the game continues freely with banner "自由探索中".

The plan's Step 5 (add `Colony.companion_wish_count` to schema) and Step 6 (sync it in wish.js) are **already done** — Task 1 added the field at `schema.json:659`, Task 4's `wish.js:43-44` already does `ctx.setField("colony", "Colony.companion_wish_count", cnt + 1)`. Do NOT re-add them.

## Files to modify

- `games/frontier/rules/quest.json` — 4 rule edits
- `games/frontier/vitric.json` — 1 gate edit

That's it. Do not touch schema.json, wish.js, or any other file.

## Real API note

Quest rules are JSON, not JS. They use:
- `on`: event trigger (`tick` or `{event: "name", filter: {...}}`)
- `if`: array of `[path, op, value]` predicates. Paths use `@entity.Comp.field` for named entities, `@<comp>.<field>` is NOT valid — must always include entity name (e.g. `@quest.QuestLog.step`, `@colony.Colony.day`, `@player.Inventory.wheat`).
- `do`: array of actions: `{set: "@ent.Comp.field", to: value}` or `{emit: "event-name", data: {...}}`

No `ctx.*` calls in rules — rules are declarative. JS scripts are a separate layer (already done in Tasks 3–5).

## Exact changes

### Edit 1: `quest-step3-done` — gate on wish + affinity, not just moved-in

**Current** (`quest.json:23-32`):
```json
{
  "id": "quest-step3-done",
  "comment": "step3 请第一个伙伴:companion-moved-in → step=4。守卫 step==3。",
  "on": { "event": "companion-moved-in" },
  "if": [ ["@quest.QuestLog.step", "==", 3] ],
  "do": [
    { "set": "@quest.QuestLog.step", "to": 4 },
    { "emit": "quest-done", "data": { "id": "first-companion" } }
  ]
},
```

**New**:
```json
{
  "id": "quest-step3-done",
  "comment": "step3 请第一个伙伴:wish-fulfilled + 第一个伙伴(companion)好感>=60 -> step=4.",
  "on": { "event": "wish-fulfilled" },
  "if": [
    ["@quest.QuestLog.step", "==", 3],
    ["@companion.Need.affinity", ">=", 60]
  ],
  "do": [
    { "set": "@quest.QuestLog.step", "to": 4 },
    { "emit": "quest-done", "data": { "id": "first-companion" } }
  ]
},
```

**Rationale**: wish fulfillment grants +30 affinity (wish.js). Base affinity is 30, so one wish → 60. Gate requires real bonding, not just walking a drifter home. `@companion.Need.affinity` reads the first companion entity (named "companion" in main.json — verify by Grep `\"name\": \"companion\"` in scenes/main.json if unsure).

### Edit 2: `quest-step6-done` — gate on wish count, not happy_count

**Current** (`quest.json:60-74`):
```json
{
  "id": "quest-step6-done",
  "comment": "step6 成群:day≥5 + 人手≥3 + 至少 1 位伙伴好感≥50(用心养到会帮忙那档)→ step=7。守卫 step==6 + day-floor + census + 关系闸门。",
  "on": "tick",
  "if": [
    ["@quest.QuestLog.step", "==", 6],
    ["@colony.Colony.day", ">=", 5],
    ["@colony.Colony.pop", ">=", 3],
    ["@colony.Colony.companion_happy_count", ">=", 1]
  ],
  "do": [
    { "set": "@quest.QuestLog.step", "to": 7 },
    { "emit": "quest-done", "data": { "id": "成群" } }
  ]
},
```

**New**:
```json
{
  "id": "quest-step6-done",
  "comment": "step6 成群:day>=5 + pop>=3 + 累计心愿达成>=2 -> step=7.",
  "on": "tick",
  "if": [
    ["@quest.QuestLog.step", "==", 6],
    ["@colony.Colony.day", ">=", 5],
    ["@colony.Colony.pop", ">=", 3],
    ["@colony.Colony.companion_wish_count", ">=", 2]
  ],
  "do": [
    { "set": "@quest.QuestLog.step", "to": 7 },
    { "emit": "quest-done", "data": { "id": "成群" } }
  ]
},
```

**Rationale**: `companion_wish_count` is the aggregate fulfilled-wish counter (synced in wish.js:43-44). Replaces the looser `companion_happy_count >= 1` (which only required one companion at affinity>=50). Now requires 2 total wishes fulfilled across all companions — tighter bonding gate.

### Edit 3: `game-won` rule → `settlement-founded` (milestone, no ending)

**Current** (`quest.json:83-97`):
```json
{
  "id": "game-won",
  "comment": "step7 → day≥6(走 stage「兴旺」闸门)+ 丰碑已立 → settlement-thrived → game-won。",
  "on": "tick",
  "if": [
    ["@quest.QuestLog.step", "==", 7],
    ["@colony.Colony.day", ">=", 6],
    ["@colony.Colony.monument_built", ">=", 1]
  ],
  "do": [
    { "emit": "settlement-thrived", "data": {} },
    { "set": "@quest.QuestLog.step", "to": 8 },
    { "emit": "game-won", "data": {} }
  ]
},
```

**New**:
```json
{
  "id": "settlement-founded",
  "comment": "step7 -> day>=6 + monument built -> settlement-founded (milestone, not ending). Game continues freely.",
  "on": "tick",
  "if": [
    ["@quest.QuestLog.step", "==", 7],
    ["@colony.Colony.day", ">=", 6],
    ["@colony.Colony.monument_built", ">=", 1]
  ],
  "do": [
    { "emit": "settlement-founded", "data": {} },
    { "set": "@quest.QuestLog.step", "to": 8 }
  ]
},
```

**Rationale**: Remove `settlement-thrived` and `game-won` emissions. Emit only `settlement-founded` (the milestone). Step still advances to 8 so the banner switches to "自由探索中". Game does NOT end — player keeps playing.

### Edit 4: `quest-banner-8` — "自由探索中" instead of "聚落兴旺 / 通关!"

**Current** (`quest.json:163-171`):
```json
{
  "id": "quest-banner-8",
  "on": "tick",
  "if": [ ["@quest.QuestLog.step", "==", 8] ],
  "do": [
    { "set": "@quest_title_lbl.UiLabel.content", "to": "聚落兴旺" },
    { "set": "@quest_sub_lbl.UiLabel.content",   "to": "通关!" }
  ]
}
```

**New**:
```json
{
  "id": "quest-banner-8",
  "on": "tick",
  "if": [ ["@quest.QuestLog.step", "==", 8] ],
  "do": [
    { "set": "@quest_title_lbl.UiLabel.content", "to": "自由探索中" },
    { "set": "@quest_sub_lbl.UiLabel.content",   "to": "定居点已建立,四个循环自驱,继续你的故事" }
  ]
}
```

### Edit 5: `vitric.json` gates.must_emit

**Current** (`vitric.json:48`):
```json
        "must_emit": "game-won"
```

**New**:
```json
        "must_emit": "settlement-founded"
```

**Rationale**: The playthrough gate now checks for `settlement-founded` (the milestone). The existing `qa/clear.json` recording will need re-recording in Task 9 — but that's Task 9's job, not Task 6's. After this edit, `vitric gate` will FAIL on the old recording (it emits `game-won`, not `settlement-founded`). That's expected and fine for now.

## Out of scope (do NOT touch)

- `schema.json` — `companion_wish_count` already added (Task 1).
- `scripts/wish.js` — `Colony.companion_wish_count` sync already present (Task 4).
- `rules/narrative.json` — has a dead `ending-show` rule on `game-won` event. Since `game-won` is no longer emitted, this rule never fires (ending panel never shows — which is the intent: no hard ending). Leaving it is harmless. Flag as Minor finding in report; do not remove (YAGNI — out of Task 6 scope, plan doesn't mention it).
- `qa/clear.json` — re-recorded in Task 9.
- `tools/record_clear.py` / `tools/test_progression.py` — these still reference `game-won`. They are dev tools, not runtime. Out of scope; will be updated in Task 10 docs pass if needed.

## Verification

After all 5 edits:

```bash
cd /Users/leolele/Documents/leo/vitric
cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10
```

Expected: `OK` (schema parses, rules load, scripts compile).

Do NOT run `vitric gate` — it will fail because `qa/clear.json` still emits `game-won`. That's expected; Task 9 re-records.

## Commit

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/rules/quest.json games/frontier/vitric.json
git commit -m "feat(frontier): convert quest to milestone-based (settlement-founded), gate on settlement-founded"
git push origin main
```

Per project memory: auto commit + push to `main` after edits + verification.

## Self-check checklist (verify in report)

- [ ] `quest-step3-done` triggers on `wish-fulfilled` (not `companion-moved-in`) and gates on `@companion.Need.affinity >= 60`
- [ ] `quest-step6-done` uses `@colony.Colony.companion_wish_count >= 2` (not `companion_happy_count >= 1`)
- [ ] `game-won` rule renamed to `settlement-founded`, emits only `settlement-founded` + sets step=8 (no `settlement-thrived`, no `game-won`)
- [ ] `quest-banner-8` text is "自由探索中" / "定居点已建立,四个循环自驱,继续你的故事"
- [ ] `vitric.json` gates.must_emit is `settlement-founded`
- [ ] schema.json NOT modified (field already exists from Task 1)
- [ ] wish.js NOT modified (sync already exists from Task 4)
- [ ] narrative.json NOT modified (dead `ending-show` rule left in place, flagged as Minor)
- [ ] `cargo run -p vitric-cli -- check games/frontier` returns `OK`
- [ ] Committed + pushed to `main`

## Report contract

Write the full report to `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-6-report.md` and return only: status (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit hash(es), one-line check summary, and any concerns.
