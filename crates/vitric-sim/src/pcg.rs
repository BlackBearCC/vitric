use serde::{Deserialize, Serialize};

/// PCG32（PCG-XSH-RR）。自实现而不用 rand crate：
/// 算法永远不变、状态就是两个 u64、可序列化进快照——确定性的地基不外包。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const MULT: u64 = 6364136223846793005;

impl Pcg32 {
    pub fn new(seed: u64) -> Pcg32 {
        // 参考实现的 seeding 流程，序列号固定取 54（任意奇数即可，写死保证确定性）
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

    /// [0, 1) 浮点。
    pub fn next_f64(&mut self) -> f64 {
        // 53 位精度：高 32 位 + 高 21 位拼
        let hi = self.next_u32() as u64;
        let lo = self.next_u32() as u64;
        (((hi << 21) | (lo >> 11)) as f64) / (1u64 << 53) as f64
    }

    /// [min, max] 闭区间整数。
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
