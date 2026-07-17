//! Seed-based exploration mine-laying acceptance (design draft section 3, section 11 item 3):
//! a puzzle/narrative mine-laying project `branching`, taking a seed recording "to ending-bad"
//! as the starting point, perturbing its input sequence in a controlled way, asserting seed
//! exploration **catches**:
//!   1. Unreachable ending `ending-good` — declared (the go-good rule emits it) but **no
//!      perturbation can reach it**, because its guard `path == 99` never holds (no rule sets
//!      path to 99, a flag bug);
//!   2. Ordering soft-lock — after perturbation scrambles/truncates the required steps, the
//!      world freezes in an unchangeable dead state (Timeout + state frozen).
//!
//! Uses a real `Runtime::boot` to run a real swarm (boot lives in vitric-cli, so the
//! integration test goes here).
//! The seed recording is hand-filled in the test (constructing a `Recording`), with no
//! dependency on external files.

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate_with_endings, perturb_plan, run_seed_swarm, run_session, Outcome, ScriptedStrategy,
    SessionConfig, TerminalSpec,
};
use vitric_sim::{InputRecord, Recording};

fn branching_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures/branching")
}

/// A seed recording reaching ending-bad (the role of the gate certificate): take-key →
/// open-door → go-bad.
/// The inputs are the script that "proves this session can reach ending-bad" — seed
/// exploration takes it as the starting point to perturb.
fn seed_to_bad() -> Recording {
    Recording {
        inputs: vec![
            InputRecord { tick: 1, action: "take-key".into(), phase: "pressed".into() },
            InputRecord { tick: 3, action: "open-door".into(), phase: "pressed".into() },
            InputRecord { tick: 5, action: "go-bad".into(), phase: "pressed".into() },
        ],
        ticks: 20,
        ..Default::default()
    }
}

/// Factory: each thread boots its own runtime + clones the read-only Engine (same QuickJS
/// non-Send constraint).
fn factory(dir: PathBuf) -> impl Fn() -> Result<(vitric_sim::Sim, Runtime, vitric_rules::Engine), String> + Sync {
    move || {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    }
}

#[test]
fn baseline_seed_reaches_ending_bad() {
    // Baseline (un-perturbed seed) should reproduce reaching ending-bad — the control
    // reference for seed exploration
    let dir = branching_dir();
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    let engine = rt.rules.clone();
    let mut strat = ScriptedStrategy::from_inputs(&seed_to_bad().inputs, None);
    let cfg = SessionConfig { max_ticks: 50, seed: 0, terminal: TerminalSpec::default(), ..Default::default() };
    let res = run_session(&mut sim, &mut rt, &engine, &mut strat, &cfg).unwrap();
    assert_eq!(res.outcome, Outcome::Win, "ending-bad 是一个结局（归 Win），基线应到达");
    assert!(
        res.fired_events.contains(&"ending-bad".to_string()),
        "基线应触达 ending-bad，实际事件 {:?}",
        res.fired_events
    );
    assert!(
        !res.fired_events.contains(&"ending-good".to_string()),
        "基线不该（也不可能）触达 ending-good"
    );
}

#[test]
fn seed_exploration_flags_unreachable_good_ending_and_softlock() {
    let dir = branching_dir();
    // Generate N perturbations from the seed (including the baseline), run one session per
    // perturbation
    let plan = perturb_plan(&seed_to_bad(), 40, 12345);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_seed_swarm(
        factory(dir.clone()),
        &plan,
        &[],
        200,
        TerminalSpec::default(),
        777,
        threads,
    )
    .expect("种子探索 swarm 应跑通");

    // Ending coverage: scan the rule-declared endings (go-good emits ending-good, go-bad emits
    // ending-bad), compare which ones were reached during the runs
    let (_, rt) = Runtime::boot(&dir).unwrap();
    let report = aggregate_with_endings(&results, &rt.rules, &TerminalSpec::default());
    let ec = report.ending_coverage.expect("传了引擎应有结局覆盖");

    // 1) Unreachable ending: ending-good is declared but 0 sessions reach it (flag bug: guard
    //    path==99 never holds)
    assert!(
        ec.declared_endings.contains(&"ending-good".to_string()),
        "go-good 规则 emit ending-good，必须算声明结局，实际 declared={:?}",
        ec.declared_endings
    );
    assert!(
        ec.unreachable_endings.contains(&"ending-good".to_string()),
        "任何扰动都到不了 ending-good，必须标不可达，实际 unreachable={:?}",
        ec.unreachable_endings
    );
    // ending-bad is reachable (the baseline reaches it), should not be mis-flagged unreachable
    assert!(
        !ec.unreachable_endings.contains(&"ending-bad".to_string()),
        "ending-bad 基线就到达，不该标不可达"
    );

    // 2) Ordering soft-lock: after perturbation scrambles/truncates the required steps, the
    //    world freezes in a dead state — stuck_clusters is non-empty
    assert!(
        !report.stuck_clusters.is_empty(),
        "扰动应制造出至少一簇顺序软锁（冻结死态），实际 {:?}",
        report.stuck_clusters
    );
    let total_hits: usize = report.stuck_clusters.iter().map(|c| c.hits).sum();
    assert!(total_hits > 0, "软锁命中局数应 > 0");
    // Each cluster carries a replayable recording (conclusion carries evidence)
    for c in &report.stuck_clusters {
        assert!(c.representative.ticks > 0, "卡死簇应带一条可重放代表录像");
    }
}

#[test]
fn seed_exploration_is_deterministic_serial_vs_parallel() {
    // Seed exploration serial/parallel results are item-wise identical (determinism iron law),
    // the same set of perturbations run at two parallelism levels
    let dir = branching_dir();
    let plan = perturb_plan(&seed_to_bad(), 20, 999);
    let serial =
        run_seed_swarm(factory(dir.clone()), &plan, &[], 150, TerminalSpec::default(), 5, 1).unwrap();
    let parallel =
        run_seed_swarm(factory(dir.clone()), &plan, &[], 150, TerminalSpec::default(), 5, 8).unwrap();
    assert_eq!(serial.len(), parallel.len());
    for (a, b) in serial.iter().zip(parallel.iter()) {
        assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
        assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
        assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
        let ja = serde_json::to_string(&a.result.recording).unwrap();
        let jb = serde_json::to_string(&b.result.recording).unwrap();
        assert_eq!(ja, jb, "同扰动串/并行录像逐字节一致");
    }
}
