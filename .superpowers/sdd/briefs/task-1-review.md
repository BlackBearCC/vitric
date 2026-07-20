# Task 1 Review — Region dormant/active/frozen

## 1. Verdict

**NEEDS_FIX**

Counts: Critical 0 · Important 1 · Minor 3.

One Important finding blocks approval: the `Sim::thaw_region` implementation omits the `was_discovered` check AND the `invoke_catch_up_for_region` stub that the brief's Step 12 pseudocode specifies. The controller's "What to check" list explicitly asks for "calls `invoke_catch_up_for_region` stub". This deviation is NOT in the controller's list of expected ambiguities, and the implementer's report does not acknowledge the omission. The stub is functionally a no-op today, but it is the contracted integration point for Task 2's catch-up scheduling.

## 2. Spec compliance

| Item | Status | Evidence |
|---|---|---|
| `Region` component schema matches brief (fields, types, defaults, enum variants) | ✅ | `games/frontier/schema.json` — 10 fields, `state` enum `["dormant","active","frozen"]` default `"active"`, `spawn_timer` default `7200` |
| `is_dormant` returns true for `dormant` AND `frozen` | ✅ | `crates/vitric-ecs/src/world.rs:248-255` — `state == "dormant" \|\| state == "frozen"` |
| `world.query` filters dormant entities | ✅ | `crates/vitric-ecs/src/world.rs:237-243` — `.filter(\|&id\| !self.is_dormant(id))` |
| `render_world` AND `describe_world` skip dormant entities | ✅ | `crates/vitric-render/src/lib.rs` — both entity loops guarded by `if !is_renderable(world, id) { continue; }` (lines 305 and 597 in current file) |
| `Sim::thaw_region` sets `state="active"` | ✅ | `crates/vitric-sim/src/sim.rs:174` — `region["state"] = json!("active")` |
| `Sim::thaw_region` sets `discovered=1` | ✅ | `crates/vitric-sim/src/sim.rs:175` — `region["discovered"] = json!(1)` |
| `Sim::thaw_region` emits `region-thaw` event | ✅ | `crates/vitric-sim/src/sim.rs:177` — `Event::new("region-thaw", json!({"id": id}))` pushed to `pending_events`, drained in `step()` §1.6 (sim.rs:251) |
| `Sim::thaw_region` calls `invoke_catch_up_for_region` stub | ❌ | `crates/vitric-sim/src/sim.rs:171-178` — no call; `invoke_catch_up_for_region` is not defined anywhere in the codebase (verified via grep). Brief Step 12 also specifies a `was_discovered` conditional that is missing too. |
| `accumulate_dormant_ticks` increments `dormant_ticks` on dormant/frozen regions each tick | ✅ | `crates/vitric-sim/src/sim.rs:424-432` — iterates `world.entities()` (not `query`), filters dormant/frozen, `set_field(id, "Region.dormant_ticks", json!(dt + 1))`. Called from `step()` §5.5 (sim.rs:279). |
| `state_hash` still covers dormant entities | ✅ | `crates/vitric-ecs/src/world.rs:353-357` — `state_hash` serializes via `CanonicalWorld` which iterates ALL entities (not `query`). Dormant entities are part of the hashed state. |
| All 4 brief tests exist and assert what the brief specifies | ✅ | `crates/vitric-cli/tests/region.rs` — 4 tests with adapted assertions (real `visible`/`offscreen` field names, `report.events` instead of `recent_events()`, `Runtime::boot` instead of `TestSim`). All deviations are in the controller's expected-ambiguities list. |
| Mountain marker entity in `scenes/main.json` with brief's Region data | ✅ | `games/frontier/scenes/main.json` — `{"name":"mountain","components":{"Region":{"id":"mountain","biome":"mountain","state":"dormant","discovered":0,"anchor_x":0,"anchor_y":12,"w":30,"h":28,"dormant_ticks":0,"spawn_timer":7200}}}`. Matches brief test data + `spawn_timer` (schema default). |
| Commit message matches brief's exact text | ✅ | `git log 4118473` — body is byte-identical to brief Step 15 (4-line summary, "state_hash still covers dormant entities. Sim::thaw_region() transitions state and emits region-thaw event."). |

## 3. Code quality findings

### Critical

None.

### Important

**I1. Missing `invoke_catch_up_for_region` stub and `was_discovered` conditional in `thaw_region`**
- File: `crates/vitric-sim/src/sim.rs:171-178`
- The brief's Step 12 pseudocode specifies:
  ```rust
  let was_discovered = region.get("discovered").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
  // ... set state + discovered + emit ...
  if was_discovered {
      self.invoke_catch_up_for_region(id);
  }
  
  fn invoke_catch_up_for_region(&mut self, _region_id: &str) {
      // Stub — implemented in Task 2
  }
  ```
- The implementation omits both the conditional and the stub method entirely. `grep -rn "invoke_catch_up_for_region\|was_discovered" crates/vitric-sim/src/` returns no matches.
- This is not in the controller's list of expected ambiguities, and the implementer's report does not acknowledge it as a deviation (it only mentions "Catch-up logic for previously-discovered regions is Task 2 (out of scope here)" in the `thaw_region` doc comment — that explains why catch-up is out of scope, but doesn't explain why the contracted stub integration point is missing).
- **Why it matters**: Task 2's brief will expect `invoke_catch_up_for_region` to exist as the integration point. Without the stub, Task 2 has to modify `thaw_region` itself to add the call, which is an extra coupling the brief did not intend. The `was_discovered` conditional also encodes important semantics: catch-up only runs when re-thawing a previously-discovered region (e.g., frozen → active), not on first discovery.
- **Fix**:
  ```rust
  pub fn thaw_region(&mut self, id: &str) {
      let Ok(region_e) = self.world.entity(id) else { return; };
      let Ok(mut region) = self.world.get_component(region_e, "Region").cloned() else { return; };
      let was_discovered = region.get("discovered").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
      region["state"] = json!("active");
      region["discovered"] = json!(1);
      let _ = self.world.set_component(region_e, "Region", region);
      self.pending_events.push(Event::new("region-thaw", json!({"id": id})));
      if was_discovered {
          self.invoke_catch_up_for_region(id);
      }
  }
  
  /// Catch-up scheduling for a re-thawed region. Stub — implemented in Task 2.
  fn invoke_catch_up_for_region(&mut self, _region_id: &str) {}
  ```

### Minor

**M1. `thaw_region` silently no-ops on missing region / missing Region component**
- File: `crates/vitric-sim/src/sim.rs:172-173`
- Brief pseudocode uses `.expect("region entity must exist")` (programmer-error panic). Implementer chose silent no-op (`let Ok(...) else { return; }`). Doc comment documents this as "defensive: the host may call this speculatively".
- This is safer than the brief — won't crash on speculative host calls — and is documented. Not a defect; just a deviation worth noting. No fix needed.

**M2. `thaw_region` is not idempotent on already-active regions**
- File: `crates/vitric-sim/src/sim.rs:171-178`
- Calling on an active region re-emits `region-thaw`. The implementer's report flags this in Concerns §3 and documents it in the doc comment. Rules can dedupe based on `discovered`. Not a defect; behavior note for Task 2.

**M3. `accumulate_dormant_ticks` increment behavior not directly tested**
- File: `crates/vitric-cli/tests/region.rs`
- The `thaw_region_activates_entities` test checks that `dormant_ticks` stays at `0` for an active region (negative case). No test verifies that a dormant region's `dormant_ticks` actually increments each tick (positive case). The brief does not require this test, so it's not a spec gap, but it is a missing coverage for documented behavior. A 1-2 line addition to `dormant_entities_skip_logic_systems` asserting `Region.dormant_ticks == 60` after the 60-tick loop would close the gap.

## 4. Cannot verify from diff

- **Pre-existing typescript test failures** (`typescript_system_runs_after_transpile`, `typescript_syntax_error_names_the_file`) — claimed pre-existing by the implementer via `git stash` test. Cannot confirm from diff alone. Controller should confirm by running `cargo test -p vitric-cli --test typescript` on the parent commit (`c0c7af5a`).
- **`cargo test -p vitric-cli --test region --` actually passing** — tests compile and read correctly, but I cannot run them. Controller should re-run.
- **`cargo run --release -- gate games/frontier` actually passing with re-recorded `qa/clear.json`** — the new recording's `final_hash: 0xab58ec29d99275df` and 37249-tick length are reported but not independently verified.
- **`check games/frontier` initial_hash `0x0b68b61d57750ff1`** matches the new scene with the mountain marker — reported but not independently verified.
- **`pending_events` not being recorded by `Recording`** — the implementer's claim is consistent with the code (the `for ev in std::mem::take(&mut self.pending_events)` block at sim.rs:251-253 does not touch `self.recorder`), but I cannot verify the broader determinism story (whether host API calls like `thaw_region` actually replay deterministically given the same host program) without running a recording/replay round-trip that involves `thaw_region`.

## 5. Test evidence spot-check

The implementer's report (`.superpowers/sdd/briefs/task-1-report.md` §3) includes real test output with specific, verifiable artifacts:

| Command | Reported result | Looks real? |
|---|---|---|
| `cargo test -p vitric-cli --test region --` | PASS 4/4 (named tests match the 4 in `region.rs`) | ✅ Test names match the file |
| `cargo test --workspace` | PASS with 2 pre-existing typescript failures (esbuild missing) | ✅ Failure message `"测试需要 esbuild：仓库里跑 cd mcp && npm install，或设 ESBUILD_BIN"` is plausible; verified pre-existing via `git stash` test |
| `cargo run --release -- check games/frontier` | `entities: 426`, `initial_hash: 0x0b68b61d57750ff1` | ✅ 426 = 425 (pre-task) + 1 (mountain marker); hash matches re-recorded scene |
| `cargo run --release -- gate games/frontier` | 37249 ticks, `final_hash: 0xab58ec29d99275df`, `settlement-founded` verified | ✅ Specific tick count and hash, not a generic "PASS" |

The implementer also re-ran with `git stash` to verify the typescript failures are pre-existing — good practice. The hashes and tick counts are specific enough to look like real run output, not fabricated.
