//! vitric-playtest — agent 集群试玩的进程内地基（设计稿第 1 阶段）。
//!
//! 三块拼起来 = 一局可重放的自动试玩：
//! - [`scene_view`]：从世界/规则自动派生一份「代理所见」（观测/动作/终止），纯投影；
//! - [`strategy`]：消费视图、产出动作的纯逻辑策略（random/greedy），PCG 播种 = 确定；
//! - [`session`]：循环「派生视图 → 选动作 → 注入 → 步进」直到通关/死亡/超时，出录像。
//!
//! 装配运行时（`Runtime::boot`）住在 vitric-cli，依赖方向是 cli → playtest，所以本 crate
//! 不 boot 项目，由调用方 boot 好再把 `(Sim, GameLogic, Engine)` 交给 [`session::run_session`]。

pub mod report;
pub mod scene_view;
pub mod session;
pub mod strategy;
pub mod swarm;

pub use report::{aggregate, Report};
pub use scene_view::{Action, Outcome, SceneView, TerminalSpec};
pub use session::{run_session, SessionConfig, SessionResult};
pub use strategy::{CoverageStrategy, GreedyStrategy, RandomStrategy, Strategy};
pub use swarm::{run_swarm, LabeledResult, SessionSpec, StrategyKind};
