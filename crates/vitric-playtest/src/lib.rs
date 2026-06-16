//! vitric-playtest — agent 集群试玩的进程内地基（设计稿第 1 阶段）。
//!
//! 三块拼起来 = 一局可重放的自动试玩：
//! - [`scene_view`]：从世界/规则自动派生一份「代理所见」（观测/动作/终止），纯投影；
//! - [`strategy`]：消费视图、产出动作的纯逻辑策略（random/greedy/coverage/scripted），PCG 播种 = 确定；
//! - [`session`]：循环「派生视图 → 选动作 → 注入 → 步进」直到通关/死亡/超时，出录像。
//!
//! 第 3 阶段又拼上**种子式探索**（设计稿三节）：
//! - [`seed`]：拿 gate 证书录像当种子，受控扰动它的输入序列生成一组变异脚本；
//! - [`strategy::ScriptedStrategy`]：按脚本在录制 tick 注入（可接 random 截断发散）；
//! - [`report`] 新增 `ending_coverage`：哪些声明的结局被触达、哪些 0 局可达（不可达结局）。
//!
//! 第 5 阶段拼上 **LLM 档**（设计稿二节/十一节第 5 条）：
//! - [`llm_agent::LlmStrategy`]：少量 LLM 代理读同一份 Scene View 拟人玩 + 吐定性 note
//!   （清晰度/连续性/选择有效性）。LLM 推理不确定，但它选的输入照样录进录像→可重放复现；
//! - [`strategy::PlaytestNote`] + `Strategy::drain_notes`：note 通道（默认空，只 LLM 产）；
//! - [`session::SessionResult`] 收 note（不进哈希/录像），[`report`] 汇成 `qualitative_notes`
//!   （按 kind 分组去重，诚实标「LLM 主观提示，待人复核」）。
//!
//! 装配运行时（`Runtime::boot`）住在 vitric-cli，依赖方向是 cli → playtest，所以本 crate
//! 不 boot 项目，由调用方 boot 好再把 `(Sim, GameLogic, Engine)` 交给 [`session::run_session`]。

pub mod config;
pub mod llm_agent;
pub mod report;
pub mod scene_view;
pub mod seed;
pub mod session;
pub mod strategy;
pub mod swarm;

pub use config::{
    DerivedSpec, DistanceMetric, GoalDirection, GoalSpec, ObservationConfig, PlaytestConfig,
    Relabel, TerminalOverride,
};
pub use llm_agent::{LlmClient, LlmStrategy};
pub use report::{
    aggregate, aggregate_with_endings, aggregate_with_endings_and_declared, CollapseField,
    DominantAction, EndingCoverage,
    NonFiniteField, NoteCluster, NumericBreakage, QualitativeNotes, RecordingRef, Report,
    RunawayField,
};
pub use scene_view::{Action, Outcome, SceneView, TerminalSpec};
pub use seed::{perturb_plan, PerturbOp, Perturbation};
pub use session::{run_session, NumericStat, SessionConfig, SessionResult};
pub use strategy::{
    CoverageStrategy, EconomyStrategy, GreedyStrategy, PlaytestNote, RandomStrategy,
    ScriptedStrategy, Strategy,
};
pub use swarm::{
    run_llm_sessions, run_seed_swarm, run_swarm, run_swarm_with_config, LabeledResult, SessionSpec,
    StrategyKind,
};
