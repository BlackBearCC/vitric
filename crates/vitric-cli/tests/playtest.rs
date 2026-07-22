//! playtest foundation end-to-end (design draft stage 1): automatically play one session on the
//! real jump project, asserting it can run to max_ticks producing Timeout and a non-empty
//! recording, that the recording passes vitric_sim's replay check, and that two runs of the
//! same (project, strategy, seed, max_ticks) are byte-identical.

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{run_session, GreedyStrategy, RandomStrategy, SessionConfig, Strategy};

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/jump")
}

/// Run one session of jump, return the result (boot on the test side, run_session takes the
/// already-booted pair).
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
    // random will not precisely clear → runs the full 300 ticks → Timeout
    assert_eq!(res.outcome, vitric_playtest::Outcome::Timeout);
    assert_eq!(res.ticks, 300);
    assert_eq!(res.recording.ticks, 300);
    // jump has left/right/space/up input vocabulary; the random strategy injected actions → the
    // recording is non-empty
    assert!(!res.recording.inputs.is_empty(), "录像应记录到随机注入的输入");

    // The recording must pass sim offline replay check (a diverged replay would ReplayDiverged)
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

/// The action vocabulary really derives from jump's input rules (left/right/space/up), not an
/// empty set.
#[test]
fn scene_view_derives_jump_input_vocabulary() {
    use vitric_playtest::{SceneView, TerminalSpec};
    let (sim, rt) = Runtime::boot(&example_dir()).unwrap();
    let view = SceneView::derive(&sim.world, &rt.rules, &TerminalSpec::default());
    let actions: Vec<&str> = view.actions.iter().map(|a| a.action.as_str()).collect();
    for want in ["left", "right", "space", "up"] {
        assert!(actions.contains(&want), "动作词汇应含 {want}，实际 {actions:?}");
    }
    // Each action's pressed/released are both listed
    let pressed = view.actions.iter().filter(|a| a.phase == "pressed").count();
    let released = view.actions.iter().filter(|a| a.phase == "released").count();
    assert_eq!(pressed, released, "pressed/released 配平");
}
