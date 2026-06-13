//! 缓动曲线——补间系统（`Tween` 组件，见 sim.rs 的 `advance_tweens`）的数学部分。
//!
//! 全部是 progress ∈ [0,1] 的解析式（纯函数），**禁累加积分**：第 T tick 的值
//! 永远由 `from + (to - from) · ease(elapsed/duration)` 一步算出，不在上一帧的
//! 值上叠增量——浮点累加的误差会让快照回退后的续播轨迹分歧，解析式天然没有这个问题。
//! 公式取业界通行的三次方系（easings.net 同款），ease-out-back 的回弹系数
//! c1 = 1.70158（约 10% 过冲的经典值）。

use std::fmt;

/// 缓动曲线。固定枚举——曲线集合是引擎约定的一部分，不开放自定义
/// （要任意曲线请用规则/脚本逐 tick 写字段，那是图灵完备通道的活）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ease {
    /// 匀速。
    Linear,
    /// 三次方加速（慢进）。
    In,
    /// 三次方减速（慢出）。
    Out,
    /// 先加速后减速。
    InOut,
    /// 减速 + 末端过冲回弹（值会短暂超过终点再回来）。
    OutBack,
}

/// 全部曲线名（错误提示用，顺序即文档顺序）。
pub const EASE_NAMES: &[&str] = &["linear", "ease-in", "ease-out", "ease-in-out", "ease-out-back"];

impl Ease {
    /// 按名字解析。未知名字显式报错并列出全部可用曲线。
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

    /// 曲线本体：progress ∈ [0,1] → 进度系数（OutBack 会短暂超过 1）。
    pub fn apply(self, p: f64) -> f64 {
        // 起点钉死为 0：OutBack 的多项式在 p=0 数学上等于 0，但浮点求值留下
        // 2.2e-16 的尾巴——起跑 tick 写的必须精确是起始值，端点不交给浮点碰运气
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

/// 第 elapsed tick（0..duration）的补间值。**只在中途调用**：到期那 tick
/// 不走这条公式，由调用方精确写终值（不留浮点尾巴）——这条约定写进了补间合同。
pub fn tween_value(from: f64, to: f64, ease: Ease, elapsed: u64, duration: u64) -> f64 {
    let p = elapsed as f64 / duration as f64;
    from + (to - from) * ease.apply(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 五条曲线逐值断言（关键点精确值——三次方在二进制分数点上是精确的）。
    #[test]
    fn ease_values_are_exact_at_key_points() {
        // 端点：所有曲线 0 → 0、1 → 1
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
        // ease-in-out: 前半 4p³，后半 1 - (2-2p)³/2
        assert_eq!(Ease::InOut.apply(0.25), 0.0625);
        assert_eq!(Ease::InOut.apply(0.5), 0.5);
        assert_eq!(Ease::InOut.apply(0.75), 0.9375);
        // ease-out-back: 1 + c3·(p-1)³ + c1·(p-1)²，c1 = 1.70158、c3 = 2.70158
        let back = |p: f64| 1.0 + 2.70158 * (p - 1.0).powi(3) + 1.70158 * (p - 1.0) * (p - 1.0);
        assert_eq!(Ease::OutBack.apply(0.25), back(0.25));
        assert_eq!(Ease::OutBack.apply(0.5), back(0.5));
        assert_eq!(Ease::OutBack.apply(0.75), back(0.75));
        // 过冲：中后段必须超过 1（这是 out-back 存在的意义）
        assert!(Ease::OutBack.apply(0.7) > 1.0);
    }

    #[test]
    fn ease_in_out_halves_join_continuously() {
        // 半点两侧公式衔接：左极限 4·(0.5)³ = 0.5 = 右公式在 0.5 的值
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
        // 同参数同 elapsed 永远同值（解析式，无内部状态）
        let a = tween_value(1.0, 5.0, Ease::InOut, 7, 40);
        let b = tween_value(1.0, 5.0, Ease::InOut, 7, 40);
        assert_eq!(a, b);
        // linear 中点恰是算术平均
        assert_eq!(tween_value(1.0, 5.0, Ease::Linear, 20, 40), 3.0);
    }
}
