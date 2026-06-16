//! playtest 认项目清单声明的通关事件（dogfood 精化）：项目在 vitric.json 的
//! gates.playthroughs[].must_emit 里声明了自己的权威通关事件，playtest 构造
//! TerminalSpec 时把它并进 win 集合——脚本/LLM 游戏（胜利事件不在通用默认集里）
//! 才不会被误判"谁也通不了"。
//!
//! fixture `quest`：声明 must_emit:"quest-done"（不在默认 TerminalSpec 里），
//! 规则在 5 tick 后 emit quest-done。断言：
//! - 默认 TerminalSpec（不并清单）→ 这局判 Timeout（修前行为）；
//! - 并进清单 must_emit 后 → 同一局判 Win（修后行为）。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_data::Project;
use vitric_playtest::{
    aggregate_with_endings, aggregate_with_endings_and_declared, run_session, LabeledResult,
    Outcome, RandomStrategy, SessionConfig, SessionSpec, Strategy, StrategyKind, TerminalSpec,
};

fn quest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quest")
}

/// 从清单读 gates.playthroughs[].must_emit 列表（cmd_playtest 用的同一口径）。
fn manifest_must_emit(dir: &PathBuf) -> Vec<String> {
    Project::load(dir)
        .unwrap()
        .manifest
        .gates
        .as_ref()
        .map(|g| g.playthroughs.iter().map(|p| p.must_emit.clone()).collect())
        .unwrap_or_default()
}

/// 跑一局 quest fixture，返回结果（给定 terminal 规格）。
fn run_quest(terminal: TerminalSpec, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (mut sim, mut rt) = Runtime::boot(&quest_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strategy: Box<dyn Strategy> = Box::new(RandomStrategy::new(1));
    let cfg = SessionConfig { max_ticks, seed: 1, terminal, ..Default::default() };
    run_session(&mut sim, &mut rt, &engine, strategy.as_mut(), &cfg).unwrap()
}

#[test]
fn default_terminal_misses_manifest_win_event_times_out() {
    // 修前行为基线：quest-done 不在默认 win 集，这局 emit 了 quest-done 也判不出 Win → Timeout。
    let res = run_quest(TerminalSpec::default(), 100);
    assert!(res.fired_events.contains(&"quest-done".to_string()), "规则确实发了 quest-done");
    assert_eq!(res.outcome, Outcome::Timeout, "默认集合不认 quest-done");
}

#[test]
fn merged_terminal_judges_manifest_win_event_as_win() {
    // 修后行为：把清单 must_emit 并进 win 集合，同一局 emit quest-done 即判 Win。
    let dir = quest_dir();
    let must_emit = manifest_must_emit(&dir);
    assert_eq!(must_emit, vec!["quest-done".to_string()], "清单声明 quest-done");
    let terminal = TerminalSpec::default().with_manifest_must_emit(&must_emit);
    let res = run_quest(terminal, 100);
    assert_eq!(res.outcome, Outcome::Win, "并进清单 must_emit 后判 Win");
}

#[test]
fn manifest_declared_endings_show_in_coverage() {
    // 结局覆盖：清单声明的 quest-done 并进声明结局集，且这批结果里触达 → reached 非空、不可达为空。
    let dir = quest_dir();
    let must_emit = manifest_must_emit(&dir);
    let terminal = TerminalSpec::default().with_manifest_must_emit(&must_emit);
    let res = run_quest(terminal.clone(), 100);
    let spec = SessionSpec::new(StrategyKind::Random, 1, 100);
    let labeled = vec![LabeledResult { spec, result: res }];

    let (_, rt) = Runtime::boot(&dir).unwrap();
    // 不并清单 + 默认 terminal（不认 quest-done 为终止）：规则的 quest-done emit 不被
    // 归为结局，声明结局集空——脚本/LLM 游戏的胜利事件就是这样被漏掉的。
    let plain = aggregate_with_endings(&labeled, &rt.rules, &TerminalSpec::default());
    let plain_ec = plain.ending_coverage.expect("传引擎应算 coverage");
    assert!(plain_ec.declared_endings.is_empty(), "默认 terminal 不认 quest-done 为结局");

    // 并清单：声明结局集含 quest-done，且这局触达了它 → reached 含 quest-done、不可达为空。
    let merged = aggregate_with_endings_and_declared(&labeled, &rt.rules, &terminal, &must_emit);
    let ec = merged.ending_coverage.expect("传引擎应算 coverage");
    assert_eq!(ec.declared_endings, vec!["quest-done".to_string()]);
    assert_eq!(ec.reached_endings, vec!["quest-done".to_string()]);
    assert!(ec.unreachable_endings.is_empty(), "触达了就不该在不可达里");
}
