# Task 5 Review — Snapshot/replay/describe plumbing (E5)

## Verdict: APPROVED

Spec-faithful, deterministic, read-only implementation. All 8 brief steps are satisfied (the commit referenced as not-performed in the implementer's report was in fact made — see Notes). All 14 region tests pass locally (re-verified by reviewer), including the 4 new E5 tests. No Critical or Important findings; two minor observations only.

## Spec compliance

- Step 1 (failing test): ✅ — `describe_classifies_dormant_entities` added at `crates/vitric-cli/tests/region.rs:13`, matches brief verbatim.
- Step 2 (run-fail): ✅ — Implementer's report records the expected pre-impl failure (`dormant` field missing, panicked on `.unwrap()` of `None`).
- Step 3 (implement dormant array): ✅ — Dormant pass added at `crates/vitric-render/src/lib.rs:2850-2912`, matches brief spec. Uses `world.entities()` + `world.is_dormant(id)` + `Position`+`Sprite` filter (YAGNI-respecting). Entry contains `id`, `name?`, `world{x,y}`, `sprite{w,h,color,image?,rot?}`, `region{id,state}`. `dormant` key inserted between `offscreen` and `texts` at `lib.rs:3232`. Minor improvement over brief pseudocode: reuses existing `sort_rows` closure instead of a new named+id-only closure — see Notes.
- Step 4 (run-pass): ✅ — `describe_classifies_dormant_entities` passes (confirmed by reviewer re-run).
- Step 5 (snapshot+replay tests): ✅ — Three tests added (`snapshot_preserves_dormant_state`, `replay_with_dormant_region_is_hash_identical`, `replay_after_pre_recording_thaw_is_hash_identical`). Approved `mut rt` → `rt` deviation in `snapshot_preserves_dormant_state` is the only change from brief code.
- Step 6 (region suite): ✅ — `cargo test -p vitric-cli --test region` → 14 passed, 0 failed (reviewer re-verified; ~70s due to pre-existing `catch_up_advances_dormant_crop_on_thaw`).
- Step 7 (full suite + gate): ✅ — Implementer's report records workspace suite green (skipping the 2 pre-existing typescript failures) and `gate games/frontier` → `final_hash: 0xab58ec29d99275df`, `pass: true`. Hash matches the brief's required value.
- Step 8 (commit): ✅ — Local commit `98fd213` exists on `main` with the brief's exact commit message. ⚠️ Implementer's report says "commit + push not performed" (cancelled by user twice) — this appears to be stale: the commit was made between report-writing and review. Push to `origin/main` not verifiable from local git log alone (see Cannot-verify items).

## Findings

### Critical
(none)

### Important
(none)

### Minor

1. **`crates/vitric-render/src/lib.rs:2917-2929` — dormant sort reuses existing `sort_rows` closure instead of brief's separate closure.** The brief's pseudocode proposes a new closure `|a,b| b.named.cmp(&a.named).then(a.id.cmp(&b.id))` for dormant. The implementation instead adds `sort_rows(&mut dormant);` to the existing block, which uses `b.named.cmp(&a.named).then(a.dist.total_cmp(&b.dist)).then(a.id.cmp(&b.id))`. Since all dormant rows have `dist: 0.0`, behavior is identical, and reusing the closure is DRY-er. This is an improvement, not a regression — flagging only because it differs from the brief's pseudocode literal text.

2. **`crates/vitric-cli/tests/region.rs:60-86` — `snapshot_preserves_dormant_state` test message says "Mountain region starts dormant in scenes/main.json. Freeze it to verify a non-default state round-trips (dormant→frozen is a state the host can set via set_component)."** The test only verifies the `frozen` + `discovered=1` round-trip; it does not also verify the default `dormant`/`discovered=0` round-trips. Not a defect — the frozen case is a stronger contract (non-default value) — just noting the comment mentions dormant→frozen transition which isn't actually tested as a transition (the test sets state directly, doesn't call a transition API). Purely cosmetic.

### ⚠️ Cannot verify from diff

1. **Push to `origin/main`.** Local `git log` confirms commit `98fd213` exists with the brief's message, but the implementer's report explicitly says "commit + push not performed". Either the report is stale (commit + push happened later) or only the local commit happened. Controller should run `git log origin/main -1` (or `git status -sb`) to confirm `98fd213` is on the remote.
2. **Frontier gate hash `0xab58ec29d99275df` and full `cargo test --workspace -- --skip typescript`.** Reviewer re-ran the region suite only (14/14 pass). The workspace-wide suite and the gate run live outside the diff; reviewer trusts the implementer's report for these. Controller may re-run `~/.cargo/bin/cargo run --release -- gate games/frontier` if a false claim is suspected.
3. **Read-only nature of `World` methods called in the new pass** (`entities`, `is_dormant`, `has_component`, `get_field`, `name_of`, `get_component`) and helpers (`num`, `rot_of`). Reviewer verified by Grep that all take `&self` / `&World` (see `crates/vitric-ecs/src/world.rs:103,117,135,147,180,248` and `crates/vitric-render/src/lib.rs:1985,3386`), and `describe_world_with_assets` itself takes `world: &World` (`lib.rs:2725-2726`), so Rust's borrow checker forbids any mutation. Flagging only because the diff alone doesn't show these signatures.

## Notes

- **Spec faithfulness**: The implementation is essentially verbatim from the brief. The only behavioral deviation from the brief's pseudocode is the sort-closure reuse (Minor #1), which is an improvement. The approved `mut rt` → `rt` deviation in `snapshot_preserves_dormant_state` is correctly applied (the report explains it silences an `unused_mut` warning since `sim.snapshot(&rt)` takes `&Runtime`).
- **Determinism contract preserved**: The dormant pass calls only `&self`/`&World` methods. No `&mut` escapes, no RNG calls, no world mutation. `describe_world_with_assets` remains read-only. ✅
- **No duplication of visible/offscreen**: The visible/offscreen loop uses `world.query(&["Position", "Sprite"])` which filters dormant entities (per Task 1). The dormant loop uses `world.entities()` + `is_dormant(id)` filter. An entity is either dormant (→ dormant array) or not (→ potentially visible/offscreen). No overlap possible. ✅
- **YAGNI respected**: Dormant array only includes entities with `Position`+`Sprite` (same minimal components as visible/offscreen), NOT all entities in dormant regions. ✅
- **Test quality**: All 4 new tests assert meaningful contracts (not just "doesn't crash"): (1) `describe_classifies_dormant_entities` asserts visible/offscreen/dormant partitioning + region info + world coords — would fail if dormant entity landed in visible/offscreen or if region info was missing; (2) `snapshot_preserves_dormant_state` asserts `state` and `discovered` round-trip non-default values — would fail if `World::snapshot`/`restore` dropped Region fields; (3) `replay_with_dormant_region_is_hash_identical` asserts final hash equality — would fail if dormant state perturbed replay; (4) `replay_after_pre_recording_thaw_is_hash_identical` asserts the host-API-re-run contract — would fail if pre-recording thaw broke replay determinism. ✅
- **Comment language**: All new comments in `crates/vitric-render/src/lib.rs` (lines 2850-2854, 2858, 2878, 2908) and `crates/vitric-cli/tests/region.rs` are English. ✅ (The existing Chinese error string in `num` at `lib.rs:3388` is pre-existing and unchanged — correctly preserved per the global constraint.)
- **No regression to existing visible/offscreen classification**: The new dormant loop is inserted between the existing visible/offscreen loop (ends `lib.rs:2848`) and the existing sort block (`lib.rs:2917-2929`). The existing loop body is byte-identical (diff shows only additions, no modifications to existing lines). ✅
- **Report staleness**: The implementer's report claims "DONE_WITH_CONCERNS" solely because the commit was supposedly not performed. In reality the commit `98fd213` exists locally on `main` with the brief's exact message. The reviewer treats Step 8 as ✅; the controller should verify the push to `origin/main` (see Cannot-verify #1).
