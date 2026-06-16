use std::collections::BTreeMap;
use std::fmt;

use serde_json::json;

use vitric_ecs::World;
use vitric_rules::Event;

use crate::tween::{tween_value, Ease};
use crate::{InputRecord, Pcg32, Recording, ReplyRecord};

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

    /// 取走本 tick 逻辑层（规则/脚本）发出的事件副本，供控制面事件日志观测。
    /// 没有可观测事件的实现用默认空集即可。
    fn drain_observed(&mut self) -> Vec<Event> {
        Vec::new()
    }

    /// 热重载逻辑层（规则/脚本从磁盘换新，世界状态不动）。
    /// 成功返回重载摘要；失败必须保持旧逻辑原样可用。
    fn reload(&mut self) -> Result<serde_json::Value, String> {
        Err("该运行时不支持热重载".to_string())
    }

    /// 逻辑层跨 tick 暂存的自有状态（如还没消化的事件）。
    /// 不进快照的状态 = restore 后轨迹静默分歧，有暂存状态的实现必须实现这对钩子。
    fn snapshot_state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// 与 [`GameLogic::snapshot_state`] 配对恢复。
    fn restore_state(&mut self, _snap: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// 本逻辑当前「能干啥」的输入动作词汇：`(动作名, 它出现过的 phase 列表)`。
    /// 控制面 describe 拿它告诉 agent「画面里有啥」之外还有「你能按什么」——和试玩
    /// SceneView 的 affordance 合一。默认空：只有规则类逻辑（Runtime）够得到规则、能返非空；
    /// sim 持的是 `dyn GameLogic`，够不到具体规则引擎，所以这条钩子由实现方按需重写。
    fn available_actions(&self) -> Vec<(String, Vec<String>)> {
        Vec::new()
    }
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
    /// 已注入未消化的外部回复（LLM 回复等）。与 pending_inputs 同级的
    /// 第二条录制通道：进 step 时变事件 + 进录像，进快照。
    pending_replies: Vec<(String, serde_json::Value)>,
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
            pending_replies: Vec::new(),
            recorder: None,
        }
    }

    /// 这局的种子（构造时定，restore 会随快照覆盖）。手工攒录像时要拿它当 `Recording.seed`。
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// 注入一条输入（下一次 step 生效）。phase: "pressed" | "released"。
    pub fn inject_input(&mut self, action: &str, phase: &str) {
        self.pending_inputs.push((action.to_string(), phase.to_string()));
    }

    /// 注入一条外部回复（下一次 step 变成事件，名为 `name`、data 为 `data`）。
    /// 这是 LLM 等异步外部内容进入模拟的**唯一**正路：和输入一样被录像记录、
    /// 重放时按原 tick 重新注入——所以带 LLM 内容的录像离线重放逐位一致。
    /// 约定 `data` 是 JSON 对象（非对象会被 Event 丢成空对象，调用方自己保证）。
    pub fn inject_reply(&mut self, name: &str, data: serde_json::Value) {
        self.pending_replies.push((name.to_string(), data));
    }

    /// 开始录像（从当前状态起记）。
    pub fn start_recording(&mut self) {
        self.recorder = Some(Recording {
            seed: self.seed,
            checkpoints: vec![(self.tick, self.world.state_hash())],
            ..Default::default()
        });
    }

    /// 是否正在录像。录像只记输入流：录制期间任何绕过输入的状态修改
    /// （RPC 改世界、检查器拖拽、restore）都会让录像不可重放，调用方必须先查这个再动手。
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// 结束录像。
    pub fn stop_recording(&mut self) -> Option<Recording> {
        let mut rec = self.recorder.take()?;
        rec.ticks = self.tick;
        rec.final_hash = self.world.state_hash();
        Some(rec)
    }

    /// 推一帧。流水线（顺序固定，这就是确定性）：
    /// 1. 注入的输入 → input 事件，注入的外部回复 → 同名事件（录像都在此记录）
    /// 2. 内建重力：Body 实体 Velocity.y += gravity * DT
    /// 3. 内建运动系统：Position += Velocity * DT（带 Body+Collider 的实体被 Solid 挡停）
    /// 4. 游戏感内建系统（跑在运动之后，相机看的是本 tick 的最终位置）：
    ///    相机跟随 → 抖动衰减 → 粒子寿命 → 补间。补间（Tween 组件）跑在全部
    ///    内建写字段系统之后：补间盯上的字段以补间为准（运动积分/相机跟随写的
    ///    值被本 tick 的补间值覆盖），碰撞检测和渲染看到的是补间后的最终值
    /// 5. 内建碰撞检测：AABB 重叠 → collision 事件
    /// 6. 游戏逻辑（规则 + 脚本）消化全部事件
    /// 7. tick + 1
    pub fn step(&mut self, logic: &mut dyn GameLogic) -> Result<StepReport, SimError> {
        let mut events = Vec::new();

        // 0. 世界的第一个 tick 发 start 事件——初始化规则的标准入口
        if self.tick == 0 {
            events.push(Event::new("start", json!({})));
        }

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

        // 1.5 外部回复（固定排在输入之后，重放按同一顺序注入才逐位一致）
        for (name, data) in std::mem::take(&mut self.pending_replies) {
            if let Some(rec) = &mut self.recorder {
                rec.replies.push(ReplyRecord {
                    tick: self.tick,
                    name: name.clone(),
                    data: data.clone(),
                });
            }
            events.push(Event::new(&name, data));
        }

        // 2. 重力 + 运动
        self.apply_gravity()?;
        self.integrate_motion()?;

        // 3. 游戏感：相机跟随在运动之后（不滞后一帧）；抖动衰减在逻辑之前
        //    （规则本 tick 设的 amplitude 第一帧按原值渲染，衰减从下一 tick 开始）；
        //    粒子在碰撞检测之前销毁（死粒子不再发 collision 事件）
        self.follow_camera()?;
        self.decay_shake()?;
        self.age_particles()?;
        self.advance_tweens(&mut events)?;

        // 4. 碰撞
        self.detect_collisions(&mut events)?;

        // 5. 逻辑
        logic
            .on_tick(&mut self.world, events.clone(), &mut self.rng, self.tick)
            .map_err(|message| SimError::Logic { tick: self.tick, message })?;

        // 6. 时间前进 + 录像校验点
        self.tick += 1;
        if let Some(rec) = &mut self.recorder {
            if self.tick.is_multiple_of(CHECKPOINT_INTERVAL) {
                rec.checkpoints.push((self.tick, self.world.state_hash()));
            }
        }

        Ok(StepReport { tick: self.tick, events })
    }

    /// 重放一段录像并逐校验点比对。调用前 world 必须处于录像起点状态
    /// （同一份项目数据实例化出来的世界天然满足）。
    pub fn replay(&mut self, rec: &Recording, logic: &mut dyn GameLogic) -> Result<(), SimError> {
        self.replay_observed(rec, logic, |_, _, _, _| {})
    }

    /// 重放 + 逐 tick 观察。每个 tick 推完后把观察窗口交给 `observe`：
    /// `(tick, 世界, 本 tick 喂给逻辑的事件, 逻辑层 emit 的事件)`。
    /// `vitric gate` 用它在重放中收集事件、跑断言——观察者**只许看不许写**，
    /// 写了世界，下一个校验点哈希必然跑偏（这正是录像作为交付证书不可伪造的根基）。
    pub fn replay_observed(
        &mut self,
        rec: &Recording,
        logic: &mut dyn GameLogic,
        mut observe: impl FnMut(u64, &World, &[Event], Vec<Event>),
    ) -> Result<(), SimError> {
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
            // 外部回复从录像取（绝不重新调网络），注入顺序与录制时一致：输入在前回复在后
            for reply in rec.replies_at(self.tick) {
                self.inject_reply(&reply.name, reply.data.clone());
            }
            let report = self.step(logic)?;
            let observed = logic.drain_observed();
            observe(self.tick, &self.world, &report.events, observed);
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
    pub fn snapshot(&self, logic: &dyn GameLogic) -> serde_json::Value {
        json!({
            "tick": self.tick,
            "seed": self.seed,
            "rng": serde_json::to_value(&self.rng).expect("rng 可序列化"),
            "world": self.world.snapshot(),
            // 已注入未消化的输入/外部回复。漏掉任何一个，restore 后它们凭空消失，轨迹静默分歧
            "pending_inputs": self.pending_inputs,
            "pending_replies": self.pending_replies,
            // 逻辑层暂存状态（脚本上一 tick 发的事件等）
            "logic": logic.snapshot_state(),
        })
    }

    /// 从快照恢复。
    pub fn restore(
        &mut self,
        snap: &serde_json::Value,
        logic: &mut dyn GameLogic,
    ) -> Result<(), String> {
        let tick = snap.get("tick").and_then(|v| v.as_u64()).ok_or("快照缺 tick")?;
        let seed = snap.get("seed").and_then(|v| v.as_u64()).ok_or("快照缺 seed")?;
        let rng: Pcg32 = serde_json::from_value(snap.get("rng").cloned().ok_or("快照缺 rng")?)
            .map_err(|e| format!("rng 解析失败: {e}"))?;
        let world_snap = snap.get("world").ok_or("快照缺 world")?;
        let mut world = World::new();
        world.restore(world_snap).map_err(|e| e.to_string())?;
        let pending: Vec<(String, String)> = serde_json::from_value(
            snap.get("pending_inputs").cloned().ok_or("快照缺 pending_inputs")?,
        )
        .map_err(|e| format!("pending_inputs 解析失败: {e}"))?;
        // 显式报缺：旧版快照没有 pending_replies，静默补空会掩盖版本不兼容
        let pending_replies: Vec<(String, serde_json::Value)> = serde_json::from_value(
            snap.get("pending_replies")
                .cloned()
                .ok_or("快照缺 pending_replies（旧版快照与当前版本不兼容，重新 sim/snapshot）")?,
        )
        .map_err(|e| format!("pending_replies 解析失败: {e}"))?;
        logic.restore_state(snap.get("logic").ok_or("快照缺 logic 状态")?)?;
        self.tick = tick;
        self.seed = seed;
        self.rng = rng;
        self.world = world;
        self.pending_inputs = pending;
        self.pending_replies = pending_replies;
        // 时间线断了，进行中的录像必然不可重放——直接作废，不留静默损坏的录像
        self.recorder = None;
        Ok(())
    }

    // ---- 内建系统 ----

    /// 重力：Body 实体每 tick 给 Velocity.y 加 gravity * DT（世界 y 朝上，重力通常是负数）。
    fn apply_gravity(&mut self) -> Result<(), SimError> {
        for id in self.world.query(&["Body", "Velocity"]) {
            let g = self.num_field(id, "Body", "gravity")?;
            if g == 0.0 {
                continue;
            }
            let vy = self.num_field(id, "Velocity", "y")?;
            self.world
                .set_field(id, "Velocity.y", json!(vy + g * DT))
                .expect("字段刚读过必然存在");
        }
        Ok(())
    }

    fn integrate_motion(&mut self) -> Result<(), SimError> {
        // Solid = 挡停体（地面/墙）。带 Body+Collider 的实体撞上会被裁剪到贴边并清掉该轴速度。
        let solid_ids = self.world.query(&["Solid", "Position", "Collider"]);
        let mut solids = Vec::with_capacity(solid_ids.len());
        for &sid in &solid_ids {
            solids.push((
                sid,
                self.num_field(sid, "Position", "x")?,
                self.num_field(sid, "Position", "y")?,
                self.num_field(sid, "Collider", "w")?,
                self.num_field(sid, "Collider", "h")?,
            ));
        }

        for id in self.world.query(&["Position", "Velocity"]) {
            let mut vx = self.num_field(id, "Velocity", "x")?;
            let mut vy = self.num_field(id, "Velocity", "y")?;
            let px = self.num_field(id, "Position", "x")?;
            let py = self.num_field(id, "Position", "y")?;

            let is_phys = self.world.get_component(id, "Body").is_ok()
                && self.world.get_component(id, "Collider").is_ok();
            if !is_phys || solids.is_empty() {
                self.world
                    .set_field(id, "Position.x", json!(px + vx * DT))
                    .expect("字段刚读过必然存在");
                self.world
                    .set_field(id, "Position.y", json!(py + vy * DT))
                    .expect("字段刚读过必然存在");
                continue;
            }

            let w = self.num_field(id, "Collider", "w")?;
            let h = self.num_field(id, "Collider", "h")?;
            // 轴分离：先走 x 再走 y，每轴撞上就贴边停（中心坐标）。
            // 重叠判定用 penetrates（带相对余量），不是 collision 事件的严格 <：
            // 贴边 snap（如 ny = sy + (sh+h)/2）的浮点加法在大坐标下会舍掉最多
            // 一个 ULP，写回的位置比精确接触深一丝。严格 < 会把这种「站着」
            // 误判成穿透——下一 tick 的 x 轴判定先命中，把站在平台上的实体
            // 横着弹出去。余量见 penetrates 的注释。
            // 注意：单 tick 位移大于障碍厚度会穿过去（无扫掠），速度预算要留余量。
            let mut nx = px + vx * DT;
            for &(sid, sx, sy, sw, sh) in &solids {
                if sid == id {
                    continue;
                }
                let overlap = penetrates(nx, sx, w + sw) && penetrates(py, sy, h + sh);
                if overlap {
                    nx = if vx > 0.0 { sx - (sw + w) / 2.0 } else { sx + (sw + w) / 2.0 };
                    vx = 0.0;
                }
            }
            let mut ny = py + vy * DT;
            let mut grounded = false;
            for &(sid, sx, sy, sw, sh) in &solids {
                if sid == id {
                    continue;
                }
                let overlap = penetrates(nx, sx, w + sw) && penetrates(ny, sy, h + sh);
                if overlap {
                    if vy <= 0.0 {
                        ny = sy + (sh + h) / 2.0; // 落在顶面
                        grounded = true;
                    } else {
                        ny = sy - (sh + h) / 2.0; // 顶到底面
                    }
                    vy = 0.0;
                }
            }
            self.world.set_field(id, "Position.x", json!(nx)).expect("字段刚读过必然存在");
            self.world.set_field(id, "Position.y", json!(ny)).expect("字段刚读过必然存在");
            self.world.set_field(id, "Velocity.x", json!(vx)).expect("字段刚读过必然存在");
            self.world.set_field(id, "Velocity.y", json!(vy)).expect("字段刚读过必然存在");
            self.world.set_field(id, "Body.grounded", json!(grounded)).map_err(|e| {
                SimError::BadComponent {
                    tick: self.tick,
                    entity: id.to_string(),
                    component: "Body".to_string(),
                    reason: format!("写 grounded 失败: {e}。Body 组件需要 gravity(number) 和 grounded(bool) 两个字段"),
                }
            })?;
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

    /// 相机跟随：`Camera.follow` 指名实体（可选字段，缺省/空串=不跟随），
    /// 每 tick 把 Camera.x/y 按 `lerp` 比例（0..=1，1=硬锁定）拉向目标 Position。
    /// follow 指向不存在的实体直接报错——静默跳过会让「相机不动」极难排查。
    fn follow_camera(&mut self) -> Result<(), SimError> {
        for id in self.world.query(&["Camera"]) {
            let Ok(follow) = self.world.get_field(id, "Camera.follow") else {
                continue; // 没定义 follow 字段 = 不跟随（可选约定）
            };
            let name = follow
                .as_str()
                .ok_or_else(|| SimError::BadComponent {
                    tick: self.tick,
                    entity: id.to_string(),
                    component: "Camera".to_string(),
                    reason: format!("follow 必须是文本（要跟随的实体名，空串=不跟随），拿到 {follow}"),
                })?
                .to_string();
            if name.is_empty() {
                continue;
            }
            let target = self.world.entity(&name).map_err(|_| SimError::BadComponent {
                tick: self.tick,
                entity: id.to_string(),
                component: "Camera".to_string(),
                reason: format!(
                    "follow 指向的实体 {name:?} 不存在。\
                     提示：填场景里实体的名字，或设为空串 \"\" 关掉跟随"
                ),
            })?;
            let lerp = self.num_field(id, "Camera", "lerp").map_err(|e| match e {
                SimError::BadComponent { tick, entity, component, reason } => {
                    SimError::BadComponent {
                        tick,
                        entity,
                        component,
                        reason: format!(
                            "{reason}。提示：设置了 follow 的相机还需要 lerp 字段\
                             （number，0..=1，每 tick 逼近比例，1=硬锁定）"
                        ),
                    }
                }
                other => other,
            })?;
            if !(0.0..=1.0).contains(&lerp) {
                return Err(SimError::BadComponent {
                    tick: self.tick,
                    entity: id.to_string(),
                    component: "Camera".to_string(),
                    reason: format!("lerp 必须在 0..=1（每 tick 逼近比例，1=硬锁定），拿到 {lerp}"),
                });
            }
            let tx = self.num_field(target, "Position", "x")?;
            let ty = self.num_field(target, "Position", "y")?;
            let cx = self.num_field(id, "Camera", "x")?;
            let cy = self.num_field(id, "Camera", "y")?;
            self.world
                .set_field(id, "Camera.x", json!(cx + (tx - cx) * lerp))
                .expect("字段刚读过必然存在");
            self.world
                .set_field(id, "Camera.y", json!(cy + (ty - cy) * lerp))
                .expect("字段刚读过必然存在");
        }
        Ok(())
    }

    /// 屏幕抖动衰减：`Shake.amplitude` 每 tick 乘 `decay`，写回组件（快照/回放安全）。
    /// 偏移本身在渲染层算——(tick, amplitude) 的纯函数（vitric-render 的 shake_offset），
    /// 不碰模拟的 RNG 流：抖不抖屏对 gameplay 轨迹零影响。
    fn decay_shake(&mut self) -> Result<(), SimError> {
        for id in self.world.query(&["Shake"]) {
            let amp = self.num_field(id, "Shake", "amplitude")?;
            if amp <= 0.0 {
                continue;
            }
            let decay = self.num_field(id, "Shake", "decay")?;
            if !(0.0..=1.0).contains(&decay) {
                return Err(SimError::BadComponent {
                    tick: self.tick,
                    entity: id.to_string(),
                    component: "Shake".to_string(),
                    reason: format!("decay 必须在 0..=1（每 tick 衰减系数），拿到 {decay}"),
                });
            }
            // 乘法衰减永远到不了 0：低于千分之一直接归零，停掉肉眼不可见的状态抖动
            let next = amp * decay;
            let next = if next < 1e-3 { 0.0 } else { next };
            self.world
                .set_field(id, "Shake.amplitude", json!(next))
                .expect("字段刚读过必然存在");
        }
        Ok(())
    }

    /// 粒子寿命：`Particle.ttl`（剩余 tick 数，整数）每 tick 减 1，到 0 当场销毁
    /// （销毁顺序 = 槽位序，确定性）。生成端 spawn 完（Sprite+Velocity+Particle）就能不管。
    fn age_particles(&mut self) -> Result<(), SimError> {
        for id in self.world.query(&["Particle"]) {
            let ttl = self
                .world
                .get_field(id, "Particle.ttl")
                .map_err(|e| SimError::BadComponent {
                    tick: self.tick,
                    entity: id.to_string(),
                    component: "Particle".to_string(),
                    reason: e.to_string(),
                })?
                .as_i64()
                .ok_or_else(|| SimError::BadComponent {
                    tick: self.tick,
                    entity: id.to_string(),
                    component: "Particle".to_string(),
                    reason: "ttl 必须是整数（剩余存活 tick 数）".to_string(),
                })?;
            if ttl - 1 <= 0 {
                self.world.despawn(id).expect("query 给出的实体必然活着");
            } else {
                self.world
                    .set_field(id, "Particle.ttl", json!(ttl - 1))
                    .expect("字段刚读过必然存在");
            }
        }
        Ok(())
    }

    /// 补间：`Tween` 组件（独立实体，由场景文件声明或规则/脚本 spawn）把目标实体的
    /// 一个数字字段从 from 平滑插到 to。状态全在组件里（进状态哈希、进存档），
    /// 第 elapsed tick 的值是 `from + (to-from)·ease(elapsed/duration)` 的解析式
    /// （见 [`crate::tween`]，禁累加积分——快照回退续播逐位一致）。
    ///
    /// 语义合同：
    /// - 第一次被处理的 tick 记下起跑点（`start` 从 -1 盖成当前 tick）并写起始值；
    /// - 到期 tick（elapsed == duration）把字段**精确**写成 to（不留浮点尾巴），
    ///   发 `tween-finished {id, target, field}` 事件，补间实体自动移除；
    /// - 同一目标实体同一字段同时只允许一个活跃补间：后来者顶掉前者（前者直接移除，
    ///   不发事件不报错——显式语义）。"后来"按起跑点判定，同 tick 并发以槽位序后者为准；
    /// - 跑在运动/相机跟随之后：补间盯上的字段本 tick 以补间值为准。
    fn advance_tweens(&mut self, events: &mut Vec<Event>) -> Result<(), SimError> {
        let ids = self.world.query(&["Tween"]);
        if ids.is_empty() {
            return Ok(());
        }
        struct Active {
            ent: vitric_ecs::EntityId,
            target: vitric_ecs::EntityId,
            field: String,
            from: f64,
            to: f64,
            duration: u64,
            ease: Ease,
            start: i64,
            event_id: String,
        }
        // 解析全部补间——任何数据问题都在动世界之前显式暴露
        let mut tweens: Vec<Active> = Vec::with_capacity(ids.len());
        for &id in &ids {
            let bad = |reason: String| SimError::BadComponent {
                tick: self.tick,
                entity: id.to_string(),
                component: "Tween".to_string(),
                reason,
            };
            let comp = self.world.get_component(id, "Tween").expect("query 给出").clone();
            let text = |key: &str| comp.get(key).and_then(|v| v.as_str()).map(String::from);
            let num = |key: &str| comp.get(key).and_then(|v| v.as_f64());
            let target_ref = text("target").ok_or_else(|| {
                bad("缺少 target（文本：目标实体的名字，或 e3v1 句柄）".to_string())
            })?;
            let field = text("field").ok_or_else(|| {
                bad("缺少 field（文本：目标字段路径，如 \"Position.x\"）".to_string())
            })?;
            if !field.contains('.') {
                return Err(bad(format!(
                    "field {field:?} 缺少字段路径。写法: \"组件.字段\"，如 \"Position.x\""
                )));
            }
            let from = num("from").ok_or_else(|| bad("缺少 from（数字：起始值）".to_string()))?;
            let to = num("to").ok_or_else(|| bad("缺少 to（数字：终值）".to_string()))?;
            let duration = comp
                .get("duration")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| bad("缺少 duration（整数：时长 tick 数）".to_string()))?;
            if duration < 1 {
                return Err(bad(format!("duration 必须 ≥ 1（tick），拿到 {duration}")));
            }
            let ease = match comp.get("ease") {
                None => Ease::Linear,
                Some(v) => {
                    let s = v.as_str().ok_or_else(|| {
                        bad(format!("ease 必须是文本（缓动曲线名），拿到 {v}"))
                    })?;
                    Ease::parse(s).map_err(&bad)?
                }
            };
            let start = comp.get("start").and_then(|v| v.as_i64()).unwrap_or(-1);
            if start > self.tick as i64 {
                return Err(bad(format!(
                    "start（{start}）超前当前 tick（{}）。start 由引擎在补间起跑时盖章，\
                     不要手填——留 -1 即可",
                    self.tick
                )));
            }
            let event_id = text("id").unwrap_or_default();
            // 目标解析：先按名字找，找不到再按句柄解析；都不行显式报错
            let target = match self.world.entity(&target_ref) {
                Ok(t) => t,
                Err(_) => match target_ref.parse::<vitric_ecs::EntityId>() {
                    Ok(h) if self.world.is_alive(h) => h,
                    _ => {
                        return Err(bad(format!(
                            "target 指向的实体 {target_ref:?} 不存在。\
                             提示：填目标实体的名字（或活句柄）；目标若已被销毁，\
                             先 despawn 补间实体再销毁目标"
                        )))
                    }
                },
            };
            tweens.push(Active { ent: id, target, field, from, to, duration: duration as u64, ease, start, event_id });
        }

        // 冲突裁决：同 (目标实体, 字段) 只留最新的一个，其余直接移除（顶掉语义）。
        // "最新" = 未起跑（start = -1，本 tick 才出现）优先于已起跑，已起跑比 start 大小，
        // 全平则槽位序后者为准——iter 本身就是槽位序，所以 ≥ 即顶掉。
        let mut winners: BTreeMap<(u32, String), usize> = BTreeMap::new();
        let mut losers: Vec<usize> = Vec::new();
        for (i, t) in tweens.iter().enumerate() {
            let key = (t.target.index, t.field.clone());
            let rank = |idx: usize| {
                let s = tweens[idx].start;
                (if s < 0 { i64::MAX } else { s }, idx)
            };
            match winners.get(&key).copied() {
                None => {
                    winners.insert(key, i);
                }
                Some(old) if rank(i) >= rank(old) => {
                    losers.push(old);
                    winners.insert(key, i);
                }
                Some(_) => losers.push(i),
            }
        }
        for &i in &losers {
            self.world.despawn(tweens[i].ent).expect("query 给出的实体必然活着");
        }
        losers.sort_unstable();

        // 应用（槽位序，确定性）
        for (i, t) in tweens.iter().enumerate() {
            if losers.binary_search(&i).is_ok() {
                continue;
            }
            let bad = |reason: String| SimError::BadComponent {
                tick: self.tick,
                entity: t.ent.to_string(),
                component: "Tween".to_string(),
                reason,
            };
            // 目标字段必须已存在且是数字——补间不创造字段，只动既有真相
            let cur = self
                .world
                .get_field(t.target, &t.field)
                .map_err(|e| bad(e.to_string()))?;
            if !cur.is_number() {
                return Err(bad(format!(
                    "目标字段 {} 不是数字（当前值 {cur}），补间只能动数字字段",
                    t.field
                )));
            }
            // 起跑盖章：start 从 -1 盖成当前 tick（进组件 = 进哈希进存档）
            let start = if t.start < 0 {
                let mut comp = self
                    .world
                    .get_component(t.ent, "Tween")
                    .expect("上面刚读过")
                    .clone();
                comp["start"] = json!(self.tick as i64);
                self.world.set_component(t.ent, "Tween", comp).expect("实体活着");
                self.tick
            } else {
                t.start as u64
            };
            let elapsed = self.tick - start;
            if elapsed >= t.duration {
                // 到期：精确终值 + 完成事件 + 自动移除
                self.world
                    .set_field(t.target, &t.field, json!(t.to))
                    .map_err(|e| bad(e.to_string()))?;
                events.push(Event::new(
                    "tween-finished",
                    json!({"id": t.event_id, "target": t.target.to_string(), "field": t.field}),
                ));
                self.world.despawn(t.ent).expect("实体活着");
            } else {
                let v = tween_value(t.from, t.to, t.ease, elapsed, t.duration);
                self.world
                    .set_field(t.target, &t.field, json!(v))
                    .map_err(|e| bad(e.to_string()))?;
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

/// 挡停体重叠判定：实体中心 a、固体中心 b、两者尺寸之和 span，
/// 穿透深度必须超过相对余量才算重叠。
///
/// 约束：余量必须跟坐标量级走。贴边 snap 的舍入误差是 ULP 级，而 ULP
/// 正比于坐标大小（y≈34 时约 7e-15，y≈1 时常为 0——所以弹飞 bug 只在
/// 大坐标出现）。固定绝对余量要么大坐标下不够用，要么小坐标下吞掉真位移。
/// 取 1e-9 × 量级：比 ULP（2.2e-16 × 量级）大六个数量级，稳压舍入误差；
/// 又比任何真实穿透（速度 × DT，最小也是 1e-3 级）小得多，不会吞掉真碰撞。
/// 低坐标场景下与原严格 < 判定结果一致，已有轨迹（含录像哈希）不变。
fn penetrates(a: f64, b: f64, span: f64) -> bool {
    let eps = 1e-9 * a.abs().max(b.abs()).max(1.0);
    span - (a - b).abs() * 2.0 > eps
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

    /// 平台物理测试场：一个带重力的角色 + 脚下地板 + 右侧墙。
    fn platformer_world(sim: &mut Sim) -> vitric_ecs::EntityId {
        let p = sim.world.spawn_named("hero").unwrap();
        sim.world.set_component(p, "Position", json!({"x": 0.0, "y": 5.0})).unwrap();
        sim.world.set_component(p, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(p, "Collider", json!({"w": 1.0, "h": 2.0})).unwrap();
        sim.world
            .set_component(p, "Body", json!({"gravity": -30.0, "grounded": false}))
            .unwrap();
        let floor = sim.world.spawn_named("floor").unwrap();
        sim.world.set_component(floor, "Position", json!({"x": 0.0, "y": -1.0})).unwrap();
        sim.world.set_component(floor, "Collider", json!({"w": 40.0, "h": 2.0})).unwrap();
        sim.world.set_component(floor, "Solid", json!({})).unwrap();
        let wall = sim.world.spawn_named("wall").unwrap();
        sim.world.set_component(wall, "Position", json!({"x": 6.0, "y": 2.0})).unwrap();
        sim.world.set_component(wall, "Collider", json!({"w": 2.0, "h": 4.0})).unwrap();
        sim.world.set_component(wall, "Solid", json!({})).unwrap();
        p
    }

    #[test]
    fn gravity_pulls_body_down_until_it_lands_grounded() {
        let mut sim = Sim::new(1);
        let p = platformer_world(&mut sim);
        // 自由落体若干 tick：速度向下累积
        for _ in 0..10 {
            sim.step(&mut ()).unwrap();
        }
        assert!(sim.world.get_field(p, "Velocity.y").unwrap().as_f64().unwrap() < 0.0);
        assert_eq!(sim.world.get_field(p, "Body.grounded").unwrap(), &json!(false));
        // 跑到落地：站在地板顶面（地板顶 0.0 + 半高 1.0），竖直速度清零，grounded
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        assert_eq!(sim.world.get_field(p, "Body.grounded").unwrap(), &json!(true));
        assert_eq!(sim.world.get_field(p, "Velocity.y").unwrap().as_f64(), Some(0.0));
        let py = sim.world.get_field(p, "Position.y").unwrap().as_f64().unwrap();
        assert!((py - 1.0).abs() < 1e-9, "应贴在地板顶面，实际 y={py}");
    }

    #[test]
    fn solid_wall_blocks_horizontal_motion() {
        let mut sim = Sim::new(1);
        let p = platformer_world(&mut sim);
        // 先落地，再向右冲墙
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        sim.world.set_field(p, "Velocity.x", json!(20.0)).unwrap();
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
            // 维持推进（撞墙会被清零，模拟持续按键的最坏情况）
            sim.world.set_field(p, "Velocity.x", json!(20.0)).unwrap();
        }
        // 墙左边缘 5.0，角色半宽 0.5 → 最远贴到 4.5
        let px = sim.world.get_field(p, "Position.x").unwrap().as_f64().unwrap();
        assert!((px - 4.5).abs() < 1e-9, "应贴墙停下，实际 x={px}");
    }

    #[test]
    fn jump_arc_rises_then_lands_back() {
        let mut sim = Sim::new(1);
        let p = platformer_world(&mut sim);
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        // 起跳 = 给一个向上的速度（规则里就是 set Velocity.y）
        sim.world.set_field(p, "Velocity.y", json!(12.0)).unwrap();
        sim.step(&mut ()).unwrap();
        assert_eq!(sim.world.get_field(p, "Body.grounded").unwrap(), &json!(false));
        let mut peak: f64 = 0.0;
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
            peak = peak.max(sim.world.get_field(p, "Position.y").unwrap().as_f64().unwrap());
        }
        assert!(peak > 2.0, "跳起来要离地，峰值 {peak}");
        assert_eq!(sim.world.get_field(p, "Body.grounded").unwrap(), &json!(true));
        let py = sim.world.get_field(p, "Position.y").unwrap().as_f64().unwrap();
        assert!((py - 1.0).abs() < 1e-9, "落回地板顶面，实际 y={py}");
    }

    /// 高台测试场：方块堆到顶面 y=top 的平台 + 一个 2.1 高的角色站在上面。
    /// 角色 2.1 高、平台块 2.0 高的组合让 snap 坐标（top + 1.05）带小数，
    /// 在大坐标下必然产生 ULP 舍入——这正是弹飞 bug 的触发形态。
    fn tall_platform_world(sim: &mut Sim, top: f64) -> vitric_ecs::EntityId {
        // 平台：一列 2.0 高的方块，最上面那块的顶面在 y=top
        let mut y = top - 1.0;
        while y > top - 7.0 {
            let t = sim.world.spawn();
            sim.world.set_component(t, "Position", json!({"x": 8.0, "y": y})).unwrap();
            sim.world.set_component(t, "Collider", json!({"w": 2.0, "h": 2.0})).unwrap();
            sim.world.set_component(t, "Solid", json!({})).unwrap();
            y -= 2.0;
        }
        // 旁边一堵墙（复刻实测场景：被弹飞的实体会沿墙列级联往上爬）
        let mut wy = top + 1.0;
        for _ in 0..4 {
            let wall = sim.world.spawn();
            sim.world.set_component(wall, "Position", json!({"x": 10.5, "y": wy})).unwrap();
            sim.world.set_component(wall, "Collider", json!({"w": 1.0, "h": 2.0})).unwrap();
            sim.world.set_component(wall, "Solid", json!({})).unwrap();
            wy += 2.0;
        }
        let hero = sim.world.spawn_named("hero").unwrap();
        // 从平台上方一点点落下，让 snap 路径真实走一遍
        sim.world.set_component(hero, "Position", json!({"x": 8.0, "y": top + 1.3})).unwrap();
        sim.world.set_component(hero, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(hero, "Collider", json!({"w": 1.0, "h": 2.1})).unwrap();
        sim.world
            .set_component(hero, "Body", json!({"gravity": -30.0, "grounded": false}))
            .unwrap();
        hero
    }

    /// 回归（弹飞 bug）：站在 top=33 的高台上，snap 写回的 y 比精确接触
    /// 低一个 ULP，旧的严格 < 判定下一 tick 把「站着」当穿透，x 轴先命中
    /// 把角色横着弹出去（实测从 top=33 飞到 (8.65, 47.05) 沿墙列级联上爬）。
    /// 修复后：120 tick 站着不动。
    #[test]
    fn standing_on_high_platform_is_stable() {
        let mut sim = Sim::new(1);
        let hero = tall_platform_world(&mut sim, 33.0);
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        let px = sim.world.get_field(hero, "Position.x").unwrap().as_f64().unwrap();
        let py = sim.world.get_field(hero, "Position.y").unwrap().as_f64().unwrap();
        assert_eq!(px, 8.0, "站着不该有任何横向位移，实际 x={px}");
        assert!((py - 34.05).abs() < 1e-9, "应贴在高台顶面 33+1.05，实际 y={py}");
        assert_eq!(sim.world.get_field(hero, "Body.grounded").unwrap(), &json!(true));
        assert_eq!(sim.world.get_field(hero, "Velocity.x").unwrap().as_f64(), Some(0.0));
    }

    /// 同一形态在低坐标（top=1）从来没出过事——锁住它继续没事。
    #[test]
    fn standing_on_low_platform_is_stable() {
        let mut sim = Sim::new(1);
        let hero = tall_platform_world(&mut sim, 1.0);
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        let px = sim.world.get_field(hero, "Position.x").unwrap().as_f64().unwrap();
        let py = sim.world.get_field(hero, "Position.y").unwrap().as_f64().unwrap();
        assert_eq!(px, 8.0, "站着不该有任何横向位移，实际 x={px}");
        assert!((py - 2.05).abs() < 1e-9, "应贴在平台顶面 1+1.05，实际 y={py}");
        assert_eq!(sim.world.get_field(hero, "Body.grounded").unwrap(), &json!(true));
    }

    /// 高坐标下起跳→落回也要稳：上升、落地回到原地、不被横向弹开。
    #[test]
    fn jump_and_land_on_high_platform_is_stable() {
        let mut sim = Sim::new(1);
        let hero = tall_platform_world(&mut sim, 33.0);
        for _ in 0..60 {
            sim.step(&mut ()).unwrap();
        }
        sim.world.set_field(hero, "Velocity.y", json!(12.0)).unwrap();
        sim.step(&mut ()).unwrap();
        assert_eq!(sim.world.get_field(hero, "Body.grounded").unwrap(), &json!(false));
        let mut peak: f64 = 0.0;
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
            peak = peak.max(sim.world.get_field(hero, "Position.y").unwrap().as_f64().unwrap());
        }
        assert!(peak > 36.0, "跳起来要离台，峰值 {peak}");
        let px = sim.world.get_field(hero, "Position.x").unwrap().as_f64().unwrap();
        let py = sim.world.get_field(hero, "Position.y").unwrap().as_f64().unwrap();
        assert_eq!(px, 8.0, "竖直跳不该有横向位移，实际 x={px}");
        assert!((py - 34.05).abs() < 1e-9, "落回高台顶面，实际 y={py}");
        assert_eq!(sim.world.get_field(hero, "Body.grounded").unwrap(), &json!(true));
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
    fn inject_reply_becomes_event_next_step_and_is_consumed() {
        let mut sim = Sim::new(1);
        moving_world(&mut sim);
        sim.inject_reply("llm-reply", json!({"id": "npc-1", "text": "你好"}));
        let mut logic = Collect(Vec::new());
        sim.step(&mut logic).unwrap();
        let reply: Vec<_> = logic.0.iter().filter(|e| e.name == "llm-reply").collect();
        assert_eq!(reply.len(), 1);
        assert_eq!(reply[0].data.get("id"), Some(&json!("npc-1")));
        assert_eq!(reply[0].data.get("text"), Some(&json!("你好")));
        // 消化即清空：第二步不能再出现
        logic.0.clear();
        sim.step(&mut logic).unwrap();
        assert!(logic.0.iter().all(|e| e.name != "llm-reply"));
    }

    /// 把 llm-reply 的 text 写进世界的逻辑——回复内容真实影响状态哈希，
    /// 重放时丢了回复必然分歧（这正是要锁死的不变量）。
    struct ApplyReply;
    impl GameLogic for ApplyReply {
        fn on_tick(&mut self, w: &mut World, ev: Vec<Event>, _: &mut Pcg32, _: u64) -> Result<(), String> {
            for e in ev {
                if e.name == "llm-reply" {
                    let m = w.entity("mover").map_err(|e| e.to_string())?;
                    let text = e.data.get("text").cloned().unwrap_or(json!(""));
                    w.set_component(m, "Dialogue", json!({"text": text}))
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn recording_with_replies_replays_bit_identically() {
        // 录一局：tick 30 注入一条影响世界状态的 LLM 回复
        let mut sim = Sim::new(7);
        moving_world(&mut sim);
        sim.start_recording();
        for t in 0..150 {
            if t == 30 {
                sim.inject_reply("llm-reply", json!({"id": "q1", "text": "宝箱在东边"}));
            }
            sim.step(&mut ApplyReply).unwrap();
        }
        let rec = sim.stop_recording().unwrap();
        assert_eq!(rec.replies.len(), 1);
        assert_eq!(rec.replies[0].tick, 30);
        assert_eq!(rec.replies[0].name, "llm-reply");

        // 重放：回复从录像注入（不碰网络），逐校验点一致 + 终态哈希一致
        let mut sim2 = Sim::new(7);
        moving_world(&mut sim2);
        sim2.replay(&rec, &mut ApplyReply).unwrap();
        assert_eq!(sim2.world.state_hash(), rec.final_hash);

        // 反证：把回复从录像里抠掉，重放必然跑偏——回复确实是被录的状态来源
        let mut crippled = rec.clone();
        crippled.replies.clear();
        let mut sim3 = Sim::new(7);
        moving_world(&mut sim3);
        let err = sim3.replay(&crippled, &mut ApplyReply).unwrap_err();
        assert!(matches!(err, SimError::ReplayDiverged { .. }), "{err}");
    }

    #[test]
    fn old_recording_without_replies_still_parses() {
        // 旧版录像 JSON 没有 replies 字段：serde(default) 补空，语义不变
        let text = r#"{"seed":1,"inputs":[],"checkpoints":[],"ticks":0,"final_hash":0}"#;
        let rec: Recording = serde_json::from_str(text).unwrap();
        assert!(rec.replies.is_empty());
    }

    #[test]
    fn snapshot_roundtrips_pending_replies() {
        let mut sim = Sim::new(3);
        moving_world(&mut sim);
        sim.inject_reply("llm-reply", json!({"id": "q1", "text": "snap"}));
        let snap = sim.snapshot(&());

        // 直接跑：回复在下一 step 生效
        sim.step(&mut ApplyReply).unwrap();
        let h_direct = sim.world.state_hash();

        // restore 进新 sim 再跑：未消化的回复必须原样回来
        let mut sim2 = Sim::new(0);
        sim2.restore(&snap, &mut ()).unwrap();
        sim2.step(&mut ApplyReply).unwrap();
        assert_eq!(sim2.world.state_hash(), h_direct);

        // 旧版快照缺 pending_replies → 显式报错，不静默补空
        let mut old_snap = snap.clone();
        old_snap.as_object_mut().unwrap().remove("pending_replies");
        let err = sim2.restore(&old_snap, &mut ()).unwrap_err();
        assert!(err.contains("pending_replies"), "{err}");
    }

    #[test]
    fn restore_invalidates_in_progress_recording() {
        // 回归：录像中途 restore，checkpoints 已错位，留着只会产出静默损坏的录像
        let mut sim = Sim::new(7);
        moving_world(&mut sim);
        let snap = sim.snapshot(&());
        sim.start_recording();
        for _ in 0..10 {
            sim.step(&mut ()).unwrap();
        }
        sim.restore(&snap, &mut ()).unwrap();
        assert!(!sim.is_recording(), "restore 后录像必须作废");
        assert!(sim.stop_recording().is_none());
    }

    #[test]
    fn snapshot_restore_resumes_identically() {
        let mut sim = Sim::new(3);
        moving_world(&mut sim);
        for _ in 0..50 {
            let _ = sim.rng.next_u32();
            sim.step(&mut ()).unwrap();
        }
        let snap = sim.snapshot(&());

        // 继续跑 50 tick
        for _ in 0..50 {
            let _ = sim.rng.next_u32();
            sim.step(&mut ()).unwrap();
        }
        let h_direct = sim.world.state_hash();

        // 从快照恢复再跑 50 tick：必须一模一样
        let mut sim2 = Sim::new(0); // 故意用错种子，restore 必须完整覆盖
        sim2.restore(&snap, &mut ()).unwrap();
        for _ in 0..50 {
            let _ = sim2.rng.next_u32();
            sim2.step(&mut ()).unwrap();
        }
        assert_eq!(sim2.world.state_hash(), h_direct);
        assert_eq!(sim2.tick, sim.tick);
    }

    /// 跟随相机测试场：匀速移动的目标 + 一台 follow 相机。
    fn follow_world(sim: &mut Sim, lerp: f64) -> (vitric_ecs::EntityId, vitric_ecs::EntityId) {
        let hero = sim.world.spawn_named("hero").unwrap();
        sim.world.set_component(hero, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(hero, "Velocity", json!({"x": 60.0, "y": 0.0})).unwrap();
        let cam = sim.world.spawn_named("cam").unwrap();
        sim.world
            .set_component(
                cam,
                "Camera",
                json!({"x": -10.0, "y": 3.0, "scale": 8.0, "follow": "hero", "lerp": lerp}),
            )
            .unwrap();
        (hero, cam)
    }

    #[test]
    fn camera_follow_converges_on_target() {
        let mut sim = Sim::new(1);
        let (hero, cam) = follow_world(&mut sim, 0.2);
        for _ in 0..300 {
            sim.step(&mut ()).unwrap();
        }
        let hx = sim.world.get_field(hero, "Position.x").unwrap().as_f64().unwrap();
        let cx = sim.world.get_field(cam, "Camera.x").unwrap().as_f64().unwrap();
        let cy = sim.world.get_field(cam, "Camera.y").unwrap().as_f64().unwrap();
        // 目标匀速运动时 lerp 跟随收敛到一个固定滞后距离：v*DT*(1-lerp)/lerp = 1*0.8/0.2 = 4
        assert!((hx - cx - 4.0).abs() < 1e-6, "应稳定滞后 4 单位，实际差 {}", hx - cx);
        assert!(cy.abs() < 1e-6, "y 轴无运动应收敛到 0，实际 {cy}");
    }

    #[test]
    fn camera_follow_lerp_one_hard_locks_after_motion() {
        let mut sim = Sim::new(1);
        let (hero, cam) = follow_world(&mut sim, 1.0);
        sim.step(&mut ()).unwrap();
        // 跟随跑在运动之后：相机锁的是本 tick 运动后的最终位置，不滞后一帧
        let hx = sim.world.get_field(hero, "Position.x").unwrap().as_f64().unwrap();
        let cx = sim.world.get_field(cam, "Camera.x").unwrap().as_f64().unwrap();
        assert!((hx - 1.0).abs() < 1e-9, "60/s 走 1 tick 应在 x=1，实际 {hx}");
        assert_eq!(cx, hx, "lerp=1 应硬锁定到目标本 tick 的最终位置");
    }

    #[test]
    fn camera_follow_missing_entity_is_explicit() {
        let mut sim = Sim::new(1);
        let cam = sim.world.spawn_named("cam").unwrap();
        sim.world
            .set_component(
                cam,
                "Camera",
                json!({"x": 0.0, "y": 0.0, "scale": 8.0, "follow": "ghost", "lerp": 0.5}),
            )
            .unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ghost") && msg.contains("不存在"), "{msg}");
        // 没定义 follow 字段 = 不跟随，照常跑
        sim.world
            .set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0}))
            .unwrap();
        sim.step(&mut ()).unwrap();
    }

    #[test]
    fn shake_amplitude_decays_and_snaps_to_zero() {
        let mut sim = Sim::new(1);
        let cam = sim.world.spawn_named("cam").unwrap();
        sim.world.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0})).unwrap();
        sim.world.set_component(cam, "Shake", json!({"amplitude": 0.5, "decay": 0.9})).unwrap();
        sim.step(&mut ()).unwrap();
        let amp = sim.world.get_field(cam, "Shake.amplitude").unwrap().as_f64().unwrap();
        assert!((amp - 0.45).abs() < 1e-12, "0.5 * 0.9 = 0.45，实际 {amp}");
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        // 乘法衰减低于阈值精确归零（不留肉眼不可见的状态抖动）
        let amp = sim.world.get_field(cam, "Shake.amplitude").unwrap().as_f64().unwrap();
        assert_eq!(amp, 0.0);
    }

    #[test]
    fn shake_never_perturbs_gameplay_trajectory() {
        // 同一个世界，一份带 Shake 一份不带：mover 的轨迹必须逐位一致
        let run = |with_shake: bool| {
            let mut sim = Sim::new(5);
            moving_world(&mut sim);
            let cam = sim.world.spawn_named("cam").unwrap();
            sim.world
                .set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0}))
                .unwrap();
            if with_shake {
                sim.world
                    .set_component(cam, "Shake", json!({"amplitude": 2.0, "decay": 0.95}))
                    .unwrap();
            }
            for _ in 0..120 {
                sim.step(&mut ()).unwrap();
            }
            let m = sim.world.entity("mover").unwrap();
            sim.world.get_component(m, "Position").unwrap().clone()
        };
        assert_eq!(run(true), run(false));
    }

    #[test]
    fn particle_despawns_when_ttl_runs_out() {
        let mut sim = Sim::new(1);
        let p = sim.world.spawn_named("dust").unwrap();
        sim.world.set_component(p, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(p, "Velocity", json!({"x": 1.0, "y": 2.0})).unwrap();
        sim.world.set_component(p, "Particle", json!({"ttl": 3})).unwrap();
        sim.step(&mut ()).unwrap();
        sim.step(&mut ()).unwrap();
        assert!(sim.world.is_alive(p), "ttl=3 应活满 3 tick");
        assert_eq!(sim.world.get_field(p, "Particle.ttl").unwrap(), &json!(1));
        sim.step(&mut ()).unwrap();
        assert!(!sim.world.is_alive(p), "第 3 tick ttl 归零应被销毁");

        // ttl 不是整数 → 显式报错
        let bad = sim.world.spawn_named("bad").unwrap();
        sim.world.set_component(bad, "Particle", json!({"ttl": "forever"})).unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        assert!(err.to_string().contains("整数"), "{err}");
    }

    #[test]
    fn emitter_fields_hash_but_particles_add_no_state() {
        // 粒子是渲染层纯函数产物：发射器字段本身入状态哈希，但跑多少 tick
        // 都不产生任何额外状态（没有 sim 系统碰 Emitter——这条测试锁死这个事实）
        let mut sim = Sim::new(1);
        let e = sim.world.spawn_named("sparks").unwrap();
        sim.world.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let h_before = sim.world.state_hash();
        sim.world
            .set_component(
                e,
                "Emitter",
                json!({"kind": "stream", "rate": 30.0, "lifetime": 40, "size": 0.5,
                       "burst": -1}),
            )
            .unwrap();
        let h_with = sim.world.state_hash();
        assert_ne!(h_before, h_with, "发射器字段必须进状态哈希");
        // 跑 120 tick：世界里没有任何东西在动，哈希一动不动（粒子零状态）
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        assert_eq!(sim.world.state_hash(), h_with, "粒子不许往模拟状态里塞任何东西");
        // burst 触发 = 规则写字段：哈希变化（被录像/快照如实捕获），照样零额外状态
        sim.world.set_field(e, "Emitter.burst", json!(120)).unwrap();
        let h_burst = sim.world.state_hash();
        assert_ne!(h_burst, h_with);
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        assert_eq!(sim.world.state_hash(), h_burst);
        // snapshot/restore 往返：哈希一致（发射器只是普通组件，快照天然覆盖）
        let snap = sim.snapshot(&());
        let mut sim2 = Sim::new(99);
        sim2.restore(&snap, &mut ()).unwrap();
        assert_eq!(sim2.world.state_hash(), h_burst);
        assert_eq!(sim2.tick, sim.tick);
    }

    // ---- 补间（Tween 组件，内建系统）----

    /// 补间测试场：一个面板 + 一条把 Position.x 从 1 补到 5 的补间。
    fn tween_world(sim: &mut Sim, ease: &str, duration: i64) -> vitric_ecs::EntityId {
        let panel = sim.world.spawn_named("panel").unwrap();
        sim.world.set_component(panel, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let tw = sim.world.spawn_named("tw").unwrap();
        sim.world
            .set_component(
                tw,
                "Tween",
                json!({"target": "panel", "field": "Position.x", "from": 1.0, "to": 5.0,
                       "duration": duration, "ease": ease, "start": -1, "id": "slide"}),
            )
            .unwrap();
        panel
    }

    #[test]
    fn tween_is_analytic_in_progress_and_finishes_exact() {
        let mut sim = Sim::new(1);
        let panel = tween_world(&mut sim, "linear", 8);
        let mut logic = Collect(Vec::new());
        // 第一个 tick：起始 tick 被记下，字段写成起始值（progress = 0）
        sim.step(&mut logic).unwrap();
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(1.0));
        // 中途：第 T tick 的值 = from + (to-from)·ease(elapsed/duration)，纯函数无累加
        for _ in 0..4 {
            sim.step(&mut logic).unwrap();
        }
        // elapsed = 4, progress = 0.5 → 1 + 4·0.5 = 3
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(3.0));
        // 到期 tick：字段精确等于终值（不留浮点尾巴）+ 完成事件（带 id）+ 补间自动移除
        for _ in 0..4 {
            sim.step(&mut logic).unwrap();
        }
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(5.0));
        let tw = sim.world.entity("tw");
        assert!(tw.is_err(), "完成后补间实体应被移除");
        let fin: Vec<_> = logic.0.iter().filter(|e| e.name == "tween-finished").collect();
        assert_eq!(fin.len(), 1, "应恰好发一次完成事件");
        assert_eq!(fin[0].data.get("id"), Some(&json!("slide")));
        assert_eq!(fin[0].data.get("field"), Some(&json!("Position.x")));
        // 之后世界静止：补间没了，字段不再动
        sim.step(&mut logic).unwrap();
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(5.0));
    }

    #[test]
    fn tween_final_value_has_no_float_tail() {
        // 0 → 0.3 走 7 tick：中间值必然带浮点尾巴，到期那刻必须精确写 0.3
        let mut sim = Sim::new(1);
        let panel = sim.world.spawn_named("panel").unwrap();
        sim.world.set_component(panel, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let tw = sim.world.spawn().to_owned();
        sim.world
            .set_component(
                tw,
                "Tween",
                json!({"target": "panel", "field": "Position.x", "from": 0.0, "to": 0.3,
                       "duration": 7, "ease": "ease-in-out", "start": -1, "id": ""}),
            )
            .unwrap();
        for _ in 0..8 {
            sim.step(&mut ()).unwrap();
        }
        let x = sim.world.get_field(panel, "Position.x").unwrap().as_f64().unwrap();
        assert!(x == 0.3, "终值必须逐位等于 0.3，实际 {x:?}");
        assert!(!sim.world.is_alive(tw));
    }

    #[test]
    fn tween_ease_out_back_overshoots_then_settles() {
        let mut sim = Sim::new(1);
        let panel = tween_world(&mut sim, "ease-out-back", 20);
        let mut peak = f64::MIN;
        for _ in 0..21 {
            sim.step(&mut ()).unwrap();
            peak = peak.max(sim.world.get_field(panel, "Position.x").unwrap().as_f64().unwrap());
        }
        assert!(peak > 5.0, "ease-out-back 必须过冲（峰值 {peak} 应 > 终值 5）");
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(5.0));
    }

    #[test]
    fn tween_same_field_latecomer_replaces_incumbent() {
        let mut sim = Sim::new(1);
        tween_world(&mut sim, "linear", 60);
        let panel = sim.world.entity("panel").unwrap();
        for _ in 0..10 {
            sim.step(&mut ()).unwrap();
        }
        // 后来者：同实体同字段，100 → 200
        let tw2 = sim.world.spawn_named("tw2").unwrap();
        sim.world
            .set_component(
                tw2,
                "Tween",
                json!({"target": "panel", "field": "Position.x", "from": 100.0, "to": 200.0,
                       "duration": 10, "ease": "linear", "start": -1, "id": "late"}),
            )
            .unwrap();
        let mut logic = Collect(Vec::new());
        sim.step(&mut logic).unwrap();
        // 前者被顶掉（无完成事件、实体移除），字段跟着后来者走
        assert!(sim.world.entity("tw").is_err(), "前者应被顶掉移除");
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(100.0));
        assert!(
            logic.0.iter().all(|e| e.name != "tween-finished"),
            "顶掉不是完成，不发完成事件"
        );
        for _ in 0..10 {
            sim.step(&mut logic).unwrap();
        }
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(200.0));
        let fin: Vec<_> = logic.0.iter().filter(|e| e.name == "tween-finished").collect();
        assert_eq!(fin.len(), 1);
        assert_eq!(fin[0].data.get("id"), Some(&json!("late")));
    }

    #[test]
    fn tween_snapshot_restore_resumes_identically() {
        let mut sim = Sim::new(3);
        tween_world(&mut sim, "ease-in-out", 40);
        for _ in 0..15 {
            sim.step(&mut ()).unwrap();
        }
        let snap = sim.snapshot(&());
        for _ in 0..40 {
            sim.step(&mut ()).unwrap();
        }
        let h_direct = sim.world.state_hash();
        // 中途回退再续播：轨迹必须逐位一致（补间状态全在组件里，快照天然覆盖）
        let mut sim2 = Sim::new(0);
        sim2.restore(&snap, &mut ()).unwrap();
        for _ in 0..40 {
            sim2.step(&mut ()).unwrap();
        }
        assert_eq!(sim2.world.state_hash(), h_direct);
    }

    #[test]
    fn tween_bad_data_is_explicit() {
        // 目标实体不存在
        let mut sim = Sim::new(1);
        let tw = sim.world.spawn().to_owned();
        sim.world
            .set_component(
                tw,
                "Tween",
                json!({"target": "ghost", "field": "Position.x", "from": 0.0, "to": 1.0,
                       "duration": 10, "ease": "linear", "start": -1, "id": ""}),
            )
            .unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        assert!(err.to_string().contains("ghost"), "{err}");

        // 未知缓动曲线
        let mut sim = Sim::new(1);
        let panel = sim.world.spawn_named("panel").unwrap();
        sim.world.set_component(panel, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let tw = sim.world.spawn().to_owned();
        sim.world
            .set_component(
                tw,
                "Tween",
                json!({"target": "panel", "field": "Position.x", "from": 0.0, "to": 1.0,
                       "duration": 10, "ease": "bounce", "start": -1, "id": ""}),
            )
            .unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bounce") && msg.contains("linear"), "要列出可用曲线: {msg}");

        // 字段不是数字
        let mut sim = Sim::new(1);
        let panel = sim.world.spawn_named("panel").unwrap();
        sim.world.set_component(panel, "Label", json!({"text": "hi"})).unwrap();
        let tw = sim.world.spawn().to_owned();
        sim.world
            .set_component(
                tw,
                "Tween",
                json!({"target": "panel", "field": "Label.text", "from": 0.0, "to": 1.0,
                       "duration": 10, "ease": "linear", "start": -1, "id": ""}),
            )
            .unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        assert!(err.to_string().contains("Label.text"), "{err}");

        // duration < 1
        let mut sim = Sim::new(1);
        let panel = sim.world.spawn_named("panel").unwrap();
        sim.world.set_component(panel, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let tw = sim.world.spawn().to_owned();
        sim.world
            .set_component(
                tw,
                "Tween",
                json!({"target": "panel", "field": "Position.x", "from": 0.0, "to": 1.0,
                       "duration": 0, "ease": "linear", "start": -1, "id": ""}),
            )
            .unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        assert!(err.to_string().contains("duration"), "{err}");
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
