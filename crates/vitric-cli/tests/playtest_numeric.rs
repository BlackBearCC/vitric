//! 数值崩埋雷测试游戏组（设计稿第 4 阶段验收、八节「埋雷测试游戏」）：故意埋了
//! **经济跑飞**和**经济崩盘软锁**的最小模拟经营项目，断言 swarm（含 economy 策略）+
//! 聚合报告的 `numeric_breakage` 维度**逐个逮出**：
//! - runaway → 某资源无界增长：`mint` 每次让 gold ×2、无上限 → 数值跑飞；
//! - collapse → 资源归零软锁：`spend` 把 fuel 减到 0 后再无规则能动状态 → 世界冻结。
//!
//! 这两颗雷靠 economy 策略才逮得稳——它锁定一个动作连按很多次，把单动作的累积效应推到
//! 极端（random/coverage 把动作打散，跑不到跑飞/掏空那一步）。所以这里把 economy 也轮进
//! 策略组。boot 需要 Runtime::boot（住在 vitric-cli），用真 boot 跑真 swarm。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{aggregate, run_swarm, Report, SessionSpec, StrategyKind};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures")
        .join(name)
}

/// 在某埋雷项目上跑一批（四策略 random/greedy/coverage/economy 轮流 × 递增 seed），聚合报告。
/// economy 必须在轮换里——找数值崩的主力就是它（设计稿六节模拟经营）。
fn playtest_report(name: &str, sessions: u64, max_ticks: u64) -> Report {
    let dir = fixture(name);
    let mut plan: Vec<SessionSpec> = Vec::with_capacity(sessions as usize);
    for k in 0..sessions {
        let kind = StrategyKind::ALL[(k as usize) % StrategyKind::ALL.len()];
        plan.push(SessionSpec::new(kind, k, max_ticks));
    }
    let factory = move || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_swarm(factory, &plan, threads).expect("swarm 应跑通");
    aggregate(&results)
}

/// economy 策略真在默认策略组里（轮换 ALL 含它）——找数值崩的前提。
#[test]
fn economy_strategy_is_in_default_rotation() {
    assert!(
        StrategyKind::ALL.iter().any(|k| k.name() == "economy"),
        "默认策略组必须含 economy，否则模拟经营数值崩没人去压"
    );
}

#[test]
fn runaway_economy_is_flagged() {
    // mint 每次让 gold ×2、无上限 → 几十次就跑飞到 ≫1e6
    let rep = playtest_report("runaway", 12, 200);
    assert!(
        !rep.numeric_breakage.runaway.is_empty(),
        "经济跑飞应被逮到，实际 numeric_breakage={:?}",
        rep.numeric_breakage.runaway
    );
    // 跑飞字段必须正是那个无界增长的 gold（按字段名聚类报）
    let gold = rep
        .numeric_breakage
        .runaway
        .iter()
        .find(|r| r.field == "treasury/Resources.gold")
        .expect("跑飞字段应是 treasury/Resources.gold");
    assert!(gold.peak_max > 1e6, "峰值应跑飞到 >1e6，实际 {}", gold.peak_max);
    assert!(gold.hits > 0, "命中局数应 > 0");
    // 每条结论挂可重放录像（结论挂证据）
    let (mut sim, mut rt) = Runtime::boot(&fixture("runaway")).unwrap();
    sim.replay(&gold.sample_recording, &mut rt).expect("跑飞代表录像必须可重放");
}

#[test]
fn collapse_economy_is_flagged() {
    // spend 把 fuel 减到 0 后无规则能动状态 → 归零后世界冻结（崩盘软锁）
    let rep = playtest_report("collapse", 12, 200);
    assert!(
        !rep.numeric_breakage.collapse.is_empty(),
        "经济崩盘软锁应被逮到（collapse 非空），实际 numeric_breakage={:?}",
        rep.numeric_breakage.collapse
    );
    let fuel = rep
        .numeric_breakage
        .collapse
        .iter()
        .find(|c| c.field == "base/Resources.fuel")
        .expect("崩盘字段应是 base/Resources.fuel");
    assert!(fuel.hits > 0, "命中局数应 > 0");
    // 崩盘 = 归零 + 卡死：那一簇软锁也应被独立逮到（两个维度互相印证）
    assert!(!rep.stuck_clusters.is_empty(), "归零后冻结应同时被软锁维度逮到");
    // 可重放
    let (mut sim, mut rt) = Runtime::boot(&fixture("collapse")).unwrap();
    sim.replay(&fuel.sample_recording, &mut rt).expect("崩盘代表录像必须可重放");
}

/// 健康项目不冤判：winnable 没有数值崩字段（runaway/collapse/non_finite 全空）。
#[test]
fn healthy_project_has_no_numeric_breakage() {
    let rep = playtest_report("winnable", 8, 100);
    assert!(rep.numeric_breakage.runaway.is_empty(), "健康项目不该有跑飞: {:?}", rep.numeric_breakage.runaway);
    assert!(rep.numeric_breakage.collapse.is_empty(), "健康项目不该有崩盘");
    assert!(rep.numeric_breakage.non_finite.is_empty(), "健康项目不该有溢出");
}

/// swarm 串/并行在含 economy + 数值遥测的项目上结果逐项一致（确定性铁律——
/// 数值遥测/聚合都进了这条路，也必须一致）。
#[test]
fn swarm_serial_and_parallel_identical_with_numeric_telemetry() {
    let dir = fixture("runaway");
    let mut plan: Vec<SessionSpec> = Vec::new();
    for k in 0..12u64 {
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
        // 数值摘要逐项一致（遥测进了确定性铁律）
        assert_eq!(a.result.numeric_summary, b.result.numeric_summary, "数值摘要逐项一致");
    }
    // 串行/并行聚合出的报告也逐字节一致
    let ja = serde_json::to_string(&aggregate(&serial)).unwrap();
    let jb = serde_json::to_string(&aggregate(&parallel)).unwrap();
    assert_eq!(ja, jb, "数值崩报告串/并行逐字节一致");
}

/// economy 单局可被 CLI 路径选中且能跑（economy 进 --strategy 白名单）。
#[test]
fn economy_single_session_runs_and_records() {
    let dir = fixture("runaway");
    let plan = vec![SessionSpec::new(StrategyKind::Economy, 0, 100)];
    let factory = || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let results = run_swarm(factory, &plan, 1).unwrap();
    let lr = &results[0];
    // economy 锁定 mint 连按 → gold 单调暴涨
    let gold = lr.result.numeric_summary.get("treasury/Resources.gold").expect("应采到 gold");
    assert!(gold.monotonic_up, "economy 连按 mint → gold 只增不减");
    assert!(gold.max > gold.first, "gold 应涨上去");
}
