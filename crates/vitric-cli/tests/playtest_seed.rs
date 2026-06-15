//! 种子式探索埋雷验收（设计稿三节、十一节第 3 条）：一个解谜/剧情埋雷项目
//! `branching`，拿一条「到 ending-bad」的种子录像当起点，受控扰动它的输入序列，
//! 断言种子探索**逮出**：
//!   1. 不可达结局 `ending-good`——声明了（go-good 规则 emit 它）但**任何扰动都到不了**，
//!      因为它的守卫 `path == 99` 永不成立（没有任何规则把 path 设成 99，flag bug）；
//!   2. 顺序软锁——扰动把必经步骤打乱/截断后，世界冻在一个改不动的死态（Timeout + 状态冻结）。
//!
//! 用真 `Runtime::boot` 跑真 swarm（boot 住在 vitric-cli，所以集成测试放这儿）。
//! 种子录像在测试里手填（构造 `Recording`），不依赖外部文件。

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

/// 一条到 ending-bad 的种子录像（gate 证书的角色）：take-key → open-door → go-bad。
/// inputs 即「证明这局能通到 ending-bad」的脚本——种子探索拿它当起点扰动。
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

/// 工厂：每线程自己 boot 一份运行时 + 复制只读 Engine（QuickJS 非 Send 同款约束）。
fn factory(dir: PathBuf) -> impl Fn() -> Result<(vitric_sim::Sim, Runtime, vitric_rules::Engine), String> + Sync {
    move || {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    }
}

#[test]
fn baseline_seed_reaches_ending_bad() {
    // 基线（未扰动种子）应复现到 ending-bad——种子探索的对照基准
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
    // 从种子生成 N 条扰动（含基线），每条跑一局
    let plan = perturb_plan(&seed_to_bad(), 40, 12345);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_seed_swarm(
        factory(dir.clone()),
        &plan,
        200,
        TerminalSpec::default(),
        777,
        threads,
    )
    .expect("种子探索 swarm 应跑通");

    // 结局覆盖：扫规则声明的结局（go-good emit ending-good、go-bad emit ending-bad），
    // 比对运行里到达了哪些
    let (_, rt) = Runtime::boot(&dir).unwrap();
    let report = aggregate_with_endings(&results, &rt.rules, &TerminalSpec::default());
    let ec = report.ending_coverage.expect("传了引擎应有结局覆盖");

    // 1) 不可达结局：ending-good 声明了但 0 局可达（flag bug：守卫 path==99 永不成立）
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
    // ending-bad 是可达的（基线就到了），不该误标不可达
    assert!(
        !ec.unreachable_endings.contains(&"ending-bad".to_string()),
        "ending-bad 基线就到达，不该标不可达"
    );

    // 2) 顺序软锁：扰动把必经步骤打乱/截断后，世界冻在死态——stuck_clusters 非空
    assert!(
        !report.stuck_clusters.is_empty(),
        "扰动应制造出至少一簇顺序软锁（冻结死态），实际 {:?}",
        report.stuck_clusters
    );
    let total_hits: usize = report.stuck_clusters.iter().map(|c| c.hits).sum();
    assert!(total_hits > 0, "软锁命中局数应 > 0");
    // 每簇挂得到可重放录像（结论挂证据）
    for c in &report.stuck_clusters {
        assert!(c.representative.ticks > 0, "卡死簇应带一条可重放代表录像");
    }
}

#[test]
fn seed_exploration_is_deterministic_serial_vs_parallel() {
    // 种子探索串行/并行结果逐项一致（确定性铁律），同一组扰动两种并行度跑
    let dir = branching_dir();
    let plan = perturb_plan(&seed_to_bad(), 20, 999);
    let serial =
        run_seed_swarm(factory(dir.clone()), &plan, 150, TerminalSpec::default(), 5, 1).unwrap();
    let parallel =
        run_seed_swarm(factory(dir.clone()), &plan, 150, TerminalSpec::default(), 5, 8).unwrap();
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
