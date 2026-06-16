//! 束搜索滚动规划器（run_session_lookahead）端到端：在真实 nav 导航项目上证明「技巧类游戏
//! 需要的多 tick 机动能被规划出来」。
//!
//! nav 关卡：hero 从 x=0 出发，x=5 处有堵挡路的墙（Collider h=2，墙顶 y=1 高过 hero 站地时的
//! 脚底 y=0），必须**先起跳越过墙、再持续右行**，走到 x≥9.5 触发 reached-exit 通关。playtest.json
//! 把目标声明成「最大化 hero 的 x 坐标」（向右推进）——跳跃那一帧不增加 x（甚至像在原地「浪费」
//! 一帧），所以**单步前瞻（depth=1）只会贪心地一直按右、撞墙卡死**；只有规划够深的束搜索才算得出
//! 「跳一下、越过墙、x 重新增长」这串收益，从而通关。
//!
//! 硬证据：
//!  1. **同一关卡、同一规划器，depth=1（退化单步前瞻）超时，depth≥8 的束搜索通关**——证明规划
//!     深度真带来了能力（这是本次「单步→束搜索」升级的核心证据）；
//!  2. 同条件下 lookahead 通关（Win）而 random 超时（Timeout）；
//!  3. lookahead 那局手工攒的录像被 Sim::replay 逐位复现（证明录像对、投机没污染）；
//!  4. 同 (项目,seed,depth,beam) 两次 lookahead 的 outcome/ticks/录像逐字节一致。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate_with_endings_and_declared, default_plan, run_session, run_session_lookahead,
    run_swarm_with_config, LookaheadConfig, Outcome, PlaytestConfig, RandomStrategy, SessionConfig,
    StrategyKind, Strategy, TerminalSpec,
};

/// 本测试用的束搜索深度：nav 越墙机动实测 depth≥8 稳定通关（最小约 8），取 12 留余量。
const NAV_DEPTH: u64 = 12;
/// 本测试用的束宽。
const NAV_BEAM: usize = 4;

fn nav_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vitric-playtest/tests/fixtures/nav")
}

/// 仓库根的示例游戏目录（examples/ 在 workspace 根，本测试 crate 在 crates/vitric-cli 下）。
fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

/// 加载 nav 的 playtest.json（含派生量 hero_x + goal:max + terminal win_events:["reached-exit"]）。
fn nav_cfg(seed: u64, max_ticks: u64) -> (PlaytestConfig, SessionConfig) {
    let config = PlaytestConfig::load(&nav_dir()).unwrap().expect("nav 有 playtest.json");
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    };
    let cfg = SessionConfig {
        max_ticks,
        seed,
        terminal,
        playtest: config.clone(),
        ..Default::default()
    };
    (config, cfg)
}

/// 跑一局束搜索 lookahead（显式指定 depth/beam，便于硬证据对照 depth=1 vs 深搜）。
fn run_lookahead_db(seed: u64, max_ticks: u64, depth: u64, beam: usize) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    run_session_lookahead(&mut sim, &mut rt, &engine, &cfg, &LookaheadConfig { depth, beam_width: beam }).unwrap()
}

/// 默认深度/束宽的 lookahead（多数硬证据用这个）。
fn run_lookahead(seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    run_lookahead_db(seed, max_ticks, NAV_DEPTH, NAV_BEAM)
}

fn run_random(seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strat: Box<dyn Strategy> = Box::new(RandomStrategy::new(seed));
    run_session(&mut sim, &mut rt, &engine, strat.as_mut(), &cfg).unwrap()
}

/// 硬证据 1（**本次升级的核心**）：同一关卡、同一规划器，**depth=1 退化单步前瞻超时，
/// depth≥8 的束搜索通关**——规划深度真带来了「解多 tick 机动」的能力。
///
/// 为什么 depth=1 解不了：nav 目标是「最大化 hero_x」。在墙前，单步前瞻对每个候选只看 1 帧——
/// 跳跃那一帧 x 不增加（甚至像浪费一帧），按右又被墙挡住 x 不变，没有任何单帧动作能改善 hero_x
/// → 贪心退化成一直按右、撞墙卡死 → 超时。深度够的束搜索能往下看到「跳起→越过墙顶→再右行，
/// hero_x 重新增长」这串收益，于是在墙前选择起跳 → 通关。这正是单步前瞻（旧实现）做不到、束搜索
/// 才有的能力。
#[test]
fn lookahead_depth1_times_out_but_beam_search_wins() {
    // depth=1：退化为单步前瞻（每候选只前瞻 1 帧）。撞墙卡死 → 超时。
    let shallow = run_lookahead_db(0, 300, 1, NAV_BEAM);
    assert_eq!(
        shallow.outcome,
        Outcome::Timeout,
        "depth=1 单步前瞻应在墙前卡死超时，实际 {:?} @ {} tick",
        shallow.outcome,
        shallow.ticks
    );
    // depth≥8 的束搜索：规划出「先跳过墙、再右行」→ 通关。
    let deep = run_lookahead_db(0, 300, NAV_DEPTH, NAV_BEAM);
    assert_eq!(deep.outcome, Outcome::Win, "深度束搜索应通关 nav（跳过墙到出口）");
    assert!(deep.ticks < shallow.ticks, "深搜通关 tick({}) 应远少于单步超时 tick({})", deep.ticks, shallow.ticks);
    // 通关路径里**确实有起跳**——多 tick 机动（跳+持续右行）真涌现了，不是蹭出来的。
    assert!(
        deep.recording.inputs.iter().any(|i| i.action == "space"),
        "深搜通关录像里应记录到起跳（space），证明跳过墙的机动被规划出来：{:?}",
        deep.recording.inputs
    );
}

/// 硬证据 2：lookahead 通关，random 同条件超时。
#[test]
fn lookahead_wins_nav_where_random_times_out() {
    let look = run_lookahead(0, 600);
    assert_eq!(look.outcome, Outcome::Win, "束搜索应通关 nav（越墙到出口）");
    assert!(look.ticks < 600, "通关应早于超时上限，实际 {} tick", look.ticks);

    let rand = run_random(0, 600);
    assert_eq!(rand.outcome, Outcome::Timeout, "随机策略凑不齐越墙序列 → 超时");
}

/// 硬证据 3：lookahead 手工攒的录像被 Sim::replay 逐位复现。
#[test]
fn lookahead_nav_recording_replays_bit_for_bit() {
    let look = run_lookahead(0, 600);
    assert_eq!(look.outcome, Outcome::Win);
    // 越墙必有输入（起跳/右行），录像非空
    assert!(!look.recording.inputs.is_empty(), "通关录像应记录到注入的输入");
    assert!(look.recording.checkpoints.first().map(|c| c.0) == Some(0), "起点 checkpoint tick=0");

    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    sim.replay(&look.recording, &mut rt)
        .expect("束搜索手工攒的录像必须可重放且逐位一致");
    assert_eq!(sim.world.state_hash(), look.recording.final_hash);
}

/// 硬证据 4：同 (项目,seed,depth,beam) 两次 lookahead 逐字节一致。
#[test]
fn lookahead_nav_is_deterministic_byte_for_byte() {
    let a = run_lookahead(0, 600);
    let b = run_lookahead(0, 600);
    assert_eq!(a.outcome, b.outcome);
    assert_eq!(a.ticks, b.ticks);
    let ja = serde_json::to_string(&a.recording).unwrap();
    let jb = serde_json::to_string(&b.recording).unwrap();
    assert_eq!(ja, jb, "同 (项目,seed,depth,beam) 两次束搜索录像必须逐字节一致");
}

// ---- 默认 swarm 自动掺前瞻：声明 goal 的技巧/导航类不再被误报 unbeatable ----
//
// 走的是 CLI cmd_playtest 默认 swarm 的同一条路：default_plan（声明 goal 时掺前瞻）+
// run_swarm_with_config（按 spec 分流：Lookahead 走 run_session_lookahead，其余走 run_session）+
// aggregate_with_endings_and_declared（CLI 默认聚合）。这里不通过子进程跑 CLI，而是直接
// 复用同样的库函数，断言更精确（能看到 per-strategy 拆分、证明是前瞻局真把它通的）。

/// 在某项目目录上跑默认 swarm（default_plan + run_swarm_with_config），聚合出报告 + 原始结果。
fn default_swarm(dir: PathBuf, sessions: u64, max_ticks: u64) -> (vitric_playtest::Report, Vec<vitric_playtest::LabeledResult>) {
    let config = PlaytestConfig::load(&dir).unwrap().unwrap_or_default();
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    };
    // CLI 同口径：声明 goal 时掺前瞻，否则纯轮换
    let plan = default_plan(sessions, 0, max_ticks, terminal.clone(), config.goal.is_some());
    let factory = {
        let dir = dir.clone();
        move || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        }
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_swarm_with_config(&factory, &plan, &config, threads).expect("默认 swarm 应跑通");
    let (_, rt) = Runtime::boot(&dir).expect("boot 引擎");
    let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &[]);
    (report, results)
}

/// 硬证据：声明了 goal 的 nav，默认 swarm 通关率 > 0 且不报 unbeatable——前瞻局把它通了。
/// （nav 声明了 goal，default_plan 自动把末尾 2 局换成束搜索前瞻；默认 swarm 深度足以算出跳墙序列。）
#[test]
fn nav_default_swarm_is_not_unbeatable_thanks_to_lookahead() {
    let (report, results) = default_swarm(nav_dir(), 8, 600);

    // 1) 默认 swarm 通关率 > 0、不报 unbeatable（修前若没有前瞻档、greedy 也通不了就会误报）。
    assert!(
        report.outcome_distribution.win_rate > 0.0,
        "声明 goal 的 nav 默认 swarm 应有通关，实际分布 {:?}",
        report.outcome_distribution
    );
    assert!(!report.reachability.unbeatable_by_swarm, "能赢就不该标 unbeatable");

    // 2) 计划里确实掺进了前瞻局（8 局 → 2 局），证明「声明 goal → 自动掺前瞻」生效。
    let look: Vec<_> = results
        .iter()
        .filter(|lr| matches!(lr.spec.strategy_kind, StrategyKind::Lookahead { .. }))
        .collect();
    assert_eq!(look.len(), 2, "8 局默认 swarm 应掺 2 局前瞻，实际 {}", look.len());

    // 3) 前瞻局**确实通关**（不是别的策略蹭的）——前瞻是把 nav 玩通的关键档之一。
    assert!(
        look.iter().any(|lr| lr.result.outcome == Outcome::Win),
        "前瞻局应至少有一局通关 nav（越墙到出口），实际 {:?}",
        look.iter().map(|lr| lr.result.outcome).collect::<Vec<_>>()
    );
    // 前瞻局带齐遥测（和普通局同口径）：state_trace 长度 = tick 数、有可序列化录像。
    for lr in &look {
        assert_eq!(lr.result.state_trace.len(), lr.result.ticks as usize, "前瞻局 state_trace 同口径");
        assert!(!lr.result.recording.checkpoints.is_empty(), "前瞻局应有录像");
    }
}

/// 向后兼容硬证据：examples/jump 没有 playtest.json goal，默认 swarm 一局前瞻都不掺，
/// 仍是 random/greedy/coverage/economy 纯轮换（行为同修前）。
#[test]
fn jump_no_goal_default_swarm_has_no_lookahead() {
    let dir = example("jump");
    // jump 无 playtest.json goal
    let config = PlaytestConfig::load(&dir).unwrap().unwrap_or_default();
    assert!(config.goal.is_none(), "jump 不该有 goal（向后兼容前提）");

    let (_report, results) = default_swarm(dir, 8, 120);
    let look = results
        .iter()
        .filter(|lr| matches!(lr.spec.strategy_kind, StrategyKind::Lookahead { .. }))
        .count();
    assert_eq!(look, 0, "无 goal 的项目默认 swarm 不该掺任何前瞻局");
    // 计划仍是四廉价策略纯轮换
    for (k, lr) in results.iter().enumerate() {
        assert_eq!(
            lr.spec.strategy_kind,
            StrategyKind::ALL[k % StrategyKind::ALL.len()],
            "无 goal 默认组应是纯轮换"
        );
    }
}
