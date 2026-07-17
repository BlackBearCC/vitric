//! jump example end-to-end: platform physics (gravity/landing/jumping) + text feedback, pure rules zero scripts.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/jump")
}

#[test]
fn hero_falls_lands_jumps_and_wins() {
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    let hero = sim.world.entity("hero").unwrap();

    // Spawns in the air → gravity pulls to the ground, stands firmly
    for _ in 0..90 {
        sim.step(&mut rt).unwrap();
    }
    assert_eq!(sim.world.get_field(hero, "Body.grounded").unwrap(), &json!(true));
    let ground_y = sim.world.get_field(hero, "Position.y").unwrap().as_f64().unwrap();
    assert!((ground_y - 1.0).abs() < 1e-9, "站在地面顶面，实际 {ground_y}");

    // Jump rule: only effective when grounded
    sim.inject_input("space", "pressed");
    for _ in 0..10 {
        sim.step(&mut rt).unwrap();
    }
    let air_y = sim.world.get_field(hero, "Position.y").unwrap().as_f64().unwrap();
    assert!(air_y > ground_y + 0.5, "应该跳起来了，实际 {air_y}");
    assert_eq!(sim.world.get_field(hero, "Body.grounded").unwrap(), &json!(false));
    // Pressing jump again in the air does nothing (not grounded)
    let vy_before = sim.world.get_field(hero, "Velocity.y").unwrap().as_f64().unwrap();
    sim.inject_input("space", "pressed");
    sim.step(&mut rt).unwrap();
    let vy_after = sim.world.get_field(hero, "Velocity.y").unwrap().as_f64().unwrap();
    assert!(vy_after < vy_before, "空中二段跳必须无效（速度只随重力降）");

    // After landing back on the ground, move the hero next to the flag to verify the end-game rule
    for _ in 0..120 {
        sim.step(&mut rt).unwrap();
    }
    sim.world.set_field(hero, "Position.x", json!(16.0)).unwrap();
    sim.world.set_field(hero, "Position.y", json!(7.0)).unwrap();
    let mut won = false;
    for _ in 0..5 {
        sim.step(&mut rt).unwrap();
        won = won || rt.drain_observed().iter().any(|e| e.name == "game-won");
    }
    assert!(won, "碰旗应发 game-won");
    let msg = sim.world.entity("msg").unwrap();
    assert_eq!(sim.world.get_field(msg, "Text.content").unwrap(), &json!("YOU MADE IT!"));
    assert!(sim.world.query(&["Goal"]).is_empty(), "旗子应被销毁");
}
