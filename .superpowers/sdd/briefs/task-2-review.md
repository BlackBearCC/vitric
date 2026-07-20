# Task 2 Review — E2 catch_up system API

## 1. Verdict

**APPROVED**

The implementation satisfies every spec item in the brief, honors all approved architectural deviations, and the implementer-reported `was_dormant` deviation is correct (see §3). No Critical or Important findings. Code is deterministic, backward-compatible, and follows existing patterns.

## 2. Spec compliance

| # | Requirement | Status | Evidence |
|---|---|---|---|
| 1 | `vitric.system` accepts optional 4th arg; throws if non-null non-function passed | ✅ | `prelude.js:14,32-34` — signature `(name, decl, fn, catch_up_fn)`; validation `catch_up_fn !== undefined && catch_up_fn !== null && typeof catch_up_fn !== "function"` throws Chinese error matching existing pattern |
| 2 | `__runCatchUp(idx, entityHandle, dormantTicks, payloadJson)` exists; invokes catch_up with `(entity, ctx, dormantTicks)` | ✅ | `prelude.js:356-371` — `globalThis.__runCatchUp = function (idx, entityHandle, dormantTicks, payloadJson) { ... sys.catch_up(entityHandle, ctx, dormantTicks); ... }`; throws if `!sys.catch_up` (defensive — Rust side already skips) |
| 3 | `SystemDecl` has `has_catch_up: bool`; `__list` includes `catch_up: true/false` | ✅ | `lib.rs:151` `pub has_catch_up: bool`; `prelude.js:401` `catch_up: !!s.catch_up`; `lib.rs:273` `has_catch_up: s.get("catch_up").and_then(\|v\| v.as_bool()).unwrap_or(false)` |
| 4 | `Sim::thaw_region` queues region_id into `pending_catch_ups` conditionally | ✅ | `sim.rs:516-528` — `was_dormant` check (state was "dormant" or "frozen") → `self.pending_catch_ups.push(id.to_string())` |
| 5 | `Sim::step` flushes `pending_catch_ups` BEFORE `logic.on_tick`; resets `Region.dormant_ticks` to 0 after | ✅ | `sim.rs:311-342` — step 4.5 loop (step 5 is `logic.on_tick` at line 344-347); `set_field(region_e, "Region.dormant_ticks", json!(0))` at line 341 |
| 6 | `GameLogic::catch_up_region` exists with default no-op impl | ✅ | `sim.rs:431-440` — `fn catch_up_region(...) -> Result<(), String> { Ok(()) }` |
| 7 | `Runtime::catch_up_region` calls `ScriptEngine::run_catch_up_for_region` | ✅ | `runtime.rs:36-51` — `self.scripts.run_catch_up_for_region(...)`; extends `observed` + `carryover` with emitted events (mirrors `on_tick` convention) |
| 8 | `ScriptEngine::run_catch_up_for_region` iterates systems with catch_up, filters by region + query, invokes `__runCatchUp` | ✅ | `lib.rs:189-248` — `if !decl.has_catch_up { continue; }`; `world.entities()` (not `query`); filters `Region.id == region_id` AND `query.iter().all(\|c\| world.has_component(id, c))`; calls `__runCatchUp` per entity with `WORLD_PTR` window |
| 9 | `crop-grow` declares catch_up as 4th arg; uses `dormant_ticks * (1/60)` as elapsed seconds | ✅ | `crops.js:46-63` — 4th arg arrow fn; `const dormantSec = dormantTicks / CROP_TICK_PER_SEC` where `CROP_TICK_PER_SEC = 60` (= `dormantTicks * (1/60)`); advances `Crop.timer`/`Crop.stage` |
| 10 | New test `catch_up_advances_dormant_crop_on_thaw` asserts timer > 0 after thaw + step | ✅ | `region.rs:79-132` — `assert!(timer_after > 0.0, ...)`; also asserts `Region.dormant_ticks == 0` on region entity after catch-up (extra check, good) |
| 11 | Commit message matches brief's exact text | ✅ | Commit `182e812` — `feat(script,sim): E2 catch_up system API` with the three-line body verbatim |
| 12 | Backward compat: existing tests + frontier gate still pass | ✅ | Report: `cargo test --workspace` green (except 2 pre-existing esbuild-dependent `typescript.rs` failures); `cargo run --release -- gate games/frontier` passes with matching hash `0xab58ec29d99275df` |
| 13 | `pending_catch_ups` in `snapshot()` + `restore()` | ✅ | `sim.rs:611` snapshot includes `"pending_catch_ups": self.pending_catch_ups`; `sim.rs:635-643` restore with forward-compat fallback to empty for old snapshots |
| 14 | Determinism: no `SystemTime`, no `thread_rng()`; catch_up fn uses only `getField`/`setField`/`dormantTicks` | ✅ | JS catch_up fn (`crops.js:53-62`) uses only `ctx.getField`/`ctx.setField` + arithmetic; no `Math.random`/`Date`/`ctx.random`. Prelude's determinism guards (lines 70-89) still in place. Rust side passes `rng` through but catch_up doesn't consume it |

## 3. Deviation evaluation: `was_dormant` vs `was_discovered`

**Yes — `was_dormant` is correct. APPROVE.**

Reasoning:
1. **The test requires first-thaw catch_up.** `region.rs:95-99` sets the test entity's Region with `discovered: 0` (matching `scenes/main.json`'s mountain region initial state). The brief's test pseudocode (lines 22-24) also uses `discovered: 0`. After 3600 ticks + thaw + step, the test asserts `timer_after > 0.0`. With `was_discovered` (discovered == 1 before thaw), catch_up would NOT queue on first thaw → timer stays 0 → test fails. The test as written (and the brief's own pseudocode) requires catch_up to fire on first thaw of a never-discovered region.

2. **`was_dormant` is semantically correct.** Catch_up's purpose is to reconcile state for entities that were NOT simulated while their region was dormant. If the region's state was "dormant" or "frozen", entities in it were skipped by `world.query()` (Task 1's dormant filter), so they have stale state that needs fast-forwarding. If the region was already "active", `dormant_ticks` is 0 (Task 1's `accumulate_dormant_ticks` only increments on dormant/frozen), so catch_up would be a no-op anyway — the `was_dormant` guard is a cheap short-circuit, not a semantic restriction.

3. **Spec intent is "reconcile dormant entities", not "only re-thaw previously-discovered".** The brief's prose (line 10) says: "On region thaw, engine invokes catch_up for each entity in the region." No mention of "previously discovered" or "re-thaw". The `was_discovered` check was an artifact of Task 1's stub, not a brief requirement. The "Known architectural deviations" note (point 2) says `thaw_region` pushes to `pending_catch_ups` but does NOT specify the condition — leaving it to the implementer.

4. **Already-active regions don't queue.** The guard correctly skips already-active regions (dormant_ticks is 0, nothing to reconcile), so there's no spurious catch_up.

## 4. Code quality findings

### Critical
None.

### Important
None.

### Minor
None worth flagging. (Observations only, not defects:
- The `__runCatchUp` JS guard `if (!sys.catch_up) throw ...` is defensive — Rust side already skips systems without `has_catch_up`. Good practice, not dead code.
- The test entity's own `Region.dormant_ticks` (on `mountain_crop`) is NOT reset by catch_up — only the region entity named in `thaw_region` is reset. This is by design (catch_up resets the budget on the region entity, not on every entity in the region). The test doesn't assert this either way.
- `WORLD_PTR.with(\|p\| p.set(...))` / clear pattern around the JS call matches the existing `run_one_system` pattern — pre-existing panic-safety characteristics, not introduced here.)

## 5. Cannot verify from diff

- **Pre-existing `typescript.rs` test failures** (require `esbuild`). Implementer reports these are environmental and unrelated. Controller should confirm by running `cargo test -p vitric-cli --test typescript` on a machine with esbuild installed, or by checking these tests were failing before commit `182e812`.
- **Frontier gate hash stability.** Report claims `0xab58ec29d99275df` matches `qa/clear.json`. Controller should re-run `cargo run --release -- gate games/frontier` to confirm the hash is bit-identical to the pre-Task-2 baseline (i.e., Task 2 didn't perturb the deterministic trajectory). The catch_up code path is only exercised on `thaw_region`, which the gate's playthrough may or may not trigger — if it doesn't trigger, the hash is unaffected by definition; if it does, the catch_up must produce the same result on every run.
- **`cargo test --workspace` full green.** Implementer ran it; controller should re-run to confirm no flaky tests.
