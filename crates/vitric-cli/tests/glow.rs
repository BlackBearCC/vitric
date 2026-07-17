//! glow flagship example end-to-end: AI art asset loading, collection/clear chain, juice (screen shake/particles/HUD) all verified.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/glow")
}

#[test]
fn collect_gem_then_light_the_lantern() {
    let (mut sim, mut rt) = Runtime::boot(&dir()).unwrap();
    let hero = sim.world.entity("hero").unwrap();
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    // Teleport to the first gem: collect it → score/HUD/screen shake/spark particles
    sim.world.set_field(hero, "Position.x", json!(10.0)).unwrap();
    sim.world.set_field(hero, "Position.y", json!(4.6)).unwrap();
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }
    assert_eq!(sim.world.get_field(hero, "Score.value").unwrap(), &json!(1));
    let hud = sim.world.entity("hud").unwrap();
    assert_eq!(sim.world.get_field(hud, "Text.content").unwrap(), &json!("GEMS 1/5"));
    let cam = sim.world.entity("camera").unwrap();
    assert!(sim.world.get_field(cam, "Shake.amplitude").unwrap().as_f64().unwrap() > 0.0);
    assert!(!sim.world.query(&["Particle"]).is_empty(), "应有火花/萤火虫粒子在场");

    // Teleport to the lantern: clear → copy/game-won; particles eventually cleaned up by the engine
    sim.world.set_field(hero, "Position.x", json!(47.0)).unwrap();
    sim.world.set_field(hero, "Position.y", json!(1.6)).unwrap();
    let mut won = false;
    for _ in 0..5 {
        sim.step(&mut rt).unwrap();
        won = won || rt.drain_observed().iter().any(|e| e.name == "game-won");
    }
    assert!(won, "碰灯笼应发 game-won");
    assert_eq!(sim.world.get_field(hud, "Text.content").unwrap(), &json!("LIT! 1/5 GEMS"));
}

#[test]
fn lantern_sparks_emitter_is_visible_and_deterministic() {
    // Lantern sparks (Emitter, pure render-layer particles): teleport next to the lantern, after the camera follows over,
    // the frame must actually have particles moving, and rendering the same tick twice is byte-identical
    let (mut sim, mut rt) = Runtime::boot(&dir()).unwrap();
    let hero = sim.world.entity("hero").unwrap();
    sim.world.set_field(hero, "Position.x", json!(44.0)).unwrap();
    sim.world.set_field(hero, "Position.y", json!(2.0)).unwrap();
    for _ in 0..240 {
        sim.step(&mut rt).unwrap(); // Camera lerp catches up + particle stream reaches steady state
    }
    let assets = vitric_render::Assets::load_dir(&dir().join("assets")).unwrap();
    let a = vitric_render::render_world(&sim.world, 320, 180, &assets, sim.tick).unwrap();
    let b = vitric_render::render_world(&sim.world, 320, 180, &assets, sim.tick).unwrap();
    assert_eq!(a, b, "同一 tick 两次渲染逐字节一致");
    // Turn the emitter off and render the same tick again: the frame must differ — proving sparks were actually drawn
    let sparks = sim.world.entity("lantern-sparks").unwrap();
    sim.world.set_field(sparks, "Emitter.active", json!(false)).unwrap();
    let muted = vitric_render::render_world(&sim.world, 320, 180, &assets, sim.tick).unwrap();
    assert_ne!(a, muted, "active=false 后画面应少了火花");
    // describe produces a summary row for the emitter
    let d = vitric_render::describe_world(&sim.world, 320, 180).unwrap();
    let ems = d["emitters"].as_array().unwrap();
    assert_eq!(ems[0]["name"], json!("lantern-sparks"));
    assert_eq!(ems[0]["kind"], json!("stream"));
}
