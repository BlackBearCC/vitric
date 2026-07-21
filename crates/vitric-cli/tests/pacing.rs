//! Task 14 (Pacing Rebalance) integration tests.
//!
//! Verifies the new compound-condition `stage` system in colony.js:
//! (a) 立足 fires at day 12 when survival_t1 is researched AND struct_count >= 5;
//! (b) 立足 does NOT advance at day 12 if survival_t1 is missing (tech is a hard gate);
//! (c) 兴旺 fires at day 96 when all 4 T2 techs are researched AND monument built
//!     AND any faction tier == "allied".
//!
//! Test setup follows the research.rs pattern: `Runtime::boot(frontier_dir())` loads the full
//! Frontier scene + logic. We use `sim.world.set_field(id, "Clock.day", json!(12))` directly
//! (the stage system reads `Clock.day` from the entity — no need to step 12*90*60 ticks).
//!
//! NOTE: `Colony.struct_count` is overwritten every tick by the tally -> apply-rates rule
//! pipeline based on actual Structure entities in the world. To make struct_count = 5 we
//! spawn 5 Structure entities (kind="plot") rather than writing the field directly — direct
//! writes would be clobbered by the rule on the next step.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

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

/// Spawn `n` Structure entities with the given kind. Returns their entity IDs.
/// These are counted by the tally system so `apply-rates` writes struct_count = n.
fn spawn_structures(sim: &mut vitric_sim::Sim, n: usize, kind: &str) {
    for i in 0..n {
        let e = sim.world.spawn_named(&format!("test_struct_{kind}_{i}")).expect("spawn structure");
        sim.world.set_component(e, "Structure", json!({
            "kind": kind,
            "tier": 1,
            "_cd_t": 0
        })).expect("set Structure component");
    }
}

#[test]
fn stage_advances_to_foothold_at_day_12() {
    // Day 11 + tech + struct 5 -> stage still "起步" (day-floor 12 not yet met).
    // Then advance to day 12 -> stage should flip to "立足".
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    // Spawn 5 plot structures so tally -> apply-rates writes Colony.struct_count = 5.
    spawn_structures(&mut sim, 5, "plot");

    set_field(&mut sim, "colony", "Clock.day", json!(11));
    set_field(&mut sim, "colony", "Clock.time", json!(0));
    set_field(&mut sim, "colony", "Research.has_survival_t1", json!(1));

    sim.step(&mut rt).unwrap();
    let stage_at_day_11 = get_field(&sim, "colony", "Colony.stage");
    assert_eq!(stage_at_day_11.as_str(), Some("起步"),
        "stage should still be 起步 at day 11 (day-floor 12 not met), got {stage_at_day_11:?}");

    // Advance to day 12 — end of spring, threshold met.
    set_field(&mut sim, "colony", "Clock.day", json!(12));
    sim.step(&mut rt).unwrap();

    let stage_at_day_12 = get_field(&sim, "colony", "Colony.stage");
    assert_eq!(stage_at_day_12.as_str(), Some("立足"),
        "stage should advance to 立足 at day 12 with survival_t1 + struct 5, got {stage_at_day_12:?}");
}

#[test]
fn stage_does_not_advance_without_tech() {
    // Day 12 + struct 5 but has_survival_t1 = 0 -> stage stays "起步".
    // Tech is a hard gate, not just a day-floor.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    // Spawn 5 plot structures so struct_count would be 5 if tech were the only gate.
    spawn_structures(&mut sim, 5, "plot");

    set_field(&mut sim, "colony", "Clock.day", json!(12));
    set_field(&mut sim, "colony", "Research.has_survival_t1", json!(0));

    sim.step(&mut rt).unwrap();

    let stage = get_field(&sim, "colony", "Colony.stage");
    assert_eq!(stage.as_str(), Some("起步"),
        "stage should remain 起步 when survival_t1 not researched, got {stage:?}");
}

#[test]
fn stage_advances_to_prosperity_at_day_96() {
    // Day 96 + all 4 T2 techs + monument + faction allied -> stage "兴旺".
    // 兴旺 is checked first (highest stage wins), so earlier-stage conditions don't matter.
    // The fields 兴旺 depends on (Research.has_*_t2, Faction.tier_*, Colony.monument_built)
    // are NOT overwritten by per-tick rules — only struct_count/pop are, and 兴旺 ignores those.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();

    set_field(&mut sim, "colony", "Clock.day", json!(96));
    set_field(&mut sim, "colony", "Research.has_survival_t1", json!(1));
    set_field(&mut sim, "colony", "Research.has_survival_t2", json!(1));
    set_field(&mut sim, "colony", "Research.has_agriculture_t1", json!(1));
    set_field(&mut sim, "colony", "Research.has_agriculture_t2", json!(1));
    set_field(&mut sim, "colony", "Research.has_exploration_t2", json!(1));
    set_field(&mut sim, "colony", "Research.has_industry_t2", json!(1));
    set_field(&mut sim, "colony", "Colony.monument_built", json!(1));
    set_field(&mut sim, "colony", "Faction.tier_caravan", json!("allied"));

    sim.step(&mut rt).unwrap();

    let stage = get_field(&sim, "colony", "Colony.stage");
    assert_eq!(stage.as_str(), Some("兴旺"),
        "stage should advance to 兴旺 at day 96 with all T2 + monument + allied faction, got {stage:?}");
}
