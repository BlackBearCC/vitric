use serde::{Deserialize, Serialize};

/// PCG32 (PCG-XSH-RR). Implemented in-house instead of using the rand crate:
/// the algorithm never changes, the state is just two u64s, and it serializes into snapshots — the foundation of determinism is not outsourced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const MULT: u64 = 6364136223846793005;

impl Pcg32 {
    pub fn new(seed: u64) -> Pcg32 {
        // Reference implementation's seeding flow; the sequence number is fixed at 54 (any odd number works; hard-coded for determinism)
        let mut rng = Pcg32 { state: 0, inc: (54 << 1) | 1 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// [0, 1) floating-point.
    pub fn next_f64(&mut self) -> f64 {
        // 53-bit precision: high 32 bits + high 21 bits combined
        let hi = self.next_u32() as u64;
        let lo = self.next_u32() as u64;
        (((hi << 21) | (lo >> 11)) as f64) / (1u64 << 53) as f64
    }

    /// [min, max] closed-interval integer.
    pub fn range_i64(&mut self, min: i64, max: i64) -> i64 {
        assert!(min <= max, "range_i64 要求 min <= max");
        let span = (max - min + 1) as u64;
        min + (self.next_u32() as u64 % span) as i64
    }
}

/// A named deterministic RNG substream, independent of the main `Pcg32` stream.
///
/// Each substream is seeded by `(world_seed, name)`: the name is FNV-1a hashed (seeded with
/// `world_seed`) into a per-name `increment`, then the same PCG32 seeding flow runs with that
/// increment. Two substreams with different names (or different world seeds) produce
/// independent sequences; the same `(world_seed, name)` always produces the same sequence
/// regardless of when the substream is first accessed.
///
/// This is the foundation of replay-safe PCG for dormant regions: a region thawing at tick
/// 100 vs tick 1000 generates the same content because the substream seed doesn't depend on
/// call timing — only on (world_seed, name).
///
/// Inlined PCG32 init (instead of delegating to `Pcg32::new`) because `Pcg32::new` hardcodes
/// `inc = (54 << 1) | 1`; substreams need their own per-name increment. The algorithm (MULT,
/// xorshift+rotate) is identical to `Pcg32` — only the seeding flow differs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Substream {
    state: u64,
    increment: u64,
}

impl Substream {
    /// Seed a new substream from `(world_seed, name)`. The name is hashed (FNV-1a, seeded
    /// with `world_seed`) into an odd `increment`; the PCG32 seeding flow then runs with that
    /// increment and `world_seed` as the seed. Same `(world_seed, name)` → same initial state.
    pub fn new(world_seed: u64, name: &str) -> Self {
        // FNV-1a hash the name, mixing in the world_seed so different worlds produce
        // different substream sequences even for the same name.
        let mut hash = world_seed;
        for byte in name.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        // PCG requires an odd increment (lowest bit = 1).
        let increment = hash | 1;
        // Inline PCG32 seeding flow (same as Pcg32::new, but with a custom increment):
        //   state = 0; next_u32(); state += seed; next_u32();
        let mut s = Substream { state: 0, increment };
        s.next_u32();
        s.state = s.state.wrapping_add(world_seed);
        s.next_u32();
        s
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULT).wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// [0, 1) floating-point (32-bit precision — half the precision of `Pcg32::next_f64`,
    /// sufficient for PCG content generation where nextInt is the primary consumer).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64) / (u32::MAX as f64 + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_across_instances() {
        let mut a = Pcg32::new(42);
        let mut b = Pcg32::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        let mut c = Pcg32::new(43);
        assert_ne!(Pcg32::new(42).next_u32(), c.next_u32());
    }

    #[test]
    fn snapshot_resumes_exactly() {
        let mut a = Pcg32::new(7);
        for _ in 0..123 {
            a.next_u32();
        }
        let snap = serde_json::to_value(&a).unwrap();
        let mut b: Pcg32 = serde_json::from_value(snap).unwrap();
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn f64_in_unit_interval() {
        let mut r = Pcg32::new(1);
        for _ in 0..1000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn range_respects_bounds() {
        let mut r = Pcg32::new(2);
        for _ in 0..1000 {
            let x = r.range_i64(-3, 3);
            assert!((-3..=3).contains(&x));
        }
    }

    // ---- Substream ----

    #[test]
    fn substream_same_seed_and_name_produces_same_sequence() {
        let mut a = Substream::new(42, "region:mountain");
        let mut b = Substream::new(42, "region:mountain");
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32(),
                "same (seed, name) must produce identical sequences");
        }
    }

    #[test]
    fn substream_different_name_produces_different_sequence() {
        let mut a = Substream::new(42, "region:mountain");
        let mut b = Substream::new(42, "region:forest");
        // Different names → different increments → almost certainly different first draw.
        // (Not a hard mathematical guarantee, but the hash space is 2^64; collision is astronomically unlikely.)
        assert_ne!(a.next_u32(), b.next_u32(),
            "different names must produce different sequences");
    }

    #[test]
    fn substream_different_seed_produces_different_sequence() {
        let mut a = Substream::new(42, "region:mountain");
        let mut b = Substream::new(99, "region:mountain");
        assert_ne!(a.next_u32(), b.next_u32(),
            "different world seeds must produce different sequences");
    }

    #[test]
    fn substream_snapshot_round_trips() {
        let mut a = Substream::new(7, "region:mountain");
        for _ in 0..123 {
            a.next_u32();
        }
        let snap = serde_json::to_value(&a).unwrap();
        let mut b: Substream = serde_json::from_value(snap).unwrap();
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32(),
                "restored substream must resume exactly");
        }
    }

    #[test]
    fn substream_next_f64_in_unit_interval() {
        let mut r = Substream::new(1, "test");
        for _ in 0..1000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x), "next_f64 out of [0, 1): {x}");
        }
    }
}
