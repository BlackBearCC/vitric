use std::fmt;

use serde_json::json;

use vitric_ecs::World;
use vitric_rules::Event;

use crate::{InputRecord, Pcg32, Recording};

/// 模拟频率固定 60Hz。固定步长是确定性的前提：墙钟时间永远不进模拟。
pub const TICKS_PER_SECOND: u64 = 60;
pub const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

/// 状态哈希校验点间隔（tick）。
const CHECKPOINT_INTERVAL: u64 = 60;

/// 游戏逻辑挂载点。规则引擎和脚本层在运行时层包成一个 GameLogic 接进来；
/// sim 只负责确定性地「喂事件、推时间」，不认识规则和脚本。
pub trait GameLogic {
    fn on_tick(
        &mut self,
        world: &mut World,
        events: Vec<Event>,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<(), String>;
}

/// 空逻辑（纯物理跑模拟用）。
impl GameLogic for () {
    fn on_tick(&mut self, _: &mut World, _: Vec<Event>, _: &mut Pcg32, _: u64) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SimError {
    /// 游戏逻辑（规则/脚本）报错。
    Logic { tick: u64, message: String },
    /// 内建系统读到不合法的组件数据。
    BadComponent { tick: u64, entity: String, component: String, reason: String },
    /// 重放跑偏：状态哈希和录像对不上。
    ReplayDiverged { tick: u64, expected: u64, actual: u64 },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::Logic { tick, message } => {
                write!(f, "tick {tick}: 游戏逻辑报错: {message}")
            }
            SimError::BadComponent { tick, entity, component, reason } => write!(
                f,
                "tick {tick}: 实体 {entity} 的组件 {component} 数据不合法: {reason}。\
                 提示：内建系统要求 Position/Velocity 是 {{x,y}} 数字、Collider 是 {{w,h}} 数字"
            ),
            SimError::ReplayDiverged { tick, expected, actual } => write!(
                f,
                "重放在 tick {tick} 跑偏：期望哈希 {expected:#x}，实际 {actual:#x}。\
                 提示：检查这段时间内的逻辑是否引入了非确定性（墙钟时间、外部状态、未声明的随机）"
            ),
        }
    }
}

impl std::error::Error for SimError {}

/// 一次 step 的产出（控制面/调试用）。
#[derive(Debug, Default)]
pub struct StepReport {
    /// step 后的 tick 值。
    pub tick: u64,
    /// 本 tick 发给逻辑层的事件。
    pub events: Vec<Event>,
}

/// 确定性模拟器。
pub struct Sim {
    pub world: World,
    pub rng: Pcg32,
    pub tick: u64,
    seed: u64,
    pending_inputs: Vec<(String, String)>,
    recorder: Option<Recording>,
}

impl Sim {
    pub fn new(seed: u64) -> Sim {
        Sim {
            world: World::new(),
            rng: Pcg32::new(seed),
            tick: 0,
            seed,
            pending_inputs: Vec::new(),
            recorder: None,
        }
    }

    /// 注入一条输入（下一次 step 生效）。phase: "pressed" | "released"。
    pub fn inject_input(&mut self, action: &str, phase: &str) {
        self.pending_inputs.push((action.to_string(), phase.to_string()));
    }

    /// 开始录像（从当前状态起记）。
    pub fn start_recording(&mut self) {
        self.recorder = Some(Recording {
            seed: self.seed,
            checkpoints: vec![(self.tick, self.world.state_hash())],
            ..Default::default()
        });
    }

    /// 结束录像。
    pub fn stop_recording(&mut self) -> Option<Recording> {
        let mut rec = self.recorder.take()?;
        rec.ticks = self.tick;
        rec.final_hash = self.world.state_hash();
        Some(rec)
    }

    /// 推一帧。流水线（顺序固定，这就是确定性）：
    /// 1. 注入的输入 → input 事件（录像在此记录）
    /// 2. 内建运动系统：Position += Velocity * DT
    /// 3. 内建碰撞检测：AABB 重叠 → collision 事件
    /// 4. 游戏逻辑（规则 + 脚本）消化全部事件
    /// 5. tick + 1
    pub fn step(&mut self, logic: &mut dyn GameLogic) -> Result<StepReport, SimError> {
        let mut events = Vec::new();

        // 1. 输入
        for (action, phase) in std::mem::take(&mut self.pending_inputs) {
            if let Some(rec) = &mut self.recorder {
                rec.inputs.push(InputRecord {
                    tick: self.tick,
                    action: action.clone(),
                    phase: phase.clone(),
                });
            }
            events.push(Event::new("input", json!({"action": action, "phase": phase})));
        }

        // 2. 运动
        self.integrate_motion()?;

        // 3. 碰撞
        self.detect_collisions(&mut events)?;

        // 4. 逻辑
        logic
            .on_tick(&mut self.world, events.clone(), &mut self.rng, self.tick)
            .map_err(|message| SimError::Logic { tick: self.tick, message })?;

        // 5. 时间前进 + 录像校验点
        self.tick += 1;
        if let Some(rec) = &mut self.recorder {
            if self.tick % CHECKPOINT_INTERVAL == 0 {
                rec.checkpoints.push((self.tick, self.world.state_hash()));
            }
        }

        Ok(StepReport { tick: self.tick, events })
    }

    /// 重放一段录像并逐校验点比对。调用前 world 必须处于录像起点状态
    /// （同一份项目数据实例化出来的世界天然满足）。
    pub fn replay(&mut self, rec: &Recording, logic: &mut dyn GameLogic) -> Result<(), SimError> {
        // 起点校验
        if let Some(&(t0, h0)) = rec.checkpoints.first() {
            let actual = self.world.state_hash();
            if self.tick != t0 || actual != h0 {
                return Err(SimError::ReplayDiverged { tick: self.tick, expected: h0, actual });
            }
        }
        let mut cp = rec.checkpoints.iter().skip(1).peekable();
        while self.tick < rec.ticks {
            for input in rec.inputs_at(self.tick) {
                self.inject_input(&input.action, &input.phase);
            }
            self.step(logic)?;
            if let Some(&&(t, expected)) = cp.peek() {
                if self.tick == t {
                    cp.next();
                    let actual = self.world.state_hash();
                    if actual != expected {
                        return Err(SimError::ReplayDiverged { tick: t, expected, actual });
                    }
                }
            }
        }
        let actual = self.world.state_hash();
        if actual != rec.final_hash {
            return Err(SimError::ReplayDiverged {
                tick: self.tick,
                expected: rec.final_hash,
                actual,
            });
        }
        Ok(())
    }

    /// 模拟器整体快照（世界 + 时间 + 随机数状态）。
    pub fn snapshot(&self) -> serde_json::Value {
        json!({
            "tick": self.tick,
            "seed": self.seed,
            "rng": serde_json::to_value(&self.rng).expect("rng 可序列化"),
            "world": self.world.snapshot(),
        })
    }

    /// 从快照恢复。
    pub fn restore(&mut self, snap: &serde_json::Value) -> Result<(), String> {
        let tick = snap.get("tick").and_then(|v| v.as_u64()).ok_or("快照缺 tick")?;
        let seed = snap.get("seed").and_then(|v| v.as_u64()).ok_or("快照缺 seed")?;
        let rng: Pcg32 = serde_json::from_value(snap.get("rng").cloned().ok_or("快照缺 rng")?)
            .map_err(|e| format!("rng 解析失败: {e}"))?;
        let world_snap = snap.get("world").ok_or("快照缺 world")?;
        let mut world = World::new();
        world.restore(world_snap).map_err(|e| e.to_string())?;
        self.tick = tick;
        self.seed = seed;
        self.rng = rng;
        self.world = world;
        self.pending_inputs.clear();
        Ok(())
    }

    // ---- 内建系统 ----

    fn integrate_motion(&mut self) -> Result<(), SimError> {
        for id in self.world.query(&["Position", "Velocity"]) {
            let vx = self.num_field(id, "Velocity", "x")?;
            let vy = self.num_field(id, "Velocity", "y")?;
            let px = self.num_field(id, "Position", "x")?;
            let py = self.num_field(id, "Position", "y")?;
            self.world
                .set_field(id, "Position.x", json!(px + vx * DT))
                .expect("字段刚读过必然存在");
            self.world
                .set_field(id, "Position.y", json!(py + vy * DT))
                .expect("字段刚读过必然存在");
        }
        Ok(())
    }

    fn detect_collisions(&mut self, events: &mut Vec<Event>) -> Result<(), SimError> {
        let ids = self.world.query(&["Position", "Collider"]);
        let mut boxes = Vec::with_capacity(ids.len());
        for &id in &ids {
            let x = self.num_field(id, "Position", "x")?;
            let y = self.num_field(id, "Position", "y")?;
            let w = self.num_field(id, "Collider", "w")?;
            let h = self.num_field(id, "Collider", "h")?;
            boxes.push((id, x, y, w, h));
        }
        for i in 0..boxes.len() {
            for j in (i + 1)..boxes.len() {
                let (a, ax, ay, aw, ah) = boxes[i];
                let (b, bx, by, bw, bh) = boxes[j];
                let overlap =
                    (ax - bx).abs() * 2.0 < (aw + bw) && (ay - by).abs() * 2.0 < (ah + bh);
                if overlap {
                    events.push(Event::new(
                        "collision",
                        json!({"a": a.to_string(), "b": b.to_string()}),
                    ));
                }
            }
        }
        Ok(())
    }

    fn num_field(&self, id: vitric_ecs::EntityId, comp: &str, field: &str) -> Result<f64, SimError> {
        let path = format!("{comp}.{field}");
        let v = self.world.get_field(id, &path).map_err(|e| SimError::BadComponent {
            tick: self.tick,
            entity: id.to_string(),
            component: comp.to_string(),
            reason: e.to_string(),
        })?;
        v.as_f64().ok_or_else(|| SimError::BadComponent {
            tick: self.tick,
            entity: id.to_string(),
            component: comp.to_string(),
            reason: format!("{path} 不是数字: {v}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn moving_world(sim: &mut Sim) {
        let e = sim.world.spawn_named("mover").unwrap();
        sim.world.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(e, "Velocity", json!({"x": 60.0, "y": 0.0})).unwrap();
        sim.world.set_component(e, "Collider", json!({"w": 1.0, "h": 1.0})).unwrap();
        let wall = sim.world.spawn_named("wall").unwrap();
        sim.world.set_component(wall, "Position", json!({"x": 5.0, "y": 0.0})).unwrap();
        sim.world.set_component(wall, "Collider", json!({"w": 1.0, "h": 1.0})).unwrap();
    }

    /// 收集事件的测试逻辑。
    struct Collect(Vec<Event>);
    impl GameLogic for Collect {
        fn on_tick(&mut self, _: &mut World, ev: Vec<Event>, _: &mut Pcg32, _: u64) -> Result<(), String> {
            self.0.extend(ev);
            Ok(())
        }
    }

    #[test]
    fn motion_and_collision_pipeline() {
        let mut sim = Sim::new(1);
        moving_world(&mut sim);
        let mut logic = Collect(Vec::new());
        // 60 tick = 1 秒，mover 走 60 单位，必然穿过 wall 产生碰撞
        for _ in 0..60 {
            sim.step(&mut logic).unwrap();
        }
        let mover = sim.world.entity("mover").unwrap();
        let x = sim.world.get_field(mover, "Position.x").unwrap().as_f64().unwrap();
        assert!((x - 60.0).abs() < 1e-9, "60 tick 后应在 x=60，实际 {x}");
        assert!(
            logic.0.iter().any(|e| e.name == "collision"),
            "穿过墙必须产生 collision 事件"
        );
    }

    #[test]
    fn same_seed_same_inputs_same_hash() {
        let run = || {
            let mut sim = Sim::new(99);
            moving_world(&mut sim);
            let mut logic = Collect(Vec::new());
            for t in 0..120 {
                if t == 30 {
                    sim.inject_input("jump", "pressed");
                }
                // 逻辑里掺随机数，验证 RNG 也确定
                let _ = sim.rng.next_f64();
                sim.step(&mut logic).unwrap();
            }
            sim.world.state_hash()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn record_then_replay_verifies() {
        // 录一局
        let mut sim = Sim::new(7);
        moving_world(&mut sim);
        sim.start_recording();
        let mut logic = ();
        for t in 0..150 {
            if t % 40 == 0 {
                sim.inject_input("fire", "pressed");
            }
            sim.step(&mut logic).unwrap();
        }
        let rec = sim.stop_recording().unwrap();
        assert_eq!(rec.ticks, 150);
        assert!(rec.checkpoints.len() >= 2, "应有周期性校验点");
        assert_eq!(rec.inputs.len(), 4);

        // 同样初始条件重放：通过
        let mut sim2 = Sim::new(7);
        moving_world(&mut sim2);
        sim2.replay(&rec, &mut ()).unwrap();
        assert_eq!(sim2.world.state_hash(), rec.final_hash);

        // 初始状态被篡改：起点就报跑偏
        let mut sim3 = Sim::new(7);
        moving_world(&mut sim3);
        let m = sim3.world.entity("mover").unwrap();
        sim3.world.set_field(m, "Position.x", json!(0.5)).unwrap();
        let err = sim3.replay(&rec, &mut ()).unwrap_err();
        assert!(matches!(err, SimError::ReplayDiverged { tick: 0, .. }), "{err}");
    }

    #[test]
    fn replay_detects_midway_divergence() {
        /// 在 tick 70 偷偷改世界的「非确定性」逻辑
        struct Saboteur;
        impl GameLogic for Saboteur {
            fn on_tick(&mut self, w: &mut World, _: Vec<Event>, _: &mut Pcg32, tick: u64) -> Result<(), String> {
                if tick == 70 {
                    let m = w.entity("mover").map_err(|e| e.to_string())?;
                    w.set_field(m, "Position.y", json!(999.0)).map_err(|e| e.to_string())?;
                }
                Ok(())
            }
        }
        let mut sim = Sim::new(7);
        moving_world(&mut sim);
        sim.start_recording();
        for _ in 0..150 {
            sim.step(&mut ()).unwrap();
        }
        let rec = sim.stop_recording().unwrap();

        let mut sim2 = Sim::new(7);
        moving_world(&mut sim2);
        let err = sim2.replay(&rec, &mut Saboteur).unwrap_err();
        // 跑偏发生在 tick 70，应被 tick 120 的校验点逮住（而不是拖到最后）
        match err {
            SimError::ReplayDiverged { tick, .. } => {
                assert_eq!(tick, 120, "应在第一个覆盖 70 的校验点（120）发现");
            }
            other => panic!("错误类型不对: {other}"),
        }
    }

    #[test]
    fn snapshot_restore_resumes_identically() {
        let mut sim = Sim::new(3);
        moving_world(&mut sim);
        for _ in 0..50 {
            let _ = sim.rng.next_u32();
            sim.step(&mut ()).unwrap();
        }
        let snap = sim.snapshot();

        // 继续跑 50 tick
        for _ in 0..50 {
            let _ = sim.rng.next_u32();
            sim.step(&mut ()).unwrap();
        }
        let h_direct = sim.world.state_hash();

        // 从快照恢复再跑 50 tick：必须一模一样
        let mut sim2 = Sim::new(0); // 故意用错种子，restore 必须完整覆盖
        sim2.restore(&snap).unwrap();
        for _ in 0..50 {
            let _ = sim2.rng.next_u32();
            sim2.step(&mut ()).unwrap();
        }
        assert_eq!(sim2.world.state_hash(), h_direct);
        assert_eq!(sim2.tick, sim.tick);
    }

    #[test]
    fn bad_component_data_is_explicit() {
        let mut sim = Sim::new(1);
        let e = sim.world.spawn().to_owned();
        sim.world.set_component(e, "Position", json!({"x": "oops", "y": 0.0})).unwrap();
        sim.world.set_component(e, "Velocity", json!({"x": 1.0, "y": 0.0})).unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Position") && msg.contains("不是数字"), "{msg}");
        // 防御性检查 Value 类型躲不过：错误必须指到具体字段
        let _: Value = json!(null);
    }
}
