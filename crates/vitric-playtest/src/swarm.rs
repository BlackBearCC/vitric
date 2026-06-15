//! swarm 跑批：把一份「跑哪些局」的计划并行铺到多个工作线程，每局出一份
//! plain-data 的 [`LabeledResult`]（设计稿七节 A2、九节性能预算）。
//!
//! **关键并行架构（避 QuickJS 非 Send 坑）**：脚本运行时（QuickJS）不是 `Send`，
//! 不能跨线程搬。所以约定：调用方给一个 **工厂闭包** `factory: Fn() -> (Sim, R, Engine)`，
//! 每个工作线程**在自己线程内**调 `factory()` 自己 boot 一份全新运行时，跑完只把
//! plain-data 的结果（`SessionResult`，全是 Send 的数值/字符串/录像）回传。运行时
//! 对象绝不跨线程边界——线程间只流动「怎么跑」的 spec 和「跑出什么」的结果。
//!
//! **确定性铁律**：每条 spec 自带 (策略, seed, max_ticks, terminal)，一局的结果只由
//! spec 决定，不碰线程调度——所以 `run_swarm` 串行跑和并行跑，结果逐项一致。线程只
//! 决定「谁先跑完」，不决定「跑出什么」；结果按 spec 在 plan 里的原始下标归位。

use std::thread;

use vitric_rules::Engine;
use vitric_sim::{GameLogic, Sim};

use crate::scene_view::{Outcome, TerminalSpec};
use crate::session::{run_session, SessionConfig, SessionResult};
use crate::strategy::{CoverageStrategy, GreedyStrategy, RandomStrategy, Strategy};

/// 策略种类（spec 里用名字指定，跑的时候据此 new 出策略实例）。
/// 是可序列化的纯标签——结果带回它，聚合器按 strategy_kind 分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StrategyKind {
    Random,
    Greedy,
    Coverage,
}

impl StrategyKind {
    /// 全部策略种类（CLI 默认轮流跑这几种用）。
    pub const ALL: [StrategyKind; 3] =
        [StrategyKind::Random, StrategyKind::Greedy, StrategyKind::Coverage];

    /// 短名（报告/CLI 显示用）。
    pub fn name(self) -> &'static str {
        match self {
            StrategyKind::Random => "random",
            StrategyKind::Greedy => "greedy",
            StrategyKind::Coverage => "coverage",
        }
    }

    /// 按种类 + seed 造一个策略实例（PCG 播种，确定可复现）。
    fn build(self, seed: u64) -> Box<dyn Strategy> {
        match self {
            StrategyKind::Random => Box::new(RandomStrategy::new(seed)),
            StrategyKind::Greedy => Box::new(GreedyStrategy::new(seed)),
            StrategyKind::Coverage => Box::new(CoverageStrategy::new(seed)),
        }
    }
}

/// 一局的规格：跑哪种策略、什么 seed、跑多少 tick、哪些事件算终止。
/// 一局的结果**只**由它决定（确定性铁律）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionSpec {
    pub strategy_kind: StrategyKind,
    pub seed: u64,
    pub max_ticks: u64,
    pub terminal: TerminalSpec,
}

impl SessionSpec {
    /// 常用构造：默认终止规格。
    pub fn new(strategy_kind: StrategyKind, seed: u64, max_ticks: u64) -> SessionSpec {
        SessionSpec { strategy_kind, seed, max_ticks, terminal: TerminalSpec::default() }
    }
}

/// 带 spec 标签的一局结果（哪个策略/seed 跑出来的 + 跑出什么）。聚合器吃这个。
#[derive(Debug, Clone)]
pub struct LabeledResult {
    pub spec: SessionSpec,
    pub result: SessionResult,
}

impl LabeledResult {
    /// 便捷读取：这局的结局。
    pub fn outcome(&self) -> Outcome {
        self.result.outcome
    }
}

/// 跑一条 spec（在调用线程内 boot 一份运行时，跑一局，出标签结果）。
/// swarm 的串行/并行两条路都收敛到这一个函数——保证「怎么跑都跑出同一份结果」。
fn run_one<R, F>(factory: &F, spec: &SessionSpec) -> Result<LabeledResult, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String>,
{
    // 每局自己 boot：运行时不跨局复用（录可重放录像必须冷启动），更不跨线程
    let (mut sim, mut logic, engine) = factory()?;
    let mut strategy = spec.strategy_kind.build(spec.seed);
    let cfg = SessionConfig {
        max_ticks: spec.max_ticks,
        seed: spec.seed,
        terminal: spec.terminal.clone(),
    };
    let result = run_session(&mut sim, &mut logic, &engine, strategy.as_mut(), &cfg)?;
    Ok(LabeledResult { spec: spec.clone(), result })
}

/// 跑一整批。`factory` 必须 `Sync`（多个线程共享同一个闭包引用、各自调一次）；
/// `threads` 是想用的并行度上限（实际取 `min(threads, plan 长度, available_parallelism)`）。
///
/// 结果顺序与 `plan` 一致（按原始下标归位），与线程调度无关——所以串行结果和并行结果
/// 逐项一致。任一局 boot/跑出错，整批返回那个错（fail-fast，不吞）。
pub fn run_swarm<R, F>(
    factory: F,
    plan: &[SessionSpec],
    threads: usize,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String> + Sync,
{
    if plan.is_empty() {
        return Ok(Vec::new());
    }

    // 实际线程数：不超过想要的、不超过任务数、不超过机器核数（默认拿 available_parallelism）
    let cpu = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let n_threads = threads.max(1).min(plan.len()).min(cpu.max(1));

    // 单线程：直接串行，连 scope 都不开（小批量/单核常态，零线程开销）
    if n_threads <= 1 {
        let mut out = Vec::with_capacity(plan.len());
        for spec in plan {
            out.push(run_one::<R, F>(&factory, spec)?);
        }
        return Ok(out);
    }

    // 多线程：把 plan 的**下标**轮流分给 n_threads 个桶（round-robin 切分），
    // 每个线程跑自己那批，结果连同原始下标一起回收，最后按下标归位。
    // 用下标而不是切片连续分段：哪种切法结果都一样（确定性不依赖切分），轮转分布更均。
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); n_threads];
    for (i, _) in plan.iter().enumerate() {
        buckets[i % n_threads].push(i);
    }

    // scope 让工作线程能借 plan/factory（栈上引用），无需 'static / Arc
    let collected: Vec<Result<Vec<(usize, LabeledResult)>, String>> = thread::scope(|scope| {
        let factory_ref = &factory;
        let plan_ref = plan;
        let handles: Vec<_> = buckets
            .into_iter()
            .map(|idxs| {
                scope.spawn(move || {
                    let mut local = Vec::with_capacity(idxs.len());
                    for i in idxs {
                        // 任一局出错就把错带出来（fail-fast，不静默丢）
                        let lr = run_one::<R, F>(factory_ref, &plan_ref[i])?;
                        local.push((i, lr));
                    }
                    Ok(local)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("工作线程不应 panic")).collect()
    });

    // 汇总：先把所有线程的错收掉（有错就返回第一个），再按原始下标排回去
    let mut indexed: Vec<(usize, LabeledResult)> = Vec::with_capacity(plan.len());
    for chunk in collected {
        indexed.extend(chunk?);
    }
    indexed.sort_by_key(|(i, _)| *i);
    Ok(indexed.into_iter().map(|(_, lr)| lr).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::{Event, RuleSet};
    use vitric_sim::Pcg32;

    fn empty_engine() -> Engine {
        let schema = Schema::parse(&json!({"components": {}}), "s.json").unwrap();
        Engine::new(RuleSet::parse(&json!({"rules": []}), "r.json").unwrap(), schema)
    }

    /// 在第 N tick 发一个终止事件的最小逻辑（与 session 测试同款，但本文件自带一份）。
    struct EmitAt {
        at: u64,
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitAt {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            tick: u64,
        ) -> Result<(), String> {
            if tick == self.at {
                self.pending.push(Event::new(&self.event, json!({})));
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    /// 工厂：每次造一对新 (sim, logic, engine)。win_at=Some(n) 则在第 n tick 发 game-won。
    fn factory_winning(win_at: Option<u64>) -> impl Fn() -> Result<(Sim, EmitAt, Engine), String> {
        move || {
            let sim = Sim::new(1);
            let logic = match win_at {
                Some(at) => EmitAt { at, event: "game-won".to_string(), pending: vec![] },
                // 永不发终止事件：用一个不可能命中的 at
                None => EmitAt { at: u64::MAX, event: "nope".to_string(), pending: vec![] },
            };
            Ok((sim, logic, empty_engine()))
        }
    }

    fn small_plan() -> Vec<SessionSpec> {
        let mut plan = Vec::new();
        for kind in StrategyKind::ALL {
            for seed in 0..4u64 {
                plan.push(SessionSpec::new(kind, seed, 50));
            }
        }
        plan
    }

    #[test]
    fn swarm_empty_plan_yields_empty() {
        let out = run_swarm(factory_winning(Some(3)), &[], 4).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn swarm_results_follow_plan_order() {
        let plan = small_plan();
        let out = run_swarm(factory_winning(Some(3)), &plan, 4).unwrap();
        assert_eq!(out.len(), plan.len());
        // 每个结果的 spec 必须等于 plan 里对应位置的 spec（顺序未被线程打乱）
        for (lr, spec) in out.iter().zip(plan.iter()) {
            assert_eq!(&lr.spec, spec);
        }
    }

    #[test]
    fn swarm_serial_and_parallel_are_identical() {
        let plan = small_plan();
        // 1 线程（串行）vs 8 线程（并行）：outcome/ticks/state_trace 必须逐项一致
        let serial = run_swarm(factory_winning(Some(5)), &plan, 1).unwrap();
        let parallel = run_swarm(factory_winning(Some(5)), &plan, 8).unwrap();
        assert_eq!(serial.len(), parallel.len());
        for (a, b) in serial.iter().zip(parallel.iter()) {
            assert_eq!(a.spec, b.spec, "spec 顺序一致");
            assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
            assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
            assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
            assert_eq!(a.result.fired_events, b.result.fired_events, "事件集一致");
        }
    }

    #[test]
    fn swarm_propagates_factory_error() {
        let factory = || -> Result<(Sim, EmitAt, Engine), String> { Err("boot 炸了".to_string()) };
        let plan = vec![SessionSpec::new(StrategyKind::Random, 0, 10)];
        let err = run_swarm(factory, &plan, 4).unwrap_err();
        assert!(err.contains("boot 炸了"), "{err}");
    }

    #[test]
    fn swarm_collects_win_outcomes() {
        // 第 3 tick 发 game-won → 每局都应在 tick 4 通关
        let plan = small_plan();
        let out = run_swarm(factory_winning(Some(3)), &plan, 4).unwrap();
        assert!(out.iter().all(|lr| lr.outcome() == Outcome::Win), "全部应通关");
        assert!(out.iter().all(|lr| lr.result.ticks == 4));
    }
}
