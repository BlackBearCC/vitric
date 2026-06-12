//! 粒子发射器端到端确定性：粒子是渲染层纯函数（不进模拟状态），所以
//! 录像重放/快照回退后**画面**也必须逐字节回到同一条轨迹——这里在
//! sim + render 两层拼起来锁死这个不变量（单元测试各自只看一层）。

use serde_json::json;

use vitric_ecs::fnv1a_64;
use vitric_render::Assets;
use vitric_sim::{GameLogic, Pcg32, Sim};

/// 测试场：一个移动实体（让状态真的在演化）+ 一个 stream 火花 + 一个待触发的 burst。
fn particle_world(sim: &mut Sim) {
    let mover = sim.world.spawn_named("mover").unwrap();
    sim.world.set_component(mover, "Position", json!({"x": -4.0, "y": 0.0})).unwrap();
    sim.world.set_component(mover, "Velocity", json!({"x": 3.0, "y": 0.0})).unwrap();
    let torch = sim.world.spawn_named("torch").unwrap();
    sim.world.set_component(torch, "Position", json!({"x": 0.0, "y": -1.0})).unwrap();
    sim.world
        .set_component(
            torch,
            "Emitter",
            json!({"kind": "stream", "rate": 24.0, "lifetime": 45, "size": 0.4,
                   "speed_min": 1.0, "speed_max": 3.0, "dir": 90.0, "spread": 40.0,
                   "gravity": -2.0, "color": "#ffcc40", "color_end": "#ff3000"}),
        )
        .unwrap();
    let boom = sim.world.spawn_named("boom").unwrap();
    sim.world.set_component(boom, "Position", json!({"x": 2.0, "y": 1.0})).unwrap();
    sim.world
        .set_component(
            boom,
            "Emitter",
            json!({"kind": "burst", "count": 16, "lifetime": 30, "size": 0.5, "burst": -1,
                   "speed_min": 2.0, "speed_max": 6.0, "color": "#80e0ff"}),
        )
        .unwrap();
}

/// 在 tick 60 触发 burst 的确定性逻辑（模拟"规则往 burst 字段写当前 tick"）。
struct BurstAt60;
impl GameLogic for BurstAt60 {
    fn on_tick(
        &mut self,
        w: &mut vitric_ecs::World,
        _: Vec<vitric_rules::Event>,
        _: &mut Pcg32,
        tick: u64,
    ) -> Result<(), String> {
        if tick == 60 {
            let boom = w.entity("boom").map_err(|e| e.to_string())?;
            w.set_field(boom, "Emitter.burst", json!(60)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn frame_hash(sim: &Sim) -> u64 {
    let buf = vitric_render::render_world(&sim.world, 96, 96, &Assets::empty(), sim.tick).unwrap();
    fnv1a_64(&buf)
}

#[test]
fn recording_replay_reproduces_particle_frames_bit_exact() {
    // 录一局：每 tick 记一帧画面哈希
    let mut sim = Sim::new(11);
    particle_world(&mut sim);
    sim.start_recording();
    let mut frames = Vec::new();
    for t in 0..180 {
        if t == 30 {
            sim.inject_input("nudge", "pressed"); // 录像里掺一条输入（走完整录制通道）
        }
        sim.step(&mut BurstAt60).unwrap();
        frames.push(frame_hash(&sim));
    }
    let rec = sim.stop_recording().unwrap();

    // burst 在 tick 60 写入：61 帧起画面必须与前一帧不同（爆发可见）、
    // 寿命 30 tick 过后那一帧爆发粒子已消失（与寿命期内任一帧都不同源——
    // 用"未触发的同 tick 平行世界"对照画面级生灭）
    let mut probe = Sim::new(11);
    particle_world(&mut probe);
    for _ in 0..180 {
        probe.step(&mut ()).unwrap(); // 不触发 burst 的平行世界
    }
    // 重放：必须逐校验点通过，且每帧画面哈希与首次运行逐位一致
    let mut sim2 = Sim::new(11);
    particle_world(&mut sim2);
    let mut replay_frames = Vec::new();
    sim2.replay_observed(&rec, &mut BurstAt60, |tick, world, _, _| {
        let buf = vitric_render::render_world(world, 96, 96, &Assets::empty(), tick).unwrap();
        replay_frames.push(fnv1a_64(&buf));
    })
    .unwrap();
    assert_eq!(frames, replay_frames, "重放的每一帧画面必须与首次运行逐字节一致");
}

#[test]
fn burst_is_visible_for_its_lifetime_in_rendered_frames() {
    let run_to = |trigger: bool, ticks: u64| -> u64 {
        let mut sim = Sim::new(11);
        particle_world(&mut sim);
        let mut logic_on = BurstAt60;
        for _ in 0..ticks {
            if trigger {
                sim.step(&mut logic_on).unwrap();
            } else {
                sim.step(&mut ()).unwrap();
            }
        }
        frame_hash(&sim)
    };
    // 触发前两个世界画面一致；爆发期内不同；寿命（30 tick）耗尽后重新一致——
    // 除 burst 字段外两个世界状态相同，画面差异只能来自爆发粒子本身。
    // 注意：burst 字段本身不画进画面，但寿命到期后值仍不同——画面一致恰好证明
    // "粒子消失"是字段值的纯函数推演，不靠渲染层记历史
    assert_eq!(run_to(true, 60), run_to(false, 60), "触发 tick 之前画面一致");
    assert_ne!(run_to(true, 61), run_to(false, 61), "爆发期画面必须可见地不同");
    assert_ne!(run_to(true, 89), run_to(false, 89), "寿命最后一刻（age 29）还在");
    assert_eq!(run_to(true, 90), run_to(false, 90), "寿命到期（age 30）当帧消失，画面归一");
}

#[test]
fn snapshot_restore_resumes_identical_particle_frames() {
    let mut sim = Sim::new(11);
    particle_world(&mut sim);
    let mut logic = BurstAt60;
    for _ in 0..70 {
        sim.step(&mut logic).unwrap(); // 在爆发进行中打快照（最刁钻的时刻）
    }
    let snap = sim.snapshot(&logic);

    for _ in 0..50 {
        sim.step(&mut logic).unwrap();
    }
    let direct = frame_hash(&sim);

    // restore 进新 sim 再跑同样的 tick 数：状态哈希和**画面**都必须一致
    let mut sim2 = Sim::new(0);
    let mut logic2 = BurstAt60;
    sim2.restore(&snap, &mut logic2).unwrap();
    assert_eq!(frame_hash(&sim2), {
        let mut probe = Sim::new(11);
        particle_world(&mut probe);
        let mut l = BurstAt60;
        for _ in 0..70 {
            probe.step(&mut l).unwrap();
        }
        frame_hash(&probe)
    }, "restore 当刻的画面 = 原轨迹同 tick 的画面（粒子无状态，自动正确）");
    for _ in 0..50 {
        sim2.step(&mut logic2).unwrap();
    }
    assert_eq!(sim2.world.state_hash(), sim.world.state_hash());
    assert_eq!(frame_hash(&sim2), direct, "回退重跑后的画面逐字节回到同一条轨迹");
}
