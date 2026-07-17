//! Mine-laying test game group: minimal Vitric projects with known defects deliberately laid,
//! asserting the swarm + aggregated report **catches each one** (design draft stage 2
//! acceptance, section 8 "mine-laying test games").
//!
//! Four mines each correspond to a report dimension:
//! - winnable    → win rate > 0 and unbeatable_by_swarm=false (a winnable game is not falsely
//!   accused);
//! - unbeatable  → unbeatable_by_swarm=true and win rate 0 (declared winnable but the condition
//!   never holds);
//! - softlock    → stuck_clusters non-empty, hit count > 0 (some action pushes the world into a
//!   frozen dead state);
//! - dead-action → a dead action appears in inert_actions (an input is declared but no rule
//!   handles it with a response).
//!
//! boot needs Runtime::boot (lives in vitric-cli), so the integration test goes here, using
//! real boot to run a real swarm.
//! The factory closure is constructed on the test side and passed to run_swarm — each worker
//! thread boots its own runtime (QuickJS is not Send, the runtime never crosses threads), and
//! only the plain-data SessionResult is returned.

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate, aggregate_with_endings, run_swarm, Report, SessionSpec, StrategyKind, TerminalSpec,
};

/// Mine-laying project directory (lives under vitric-playtest's tests/fixtures).
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures")
        .join(name)
}

/// Repository-root example game directory (examples/ is at the workspace root; this test crate
/// is under crates/vitric-cli).
fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

/// Run a batch on a project directory (three strategies rotated × incrementing seed to make up
/// `sessions` sessions), aggregate into a report.
/// `with_engine=true` goes through `aggregate_with_endings` (the CLI default path,
/// inert_actions uses the static criterion); false goes through bare `aggregate`
/// (inert_actions degrades to the old runtime heuristic).
fn playtest_report_at(dir: PathBuf, sessions: u64, max_ticks: u64, with_engine: bool) -> Report {
    // Plan: random+greedy+coverage rotated, seed 0..sessions
    let mut plan: Vec<SessionSpec> = Vec::with_capacity(sessions as usize);
    for k in 0..sessions {
        let kind = StrategyKind::ALL[(k as usize) % StrategyKind::ALL.len()];
        plan.push(SessionSpec::new(kind, k, max_ticks));
    }
    // Factory: each thread boots its own fresh runtime + clones the read-only Engine
    let factory = {
        let dir = dir.clone();
        move || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        }
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_swarm(factory, &plan, threads).expect("swarm 应跑通");
    if with_engine {
        let (_, rt) = Runtime::boot(&dir).expect("boot 引擎");
        aggregate_with_endings(&results, &rt.rules, &TerminalSpec::default())
    } else {
        aggregate(&results)
    }
}

/// Run a batch on a mine-laying project (bare aggregate path, reuses the old test cases).
fn playtest_report(name: &str, sessions: u64, max_ticks: u64) -> Report {
    playtest_report_at(fixture(name), sessions, max_ticks, false)
}

#[test]
fn winnable_is_beaten_by_swarm() {
    // Pressing "key" emits game-won immediately; random/coverage clears within a few ticks
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
    // The win rule is blocked by a never-satisfied condition (gate.Lock.open==true, nobody sets
    // it)
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
    // "seal" pushes the world into the sealed=true frozen dead state; after that any input
    // cannot change state, and no terminal is emitted
    let rep = playtest_report("softlock", 9, 200);
    assert!(
        !rep.stuck_clusters.is_empty(),
        "softlock 应被聚成至少一簇卡死候选，实际 {:?}",
        rep.stuck_clusters
    );
    let total_hits: usize = rep.stuck_clusters.iter().map(|c| c.hits).sum();
    assert!(total_hits > 0, "命中局数应 > 0");
    // Each cluster carries a replayable recording (conclusion carries evidence)
    for c in &rep.stuck_clusters {
        assert_eq!(c.representative.ticks, 200, "代表录像应跑满 max_ticks（卡死=超时）");
    }
}

#[test]
fn dead_action_is_inert() {
    // "useless" is the only declared input action; the rule sets Scratch.v to its original
    // initial value 0 (a self-noop), producing no real effect. Under the bare aggregate
    // (runtime heuristic) path it should be judged inert.
    let rep = playtest_report("dead-action", 9, 100);
    assert!(
        rep.inert_actions.contains(&"useless".to_string()),
        "声明了输入但没引发响应的废动作应出现在 inert_actions（运行时路径），实际 {:?}",
        rep.inert_actions
    );
}

#[test]
fn dead_action_is_inert_static_path() {
    // Regression guard: after switching to the static criterion
    // (aggregate_with_endings = CLI default path), "useless" is still judged inert — it sets
    // the field to its initial value, a self-noop with no real effect.
    let rep = playtest_report_at(fixture("dead-action"), 9, 100, true);
    assert!(
        rep.inert_actions.contains(&"useless".to_string()),
        "静态判据下 useless（set 成初值的自我无效操作）仍应判惰性，实际 {:?}",
        rep.inert_actions
    );
}

#[test]
fn jump_movement_keys_not_inert() {
    // dogfood-fixed false positive: jump's movement keys left/right/space/up only set Velocity
    // (change state) without emitting events; the old runtime criterion would falsely flag
    // them inert. The static criterion (CLI default path) judges by "set to a new value ≠
    // initial value = has real effect", so these keys should no longer enter inert_actions.
    let rep = playtest_report_at(example("jump"), 12, 120, true);
    for key in ["left", "right", "space", "up"] {
        assert!(
            !rep.inert_actions.contains(&key.to_string()),
            "移动键 {key} 改了 Velocity（真实效果），不该被冤标惰性，实际 inert={:?}",
            rep.inert_actions
        );
    }
}

/// swarm serial vs parallel results are item-wise identical (determinism iron law), verified
/// on a real-boot mine-laying project.
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
        // Recording byte-identical (hard evidence of determinism)
        let ja = serde_json::to_string(&a.result.recording).unwrap();
        let jb = serde_json::to_string(&b.result.recording).unwrap();
        assert_eq!(ja, jb, "同 spec 串/并行录像逐字节一致");
    }
}

/// The clear recording really passes sim's offline replay check (each conclusion carries a
/// replayable recording = truly replayable).
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
    // Offline recording replay: a divergence would raise ReplayDiverged
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.replay(&lr.result.recording, &mut rt).expect("通关录像必须可重放且逐位一致");
}
