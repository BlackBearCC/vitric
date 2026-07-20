use std::collections::BTreeMap;
use std::fmt;

use serde_json::json;

use vitric_ecs::World;
use vitric_rules::Event;

use crate::tween::{tween_value, Ease};
use crate::{InputRecord, Pcg32, Recording, ReplyRecord};

/// Simulation frequency is fixed at 60Hz. Fixed timestep is the prerequisite for determinism: wall-clock time never enters the simulation.
pub const TICKS_PER_SECOND: u64 = 60;
pub const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

/// State hash checkpoint interval (in ticks).
const CHECKPOINT_INTERVAL: u64 = 60;

/// Game logic mount point. The rules engine and script layer are wrapped as a GameLogic at the runtime layer;
/// sim only deterministically "feeds events and advances time" — it knows nothing about rules and scripts.
pub trait GameLogic {
    fn on_tick(
        &mut self,
        world: &mut World,
        events: Vec<Event>,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<(), String>;

    /// Take away the event copies emitted by the logic layer (rules/scripts) this tick, for control-plane event log observation.
    /// Implementations without observable events can use the default empty set.
    fn drain_observed(&mut self) -> Vec<Event> {
        Vec::new()
    }

    /// Hot-reload the logic layer (rules/scripts replaced from disk, world state untouched).
    /// Returns a reload summary on success; on failure the old logic must remain usable as-is.
    fn reload(&mut self) -> Result<serde_json::Value, String> {
        Err("该运行时不支持热重载".to_string())
    }

    /// Logic layer's own state stashed across ticks (e.g. events not yet consumed).
    /// State not in the snapshot = silent trajectory divergence after restore; implementations with stashed state must implement this pair of hooks.
    fn snapshot_state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Restores in pair with [`GameLogic::snapshot_state`].
    fn restore_state(&mut self, _snap: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// This logic's current "what can I do" input action vocabulary: `(action_name, list of phases it has appeared in)`.
    /// The control plane describe uses it to tell the agent, beyond "what's on the screen", also "what you can press" — unified with the playable
    /// SceneView affordance. Default empty: only rules-style logic (Runtime) reaches the rules and can return non-empty;
    /// sim holds a `dyn GameLogic` and cannot reach the concrete rules engine, so this hook is overridden by implementors as needed.
    fn available_actions(&self) -> Vec<(String, Vec<String>)> {
        Vec::new()
    }
}

/// Empty logic (for pure physics simulation).
impl GameLogic for () {
    fn on_tick(&mut self, _: &mut World, _: Vec<Event>, _: &mut Pcg32, _: u64) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SimError {
    /// Game logic (rules/scripts) reported an error.
    Logic { tick: u64, message: String },
    /// A built-in system read illegal component data.
    BadComponent { tick: u64, entity: String, component: String, reason: String },
    /// Replay diverged: state hash does not match the recording.
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

/// Output of a single step (for control plane / debugging).
#[derive(Debug, Default)]
pub struct StepReport {
    /// Tick value after the step.
    pub tick: u64,
    /// Events dispatched to the logic layer this tick.
    pub events: Vec<Event>,
}

/// Deterministic simulator.
pub struct Sim {
    pub world: World,
    pub rng: Pcg32,
    pub tick: u64,
    seed: u64,
    pending_inputs: Vec<(String, String)>,
    /// Injected but unconsumed external replies (LLM replies, etc.). A second recording channel at the
    /// same level as pending_inputs: enters step as events + enters the recording, enters the snapshot.
    pending_replies: Vec<(String, serde_json::Value)>,
    /// Events queued by host API calls (e.g. [`Sim::thaw_region`]) between steps. Same lifecycle
    /// as `pending_replies` — drained into the logic inbox at the start of the next step. NOT
    /// recorded by the recording: a host API call is deterministic given the same host program
    /// (replay re-runs the same host program, so the same `thaw_region` call happens at the same
    /// tick). If a recording must capture an externally-injected event, use `inject_reply`
    /// (which IS recorded, same channel as LLM replies).
    pending_events: Vec<Event>,
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
            pending_events: Vec::new(),
            recorder: None,
        }
    }

    /// Seed of this run (set at construction, overwritten by snapshot on restore). When manually assembling a recording, use it as `Recording.seed`.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Inject an input (takes effect on the next step). phase: "pressed" | "released".
    pub fn inject_input(&mut self, action: &str, phase: &str) {
        self.pending_inputs.push((action.to_string(), phase.to_string()));
    }

    /// Inject an external reply (becomes an event named `name` with `data` on the next step).
    /// This is the **only** proper channel for async external content such as LLM to enter the simulation: like inputs, it is recorded by the recording,
    /// and re-injected at the original tick during replay — so recordings containing LLM content replay bit-identically offline.
    /// Convention: `data` is a JSON object (non-objects are dropped to empty object by Event; caller must guarantee this).
    pub fn inject_reply(&mut self, name: &str, data: serde_json::Value) {
        self.pending_replies.push((name.to_string(), data));
    }

    /// Transition a Region entity from dormant/frozen to active, mark it discovered, and queue
    /// a `region-thaw` event for the next step. The event carries `{"id": <region_id>}` and
    /// reaches the logic inbox on the next step (where rules can react to it).
    ///
    /// Safe to call on a non-existent region or an entity without a Region component — both
    /// are silent no-ops (defensive: the host may call this speculatively). Not idempotent on
    /// already-active regions — the state is re-set to "active" (no-op) and the event is still
    /// emitted (rules can decide whether to dedupe based on `discovered`).
    ///
    /// Catch-up logic for previously-discovered regions is Task 2 (out of scope here).
    pub fn thaw_region(&mut self, id: &str) {
        let Ok(region_e) = self.world.entity(id) else { return; };
        let Ok(mut region) = self.world.get_component(region_e, "Region").cloned() else { return; };
        region["state"] = json!("active");
        region["discovered"] = json!(1);
        let _ = self.world.set_component(region_e, "Region", region);
        self.pending_events.push(Event::new("region-thaw", json!({"id": id})));
    }

    /// Start recording (records from the current state).
    pub fn start_recording(&mut self) {
        self.recorder = Some(Recording {
            seed: self.seed,
            checkpoints: vec![(self.tick, self.world.state_hash())],
            ..Default::default()
        });
    }

    /// Whether recording is in progress. Recordings only record the input stream: any state modification bypassing inputs during recording
    /// (RPC mutating the world, inspector drag, restore) makes the recording non-replayable; callers must check this before acting.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Stop recording.
    pub fn stop_recording(&mut self) -> Option<Recording> {
        let mut rec = self.recorder.take()?;
        rec.ticks = self.tick;
        rec.final_hash = self.world.state_hash();
        Some(rec)
    }

    /// Advance one frame. Pipeline (fixed order, this is determinism):
    /// 1. Injected inputs → input events, injected external replies → same-named events (recording happens here)
    /// 2. Built-in gravity: Body entities Velocity.y += gravity * DT
    /// 3. Built-in motion system: Position += Velocity * DT (entities with Body+Collider are stopped by Solids)
    /// 4. Game-feel built-in systems (run after motion so the camera sees this tick's final position):
    ///    camera follow → shake decay → particle lifetime → tween. Tweens (Tween component) run after all
    ///    built-in field-writing systems: fields targeted by a tween are governed by the tween (motion integration / camera-follow
    ///    writes are overwritten by this tick's tween value), collision detection and rendering see the post-tween final value
    /// 5. Built-in collision detection: AABB overlap → collision event
    /// 6. Game logic (rules + scripts) consumes all events
    /// 7. tick + 1
    pub fn step(&mut self, logic: &mut dyn GameLogic) -> Result<StepReport, SimError> {
        let mut events = Vec::new();

        // 0. The world's first tick dispatches a start event — the standard entry for rule initialization
        if self.tick == 0 {
            events.push(Event::new("start", json!({})));
        }

        // 1. Inputs
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

        // 1.5 External replies (fixed to come after inputs; replay injects in the same order to stay bit-identical)
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

        // 1.6 Host API events (thaw_region, etc.). NOT recorded — host API calls are deterministic
        // given the same host program, so replay re-runs the same calls at the same ticks. Order:
        // after external replies, so a region-thaw event arrives at the logic layer this tick and
        // can be combined with whatever rule/script logic that tick produces.
        for ev in std::mem::take(&mut self.pending_events) {
            events.push(ev);
        }

        // 2. Gravity + motion
        self.apply_gravity()?;
        self.integrate_motion()?;

        // 3. Game feel: camera follow runs after motion (no one-frame lag); shake decay runs before logic
        //    (the amplitude set by rules this tick renders at its original value on the first frame, decay starts next tick);
        //    particles are despawned before collision detection (dead particles emit no collision event)
        self.follow_camera()?;
        self.decay_shake()?;
        self.age_particles()?;
        self.advance_tweens(&mut events)?;

        // 4. Collisions
        self.detect_collisions(&mut events)?;

        // 5. Logic
        logic
            .on_tick(&mut self.world, events.clone(), &mut self.rng, self.tick)
            .map_err(|message| SimError::Logic { tick: self.tick, message })?;

        // 5.5 Region dormant accounting: every step, dormant/frozen regions accumulate one tick
        // of `dormant_ticks` — used by catch-up logic (Task 2) to decide how much sim time to
        // fast-forward when the region thaws. Runs after logic so rule/script state mutations
        // this tick are visible; uses `entities()` (not `query`) because query filters dormant.
        self.accumulate_dormant_ticks();

        // 6. Time advance + recording checkpoint
        self.tick += 1;
        if let Some(rec) = &mut self.recorder {
            if self.tick.is_multiple_of(CHECKPOINT_INTERVAL) {
                rec.checkpoints.push((self.tick, self.world.state_hash()));
            }
        }

        Ok(StepReport { tick: self.tick, events })
    }

    /// Replay a recording and compare at each checkpoint. Before calling, the world must be at the recording's starting state
    /// (a world instantiated from the same project data naturally satisfies this).
    pub fn replay(&mut self, rec: &Recording, logic: &mut dyn GameLogic) -> Result<(), SimError> {
        self.replay_observed(rec, logic, |_, _, _, _| {})
    }

    /// Replay + per-tick observation. After each tick is advanced, the observation window is handed to `observe`:
    /// `(tick, world, events fed to logic this tick, events emitted by the logic layer)`.
    /// `vitric gate` uses it to collect events and run asserts during replay — the observer **may only look, not write**;
    /// writing the world guarantees the next checkpoint hash diverges (this is precisely the foundation that makes a recording a non-forgeable delivery certificate).
    pub fn replay_observed(
        &mut self,
        rec: &Recording,
        logic: &mut dyn GameLogic,
        mut observe: impl FnMut(u64, &World, &[Event], Vec<Event>),
    ) -> Result<(), SimError> {
        // Start-point verification
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
            // External replies are taken from the recording (network is never re-called); injection order matches recording: inputs first, replies after
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

    /// Full simulator snapshot (world + time + random number state).
    pub fn snapshot(&self, logic: &dyn GameLogic) -> serde_json::Value {
        json!({
            "tick": self.tick,
            "seed": self.seed,
            "rng": serde_json::to_value(&self.rng).expect("rng 可序列化"),
            "world": self.world.snapshot(),
            // Injected but unconsumed inputs/external replies. Dropping any of them makes them vanish on restore, causing silent trajectory divergence
            "pending_inputs": self.pending_inputs,
            "pending_replies": self.pending_replies,
            // Host API events queued between save and next step (region-thaw etc.). Serialized
            // manually because Event doesn't derive Serialize — keep shape in sync with restore.
            "pending_events": self.pending_events.iter()
                .map(|e| json!({"name": e.name, "data": serde_json::Value::Object(e.data.clone())}))
                .collect::<Vec<_>>(),
            // Logic layer stashed state (events emitted by the script last tick, etc.)
            "logic": logic.snapshot_state(),
        })
    }

    /// Restore from snapshot.
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
        // Report missing explicitly: old snapshots have no pending_replies; silently filling empty would mask version incompatibility
        let pending_replies: Vec<(String, serde_json::Value)> = serde_json::from_value(
            snap.get("pending_replies")
                .cloned()
                .ok_or("快照缺 pending_replies（旧版快照与当前版本不兼容，重新 sim/snapshot）")?,
        )
        .map_err(|e| format!("pending_replies 解析失败: {e}"))?;
        // pending_events: manual deserialization (Event has no Deserialize derive).
        // Old snapshots predating this field → treat as empty (forward compatibility: no
        // host API events could have been queued before the field existed).
        let pending_events_arr = snap.get("pending_events").and_then(|v| v.as_array());
        let mut pending_events: Vec<Event> = Vec::new();
        if let Some(arr) = pending_events_arr {
            for ev in arr {
                let name = ev.get("name").and_then(|v| v.as_str())
                    .ok_or("pending_events 缺 name 字段")?;
                let data = ev.get("data").cloned().unwrap_or(serde_json::Value::Null);
                pending_events.push(Event::new(name, data));
            }
        }
        logic.restore_state(snap.get("logic").ok_or("快照缺 logic 状态")?)?;
        self.tick = tick;
        self.seed = seed;
        self.rng = rng;
        self.world = world;
        self.pending_inputs = pending;
        self.pending_replies = pending_replies;
        self.pending_events = pending_events;
        // The timeline is broken, so an in-progress recording is necessarily non-replayable — invalidate it directly, no silently corrupted recording left behind
        self.recorder = None;
        Ok(())
    }

    // ---- Built-in systems ----

    /// Increment `Region.dormant_ticks` on every Region entity currently in dormant or frozen
    /// state. Uses `entities()` (not `query`) because `query` filters dormant — we explicitly
    /// want to find dormant entities to increment their counter. Used by catch-up logic (Task 2)
    /// to decide how much sim time to fast-forward when the region thaws.
    fn accumulate_dormant_ticks(&mut self) {
        for id in self.world.entities() {
            let Ok(region) = self.world.get_component(id, "Region") else { continue; };
            let Some(state) = region.get("state").and_then(|v| v.as_str()) else { continue; };
            if state != "dormant" && state != "frozen" { continue; }
            let dt = region.get("dormant_ticks").and_then(|v| v.as_i64()).unwrap_or(0);
            let _ = self.world.set_field(id, "Region.dormant_ticks", json!(dt + 1));
        }
    }

    /// Gravity: Body entities add gravity * DT to Velocity.y each tick (world y points up, gravity is usually negative).
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
        // Solid = stopper (ground/wall). Entities with Body+Collider are clipped flush against it and have that axis velocity zeroed.
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
            // Axis separation: x first then y, each axis snaps flush on contact (center coordinates).
            // Overlap uses penetrates (with a relative margin), not the strict < of collision events:
            // the floating-point addition of a flush snap (e.g. ny = sy + (sh+h)/2) can shed up to
            // one ULP at large coordinates, writing back a position a hair deeper than the exact contact. Strict < would misjudge this "standing"
            // as penetration — the next tick's x-axis check hits first and flings the entity standing on the platform
            // sideways. See the comment on penetrates for the margin.
            // Note: a single-tick displacement larger than obstacle thickness tunnels through (no sweep); leave headroom in the velocity budget.
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
                        ny = sy + (sh + h) / 2.0; // Lands on the top surface
                        grounded = true;
                    } else {
                        ny = sy - (sh + h) / 2.0; // Hits the bottom surface from below
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

    /// Camera follow: `Camera.follow` names an entity (optional field, default/empty = no follow);
    /// each tick pulls Camera.x/y toward the target Position by a `lerp` ratio (0..=1, 1 = hard lock).
    /// A follow pointing to a non-existent entity errors directly — silently skipping would make "camera doesn't move" very hard to diagnose.
    fn follow_camera(&mut self) -> Result<(), SimError> {
        for id in self.world.query(&["Camera"]) {
            let Ok(follow) = self.world.get_field(id, "Camera.follow") else {
                continue; // No follow field defined = no follow (optional convention)
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

    /// Screen shake decay: `Shake.amplitude` is multiplied by `decay` each tick and written back to the component (snapshot/replay safe).
    /// The offset itself is computed in the render layer — a pure function of (tick, amplitude) (vitric-render's shake_offset),
    /// and never touches the simulation's RNG stream: shaking the screen or not has zero impact on the gameplay trajectory.
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
            // Multiplicative decay never reaches 0: below one thousandth it snaps to zero, stopping state jitter invisible to the eye
            let next = amp * decay;
            let next = if next < 1e-3 { 0.0 } else { next };
            self.world
                .set_field(id, "Shake.amplitude", json!(next))
                .expect("字段刚读过必然存在");
        }
        Ok(())
    }

    /// Particle lifetime: `Particle.ttl` (remaining ticks, integer) decreases by 1 each tick; on reaching 0 it is despawned on the spot
    /// (despawn order = slot order, deterministic). The spawner can fire-and-forget once it has spawned (Sprite+Velocity+Particle).
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

    /// Tween: the `Tween` component (a standalone entity, declared by the scene file or spawned by rules/scripts) smoothly interpolates a
    /// numeric field of a target entity from `from` to `to`. All state lives in the component (enters state hash, enters save),
    /// the value at the elapsed-th tick is the closed-form `from + (to-from)·ease(elapsed/duration)`
    /// (see [`crate::tween`]; cumulative integration is forbidden — snapshot rollback and resume are bit-identical).
    ///
    /// Semantic contract:
    /// - The first tick it is processed records the starting point (`start` is stamped from -1 to the current tick) and writes the starting value;
    /// - On the expiry tick (elapsed == duration) the field is written **exactly** as `to` (no floating-point tail),
    ///   a `tween-finished {id, target, field}` event is dispatched, and the tween entity is auto-removed;
    /// - Only one active tween is allowed per (target entity, field) at a time: the latecomer replaces the incumbent (the former is removed directly,
    ///   with no event and no error — explicit semantics). "Late" is judged by starting point; ties within the same tick go to the later slot order;
    /// - Runs after motion/camera follow: a field targeted by a tween is governed by the tween value this tick.
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
        // Parse all tweens — any data issue is surfaced explicitly before touching the world
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
            // Target resolution: first look up by name, then parse by handle; both failing is an explicit error
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

        // Conflict resolution: for each (target entity, field) keep only the newest; the rest are removed directly (replace semantics).
        // "Newest" = not-yet-started (start = -1, appeared this tick) beats started; among started, larger start wins;
        // full tie goes to the later slot order — iter is already in slot order, so ≥ means replace.
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

        // Apply (slot order, deterministic)
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
            // The target field must already exist and be numeric — tweens don't create fields, only mutate existing truth
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
            // Starting stamp: `start` is stamped from -1 to the current tick (enters component = enters hash and save)
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
                // Expiry: exact final value + completion event + auto-remove
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

/// Stopper overlap test: entity center a, solid center b, sum of both sizes span;
/// the penetration depth must exceed a relative margin to count as overlap.
///
/// Constraint: the margin must scale with the coordinate magnitude. The rounding error of a flush snap is ULP-sized, and ULP
/// is proportional to the coordinate (about 7e-15 when y≈34, often 0 when y≈1 — so the fling bug only appears at
/// large coordinates). A fixed absolute margin is either insufficient at large coordinates or swallows real displacement at small ones.
/// Take 1e-9 × magnitude: six orders larger than ULP (2.2e-16 × magnitude), comfortably above rounding error;
/// and far smaller than any real penetration (velocity × DT, minimum on the order of 1e-3), so it never swallows a real collision.
/// At low coordinates the result matches the original strict < test, so existing trajectories (including recording hashes) are unchanged.
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

    /// Platformer physics test bed: a gravity-affected character + floor below + wall on the right.
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
        // Free-fall a few ticks: downward velocity accumulates
        for _ in 0..10 {
            sim.step(&mut ()).unwrap();
        }
        assert!(sim.world.get_field(p, "Velocity.y").unwrap().as_f64().unwrap() < 0.0);
        assert_eq!(sim.world.get_field(p, "Body.grounded").unwrap(), &json!(false));
        // Run until landing: standing on the floor top (floor top 0.0 + half-height 1.0), vertical velocity zeroed, grounded
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
        // Land first, then charge right into the wall
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        sim.world.set_field(p, "Velocity.x", json!(20.0)).unwrap();
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
            // Keep pushing (collision zeroes it, simulating the worst case of a held key)
            sim.world.set_field(p, "Velocity.x", json!(20.0)).unwrap();
        }
        // Wall left edge 5.0, character half-width 0.5 → flush at 4.5 at most
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
        // Jump = give an upward velocity (in rules this is just set Velocity.y)
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

    /// Tall-platform test bed: a stack of blocks whose top is at y=top + a 2.1-tall character standing on it.
    /// The 2.1-tall character + 2.0-tall platform block combo makes the snap coordinate (top + 1.05) fractional,
    /// which inevitably produces ULP rounding at large coordinates — exactly the form that triggers the fling bug.
    fn tall_platform_world(sim: &mut Sim, top: f64) -> vitric_ecs::EntityId {
        // Platform: a column of 2.0-tall blocks, the topmost block's top at y=top
        let mut y = top - 1.0;
        while y > top - 7.0 {
            let t = sim.world.spawn();
            sim.world.set_component(t, "Position", json!({"x": 8.0, "y": y})).unwrap();
            sim.world.set_component(t, "Collider", json!({"w": 2.0, "h": 2.0})).unwrap();
            sim.world.set_component(t, "Solid", json!({})).unwrap();
            y -= 2.0;
        }
        // A wall next to it (replicates the observed scenario: a flung entity cascades up along the wall column)
        let mut wy = top + 1.0;
        for _ in 0..4 {
            let wall = sim.world.spawn();
            sim.world.set_component(wall, "Position", json!({"x": 10.5, "y": wy})).unwrap();
            sim.world.set_component(wall, "Collider", json!({"w": 1.0, "h": 2.0})).unwrap();
            sim.world.set_component(wall, "Solid", json!({})).unwrap();
            wy += 2.0;
        }
        let hero = sim.world.spawn_named("hero").unwrap();
        // Drop from a little above the platform so the snap path is exercised for real
        sim.world.set_component(hero, "Position", json!({"x": 8.0, "y": top + 1.3})).unwrap();
        sim.world.set_component(hero, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(hero, "Collider", json!({"w": 1.0, "h": 2.1})).unwrap();
        sim.world
            .set_component(hero, "Body", json!({"gravity": -30.0, "grounded": false}))
            .unwrap();
        hero
    }

    /// Regression (fling bug): standing on a tall platform at top=33, the snap-written y is one ULP
    /// lower than the exact contact; the old strict < test misjudges "standing" as penetration, the x-axis hits first
    /// and flings the character sideways (observed: from top=33 it flies to (8.65, 47.05) cascading up the wall column).
    /// After the fix: stands still for 120 ticks.
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

    /// The same form has never broken at low coordinates (top=1) — lock it down to keep it that way.
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

    /// Jumping and landing back at high coordinates must also be stable: rise, land back in place, no sideways fling.
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

    /// Test logic that collects events.
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
        // 60 ticks = 1 second, mover travels 60 units, must pass through the wall and produce a collision
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
                // Mix random numbers into the logic to verify the RNG is also deterministic
                let _ = sim.rng.next_f64();
                sim.step(&mut logic).unwrap();
            }
            sim.world.state_hash()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn record_then_replay_verifies() {
        // Record a run
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

        // Replay with the same initial conditions: passes
        let mut sim2 = Sim::new(7);
        moving_world(&mut sim2);
        sim2.replay(&rec, &mut ()).unwrap();
        assert_eq!(sim2.world.state_hash(), rec.final_hash);

        // Initial state tampered: divergence is reported at the start
        let mut sim3 = Sim::new(7);
        moving_world(&mut sim3);
        let m = sim3.world.entity("mover").unwrap();
        sim3.world.set_field(m, "Position.x", json!(0.5)).unwrap();
        let err = sim3.replay(&rec, &mut ()).unwrap_err();
        assert!(matches!(err, SimError::ReplayDiverged { tick: 0, .. }), "{err}");
    }

    #[test]
    fn replay_detects_midway_divergence() {
        /// "Non-deterministic" logic that secretly mutates the world at tick 70
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
        // Divergence happens at tick 70 and should be caught by the tick 120 checkpoint (rather than dragging to the end)
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
        // Consumed = cleared: the second step must not see it again
        logic.0.clear();
        sim.step(&mut logic).unwrap();
        assert!(logic.0.iter().all(|e| e.name != "llm-reply"));
    }

    /// Logic that writes the llm-reply's text into the world — the reply content really affects the state hash,
    /// so dropping the reply during replay necessarily diverges (this is the invariant we want to lock down).
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
        // Record a run: at tick 30 inject an LLM reply that affects world state
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

        // Replay: replies injected from the recording (no network touched), checkpoint-by-checkpoint match + final hash matches
        let mut sim2 = Sim::new(7);
        moving_world(&mut sim2);
        sim2.replay(&rec, &mut ApplyReply).unwrap();
        assert_eq!(sim2.world.state_hash(), rec.final_hash);

        // Counter-example: strip the reply from the recording and replay must diverge — the reply is indeed a recorded source of state
        let mut crippled = rec.clone();
        crippled.replies.clear();
        let mut sim3 = Sim::new(7);
        moving_world(&mut sim3);
        let err = sim3.replay(&crippled, &mut ApplyReply).unwrap_err();
        assert!(matches!(err, SimError::ReplayDiverged { .. }), "{err}");
    }

    #[test]
    fn old_recording_without_replies_still_parses() {
        // Old recording JSON has no replies field: serde(default) fills empty, semantics unchanged
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

        // Run directly: the reply takes effect on the next step
        sim.step(&mut ApplyReply).unwrap();
        let h_direct = sim.world.state_hash();

        // Restore into a new sim then run: the unconsumed reply must come back as-is
        let mut sim2 = Sim::new(0);
        sim2.restore(&snap, &mut ()).unwrap();
        sim2.step(&mut ApplyReply).unwrap();
        assert_eq!(sim2.world.state_hash(), h_direct);

        // Old snapshot missing pending_replies → explicit error, no silent empty fill
        let mut old_snap = snap.clone();
        old_snap.as_object_mut().unwrap().remove("pending_replies");
        let err = sim2.restore(&old_snap, &mut ()).unwrap_err();
        assert!(err.contains("pending_replies"), "{err}");
    }

    #[test]
    fn restore_invalidates_in_progress_recording() {
        // Regression: restoring mid-recording misaligns checkpoints; keeping it would only produce a silently corrupted recording
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

        // Run another 50 ticks
        for _ in 0..50 {
            let _ = sim.rng.next_u32();
            sim.step(&mut ()).unwrap();
        }
        let h_direct = sim.world.state_hash();

        // Restore from snapshot and run another 50 ticks: must be identical
        let mut sim2 = Sim::new(0); // Intentionally wrong seed; restore must fully cover it
        sim2.restore(&snap, &mut ()).unwrap();
        for _ in 0..50 {
            let _ = sim2.rng.next_u32();
            sim2.step(&mut ()).unwrap();
        }
        assert_eq!(sim2.world.state_hash(), h_direct);
        assert_eq!(sim2.tick, sim.tick);
    }

    /// Camera-follow test bed: a uniformly moving target + a follow camera.
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
        // When the target moves uniformly, lerp follow converges to a fixed lag distance: v*DT*(1-lerp)/lerp = 1*0.8/0.2 = 4
        assert!((hx - cx - 4.0).abs() < 1e-6, "应稳定滞后 4 单位，实际差 {}", hx - cx);
        assert!(cy.abs() < 1e-6, "y 轴无运动应收敛到 0，实际 {cy}");
    }

    #[test]
    fn camera_follow_lerp_one_hard_locks_after_motion() {
        let mut sim = Sim::new(1);
        let (hero, cam) = follow_world(&mut sim, 1.0);
        sim.step(&mut ()).unwrap();
        // Follow runs after motion: the camera locks to this tick's post-motion final position, no one-frame lag
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
        // No follow field defined = no follow, runs normally
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
        // Multiplicative decay below the threshold snaps exactly to zero (no invisible state jitter left)
        let amp = sim.world.get_field(cam, "Shake.amplitude").unwrap().as_f64().unwrap();
        assert_eq!(amp, 0.0);
    }

    #[test]
    fn shake_never_perturbs_gameplay_trajectory() {
        // Same world, one with Shake one without: the mover's trajectory must be bit-identical
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

        // ttl not an integer → explicit error
        let bad = sim.world.spawn_named("bad").unwrap();
        sim.world.set_component(bad, "Particle", json!({"ttl": "forever"})).unwrap();
        let err = sim.step(&mut ()).unwrap_err();
        assert!(err.to_string().contains("整数"), "{err}");
    }

    #[test]
    fn emitter_fields_hash_but_particles_add_no_state() {
        // Particles are pure-function products of the render layer: the emitter fields themselves enter the state hash, but no matter how many ticks run,
        // they produce no extra state (no sim system touches Emitter — this test locks down that fact)
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
        // Run 120 ticks: nothing in the world is moving, the hash doesn't budge (particles are zero-state)
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        assert_eq!(sim.world.state_hash(), h_with, "粒子不许往模拟状态里塞任何东西");
        // burst trigger = a rule writing a field: hash changes (captured faithfully by recording/snapshot), still zero extra state
        sim.world.set_field(e, "Emitter.burst", json!(120)).unwrap();
        let h_burst = sim.world.state_hash();
        assert_ne!(h_burst, h_with);
        for _ in 0..120 {
            sim.step(&mut ()).unwrap();
        }
        assert_eq!(sim.world.state_hash(), h_burst);
        // snapshot/restore round-trip: hash matches (the emitter is just an ordinary component, snapshots cover it naturally)
        let snap = sim.snapshot(&());
        let mut sim2 = Sim::new(99);
        sim2.restore(&snap, &mut ()).unwrap();
        assert_eq!(sim2.world.state_hash(), h_burst);
        assert_eq!(sim2.tick, sim.tick);
    }

    // ---- Tween (Tween component, built-in system) ----

    /// Tween test bed: a panel + a tween that takes Position.x from 1 to 5.
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
        // First tick: the starting tick is recorded, the field is written to the starting value (progress = 0)
        sim.step(&mut logic).unwrap();
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(1.0));
        // Mid-flight: the value at tick T = from + (to-from)·ease(elapsed/duration), pure function with no accumulation
        for _ in 0..4 {
            sim.step(&mut logic).unwrap();
        }
        // elapsed = 4, progress = 0.5 → 1 + 4·0.5 = 3
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(3.0));
        // Expiry tick: the field is exactly equal to the final value (no floating-point tail) + completion event (with id) + tween auto-removed
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
        // Afterwards the world is still: the tween is gone, the field doesn't move
        sim.step(&mut logic).unwrap();
        assert_eq!(sim.world.get_field(panel, "Position.x").unwrap().as_f64(), Some(5.0));
    }

    #[test]
    fn tween_final_value_has_no_float_tail() {
        // 0 → 0.3 over 7 ticks: intermediate values inevitably carry floating-point tails, but at expiry it must write exactly 0.3
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
        // Latecomer: same entity same field, 100 → 200
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
        // The incumbent is replaced (no completion event, entity removed), the field follows the latecomer
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
        // Mid-flight rollback then resume: trajectory must be bit-identical (tween state is all in the component, snapshots cover it naturally)
        let mut sim2 = Sim::new(0);
        sim2.restore(&snap, &mut ()).unwrap();
        for _ in 0..40 {
            sim2.step(&mut ()).unwrap();
        }
        assert_eq!(sim2.world.state_hash(), h_direct);
    }

    #[test]
    fn tween_bad_data_is_explicit() {
        // Target entity doesn't exist
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

        // Unknown easing curve
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

        // Field is not numeric
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
        // Defensive check: Value-typed mistakes can't be dodged; the error must point to the specific field
        let _: Value = json!(null);
    }
}
