//! 第 6 阶段验收：每游戏 playtest.json 覆盖 + 派生量让 greedy 真有方向。
//!
//! guided 是个 1D 引导平台：hero 在 x=0，exit 在 x=8，按 right 朝出口走、到 x≥8 发 game-won。
//! 它的 playtest.json 声明了一个 `distance` 派生量 `to_exit`（hero→exit 曼哈顿距离）+ `goal:min`。
//!
//! 断言（证明派生量真让 greedy 有方向，不是随机乱晃）：
//! - greedy（接 goal）在更小 max_ticks 内通关，random 同条件超时 → greedy 更快接近目标；
//! - observation.derived 含声明的 to_exit 距离值，且随 hero 移动而变化；
//! - 有 goal 的 greedy 行为与无 goal（默认 config）不同。

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

/// 加载 guided 项目根的 playtest.json（声明派生量 to_exit + goal:min）。
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

/// 跑一种策略 N 局（同 max_ticks、同起始 seed 递增），返回通关局数 + 最快通关 tick。
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
    // greedy 接 config.goal（朝 to_exit min 走）；random 同 config 但不看目标。
    // 给一个对 random 偏紧的 max_ticks：greedy 有方向能在内通关，random 大概率超时。
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
    // 最快通关 tick 应明显小于 max_ticks（greedy 直奔出口，不是踩点超时）
    let fastest = greedy_fastest.expect("greedy 有通关局");
    assert!(fastest < max_ticks, "greedy 最快通关 {fastest} tick 应 < {max_ticks}");
}

#[test]
fn observation_carries_declared_distance_and_it_changes_with_movement() {
    let config = guided_config();
    let (mut sim, mut rt) = Runtime::boot(&guided_dir()).unwrap();
    let engine = rt.rules.clone();
    let terminal = TerminalSpec::default();

    // 初始：hero(0,0) exit(8,0) → 曼哈顿距离 8
    let view0 = SceneView::derive_with_config(&sim.world, &engine, &terminal, &config);
    let d0 = view0
        .observation
        .get("derived")
        .and_then(|d| d.get("to_exit"))
        .and_then(|v| v.as_f64())
        .expect("observation.derived.to_exit 应存在");
    assert!((d0 - 8.0).abs() < 1e-9, "初始距离应为 8，实际 {d0}");

    // 朝出口走若干 tick，距离应变小（派生量随移动变化，不是静态常量）
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
    // 同一个 guided 项目：有 goal（playtest.json）vs 无 goal（默认 config），
    // greedy 的录像应不同——证明 goal 真改变了行为（而非巧合一致）。
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
    // 经二进制跑 softlock swarm（有卡死簇 → 有代表录像），落代表录像到临时 report-dir。
    // 断言：stdout 报告主体不内联录像（无 checkpoints），report-dir 里有单独 json，引用挂相对路径。
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

    // 报告主体不内联录像（checkpoints 是录像独有的大块字段）
    assert!(!stdout.contains("checkpoints"), "报告主体不该内联录像: {stdout}");
    let clusters = report["stuck_clusters"].as_array().unwrap();
    assert!(!clusters.is_empty(), "softlock 应聚出卡死簇");
    let repr = &clusters[0]["representative"];
    let rel = repr["path"].as_str().expect("落盘后代表录像应挂相对路径");
    assert!(rel.ends_with(".json"), "路径是 json 文件: {rel}");
    // report-dir 里那份文件真存在且能解析回录像
    let full = report_dir.join(rel);
    assert!(full.exists(), "代表录像文件应真写出: {}", full.display());
    let rec: vitric_sim::Recording =
        serde_json::from_str(&std::fs::read_to_string(&full).unwrap()).unwrap();
    assert!(rec.ticks > 0, "落盘录像应是真录像");

    let _ = std::fs::remove_dir_all(&report_dir);
}

#[test]
fn cli_without_playtest_json_uses_default_config() {
    // softlock 没有 playtest.json → 默认 config（自动推视图、greedy 无目标退化随机），
    // 报告照样产出（向后兼容：无配置文件不改变行为）。
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
    // 带 config 的 swarm 也满足确定性铁律：串行/并行逐项一致（含派生量视图、greedy 接 goal）。
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
