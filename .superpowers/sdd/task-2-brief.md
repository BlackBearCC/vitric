# Task 2 Brief: Solar Flare + Day/Night System

You are implementing Task 2 of the frontier deepening plan. This task adds a flare + day/night system that drives survival pressure.

## Project Context

- Repo root: `/Users/leolele/Documents/leo/vitric`
- Target files (3): 
  - CREATE `games/frontier/scripts/flare.js`
  - CREATE `games/frontier/rules/flare.json`
  - MODIFY `games/frontier/vitric.json` (register new files)
- Project: Vitric is a deterministic 2D game engine for AI agents. `games/frontier/` is a survival game demo.
- Task 1 (complete, commit 35004ad) added `Colony.flare_timer / flare_warning / is_night / wild_threat` fields.

## CRITICAL: Learn the actual script API first

**The plan's pseudocode used a wrong API.** Before writing any code, READ these existing scripts to learn the real API:

1. `games/frontier/scripts/clock.js` — shows `vitric.system(name, {query, writes}, (entities, ctx) => {...})` pattern. Note: `entities` is an array; for singleton like Colony, use `const c = entities[0]; if (!c) return;`
2. `games/frontier/scripts/colony.js` — shows multi-component query `{query: ["Colony", "Clock"], writes: ["Colony"]}` and accessing `e.Clock.field` / `e.Colony.field` on the same entity
3. `games/frontier/scripts/companion.js` lines 360-380 — shows `vitric.fn(name, (args, ctx) => {...})` for named functions called from rules, and `ctx.ask("llm", prompt, "callbackName")` for LLM calls
4. `games/frontier/rules/companion.json` — shows how rules invoke JS functions via `{"call": "fnName", "with": {...}}`

**Real API summary:**
- `vitric.system(name, {query: [...], writes: [...]}, (entities, ctx) => {...})` — per-tick system. `entities` is array; iterate with `for (const e of entities)`. `ctx.dt` is delta time. `ctx.emit(name, data)` emits event.
- `vitric.fn(name, (args, ctx) => {...})` — named function callable from rules via `{"call": "name", "with": {...}}`. `args` is the `with` object.
- NO `ctx.singleton()`, NO `ctx.each()`, NO `vitric.on()`, NO `vitric.expose()`, NO `vitric.call()` from JS.
- Events emitted from JS are consumed by **rules** (JSON), not by JS handlers. For event-driven JS logic: emit from JS → rule catches → rule calls JS function via `{"call": ...}`.

## Your Requirements (intent, not pseudocode)

### Behavior

1. **Day/night detection** (per-tick system, query Colony + Clock):
   - Read `Clock.time` (0-60 per day, 60 = DAY_SEC). From `clock.js`: morning 0-25%, noon 25-50%, dusk 50-75%, night 75-100%.
   - Night = `Clock.time / CLOCK_DAY_SEC >= 0.75` (CLOCK_DAY_SEC = 60.0, copy from clock.js).
   - When `Colony.is_night` flips 0→1: set `wild_threat = 1 + Math.floor(Clock.day / 3)` (caps loosely at 3+), emit `night-fall{threat}`.
   - When flips 1→0: set `wild_threat = 0`, emit `dawn-break{}`.

2. **Flare timer** (same system, query Colony):
   - Decrement `Colony.flare_timer` by `ctx.dt` each tick.
   - When timer in (0, 30] and `flare_warning != 1`: set `flare_warning = 1`, emit `flare-imminent{eta: timer}`.
   - When timer > 30 and `flare_warning != 0`: set `flare_warning = 0` (clear warning if somehow still set).
   - When timer <= 0: 
     - Emit `flare-hit{power_loss: Colony.power * 0.4, o2_loss: Colony.oxygen * 0.4}`
     - Set `Colony.power = Colony.power * 0.6`
     - Set `Colony.oxygen = Colony.oxygen * 0.6`
     - Set `flare_warning = 0`
     - Set `flare_timer = 180 + Math.floor(Math.random() * 120)` (3-5 min cooldown)
     - Return (don't continue decrementing this tick)
   - Else: write back `Colony.flare_timer = timer`.

3. **Rules (flare.json)** — catch events and show toasts (use existing `toast-show` event pattern from other rules):
   - `flare-imminent` → `{"call": "toastShow", "with": {"text": "耀斑 30 秒后来袭!储备电力氧气!"}}` (check existing toast rules for exact pattern)
   - `flare-hit` → toast "耀斑冲击!电力氧气大幅下降"
   - `night-fall` → toast "夜幕降临,野外危险,速回基地"
   - `dawn-break` → toast "天亮了,新的一天"

   **Check `games/frontier/rules/toast.json` first** to see the actual toast-show invocation pattern and the function name (might be `toastShow` or `toast-show` or similar).

### File registrations (vitric.json)

Add `"rules/flare.json"` to the `rules` array (after `"rules/toast.json"` to keep ordering logical).
Add `"scripts/flare.js"` to the `scripts` array (after `"scripts/toast.js"`).

### Verification

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -15`
Expected: exit code 0, no parse errors. (Engine emits a JSON diagnostic report, not literal "OK". Success = exit 0 + no error strings that aren't system names.)

### Commit + push

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/flare.js games/frontier/rules/flare.json games/frontier/vitric.json
git commit -m "feat(frontier): add solar flare + day/night cycle system"
git push origin main
```

## Global Constraints

- Engine zero-change: all work in `games/frontier/`
- Code comments in English (project rule)
- String literals (toast text) stay Chinese (game content language)
- `vitric check games/frontier` must exit 0 at end
- Auto commit + push to main

## Self-Review Checklist

Before reporting DONE:
- [ ] flare.js uses correct API (`vitric.system` with `{query, writes}`, no `ctx.singleton`/`ctx.each`/`vitric.on`)
- [ ] Both day/night and flare timer handled (can be one system or two — your call based on clarity)
- [ ] flare.json rules use the actual toast invocation pattern (verified from toast.json)
- [ ] vitric.json registers both new files
- [ ] `vitric check` exits 0
- [ ] Committed and pushed to main
- [ ] Comments in English, toast strings in Chinese

## Report

Write your full report to: `/Users/leolele/Documents/leo/vitric/.superpowers/sdd/task-2-report.md`

Report format:
- Status: DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED
- Commits: list of commit hashes
- Test summary: one line
- Concerns: any doubts

Return to controller: status, commits, one-line test summary, concerns. Nothing else.
