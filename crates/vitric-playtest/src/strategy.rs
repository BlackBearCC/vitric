//! 策略库——消费 Scene View、产出动作的纯逻辑。每个策略都用**独立的 Pcg32**
//! 播种（从 playtest seed 来，不碰 sim.rng），所以同 seed 同序列、完全可复现。

use serde::{Deserialize, Serialize};
use vitric_sim::{InputRecord, Pcg32};

use crate::scene_view::{Action, SceneView};

/// 一条定性 note（LLM 档拟人玩时吐的主观提示，设计稿二节/五节「LLM 定性 note」）。
///
/// 只有 LLM 策略会产 note（清晰度/连续性/选择有效性这类「人话感受」），廉价策略档不产。
/// note 是**纯遥测**：不进录像、不进哈希、不影响确定性——它是「LLM 这局看着怎么样」的
/// 旁观记录，和 state_trace/fired_events 同级。报告里诚实标成「LLM 主观提示，待人复核」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaytestNote {
    /// 这条 note 是在第几个决策 tick 冒出来的（LLM 看哪一刻的视图说的）。
    pub tick: u64,
    /// note 类型（归一后的标签）：常见 "clarity"（看不懂该干嘛）/"continuity"（前后矛盾）/
    /// "choice"（选项没意义）/"other"。由 LLM 自报，解析时归一到这几类。
    pub kind: String,
    /// note 正文（LLM 的原话）。
    pub text: String,
}

/// 策略接口：看一份视图，选一个动作（或本 tick 不操作）。
pub trait Strategy {
    /// None = 本 tick 什么都不按。返回的动作必须来自 `view.actions`（合法集合）。
    fn choose(&mut self, view: &SceneView) -> Option<Action>;

    /// 取走本策略到目前为止累积的定性 note（取走即清空，避免重复收）。
    /// 默认空实现——只有 LLM 策略会覆盖它产 note，廉价策略档（random/greedy/…）一律不产。
    /// session 每 tick 调一次把 note 收进 SessionResult.notes（不进录像/哈希）。
    fn drain_notes(&mut self) -> Vec<PlaytestNote> {
        Vec::new()
    }
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

/// 覆盖策略：系统性轮着把动作词汇里**每个**动作至少注入一次（设计稿二节 coverage）。
/// 专找「从没被触发过的废动作」——随机策略可能整局都没碰到某个动作，coverage 保证碰到。
///
/// 怎么轮：维护一个轮转游标，每 tick 选 `actions[cursor]` 再前进；游标对当前动作数取模，
/// 所以动作集变化（关卡切换后词汇变了）也不越界。起点用 PCG 从 seed 打散——不同 seed
/// 从词汇的不同位置起轮，覆盖顺序不同但都遍历全集；同 seed 完全可复现（确定性铁律）。
/// 故意**不**插「不操作」：coverage 的职责是把动作全试一遍，松手探索交给 random。
pub struct CoverageStrategy {
    /// 轮转游标（持续递增，用时对动作数取模）。
    cursor: usize,
    /// PCG 播种出的起点偏移（确定可复现），与 cursor 相加再取模。
    start_offset: u64,
}

impl CoverageStrategy {
    pub fn new(seed: u64) -> CoverageStrategy {
        // 用一次性 PCG 抽一个起点偏移：同 seed 同偏移，不同 seed 从不同位置起轮
        let mut rng = Pcg32::new(seed);
        let start_offset = rng.next_u32() as u64;
        CoverageStrategy { cursor: 0, start_offset }
    }
}

impl Strategy for CoverageStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        let n = view.actions.len() as u64;
        // (起点偏移 + 游标) 对动作数取模 = 本 tick 要试的动作下标
        let idx = ((self.start_offset + self.cursor as u64) % n) as usize;
        self.cursor = self.cursor.wrapping_add(1);
        Some(view.actions[idx].clone())
    }
}

/// 经济压力策略（设计稿二节 `economy` / 六节模拟经营主力）——专为**找数值崩**：
/// 资源**无界增长（跑飞）**或**归零卡死（崩盘）**。
///
/// 怎么找：random/coverage 把动作打散，单个动作的累积效应被稀释、跑不到极端；而经济崩是
/// **某一个动作连按很多次**才暴露的（一直点「卖」把金币堆到溢出、一直点「买」把资源掏到 0）。
/// 所以本策略**锁定一个动作连续重复 R 次再轮到下一个**——把每个动作的 per-action 累积效应
/// 推到极端，让数值遥测的 max/min/monotonic 把跑飞/崩盘照出来。
///
/// R 与轮转用 PCG 播种（从 playtest seed 来，不碰 sim.rng）：同 seed 同序列，完全可复现
/// （确定性铁律）。R 在一个区间里随机取（避免每个动作都按死同样次数过于规整）；轮转游标
/// 顺序推进对动作数取模，所以动作集变化（关卡切换词汇变了）也不越界。起点用 PCG 打散，
/// 不同 seed 从不同动作起压。故意**不**插「不操作」：经济压力要的就是猛按到极端。
pub struct EconomyStrategy {
    /// PCG（播种 R 和起点）。
    rng: Pcg32,
    /// 当前锁定的动作下标（对动作数取模后用）。
    cursor: usize,
    /// 当前动作还要重复几次（剩余次数，归 0 就换下一个动作）。
    remaining: u64,
}

/// 单个动作连续重复次数 R 的取值区间（闭区间）。够大才能把累积效应推到极端
/// （翻倍类跑飞 ~30 次就到 1e9；掏空类崩盘几十次就归零），又不至于一个动作占满整局。
const ECONOMY_REPEAT_MIN: i64 = 24;
const ECONOMY_REPEAT_MAX: i64 = 64;

impl EconomyStrategy {
    pub fn new(seed: u64) -> EconomyStrategy {
        let mut rng = Pcg32::new(seed);
        // 起点：从词汇的哪个位置起压（不同 seed 不同），先抽出来存进 cursor 的基偏移。
        // 这里 cursor 先记成一个大偏移，choose 时再对当前动作数取模——派生时还不知道动作数。
        let cursor = rng.next_u32() as usize;
        EconomyStrategy { rng, cursor, remaining: 0 }
    }

    /// 抽一个新的重复次数 R（区间内 PCG 随机）。
    fn roll_repeat(&mut self) -> u64 {
        self.rng.range_i64(ECONOMY_REPEAT_MIN, ECONOMY_REPEAT_MAX) as u64
    }
}

impl Strategy for EconomyStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        let n = view.actions.len();
        // 当前锁定动作的重复次数耗尽：轮到下一个动作，重新抽 R
        if self.remaining == 0 {
            self.cursor = self.cursor.wrapping_add(1);
            self.remaining = self.roll_repeat();
        }
        self.remaining -= 1;
        // 锁定的就是 cursor 对当前动作数取模那个（每次连按同一个，直到 remaining 归 0）
        Some(view.actions[self.cursor % n].clone())
    }
}

/// 脚本策略：按一条固定的输入序列在**录制的 tick** 上注入动作，**不看 SceneView**
/// （设计稿三节「种子式探索」的回放部分）。种子录像本身就是一段「在第 N tick 按了什么」
/// 的脚本——把它原样喂回去就复现那一局；扰动过的脚本喂回去就是「在原解附近走一步岔路」。
///
/// 怎么追 tick：`Strategy::choose` 每 tick 被调一次（session 循环里），策略自己维护一个
/// 计数器 `cur_tick`，每调一次 +1。当 `cur_tick` 命中脚本里某条 `InputRecord.tick` 时吐它的
/// 动作。**同一 tick 多条输入**：种子录像允许一个 tick 注入多个动作（如 left+space 同帧），
/// 但 `choose` 每 tick 只能返一个动作——所以本策略对「同 tick 多条」只注入第一条（按脚本序）。
/// 这是已知局限：种子录像里同帧多输入会被截到一条。绝大多数解谜/剧情脚本是「一帧一动作」的
/// 序列，不受影响；真要逐字节复现同帧多输入得走 `sim.replay`（那是另一条路，非策略路）。
///
/// `then_explore`：脚本放完后改用该策略继续（截断 + 发散）——种子探索的 truncate 算子靠它，
/// 「照脚本走到第 K 步，之后交给 random 乱走」。None = 脚本放完就什么都不按（纯复现/纯前缀）。
pub struct ScriptedStrategy {
    /// 脚本：(tick, 动作)，按 tick 升序。choose 时 cur_tick 命中某条就吐它。
    script: Vec<(u64, Action)>,
    /// 当前 tick（每 choose 一次 +1）——session 每 tick 调一次 choose，与 sim.tick 同步。
    cur_tick: u64,
    /// 脚本里已放到第几条（脚本按 tick 升序，游标单调前进，避免每 tick 全表扫）。
    cursor: usize,
    /// 脚本放完后接力的策略（截断+发散）。None=放完就静默。
    then_explore: Option<Box<dyn Strategy>>,
}

impl ScriptedStrategy {
    /// 从一串 (tick, Action) 造脚本策略。脚本会按 tick 升序稳定排序（容忍调用方乱序传入）。
    /// `then_explore`：脚本放完后接力的策略，None=放完静默。
    pub fn new(
        mut script: Vec<(u64, Action)>,
        then_explore: Option<Box<dyn Strategy>>,
    ) -> ScriptedStrategy {
        // 稳定排序：同 tick 的多条保持原相对序（取第一条时才确定）
        script.sort_by_key(|(t, _)| *t);
        ScriptedStrategy { script, cur_tick: 0, cursor: 0, then_explore }
    }

    /// 从一条录像的输入序列造脚本（种子录像→脚本）。phase 一并带上（pressed/released）。
    pub fn from_inputs(
        inputs: &[InputRecord],
        then_explore: Option<Box<dyn Strategy>>,
    ) -> ScriptedStrategy {
        let script = inputs
            .iter()
            .map(|r| (r.tick, Action { action: r.action.clone(), phase: r.phase.clone() }))
            .collect();
        ScriptedStrategy::new(script, then_explore)
    }
}

impl Strategy for ScriptedStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        let tick = self.cur_tick;
        self.cur_tick += 1;

        // 脚本游标越过当前 tick：脚本到此为止，交给接力策略（或静默）
        if self.cursor >= self.script.len() {
            return match &mut self.then_explore {
                // 接力策略仍要看 view（它是 random/coverage 这类反应式策略）
                Some(s) => s.choose(view),
                None => None,
            };
        }
        // 脚本按 tick 升序：游标这条的 tick 就是「下一个该放的 tick」
        let (script_tick, action) = &self.script[self.cursor];
        if *script_tick == tick {
            let out = action.clone();
            self.cursor += 1;
            // 跳过同 tick 的其余条目（每 tick 只注一个动作，见类型注释的局限）
            while self.cursor < self.script.len() && self.script[self.cursor].0 == tick {
                self.cursor += 1;
            }
            Some(out)
        } else {
            // 还没到这条的 tick（中间这些 tick 脚本没安排动作）——本 tick 不操作。
            // 注意：不接力——脚本还没放完，中间的空 tick 就该是「按兵不动」，
            // 不能让 then_explore 在脚本中途乱插（那会破坏脚本复现）。
            None
        }
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
    fn non_llm_strategies_drain_no_notes() {
        // 廉价策略档默认不产 note（drain_notes 默认空实现）——note 通道是 LLM 档专属
        let view = view_with_actions(&["a", "b"]);
        let mut r = RandomStrategy::new(1);
        let mut c = CoverageStrategy::new(1);
        let mut e = EconomyStrategy::new(1);
        for s in [&mut r as &mut dyn Strategy, &mut c, &mut e] {
            let _ = s.choose(&view);
            assert!(s.drain_notes().is_empty(), "非 LLM 策略不产 note");
        }
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

    #[test]
    fn coverage_visits_every_action_within_a_full_cycle() {
        let view = view_with_actions(&["a", "b", "c", "d"]);
        let mut s = CoverageStrategy::new(7);
        // 跑满一整轮（动作数那么多 tick）应把每个动作各试到一次
        let mut hit: std::collections::HashSet<String> = std::collections::HashSet::new();
        for _ in 0..view.actions.len() {
            let a = s.choose(&view).expect("非空词汇必出动作");
            hit.insert(a.action);
        }
        assert_eq!(hit.len(), 4, "一整轮必须覆盖全部 4 个动作: {hit:?}");
    }

    #[test]
    fn coverage_is_deterministic_same_seed() {
        let view = view_with_actions(&["a", "b", "c"]);
        let mut a = CoverageStrategy::new(11);
        let mut b = CoverageStrategy::new(11);
        let seq_a: Vec<_> = (0..50).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..50).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b, "同 seed coverage 必出同一序列");
    }

    #[test]
    fn coverage_start_offset_differs_across_seeds() {
        // 不同 seed 起点不同：至少有一对相邻 tick 的选择不一样（否则就是没打散）
        let view = view_with_actions(&["a", "b", "c", "d", "e"]);
        let mut a = CoverageStrategy::new(1);
        let mut b = CoverageStrategy::new(999);
        let seq_a: Vec<_> = (0..5).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..5).map(|_| b.choose(&view)).collect();
        assert_ne!(seq_a, seq_b, "不同 seed 应从不同起点起轮");
    }

    #[test]
    fn coverage_empty_actions_yields_no_op() {
        let view = view_with_actions(&[]);
        let mut s = CoverageStrategy::new(3);
        assert_eq!(s.choose(&view), None);
    }

    #[test]
    fn coverage_never_picks_illegal_action() {
        let view = view_with_actions(&["x", "y"]);
        let mut s = CoverageStrategy::new(5);
        for _ in 0..100 {
            let a = s.choose(&view).unwrap();
            assert!(view.actions.contains(&a), "{a:?}");
        }
    }

    #[test]
    fn economy_locks_one_action_for_a_run_then_rotates() {
        // 经济策略应**连按同一个动作很多次**再换，不是每 tick 换（这是它和 coverage 的区别）
        let view = view_with_actions(&["buy", "sell", "wait"]);
        let mut s = EconomyStrategy::new(3);
        let seq: Vec<String> = (0..200).map(|_| s.choose(&view).unwrap().action).collect();
        // 至少存在一段「同一动作连续 ≥ 20 次」（锁定重复的特征）
        let mut max_run = 1usize;
        let mut run = 1usize;
        for w in seq.windows(2) {
            if w[0] == w[1] {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 1;
            }
        }
        assert!(max_run >= 20, "应有一段连按 ≥20 次同一动作，实际最长连段 {max_run}");
        // 200 tick 内应轮到过不止一个动作（不是死锁在一个上）
        let distinct: std::collections::HashSet<&String> = seq.iter().collect();
        assert!(distinct.len() >= 2, "应轮转过多个动作: {distinct:?}");
    }

    #[test]
    fn economy_only_picks_legal_actions() {
        let view = view_with_actions(&["a", "b"]);
        let mut s = EconomyStrategy::new(1);
        for _ in 0..300 {
            let a = s.choose(&view).unwrap();
            assert!(view.actions.contains(&a), "经济策略只选合法动作: {a:?}");
        }
    }

    #[test]
    fn economy_is_deterministic_same_seed() {
        let view = view_with_actions(&["buy", "sell", "hold"]);
        let mut a = EconomyStrategy::new(77);
        let mut b = EconomyStrategy::new(77);
        let seq_a: Vec<_> = (0..500).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..500).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b, "同 seed economy 必出同一序列");
    }

    #[test]
    fn economy_differs_across_seeds() {
        let view = view_with_actions(&["buy", "sell", "hold", "tax", "spend"]);
        let mut a = EconomyStrategy::new(1);
        let mut b = EconomyStrategy::new(424242);
        let seq_a: Vec<_> = (0..50).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..50).map(|_| b.choose(&view)).collect();
        assert_ne!(seq_a, seq_b, "不同 seed 应从不同动作/不同 R 起压");
    }

    #[test]
    fn economy_empty_actions_yields_no_op() {
        let view = view_with_actions(&[]);
        let mut s = EconomyStrategy::new(2);
        assert_eq!(s.choose(&view), None);
    }

    fn act(name: &str) -> Action {
        Action { action: name.to_string(), phase: "pressed".to_string() }
    }

    #[test]
    fn scripted_injects_action_on_its_recorded_tick() {
        // 脚本：tick 2 按 a，tick 5 按 b。其余 tick 不操作。
        let script = vec![(2u64, act("a")), (5u64, act("b"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["a", "b", "c"]);
        let mut got: Vec<Option<String>> = Vec::new();
        for _ in 0..8 {
            got.push(s.choose(&view).map(|a| a.action));
        }
        // tick 0,1 无；tick2=a；tick3,4 无；tick5=b；tick6,7 无
        assert_eq!(
            got,
            vec![
                None,
                None,
                Some("a".to_string()),
                None,
                None,
                Some("b".to_string()),
                None,
                None,
            ]
        );
    }

    #[test]
    fn scripted_ignores_scene_view() {
        // 脚本吐的动作即便不在 view.actions 里也照吐（脚本不受合法集合约束——它是回放）
        let script = vec![(0u64, act("offscreen"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["only-this"]);
        assert_eq!(s.choose(&view).map(|a| a.action), Some("offscreen".to_string()));
    }

    #[test]
    fn scripted_then_explore_takes_over_after_script_ends() {
        // 脚本只到 tick 0；之后交给 random 接力（截断+发散）
        let script = vec![(0u64, act("scripted"))];
        let mut s = ScriptedStrategy::new(script, Some(Box::new(RandomStrategy::new(7))));
        let view = view_with_actions(&["x", "y", "z"]);
        // tick0 = 脚本的 scripted
        assert_eq!(s.choose(&view).map(|a| a.action), Some("scripted".to_string()));
        // tick1 起交给 random：选出的动作（若有）必须来自 view 合法集合
        let mut saw_explore = false;
        for _ in 0..200 {
            if let Some(a) = s.choose(&view) {
                assert!(view.actions.contains(&a), "接力策略只选合法动作: {a:?}");
                saw_explore = true;
            }
        }
        assert!(saw_explore, "脚本放完后接力策略应注入过动作");
    }

    #[test]
    fn scripted_no_explore_goes_silent_after_script() {
        let script = vec![(0u64, act("a"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["a"]);
        assert_eq!(s.choose(&view).map(|a| a.action), Some("a".to_string()));
        // 脚本放完、无接力：之后永远不操作
        for _ in 0..50 {
            assert_eq!(s.choose(&view), None);
        }
    }

    #[test]
    fn scripted_from_inputs_round_trips_ticks_and_phases() {
        use vitric_sim::InputRecord;
        let inputs = vec![
            InputRecord { tick: 1, action: "left".to_string(), phase: "pressed".to_string() },
            InputRecord { tick: 3, action: "left".to_string(), phase: "released".to_string() },
        ];
        let mut s = ScriptedStrategy::from_inputs(&inputs, None);
        let view = view_with_actions(&["left"]);
        let mut seq = Vec::new();
        for _ in 0..5 {
            seq.push(s.choose(&view).map(|a| (a.action, a.phase)));
        }
        assert_eq!(seq[1], Some(("left".to_string(), "pressed".to_string())));
        assert_eq!(seq[3], Some(("left".to_string(), "released".to_string())));
        assert_eq!(seq[0], None);
        assert_eq!(seq[2], None);
        assert_eq!(seq[4], None);
    }

    #[test]
    fn scripted_same_tick_multiple_keeps_first_only() {
        // 同 tick 两条：只注第一条（已知局限），第二条被跳过
        let script = vec![(2u64, act("first")), (2u64, act("second"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["first", "second"]);
        let mut seq = Vec::new();
        for _ in 0..4 {
            seq.push(s.choose(&view).map(|a| a.action));
        }
        assert_eq!(seq, vec![None, None, Some("first".to_string()), None]);
    }
}
