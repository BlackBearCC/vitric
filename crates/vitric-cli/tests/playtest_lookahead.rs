//! 前瞻搜索（run_session_lookahead）端到端：在真实 nav 导航项目上证明「技巧类游戏能玩了」。
//!
//! nav 关卡：hero 从 x=0 出发，x=5 处有堵 3 高的墙挡路，必须先跳上墙顶再继续往右，
//! 走到 x≥9.5 触发 reached-exit 通关。随机策略乱按几乎不可能凑齐「贴墙→起跳→越过→右行」
//! 这串操作 → 超时；前瞻搜索每真 tick 投机 horizon 帧、按 to_exit 距离打分，能算出该跳该走 → 通关。
//!
//! 硬证据三条：
//!  1. 同条件下 lookahead 通关（Win）而 random 超时（Timeout）；
//!  2. lookahead 那局手工攒的录像被 Sim::replay 逐位复现（证明录像对、投机没污染）；
//!  3. 同 (项目,seed,horizon) 两次 lookahead 的 outcome/ticks/录像逐字节一致。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    run_session, run_session_lookahead, LookaheadConfig, Outcome, PlaytestConfig, RandomStrategy,
    SessionConfig, Strategy, TerminalSpec,
};

fn nav_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vitric-playtest/tests/fixtures/nav")
}

/// 加载 nav 的 playtest.json（含派生量 to_exit + goal:min + terminal win_events:["reached-exit"]）。
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

fn run_lookahead(seed: u64, max_ticks: u64, horizon: u64) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    run_session_lookahead(&mut sim, &mut rt, &engine, &cfg, &LookaheadConfig { horizon }).unwrap()
}

fn run_random(seed: u64, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (_, cfg) = nav_cfg(seed, max_ticks);
    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strat: Box<dyn Strategy> = Box::new(RandomStrategy::new(seed));
    run_session(&mut sim, &mut rt, &engine, strat.as_mut(), &cfg).unwrap()
}

/// 硬证据 1：lookahead 通关，random 同条件超时。
#[test]
fn lookahead_wins_nav_where_random_times_out() {
    let look = run_lookahead(0, 600, 24);
    assert_eq!(look.outcome, Outcome::Win, "前瞻搜索应通关 nav（越墙到出口）");
    assert!(look.ticks < 600, "通关应早于超时上限，实际 {} tick", look.ticks);

    let rand = run_random(0, 600);
    assert_eq!(rand.outcome, Outcome::Timeout, "随机策略凑不齐越墙序列 → 超时");
}

/// 硬证据 2：lookahead 手工攒的录像被 Sim::replay 逐位复现。
#[test]
fn lookahead_nav_recording_replays_bit_for_bit() {
    let look = run_lookahead(0, 600, 24);
    assert_eq!(look.outcome, Outcome::Win);
    // 越墙必有输入（贴墙/起跳/右行），录像非空
    assert!(!look.recording.inputs.is_empty(), "通关录像应记录到注入的输入");
    assert!(look.recording.checkpoints.first().map(|c| c.0) == Some(0), "起点 checkpoint tick=0");

    let (mut sim, mut rt) = Runtime::boot(&nav_dir()).unwrap();
    sim.replay(&look.recording, &mut rt)
        .expect("前瞻搜索手工攒的录像必须可重放且逐位一致");
    assert_eq!(sim.world.state_hash(), look.recording.final_hash);
}

/// 硬证据 3：同 (项目,seed,horizon) 两次 lookahead 逐字节一致。
#[test]
fn lookahead_nav_is_deterministic_byte_for_byte() {
    let a = run_lookahead(0, 600, 24);
    let b = run_lookahead(0, 600, 24);
    assert_eq!(a.outcome, b.outcome);
    assert_eq!(a.ticks, b.ticks);
    let ja = serde_json::to_string(&a.recording).unwrap();
    let jb = serde_json::to_string(&b.recording).unwrap();
    assert_eq!(ja, jb, "同 (项目,seed,horizon) 两次前瞻录像必须逐字节一致");
}
