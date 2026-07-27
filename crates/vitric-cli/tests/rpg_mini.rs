//! End-to-end test for rpg-mini — the complete-game proof.
//!
//! Composes all four gameplay modules (inventory + quest + dialogue + game-flow)
//! into a single closed loop: title → talk to elder → accept quest → collect 3
//! herbs → turn in quest → win → restart. Also verifies the lose path (wolf).
//!
//! This is the structural proof that the engine supports commercial-game closed
//! loops, not just demos: the four modules compose without glue code, driven
//! purely by rules + module-emitted events.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/rpg-mini")
}

fn phase(sim: &vitric_sim::Sim) -> String {
    let game = sim.world.entity("game").unwrap();
    sim.world
        .get_field(game, "GameState.phase")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn quest_state(sim: &vitric_sim::Sim) -> String {
    let q = sim.world.entity("herb-quest").unwrap();
    sim.world
        .get_field(q, "QuestState.state")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn herb_count(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    let items = sim.world.get_field(p, "Inventory.items").unwrap().clone();
    let counts = sim.world.get_field(p, "Inventory.counts").unwrap().clone();
    let arr = items.as_array().unwrap();
    let cnt = counts.as_array().unwrap();
    let mut total = 0;
    for (i, it) in arr.iter().enumerate() {
        if it == &json!("herb") {
            total += cnt[i].as_i64().unwrap_or(0);
        }
    }
    total
}

fn talked_count(sim: &vitric_sim::Sim) -> i64 {
    let elder = sim.world.entity("elder").unwrap();
    sim.world
        .get_field(elder, "Talked.count")
        .unwrap()
        .as_i64()
        .unwrap()
}

fn player_pos(sim: &vitric_sim::Sim) -> (f64, f64) {
    let p = sim.world.entity("player").unwrap();
    let x = sim.world.get_field(p, "Position.x").unwrap().as_f64().unwrap();
    let y = sim.world.get_field(p, "Position.y").unwrap().as_f64().unwrap();
    (x, y)
}

#[test]
fn rpg_mini_check_passes() {
    let (_sim, _rt) = Runtime::boot(&demo_dir()).expect("rpg-mini must pass vitric check and boot");
}

#[test]
fn rpg_mini_full_win_loop() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // --- title → playing ---
    assert_eq!(phase(&sim), "title");
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap(); // game-start processed → phase=playing
    assert_eq!(phase(&sim), "playing");

    // --- walk right to elder at (1,0): offer quest + start dialogue ---
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap(); // player (0,0)→(1,0), collision → quest-offer + talk emitted
    sim.inject_input("right", "released");
    sim.step(&mut rt).unwrap(); // quest-offer processed (state=offered), talk processed (current=0),
                                 // collision again → quest-accept emitted
    sim.step(&mut rt).unwrap(); // quest-accept processed (state=active)
    assert_eq!(quest_state(&sim), "active", "quest should be active after accepting");

    // --- dialogue: press 1 twice to advance through 2 nodes and end ---
    sim.inject_input("1", "pressed");
    sim.step(&mut rt).unwrap(); // dialogue-choose → current=1 (node 1)
    sim.step(&mut rt).unwrap(); // buffer
    sim.inject_input("1", "pressed");
    sim.step(&mut rt).unwrap(); // dialogue-choose → current=-1 (end), Talked.count=1
    sim.step(&mut rt).unwrap(); // buffer for deferred Talked.count write
    assert_eq!(talked_count(&sim), 1, "dialogue end should increment Talked.count");

    // --- collect herb-1 at (3,0): right 2 ticks ---
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap(); // (1,0)→(2,0)
    sim.step(&mut rt).unwrap(); // (2,0)→(3,0), collision herb-1 → pickup emitted, herb-1 despawned
    sim.inject_input("right", "released");
    sim.step(&mut rt).unwrap(); // pickup processed → inventory +1 herb
    assert_eq!(herb_count(&sim), 1, "should have 1 herb after picking herb-1");

    // --- collect herb-2 at (3,3): up 3 ticks ---
    sim.inject_input("up", "pressed");
    sim.step(&mut rt).unwrap(); // (3,0)→(3,1)
    sim.step(&mut rt).unwrap(); // (3,1)→(3,2)
    sim.step(&mut rt).unwrap(); // (3,2)→(3,3), collision herb-2 → pickup
    sim.inject_input("up", "released");
    sim.step(&mut rt).unwrap(); // pickup processed → inventory +1 herb (total 2)
    assert_eq!(herb_count(&sim), 2, "should have 2 herbs after picking herb-2");

    // --- collect herb-3 at (0,3): left 3 ticks ---
    sim.inject_input("left", "pressed");
    sim.step(&mut rt).unwrap(); // (3,3)→(2,3)
    sim.step(&mut rt).unwrap(); // (2,3)→(1,3)
    sim.step(&mut rt).unwrap(); // (1,3)→(0,3), collision herb-3 → pickup
    sim.inject_input("left", "released");
    sim.step(&mut rt).unwrap(); // pickup processed → inventory +1 herb (total 3), quest auto-completes
    sim.step(&mut rt).unwrap(); // buffer for quest-track to set state=completed
    assert_eq!(herb_count(&sim), 3, "should have 3 herbs after picking herb-3");
    assert_eq!(quest_state(&sim), "completed", "quest should auto-complete at 3 herbs");

    // --- return to elder at (1,0): down 3 ticks, right 1 tick ---
    sim.inject_input("down", "pressed");
    sim.step(&mut rt).unwrap(); // (0,3)→(0,2)
    sim.step(&mut rt).unwrap(); // (0,2)→(0,1)
    sim.step(&mut rt).unwrap(); // (0,1)→(0,0)
    sim.inject_input("down", "released");
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap(); // (0,0)→(1,0), collision elder → quest-turn-in emitted
    sim.inject_input("right", "released");
    sim.step(&mut rt).unwrap(); // quest-turn-in processed → state=turned-in, reward, quest-turned-in emitted
    sim.step(&mut rt).unwrap(); // quest-turned-in → win-on-quest-turned-in → game-win emitted
    sim.step(&mut rt).unwrap(); // game-win processed → phase=won
    assert_eq!(phase(&sim), "won", "should win after turning in the quest");

    // Reward: 5 coins in inventory.
    let p = sim.world.entity("player").unwrap();
    let items = sim.world.get_field(p, "Inventory.items").unwrap().clone();
    let counts = sim.world.get_field(p, "Inventory.counts").unwrap().clone();
    let arr = items.as_array().unwrap();
    let cnt = counts.as_array().unwrap();
    let coin_idx = arr.iter().position(|v| v == &json!("coin"));
    assert!(coin_idx.is_some(), "reward should include coins, items: {items}");
    let coin_count = cnt[coin_idx.unwrap()].as_i64().unwrap();
    assert_eq!(coin_count, 5, "should receive 5 coins as quest reward");

    // --- restart → title ---
    sim.inject_input("r", "pressed");
    sim.step(&mut rt).unwrap(); // reset_game runs → game-restart emitted
    sim.step(&mut rt).unwrap(); // game-restart processed → phase=title
    assert_eq!(phase(&sim), "title", "R should restart to title");

    // After restart, quest and inventory should be reset.
    assert_eq!(quest_state(&sim), "inactive", "quest should reset to inactive");
    assert_eq!(herb_count(&sim), 0, "inventory should be cleared");
    let (px, py) = player_pos(&sim);
    assert!((px - 0.0).abs() < 1e-9 && (py - 0.0).abs() < 1e-9, "player should be back at origin");

    // Herbs should be respawned.
    let herbs = sim.world.query(&["Pickup"]);
    assert_eq!(herbs.len(), 3, "3 herbs should be respawned after restart");
}

#[test]
fn rpg_mini_lose_path() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // title → playing.
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim), "playing");

    // Walk into the wolf at (1,2): right to (1,0), up to (1,2).
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap(); // (0,0)→(1,0)
    sim.inject_input("right", "released");
    sim.inject_input("up", "pressed");
    sim.step(&mut rt).unwrap(); // (1,0)→(1,1)
    sim.step(&mut rt).unwrap(); // (1,1)→(1,2), collision wolf → game-lose emitted
    sim.inject_input("up", "released");
    sim.step(&mut rt).unwrap(); // game-lose processed → phase=lost
    sim.step(&mut rt).unwrap(); // buffer for deferred phase write
    assert_eq!(phase(&sim), "lost", "touching the wolf should lose the game");

    // Restart → title.
    sim.inject_input("r", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim), "title");
}
