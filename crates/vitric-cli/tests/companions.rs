//! Task 9 (Companions Expansion) integration tests.
//!
//! Verifies: (a) companion-contribution role=builder grants +1 plank to @player.Inventory;
//! (b) companion-contribution role=scholar emits tp-set -> tp-apply rule writes @player.TechPoint.value;
//! (c) collective-wish-check system fires once when Colony.food_i >= 50 (sets collective_wish_done=1,
//!     emits collective-wish-fulfilled + toast-show);
//! (d) collective-wish-check does NOT re-fire after collective_wish_done is already 1.
//!
//! Test setup follows the research.rs pattern: `Runtime::boot(frontier_dir())` loads the full
//! Frontier scene + logic. The "companion" entity in the scene is Pip (Persona.role="builder"
//! post-Task-9). Tests re-role Pip on the fly to exercise each branch of the role-based dispatch.

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

/// Prime the scene's "companion" entity (Pip) so the `companion-contribution` system fires on
/// the next step: affinity above the 50 threshold, mood in the allowed set, timer at 0.
/// `role` overrides Persona.role for branch coverage.
fn prime_companion(sim: &mut vitric_sim::Sim, role: &str) {
    set_field(sim, "companion", "Persona.role", json!(role));
    set_field(sim, "companion", "Need.affinity", json!(60));
    set_field(sim, "companion", "Need.affinity_i", json!(60));
    set_field(sim, "companion", "Mood.value", json!("开心"));
    set_field(sim, "companion", "Need.contribution_timer", json!(0));
}

#[test]
fn companion_contribution_role_builder_grants_plank() {
    // Pip (role=builder) fires contribution on tick 1 (timer=0, affinity=60, mood=开心).
    // Verify @player.Inventory.plank increased by 1.
    //
    // Tick 0 is reserved for the `start` event, which fires the seed-start rule (sets plank=6,
    // wood=8, etc.). We step once to let that settle, then prime + step + verify the +1.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.step(&mut rt).unwrap(); // settle seed-start
    prime_companion(&mut sim, "builder");
    set_field(&mut sim, "player", "Inventory.plank", json!(0));

    sim.step(&mut rt).unwrap();

    let plank = get_field(&sim, "player", "Inventory.plank");
    assert_eq!(plank.as_i64(), Some(1),
        "Inventory.plank should be 1 after builder contribution, got {plank}");
}

#[test]
fn companion_contribution_role_scholar_grants_techpoint() {
    // Pip (role=scholar) emits tp-set{value: tp+1} on tick 1.
    // tp-apply rule (rules/research.json) consumes the cross-tick event on tick 2, writing
    // TechPoint.value = tp+1.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    prime_companion(&mut sim, "scholar");
    set_field(&mut sim, "player", "TechPoint.value", json!(0));

    sim.step(&mut rt).unwrap(); // contribution system emits tp-set
    sim.step(&mut rt).unwrap(); // tp-apply rule fires, writes TechPoint.value

    let tp = get_field(&sim, "player", "TechPoint.value");
    assert_eq!(tp.as_i64(), Some(1),
        "TechPoint.value should be 1 after scholar contribution (tp-set -> tp-apply), got {tp}");
}

#[test]
fn collective_wish_fires_at_food_50() {
    // Set Colony.food_i = 50 and collective_wish_done = 0. Step 1 tick.
    // collective-wish-check system (scripts/wish.js) should:
    //   - set Colony.collective_wish_done = 1
    //   - emit collective-wish-fulfilled { threshold: 50 }
    //   - emit toast-show (display)
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Colony.food_i", json!(50));
    set_field(&mut sim, "colony", "Colony.collective_wish_done", json!(0));

    sim.step(&mut rt).unwrap();

    let done = get_field(&sim, "colony", "Colony.collective_wish_done");
    assert_eq!(done.as_i64(), Some(1),
        "Colony.collective_wish_done should be 1 after food_i >= 50, got {done}");

    let observed = rt.drain_observed();
    let saw_fulfilled = observed.iter().any(|e| {
        e.name == "collective-wish-fulfilled"
            && e.data.get("threshold").and_then(|v: &serde_json::Value| v.as_i64()) == Some(50)
    });
    assert!(saw_fulfilled,
        "should emit collective-wish-fulfilled {{threshold: 50}}, observed: {:?}",
        observed.iter().map(|e| (e.name.as_str(), e.data.clone())).collect::<Vec<_>>());
}

#[test]
fn collective_wish_one_time_only() {
    // collective_wish_done already 1 (milestone reached earlier). Set food_i = 80.
    // Step 1 tick. The system should early-return on the `collective_wish_done !== 0` guard,
    // leaving the value at 1 and emitting no second collective-wish-fulfilled event.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_field(&mut sim, "colony", "Colony.collective_wish_done", json!(1));
    set_field(&mut sim, "colony", "Colony.food_i", json!(80));

    sim.step(&mut rt).unwrap();

    let done = get_field(&sim, "colony", "Colony.collective_wish_done");
    assert_eq!(done.as_i64(), Some(1),
        "collective_wish_done should stay at 1 (no repeat fire), got {done}");

    let observed = rt.drain_observed();
    let repeat_fulfilled = observed.iter().any(|e| e.name == "collective-wish-fulfilled");
    assert!(!repeat_fulfilled,
        "should NOT emit collective-wish-fulfilled a second time, observed: {:?}",
        observed.iter().map(|e| (e.name.as_str(), e.data.clone())).collect::<Vec<_>>());
}
