//! 埋雷测试游戏组：故意埋了已知缺陷的最小 Vitric 项目，断言 swarm + 聚合报告
//! **逐个逮出**（设计稿第 2 阶段验收、八节「埋雷测试游戏」）。
//!
//! 四颗雷各对应一个报告维度：
//! - winnable    → 通关率 > 0 且 unbeatable_by_swarm=false（能赢的游戏不冤判）；
//! - unbeatable  → unbeatable_by_swarm=true 且通关率 0（声明能赢但条件永不满足）；
//! - softlock    → stuck_clusters 非空、命中局数 > 0（某动作把世界推进冻结死态）；
//! - dead-action → 废动作出现在 inert_actions（声明了输入但 rules 没人接出响应）。
//!
//! boot 需要 Runtime::boot（住在 vitric-cli），所以集成测试放这儿，用真 boot 跑真 swarm。
//! 工厂闭包在测试侧 new 出来传给 run_swarm——每个工作线程自己 boot 一份运行时
//! （QuickJS 非 Send，运行时绝不跨线程），跑完只回传 plain-data 的 SessionResult。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate, run_swarm, Report, SessionSpec, StrategyKind,
};

/// 埋雷项目目录（住在 vitric-playtest 的 tests/fixtures 下）。
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures")
        .join(name)
}

/// 在某埋雷项目上跑一批（三策略轮流 × 递增 seed 凑够 sessions 局），聚合成报告。
fn playtest_report(name: &str, sessions: u64, max_ticks: u64) -> Report {
    let dir = fixture(name);
    // 计划：random+greedy+coverage 轮流，seed 0..sessions
    let mut plan: Vec<SessionSpec> = Vec::with_capacity(sessions as usize);
    for k in 0..sessions {
        let kind = StrategyKind::ALL[(k as usize) % StrategyKind::ALL.len()];
        plan.push(SessionSpec::new(kind, k, max_ticks));
    }
    // 工厂：每线程自己 boot 一份全新运行时 + 复制只读 Engine
    let factory = move || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_swarm(factory, &plan, threads).expect("swarm 应跑通");
    aggregate(&results)
}

#[test]
fn winnable_is_beaten_by_swarm() {
    // 按 "key" 即发 game-won，random/coverage 几 tick 内就能通关
    let rep = playtest_report("winnable", 9, 100);
    assert!(
        rep.outcome_distribution.win_rate > 0.0,
        "可通关游戏应被逮到能赢，实际分布 {:?}",
        rep.outcome_distribution
    );
    assert!(!rep.reachability.unbeatable_by_swarm, "能赢就不该标 unbeatable");
    assert!(rep.outcome_distribution.win > 0);
}

#[test]
fn unbeatable_is_flagged() {
    // win 规则被永不满足的条件（gate.Lock.open==true，无人 set）挡死
    let rep = playtest_report("unbeatable", 9, 100);
    assert!(
        rep.reachability.unbeatable_by_swarm,
        "声明了 win 但任何输入都触发不到 → 必须标 unbeatable_by_swarm"
    );
    assert_eq!(rep.outcome_distribution.win, 0, "一局都赢不了");
    assert_eq!(rep.outcome_distribution.win_rate, 0.0);
}

#[test]
fn softlock_is_clustered() {
    // "seal" 把世界推进 sealed=true 的冻结死态，此后任何输入都改不动状态、也没发终止
    let rep = playtest_report("softlock", 9, 200);
    assert!(
        !rep.stuck_clusters.is_empty(),
        "softlock 应被聚成至少一簇卡死候选，实际 {:?}",
        rep.stuck_clusters
    );
    let total_hits: usize = rep.stuck_clusters.iter().map(|c| c.hits).sum();
    assert!(total_hits > 0, "命中局数应 > 0");
    // 每簇都挂得到一条可重放录像（结论挂证据）
    for c in &rep.stuck_clusters {
        assert_eq!(c.sample_recording.ticks, 200, "代表录像应跑满 max_ticks（卡死=超时）");
    }
}

#[test]
fn dead_action_is_inert() {
    // "useless" 是唯一声明的输入动作，规则只 set 一个没人读的字段、不 emit 任何事件
    let rep = playtest_report("dead-action", 9, 100);
    assert!(
        rep.inert_actions.contains(&"useless".to_string()),
        "声明了输入但没引发响应的废动作应出现在 inert_actions，实际 {:?}",
        rep.inert_actions
    );
}

/// swarm 串行 vs 并行结果逐项一致（确定性铁律），在真 boot 的埋雷项目上验。
#[test]
fn swarm_serial_and_parallel_identical_on_fixture() {
    let dir = fixture("softlock");
    let mut plan: Vec<SessionSpec> = Vec::new();
    for k in 0..9u64 {
        let kind = StrategyKind::ALL[(k as usize) % StrategyKind::ALL.len()];
        plan.push(SessionSpec::new(kind, k, 150));
    }
    let factory = || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let serial = run_swarm(factory, &plan, 1).expect("串行");
    let parallel = run_swarm(factory, &plan, 8).expect("并行");
    assert_eq!(serial.len(), parallel.len());
    for (a, b) in serial.iter().zip(parallel.iter()) {
        assert_eq!(a.spec, b.spec, "spec 顺序一致");
        assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
        assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
        assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
        assert_eq!(a.result.fired_events, b.result.fired_events, "事件集一致");
        // 录像逐字节一致（确定性的硬证据）
        let ja = serde_json::to_string(&a.result.recording).unwrap();
        let jb = serde_json::to_string(&b.result.recording).unwrap();
        assert_eq!(ja, jb, "同 spec 串/并行录像逐字节一致");
    }
}

/// 通关录像真能被 sim 离线重放校验通过（每条结论挂可重放录像 = 真可重放）。
#[test]
fn winnable_recording_replays() {
    let dir = fixture("winnable");
    let plan = vec![SessionSpec::new(StrategyKind::Coverage, 0, 100)];
    let factory = || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let results = run_swarm(factory, &plan, 1).unwrap();
    let lr = &results[0];
    assert_eq!(lr.result.outcome, vitric_playtest::Outcome::Win, "coverage 应通关 winnable");
    // 录像离线重放：跑偏会 ReplayDiverged
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.replay(&lr.result.recording, &mut rt).expect("通关录像必须可重放且逐位一致");
}
