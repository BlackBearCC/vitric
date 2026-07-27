//! End-to-end test for the combat module: boot the combat-demo, drive attacks
//! and healing, verify HP changes and enemy death/despawn through the full
//! rules → script (module) → rules pipeline.

use std::path::PathBuf;

use serde_json::json;
use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/combat-demo")
}

fn player_hp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Health.hp").unwrap().as_i64().unwrap()
}

fn enemy_hp(sim: &vitric_sim::Sim) -> Option<i64> {
    sim.world.entity("enemy")
        .ok()
        .and_then(|e| sim.world.get_field(e, "Health.hp").ok())
        .and_then(|v| v.as_i64())
}

/// Press X to attack: input → emit attack → __combat_attack → emit damage → __combat_damage.
/// Takes 3 ticks for the damage to land. We step 4 to be safe (extra tick for deferred writes).
fn attack(sim: &mut vitric_sim::Sim, rt: &mut Runtime) {
    sim.inject_input("x", "pressed");
    for _ in 0..4 {
        sim.step(rt).unwrap();
    }
}

/// Press H to heal: input → emit heal → __combat_heal. Takes 2 ticks; step 3 for safety.
fn heal(sim: &mut vitric_sim::Sim, rt: &mut Runtime) {
    sim.inject_input("h", "pressed");
    for _ in 0..3 {
        sim.step(rt).unwrap();
    }
}

#[test]
fn combat_demo_check_passes() {
    let (_sim, _rt) = Runtime::boot(&demo_dir()).expect("combat-demo must pass vitric check and boot");
}

#[test]
fn combat_attack_reduces_enemy_hp_and_kills() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Initial state: both at 100 hp.
    assert_eq!(player_hp(&sim), 100);
    assert_eq!(enemy_hp(&sim), Some(100));

    // Attack 1: 100 → 60 (player power = 40).
    attack(&mut sim, &mut rt);
    assert_eq!(enemy_hp(&sim), Some(60), "enemy hp should be 60 after 1st attack");

    // Attack 2: 60 → 20.
    attack(&mut sim, &mut rt);
    assert_eq!(enemy_hp(&sim), Some(20), "enemy hp should be 20 after 2nd attack");

    // Attack 3: 20 → 0 → died → stashed off-screen (entity kept alive for HUD).
    attack(&mut sim, &mut rt);
    // The died event is emitted as carryover; step once more to let enemy-dies rule stash.
    sim.step(&mut rt).unwrap();
    assert_eq!(enemy_hp(&sim), Some(0), "enemy hp should be 0 after death");

    // Player should be unharmed (enemy never attacked back in this demo).
    assert_eq!(player_hp(&sim), 100, "player should be at full hp");
}

#[test]
fn combat_heal_restores_hp() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Damage the player directly via a damage event to test healing.
    // We emit damage by injecting it through the rules — but this demo has no rule
    // to damage the player. Instead, test healing from full: heal should be clamped to max.
    assert_eq!(player_hp(&sim), 100);
    heal(&mut sim, &mut rt);
    assert_eq!(player_hp(&sim), 100, "healing at full hp should clamp to max");

    // Manually damage the player via setField, then heal.
    let p = sim.world.entity("player").unwrap();
    sim.world.set_field(p, "Health.hp", json!(50)).unwrap();
    sim.step(&mut rt).unwrap(); // let the write flush
    assert_eq!(player_hp(&sim), 50);

    // Heal 30 → 80.
    heal(&mut sim, &mut rt);
    assert_eq!(player_hp(&sim), 80, "healing 30 from 50 should give 80");

    // Heal 30 again → 100 (clamped to max).
    heal(&mut sim, &mut rt);
    assert_eq!(player_hp(&sim), 100, "healing past max should clamp to 100");
}
