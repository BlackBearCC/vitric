# Task 5 Report — E5: Snapshot/replay/describe plumbing

## Status

**DONE_WITH_CONCERNS** — Implementation complete, all tests pass, frontier gate hash matches. The commit + push step was NOT performed: the `git commit` command was cancelled by the user twice (the changes remain staged in the working tree, ready for the user to commit). See Concerns.

## Commits

None pushed. The two intended files are staged (`git diff --cached --stat` shows `crates/vitric-render/src/lib.rs` +67 and `crates/vitric-cli/tests/region.rs` +123). The exact commit message from the brief is preserved and ready to use:

```
feat(render,sim): E5 describe dormant dim + snapshot/replay plumbing

describe_world output gains a top-level `dormant` array listing entities
whose Region.state is dormant/frozen. The existing visible/offscreen
arrays are unchanged (they already exclude dormant entities via world.query
filtering). Dormant entries include region info (id, state) so the agent
can reason about unexplored regions.

Snapshot/restore already preserves dormant state (Region is a World
component) — locked by an explicit test. Replay of a recording that
starts with a dormant region is hash-identical — locked by a test.
Pre-recording thaw_region (host API call re-run at replay) is also
hash-identical — locked by a test.

Note: thaw_region DURING recording is NOT replayable via Sim::replay (by
design — host API calls are not recorded; the host must re-run them at
the same ticks during replay). This is a design decision, not a bug;
adding a between-tick host hook to replay is out of scope for E5.
```

To finish the git step, the user can run:
```
git commit -F <path-to-message-file>   # or paste the message above
git push origin main
```

## Test results

| Command | Result |
|---|---|
| `~/.cargo/bin/cargo test -p vitric-cli --test region describe_classifies_dormant_entities` (pre-impl) | FAIL — `dormant` field missing (panicked on `.unwrap()` of `None`) — confirmed the test fails for the right reason |
| `~/.cargo/bin/cargo test -p vitric-cli --test region describe_classifies_dormant_entities` (post-impl) | PASS |
| `~/.cargo/bin/cargo test -p vitric-cli --test region` | PASS — 14 passed, 0 failed (incl. 4 new E5 tests: `describe_classifies_dormant_entities`, `snapshot_preserves_dormant_state`, `replay_with_dormant_region_is_hash_identical`, `replay_after_pre_recording_thaw_is_hash_identical`) |
| `~/.cargo/bin/cargo test --workspace -- --skip typescript` | PASS — all suites green, 0 failures (the 2 pre-existing typescript failures are skipped as instructed) |
| `~/.cargo/bin/cargo run --release -- gate games/frontier` | PASS — `final_hash: 0xab58ec29d99275df` (matches the expected hash exactly), `pass: true`, 426 entities, 37249 ticks, `settlement-founded` verified |

## Files touched

| File | Lines added | Lines removed |
|---|---|---|
| `crates/vitric-render/src/lib.rs` | +67 | 0 |
| `crates/vitric-cli/tests/region.rs` | +123 | 0 |
| **Total** | **+190** | **0** |

`crates/vitric-render/src/lib.rs` changes:
- Added a dormant-classification pass in `describe_world_with_assets` (after the existing visible/offscreen loop, before the sort). Iterates `world.entities()` (NOT `world.query`) because `query` filters dormant — to list dormant entities we must bypass the filter. Filters to entities with `Position`+`Sprite` (same minimal components as visible/offscreen) AND `world.is_dormant(id)`. Each entry includes `id`, `name` (if any), `world` {x,y}, `sprite` {w,h,color,image?,rot?}, and `region` {id, state} so the agent knows which dormant region the entity belongs to.
- Added `sort_rows(&mut dormant)` to the existing focal-point sort block (named-first, then dist, then id — same as visible/offscreen).
- Added `let dormant: Vec<serde_json::Value> = dormant.into_iter().map(|r| r.value).collect();`.
- Added `"dormant": dormant` to the final JSON object, placed immediately after `"offscreen"` (before `"texts"`) to maintain logical ordering (visible, offscreen, dormant).

`crates/vitric-cli/tests/region.rs` changes:
- Added 4 new tests under a new `// ---- Task 5 (E5): describe dormant dim + snapshot/replay plumbing ----` section header:
  1. `describe_classifies_dormant_entities` — isolated world; verifies a dormant Position+Sprite entity lands in `dormant` (not `visible`/`offscreen`) and carries region info.
  2. `snapshot_preserves_dormant_state` — boots frontier sim, flips mountain region to `frozen`/`discovered=1`, snapshot → fresh sim → restore, asserts state round-trips.
  3. `replay_with_dormant_region_is_hash_identical` — 60-tick recording with the default dormant mountain region; replay on a fresh sim reproduces `final_hash` exactly.
  4. `replay_after_pre_recording_thaw_is_hash_identical` — `thaw_region("mountain")` BEFORE `start_recording` (so thawed state is in the initial checkpoint), 60-tick recording, then on a fresh sim `thaw_region` again BEFORE `replay` (same host call, same tick 0) — replay reproduces `final_hash` exactly. This locks the contract documented in `sim.rs` lines 362–365: host API calls are not recorded; the host re-runs them at the same ticks during replay.

## Deviations from brief

- **`snapshot_preserves_dormant_state` uses `let (mut sim, rt)` instead of `let (mut sim, mut rt)`** — the brief's test code declared `mut rt`, but `sim.snapshot(&rt)` takes `&Runtime` (not `&mut`), so `rt` is never used mutably in this test. The Rust compiler emitted an `unused_mut` warning; removing `mut` from `rt` silences the warning with no behavior change. The other three tests follow the brief's code verbatim.
- No other deviations. The implementation code, test bodies, and commit message match the brief exactly.

## Concerns

1. **Commit + push not performed (DONE_WITH_CONCERNS reason).** The `git commit` command was cancelled by the user twice. Per the system note ("You MUST avoid using similar commands in next steps"), I did not retry. The two intended files are staged in the working tree (`git diff --cached --stat` confirms +190 lines across the two files). The commit message from the brief is preserved in this report. The user (or parent agent) needs to run `git commit` and `git push origin main` to complete step 8 of the brief.
2. **`catch_up_advances_dormant_crop_on_thaw` is slow (~60s).** This is a pre-existing test (not added by E5); it runs 3600 sim ticks. It passed, but it dominates the region test suite runtime. Not a regression — just noting it.
3. **The `dormant` array includes ALL dormant entities with Position+Sprite**, including those that would be off-screen if they were active. This matches the brief's spec (the agent needs to reason about unexplored regions regardless of camera position). If a future scene has many dormant entities, the `dormant` array could grow large — but this is the same scaling characteristic as `offscreen`, and the sort is deterministic.
4. **The `dormant` array does not include `relative_to_focal` / `direction` / `distance_units`** — dormant entities are not on-screen and have no focal-point relation (the brief's code sets `dist: 0.0` with a comment "No focal-point distance for dormant entities (they're not on-screen)"). This is intentional per the brief; the agent uses the `region` field to locate dormant entities instead.
5. **`thaw_region` during recording remains non-replayable by design.** The brief's Step 5 explicitly replaces the plan's `replay_with_region_thaw_is_hash_identical` test with two alternatives (`replay_with_dormant_region_is_hash_identical` and `replay_after_pre_recording_thaw_is_hash_identical`) because `Sim::replay` has no between-tick host hook. This is a documented design decision, not a bug — adding such a hook would be a new replay mode and is out of scope for E5.
