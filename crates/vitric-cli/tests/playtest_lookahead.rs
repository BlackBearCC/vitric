//! Beam-search rolling planner (run_session_lookahead) end-to-end: prove on the real nav
//! navigation project that "the multi-tick maneuver skill games need can be planned out".
//!
//! nav level: hero starts at x=0; at x=5 there is a wall in the way (Collider h=2, wall top y=1
//! higher than hero's foot y=0 when standing on ground); you must **first jump over the wall,
//! then keep going right**, reaching x≥9.5 to trigger reached-exit and clear. playtest.json
//! declares the goal as "maximize hero's x coordinate" (push right) — the jump frame does not
//! increase x (it even looks like "wasting" a frame in place), so **single-step lookahead
//! (depth=1) will only greedily keep pressing right, hit the wall and get stuck**; only beam
//! search with enough depth can compute the return string "jump once, clear the wall, x grows
//! again", and thus clear.
//!
//! Hard evidence:
//!  1. **Same level, same planner, depth=1 (degenerate single-step lookahead) times out,
//!     depth≥8 beam search clears** — proving planning depth really brought capability (this is
//!     the core evidence of this "single-step → beam search" upgrade);
//!  2. Under the same conditions lookahead clears (Win) while random times out (Timeout);
//!  3. The recording hand-built by that lookahead session is bit-reproduced by Sim::replay
//!     (proving the recording is correct, speculation did not pollute);
//!  4. Two lookahead runs of the same (project, seed, depth, beam) are byte-identical in
//!     outcome/ticks/recording.

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate_with_endings_and_declared, default_plan, run_session, run_session_lookahead,
    run_swarm_with_config, LookaheadConfig, Outcome, PlaytestConfig, RandomStrategy, SessionConfig,
    StrategyKind, Strategy, TerminalSpec,
};

/// Beam-search depth used in this test: nav wall-clearing maneuver measured depth≥8 stable clear
/// (minimum about 8), take 12 with margin.
const NAV_DEPTH: u64 = 12;
/// Beam width used in this test.
const NAV_BEAM: usize = 4;

fn nav_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vitric-playtest/tests/fixtures/nav")
}

/// Repository-root example game directory (examples/ is at the workspace root; this test crate is
/// under crates/vitric-cli).
fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

/// Load nav's playtest.json (includes derived quantity hero_x + goal:max + terminal
/// win_events:["reached-exit"]).
fn nav_cfg(seed: u64, max_ticks: u64) -> (PlaytestConfig, SessionConfig) {
    let config = PlaytestConfig::load(&nav_dir()).unwrap().expect("nav 有 playtest.json");
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    };
    let cfg = SessionConfig {
        max_ticks,
        seed,
        terminal,
        playtest: config.clone(),
        ..Default::default()
    };
    (config, cfg)
}

/// Run one beam-search lookahead session (explicitly specify depth/beam, for hard-evidence
/// depth=1 vs deep search comparison).
fn run_lookahead_db(seed: u64, max_ticks: u64, depth: u64, beam: usize) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    run_session_lookahead(&mut sim, &mut rt, &engine, &cfg, &LookaheadConfig { depth, beam_width: beam }).unwrap()
}

/// Lookahead with default depth/beam width (most hard evidence uses this).
fn run_lookahead(seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    run_lookahead_db(seed, max_ticks, NAV_DEPTH, NAV_BEAM)
}

fn run_random(seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strat: Box<dyn Strategy> = Box::new(RandomStrategy::new(seed));
    run_session(&mut sim, &mut rt, &engine, strat.as_mut(), &cfg).unwrap()
}

/// Hard evidence 1 (**core of this upgrade**): same level, same planner, **depth=1 degenerate
/// single-step lookahead times out, depth≥8 beam search clears** — planning depth really brought
/// the capability to "solve multi-tick maneuvers".
///
/// Why depth=1 cannot solve it: nav's goal is "maximize hero_x". In front of the wall,
/// single-step lookahead only looks 1 frame per candidate — the jump frame does not increase x
/// (looks like wasting a frame), pressing right is blocked by the wall so x does not change, no
/// single-frame action can improve hero_x → greedy degrades to keeping pressing right, hitting
/// the wall and getting stuck → timeout. Beam search with enough depth can look down to the
/// return string "jump up → clear the wall top → go right again, hero_x grows again", so in
/// front of the wall it chooses to jump → clears. This is exactly the capability single-step
/// lookahead (the old implementation) lacked and beam search has.
#[test]
fn lookahead_depth1_times_out_but_beam_search_wins() {
    // depth=1: degenerates to single-step lookahead (each candidate only looks ahead 1 frame).
    // Hits the wall and gets stuck → timeout.
    let shallow = run_lookahead_db(0, 300, 1, NAV_BEAM);
    assert_eq!(
        shallow.outcome,
        Outcome::Timeout,
        "depth=1 单步前瞻应在墙前卡死超时，实际 {:?} @ {} tick",
        shallow.outcome,
        shallow.ticks
    );
    // depth≥8 beam search: plans out "first jump over the wall, then go right" → clears.
    let deep = run_lookahead_db(0, 300, NAV_DEPTH, NAV_BEAM);
    assert_eq!(deep.outcome, Outcome::Win, "深度束搜索应通关 nav（跳过墙到出口）");
    assert!(deep.ticks < shallow.ticks, "深搜通关 tick({}) 应远少于单步超时 tick({})", deep.ticks, shallow.ticks);
    // The clear path **really has a jump** — multi-tick maneuver (jump + sustained right) really
    // emerged, not fluked.
    assert!(
        deep.recording.inputs.iter().any(|i| i.action == "space"),
        "深搜通关录像里应记录到起跳（space），证明跳过墙的机动被规划出来：{:?}",
        deep.recording.inputs
    );
}

/// Hard evidence 2: lookahead clears, random times out under the same conditions.
#[test]
fn lookahead_wins_nav_where_random_times_out() {
    let look = run_lookahead(0, 600);
    assert_eq!(look.outcome, Outcome::Win, "束搜索应通关 nav（越墙到出口）");
    assert!(look.ticks < 600, "通关应早于超时上限，实际 {} tick", look.ticks);

    let rand = run_random(0, 600);
    assert_eq!(rand.outcome, Outcome::Timeout, "随机策略凑不齐越墙序列 → 超时");
}

/// Hard evidence 3: lookahead's hand-built recording is bit-reproduced by Sim::replay.
#[test]
fn lookahead_nav_recording_replays_bit_for_bit() {
    let look = run_lookahead(0, 600);
    assert_eq!(look.outcome, Outcome::Win);
    // Wall-clearing must have inputs (jump/right), the recording is non-empty
    assert!(!look.recording.inputs.is_empty(), "通关录像应记录到注入的输入");
    assert!(look.recording.checkpoints.first().map(|c| c.0) == Some(0), "起点 checkpoint tick=0");

    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    sim.replay(&look.recording, &mut rt)
        .expect("束搜索手工攒的录像必须可重放且逐位一致");
    assert_eq!(sim.world.state_hash(), look.recording.final_hash);
}

/// Hard evidence 4: two lookahead runs of the same (project, seed, depth, beam) are
/// byte-identical.
#[test]
fn lookahead_nav_is_deterministic_byte_for_byte() {
    let a = run_lookahead(0, 600);
    let b = run_lookahead(0, 600);
    assert_eq!(a.outcome, b.outcome);
    assert_eq!(a.ticks, b.ticks);
    let ja = serde_json::to_string(&a.recording).unwrap();
    let jb = serde_json::to_string(&b.recording).unwrap();
    assert_eq!(ja, jb, "同 (项目,seed,depth,beam) 两次束搜索录像必须逐字节一致");
}

// ---- Default swarm auto-mixes lookahead: skill/navigation games that declare goal are no longer
// misreported as unbeatable ----
//
// It goes through the same path as the CLI cmd_playtest default swarm: default_plan (mixes
// lookahead when goal is declared) + run_swarm_with_config (dispatches by spec: Lookahead goes to
// run_session_lookahead, the rest go to run_session) +
// aggregate_with_endings_and_declared (CLI default aggregation). Here we do not run the CLI via
// subprocess, but directly reuse the same library functions for more precise assertions (can see
// per-strategy split, proving the lookahead session really cleared it).

/// Run the default swarm (default_plan + run_swarm_with_config) on a project directory, aggregate
/// the report + raw results.
fn default_swarm(dir: PathBuf, sessions: u64, max_ticks: u64) -> (vitric_playtest::Report, Vec<vitric_playtest::LabeledResult>) {
    let config = PlaytestConfig::load(&dir).unwrap().unwrap_or_default();
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    };
    // Same as CLI: mix lookahead when goal is declared, otherwise pure rotation
    let plan = default_plan(sessions, 0, max_ticks, terminal.clone(), config.goal.is_some());
    let factory = {
        let dir = dir.clone();
        move || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        }
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_swarm_with_config(&factory, &plan, &config, threads).expect("默认 swarm 应跑通");
    let (_, rt) = Runtime::boot(&dir).expect("boot 引擎");
    let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &[]);
    (report, results)
}

/// Hard evidence: nav, which declares a goal, has a default swarm win rate > 0 and is not
/// reported unbeatable — the lookahead session cleared it.
/// (nav declares a goal, default_plan automatically swaps the last 2 sessions to beam-search
/// lookahead; the default swarm depth is enough to compute the wall-jump sequence.)
#[test]
fn nav_default_swarm_is_not_unbeatable_thanks_to_lookahead() {
    let (report, results) = default_swarm(nav_dir(), 8, 600);

    // 1) Default swarm win rate > 0, not reported unbeatable (before the fix, with no lookahead
    // tier, greedy also could not clear, it would be misreported).
    assert!(
        report.outcome_distribution.win_rate > 0.0,
        "声明 goal 的 nav 默认 swarm 应有通关，实际分布 {:?}",
        report.outcome_distribution
    );
    assert!(!report.reachability.unbeatable_by_swarm, "能赢就不该标 unbeatable");

    // 2) The plan really mixed in lookahead sessions (8 sessions → 2 sessions), proving
    // "declare goal → auto-mix lookahead" took effect.
    let look: Vec<_> = results
        .iter()
        .filter(|lr| matches!(lr.spec.strategy_kind, StrategyKind::Lookahead { .. }))
        .collect();
    assert_eq!(look.len(), 2, "8 局默认 swarm 应掺 2 局前瞻，实际 {}", look.len());

    // 3) The lookahead session **really cleared** (not fluked by another strategy) — lookahead is
    // one of the key tiers that plays nav through.
    assert!(
        look.iter().any(|lr| lr.result.outcome == Outcome::Win),
        "前瞻局应至少有一局通关 nav（越墙到出口），实际 {:?}",
        look.iter().map(|lr| lr.result.outcome).collect::<Vec<_>>()
    );
    // The lookahead session carries full telemetry (same as a normal session): state_trace length
    // = tick count, has a serializable recording.
    for lr in &look {
        assert_eq!(lr.result.state_trace.len(), lr.result.ticks as usize, "前瞻局 state_trace 同口径");
        assert!(!lr.result.recording.checkpoints.is_empty(), "前瞻局应有录像");
    }
}

/// Backward-compat hard evidence: examples/jump has no playtest.json goal, the default swarm
// mixes in zero lookahead sessions, still pure rotation of random/greedy/coverage/economy
// (behavior unchanged from before the fix).
#[test]
fn jump_no_goal_default_swarm_has_no_lookahead() {
    let dir = example("jump");
    // jump has no playtest.json goal
    let config = PlaytestConfig::load(&dir).unwrap().unwrap_or_default();
    assert!(config.goal.is_none(), "jump 不该有 goal（向后兼容前提）");

    let (_report, results) = default_swarm(dir, 8, 120);
    let look = results
        .iter()
        .filter(|lr| matches!(lr.spec.strategy_kind, StrategyKind::Lookahead { .. }))
        .count();
    assert_eq!(look, 0, "无 goal 的项目默认 swarm 不该掺任何前瞻局");
    // The plan is still pure rotation of the four cheap strategies
    for (k, lr) in results.iter().enumerate() {
        assert_eq!(
            lr.spec.strategy_kind,
            StrategyKind::ALL[k % StrategyKind::ALL.len()],
            "无 goal 默认组应是纯轮换"
        );
    }
}
