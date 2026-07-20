//! Task 8 (Tech Tree) integration tests.
//!
//! Verifies: (a) research-progress system advances Research.progress by ctx.dt each tick;
//! (b) research completes after enough ticks elapse (known/has_*/current reset + researched event);
//! (c) start_research deducts TechPoints via the tp-set -> tp-apply rule pipeline;
//! (d) start_research rejects when TechPoints are insufficient (toast-show emitted, no state change).
//!
//! Test setup follows the seasons.rs pattern: `Runtime::boot(frontier_dir())` loads the full
//! Frontier scene + logic. The event-driven approach (inject ui-activate reply matching the
//! pick-tech-<id> rule) exercises the full rule -> script -> rule pipeline.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

/// Helper: set a single field on a named entity.
fn set_field(sim: &mut vitric_sim::Sim, name: &str, path: &str, value: serde_json::Value) {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.set_field(id, path, value).expect("set_field ok");
}

/// Helper: read a single field from a named entity as a cloned Value.
fn get_field(sim: &vitric_sim::Sim, name: &str, path: &str) -> serde_json::Value {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.get_field(id, path).expect("get_field ok").clone()
}

#[test]
fn research_progress_advances_with_dt() {
    // Boot, give 5 TechPoints, trigger pick-tech-survival_t1 via ui-activate, step 1 tick.
    // After the step: Research.current = "survival_t1" and Research.progress > 0
    // (research-progress system ran once with dt = 1/60).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "player", "TechPoint.value", json!(5));

    sim.inject_reply("ui-activate", json!({ "action": "pick-tech-survival_t1" }));
    sim.step(&mut rt).unwrap();

    let current = get_field(&sim, "colony", "Research.current");
    assert_eq!(current.as_str(), Some("survival_t1"),
        "Research.current should be set to survival_t1 after start_research");

    let progress = get_field(&sim, "colony", "Research.progress");
    let progress = progress.as_f64().expect("progress is a number");
    assert!(progress > 0.0,
        "Research.progress should be > 0 after one tick of research-progress system, got {progress}");
}

#[test]
fn research_completes_after_time() {
    // Bypass start_research: set Research.current + a tiny cost_total (0.1s = 6 ticks at 60Hz)
    // directly on the colony. Step until progress >= cost_total. Verify completion:
    // known contains the id, has_survival_t1 == 1, current reset to "", researched event emitted.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Research.current", json!("survival_t1"));
    set_field(&mut sim, "colony", "Research.cost_total", json!(0.1));

    let mut saw_researched = false;
    // 6 ticks = 0.1s; step 8 to be safe (completion fires on tick 6, verify state after).
    for _ in 0..8 {
        sim.step(&mut rt).unwrap();
        let observed = rt.drain_observed();
        if observed.iter().any(|e| e.name == "researched"
            && e.data.get("id").and_then(|v| v.as_str()) == Some("survival_t1"))
        {
            saw_researched = true;
        }
    }

    assert!(saw_researched, "researched event for survival_t1 should have been emitted");

    let current = get_field(&sim, "colony", "Research.current");
    assert_eq!(current.as_str(), Some(""),
        "Research.current should be reset to empty after completion");

    let known = get_field(&sim, "colony", "Research.known");
    let known_str = known.as_str().expect("known is a text field");
    assert!(known_str.contains("survival_t1"),
        "Research.known should contain survival_t1, got {known_str}");

    let has = get_field(&sim, "colony", "Research.has_survival_t1");
    assert_eq!(has.as_i64(), Some(1),
        "Research.has_survival_t1 should be 1 after completion");
}

#[test]
fn start_research_deducts_techpoints() {
    // Player has 5 TechPoints. Trigger start_research for survival_t1 (cost 2).
    // Step 1: start_research fn runs, emits tp-set{value: 3}, sets Research.current.
    // Step 2: tp-apply rule consumes tp-set, writes TechPoint.value = 3.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "player", "TechPoint.value", json!(5));

    sim.inject_reply("ui-activate", json!({ "action": "pick-tech-survival_t1" }));
    sim.step(&mut rt).unwrap(); // start_research runs, tp-set emitted to carryover
    sim.step(&mut rt).unwrap(); // tp-apply rule fires, TechPoint.value written back

    let tp = get_field(&sim, "player", "TechPoint.value");
    assert_eq!(tp.as_i64(), Some(3),
        "TechPoint.value should be 5 - 2 = 3 after start_research, got {tp}");
}

#[test]
fn start_research_rejects_insufficient_techpoints() {
    // Player has 1 TechPoint. survival_t1 costs 2. start_research should reject:
    // emit toast-show{ text: "科技点不足" }, NOT emit tp-set, NOT set Research.current.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "player", "TechPoint.value", json!(1));

    sim.inject_reply("ui-activate", json!({ "action": "pick-tech-survival_t1" }));
    sim.step(&mut rt).unwrap();

    let observed = rt.drain_observed();
    let rejected = observed.iter().any(|e| {
        e.name == "toast-show"
            && e.data.get("text").and_then(|v| v.as_str()).map(|s| s.contains("科技点不足")).unwrap_or(false)
    });
    assert!(rejected,
        "should emit toast-show with '科技点不足', observed: {:?}",
        observed.iter().map(|e| (e.name.as_str(), e.data.clone())).collect::<Vec<_>>());

    let tp = get_field(&sim, "player", "TechPoint.value");
    assert_eq!(tp.as_i64(), Some(1),
        "TechPoint.value should remain 1 (no deduction on reject), got {tp}");

    let current = get_field(&sim, "colony", "Research.current");
    assert_eq!(current.as_str(), Some(""),
        "Research.current should remain empty on reject, got {current}");
}
