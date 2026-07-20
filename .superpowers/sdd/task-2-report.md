# Task 2 Report: Solar Flare + Day/Night System

- Status: DONE
- Commits:
  - `959ee3b` — feat(frontier): add solar flare + day/night cycle system (pushed to main, `35004ad..959ee3b`)
- Test summary: `cargo run -p vitric-cli -- check games/frontier` exits 0; diagnostic report lists the `flare` system with query `[Colony, Clock]` and writes `[Colony]`, and all 4 toast rules parse.
- Concerns:
  - Scripts in `games/frontier/scripts/` share a single global JS scope, so `const CLOCK_DAY_SEC` in clock.js collided with the same name in flare.js. Renamed my local to `FLARE_DAY_SEC` (still 60.0, mirrors clock.js). Worth noting for any future script that wants to reuse clock constants — there is no module system, each top-level `const`/`function` lives in the shared eval scope.
  - The brief's pseudocode mentioned `{"call": "toastShow", ...}` for the toast rules, but `rules/toast.json` actually uses inline `{"set": "@toast_lbl.UiLabel.content", ...}` + `{"set": "@toast_lbl.Toast.timer", ...}` directives with no `toastShow` function. Followed the existing pattern (verified against toast.json).
  - `wild_threat` uses the brief's literal formula `1 + Math.floor(Clock.day / 3)` with no hard clamp; the brief's "(caps loosely at 3+)" reads as descriptive of typical 5-7 day sessions, not a directive to add a `Math.min`. Over longer sessions the value keeps growing — flagging in case a hard cap was intended.
  - `Math.random()` is used for the flare cooldown as the brief specifies; the engine is described as deterministic, so replay determinism across runs may diverge here. Did not change because the brief explicitly mandated this expression.

---

## Follow-up Fix: Replace poisoned `Math.random()` with `ctx.random()`

- **Status:** DONE
- **Commit:** `300cf73` — fix(frontier): use ctx.random() instead of poisoned Math.random() in flare system (pushed to main, `959ee3b..300cf73`)
- **Lines changed:**
  - `games/frontier/scripts/flare.js:49` — `Math.floor(Math.random() * 120)` → `Math.floor(ctx.random() * 120)`
- **Rationale:** The engine's QuickJS runtime poisons `Math.random` and throws at runtime, pointing users to `ctx.random()`. `ctx` is already in scope as the second parameter of the system callback `(entities, ctx) => {...}`. Only the RNG source was swapped; `Math.floor` retained. No other logic changed.
- **Grep verification:** `Grep` for `Math\.random` in flare.js returns "No matches found" after the edit.
- **Test command:** `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
- **Test output:** exit code `0`; diagnostic report lists the `flare` system with query `[Colony, Clock]` and writes `[Colony]`.
