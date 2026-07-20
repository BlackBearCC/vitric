# Task 6 Report — Quest System: Convert to Milestone (settlement-founded)

## Status

DONE_WITH_CONCERNS — All edits applied, `cargo run -p vitric-cli -- check games/frontier` exits 0 with JSON success report. One Minor finding flagged per brief (dead `ending-show` rule in narrative.json, intentionally left in place).

## Files changed

- `games/frontier/rules/quest.json` — 4 rule edits
- `games/frontier/vitric.json` — 1 gate edit

`git diff --stat HEAD~1`:
```
 games/frontier/rules/quest.json | 26 ++++++++++++++------------
 games/frontier/vitric.json      |  2 +-
 2 files changed, 15 insertions(+), 13 deletions(-)
```

## Edits applied (per brief)

### Edit 1 — `quest-step3-done` (quest.json:23-35)
Changed trigger from `companion-moved-in` to `wish-fulfilled` and added `@companion.Need.affinity >= 60` gate alongside the existing `step == 3` guard. Action block unchanged (set step=4, emit `quest-done` with `first-companion`).

### Edit 2 — `quest-step6-done` (quest.json:63-77)
Replaced `@colony.Colony.companion_happy_count >= 1` predicate with `@colony.Colony.companion_wish_count >= 2`. Comment updated to reflect the new aggregate-wish gate. Other predicates (step==6, day>=5, pop>=3) unchanged.

### Edit 3 — `game-won` rule renamed to `settlement-founded` (quest.json:86-99)
Renamed rule id `game-won` → `settlement-founded`. `do` block now emits only `settlement-founded` and sets `step=8`. Removed `settlement-thrived` and `game-won` emissions. Predicates unchanged.

### Edit 4 — `quest-banner-8` (quest.json:165-173)
Banner text changed from `"聚落兴旺"` / `"通关!"` to `"自由探索中"` / `"定居点已建立,四个循环自驱,继续你的故事"` (Chinese verbatim per brief — correct).

### Edit 5 — `vitric.json` gates.must_emit (vitric.json:48)
Changed `"must_emit": "game-won"` → `"must_emit": "settlement-founded"`.

## Verification

Command run (from `/Users/leolele/Documents/leo/vitric`):
```
cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -15
```

Result: exit code 0. The CLI emits a JSON success report (assets/rules/scripts summary) rather than the literal string "OK" — exit code 0 is the success indicator. Verbatim tail:
```
    }
  ]
}
```
Verbatim head (showing successful load):
```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running `target/debug/vitric check games/frontier`
{
  "assets": {
    "count": 17,
```

`vitric gate` was NOT run, per brief — qa/clear.json still emits `game-won` and will fail until Task 9 re-records.

## Commit

- Hash: `06464fb`
- Message: `feat(frontier): convert quest to milestone-based (settlement-founded), gate on settlement-founded`
- Pushed to: `origin/main` (`eb0ac67..06464fb main -> main`)
- Files in commit: `games/frontier/rules/quest.json`, `games/frontier/vitric.json` only (verified via `git diff --stat HEAD~1`).

## Self-check checklist

- [x] `quest-step3-done` triggers on `wish-fulfilled` (not `companion-moved-in`) and gates on `@companion.Need.affinity >= 60`
- [x] `quest-step6-done` uses `@colony.Colony.companion_wish_count >= 2` (not `companion_happy_count >= 1`)
- [x] `game-won` rule renamed to `settlement-founded`, emits only `settlement-founded` + sets step=8 (no `settlement-thrived`, no `game-won`)
- [x] `quest-banner-8` text is "自由探索中" / "定居点已建立,四个循环自驱,继续你的故事"
- [x] `vitric.json` gates.must_emit is `settlement-founded`
- [x] schema.json NOT modified (field already exists from Task 1)
- [x] wish.js NOT modified (sync already exists from Task 4)
- [x] narrative.json NOT modified (dead `ending-show` rule left in place, flagged as Minor)
- [x] `cargo run -p vitric-cli -- check games/frontier` exits 0 (success)
- [x] Committed + pushed to `main`

## Concerns / Minor findings

1. **Dead `ending-show` rule in `rules/narrative.json`** — Per brief, left untouched (YAGNI / out of Task 6 scope). The rule triggers on `game-won` event. Since Task 6 removed the only `game-won` emission, this rule is now unreachable and the ending panel will never show. This is the intended behavior (no hard ending — game continues into free play). Flag for Task 10 docs/cleanup pass if desired.

2. **`qa/clear.json` stale** — Expected; emits `game-won` instead of `settlement-founded`. `vitric gate` will fail until Task 9 re-records. Not a Task 6 concern.

3. **Dev tools referencing `game-won`** — `tools/record_clear.py` and `tools/test_progression.py` still reference `game-won` per brief's Out-of-scope section. Dev-only, not runtime. Will be addressed in Task 10 docs pass if needed.

4. **CLI `check` output format** — Brief said expected output is `OK`, but the actual CLI emits a JSON report (success indicated by exit code 0, not a literal "OK" string). This is a documentation nuance in the brief, not a code issue.
