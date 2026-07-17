//! playtest honors the project manifest's declared clear events (dogfood refinement): the
//! project declares its authoritative clear events in vitric.json's
//! gates.playthroughs[].must_emit; playtest merges them into the TerminalSpec win set when
//! constructing it — so scripted/LLM games (whose win events are not in the generic default
//! set) are not misjudged as "nobody can clear it".
//!
//! fixture `quest`: declares must_emit:"quest-done" (not in the default TerminalSpec), the
//! rule emits quest-done after 5 ticks. Assertions:
//! - default TerminalSpec (without merging the manifest) → this session is judged Timeout
//!   (pre-fix behavior);
//! - after merging the manifest must_emit → the same session is judged Win (post-fix behavior).

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

/// Read the gates.playthroughs[].must_emit list from the manifest (same channel cmd_playtest
/// uses).
fn manifest_must_emit(dir: &PathBuf) -> Vec<String> {
    Project::load(dir)
        .unwrap()
        .manifest
        .gates
        .as_ref()
        .map(|g| g.playthroughs.iter().map(|p| p.must_emit.clone()).collect())
        .unwrap_or_default()
}

/// Run one session of the quest fixture, return the result (given a terminal spec).
fn run_quest(terminal: TerminalSpec, max_ticks: u64) -> vitric_playtest::SessionResult {
    let (mut sim, mut rt) = Runtime::boot(&quest_dir()).unwrap();
    let engine = rt.rules.clone();
    let mut strategy: Box<dyn Strategy> = Box::new(RandomStrategy::new(1));
    let cfg = SessionConfig { max_ticks, seed: 1, terminal, ..Default::default() };
    run_session(&mut sim, &mut rt, &engine, strategy.as_mut(), &cfg).unwrap()
}

#[test]
fn default_terminal_misses_manifest_win_event_times_out() {
    // Pre-fix behavior baseline: quest-done is not in the default win set, even though this
    // session emitted quest-done it cannot be judged Win → Timeout.
    let res = run_quest(TerminalSpec::default(), 100);
    assert!(res.fired_events.contains(&"quest-done".to_string()), "规则确实发了 quest-done");
    assert_eq!(res.outcome, Outcome::Timeout, "默认集合不认 quest-done");
}

#[test]
fn merged_terminal_judges_manifest_win_event_as_win() {
    // Post-fix behavior: merge the manifest must_emit into the win set, the same session emits
    // quest-done and is judged Win.
    let dir = quest_dir();
    let must_emit = manifest_must_emit(&dir);
    assert_eq!(must_emit, vec!["quest-done".to_string()], "清单声明 quest-done");
    let terminal = TerminalSpec::default().with_manifest_must_emit(&must_emit);
    let res = run_quest(terminal, 100);
    assert_eq!(res.outcome, Outcome::Win, "并进清单 must_emit 后判 Win");
}

#[test]
fn manifest_declared_endings_show_in_coverage() {
    // Ending coverage: the manifest-declared quest-done merges into the declared endings set,
    // and this batch of results reaches it → reached is non-empty, unreachable is empty.
    let dir = quest_dir();
    let must_emit = manifest_must_emit(&dir);
    let terminal = TerminalSpec::default().with_manifest_must_emit(&must_emit);
    let res = run_quest(terminal.clone(), 100);
    let spec = SessionSpec::new(StrategyKind::Random, 1, 100);
    let labeled = vec![LabeledResult { spec, result: res }];

    let (_, rt) = Runtime::boot(&dir).unwrap();
    // Without merging the manifest + default terminal (does not recognize quest-done as a
    // terminal): the rule's quest-done emit is not classified as an ending, the declared
    // endings set is empty — this is how scripted/LLM game win events get missed.
    let plain = aggregate_with_endings(&labeled, &rt.rules, &TerminalSpec::default());
    let plain_ec = plain.ending_coverage.expect("传引擎应算 coverage");
    assert!(plain_ec.declared_endings.is_empty(), "默认 terminal 不认 quest-done 为结局");

    // With the manifest merged: the declared endings set contains quest-done, and this session
    // reached it → reached contains quest-done, unreachable is empty.
    let merged = aggregate_with_endings_and_declared(&labeled, &rt.rules, &terminal, &must_emit);
    let ec = merged.ending_coverage.expect("传引擎应算 coverage");
    assert_eq!(ec.declared_endings, vec!["quest-done".to_string()]);
    assert_eq!(ec.reached_endings, vec!["quest-done".to_string()]);
    assert!(ec.unreachable_endings.is_empty(), "触达了就不该在不可达里");
}
