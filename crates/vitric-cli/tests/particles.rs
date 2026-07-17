//! Particle emitter end-to-end determinism: particles are a render-layer pure function (do not
//! enter simulation state), so after recording replay / snapshot rollback the **frame** must also
//! return bit-by-bit to the same trajectory — here we lock this invariant by composing the sim
//! and render layers (unit tests only look at one layer each).

use serde_json::json;

use vitric_ecs::fnv1a_64;
use vitric_render::Assets;
use vitric_sim::{GameLogic, Pcg32, Sim};

/// Test arena: a moving entity (so state really evolves) + a stream spark + a burst to trigger.
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

/// Deterministic logic that triggers the burst at tick 60 (simulates "rule writes the current
/// tick into the burst field").
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
    // Record one run: capture one frame hash per tick
    let mut sim = Sim::new(11);
    particle_world(&mut sim);
    sim.start_recording();
    let mut frames = Vec::new();
    for t in 0..180 {
        if t == 30 {
            sim.inject_input("nudge", "pressed"); // mix an input into the recording (full record channel)
        }
        sim.step(&mut BurstAt60).unwrap();
        frames.push(frame_hash(&sim));
    }
    let rec = sim.stop_recording().unwrap();

    // burst is written at tick 60: from frame 61 the frame must differ from the previous one
    // (burst visible); after the 30-tick lifetime the burst particles are gone on that frame
    // (different from any frame during the lifetime — use a "parallel world at the same tick
    // without trigger" to compare frame-level birth/death)
    let mut probe = Sim::new(11);
    particle_world(&mut probe);
    for _ in 0..180 {
        probe.step(&mut ()).unwrap(); // parallel world without triggering the burst
    }
    // Replay: must pass every checkpoint, and each frame hash must be bit-identical to the first
    // run
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
    // Before trigger the two worlds' frames are identical; during the burst they differ; after
    // the lifetime (30 ticks) they are identical again — except for the burst field the two
    // worlds' state is the same, the frame difference can only come from the burst particles
    // themselves.
    // Note: the burst field itself is not drawn into the frame, but its value still differs
    // after the lifetime expires — the identical frames exactly prove "particle disappearance" is
    // a pure-function inference of the field value, not relying on the render layer remembering
    // history
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
        sim.step(&mut logic).unwrap(); // snapshot mid-burst (the trickiest moment)
    }
    let snap = sim.snapshot(&logic);

    for _ in 0..50 {
        sim.step(&mut logic).unwrap();
    }
    let direct = frame_hash(&sim);

    // restore into a new sim and run the same number of ticks: state hash and **frame** must
    // both match
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
