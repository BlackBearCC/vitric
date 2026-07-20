# Task 3 (E3) Review — Seeded RNG Substreams

## 1. Verdict

**APPROVED.**

Spec-compliant, determinism sound, no Critical or Important findings. Three Minor notes
below; none block merge. Two "cannot verify from diff" items for the controller to keep
in mind for Task 12.

## 2. Spec compliance

| # | Item | Status | Evidence |
|---|---|---|---|
| 1 | `Substream` struct in `pcg.rs` with `new(world_seed, name)`, `next_u32`, `next_f64` | ✅ | `crates/vitric-sim/src/pcg.rs:63-103` — struct with `state`/`increment`, all three methods present |
| 2 | `Substream::new` uses FNV-like hash `(world_seed, name) → u64` to derive increment; state init follows PCG32 pattern | ✅ | `pcg.rs:72-89` — FNV-1a with `0x100000001b3` prime, `increment = hash \| 1`, inlined PCG32 seeding flow (`state=0; next_u32(); state+=seed; next_u32()`) matches `Pcg32::new` at `pcg.rs:14-21` |
| 3 | `Sim::substreams: HashMap<String, Substream>` field exists | ✅ | `sim.rs:212` |
| 4 | `Sim::random_stream(name) -> &mut Substream` inserts-if-absent using `Substream::new(self.seed, name)` | ✅ | `sim.rs:240-245` — `entry(name).or_insert_with(\|\| Substream::new(seed, name))` with `seed` captured before the closure |
| 5 | `Sim::snapshot` includes `"substreams"` key | ✅ | `sim.rs:531` — `serde_json::to_value(&self.substreams)` |
| 6 | `Sim::restore` deserializes substreams | ✅ | `sim.rs:592-595` — `serde_json::from_value(...).ok().unwrap_or_default()` (forward-compat: missing → empty) |
| 7 | Native `__randomStreamNext(name)` registered; uses `SIM_PTR` to access Sim | ✅ | `lib.rs:169-176` — `Function::new` callback calls `vitric_sim::with_sim_ptr(\|sim\| ...)` |
| 8 | `ctx.random_stream(name)` in prelude returns `{ next(), nextInt(min, max) }` | ✅ | `prelude.js:124-152` |
| 9 | `next()` returns `[0, 1)` float; `nextInt(min, max)` returns `[min, max]` inclusive int | ✅ | `prelude.js:131-150` — `Number(raw) / 2^32` for next; `min + (u % span)` for nextInt (mirrors `Pcg32::range_i64` at `pcg.rs:40-44`) |
| 10 | 2 new tests: `random_stream_same_seed_regardless_of_call_timing` and `random_stream_state_in_snapshot` | ✅ | `region.rs:104-157` — both tests present with the exact spec names |
| 11 | Commit message matches brief | ✅ | `git log -1 --format='%B' c286ca5` — subject + body match the brief verbatim (including the slightly-stale "included in state_hash" phrase, which is an approved deviation) |

## 3. Code quality findings

### Critical
None.

### Important
None.

### Minor

**M1. Per-call FFI cost for `__randomStreamNext` (implementer concern #2, acknowledged).**
Each `next()` / `nextInt()` call crosses the Rust↔QuickJS boundary. Acceptable for
correctness; flag for Task 12 re-measurement when region content generation may call this
hundreds of times per region. Mitigation path documented in the report (batched native
`__randomStreamBatch(name, n)` or pure-JS PCG iteration). Not a blocker.

**M2. `call_next_int` test helper doesn't use RAII for `clear_sim_ptr`** (`region.rs:83-102`).
If `rt.scripts.call_fn(...)` returns `Err` and the `expect` panics, `clear_sim_ptr()` is
skipped, leaving a dangling `SIM_PTR`. In practice this is harmless because each test runs
on its own thread (thread-local state doesn't leak) and a failed test aborts the test case
anyway. The production paths in `sim.rs:407-416` and `sim.rs:427-431` are correct — they
call `clear_sim_ptr()` before the `?` on the result, so an `Err` from logic still clears
the pointer. A `Drop` guard would be cleaner but is not required.

**M3. Safety comment in `with_sim_ptr` is slightly imprecise** (`sim.rs:57-61`).
The comment says "the `&mut Sim` here does not alias any live `&mut Sim` borrow", but
`Sim::step` does hold `&mut self` (the method receiver) live across the `set_sim_ptr` →
`logic.on_tick` → `clear_sim_ptr` window. The operational reasoning is sound — `step`
reborrowed `&mut self.world` and `&mut self.rng` to logic, and the closure only touches
`sim.substreams`, so there's no field-level aliasing — but the formal claim about no live
`&mut Sim` borrow is not quite right. Mirrors the existing `WORLD_PTR` pattern's safety
reasoning. Not a soundness bug in practice; the comment could be tightened.

### Verified non-issues (implementer concerns)

**C1. `with_sim_ptr` panic on null (implementer concern #1) — RESOLVED, not a finding.**
Verified by reading `rquickjs-core-0.12.0`:
- `src/class/ffi.rs:261-279` (`call_impl`) wraps the user callback in
  `ctx.handle_panic(AssertUnwindSafe(|| { C::call(...) }))`.
- `src/result.rs:694-699` (`handle_panic`) calls `crate::util::catch_unwind(f)` and on
  `Err(e)` stores the panic via `opaque.set_panic(e)` then throws a JS exception via
  `JS_Throw(... JS_MKVAL(JS_TAG_EXCEPTION, 0) ...)`.
- `src/util.rs:154-157` confirms `catch_unwind` delegates to `std::panic::catch_unwind`.

So a panic in `with_sim_ptr` (null `SIM_PTR`) is caught, converted to a JS exception,
propagates to `logic.on_tick` as an `Err`, and becomes a `SimError::Logic`. No process
crash, no UB across the FFI boundary. The fail-fast contract is sound.

**C2. HashMap iteration order vs. snapshot byte-stability — RESOLVED, not a finding.**
`Cargo.toml` (workspace root, line 23) declares `serde_json = "1"` with no features.
`preserve_order` is OFF by default, so `serde_json::Map` is `BTreeMap`-backed and object
keys are sorted on serialize. `serde_json::to_value(&self.substreams)` produces a
`Value::Object(Map)` with sorted keys regardless of HashMap iteration order. The snapshot
is byte-stable across runs. (The report's comment at `sim.rs:525-527` correctly documents
this, though the phrasing "serde_json sorts object keys on serialize" is slightly loose —
the sorting happens at `Map` construction time inside `to_value`, not at string
serialization time. The end result is the same.)

**C3. Substream state not in recording checkpoint hash — RESOLVED, not a finding.**
`Sim` has no `state_hash` method; only `World::state_hash()` exists. The recording's
`final_hash` is `world.state_hash()`. Substreams live in `Sim::substreams`, not in any
entity's components, so they cannot affect the world hash. This is the approved
architectural deviation #6. Determinism is guaranteed by the seed contract
`(world_seed, name)` + call-order replay, not by checkpoint hashing.

### Determinism deep-check — SOUND

Traced the full replay scenario:
1. Recording at tick T calls `ctx.random_stream("region:mountain").nextInt(0, 100)` → V.
2. Replay re-executes the same system at tick T.
3. Substream is seeded by `(world_seed, name)` on first access — `world_seed` is restored
   from snapshot, so it's the same across recording and replay.
4. Substream state advances once per `nextInt` call. Replay re-executes systems in the
   same order at the same ticks → substream advances in the same order → same V. ✓

Mid-recording-start scenario: if `start_recording()` happens after some substream calls,
the recording's start checkpoint only verifies `world.state_hash()` (substream state not
included). Replay must reproduce the host program up to the recording's start tick to
re-establish the same substream state. This is the same determinism contract as for the
main `rng` stream (whose state is also not in the world hash). Sound as long as the host
program is deterministic — which is the engine's existing contract.

## 4. Cannot verify from diff

1. **`thaw_region` not captured by recording — Task 12 concern.** `thaw_region` is a host
   API call that pushes to `pending_events` / `pending_catch_ups`, neither of which is
   recorded. Replay re-runs the same host program, so the same `thaw_region` calls happen
   at the same ticks. If Task 12's `catch_up` functions call `random_stream`, the substream
   advances during catch-up; replay re-runs the same catch-up at the same tick → same
   substream values. **However**: this only holds if the host program's `thaw_region`
   calls are themselves deterministic (triggered by recorded inputs or by deterministic
   rules). If a future task triggers `thaw_region` from non-recorded external state
   (e.g. an LLM reply that's not in the recording), catch-up PCG would diverge silently
   (substream state isn't in the checkpoint hash). The controller should verify Task 12's
   region-thaw triggers are all deterministic-given-recording.

2. **Substream divergence detection — Task 12 concern.** Because substream state is not
   in `world.state_hash()`, a substream divergence is only detected by the recording's
   checkpoint mechanism if the substream output affects world state (entity writes). If
   Task 12 uses substream output for anything that doesn't become world state (e.g. pure
   cosmetic decisions that don't get written), divergence would be invisible to the gate.
   The controller should verify Task 12's substream consumers always write results into
   entity components (which is the natural pattern for region content generation).

## 5. Global constraints

- **Comments are English**: ✅ — all new comments in `pcg.rs`, `sim.rs`, `lib.rs`,
  `prelude.js`, `region.rs` are English. (Chinese appears only in `expect`/panic messages
  and JS `throw new Error(...)` messages, which is the existing pattern throughout the
  codebase, e.g. `pcg.rs:41` `"range_i64 要求 min <= max"`, `prelude.js:154`
  `"ctx.emit: 事件名必须是非空字符串"`. The constraint is about comments, not messages.)
- **No `SystemTime`, no `thread_rng()`**: ✅ — grep across `vitric-sim` and `vitric-script`
  returns no matches for `SystemTime`, `thread_rng`, or `Instant::now`.
- **Backward compat**: ✅ — report confirms frontier gate passes with
  `final_hash: 0xab58ec29d99275df`, bit-identical to pre-Task-3 recording. Substream
  forward-compat (missing field → empty HashMap) is correct for pre-Task-3 snapshots.
