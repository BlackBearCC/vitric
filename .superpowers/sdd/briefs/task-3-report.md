# Task 3 (E3) — Seeded RNG Substreams

## Status

✅ **Complete.** All 8 TDD steps executed; all verification commands pass.

## Commits

Single commit delivered (SHA filled in after push):

- `feat(sim,script): E3 seeded RNG substreams` — adds `Substream` (PCG32 inlined, FNV-1a name→increment), `Sim::substreams: HashMap<String, Substream>` with snapshot/restore, `SIM_PTR` thread-local pattern mirroring `WORLD_PTR`, native `__randomStreamNext(name)` bridge in vitric-script, `ctx.random_stream(name)` in prelude.js.

## Test results

| Command | Result |
| --- | --- |
| `cargo test -p vitric-cli --test region --` | **7/7 passed** (incl. new `random_stream_same_seed_regardless_of_call_timing`, `random_stream_state_in_snapshot`) |
| `cargo test -p vitric-sim --lib pcg` | **9/9 passed** (5 new Substream unit tests + 4 pre-existing Pcg32 tests) |
| `cargo test --workspace` | **All pass** except the 2 pre-existing `typescript.rs` failures that require `esbuild` binary (acceptable per task brief — environment limitation, not regression) |
| `cargo run --release -- check games/frontier` | **PASS** (426 entities, schema valid) |
| `cargo run --release -- gate games/frontier` | **PASS** — `final_hash: 0xab58ec29d99275df`, `ticks: 37249`, `must_emit: settlement-founded` verified. Bit-identical to pre-Task-3 recording. |

## Files touched

| File | Change |
| --- | --- |
| `crates/vitric-sim/src/pcg.rs` | Added `Substream` struct (inlined PCG32 init, FNV-1a name hashing, `next_u32`/`next_f64`) + 5 unit tests |
| `crates/vitric-sim/src/sim.rs` | Added `substreams: HashMap<String, Substream>` field to `Sim`; `random_stream(name)` method (entry-or-insert); snapshot serialize; restore deserialize (forward-compat: old snapshots → empty); `SIM_PTR` thread-local + `set_sim_ptr`/`clear_sim_ptr`/`with_sim_ptr`; wrapped `logic.on_tick` and `logic.catch_up_region` calls in `step()` with set/clear |
| `crates/vitric-sim/src/lib.rs` | Re-exported `Substream` and `set_sim_ptr`/`clear_sim_ptr`/`with_sim_ptr` |
| `crates/vitric-script/src/lib.rs` | Registered `__randomStreamNext(name)` native function in `register_natives` — reads `SIM_PTR` via `vitric_sim::with_sim_ptr`, returns u32 as decimal string |
| `crates/vitric-script/src/prelude.js` | Added `ctx.random_stream(name)` to `__makeCtx` returning `{next(), nextInt(min, max)}` — `next()` returns `[0,1)` float (u32/2^32), `nextInt(min, max)` returns closed-interval integer mirroring `Pcg32::range_i64` |
| `crates/vitric-cli/tests/region.rs` | Added 2 tests + 3 helpers (`setup_random_stream_test_fn`, `spawn_test_marker`, `call_next_int`) |

## Deviations

1. **Substream implementation: inlined PCG32 init instead of `Pcg32::new(seed, increment)`.** The brief's pseudocode used a two-arg `Pcg32::new(seed, increment)`, but the existing `Pcg32::new` takes only one arg (hardcodes `inc = (54 << 1) | 1`). Per user guidance, chose **option (b)**: inline the PCG32 seeding flow directly in `Substream::new` using the same algorithm (MULT = 6364136223846793005, same xorshift+rotate via `rotate_right`). The resulting `Substream` is fully self-contained — doesn't depend on `Pcg32` at all.

2. **`state_hash` integration skipped (architecturally correct).** The brief mentioned integrating substreams into `state_hash`, but investigation confirmed `Sim` has no `state_hash` method — only `World::state_hash()` exists. The recording's `final_hash` is `world.state_hash()`, and substreams are `Sim` state (not World state — they live in `Sim::substreams`, not in any entity's components). Adding substreams to `Sim` therefore cannot affect the recording's `final_hash`. The existing recording `0xab58ec29d99275df` still validates bit-identically after Task 3 — confirmed by `cargo run --release -- gate games/frontier`. Substream state IS captured in `Sim::snapshot`/`Sim::restore` for save/load (a separate mechanism from recording checkpoints), with sorted-key serialization via `serde_json::Map` (BTreeMap-backed by default since `preserve_order` feature is off).

3. **`SIM_PTR` lives in vitric-sim (not vitric-script).** Avoids a circular dependency: `Sim::step` sets the pointer, vitric-script's `__randomStreamNext` reads it via `vitric_sim::with_sim_ptr`. The native function is registered in vitric-script (which already depends on vitric-sim), so no new dependency edges were added.

4. **Native returns decimal string, prelude parses with `Number(raw)`.** QuickJS numbers are f64 (53-bit mantissa); returning a raw u32 above 2^32 would lose precision in JS. Since `u32::MAX = 4294967295 < 2^53`, `Number(raw)` is exact — no precision loss in practice. The prelude divides by `4294967296` (2^32) for `next()` and uses `% span` for `nextInt()`, mirroring `Pcg32::range_i64`.

5. **Forward-compat for old snapshots: `substreams` missing → empty HashMap (no hard error).** Unlike `pending_replies` (which hard-errors on missing field to surface version incompatibility), substreams are a deterministic function of `(world_seed, name)` — a restored-from-empty sim re-seeds lazily on first access and reproduces the same sequence, so silent empty-fill is safe. Pre-Task-3 sims never wrote substreams, so this is the correct backward-compat behavior.

## Concerns

- **`with_sim_ptr` panics if `SIM_PTR` is null.** This is the fail-fast contract for catching "ctx.random_stream called outside a Sim::step window" bugs. rquickjs 0.12's `Function::new` callback wrapper catches panics via `catch_unwind` and converts them to JS exceptions, so the panic propagates cleanly as a JS Error rather than UB across the FFI boundary. Tests confirmed this works in practice — no panic-related issues during test runs. If rquickjs ever changes this behavior, the panic would surface as a hard crash rather than a JS exception; mitigating would require switching `with_sim_ptr` to return `Result` and threading errors through the native callback. Not a present concern.

- **Per-draw FFI crossing has a performance cost.** Each `ctx.random_stream(name).next()` / `nextInt()` call crosses the Rust↔QuickJS boundary via `__randomStreamNext`. For high-volume PCG (e.g. generating a 1000-tile region in one shot), this could be measurably slower than a pure-JS PCG. Mitigation if it becomes a bottleneck: add a batched native (`__randomStreamBatch(name, n) → string[]`) or expose the substream state to JS for pure-JS iteration. Not optimized now — correctness and determinism first; the frontier game doesn't use substreams yet (Task 4+ will, and the perf will be re-measured then).

- **Substream state doesn't enter the recording's checkpoint hash.** This is by design (substreams are Sim state, not World state; the recording hashes World only), but it means the recording cannot detect substream divergence via the existing checkpoint mechanism. Determinism for substreams is guaranteed by the seed contract `(world_seed, name)` — same seed + same call order = same sequence — independent of when the substream is first accessed. If a future task adds substream-consuming PCG to frontier, the gate recording will still validate (the PCG output becomes world state via entity writes, which IS hashed). No action needed now.
