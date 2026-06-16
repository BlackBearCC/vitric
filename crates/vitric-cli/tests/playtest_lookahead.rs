//! 前瞻搜索（run_session_lookahead）端到端：在真实 nav 导航项目上证明「技巧类游戏能玩了」。
//!
//! nav 关卡：hero 从 x=0 出发，x=5 处有堵 3 高的墙挡路，必须先跳上墙顶再继续往右，
//! 走到 x≥9.5 触发 reached-exit 通关。随机策略乱按几乎不可能凑齐「贴墙→起跳→越过→右行」
//! 这串操作 → 超时；前瞻搜索每真 tick 投机 horizon 帧、按 to_exit 距离打分，能算出该跳该走 → 通关。
//!
//! 硬证据三条：
//!  1. 同条件下 lookahead 通关（Win）而 random 超时（Timeout）；
//!  2. lookahead 那局手工攒的录像被 Sim::replay 逐位复现（证明录像对、投机没污染）；
//!  3. 同 (项目,seed,horizon) 两次 lookahead 的 outcome/ticks/录像逐字节一致。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate_with_endings_and_declared, default_plan, run_session, run_session_lookahead,
    run_swarm_with_config, LookaheadConfig, Outcome, PlaytestConfig, RandomStrategy, SessionConfig,
    StrategyKind, Strategy, TerminalSpec,
};

fn nav_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vitric-playtest/tests/fixtures/nav")
}

/// 仓库根的示例游戏目录（examples/ 在 workspace 根，本测试 crate 在 crates/vitric-cli 下）。
fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

/// 加载 nav 的 playtest.json（含派生量 to_exit + goal:min + terminal win_events:["reached-exit"]）。
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

fn run_lookahead(seed: u64, max_ticks: u64, horizon: u64) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    run_session_lookahead(&mut sim, &mut rt, &engine, &cfg, &LookaheadConfig { horizon }).unwrap()
}

fn run_random(seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strat: Box<dyn Strategy> = Box::new(RandomStrategy::new(seed));
    run_session(&mut sim, &mut rt, &engine, strat.as_mut(), &cfg).unwrap()
}

/// 硬证据 1：lookahead 通关，random 同条件超时。
#[test]
fn lookahead_wins_nav_where_random_times_out() {
    let look = run_lookahead(0, 600, 24);
    assert_eq!(look.outcome, Outcome::Win, "前瞻搜索应通关 nav（越墙到出口）");
    assert!(look.ticks < 600, "通关应早于超时上限，实际 {} tick", look.ticks);

    let rand = run_random(0, 600);
    assert_eq!(rand.outcome, Outcome::Timeout, "随机策略凑不齐越墙序列 → 超时");
}

/// 硬证据 2：lookahead 手工攒的录像被 Sim::replay 逐位复现。
#[test]
fn lookahead_nav_recording_replays_bit_for_bit() {
    let look = run_lookahead(0, 600, 24);
    assert_eq!(look.outcome, Outcome::Win);
    // 越墙必有输入（贴墙/起跳/右行），录像非空
    assert!(!look.recording.inputs.is_empty(), "通关录像应记录到注入的输入");
    assert!(look.recording.checkpoints.first().map(|c| c.0) == Some(0), "起点 checkpoint tick=0");

    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    sim.replay(&look.recording, &mut rt)
        .expect("前瞻搜索手工攒的录像必须可重放且逐位一致");
    assert_eq!(sim.world.state_hash(), look.recording.final_hash);
}

/// 硬证据 3：同 (项目,seed,horizon) 两次 lookahead 逐字节一致。
#[test]
fn lookahead_nav_is_deterministic_byte_for_byte() {
    let a = run_lookahead(0, 600, 24);
    let b = run_lookahead(0, 600, 24);
    assert_eq!(a.outcome, b.outcome);
    assert_eq!(a.ticks, b.ticks);
    let ja = serde_json::to_string(&a.recording).unwrap();
    let jb = serde_json::to_string(&b.recording).unwrap();
    assert_eq!(ja, jb, "同 (项目,seed,horizon) 两次前瞻录像必须逐字节一致");
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
/// （nav 声明了 goal，default_plan 自动把末尾 2 局换成前瞻；前瞻 horizon=24 能算出跳墙序列。）
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
