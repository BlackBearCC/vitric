//! Numeric collapse mine-laying test game group (design draft stage 4 acceptance, section 8
//! "mine-laying test games"): a minimal simulation-management project that deliberately lays
//! **economy runaway** and **economy collapse soft-lock** mines, asserting the swarm (incl. the
//! economy strategy) + the aggregated report's `numeric_breakage` dimension **catches each
//! one**:
//! - runaway → some resource grows unboundedly: `mint` doubles gold each time with no upper
//!   bound → numeric runaway;
//! - collapse → resource hits zero soft-lock: `spend` reduces fuel to 0 and then no rule can
//!   move state → the world freezes.
//!
//! These two mines are caught reliably only by the economy strategy — it locks one action and
//! presses it many times, pushing the cumulative effect of a single action to the extreme
//! (random/coverage spread actions out and never reach the runaway/empty-out step). So economy
//! is rotated into the strategy group here. boot needs Runtime::boot (lives in vitric-cli),
//! so we use real boot to run a real swarm.

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{aggregate, run_swarm, Report, SessionSpec, StrategyKind};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures")
        .join(name)
}

/// Run a batch on a mine-laying project (four strategies random/greedy/coverage/economy rotated
/// × incrementing seed), aggregate the report.
/// economy must be in the rotation — it is the main force for catching numeric collapse (design
/// draft section 6 simulation management).
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

/// The economy strategy is really in the default strategy group (rotation ALL contains it) — a
/// prerequisite for catching numeric collapse.
#[test]
fn economy_strategy_is_in_default_rotation() {
    assert!(
        StrategyKind::ALL.iter().any(|k| k.name() == "economy"),
        "默认策略组必须含 economy，否则模拟经营数值崩没人去压"
    );
}

#[test]
fn runaway_economy_is_flagged() {
    // mint doubles gold each time with no upper bound → after dozens of presses it runs away
    // to ≫1e6
    let rep = playtest_report("runaway", 12, 200);
    assert!(
        !rep.numeric_breakage.runaway.is_empty(),
        "经济跑飞应被逮到，实际 numeric_breakage={:?}",
        rep.numeric_breakage.runaway
    );
    // The runaway field must be exactly that unboundedly growing gold (reported clustered by
    // field name)
    let gold = rep
        .numeric_breakage
        .runaway
        .iter()
        .find(|r| r.field == "treasury/Resources.gold")
        .expect("跑飞字段应是 treasury/Resources.gold");
    assert!(gold.peak_max > 1e6, "峰值应跑飞到 >1e6，实际 {}", gold.peak_max);
    assert!(gold.hits > 0, "命中局数应 > 0");
    // Each conclusion carries a replayable recording (conclusion carries evidence)
    let (mut sim, mut rt) = Runtime::boot(&fixture("runaway")).unwrap();
    sim.replay(&gold.representative.recording, &mut rt).expect("跑飞代表录像必须可重放");
}

#[test]
fn collapse_economy_is_flagged() {
    // spend reduces fuel to 0 then no rule can move state → after hitting zero the world
    // freezes (collapse soft-lock)
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
    // Collapse = zeroed out + frozen: that cluster's soft-lock should also be independently
    // caught (the two dimensions corroborate each other)
    assert!(!rep.stuck_clusters.is_empty(), "归零后冻结应同时被软锁维度逮到");
    // Replayable
    let (mut sim, mut rt) = Runtime::boot(&fixture("collapse")).unwrap();
    sim.replay(&fuel.representative.recording, &mut rt).expect("崩盘代表录像必须可重放");
}

/// A healthy project is not falsely accused: winnable has no numeric breakage fields
/// (runaway/collapse/non_finite all empty).
#[test]
fn healthy_project_has_no_numeric_breakage() {
    let rep = playtest_report("winnable", 8, 100);
    assert!(rep.numeric_breakage.runaway.is_empty(), "健康项目不该有跑飞: {:?}", rep.numeric_breakage.runaway);
    assert!(rep.numeric_breakage.collapse.is_empty(), "健康项目不该有崩盘");
    assert!(rep.numeric_breakage.non_finite.is_empty(), "健康项目不该有溢出");
}

/// swarm serial/parallel results are item-wise identical on a project with economy + numeric
/// telemetry (the determinism iron law — numeric telemetry/aggregation all go down this path
/// and must also be identical).
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
        // Numeric summary item-wise identical (telemetry enters the determinism iron law)
        assert_eq!(a.result.numeric_summary, b.result.numeric_summary, "数值摘要逐项一致");
    }
    // The reports aggregated from serial/parallel are also byte-identical
    let ja = serde_json::to_string(&aggregate(&serial)).unwrap();
    let jb = serde_json::to_string(&aggregate(&parallel)).unwrap();
    assert_eq!(ja, jb, "数值崩报告串/并行逐字节一致");
}

/// A single economy session can be selected via the CLI path and runs (economy enters the
/// --strategy whitelist).
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
    // economy locks mint and presses it repeatedly → gold grows monotonically
    let gold = lr.result.numeric_summary.get("treasury/Resources.gold").expect("应采到 gold");
    assert!(gold.monotonic_up, "economy 连按 mint → gold 只增不减");
    assert!(gold.max > gold.first, "gold 应涨上去");
}
