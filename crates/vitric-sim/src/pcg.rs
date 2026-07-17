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
}
