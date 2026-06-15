//! playtest 地基端到端（设计稿第 1 阶段）：在真实 jump 项目上自动试玩一局，
//! 断言能跑满 max_ticks 出 Timeout + 非空录像 + 录像被 vitric_sim 重放校验通过，
//! 且同 (项目,策略,seed,max_ticks) 两次跑逐字节一致。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{run_session, GreedyStrategy, RandomStrategy, SessionConfig, Strategy};

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/jump")
}

/// 跑一局 jump，返回结果（boot 在测试侧，run_session 接已 boot 的一对）。
fn playtest_jump(strategy_name: &str, seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strategy: Box<dyn Strategy> = match strategy_name {
        "random" => Box::new(RandomStrategy::new(seed)),
        "greedy" => Box::new(GreedyStrategy::new(seed)),
        other => panic!("未知策略 {other}"),
    };
    let cfg = SessionConfig { max_ticks, seed, ..Default::default() };
    run_session(&mut sim, &mut rt, &engine, strategy.as_mut(), &cfg).unwrap()
}

#[test]
fn random_playtest_jump_times_out_with_replayable_recording() {
    let res = playtest_jump("random", 1, 300);
    // random 不会精确通关 → 跑满 300 tick 判 Timeout
    assert_eq!(res.outcome, vitric_playtest::Outcome::Timeout);
    assert_eq!(res.ticks, 300);
    assert_eq!(res.recording.ticks, 300);
    // jump 有 left/right/space/up 输入词汇，随机策略注入过动作 → 录像非空
    assert!(!res.recording.inputs.is_empty(), "录像应记录到随机注入的输入");

    // 录像必须能被 sim 离线重放校验通过（重放跑偏会 ReplayDiverged）
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.replay(&res.recording, &mut rt).expect("playtest 出的录像必须可重放且逐位一致");
}

#[test]
fn greedy_playtest_jump_runs_and_records() {
    let res = playtest_jump("greedy", 7, 300);
    assert_eq!(res.outcome, vitric_playtest::Outcome::Timeout);
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.replay(&res.recording, &mut rt).expect("greedy 录像也必须可重放");
}

#[test]
fn playtest_is_deterministic_byte_for_byte() {
    let a = playtest_jump("random", 42, 250);
    let b = playtest_jump("random", 42, 250);
    assert_eq!(a.outcome, b.outcome);
    assert_eq!(a.ticks, b.ticks);
    let ja = serde_json::to_string(&a.recording).unwrap();
    let jb = serde_json::to_string(&b.recording).unwrap();
    assert_eq!(ja, jb, "同 (项目,策略,seed,max_ticks) 两次跑录像必须逐字节一致");
}

/// 动作词汇真从 jump 的 input 规则派生出来（left/right/space/up），不是空集。
#[test]
fn scene_view_derives_jump_input_vocabulary() {
    use vitric_playtest::{SceneView, TerminalSpec};
    let (sim, rt) = Runtime::boot(&example_dir()).unwrap();
    let view = SceneView::derive(&sim.world, &rt.rules, &TerminalSpec::default());
    let actions: Vec<&str> = view.actions.iter().map(|a| a.action.as_str()).collect();
    for want in ["left", "right", "space", "up"] {
        assert!(actions.contains(&want), "动作词汇应含 {want}，实际 {actions:?}");
    }
    // 每个动作 pressed/released 都列
    let pressed = view.actions.iter().filter(|a| a.phase == "pressed").count();
    let released = view.actions.iter().filter(|a| a.phase == "released").count();
    assert_eq!(pressed, released, "pressed/released 配平");
}
