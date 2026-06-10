//! cave-gen 示例端到端：配方生成关卡的确定性与玩法规则。

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cave-gen")
}

#[test]
fn recipe_generates_exactly_what_it_says() {
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    // tick 0 的 start 事件触发生成（call 的事件下一 tick 才被脚本消化完）
    sim.step(&mut rt).unwrap();
    assert_eq!(sim.world.query(&["Gem"]).len(), 10, "配方说 10 颗宝石");
    assert_eq!(sim.world.query(&["Hazard"]).len(), 14, "配方说 14 个尖刺");
    // 生成物都有图形和碰撞体
    assert_eq!(sim.world.query(&["Gem", "Sprite", "Collider"]).len(), 10);
    // 出生点安全区干净
    for id in sim.world.query(&["Hazard"]) {
        let x = sim.world.get_field(id, "Position.x").unwrap().as_f64().unwrap();
        let y = sim.world.get_field(id, "Position.y").unwrap().as_f64().unwrap();
        assert!(
            x.abs() >= 3.0 || y.abs() >= 3.0,
            "尖刺不能压在出生点上: ({x},{y})"
        );
    }
}

#[test]
fn same_seed_same_cave_and_replay_holds() {
    let gen_hash = || {
        let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
        for _ in 0..30 {
            sim.step(&mut rt).unwrap();
        }
        sim.world.state_hash()
    };
    assert_eq!(gen_hash(), gen_hash(), "同种子必然生成同一张关卡");

    // 含生成过程的录像重放
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.start_recording();
    sim.inject_input("right", "pressed");
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    let rec = sim.stop_recording().unwrap();
    let (mut sim2, mut rt2) = Runtime::boot(&example_dir()).unwrap();
    sim2.replay(&rec, &mut rt2).unwrap();
}

#[test]
fn gem_collection_and_hazard_reset_rules() {
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.step(&mut rt).unwrap();
    let player = sim.world.entity("player").unwrap();

    // 把玩家直接放到一颗宝石上 → 吃到 +1
    let gem = sim.world.query(&["Gem"])[0];
    let gx = sim.world.get_field(gem, "Position.x").unwrap().clone();
    let gy = sim.world.get_field(gem, "Position.y").unwrap().clone();
    sim.world.set_field(player, "Position.x", gx).unwrap();
    sim.world.set_field(player, "Position.y", gy).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(sim.world.get_field(player, "Score.value").unwrap(), &json!(1));
    assert!(!sim.world.is_alive(gem), "宝石被吃掉");
    assert_eq!(sim.world.query(&["Gem"]).len(), 9);

    // 放到尖刺上 → 弹回出生点
    let hazard = sim.world.query(&["Hazard"])[0];
    let hx = sim.world.get_field(hazard, "Position.x").unwrap().clone();
    let hy = sim.world.get_field(hazard, "Position.y").unwrap().clone();
    sim.world.set_field(player, "Position.x", hx).unwrap();
    sim.world.set_field(player, "Position.y", hy).unwrap();
    sim.step(&mut rt).unwrap();
    assert_eq!(sim.world.get_field(player, "Position.x").unwrap().as_f64(), Some(0.0));
    assert_eq!(sim.world.get_field(player, "Position.y").unwrap().as_f64(), Some(0.0));
}

#[test]
fn collecting_all_gems_clears_level() {
    use vitric_sim::GameLogic;
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.step(&mut rt).unwrap();
    let player = sim.world.entity("player").unwrap();
    // 逐颗瞬移收集
    let mut events = Vec::new();
    for _ in 0..10 {
        let gems = sim.world.query(&["Gem"]);
        let gem = gems[0];
        let gx = sim.world.get_field(gem, "Position.x").unwrap().clone();
        let gy = sim.world.get_field(gem, "Position.y").unwrap().clone();
        sim.world.set_field(player, "Position.x", gx).unwrap();
        sim.world.set_field(player, "Position.y", gy).unwrap();
        sim.step(&mut rt).unwrap();
        events.extend(rt.drain_observed());
    }
    assert_eq!(sim.world.get_field(player, "Score.value").unwrap(), &json!(10));
    assert!(sim.world.query(&["Gem"]).is_empty());
    assert!(
        events.iter().any(|e| e.name == "level-clear"),
        "应发出 level-clear，实际事件: {:?}",
        events.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
}
