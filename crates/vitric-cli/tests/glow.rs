//! glow 旗舰示例端到端：AI 美术素材加载、收集/通关链路、juice（屏震/粒子/HUD）全验。

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
    // 传送到第一颗宝石:吃到 → 计分/HUD/屏震/火花粒子
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

    // 传送到灯笼:通关 → 文案/game-won;粒子最终会被引擎清干净
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
