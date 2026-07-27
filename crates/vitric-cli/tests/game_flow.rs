//! End-to-end test for the game-flow module — the closed-loop backbone.
//!
//! Drives game-flow-demo through the full state machine:
//!   title → (space) → playing → (collect 3 coins) → won → (r) → title
//!   title → (space) → playing → (touch enemy) → lost → (r) → title
//! Verifies phase transitions, score, time increment, and restart reset.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/game-flow-demo")
}

fn phase(sim: &vitric_sim::Sim, _rt: &vitric_cli::runtime::Runtime) -> String {
    let game = sim.world.entity("game").unwrap();
    sim.world
        .get_field(game, "GameState.phase")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn game_flow_demo_check_passes() {
    let (_sim, _rt) = Runtime::boot(&demo_dir()).expect("game-flow-demo 应通过校验并启动");
}

#[test]
fn game_flow_full_loop_win_and_restart() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Boot → title.
    assert_eq!(phase(&sim, &rt), "title", "启动后应在标题屏");

    // Press SPACE → playing (game-start emitted this tick, processed next tick).
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim, &rt), "playing", "SPACE 后应进入 playing");

    let game = sim.world.entity("game").unwrap();
    // time may have already advanced by 1: game-tick-time fires in the same tick
    // that game-start transitions phase to "playing" (rule engine sees the updated
    // phase within the same process_tick pass). The key invariant is that time
    // started from 0 (was reset), not carried over from a prior playthrough.
    let time0 = sim.world.get_field(game, "GameState.time").unwrap().as_i64().unwrap();
    assert!(time0 <= 1, "刚进入 playing 时 time 应从 0 起步, 实际 {time0}");

    // --- collect 3 coins along a path that avoids the enemy at (3,2) ---
    // Path: (0,0) → right to (6,0) [collect coin-1 at (3,0)]
    //       → up to (6,4) [collect coin-2 at (6,2)]
    //       → left to (3,4) [collect coin-3 at (3,4)]
    // 1 unit/tick (speed 60). Enemy at (3,2) is never on this path.

    // Right 6 ticks: x = 1,2,3(coin-1),4,5,6
    sim.inject_input("right", "pressed");
    for _ in 0..6 {
        sim.step(&mut rt).unwrap();
    }
    let score = sim.world.get_field(game, "GameState.score").unwrap().as_i64().unwrap();
    assert_eq!(score, 1, "应已收集 coin-1");

    // Up 4 ticks: y = 1,2(coin-2),3,4
    sim.inject_input("right", "released");
    sim.inject_input("up", "pressed");
    for _ in 0..4 {
        sim.step(&mut rt).unwrap();
    }
    let score = sim.world.get_field(game, "GameState.score").unwrap().as_i64().unwrap();
    assert_eq!(score, 2, "应已收集 coin-2");

    // Left 3 ticks: x = 5,4,3(coin-3) → remaining=0 → emit game-win (carryover)
    sim.inject_input("up", "released");
    sim.inject_input("left", "pressed");
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }

    // coin-3 collision happens on the 3rd left tick; collect_coin's setField
    // (score+1, remaining-1) and emit("game-win") are deferred/carryover.
    // Tick 4: flush score/remaining writes, process game-win → __game_win queues
    //         phase="won" as deferred.
    // Tick 5: flush phase="won" write → visible to reads.
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();

    let score = sim.world.get_field(game, "GameState.score").unwrap().as_i64().unwrap();
    assert_eq!(score, 3, "应已收集全部 3 币");
    let remaining = sim.world.get_field(game, "Coins.remaining").unwrap().as_i64().unwrap();
    assert_eq!(remaining, 0, "Coins.remaining 应为 0");
    assert_eq!(phase(&sim, &rt), "won", "收集全部币后应进入 won");

    // time should have advanced during play.
    let time = sim.world.get_field(game, "GameState.time").unwrap().as_i64().unwrap();
    assert!(time > 0, "playing 期间 time 应已累加, 实际 {time}");

    // --- press R → reset_game + game-restart → title, coins/player restored ---
    sim.inject_input("left", "released");
    sim.inject_input("r", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim, &rt), "title", "R 后应回到 title");

    let score = sim.world.get_field(game, "GameState.score").unwrap().as_i64().unwrap();
    assert_eq!(score, 0, "重启后 score 应归零");
    let time_after = sim.world.get_field(game, "GameState.time").unwrap().as_i64().unwrap();
    assert_eq!(time_after, 0, "重启后 time 应归零");
    let remaining = sim.world.get_field(game, "Coins.remaining").unwrap().as_i64().unwrap();
    assert_eq!(remaining, 3, "重启后 Coins.remaining 应恢复为 3");

    // Player back at origin.
    let player = sim.world.entity("player").unwrap();
    let px = sim.world.get_field(player, "Position.x").unwrap().as_f64().unwrap();
    let py = sim.world.get_field(player, "Position.y").unwrap().as_f64().unwrap();
    assert!((px - 0.0).abs() < 1e-9 && (py - 0.0).abs() < 1e-9, "重启后玩家应回到 (0,0), 实际 ({px},{py})");

    // Coin-1 back at (3,0).
    let coin1 = sim.world.entity("coin-1").unwrap();
    let cx = sim.world.get_field(coin1, "Position.x").unwrap().as_f64().unwrap();
    assert!((cx - 3.0).abs() < 1e-9, "重启后 coin-1 应回到 x=3, 实际 {cx}");
}

#[test]
fn game_flow_lose_path_and_restart() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // title → playing.
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim, &rt), "playing");

    // Walk into the enemy at (3,2): right to x=3 (y=0), then up to y=2.
    sim.inject_input("right", "pressed");
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }
    // Player at (3,0). Now go up — at y=2 the enemy is hit.
    sim.inject_input("right", "released");
    sim.inject_input("up", "pressed");
    for _ in 0..2 {
        sim.step(&mut rt).unwrap();
    }
    // game-lose emitted as carryover at y=2; step once to process → lost.
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim, &rt), "lost", "碰到敌人应进入 lost");

    // Restart → title.
    sim.inject_input("up", "released");
    sim.inject_input("r", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(phase(&sim, &rt), "title", "R 后应回到 title");
}

#[test]
fn game_flow_time_does_not_advance_on_title() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    // Step several ticks on title — time must stay 0.
    for _ in 0..5 {
        sim.step(&mut rt).unwrap();
    }
    let game = sim.world.entity("game").unwrap();
    let time = sim.world.get_field(game, "GameState.time").unwrap().as_i64().unwrap();
    assert_eq!(time, 0, "title 屏 time 不应累加");

    let phase_val = phase(&sim, &rt);
    assert_eq!(phase_val, "title");
    let _ = phase_val; // silence unused warning path
    let _ = json!(0);
}
