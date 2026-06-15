//! 策略库——消费 Scene View、产出动作的纯逻辑。每个策略都用**独立的 Pcg32**
//! 播种（从 playtest seed 来，不碰 sim.rng），所以同 seed 同序列、完全可复现。

use vitric_sim::Pcg32;

use crate::scene_view::{Action, SceneView};

/// 策略接口：看一份视图，选一个动作（或本 tick 不操作）。
pub trait Strategy {
    /// None = 本 tick 什么都不按。返回的动作必须来自 `view.actions`（合法集合）。
    fn choose(&mut self, view: &SceneView) -> Option<Action>;
}

/// 随机策略：合法 actions 里均匀随机挑一个（含一定概率「不操作」）。
/// 覆盖广、专找意外软锁（设计稿二节）。
pub struct RandomStrategy {
    rng: Pcg32,
}

impl RandomStrategy {
    pub fn new(seed: u64) -> RandomStrategy {
        RandomStrategy { rng: Pcg32::new(seed) }
    }
}

impl Strategy for RandomStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        // [0, n]：n 个动作 + 1 个「不操作」槽。不操作也是合法选择
        // （一直猛按和偶尔松手是不同的探索路径）。
        let n = view.actions.len() as i64;
        let pick = self.rng.range_i64(0, n);
        if pick == n {
            None
        } else {
            Some(view.actions[pick as usize].clone())
        }
    }
}

/// 贪心策略：朝目标的派生量贪心（设计稿二节）。
///
/// 第 1 阶段还没有「通用目标量」（距出口距离/敌人血量这些要 playtest.json 声明派生量，
/// 是阶段 4 的活）。所以现在退化成「带 PCG 的随机」——结构留好，等阶段 4 接入派生目标量
/// 后，这里改成读 view 里的目标量做贪心即可，接口不变。刻意不在此过度设计启发式：
/// 没有目标量时任何「启发」都是瞎猜，不如老实退化成可复现的随机。
pub struct GreedyStrategy {
    inner: RandomStrategy,
}

impl GreedyStrategy {
    pub fn new(seed: u64) -> GreedyStrategy {
        GreedyStrategy { inner: RandomStrategy::new(seed) }
    }
}

impl Strategy for GreedyStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        // TODO(阶段4)：view 带派生目标量后，这里改成朝目标贪心；现在退化为随机。
        self.inner.choose(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_view::Action;

    fn view_with_actions(names: &[&str]) -> SceneView {
        let actions = names
            .iter()
            .map(|n| Action { action: n.to_string(), phase: "pressed".to_string() })
            .collect();
        SceneView { observation: serde_json::json!({}), actions, done: None }
    }

    #[test]
    fn random_only_picks_legal_actions() {
        let view = view_with_actions(&["left", "right", "space"]);
        let mut s = RandomStrategy::new(7);
        for _ in 0..500 {
            if let Some(a) = s.choose(&view) {
                assert!(view.actions.contains(&a), "选出的动作必须在合法集合里: {a:?}");
            }
        }
    }

    #[test]
    fn random_is_deterministic_same_seed_same_sequence() {
        let view = view_with_actions(&["left", "right", "space"]);
        let mut a = RandomStrategy::new(42);
        let mut b = RandomStrategy::new(42);
        let seq_a: Vec<_> = (0..1000).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..1000).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b, "同 seed 必须出同一序列");
    }

    #[test]
    fn random_differs_across_seeds() {
        let view = view_with_actions(&["left", "right", "space"]);
        let mut a = RandomStrategy::new(1);
        let mut b = RandomStrategy::new(2);
        let seq_a: Vec<_> = (0..200).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..200).map(|_| b.choose(&view)).collect();
        assert_ne!(seq_a, seq_b, "不同 seed 应给出不同序列");
    }

    #[test]
    fn empty_actions_yields_no_op() {
        let view = view_with_actions(&[]);
        let mut s = RandomStrategy::new(3);
        assert_eq!(s.choose(&view), None);
    }

    #[test]
    fn greedy_is_deterministic() {
        let view = view_with_actions(&["a", "b"]);
        let mut a = GreedyStrategy::new(99);
        let mut b = GreedyStrategy::new(99);
        let seq_a: Vec<_> = (0..300).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..300).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b);
    }
}
