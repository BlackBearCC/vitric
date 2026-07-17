//! Easing curves — the math part of the tween system (`Tween` component, see `advance_tweens` in sim.rs).
//!
//! All are closed-form expressions of progress ∈ [0,1] (pure functions), **no cumulative integration**: the value at tick T
//! is always computed in one step as `from + (to - from) · ease(elapsed/duration)`, not by stacking deltas on the previous frame's
//! value — floating-point accumulation error would diverge the resume trajectory after a snapshot rollback, while closed-form expressions have no such problem.
//! Formulas follow the industry-standard cubic family (same as easings.net); the overshoot coefficient of ease-out-back is
//! c1 = 1.70158 (the classic ~10% overshoot value).

use std::fmt;

/// Easing curve. Fixed enum — the curve set is part of the engine contract; custom curves are not exposed
/// (for arbitrary curves, use rules/scripts to write the field tick by tick; that's the job of the Turing-complete channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ease {
    /// Uniform speed.
    Linear,
    /// Cubic acceleration (slow-in).
    In,
    /// Cubic deceleration (slow-out).
    Out,
    /// Accelerate then decelerate.
    InOut,
    /// Decelerate + end overshoot bounce (the value briefly exceeds the target then returns).
    OutBack,
}

/// All curve names (used in error messages; order matches the doc order).
pub const EASE_NAMES: &[&str] = &["linear", "ease-in", "ease-out", "ease-in-out", "ease-out-back"];

impl Ease {
    /// Parse by name. Unknown names produce an explicit error listing all available curves.
    pub fn parse(name: &str) -> Result<Ease, String> {
        match name {
            "linear" => Ok(Ease::Linear),
            "ease-in" => Ok(Ease::In),
            "ease-out" => Ok(Ease::Out),
            "ease-in-out" => Ok(Ease::InOut),
            "ease-out-back" => Ok(Ease::OutBack),
            other => Err(format!(
                "未知缓动曲线 {other:?}。可用曲线: [{}]",
                EASE_NAMES.join(", ")
            )),
        }
    }

    /// Curve body: progress ∈ [0,1] → progress coefficient (OutBack briefly exceeds 1).
    pub fn apply(self, p: f64) -> f64 {
        // Pin the start point to 0: OutBack's polynomial is mathematically 0 at p=0, but floating-point evaluation leaves
        // a 2.2e-16 tail — the starting tick must write exactly the starting value; endpoints shouldn't be left to floating-point luck
        if p == 0.0 {
            return 0.0;
        }
        match self {
            Ease::Linear => p,
            Ease::In => p * p * p,
            Ease::Out => 1.0 - (1.0 - p).powi(3),
            Ease::InOut => {
                if p < 0.5 {
                    4.0 * p * p * p
                } else {
                    1.0 - (-2.0 * p + 2.0).powi(3) / 2.0
                }
            }
            Ease::OutBack => {
                const C1: f64 = 1.70158;
                const C3: f64 = C1 + 1.0;
                1.0 + C3 * (p - 1.0).powi(3) + C1 * (p - 1.0) * (p - 1.0)
            }
        }
    }
}

impl fmt::Display for Ease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Ease::Linear => "linear",
            Ease::In => "ease-in",
            Ease::Out => "ease-out",
            Ease::InOut => "ease-in-out",
            Ease::OutBack => "ease-out-back",
        };
        write!(f, "{name}")
    }
}

/// The tween value at the elapsed-th tick (0..duration). **Only called mid-flight**: at the expiry tick
/// this formula is not used; the caller writes the final value exactly (no floating-point tail) — this convention is part of the tween contract.
pub fn tween_value(from: f64, to: f64, ease: Ease, elapsed: u64, duration: u64) -> f64 {
    let p = elapsed as f64 / duration as f64;
    from + (to - from) * ease.apply(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-value asserts for the five curves (exact values at key points — cubics are exact at binary fractional points).
    #[test]
    fn ease_values_are_exact_at_key_points() {
        // Endpoints: all curves 0 → 0, 1 → 1
        for &name in EASE_NAMES {
            let e = Ease::parse(name).unwrap();
            assert_eq!(e.apply(0.0), 0.0, "{name}(0)");
            assert_eq!(e.apply(1.0), 1.0, "{name}(1)");
        }
        // linear
        assert_eq!(Ease::Linear.apply(0.25), 0.25);
        assert_eq!(Ease::Linear.apply(0.5), 0.5);
        assert_eq!(Ease::Linear.apply(0.75), 0.75);
        // ease-in: p³
        assert_eq!(Ease::In.apply(0.25), 0.015625);
        assert_eq!(Ease::In.apply(0.5), 0.125);
        assert_eq!(Ease::In.apply(0.75), 0.421875);
        // ease-out: 1 - (1-p)³
        assert_eq!(Ease::Out.apply(0.25), 1.0 - 0.421875);
        assert_eq!(Ease::Out.apply(0.5), 0.875);
        assert_eq!(Ease::Out.apply(0.75), 1.0 - 0.015625);
        // ease-in-out: first half 4p³, second half 1 - (2-2p)³/2
        assert_eq!(Ease::InOut.apply(0.25), 0.0625);
        assert_eq!(Ease::InOut.apply(0.5), 0.5);
        assert_eq!(Ease::InOut.apply(0.75), 0.9375);
        // ease-out-back: 1 + c3·(p-1)³ + c1·(p-1)², c1 = 1.70158, c3 = 2.70158
        let back = |p: f64| 1.0 + 2.70158 * (p - 1.0).powi(3) + 1.70158 * (p - 1.0) * (p - 1.0);
        assert_eq!(Ease::OutBack.apply(0.25), back(0.25));
        assert_eq!(Ease::OutBack.apply(0.5), back(0.5));
        assert_eq!(Ease::OutBack.apply(0.75), back(0.75));
        // Overshoot: the mid-late segment must exceed 1 (this is the whole point of out-back)
        assert!(Ease::OutBack.apply(0.7) > 1.0);
    }

    #[test]
    fn ease_in_out_halves_join_continuously() {
        // Formula continuity across the midpoint: left limit 4·(0.5)³ = 0.5 = the right formula's value at 0.5
        let left = Ease::InOut.apply(0.5 - 1e-12);
        let right = Ease::InOut.apply(0.5 + 1e-12);
        assert!((left - 0.5).abs() < 1e-9 && (right - 0.5).abs() < 1e-9);
    }

    #[test]
    fn parse_rejects_unknown_curve_listing_all() {
        let err = Ease::parse("bounce").unwrap_err();
        for name in EASE_NAMES {
            assert!(err.contains(name), "错误要列出 {name}: {err}");
        }
        for &name in EASE_NAMES {
            assert_eq!(Ease::parse(name).unwrap().to_string(), name);
        }
    }

    #[test]
    fn tween_value_is_pure_function_of_elapsed() {
        // Same params same elapsed → always same value (closed-form, no internal state)
        let a = tween_value(1.0, 5.0, Ease::InOut, 7, 40);
        let b = tween_value(1.0, 5.0, Ease::InOut, 7, 40);
        assert_eq!(a, b);
        // linear midpoint is exactly the arithmetic mean
        assert_eq!(tween_value(1.0, 5.0, Ease::Linear, 20, 40), 3.0);
    }
}
