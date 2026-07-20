# Task 1 Report — Region dormant/active/frozen

## 1. Status

**DONE_WITH_CONCERNS**

All four verification commands pass. Re-recording `games/frontier/qa/clear.json` was required because the new mountain marker entity changes the tick-0 state hash (legitimate scene change → legitimate recording invalidation). The new recording passes the gate with the `settlement-founded` event emitted.

## 2. Commits

- `4118473` — `feat(ecs,sim,render): E1 Region dormant/active` (pushed to `origin/main`)

## 3. Test results

| Command | Result |
|---|---|
| `cargo test -p vitric-cli --test region --` | **PASS** — 4/4 tests pass (`dormant_entities_excluded_from_query`, `dormant_entities_skipped_in_render`, `dormant_entities_skip_logic_systems`, `thaw_region_activates_entities`) |
| `cargo test --workspace` | **PASS** with one pre-existing environmental failure unrelated to Task 1: 2 typescript tests (`typescript_system_runs_after_transpile`, `typescript_syntax_error_names_the_file`) panic with `"测试需要 esbuild：仓库里跑 cd mcp && npm install，或设 ESBUILD_BIN"`. Verified pre-existing by `git stash`-ing Task 1 changes and re-running — the same 2 tests fail identically. All other tests pass. |
| `cargo run --release -- check games/frontier` | **PASS** — `entities: 426`, `initial_hash: 0x0b68b61d57750ff1` |
| `cargo run --release -- gate games/frontier` | **PASS** — schema check pass; playthrough replay pass (37249 ticks, `final_hash: 0xab58ec29d99275df`, `settlement-founded` verified) |

## 4. Files touched

| File | Lines |
|---|---|
| `crates/vitric-ecs/src/world.rs` | +18 (query filter + `is_dormant` helper) |
| `crates/vitric-render/src/lib.rs` | +15 (`is_renderable` helper + 2 defensive guards in `render_with` and `describe_world_with_assets`) |
| `crates/vitric-sim/src/sim.rs` | +74 (`pending_events` field + `thaw_region` + `accumulate_dormant_ticks` + `step()` pipeline + `snapshot`/`restore` manual (de)serialization of `pending_events`) |
| `games/frontier/schema.json` | +14 (new `Region` component with 10 fields) |
| `games/frontier/scenes/main.json` | +1 / -1 (new `mountain` region marker entity at index 425; total entities 426) |
| `games/frontier/qa/clear.json` | re-recorded (37249 ticks, captures new initial state) |
| `crates/vitric-cli/tests/region.rs` | new test file, 4 tests |
| `.superpowers/sdd/briefs/task-1-brief.md` | new (single source of truth, committed alongside the code) |

## 5. Deviations from brief

The brief's pseudocode showed intent; the implementation adapted to the real APIs as follows:

1. **`TestSim` does't exist** — Tests use `Runtime::boot(dir)` returning `(Sim, Runtime)` for full scene + logic tests (matching the `saves.rs` / `glow.rs` pattern), and direct `World::new()` for isolated world-level checks where schema validation isn't needed.
2. **`get_component` returns `Result<&Value, EcsError>`, not `Option`** — Adapted `if let Some(region)` to `if let Ok(region)` in both `World::is_dormant` and `Sim::accumulate_dormant_ticks`.
3. **`Sim` has no `ctx` or `logic_systems` field** — For `thaw_region` event emission, added a `pending_events: Vec<Event>` field directly to `Sim`, push in `thaw_region`, flush in `step()` section 1.6 (after external replies, before gravity). For dormant filtering in logic: happens naturally because `GameLogic::on_tick` calls `world.query`, which now filters dormant.
4. **`describe_world` JSON fields are `visible` and `offscreen`** — Brief pseudocode used `on_screen`/`off_screen`; adapted test assertions to the real field names.
5. **`Event` doesn't derive `Serialize`/`Deserialize`** — Manual serialization in `snapshot()` (`json!({"name": e.name, "data": Value::Object(e.data.clone())})`) and manual deserialization in `restore()` (forward-compatible: old snapshots pre-dating `pending_events` are treated as empty).
6. **`thaw_region_activates_entities` test initially checked `rt.drain_observed()`** — Wrong channel: `region-thaw` is fed TO the logic as input (via `pending_events`), not emitted BY the logic. Changed assertion to check `report.events` (the `StepReport.events` field — list of events fed to the logic this tick). Also added a check that `dormant_ticks` stays at 0 for the now-active region (since `accumulate_dormant_ticks` skips active regions).
7. **Re-recording `qa/clear.json`** — Adding the `mountain` region marker entity to `scenes/main.json` changes the tick-0 state hash (`0xf1787886578a150a` → `0x0b68b61d57750ff1`), so the existing recording diverges at tick 0. Re-ran `python3 games/frontier/tools/record_clear.py` to regenerate the recording (37249 ticks, `final_hash 0xab58ec29d99275df`, `settlement-founded` emitted). This is consistent with the established precedent in this repo (memory: re-recording is the standard response to legitimate scene changes).

## 6. Concerns

1. **Re-recording precedent** — The gate recording is now invalidated by *any* scene change that affects the initial state hash. Task 2/3/4 (catch-up, RNG substreams, view-frustum culling) will likely touch scenes or schema again and require re-recording. Consider automating this in CI if it becomes a recurring pain point.
2. **Pre-existing typescript test failures** — 2 tests in `crates/vitric-cli/tests/typescript.rs` require `esbuild` binary (`cd mcp && npm install` or `ESBUILD_BIN`). Unrelated to Task 1 (verified by stash test). The reviewer may want to set up `esbuild` separately.
3. **`thaw_region` is not idempotent on already-active regions** — Calling it on an active region re-sets `state="active"` (no-op) but still emits a `region-thaw` event. Rules can dedupe based on the `discovered` field. Documented in the doc comment, but worth noting for Task 2's catch-up logic.
4. **Schema field audit** — The `Region.state` enum (`dormant`/`active`/`frozen`) is properly declared in `schema.json` under `components.Region.fields.state.variants`. The dormant filter reads this field via `region.get("state").and_then(|v| v.as_str())` (NOT through `ctx.getField` / `@entity.Region.state` rule syntax), so it's not subject to the "undeclared field crash" lesson from the Frontier deepening. Still, the pattern applies going forward — any new field added to `Region` and read by rules must be declared.
5. **`pending_events` is NOT recorded by the recording** — This is intentional (host API calls are deterministic given the same host program), but it means a *different* host program calling `thaw_region` at a different tick would diverge. This is the same model as direct `Sim` method calls in general — recordings only capture the input stream, not arbitrary host API calls. Documented in the field's doc comment.
6. **Defensive `is_renderable` guard added redundantly** — `world.query` already filters dormant, so the explicit `if !is_renderable(world, id) { continue; }` in `render_with` and `describe_world_with_assets` is technically dead code today. Kept it because the brief's pseudocode showed it as the intended invariant guard, and it survives any future refactor that bypasses `query`. If the reviewer prefers DRY, both guards can be removed — the `query` filter alone is sufficient.

## Fix for I1

- **Status:** DONE
- **Commit hash:** `a9e1167ef7eb6832520dfd8b59537718f56e4b9a` (pushed to `origin/main`)
- **Test results:**

| Command | Result |
|---|---|
| `cargo test -p vitric-cli --test region --` | **PASS** — 4/4 (`dormant_entities_excluded_from_query`, `dormant_entities_skipped_in_render`, `thaw_region_activates_entities`, `dormant_entities_skip_logic_systems`) |
| `cargo build -p vitric-sim` | **PASS** — clean compile, no warnings |
| `cargo test --workspace` | **PASS** (modulo pre-existing) — only the 2 known typescript tests fail (`typescript_system_runs_after_transpile`, `typescript_syntax_error_names_the_file`) with the esbuild-missing message; all other tests pass, no regressions |

- **Files touched:** `crates/vitric-sim/src/sim.rs` only (7 insertions: `was_discovered` computation, conditional `invoke_catch_up_for_region(id)` call, and the no-op stub method with its doc comment). The existing doc comment on `thaw_region` was preserved.
