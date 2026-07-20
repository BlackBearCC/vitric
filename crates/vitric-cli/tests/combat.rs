//! Task 10 (Combat System) integration tests.
//!
//! Verifies: (a) night-fall{threat} spawns a wave (enemy_snapshot non-empty);
//! (b) enemy-ai moves enemies toward the cached player position;
//! (c) player_attack (combat mode + mouse click) kills an adjacent enemy and drops loot;
//! (d) player-respawn-check restores Hp + teleports to (7,7) on Hp<=0.
//!
//! Test setup follows the research.rs / companions.rs pattern: `Runtime::boot(frontier_dir())`
//! loads the full Frontier scene + logic. Tick 0 is reserved for the `start` event (seed-start
//! rule sets initial inventory). Tests step once to settle before priming.

use std::path::PathBuf;

use serde_json::{json, Value};

use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

/// Helper: set a single field on a named entity.
fn set_field(sim: &mut vitric_sim::Sim, name: &str, path: &str, value: Value) {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.set_field(id, path, value).expect("set_field ok");
}

/// Helper: read a single field from a named entity as a cloned Value.
fn get_field(sim: &vitric_sim::Sim, name: &str, path: &str) -> Value {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.get_field(id, path).expect("get_field ok").clone()
}

/// Parse Colony.enemy_snapshot (JSON text) into a Vec of enemy objects.
fn parse_enemy_snapshot(sim: &vitric_sim::Sim) -> Vec<Value> {
    let raw = get_field(sim, "colony", "Colony.enemy_snapshot");
    let s = raw.as_str().unwrap_or("[]");
    serde_json::from_str::<Vec<Value>>(s).unwrap_or_default()
}

/// Spawn a single gnawer enemy at (x, y) directly on the world (bypasses the spawn_wave fn).
/// Sets all components that enemy-snapshot / enemy-ai / player_attack expect.
fn spawn_gnawer(sim: &mut vitric_sim::Sim, x: f64, y: f64) -> vitric_ecs::EntityId {
    let id = sim.world.spawn();
    sim.world
        .set_component(id, "Enemy", json!({ "kind": "gnawer", "damage": 5, "aggro_range": 8, "home_region": "wild", "_attack_cd": 0 }))
        .expect("set Enemy");
    sim.world
        .set_component(id, "Position", json!({ "x": x, "y": y }))
        .expect("set Position");
    sim.world
        .set_component(id, "Velocity", json!({ "x": 0, "y": 0 }))
        .expect("set Velocity");
    sim.world
        .set_component(id, "Collider", json!({ "w": 0.8, "h": 0.8 }))
        .expect("set Collider");
    sim.world
        .set_component(id, "Hp", json!({ "value": 20, "max": 20 }))
        .expect("set Hp");
    id
}

#[test]
fn enemy_spawns_on_night_fall() {
    // Inject night-fall{threat: 2}. spawn_wave fn should fire (rule: night-fall-spawn-wave).
    // With threat=2 + regionCount=1 (home only), waveSize = min(8, floor(2 * 1.3)) = 2.
    // After 1 step: Colony.enemy_snapshot should be a non-empty JSON array (>= 1 enemy).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.step(&mut rt).unwrap(); // tick 0: settle (start event + cache-player-pos writes 7,7)

    sim.inject_reply("night-fall", json!({ "threat": 2 }));
    sim.step(&mut rt).unwrap(); // tick 1: spawn_wave fn runs, enemy-snapshot packs enemies

    let enemies = parse_enemy_snapshot(&sim);
    assert!(!enemies.is_empty(),
        "enemy_snapshot should be non-empty after night-fall{{threat:2}}, got: {enemies:?}");
    // waveSize = min(8, floor(2 * 1.3)) = 2 (regionCount=1: home only, mountain not thawed).
    assert_eq!(enemies.len(), 2,
        "wave size should be 2 for threat=2 regionCount=1, got {} enemies: {enemies:?}",
        enemies.len());
}

#[test]
fn enemy_ai_moves_toward_player() {
    // Inject night-fall{threat: 1} (spawns 1 gnawer at x≈30, y∈[5,15]).
    // Step 1 tick (spawns + caches player pos at 7,7). Record the enemy's initial position.
    // Step 30 more ticks (enemy-ai moves the enemy toward 7,7 at 0.8 tiles/sec).
    // Verify the enemy's distance to (7,7) decreased.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.step(&mut rt).unwrap(); // tick 0: settle (player_x/y cached to 7,7)

    sim.inject_reply("night-fall", json!({ "threat": 1 }));
    sim.step(&mut rt).unwrap(); // tick 1: spawn_wave + enemy-snapshot

    let enemies_before = parse_enemy_snapshot(&sim);
    assert_eq!(enemies_before.len(), 1, "expected 1 enemy after threat=1 wave");
    let enemy_before = &enemies_before[0];
    let x0 = enemy_before["x"].as_f64().expect("x is number");
    let y0 = enemy_before["y"].as_f64().expect("y is number");
    let dist0 = ((x0 - 7.0).powi(2) + (y0 - 7.0).powi(2)).sqrt();

    // Step 30 more ticks (~0.5s). At 0.8 tiles/sec, the enemy moves ~0.4 tiles closer.
    // (The enemy spawns at x≈30, distance ~23 from player. After 30 ticks it's ~22.6.)
    // The delta is small but measurable (>0.01).
    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }

    let enemies_after = parse_enemy_snapshot(&sim);
    assert_eq!(enemies_after.len(), 1, "enemy should still exist after 30 ticks");
    let enemy_after = &enemies_after[0];
    let x1 = enemy_after["x"].as_f64().expect("x is number");
    let y1 = enemy_after["y"].as_f64().expect("y is number");
    let dist1 = ((x1 - 7.0).powi(2) + (y1 - 7.0).powi(2)).sqrt();

    assert!(dist1 < dist0,
        "enemy should be closer to (7,7) after 30 ticks: before={dist0:.3} (at {x0:.2},{y0:.2}), after={dist1:.3} (at {x1:.2},{y1:.2})");
}

#[test]
fn player_attack_kills_enemy() {
    // Spawn a single gnawer at (8, 7) — distance 1 from player at (7, 7), within weapon range 2.
    // Set Mode=combat. Step once (cache-player-pos + enemy-snapshot). Inject mouse, step (first swing:
    // hp 20→10, _cd_t=1). Step 65 ticks (cooldown elapses). Inject mouse, step (second swing: hp 10→0,
    // killed, enemy-killed emitted). Step 3 more ticks (enemy-killed → apply_loot → inv-set → inv-apply).
    // Verify Inventory.hide > 0 (gnawer drops 1-2 hide).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.step(&mut rt).unwrap(); // tick 0: settle

    // Spawn gnawer at (8, 7) — within weapon range (2) of player at (7, 7).
    spawn_gnawer(&mut sim, 8.0, 7.0);
    // Reset hide to 0 for clean assertion.
    set_field(&mut sim, "player", "Inventory.hide", json!(0));
    // Switch to combat mode.
    set_field(&mut sim, "uistate", "Mode.value", json!("combat"));
    // Step once to let cache-player-pos + enemy-snapshot populate.
    sim.step(&mut rt).unwrap(); // tick 1

    // First swing: inject mouse, step. player_attack fn runs, hp 20→10, _cd_t=1.
    sim.inject_reply("mouse", json!({}));
    sim.step(&mut rt).unwrap(); // tick 2: first swing

    // Step 65 ticks for weapon cooldown (1 sec = 60 ticks + margin).
    for _ in 0..65 {
        sim.step(&mut rt).unwrap();
    }

    // Second swing: inject mouse, step. hp 10→0, killed, enemy-killed emitted.
    sim.inject_reply("mouse", json!({}));
    sim.step(&mut rt).unwrap(); // tick 68: second swing (kill)

    // Step 3 more ticks for the enemy-killed → apply_loot → inv-set → inv-apply pipeline.
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }

    let hide = get_field(&sim, "player", "Inventory.hide");
    let hide_n = hide.as_i64().unwrap_or(0);
    assert!(hide_n > 0,
        "Inventory.hide should be > 0 after killing a gnawer (drops 1-2 hide), got {hide_n}");

    // Enemy should be gone from the snapshot (despawned on kill).
    let enemies = parse_enemy_snapshot(&sim);
    assert!(enemies.is_empty(),
        "enemy_snapshot should be empty after the gnawer was killed, got {enemies:?}");
}

#[test]
fn player_respawns_on_death() {
    // Set player.Hp.value = 0. Step 1 tick. player-respawn-check system should fire:
    // Hp.value = 100, Hp.max = 100, Position.x = 7, Position.y = 7, Colony.food *= 0.8.
    // Verify state + player-respawned event emitted.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    sim.step(&mut rt).unwrap(); // tick 0: settle

    // Move player away from (7,7) so we can verify teleport.
    set_field(&mut sim, "player", "Position.x", json!(20.0));
    set_field(&mut sim, "player", "Position.y", json!(20.0));
    // Set Hp to 0 to trigger respawn.
    set_field(&mut sim, "player", "Hp.value", json!(0));
    // Zero out Colony.food_rate so food consumption systems don't interfere with the -20% penalty assertion.
    set_field(&mut sim, "colony", "Colony.food_rate", json!(0));
    // Record food before respawn.
    let food_before = get_field(&sim, "colony", "Colony.food").as_f64().unwrap_or(60.0);

    sim.step(&mut rt).unwrap(); // tick 1: player-respawn-check fires

    let hp = get_field(&sim, "player", "Hp.value").as_f64().unwrap_or(-1.0);
    let hp_max = get_field(&sim, "player", "Hp.max").as_f64().unwrap_or(-1.0);
    let px = get_field(&sim, "player", "Position.x").as_f64().unwrap_or(-1.0);
    let py = get_field(&sim, "player", "Position.y").as_f64().unwrap_or(-1.0);
    let food_after = get_field(&sim, "colony", "Colony.food").as_f64().unwrap_or(-1.0);

    assert_eq!(hp, 100.0, "Hp.value should be restored to 100 on respawn, got {hp}");
    assert_eq!(hp_max, 100.0, "Hp.max should be 100 on respawn, got {hp_max}");
    assert_eq!(px, 7.0, "Position.x should be 7 (respawn point) on respawn, got {px}");
    assert_eq!(py, 7.0, "Position.y should be 7 (respawn point) on respawn, got {py}");
    // Food penalty: -20% (with small tolerance for 1 tick of consumption rate jitter).
    let expected_food = (food_before * 0.8).max(0.0);
    let food_diff = (food_after - expected_food).abs();
    assert!(food_diff < 0.05,
        "Colony.food should be ~{expected_food:.3} (80% of {food_before:.3}) after respawn, got {food_after:.3} (diff {food_diff:.3})");

    // Verify player-respawned event was emitted.
    let observed = rt.drain_observed();
    let saw_respawn = observed.iter().any(|e| e.name == "player-respawned");
    assert!(saw_respawn,
        "player-respawned event should be emitted on death, observed: {:?}",
        observed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>());
}
