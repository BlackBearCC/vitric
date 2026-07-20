# Task 1 Brief: Schema Extension

You are implementing Task 1 of the frontier deepening plan. This is a JSON schema edit task — mechanical, single file.

## Project Context

- Repo root: `/Users/leolele/Documents/leo/vitric`
- Target file: `games/frontier/schema.json` (single file edit)
- Project: Vitric is a deterministic 2D game engine for AI agents. `games/frontier/` is a survival game demo where AI companions help build a settlement.
- Engine reads `schema.json` to register ECS components. Component fields use types: `text`, `number`, `int`, `bool`, `enum`, `list`, `entity`. Lists of structs are NOT supported — use `text` with JSON string for nested data.

## Your Requirements (exact, verbatim)

### Step 1: Add `Wish` component

Insert before `"QuestLog"` in `components`:

```json
    "Wish": {
      "fields": {
        "items": {
          "type": "text",
          "default": "[]"
        },
        "fulfilled": {
          "type": "int",
          "default": 0
        }
      }
    },
```

`items` stores a JSON string like `[{"desc":"建 3 个结构","done":false,"kind":"build","target":3,"progress":0}, ...]`. `fulfilled` counts how many wishes are done.

### Step 2: Add `Poi` component

Insert after `Wish`:

```json
    "Poi": {
      "fields": {
        "kind": {
          "type": "text",
          "default": "abandoned-camp"
        },
        "state": {
          "type": "enum",
          "variants": ["fresh", "looted", "depleted"],
          "default": "fresh"
        },
        "cooldown": {
          "type": "number",
          "default": 0
        },
        "reward_table": {
          "type": "text",
          "default": "{}"
        }
      }
    },
```

### Step 3: Extend `Colony` component

Add 5 new fields after `companion_affinity_avg` (the last Colony field):

```json
        "flare_timer": {
          "type": "number",
          "default": 240
        },
        "flare_warning": {
          "type": "int",
          "default": 0
        },
        "is_night": {
          "type": "int",
          "default": 0
        },
        "wild_threat": {
          "type": "int",
          "default": 0
        },
        "companion_wish_count": {
          "type": "int",
          "default": 0
        }
```

Defaults explained:
- `flare_timer=240` = 4 minutes (240 seconds) until first flare (gives early-game breathing room)
- `flare_warning=0` = no warning active
- `is_night=0` = starts at daytime
- `wild_threat=0` = no threat during day
- `companion_wish_count=0` = aggregate wish counter for quest gating

### Step 4: Extend `Need` component

Add new field after `contribution_timer` (the last Need field):

```json
        "memory_unlocked": {
          "type": "int",
          "default": 0
        }
```

### Step 5: Verify

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK` (schema parses successfully)

### Step 6: Commit + push

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/schema.json
git commit -m "feat(frontier): add Wish/Poi components, extend Colony/Need for deepening"
git push origin main
```

## Global Constraints

- Engine zero-change: all work in `games/frontier/` only
- Code comments in English (but this task has no comments — pure JSON)
- String literals stay Chinese (game content language)
- `vitric check games/frontier` must pass at end
- Auto commit + push to main after completion (user preference)

## Self-Review Checklist

Before reporting DONE:
- [ ] All 4 additions present: Wish component, Poi component, 5 Colony fields, 1 Need field
- [ ] JSON is valid (no trailing commas, proper brackets)
- [ ] `vitric check games/frontier` returns OK
- [ ] Committed and pushed to main

## Report

Write your full report to: `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-1-report.md`

Report format:
- Status: DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED
- Commits: list of commit hashes
- Test summary: one line ("vitric check OK")
- Concerns: any doubts (observations about the codebase are fine)

Return to controller: status, commits, one-line test summary, concerns. Nothing else.
