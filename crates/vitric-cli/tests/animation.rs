//! 声明式动画端到端：引擎独占 Sprite.image 写权、切片段从头播、
//! 非循环片段播完发 anim-finished、全程确定性。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::{advance_animations, Runtime};
use vitric_data::Clip;
use vitric_ecs::World;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run")
}

fn clip(frames: &[&str], fps: u32, looping: bool) -> Clip {
    serde_json::from_value(json!({
        "frames": frames, "fps": fps, "loop": looping
    }))
    .unwrap()
}

fn anim_entity(w: &mut World, clip: &str) -> vitric_ecs::EntityId {
    let e = w.spawn();
    w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#fff", "image": ""}))
        .unwrap();
    w.set_component(e, "Anim", json!({"clip": clip, "prev": "", "t": 0, "done": false}))
        .unwrap();
    e
}

#[test]
fn looping_clip_cycles_frames() {
    let mut w = World::new();
    let e = anim_entity(&mut w, "spin");
    let clips = BTreeMap::from([("spin".to_string(), clip(&["a.png", "b.png"], 30, true))]);
    // fps=30：每 2 tick 换一帧；4 tick 一轮回
    let mut seen = Vec::new();
    for _ in 0..8 {
        advance_animations(&mut w, &clips).unwrap();
        seen.push(w.get_field(e, "Sprite.image").unwrap().as_str().unwrap().to_string());
    }
    assert_eq!(seen, vec!["a.png", "a.png", "b.png", "b.png", "a.png", "a.png", "b.png", "b.png"]);
}

#[test]
fn switching_clip_restarts_and_oneshot_finishes_once() {
    let mut w = World::new();
    let e = anim_entity(&mut w, "boom");
    let clips = BTreeMap::from([
        ("boom".to_string(), clip(&["b0.png", "b1.png"], 60, false)),
        ("idle".to_string(), clip(&["i.png"], 1, true)),
    ]);
    // 非循环 60fps 两帧：tick0=b0, tick1=b1, tick2 起播完保持末帧
    let mut finished = 0;
    for _ in 0..5 {
        finished += advance_animations(&mut w, &clips).unwrap().len();
    }
    assert_eq!(w.get_field(e, "Sprite.image").unwrap(), &json!("b1.png"), "停在末帧");
    assert_eq!(finished, 1, "anim-finished 只发一次");

    // 切回 boom：从头重播，再发一次
    w.set_field(e, "Anim.clip", json!("idle")).unwrap();
    advance_animations(&mut w, &clips).unwrap();
    w.set_field(e, "Anim.clip", json!("boom")).unwrap();
    advance_animations(&mut w, &clips).unwrap();
    assert_eq!(w.get_field(e, "Sprite.image").unwrap(), &json!("b0.png"), "切换后从头播");
}

#[test]
fn unknown_clip_is_explicit() {
    let mut w = World::new();
    anim_entity(&mut w, "ghost");
    let clips = BTreeMap::from([("real".to_string(), clip(&["x.png"], 1, true))]);
    let err = advance_animations(&mut w, &clips).unwrap_err();
    assert!(err.contains("ghost") && err.contains("real"), "{err}");
}

#[test]
fn coin_run_walk_cycle_via_rules() {
    // 完整链路：input → 规则切 Anim.clip → 引擎推帧 → Sprite.image 换成走路帧
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    let player = sim.world.entity("player").unwrap();
    // 初始 idle
    sim.step(&mut rt).unwrap();
    assert_eq!(sim.world.get_field(player, "Sprite.image").unwrap(), &json!("player.png"));
    // 按右键 → walk 动画
    sim.inject_input("right", "pressed");
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }
    let img = sim.world.get_field(player, "Sprite.image").unwrap().as_str().unwrap().to_string();
    assert!(img.starts_with("player-walk-"), "应在播走路动画，实际 {img}");
    // 松开 → 回 idle
    sim.inject_input("right", "released");
    for _ in 0..2 {
        sim.step(&mut rt).unwrap();
    }
    assert_eq!(sim.world.get_field(player, "Sprite.image").unwrap(), &json!("player.png"));
    // 金币也在转
    let coins = sim.world.query(&["Coin"]);
    let coin_img = sim.world.get_field(coins[0], "Sprite.image").unwrap().as_str().unwrap().to_string();
    assert!(coin_img.starts_with("coin-"), "{coin_img}");
}

#[test]
fn animation_is_deterministic_in_replay() {
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.start_recording();
    sim.inject_input("right", "pressed");
    for _ in 0..90 {
        sim.step(&mut rt).unwrap();
    }
    let rec = sim.stop_recording().unwrap();
    let (mut sim2, mut rt2) = Runtime::boot(&example_dir()).unwrap();
    sim2.replay(&rec, &mut rt2).expect("含动画状态的世界必须逐帧重放一致");
}
