# Task 3: E3 — Seeded RNG substreams

**Files:**
- Modify: `crates/vitric-sim/src/pcg.rs`
- Modify: `crates/vitric-script/src/lib.rs`
- Modify: `crates/vitric-sim/src/sim.rs` (snapshot/restore/hash integration)
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `ctx.random_stream(name)` returns `{ next(): number [0,1), nextInt(min,max): int }`. Substream state persisted in snapshot and hashed.

## Step 1: Write failing test for substream determinism

```rust
#[test]
fn random_stream_same_seed_regardless_of_call_timing() {
    let mut sim1 = TestSim::with_scene("games/frontier/scenes/main.json");
    let mut sim2 = TestSim::with_scene("games/frontier/scenes/main.json");

    let r1: Vec<i32> = (0..5).map(|_| {
        sim1.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)")
    }).collect();

    sim2.step(1000);
    let r2: Vec<i32> = (0..5).map(|_| {
        sim2.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)")
    }).collect();

    assert_eq!(r1, r2);
}

#[test]
fn random_stream_state_in_snapshot() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    sim.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");
    sim.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");

    let snapshot = sim.snapshot();
    let mut restored = TestSim::restore(&snapshot);

    let r1 = sim.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");
    let r2 = restored.call_js("ctx.random_stream('region:mountain').nextInt(0, 100)");
    assert_eq!(r1, r2);
}
```

## Step 2: Run tests to verify they fail

Run: `cargo test -p vitric-cli --test region random_stream`
Expected: FAIL.

## Step 3: Implement Substream in pcg.rs

```rust
#[derive(Clone, Debug)]
pub struct Substream {
    state: u64,
    increment: u64,
}

impl Substream {
    pub fn new(world_seed: u64, name: &str) -> Self {
        let mut hash = world_seed;
        for byte in name.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let increment = hash | 1;
        let state = Self::init_state(0, increment);
        Self { state, increment }
    }

    fn init_state(seed: u64, increment: u64) -> u64 {
        let mut s = Pcg32::new(seed, increment);
        s.next_u32();
        s.state
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6364136223846793005).wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        (xorshifted >> rot) | (xorshifted << ((!rot).wrapping_add(1) & 31))
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64) / (u32::MAX as f64 + 1.0)
    }
}
```

## Step 4: Add substream registry to Sim + integrate into snapshot/hash

In `crates/vitric-sim/src/sim.rs`, add `substreams: HashMap<String, Substream>` to `Sim`. Expose `random_stream(name) -> &mut Substream`. Include substream state in `snapshot()`, `restore()`, and `state_hash()` (sort keys for deterministic hash).

## Step 5: Expose ctx.random_stream to JS

In `crates/vitric-script/src/lib.rs`, add `ctx.random_stream(name)` returning `{ next(), nextInt(min, max) }` that bridges to `sim.substreams`.

## Step 6: Run substream tests

Run: `cargo test -p vitric-cli --test region random_stream`
Expected: PASS.

## Step 7: Run full test suite + frontier gate

Run: `cargo test --workspace && cargo run --release -- gate games/frontier`
Expected: all pass.

## Step 8: Commit

```bash
git add crates/vitric-sim/src/pcg.rs crates/vitric-sim/src/sim.rs crates/vitric-script/src/lib.rs crates/vitric-cli/tests/region.rs
git commit -m "feat(sim,script): E3 seeded RNG substreams

ctx.random_stream(name) returns a deterministic substream seeded by
(world_seed, name). Independent of call timing — replay-safe even if
region thaw happens at different ticks. State persisted in snapshot
and included in state_hash."
git push origin main
```
