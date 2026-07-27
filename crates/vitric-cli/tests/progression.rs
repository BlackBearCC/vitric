//! End-to-end test for the progression module: boot the progression-demo, kill
//! the enemy for XP, verify level-up + stat bonus + threshold growth.
//!
//! Tests the full combat → progression composition: attack → damage → died →
//! gain-xp → leveled-up → apply_level_up_bonus. This is the proof that
//! progression composes with combat without glue code (pure event flow).

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/progression-demo")
}

fn player_hp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Health.hp").unwrap().as_i64().unwrap()
}

fn player_max_hp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Health.max").unwrap().as_i64().unwrap()
}

fn player_attack(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Attack.power").unwrap().as_i64().unwrap()
}

fn player_level(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Level.value").unwrap().as_i64().unwrap()
}

fn player_xp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "XP.current").unwrap().as_i64().unwrap()
}

fn player_threshold(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "XP.threshold").unwrap().as_i64().unwrap()
}

fn player_points(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Level.points").unwrap().as_i64().unwrap()
}

fn enemy_hp(sim: &vitric_sim::Sim) -> i64 {
    let e = sim.world.entity("enemy").unwrap();
    sim.world.get_field(e, "Health.hp").unwrap().as_i64().unwrap()
}

/// Press X once and step enough ticks for the attack cascade to land:
/// input → attack → damage → HP write. 4 ticks for the full chain.
fn attack(sim: &mut vitric_sim::Sim, rt: &mut Runtime) {
    sim.inject_input("x", "pressed");
    for _ in 0..4 {
        sim.step(rt).unwrap();
    }
}

#[test]
fn progression_demo_check_passes() {
    let (_sim, _rt) =
        Runtime::boot(&demo_dir()).expect("progression-demo must pass vitric check and boot");
}

#[test]
fn kill_enemy_grants_xp_and_levels_up() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Initial state: level 1, 0 XP, threshold 100, 100 HP, 40 attack.
    assert_eq!(player_level(&sim), 1);
    assert_eq!(player_xp(&sim), 0);
    assert_eq!(player_threshold(&sim), 100);
    assert_eq!(player_max_hp(&sim), 100);
    assert_eq!(player_attack(&sim), 40);
    assert_eq!(player_points(&sim), 0);

    // Kill the enemy: 3 attacks (40+40+40=120 > 100 HP).
    attack(&mut sim, &mut rt); // 100 → 60
    assert_eq!(enemy_hp(&sim), 60);
    attack(&mut sim, &mut rt); // 60 → 20
    assert_eq!(enemy_hp(&sim), 20);
    attack(&mut sim, &mut rt); // 20 → 0 → died → gain-xp(120)

    // Full cascade after the killing blow:
    //   tick N:   input → attack (carryover)
    //   tick N+1: attack → damage (carryover)
    //   tick N2:  damage → hp=0, died (carryover)
    //   tick N+3: died → enemy-dies rule → gain-xp (carryover)
    //   tick N+4: gain-xp → level up, leveled-up (carryover), XP/Level writes deferred
    //   tick N+5: leveled-up → apply_level_up_bonus, Health/Attack writes deferred
    //   tick N+6: all deferred writes visible
    // attack() already stepped 4 ticks (N..N+3). Step 4 more to clear the cascade.
    for _ in 0..4 {
        sim.step(&mut rt).unwrap();
    }

    // Enemy dead.
    assert_eq!(enemy_hp(&sim), 0, "enemy should be dead");

    // Player gained 120 XP. Threshold was 100 → level up: 120 - 100 = 20 remaining.
    // Level 1 → 2. Threshold grows 100 → 150 (floor(100 * 3/2)).
    assert_eq!(player_level(&sim), 2, "player should level up to 2");
    assert_eq!(player_xp(&sim), 20, "20 XP should remain after level-up");
    assert_eq!(player_threshold(&sim), 150, "threshold should grow to 150");
    assert_eq!(player_points(&sim), 1, "1 unspent stat point from level-up");

    // Level-up bonus: +20 max HP (full heal to new max), +10 attack.
    assert_eq!(player_max_hp(&sim), 120, "max HP should increase by 20");
    assert_eq!(player_hp(&sim), 120, "HP should be full after level-up heal");
    assert_eq!(player_attack(&sim), 50, "attack should increase by 10");
}

#[test]
fn xp_below_threshold_does_not_level() {
    // Damage the enemy but don't kill it — no XP granted, no level-up.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // One attack: 100 → 60. Enemy still alive → no died → no gain-xp.
    attack(&mut sim, &mut rt);
    assert_eq!(enemy_hp(&sim), 60, "enemy should be at 60 HP");
    assert_eq!(player_level(&sim), 1, "no level-up without XP gain");
    assert_eq!(player_xp(&sim), 0, "no XP without a kill");
    assert_eq!(player_max_hp(&sim), 100, "no stat bonus without level-up");
}
