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

use std::sync::Arc;
use std::thread;

use vitric_rules::Engine;
use vitric_sim::{GameLogic, ReplyRecord, Sim};

use crate::llm_agent::{LlmClient, LlmStrategy};
use crate::scene_view::{Outcome, TerminalSpec};
use crate::seed::Perturbation;
use crate::session::{run_session, run_session_lookahead, LookaheadConfig, SessionConfig, SessionResult};
use crate::strategy::{
    CoverageStrategy, EconomyStrategy, GreedyStrategy, RandomStrategy, ScriptedStrategy, Strategy,
};

/// 策略种类（spec 里用名字指定，跑的时候据此 new 出策略实例）。
/// 是可序列化的纯标签——结果带回它，聚合器按 strategy_kind 分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StrategyKind {
    Random,
    Greedy,
    Coverage,
    /// 经济压力策略（模拟经营找数值崩）：锁定一个动作连按 R 次再轮转（设计稿四阶段）。
    Economy,
    /// 种子探索的脚本回放局——只作**标签**（结果归类用）。脚本本身不在 SessionSpec 里
    /// （脚本是变长的，且每条不同），由 `run_seed_swarm` 直接持有 Perturbation 构造策略。
    Scripted,
    /// LLM 拟人玩局——只作**标签**（设计稿五阶段）。LLM 策略带 client（非 Send 的 trait 对象
    /// 形态各异），不从 (kind,seed) 构造，由 `cmd_playtest --llm` 直接持有 LlmStrategy 跑，
    /// 跑完贴这个标签把结果并进 swarm 结果集（note 进 qualitative_notes）。
    Llm,
    /// 前瞻搜索局（技巧/导航类专用）。它不是 `Strategy`——要 sim/logic 自己 snapshot/restore
    /// 投机选最优动作（走 `run_session_lookahead` 而非 `run_session`），所以 horizon 直接挂在
    /// 变体上。声明了 goal 的项目，默认 swarm 会掺几局这种进来（导航类才不被误报 unbeatable）。
    /// `run_one` 在每会话执行处按这个变体分流到 `run_session_lookahead`。
    Lookahead { horizon: u64 },
}

impl StrategyKind {
    /// 廉价策略档全集（CLI 默认轮流跑这几种用）。**不含 Scripted**——脚本回放走种子探索
    /// 专路（`run_seed_swarm`），不进「广度覆盖」的策略轮换。含 Economy：模拟经营游戏靠它
    /// 才逮得到经济跑飞/崩盘，把它放进默认轮换不挑游戏类型（非经营游戏它就是另一种压力测试）。
    pub const ALL: [StrategyKind; 4] = [
        StrategyKind::Random,
        StrategyKind::Greedy,
        StrategyKind::Coverage,
        StrategyKind::Economy,
    ];

    /// 短名（报告/CLI 显示用）。
    pub fn name(self) -> &'static str {
        match self {
            StrategyKind::Random => "random",
            StrategyKind::Greedy => "greedy",
            StrategyKind::Coverage => "coverage",
            StrategyKind::Economy => "economy",
            StrategyKind::Scripted => "scripted",
            StrategyKind::Llm => "llm",
            StrategyKind::Lookahead { .. } => "lookahead",
        }
    }

    /// 按种类 + seed 造一个策略实例（PCG 播种，确定可复现）。
    /// `goal`：greedy 的派生目标（playtest.json 声明）——有目标时 greedy 朝它走，无则退化随机。
    /// 其他策略不看 goal。Scripted/Llm 不走这条路（脚本/client 不在 seed 里）——明确 panic 不静默退化。
    fn build(self, seed: u64, goal: &Option<crate::config::GoalSpec>) -> Box<dyn Strategy> {
        match self {
            StrategyKind::Random => Box::new(RandomStrategy::new(seed)),
            StrategyKind::Greedy => match goal {
                Some(g) => Box::new(GreedyStrategy::with_goal(seed, g.clone())),
                None => Box::new(GreedyStrategy::new(seed)),
            },
            StrategyKind::Coverage => Box::new(CoverageStrategy::new(seed)),
            StrategyKind::Economy => Box::new(EconomyStrategy::new(seed)),
            StrategyKind::Scripted => {
                panic!("Scripted 策略要带脚本，必须走 run_seed_swarm，不能用 StrategyKind::build")
            }
            StrategyKind::Llm => {
                panic!("Llm 策略要带 client，必须由 cmd_playtest --llm 直接构造 LlmStrategy，不能用 StrategyKind::build")
            }
            StrategyKind::Lookahead { .. } => {
                panic!("Lookahead 不是 Strategy，要 sim/logic 自己投机，必须走 run_session_lookahead，不能用 StrategyKind::build")
            }
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

/// 默认 swarm 掺入的前瞻局用的搜索地平线。比单局 `--strategy lookahead` 的默认 12 大：
/// 技巧/导航类要看够远才算得出「先绕一步再回正」的收益（如 nav 跳墙：跳起到越过墙顶减小
/// 距离要 ~20+ 帧，horizon 12 看不到落点收益就退化成原地踏步）。24 是 nav fixture 实测能稳定
/// 通关的最小值；动作集小（导航类一般个位数候选），24 帧投机成本仍可控。单局 `--strategy
/// lookahead` 不受此影响（仍用 `--horizon`/默认 12，用户可显式调大）。
pub const DEFAULT_SWARM_LOOKAHEAD_HORIZON: u64 = 24;

/// 默认 swarm 的会话计划：N 局，廉价策略（random/greedy/coverage/economy）轮换 × 递增 seed。
///
/// **声明了 goal 时掺前瞻**：lookahead 需要项目声明 goal（`playtest.json` 的 PlaytestConfig.goal）
/// 才有方向，否则没意义。所以仅当 `has_goal` 为真时，把计划里固定少数几局换成 `Lookahead`——
/// 导航/技巧类（先跳上墙再走）random 0% 通关，不掺前瞻就会被误报 unbeatable。比例克制（前瞻慢：
/// 每真 tick = |候选+不操作|×horizon 个投机 step）：取 `min(N, max(2, N/4))` 局（约 25%，至少 2、
/// 不超过 N）。**没声明 goal 时一局都不换**，默认组完全不变（向后兼容）。
///
/// 掺入的前瞻局用 [`DEFAULT_SWARM_LOOKAHEAD_HORIZON`]（比单局默认大，技巧类才看得到收益）。
/// 哪几局换成前瞻：取计划**末尾**那几局换（前面的轮换槽位不动，diff 最小、好对账）。被换的局
/// 沿用它原本的 seed/max_ticks/terminal，只把 strategy_kind 改成 `Lookahead{horizon}`。
pub fn default_plan(
    sessions: u64,
    seed: u64,
    max_ticks: u64,
    terminal: TerminalSpec,
    has_goal: bool,
) -> Vec<SessionSpec> {
    let mut plan: Vec<SessionSpec> = Vec::with_capacity(sessions as usize);
    for k in 0..sessions {
        let kind = StrategyKind::ALL[(k as usize) % StrategyKind::ALL.len()];
        plan.push(SessionSpec { strategy_kind: kind, seed: seed + k, max_ticks, terminal: terminal.clone() });
    }
    if has_goal && sessions > 0 {
        // 掺入局数：约 1/4，至少 2，但不超过总局数。
        let look_n = ((sessions / 4).max(2)).min(sessions) as usize;
        // 换末尾 look_n 局（前面轮换槽位不动）。
        let start = plan.len() - look_n;
        for spec in &mut plan[start..] {
            spec.strategy_kind =
                StrategyKind::Lookahead { horizon: DEFAULT_SWARM_LOOKAHEAD_HORIZON };
        }
    }
    plan
}

/// 跑一条 spec（在调用线程内 boot 一份运行时，跑一局，出标签结果）。
/// swarm 的串行/并行两条路都收敛到这一个函数——保证「怎么跑都跑出同一份结果」。
/// `config`：每游戏视图覆盖（include/exclude/relabel/派生量/goal/terminal）。greedy 用它的 goal
/// 找目标；session 用它走 derive_with_config。默认空配置=自动推，行为同阶段 1~5。
fn run_one<R, F>(
    factory: &F,
    spec: &SessionSpec,
    config: &crate::config::PlaytestConfig,
) -> Result<LabeledResult, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String>,
{
    // 每局自己 boot：运行时不跨局复用（录可重放录像必须冷启动），更不跨线程
    let (mut sim, mut logic, engine) = factory()?;
    let cfg = SessionConfig {
        max_ticks: spec.max_ticks,
        seed: spec.seed,
        terminal: spec.terminal.clone(),
        playtest: config.clone(),
        // 普通 swarm/单局不重放种子回复（那是种子探索专路 run_seed_swarm 才有的事）
        seed_replies: Vec::new(),
    };
    // 分流：Lookahead 不是 Strategy，要 sim/logic 自己 snapshot/restore 投机选动作，走
    // run_session_lookahead；其余照旧 build 出 Strategy 走 run_session。两条路同口径产
    // SessionResult（state_trace/numeric_summary/fired_events/notes/recording 都齐），聚合不缺数据。
    let result = match spec.strategy_kind {
        StrategyKind::Lookahead { horizon } => {
            run_session_lookahead(&mut sim, &mut logic, &engine, &cfg, &LookaheadConfig { horizon })?
        }
        _ => {
            let mut strategy = spec.strategy_kind.build(spec.seed, &config.goal);
            run_session(&mut sim, &mut logic, &engine, strategy.as_mut(), &cfg)?
        }
    };
    Ok(LabeledResult { spec: spec.clone(), result })
}

/// 跑一整批策略局。`factory` 必须 `Sync`（多个线程共享同一个闭包引用、各自调一次）；
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
    // 默认空配置（自动推视图、greedy 无目标退化随机）——和阶段 1~5 行为一致。
    run_swarm_with_config(factory, plan, &crate::config::PlaytestConfig::default(), threads)
}

/// 带每游戏视图覆盖的 swarm（设计稿一节「自动推 + 可选覆盖」、十一节第 6 条）。
/// `config` 对整批生效：策略看到 include/exclude/relabel/派生量调整后的视图，greedy 朝
/// config.goal 走。其余确定性/并行/归位保证同 [`run_swarm`]。
pub fn run_swarm_with_config<R, F>(
    factory: F,
    plan: &[SessionSpec],
    config: &crate::config::PlaytestConfig,
    threads: usize,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String> + Sync,
{
    // 按下标跑：每条 spec 在调用线程内 boot 一份运行时 + build 策略（带 config.goal），跑一局
    run_indexed(plan.len(), threads, |i| run_one::<R, F>(&factory, &plan[i], config))
}

/// 种子探索批跑（设计稿三节）：把一组**扰动脚本**铺到并行线程，每条用
/// [`ScriptedStrategy`] 跑一局——脚本喂回去复现/走岔，截断的接 random 发散。
/// 复用 `run_swarm` 同款下标并行核（`run_indexed`），所以**串行/并行结果逐项一致**，
/// 结果可直接喂 `aggregate_with_endings`（含不可达结局）。
///
/// `seed_replies`：种子录像里的外部回复（LLM 内容等）。扰动只动**输入**，回复跟着种子走、
/// 原样按原 tick 注回去（和 `Sim::replay` 同口径）——否则靠回复才通关的结局（如 echo）基线
/// 复现不出来。截断脚本只注**截断点之前**的回复（截断后是 random 发散，没有种子回复了）。
///
/// 每条结果的 spec 标 `StrategyKind::Scripted`、seed=该脚本在 plan 里的下标
/// （让结果可对账到具体哪条扰动；脚本本身不进 spec，太长且变长）。
/// 截断脚本（`truncate_at=Some`）的发散随机用 `explore_seed + 下标` 播种——确定可复现。
pub fn run_seed_swarm<R, F>(
    factory: F,
    plan: &[Perturbation],
    seed_replies: &[ReplyRecord],
    max_ticks: u64,
    terminal: TerminalSpec,
    explore_seed: u64,
    threads: usize,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String> + Sync,
{
    run_indexed(plan.len(), threads, |i| {
        let pert = &plan[i];
        let (mut sim, mut logic, engine) = factory()?;
        // 截断脚本接 random 发散；非截断脚本放完即止（None）。
        // 发散 random 用 explore_seed + 下标播种：每条脚本一个独立可复现的发散序列。
        let then_explore: Option<Box<dyn Strategy>> = if pert.truncate_at.is_some() {
            Some(Box::new(RandomStrategy::new(explore_seed.wrapping_add(i as u64))))
        } else {
            None
        };
        let mut strategy = ScriptedStrategy::new(pert.script.clone(), then_explore);
        // 这局要注的种子回复：截断脚本只留截断点之前的（截断后没有种子回复）；
        // 非截断脚本注全部。drop/swap/substitute 不改 tick 轴，回复照原 tick 注。
        let replies: Vec<ReplyRecord> = match pert.truncate_at {
            Some(cut) => seed_replies.iter().filter(|r| r.tick < cut).cloned().collect(),
            None => seed_replies.to_vec(),
        };
        let cfg = SessionConfig {
            max_ticks,
            seed: i as u64,
            terminal: terminal.clone(),
            seed_replies: replies,
            ..Default::default()
        };
        let result = run_session(&mut sim, &mut logic, &engine, &mut strategy, &cfg)?;
        // spec 标 Scripted + seed=下标，便于把结果对回具体哪条扰动
        let spec = SessionSpec {
            strategy_kind: StrategyKind::Scripted,
            seed: i as u64,
            max_ticks,
            terminal: terminal.clone(),
        };
        Ok(LabeledResult { spec, result })
    })
}

/// LLM 档批跑（设计稿五阶段）：少量 LLM 代理读同一份 Scene View 拟人玩 + 吐定性 note。
///
/// **为什么串行、不并行**：LLM 局天然慢、单独限流、不拖累策略档（设计稿九节）；而且 LLM
/// 推理不确定，这几局的 outcome/note **不要求跨次复现**——并行与否对结果没有「确定性」意义。
/// 串行最简单：共享一个 `client`（`Arc<dyn LlmClient>`，Send+Sync），逐局自己 boot 一份运行时
/// （和策略档同款冷启动约束，录像才可重放），跑一局 LlmStrategy，贴 `StrategyKind::Llm` 标签。
///
/// 每局 seed=`base_seed + 下标`（给 LlmStrategy 的预留 PCG 播种，当前不影响选择，留作对账）。
/// `goal` 拼进提示词的目标描述。返回的 `LabeledResult` 可直接和策略档结果**拼进同一个结果集**
/// 喂聚合器——LLM 局的 note 进 `qualitative_notes`，它选的输入进录像（可 `Sim::replay` 复现）。
pub fn run_llm_sessions<R, F>(
    factory: F,
    client: Arc<dyn LlmClient>,
    count: usize,
    goal: &str,
    base_seed: u64,
    max_ticks: u64,
    terminal: TerminalSpec,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String>,
{
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (mut sim, mut logic, engine) = factory()?;
        let seed = base_seed.wrapping_add(i as u64);
        // 每局一个新 LlmStrategy，共享同一个 client（Arc 包成 Box<dyn LlmClient> 喂构造）
        let mut strategy = LlmStrategy::new(Box::new(SharedClient(client.clone())), goal, seed);
        let cfg = SessionConfig { max_ticks, seed, terminal: terminal.clone(), ..Default::default() };
        let result = run_session(&mut sim, &mut logic, &engine, &mut strategy, &cfg)?;
        let spec = SessionSpec {
            strategy_kind: StrategyKind::Llm,
            seed,
            max_ticks,
            terminal: terminal.clone(),
        };
        out.push(LabeledResult { spec, result });
    }
    Ok(out)
}

/// 把 `Arc<dyn LlmClient>` 包成一个 `LlmClient`，让多局共享同一个底层 client
/// （Box 要独占所有权，Arc 让 N 局都拿到同一个真 client，省 N 次重连/重配）。
struct SharedClient(Arc<dyn LlmClient>);

impl LlmClient for SharedClient {
    fn complete(&self, prompt: &str) -> Result<String, String> {
        self.0.complete(prompt)
    }
}

/// 下标并行核：跑 `len` 个任务，第 i 个任务由 `task(i)` 定义，结果按下标归位。
/// `run_swarm` 和 `run_seed_swarm` 共用它——线程切分/归位/fail-fast 只此一份，
/// 保证两条路「串行/并行逐项一致」的确定性铁律是同一套保证。
fn run_indexed<T>(
    len: usize,
    threads: usize,
    task: T,
) -> Result<Vec<LabeledResult>, String>
where
    T: Fn(usize) -> Result<LabeledResult, String> + Sync,
{
    if len == 0 {
        return Ok(Vec::new());
    }

    // 实际线程数：不超过想要的、不超过任务数、不超过机器核数（默认拿 available_parallelism）
    let cpu = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let n_threads = threads.max(1).min(len).min(cpu.max(1));

    // 单线程：直接串行，连 scope 都不开（小批量/单核常态，零线程开销）
    if n_threads <= 1 {
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            out.push(task(i)?);
        }
        return Ok(out);
    }

    // 多线程：把下标轮流分给 n_threads 个桶（round-robin 切分），每个线程跑自己那批，
    // 结果连同原始下标一起回收，最后按下标归位。切法不影响结果（确定性不依赖切分）。
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); n_threads];
    for i in 0..len {
        buckets[i % n_threads].push(i);
    }

    // scope 让工作线程能借 task（栈上引用），无需 'static / Arc
    let collected: Vec<Result<Vec<(usize, LabeledResult)>, String>> = thread::scope(|scope| {
        let task_ref = &task;
        let handles: Vec<_> = buckets
            .into_iter()
            .map(|idxs| {
                scope.spawn(move || {
                    let mut local = Vec::with_capacity(idxs.len());
                    for i in idxs {
                        // 任一局出错就把错带出来（fail-fast，不静默丢）
                        local.push((i, task_ref(i)?));
                    }
                    Ok(local)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("工作线程不应 panic")).collect()
    });

    // 汇总：先把所有线程的错收掉（有错就返回第一个），再按原始下标排回去
    let mut indexed: Vec<(usize, LabeledResult)> = Vec::with_capacity(len);
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

    // ---- default_plan：声明 goal 时掺前瞻 / 无 goal 完全不变 ----

    /// 一个会话计划里前瞻局的数量。
    fn count_lookahead(plan: &[SessionSpec]) -> usize {
        plan.iter()
            .filter(|s| matches!(s.strategy_kind, StrategyKind::Lookahead { .. }))
            .count()
    }

    #[test]
    fn default_plan_no_goal_is_pure_rotation_unchanged() {
        // 无 goal：计划必须和「四策略轮换 × 递增 seed」逐条一致，一局前瞻都不掺（向后兼容）。
        let plan = default_plan(8, 0, 50, TerminalSpec::default(), false);
        assert_eq!(plan.len(), 8);
        assert_eq!(count_lookahead(&plan), 0, "无 goal 不该掺前瞻");
        for (k, spec) in plan.iter().enumerate() {
            assert_eq!(spec.strategy_kind, StrategyKind::ALL[k % StrategyKind::ALL.len()]);
            assert_eq!(spec.seed, k as u64);
        }
    }

    #[test]
    fn default_plan_with_goal_mixes_in_lookahead() {
        // 有 goal：约 1/4 局换成前瞻（8 局 → 2 局），换的是末尾几局，其余轮换槽位不动。
        let plan = default_plan(8, 0, 50, TerminalSpec::default(), true);
        assert_eq!(plan.len(), 8);
        assert_eq!(count_lookahead(&plan), 2, "8 局应掺 2 局前瞻（约 25%）: {plan:?}");
        // 末尾 2 局是前瞻、前 6 局仍是原轮换
        assert!(matches!(plan[6].strategy_kind, StrategyKind::Lookahead { .. }));
        assert!(matches!(plan[7].strategy_kind, StrategyKind::Lookahead { .. }));
        for (k, spec) in plan.iter().enumerate().take(6) {
            assert_eq!(spec.strategy_kind, StrategyKind::ALL[k % StrategyKind::ALL.len()]);
        }
        // 前瞻局沿用原 seed/max_ticks/terminal，只换 kind
        assert_eq!(plan[7].seed, 7);
        assert_eq!(plan[7].max_ticks, 50);
        // 掺入的前瞻用默认 swarm 地平线（比单局默认大）
        assert_eq!(
            plan[7].strategy_kind,
            StrategyKind::Lookahead { horizon: DEFAULT_SWARM_LOOKAHEAD_HORIZON }
        );
    }

    #[test]
    fn default_plan_with_goal_keeps_at_least_two_lookahead_for_small_n() {
        // 小 N：至少 2 局前瞻、但不超过总局数。N=1 → 1 局（min 起作用）。
        assert_eq!(count_lookahead(&default_plan(1, 0, 50, TerminalSpec::default(), true)), 1);
        assert_eq!(count_lookahead(&default_plan(3, 0, 50, TerminalSpec::default(), true)), 2);
        // N=20 → 1/4=5 局
        assert_eq!(count_lookahead(&default_plan(20, 0, 50, TerminalSpec::default(), true)), 5);
    }

    /// 一个会发 game-won 的最小逻辑，但只有「没注入任何输入」才发——用来证明前瞻局确实走了
    /// `run_session_lookahead`（它会逐候选投机；空动作词汇下「不操作」是唯一候选，照样能跑通通关）。
    /// 这里主要验证 run_one 对 Lookahead 变体的分流：不 panic（build 会 panic）、产出齐遥测。
    #[test]
    fn run_swarm_dispatches_lookahead_spec_to_lookahead_runner() {
        // 计划：一局普通 random + 一局 Lookahead。两局都应跑通、都带齐遥测（state_trace 非空、有录像）。
        let plan = vec![
            SessionSpec::new(StrategyKind::Random, 0, 20),
            SessionSpec::new(StrategyKind::Lookahead { horizon: 4 }, 1, 20),
        ];
        // factory_winning(Some(3))：tick 3 发 game-won，两条路都该在 tick 4 通关
        let out = run_swarm(factory_winning(Some(3)), &plan, 2).unwrap();
        assert_eq!(out.len(), 2);
        // 前瞻局没 panic（若误走 build 会 panic）、结局/遥测齐全
        let look = &out[1];
        assert!(matches!(look.spec.strategy_kind, StrategyKind::Lookahead { .. }));
        assert_eq!(look.outcome(), Outcome::Win, "前瞻局也该收到 tick3 的 game-won");
        assert_eq!(look.result.ticks, 4);
        // 遥测同口径：每 tick 一条 state_hash + 一份可序列化录像
        assert_eq!(look.result.state_trace.len(), look.result.ticks as usize);
        assert!(!look.result.recording.checkpoints.is_empty());
    }

    /// 含前瞻局的混合计划，串行 vs 并行逐项一致（确定性铁律对前瞻同样成立）。
    #[test]
    fn swarm_mixed_lookahead_serial_and_parallel_identical() {
        let plan = vec![
            SessionSpec::new(StrategyKind::Random, 0, 40),
            SessionSpec::new(StrategyKind::Lookahead { horizon: 5 }, 1, 40),
            SessionSpec::new(StrategyKind::Greedy, 2, 40),
            SessionSpec::new(StrategyKind::Lookahead { horizon: 8 }, 3, 40),
        ];
        let serial = run_swarm(factory_winning(Some(10)), &plan, 1).unwrap();
        let parallel = run_swarm(factory_winning(Some(10)), &plan, 8).unwrap();
        assert_eq!(serial.len(), parallel.len());
        for (a, b) in serial.iter().zip(parallel.iter()) {
            assert_eq!(a.spec, b.spec, "spec 顺序一致");
            assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
            assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
            assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
            let ja = serde_json::to_string(&a.result.recording).unwrap();
            let jb = serde_json::to_string(&b.result.recording).unwrap();
            assert_eq!(ja, jb, "含前瞻局的混合计划串/并行录像逐字节一致");
        }
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

    use crate::scene_view::Action;
    use crate::seed::{PerturbOp, Perturbation};

    fn pert(op: PerturbOp, script: Vec<(u64, &str)>, trunc: Option<u64>) -> Perturbation {
        Perturbation {
            op,
            script: script
                .into_iter()
                .map(|(t, a)| (t, Action { action: a.to_string(), phase: "pressed".to_string() }))
                .collect(),
            truncate_at: trunc,
        }
    }

    fn seed_plan() -> Vec<Perturbation> {
        vec![
            pert(PerturbOp::Baseline, vec![(1, "go")], None),
            pert(PerturbOp::Drop, vec![], None),
            pert(PerturbOp::Truncate, vec![(1, "go")], Some(2)), // 截断后 random 发散
        ]
    }

    #[test]
    fn seed_swarm_results_follow_plan_order_and_label_scripted() {
        let plan = seed_plan();
        let out = run_seed_swarm(
            factory_winning(Some(3)),
            &plan,
            &[],
            50,
            TerminalSpec::default(),
            7,
            4,
        )
        .unwrap();
        assert_eq!(out.len(), plan.len());
        for (i, lr) in out.iter().enumerate() {
            assert_eq!(lr.spec.strategy_kind, StrategyKind::Scripted, "脚本局标 Scripted");
            assert_eq!(lr.spec.seed, i as u64, "seed=下标，便于对账");
        }
    }

    #[test]
    fn seed_swarm_serial_and_parallel_identical() {
        let plan = seed_plan();
        let serial =
            run_seed_swarm(factory_winning(Some(3)), &plan, &[], 80, TerminalSpec::default(), 11, 1)
                .unwrap();
        let parallel =
            run_seed_swarm(factory_winning(Some(3)), &plan, &[], 80, TerminalSpec::default(), 11, 8)
                .unwrap();
        assert_eq!(serial.len(), parallel.len());
        for (a, b) in serial.iter().zip(parallel.iter()) {
            assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
            assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
            assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
            let ja = serde_json::to_string(&a.result.recording).unwrap();
            let jb = serde_json::to_string(&b.result.recording).unwrap();
            assert_eq!(ja, jb, "种子探索串/并行录像逐字节一致");
        }
    }

    #[test]
    fn seed_swarm_empty_plan_yields_empty() {
        let out =
            run_seed_swarm(factory_winning(Some(3)), &[], &[], 50, TerminalSpec::default(), 0, 4)
                .unwrap();
        assert!(out.is_empty());
    }

    /// 一个只有收到 "oracle-says" 回复才发 game-won 的逻辑——靠回复才通关，光按输入通不了。
    /// 用来验证种子探索真把种子回复按 tick 注回去了（基线该复现到 Win）。
    struct WinOnReply {
        pending: Vec<Event>,
    }
    impl GameLogic for WinOnReply {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            events: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            for e in events {
                if e.name == "oracle-says" && e.data.get("answer") == Some(&json!("open")) {
                    self.pending.push(Event::new("game-won", json!({})));
                }
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    fn factory_reply_gated() -> impl Fn() -> Result<(Sim, WinOnReply, Engine), String> {
        || Ok((Sim::new(1), WinOnReply { pending: vec![] }, empty_engine()))
    }

    #[test]
    fn seed_swarm_injects_seed_replies_baseline_reaches_win() {
        // 种子：tick 2 一条 oracle-says{answer:"open"}（靠它才通关）。脚本只有输入（这里空脚本）。
        let plan = vec![pert(PerturbOp::Baseline, vec![], None)];
        let replies = vec![ReplyRecord {
            tick: 2,
            name: "oracle-says".to_string(),
            data: json!({"answer": "open"}),
        }];
        // 注了种子回复 → 基线复现到 Win
        let out = run_seed_swarm(
            factory_reply_gated(),
            &plan,
            &replies,
            50,
            TerminalSpec::default(),
            0,
            1,
        )
        .unwrap();
        assert_eq!(out[0].outcome(), Outcome::Win, "种子回复注回去了，基线该通关");

        // 反证：不传种子回复 → 没有 oracle-says → 永远通不了 → Timeout
        let out_no = run_seed_swarm(
            factory_reply_gated(),
            &plan,
            &[],
            50,
            TerminalSpec::default(),
            0,
            1,
        )
        .unwrap();
        assert_eq!(out_no[0].outcome(), Outcome::Timeout, "没回复就通不了");
    }

    #[test]
    fn seed_swarm_truncate_drops_replies_after_cut() {
        // 种子回复在 tick 5；截断点在 tick 3 → 截断后没有种子回复 → 注不到那条 → 通不了。
        let plan = vec![pert(PerturbOp::Truncate, vec![], Some(3))];
        let replies = vec![ReplyRecord {
            tick: 5,
            name: "oracle-says".to_string(),
            data: json!({"answer": "open"}),
        }];
        let out = run_seed_swarm(
            factory_reply_gated(),
            &plan,
            &replies,
            50,
            TerminalSpec::default(),
            0,
            1,
        )
        .unwrap();
        assert_eq!(out[0].outcome(), Outcome::Timeout, "截断点之后的种子回复不该被注入");
    }
}
