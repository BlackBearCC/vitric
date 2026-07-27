//! End-to-end test for rpg-mini — the complete-game proof.
//!
//! Composes all seven gameplay modules (inventory + quest + dialogue + game-flow
//! + combat + progression + loot) into a single closed loop: title → talk to elder →
//!   accept quest → collect 3 herbs → turn in quest → win → restart. Also covers
//!   the combat lose path (wolf attacks player to death), the combat kill path
//!   (X kills wolf → loot drops → XP → level up → bonus).
//!
//! This is the structural proof that the engine supports commercial-game closed
//! loops, not just demos: the seven modules compose without glue code, driven
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

fn wolf_hp(sim: &vitric_sim::Sim) -> i64 {
    let w = sim.world.entity("wolf").unwrap();
    sim.world.get_field(w, "Health.hp").unwrap().as_i64().unwrap()
}

fn wolf_pos(sim: &vitric_sim::Sim) -> (f64, f64) {
    let w = sim.world.entity("wolf").unwrap();
    let x = sim.world.get_field(w, "Position.x").unwrap().as_f64().unwrap();
    let y = sim.world.get_field(w, "Position.y").unwrap().as_f64().unwrap();
    (x, y)
}

fn coin_count(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    let items = sim.world.get_field(p, "Inventory.items").unwrap().clone();
    let counts = sim.world.get_field(p, "Inventory.counts").unwrap().clone();
    let arr = items.as_array().unwrap();
    let cnt = counts.as_array().unwrap();
    let mut total = 0;
    for (i, it) in arr.iter().enumerate() {
        if it == &json!("coin") {
            total += cnt[i].as_i64().unwrap_or(0);
        }
    }
    total
}

/// Press X once and step enough ticks for the attack cascade to land:
/// input → emit attack → __combat_attack → emit damage → __combat_damage → HP write.
fn press_x(sim: &mut vitric_sim::Sim, rt: &mut Runtime) {
    sim.inject_input("x", "pressed");
    for _ in 0..4 {
        sim.step(rt).unwrap();
    }
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
fn rpg_mini_combat_death() {
    // Replaces the old instant-lose wolf-hit: the wolf now attacks the player on
    // contact (combat module), and the player only loses when HP reaches 0.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // title → playing.
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim), "playing");
    assert_eq!(player_hp(&sim), 100, "player should start at full HP");

    // Walk into the wolf at (1,2): right to (1,0), up to (1,2).
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap(); // (0,0)→(1,0)
    sim.inject_input("right", "released");
    sim.inject_input("up", "pressed");
    sim.step(&mut rt).unwrap(); // (1,0)→(1,1)
    sim.step(&mut rt).unwrap(); // (1,1)→(1,2), collision → emit attack (carryover)
    sim.inject_input("up", "released");

    // Stand on the wolf: collision fires every tick → wolf keeps attacking.
    // HP cascade per attack: collision (tick N) → attack → damage → HP -= 20 (lands ~tick N+2).
    // Player HP 100, wolf Attack.power 20 → 5 hits to die → died → game-lose → phase=lost.
    // Step plenty of ticks to let the cascade drain HP to 0 and propagate to phase.
    let mut died = false;
    for _ in 0..25 {
        sim.step(&mut rt).unwrap();
        if !died && player_hp(&sim) == 0 {
            died = true;
        }
        if phase(&sim) == "lost" {
            break;
        }
    }
    assert!(died, "player HP should reach 0 from wolf attacks");
    assert_eq!(phase(&sim), "lost", "player death (HP=0) should trigger game-lose");

    // Restart → title, player HP restored.
    sim.inject_input("r", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim), "title");
    assert_eq!(player_hp(&sim), 100, "player HP should reset on restart");
}

#[test]
fn rpg_mini_combat_kill_wolf() {
    // Player can kill the wolf with X (2 hits: 40+40 > 60 HP). After death the
    // wolf is stashed off-screen (entity kept for restart) and the player gains
    // XP → levels up → +20 max HP / +10 attack. Proves combat + progression
    // compose with the other four modules.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // title → playing.
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim), "playing");
    assert_eq!(wolf_hp(&sim), 60, "wolf should start at full HP");
    assert_eq!(player_level(&sim), 1, "player starts at level 1");

    // Player presses X to attack the wolf (no collision needed — X targets @wolf).
    // 1st hit: 60 → 20.
    press_x(&mut sim, &mut rt);
    assert_eq!(wolf_hp(&sim), 20, "wolf HP should be 20 after 1st player attack");

    // 2nd hit: 20 → 0 → died → stash_wolf + gain-xp(100) → level-up → bonus.
    // Full cascade: died(N) → stash+gain-xp(N+1) → level-up+leveled-up(N+2)
    // → apply_level_up_bonus(N+3) → deferred writes visible(N+4).
    press_x(&mut sim, &mut rt);
    for _ in 0..5 {
        sim.step(&mut rt).unwrap(); // clear the died → gain-xp → level-up → bonus cascade
    }
    assert_eq!(wolf_hp(&sim), 0, "wolf HP should be 0 after 2nd attack");

    // Wolf stashed off-screen (entity kept alive for restart, not despawned).
    let (wx, wy) = wolf_pos(&sim);
    assert!(wx < 0.0 && wy < 0.0, "wolf should be stashed off-screen after death, got ({wx}, {wy})");

    // Progression: kill gave 100 XP, threshold was 100 → level up (100-100=0 remaining).
    assert_eq!(player_level(&sim), 2, "player should level up to 2 after killing the wolf");
    assert_eq!(player_xp(&sim), 0, "0 XP remaining after exact-threshold level-up");

    // Level-up bonus: +20 max HP (full heal), +10 attack.
    assert_eq!(player_max_hp(&sim), 120, "max HP should increase by 20 on level-up");
    assert_eq!(player_hp(&sim), 120, "HP should be full (120) after level-up heal");
    assert_eq!(player_attack(&sim), 50, "attack should increase by 10 on level-up");

    // Loot: wolf's LootTable drops 2 coins (chance 1.0, count 2-2) → auto-pickup
    // to player's inventory via the combat → died → loot → pickup → inventory cascade.
    assert_eq!(coin_count(&sim), 2, "wolf death should drop 2 coins into player's inventory");

    // Player can walk through the wolf's former spot (1,2) without dying —
    // the stashed wolf is at (-100,-100), so no collision fires.
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap(); // (0,0)→(1,0)
    sim.inject_input("right", "released");
    sim.inject_input("up", "pressed");
    sim.step(&mut rt).unwrap(); // (1,0)→(1,1)
    sim.step(&mut rt).unwrap(); // (1,1)→(1,2) — wolf's former spot, now empty
    sim.step(&mut rt).unwrap(); // (1,2)→(1,3) — keep moving, no collision
    sim.inject_input("up", "released");
    assert_eq!(phase(&sim), "playing", "walking through the dead wolf's spot should not lose");
    assert_eq!(player_hp(&sim), 120, "player should be unharmed walking through the dead wolf's spot");
}
