//! Stage 6 acceptance: per-game playtest.json coverage + derived quantities give greedy a real
//! direction.
//!
//! guided is a 1D guided platform: hero at x=0, exit at x=8; press right toward the exit; at
//! x≥8 emit game-won.
//! Its playtest.json declares a `distance` derived quantity `to_exit` (hero→exit Manhattan
//! distance) + `goal:min`.
//!
//! Assertions (proving the derived quantity really gives greedy a direction, not random
//! wandering):
//! - greedy (taking goal) clears within a smaller max_ticks, random times out under the same
//!   conditions → greedy approaches the goal faster;
//! - observation.derived contains the declared to_exit distance value, and it changes as hero
//!   moves;
//! - greedy with goal behaves differently from greedy without goal (default config).

use std::path::PathBuf;
use std::process::Command;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    run_swarm_with_config, PlaytestConfig, SceneView, SessionSpec, StrategyKind, TerminalSpec,
};

fn guided_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures/guided")
}

/// Load guided project root's playtest.json (declares the derived quantity to_exit + goal:min).
fn guided_config() -> PlaytestConfig {
    PlaytestConfig::load(&guided_dir())
        .expect("playtest.json 应解析成功")
        .expect("guided 应有 playtest.json")
}

fn factory_for(dir: PathBuf) -> impl Fn() -> Result<(vitric_sim::Sim, Runtime, vitric_rules::Engine), String> + Sync {
    move || {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    }
}

/// Run N sessions of one strategy (same max_ticks, incrementing start seed), return the win count
/// + fastest win tick.
fn run_kind(
    kind: StrategyKind,
    config: &PlaytestConfig,
    sessions: u64,
    max_ticks: u64,
) -> (usize, Option<u64>) {
    let plan: Vec<SessionSpec> = (0..sessions)
        .map(|k| SessionSpec::new(kind, k, max_ticks))
        .collect();
    let factory = factory_for(guided_dir());
    let results = run_swarm_with_config(factory, &plan, config, 1).expect("swarm 应跑通");
    let wins: Vec<u64> = results
        .iter()
        .filter(|lr| lr.result.outcome == vitric_playtest::Outcome::Win)
        .map(|lr| lr.result.ticks)
        .collect();
    (wins.len(), wins.iter().copied().min())
}

#[test]
fn greedy_with_goal_reaches_exit_faster_than_random() {
    let config = guided_config();
    // greedy takes config.goal (walks toward to_exit min); random uses the same config but does
    // not look at the goal.
    // Give a max_ticks that is tight for random: greedy has direction and can clear within it,
    // random mostly times out.
    let max_ticks = 200;
    let (greedy_wins, greedy_fastest) = run_kind(StrategyKind::Greedy, &config, 6, max_ticks);
    let (random_wins, _) = run_kind(StrategyKind::Random, &config, 6, max_ticks);

    assert!(
        greedy_wins > 0,
        "带 goal 的 greedy 应能通关（朝出口走），实际通关 {greedy_wins} 局"
    );
    assert!(
        greedy_wins > random_wins,
        "带 goal 的 greedy 通关数应多于 random（更快接近目标）：greedy={greedy_wins} random={random_wins}"
    );
    // The fastest win tick should be clearly smaller than max_ticks (greedy heads straight to the
    // exit, not edge-case timeout)
    let fastest = greedy_fastest.expect("greedy 有通关局");
    assert!(fastest < max_ticks, "greedy 最快通关 {fastest} tick 应 < {max_ticks}");
}

#[test]
fn observation_carries_declared_distance_and_it_changes_with_movement() {
    let config = guided_config();
    let (mut sim, mut rt) = Runtime::boot(&guided_dir()).unwrap();
    let engine = rt.rules.clone();
    let terminal = TerminalSpec::default();

    // Initial: hero(0,0) exit(8,0) → Manhattan distance 8
    let view0 = SceneView::derive_with_config(&sim.world, &engine, &terminal, &config);
    let d0 = view0
        .observation
        .get("derived")
        .and_then(|d| d.get("to_exit"))
        .and_then(|v| v.as_f64())
        .expect("observation.derived.to_exit 应存在");
    assert!((d0 - 8.0).abs() < 1e-9, "初始距离应为 8，实际 {d0}");

    // Walk toward the exit for several ticks, the distance should shrink (the derived quantity
    // changes with movement, not a static constant)
    sim.inject_input("right", "pressed");
    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }
    let view1 = SceneView::derive_with_config(&sim.world, &engine, &terminal, &config);
    let d1 = view1
        .observation
        .get("derived")
        .and_then(|d| d.get("to_exit"))
        .and_then(|v| v.as_f64())
        .expect("to_exit 仍应存在");
    assert!(d1 < d0, "朝出口走后距离应变小：{d0} → {d1}");
}

#[test]
fn greedy_behaves_differently_with_and_without_goal() {
    // Same guided project: with goal (playtest.json) vs without goal (default config),
    // greedy's recording should differ — proving goal really changed behavior (not coincidental
    // identity).
    let with_goal = guided_config();
    let no_goal = PlaytestConfig::default();
    let plan = vec![SessionSpec::new(StrategyKind::Greedy, 0, 200)];

    let r_with = run_swarm_with_config(factory_for(guided_dir()), &plan, &with_goal, 1).unwrap();
    let r_without = run_swarm_with_config(factory_for(guided_dir()), &plan, &no_goal, 1).unwrap();

    let j_with = serde_json::to_string(&r_with[0].result.recording).unwrap();
    let j_without = serde_json::to_string(&r_without[0].result.recording).unwrap();
    assert_ne!(j_with, j_without, "有 goal 的 greedy 录像必须和无 goal 不同");
}

fn softlock_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures/softlock")
}

#[test]
fn cli_report_dir_externalizes_recordings_and_keeps_body_clean() {
    // Run the softlock swarm via the binary (has stuck clusters → has representative recordings),
    // drop representative recordings into a temp report-dir.
    // Assertions: the stdout report body does not inline recordings (no checkpoints), the
    // report-dir has separate json files, references hang on relative paths.
    let report_dir = std::env::temp_dir()
        .join(format!("vitric-cli-report-dir-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&report_dir);

    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .args([
            "playtest",
            softlock_dir().to_str().unwrap(),
            "--sessions",
            "9",
            "--seed",
            "0",
            "--max-ticks",
            "200",
            "--report-dir",
            report_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "playtest 应成功: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("stdout 应是干净 JSON");

    // Report body does not inline recordings (checkpoints is the large block unique to recordings)
    assert!(!stdout.contains("checkpoints"), "报告主体不该内联录像: {stdout}");
    let clusters = report["stuck_clusters"].as_array().unwrap();
    assert!(!clusters.is_empty(), "softlock 应聚出卡死簇");
    let repr = &clusters[0]["representative"];
    let rel = repr["path"].as_str().expect("落盘后代表录像应挂相对路径");
    assert!(rel.ends_with(".json"), "路径是 json 文件: {rel}");
    // The file in report-dir really exists and parses back to a recording
    let full = report_dir.join(rel);
    assert!(full.exists(), "代表录像文件应真写出: {}", full.display());
    let rec: vitric_sim::Recording =
        serde_json::from_str(&std::fs::read_to_string(&full).unwrap()).unwrap();
    assert!(rec.ticks > 0, "落盘录像应是真录像");

    let _ = std::fs::remove_dir_all(&report_dir);
}

#[test]
fn cli_without_playtest_json_uses_default_config() {
    // softlock has no playtest.json → default config (auto-derive view, greedy without target
    // degrades to random); the report is still produced (backward compatible: no config file
    // does not change behavior).
    let report_dir = std::env::temp_dir()
        .join(format!("vitric-cli-nocfg-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&report_dir);
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .args([
            "playtest",
            softlock_dir().to_str().unwrap(),
            "--sessions",
            "4",
            "--report-dir",
            report_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "无 playtest.json 也应跑通: {}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(report["sessions"].as_u64().unwrap(), 4);
    let _ = std::fs::remove_dir_all(&report_dir);
}

#[test]
fn guided_swarm_serial_and_parallel_identical_with_config() {
    // A swarm with config also satisfies the determinism iron law: serial/parallel identical item
    // by item (including derived-quantity view, greedy taking goal).
    let config = guided_config();
    let plan: Vec<SessionSpec> = (0..6)
        .map(|k| SessionSpec::new(StrategyKind::Greedy, k, 200))
        .collect();
    let serial = run_swarm_with_config(factory_for(guided_dir()), &plan, &config, 1).unwrap();
    let parallel = run_swarm_with_config(factory_for(guided_dir()), &plan, &config, 8).unwrap();
    assert_eq!(serial.len(), parallel.len());
    for (a, b) in serial.iter().zip(parallel.iter()) {
        assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
        assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
        let ja = serde_json::to_string(&a.result.recording).unwrap();
        let jb = serde_json::to_string(&b.result.recording).unwrap();
        assert_eq!(ja, jb, "带 config 串/并行录像逐字节一致");
    }
}
