use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};
use vitric_rules::{Engine, Event, RuleSet};
use vitric_sim::{GameLogic, Sim};

use crate::saves::SaveStore;

/// Main loop pacing control (written by dispatcher, read by main loop).
#[derive(Debug, Clone, PartialEq)]
pub struct LoopCtl {
    pub paused: bool,
    /// Speed multiplier (1.0 = realtime, 0 = no upper limit — headless AI can fast-forward freely).
    pub speed: f64,
    pub quit: bool,
}

impl Default for LoopCtl {
    fn default() -> Self {
        LoopCtl { paused: false, speed: 1.0, quit: false }
    }
}

const EVENT_LOG_CAP: usize = 10_000;

/// Control plane command executor. The main loop calls `handle` each frame to drain pending requests.
pub struct Dispatcher {
    /// Used only for assertion condition evaluation and spawn validation (empty rule set + project schema).
    checker: Engine,
    /// Assertions: id -> list of condition triples (all hold = healthy).
    assertions: BTreeMap<String, Vec<(String, String, Value)>>,
    /// Assertions currently being violated (debounced: only recorded when flipping from holding to violating).
    failing: BTreeSet<String>,
    /// Assertion failure records.
    failures: Vec<Value>,
    /// Ring buffer of recent events.
    events: VecDeque<(u64, Event)>,
    /// Asset store (used by render methods; an empty store if the project has no assets).
    assets: vitric_render::Assets,
    /// Asset generation: incremented on each (re)load. The GPU presentation path uses it to decide
    /// whether the atlas needs rebuilding — comparing contents is too expensive and comparing count
    /// misses the "same name, swapped image" case.
    assets_generation: u64,
    /// Inspector-selected entity — human click and AI set share the same state, visible both ways.
    selection: Option<EntityId>,
    /// Performance budgets (0 = no limit).
    budgets: vitric_data::Budgets,
    /// Event count from the most recent step (used for budget checks).
    last_tick_events: u64,
    /// Tick corresponding to the event counter.
    events_count_tick: u64,
    /// Player save store (`vitric run` mounts `<project>/saves/`; embedded/test harnesses may skip it,
    /// in which case save-related methods explicitly error instead of silently writing to nowhere).
    saves: Option<SaveStore>,
    /// The last scene view served by render/describe (does NOT include the actions/changes delta packages;
    /// only the raw visible/offscreen/… body is stored, so the diff is stable). The next describe computes
    /// "what changed" against this. Stored per dispatcher instance (one connection/session = one dispatcher).
    last_describe: Option<Value>,
    pub ctl: LoopCtl,
}

impl Dispatcher {
    pub fn new(schema: Schema) -> Dispatcher {
        Dispatcher {
            checker: Engine::new(RuleSet::default(), schema),
            assertions: BTreeMap::new(),
            failing: BTreeSet::new(),
            failures: Vec::new(),
            events: VecDeque::new(),
            assets: vitric_render::Assets::empty(),
            assets_generation: 0,
            selection: None,
            budgets: vitric_data::Budgets::default(),
            last_tick_events: 0,
            events_count_tick: u64::MAX,
            saves: None,
            last_describe: None,
            ctl: LoopCtl::default(),
        }
    }

    /// Mount the player save store (execution end for save-game/load-game convention events and save/* RPCs).
    pub fn set_save_store(&mut self, store: SaveStore) {
        self.saves = Some(store);
    }

    pub fn set_budgets(&mut self, budgets: vitric_data::Budgets) {
        self.budgets = budgets;
    }

    /// Inspector selection state (written by window click and AI inspect/select; both sides read here).
    pub fn selection(&self) -> Option<EntityId> {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Option<EntityId>) {
        self.selection = selection;
    }

    /// Mount the project asset directory (load validates; bad images / over-budget fail immediately).
    pub fn load_assets(&mut self, dir: &std::path::Path) -> Result<(), String> {
        self.assets = vitric_render::Assets::load_dir(dir)?;
        self.assets_generation += 1;
        Ok(())
    }

    /// Mount the TTF font from the manifest `font` field (missing/corrupt fails immediately — a startup-time
    /// failure, not text silently disappearing at runtime). Once mounted, all Text goes through the vector path.
    pub fn load_font(&mut self, path: &std::path::Path) -> Result<(), String> {
        self.assets.load_font(path)?;
        // Font changes must also trigger GPU-side rebuild (the glyph atlas is rasterized per font)
        self.assets_generation += 1;
        Ok(())
    }

    /// Read-only access to the asset store (shared with the window presentation).
    pub fn assets(&self) -> &vitric_render::Assets {
        &self.assets
    }

    /// Asset generation (see field comment).
    pub fn assets_generation(&self) -> u64 {
        self.assets_generation
    }

    /// Called by the main loop after each step: records events into the ring buffer
    /// (also counts per tick for budget use).
    pub fn record_events(&mut self, tick: u64, events: &[Event]) {
        if tick != self.events_count_tick {
            self.events_count_tick = tick;
            self.last_tick_events = 0;
        }
        self.last_tick_events += events.len() as u64;
        for e in events {
            if self.events.len() == EVENT_LOG_CAP {
                self.events.pop_front();
            }
            self.events.push_back((tick, e.clone()));
        }
    }

    /// Called by the main loop after each step: checks all assertions.
    /// Returns newly occurring failures (one record on the frame flipping from holding to violating).
    pub fn check_assertions(&mut self, sim: &Sim) -> Vec<Value> {
        let mut fresh = Vec::new();
        for (id, conds) in &self.assertions {
            let healthy = match self.checker.check(&sim.world, conds) {
                Ok(ok) => ok,
                Err(e) => {
                    // An assertion failing to evaluate (e.g. the referenced entity is gone) also counts
                    // as a violation, but the reason must be made clear.
                    let record = json!({
                        "id": id, "tick": sim.tick,
                        "kind": "eval-error", "detail": e.to_string(),
                    });
                    if self.failing.insert(id.clone()) {
                        self.failures.push(record.clone());
                        fresh.push(record);
                    }
                    continue;
                }
            };
            if healthy {
                self.failing.remove(id);
            } else if self.failing.insert(id.clone()) {
                let record = json!({
                    "id": id, "tick": sim.tick,
                    "kind": "violated", "conditions": conds.iter()
                        .map(|(l, o, r)| json!([l, o, r])).collect::<Vec<_>>(),
                });
                self.failures.push(record.clone());
                fresh.push(record);
            }
        }

        // Performance budget: going over isn't silent slowdown — it's an explicit failure
        // on the same level as an assertion violation.
        let entities = sim.world.entities().len() as u64;
        let budget_checks = [
            ("budget:max_entities", self.budgets.max_entities, entities,
             "实体数超预算。提示：检查是否有规则/脚本在无限 spawn，或上调清单 budgets.max_entities"),
            ("budget:max_events_per_tick", self.budgets.max_events_per_tick, self.last_tick_events,
             "单 tick 事件数超预算，疑似事件风暴。提示：查 events/recent 看是什么事件在刷屏"),
        ];
        for (id, limit, actual, hint) in budget_checks {
            if limit == 0 {
                continue;
            }
            if actual <= limit {
                self.failing.remove(id);
            } else if self.failing.insert(id.to_string()) {
                let record = json!({
                    "id": id, "tick": sim.tick, "kind": "budget",
                    "limit": limit, "actual": actual, "hint": hint,
                });
                self.failures.push(record.clone());
                fresh.push(record);
            }
        }
        fresh
    }

    /// Called by the run loop each tick: executes the save convention events emitted
    /// by this tick's rules/scripts.
    ///
    /// Constraints (relationship with determinism):
    /// - `save-game` is a pure output side effect (like play-sound): writing to disk does
    ///   not flow back into the simulation, so when it runs has no impact on the trajectory;
    /// - `load-game` rewrites the simulation and must execute at a frame boundary (between
    ///   two ticks); it is a "session boundary" operation — it does not enter the recording
    ///   and replays do not reproduce it, so during recording it is explicitly rejected
    ///   (same guard as the sim/restore RPC: after a timeline break, the recording is
    ///   necessarily unreplayable).
    ///
    /// Returns structured error records (illegal slot name / missing file / version mismatch /
    /// recording mutex); the caller is responsible for writing them to stderr — a save failure
    /// does not crash the game, but it is never silent.
    pub fn handle_save_load_events(
        &mut self,
        observed: &[Event],
        sim: &mut Sim,
        logic: &mut dyn GameLogic,
    ) -> Vec<Value> {
        let mut errors = Vec::new();
        for e in observed {
            if e.name != "save-game" && e.name != "load-game" {
                continue;
            }
            if let Err(message) = self.run_save_event(e, sim, logic) {
                errors.push(json!({
                    "event": e.name,
                    "data": Value::Object(e.data.clone()),
                    "error": message,
                }));
            }
        }
        errors
    }

    /// Execution of a single save-game / load-game event (errors are centralized here
    /// for the caller above to wrap).
    fn run_save_event(
        &mut self,
        e: &Event,
        sim: &mut Sim,
        logic: &mut dyn GameLogic,
    ) -> Result<(), String> {
        let slot = e.data.get("slot").and_then(|v| v.as_str()).ok_or_else(|| {
            format!(
                "{} 事件缺少 slot 字段（文本）。写法: \
                 {{\"emit\": \"{}\", \"data\": {{\"slot\": \"slot1\"}}}}，实际 data: {}",
                e.name,
                e.name,
                Value::Object(e.data.clone())
            )
        })?;
        let store = self.saves.as_ref().ok_or(NO_SAVE_STORE)?;
        if e.name == "save-game" {
            store.write(slot, sim, &*logic)?;
            return Ok(());
        }
        if sim.is_recording() {
            return Err(format!(
                "正在录像：load-game（槽 {slot:?}）会把模拟跳回存档时刻，时间线断裂后\
                 录像必然不可重放——读档与录像互斥。要读档请先结束录制（去掉 --record）"
            ));
        }
        let snap = store.read(slot)?;
        sim.restore(&snap, logic)?;
        // Entity handles from the old world all become invalid on restore; clear the selection
        // too (no ghost selection left behind).
        self.selection = None;
        Ok(())
    }

    /// Handle one control request, returning the response JSON.
    pub fn handle(&mut self, request: &Value, sim: &mut Sim, logic: &mut dyn GameLogic) -> Value {
        let method = match request.get("method").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => return err_response("请求缺少 method 字段"),
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        match self.dispatch(method, &params, sim, logic) {
            Ok(result) => json!({"ok": true, "result": result}),
            Err(message) => err_response(&message),
        }
    }

    fn dispatch(
        &mut self,
        method: &str,
        params: &Value,
        sim: &mut Sim,
        logic: &mut dyn GameLogic,
    ) -> Result<Value, String> {
        // The recording only captures the input stream. These methods bypass input and mutate the
        // world/rules directly; allowing them during recording would produce a recording that
        // necessarily diverges on replay, and would also mislead debugging toward "non-determinism"
        // — explicitly reject.
        const BREAKS_RECORDING: &[&str] =
            &["world/set", "world/spawn", "world/despawn", "project/reload", "sim/restore", "save/load"];
        if sim.is_recording() && BREAKS_RECORDING.contains(&method) {
            return Err(format!(
                "正在录像：{method} 不走输入流，改了的状态不会进录像，录像会变得不可重放。\
                 要改状态请改用 input/inject（输入会被录下来），或先结束录制"
            ));
        }
        match method {
            "ping" => Ok(json!({
                "tick": sim.tick,
                "paused": self.ctl.paused,
                "speed": self.ctl.speed,
            })),

            // ---- See ----
            "world/entities" => {
                let comps: Vec<String> = params
                    .get("components")
                    .map(to_string_vec)
                    .transpose()?
                    .unwrap_or_default();
                let refs: Vec<&str> = comps.iter().map(|s| s.as_str()).collect();
                let ids = if refs.is_empty() { sim.world.entities() } else { sim.world.query(&refs) };
                Ok(json!(ids
                    .into_iter()
                    .map(|id| entity_json(&sim.world, id))
                    .collect::<Vec<_>>()))
            }
            "world/get" => {
                let id = entity_param(&sim.world, params, "entity")?;
                Ok(entity_json(&sim.world, id))
            }

            // ---- Act ----
            "world/set" => {
                let id = entity_param(&sim.world, params, "entity")?;
                let path = str_param(params, "path")?;
                let value = params.get("value").cloned().ok_or("缺少 value 参数")?;
                // Mutating state also goes through schema: mutate a copy first, validate the whole
                // component, then commit.
                let comp = path.split('.').next().expect("split 至少一段");
                let mut after = sim.world.get_component(id, comp).map_err(|e| e.to_string())?.clone();
                if comp == path {
                    after = value.clone();
                } else {
                    set_in_value(&mut after, &path[comp.len() + 1..], value)?;
                }
                // Same law as world/spawn: components outside the schema are rejected outright —
                // there is no back door that silently skips validation.
                let cschema = self.checker.schema.component(comp).ok_or_else(|| {
                    format!(
                        "未知组件 {comp:?}。schema 定义的组件: [{}]",
                        self.checker.schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                })?;
                let mut report = vitric_data::ValidationReport::default();
                let normalized = cschema.normalize(&after, &format!("world/set/{comp}"), &mut report);
                if !report.ok() {
                    return Err(format!("修改未通过 schema 校验:\n{report}"));
                }
                after = normalized;
                sim.world.set_component(id, comp, after).map_err(|e| e.to_string())?;
                Ok(json!({"entity": id.to_string()}))
            }
            "world/spawn" => {
                let comps = params
                    .get("components")
                    .and_then(|v| v.as_object())
                    .ok_or("缺少 components 对象参数")?;
                let id = match params.get("name").and_then(|v| v.as_str()) {
                    Some(name) => sim.world.spawn_named(name).map_err(|e| e.to_string())?,
                    None => sim.world.spawn(),
                };
                for (cname, cval) in comps {
                    let cschema = self.checker.schema.component(cname).ok_or_else(|| {
                        format!(
                            "未知组件 {cname:?}。schema 定义的组件: [{}]",
                            self.checker.schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
                        )
                    })?;
                    let mut report = vitric_data::ValidationReport::default();
                    let normalized = cschema.normalize(cval, &format!("world/spawn/{cname}"), &mut report);
                    if !report.ok() {
                        return Err(format!("spawn 未通过 schema 校验:\n{report}"));
                    }
                    sim.world.set_component(id, cname, normalized).map_err(|e| e.to_string())?;
                }
                Ok(json!({"entity": id.to_string()}))
            }
            "world/despawn" => {
                let id = entity_param(&sim.world, params, "entity")?;
                sim.world.despawn(id).map_err(|e| e.to_string())?;
                Ok(json!({}))
            }
            "input/inject" => {
                let action = str_param(params, "action")?;
                let phase = params.get("phase").and_then(|v| v.as_str()).unwrap_or("pressed");
                if phase != "pressed" && phase != "released" {
                    return Err(format!("phase 必须是 pressed 或 released，拿到 {phase:?}"));
                }
                sim.inject_input(&action, phase);
                Ok(json!({}))
            }
            "input/click" => {
                // Headless agent's "mouse": world-coordinate click, with pick resolution on the
                // same path as window clicking. Goes through the reply channel, so it is allowed
                // during recording — the click is recorded into the recording and replayed as-is.
                let x = params
                    .get("x")
                    .and_then(|v| v.as_f64())
                    .ok_or("缺少 x 数字参数（世界坐标）")?;
                let y = params
                    .get("y")
                    .and_then(|v| v.as_f64())
                    .ok_or("缺少 y 数字参数（世界坐标）")?;
                let button = params.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                inject_click(sim, x, y, button)
            }
            "input/ui-click" => {
                // Headless agent's "UI click": screen-normalized coordinates (0..1); picking is
                // deferred to the in-tick UI interaction system (converted to the 1920×1080 reference
                // frame then compared against UI rects). Goes through the reply channel, allowed during recording.
                let nx = params
                    .get("nx")
                    .and_then(|v| v.as_f64())
                    .ok_or("缺少 nx 数字参数（屏幕归一化坐标 0..1）")?;
                let ny = params
                    .get("ny")
                    .and_then(|v| v.as_f64())
                    .ok_or("缺少 ny 数字参数（屏幕归一化坐标 0..1）")?;
                let button = params.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                inject_ui_click(sim, nx, ny, button)
            }

            // ---- Control time ----
            "sim/pause" => {
                self.ctl.paused = true;
                Ok(json!({"tick": sim.tick}))
            }
            "sim/resume" => {
                self.ctl.paused = false;
                Ok(json!({"tick": sim.tick}))
            }
            "sim/step" => {
                let ticks = params.get("ticks").and_then(|v| v.as_u64()).unwrap_or(1);
                if !self.ctl.paused {
                    return Err("sim/step 只在暂停状态下可用（先 sim/pause），否则和自由运行的帧搅在一起".into());
                }
                let mut new_failures = Vec::new();
                for _ in 0..ticks {
                    let report = sim.step(logic).map_err(|e| e.to_string())?;
                    self.record_events(report.tick, &report.events);
                    let observed = logic.drain_observed();
                    self.record_events(report.tick, &observed);
                    new_failures.extend(self.check_assertions(sim));
                }
                Ok(json!({"tick": sim.tick, "assert_failures": new_failures}))
            }
            "sim/speed" => {
                let speed = params.get("multiplier").and_then(|v| v.as_f64()).ok_or("缺少 multiplier 数字参数")?;
                if speed <= 0.0 {
                    return Err(format!("multiplier 必须 > 0，拿到 {speed}。暂停请用 sim/pause"));
                }
                self.ctl.speed = speed;
                Ok(json!({"speed": speed}))
            }
            "sim/quit" => {
                self.ctl.quit = true;
                Ok(json!({}))
            }

            // ---- Hot reload (after AI edits rules/scripts/assets, takes effect in milliseconds;
            //      world state is untouched) ----
            "project/reload" => {
                let mut summary = logic.reload()?;
                // If an asset directory is mounted, reload it too; on failure explain separately
                // (the rules/scripts have already been swapped in).
                match self.assets.reload() {
                    Ok(()) => self.assets_generation += 1,
                    Err(e) if e.contains("没有目录") => {} // no asset directory mounted, legal skip
                    Err(e) => {
                        return Err(format!("规则/脚本已重载，但素材重载失败: {e}"));
                    }
                }
                summary["assets"] = json!(self.assets.count());
                Ok(summary)
            }

            // ---- Snapshot / hash ----
            "sim/snapshot" => Ok(sim.snapshot(logic)),
            "sim/restore" => {
                let snap = params.get("snapshot").ok_or("缺少 snapshot 参数")?;
                sim.restore(snap, logic)?;
                Ok(json!({"tick": sim.tick}))
            }
            "sim/hash" => Ok(json!(format!("{:#018x}", sim.world.state_hash()))),

            // ---- Player saves (saves/<slot>.json; same code path as the save-game/load-game
            //      convention events. save/write is a pure output side effect, allowed during recording;
            //      save/load is equivalent to sim/restore and is subject to the same guard) ----
            "save/write" => {
                let slot = str_param(params, "slot")?;
                let store = self.saves.as_ref().ok_or(NO_SAVE_STORE)?;
                store.write(&slot, sim, &*logic)
            }
            "save/load" => {
                let slot = str_param(params, "slot")?;
                let store = self.saves.as_ref().ok_or(NO_SAVE_STORE)?;
                let snap = store.read(&slot)?;
                sim.restore(&snap, logic)?;
                // Entity handles from the old world all become invalid on restore; clear the selection too.
                self.selection = None;
                Ok(json!({"slot": slot, "tick": sim.tick}))
            }
            "save/list" => {
                let store = self.saves.as_ref().ok_or(NO_SAVE_STORE)?;
                Ok(json!(store.list()?))
            }

            // ---- Observation (semantic description is the main channel; screenshot is fallback verification) ----
            "render/describe" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(320) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(240) as u32;
                // Carry the asset store: text contrast measurement renders the background against the
                // real texture (falls back to a solid-color approximation only when the image is missing).
                let mut out =
                    vitric_render::describe_world_with_assets(&sim.world, width, height, &self.assets)?;

                // Inter-frame delta: relative to the last frame served by describe, only describe
                // "what changed". Included by default (interactive consumption mostly wants "did anything
                // change since last time"); on the first call there is no previous frame, so the changes
                // key is not added (backward compatible: existing visible/offscreen/… fields unchanged).
                // The changes are computed against the previous frame's raw body (without actions/changes),
                // and the current frame body is also stored as the raw body only — delta packages do not
                // enter the diff, so the diff stays stable.
                if let Some(prev) = &self.last_describe {
                    out["changes"] = vitric_ecs::scene_delta(prev, &out);
                }
                self.last_describe = Some(out.clone());

                // Optionally fold actions into describe: describe doesn't just say "what's on screen /
                // where", it also says "what can you do" — unified with the playtest SceneView affordance.
                // The dispatcher holds a dyn GameLogic and cannot reach the rules, so the action vocabulary
                // is handed up via GameLogic::available_actions (non-empty for pure-rule logic, empty by
                // default otherwise). Shape aligns with SceneView's actions: each action × {pressed, released}
                // is flattened out. goal is a playtest.json concept and the control plane does not load it,
                // so it is not force-injected here — goal stays only on the SceneView/playtest side; the
                // control plane is not coupled to playtest config just for goal.
                let mut actions = Vec::new();
                for (action, _phases) in logic.available_actions() {
                    actions.push(json!({"action": action, "phase": "pressed"}));
                    actions.push(json!({"action": action, "phase": "released"}));
                }
                out["actions"] = Value::Array(actions);

                Ok(out)
            }
            "render/screenshot" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(320) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(240) as u32;
                let png =
                    vitric_render::screenshot_png(&sim.world, width, height, &self.assets, sim.tick)?;
                let mut result = serde_json::Map::new();
                result.insert("width".into(), json!(width));
                result.insert("height".into(), json!(height));
                result.insert("bytes".into(), json!(png.len()));
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    std::fs::write(path, &png).map_err(|e| format!("写截图 {path} 失败: {e}"))?;
                    result.insert("path".into(), json!(path));
                }
                if params.get("inline").and_then(|v| v.as_bool()).unwrap_or(false) {
                    result.insert("png_base64".into(), json!(base64(&png)));
                }
                Ok(Value::Object(result))
            }

            // ---- Performance observation ----
            "perf/stats" => Ok(json!({
                "tick": sim.tick,
                "entities": sim.world.entities().len(),
                "events_last_tick": self.last_tick_events,
                "assets": {"count": self.assets.count(), "decoded_kb": self.assets.total_bytes() / 1024},
                "budgets": {
                    "max_entities": self.budgets.max_entities,
                    "max_events_per_tick": self.budgets.max_events_per_tick,
                },
            })),

            // ---- Inspector (human points, AI sees; and vice versa) ----
            "inspect/selection" => match self.selection.filter(|&id| sim.world.is_alive(id)) {
                Some(id) => Ok(json!({"selected": entity_json(&sim.world, id)})),
                None => Ok(json!({"selected": null})),
            },
            "inspect/select" => {
                if params.get("entity").is_some_and(|v| v.is_null()) {
                    self.selection = None;
                    return Ok(json!({"selected": null}));
                }
                let id = entity_param(&sim.world, params, "entity")?;
                self.selection = Some(id);
                Ok(json!({"selected": entity_json(&sim.world, id)}))
            }

            // ---- Events ----
            "events/recent" => {
                let since = params.get("since").and_then(|v| v.as_u64()).unwrap_or(0);
                Ok(json!(self
                    .events
                    .iter()
                    .filter(|(t, _)| *t >= since)
                    .map(|(t, e)| json!({"tick": t, "name": e.name, "data": e.data}))
                    .collect::<Vec<_>>()))
            }

            // ---- Test ----
            "assert/add" => {
                let id = str_param(params, "id")?;
                let conds = parse_conditions(params.get("if").ok_or("缺少 if 条件数组")?)?;
                self.assertions.insert(id.clone(), conds);
                self.failing.remove(&id);
                Ok(json!({"id": id}))
            }
            "assert/remove" => {
                let id = str_param(params, "id")?;
                if self.assertions.remove(&id).is_none() {
                    return Err(format!(
                        "没有 id 为 {id:?} 的断言。现有断言: [{}]",
                        self.assertions.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
                self.failing.remove(&id);
                Ok(json!({}))
            }
            "assert/list" => Ok(json!(self
                .assertions
                .iter()
                .map(|(id, conds)| json!({
                    "id": id,
                    "if": conds.iter().map(|(l, o, r)| json!([l, o, r])).collect::<Vec<_>>(),
                    "failing": self.failing.contains(id),
                }))
                .collect::<Vec<_>>())),
            "assert/failures" => Ok(json!(self.failures)),

            other => Err(format!(
                "未知方法 {other:?}。可用方法: ping, world/entities, world/get, world/set, \
                 world/spawn, world/despawn, input/inject, input/click, input/ui-click, sim/pause, sim/resume, sim/step, \
                 sim/speed, sim/quit, sim/snapshot, sim/restore, sim/hash, project/reload, \
                 save/write, save/load, save/list, \
                 inspect/selection, inspect/select, events/recent, perf/stats, render/describe, \
                 render/screenshot, assert/add, assert/remove, assert/list, assert/failures"
            )),
        }
    }
}

/// Inject a mouse click into the simulation — the single shared path used by both
/// window clicks and the `input/click` RPC.
///
/// Coordinates are **world coordinates**; picking happens at (x,y) (same hit rules as
/// window clicking, see `vitric_render::pick_world`); the injected event data is
/// `{x, y, entity}`: entity = the hit entity's name (handle text for unnamed entities),
/// or null if nothing was hit. `button`: "left" → event `mouse`, "right" → event `mouse-alt`.
///
/// Goes through the reply channel (`Sim::inject_reply`): the recording channel on the same
/// level as LLM replies — the click together with the tick and pick result enter the recording
/// (`Recording.replies`), replays inject it at the original tick as-is, and the snapshot includes
/// undigested clicks. So recordings of click-driven games replay bit-for-bit offline, with zero
/// new machinery.
pub fn inject_click(sim: &mut Sim, x: f64, y: f64, button: &str) -> Result<Value, String> {
    let event = match button {
        "left" => "mouse",
        "right" => "mouse-alt",
        other => return Err(format!("button 必须是 left 或 right，拿到 {other:?}")),
    };
    let (entity, comp) = match vitric_render::pick_world(&sim.world, x, y)? {
        Some(id) => {
            let ent = match sim.world.name_of(id) {
                Some(name) => json!(name),
                None => json!(id.to_string()),
            };
            // Also carry the hit entity's components into the event — scripts use this to know what was
            // hit (is it a plot, does it have Crop), then act on it via ctx.setField(event.entity, ...).
            // For anonymous entities the entity is the handle text, still addressable.
            let mut comps = serde_json::Map::new();
            for name in sim.world.components_of(id) {
                comps.insert(
                    name.clone(),
                    sim.world.get_component(id, &name).expect("components_of 列出").clone(),
                );
            }
            (ent, Value::Object(comps))
        }
        None => (Value::Null, Value::Null),
    };
    sim.inject_reply(event, json!({"x": x, "y": y, "entity": entity, "comp": comp}));
    Ok(json!({"event": event, "entity": entity}))
}

/// Inject a **UI click** into the simulation (screen-space overlay picking) — the single
/// shared path used by both window UI clicks and the `input/ui-click` RPC.
///
/// Unlike [`inject_click`] (world-coordinate Sprite picking), this is a **different coordinate
/// system**: UI is anchored to the viewport, with layout rects in the 1920×1080 reference frame
/// (not through the camera). So what is injected here are **screen-normalized coordinates**
/// `(nx, ny) ∈ [0,1]` (= physical pixels / viewport size); picking is **not done at injection
/// time** — it is deferred to the deterministic in-tick UI interaction system
/// ([`vitric_cli`'s `advance_ui_interaction`]) which multiplies (nx,ny) back to the 1920×1080
/// reference frame and compares against UI rects. This way hit testing always targets the same
/// reference-frame rects, decoupled from the real resolution, and replays are bit-for-bit identical.
///
/// Goes through the reply channel (`Sim::inject_reply`): the click together with the tick enters
/// the recording (`Recording.replies`) and replays inject it at the original tick as-is — recordings
/// of UI-click-driven games replay bit-for-bit offline, with zero new machinery (same as [`inject_click`]).
pub fn inject_ui_click(sim: &mut Sim, nx: f64, ny: f64, button: &str) -> Result<Value, String> {
    if !matches!(button, "left" | "right") {
        return Err(format!("button 必须是 left 或 right，拿到 {button:?}"));
    }
    sim.inject_reply("ui-click", json!({"nx": nx, "ny": ny, "button": button}));
    Ok(json!({"event": "ui-click", "nx": nx, "ny": ny, "button": button}))
}

/// Unified error when no save store is mounted (embedded/test harnesses may skip it; `vitric run` always mounts one).
const NO_SAVE_STORE: &str =
    "该运行时没有挂存档目录（嵌入式/测试装配）。vitric run 会自动挂 <项目>/saves/";

fn err_response(message: &str) -> Value {
    json!({"ok": false, "error": message})
}

/// Standard base64 (padding-free dependency, 20 lines of self-implementation saves a crate).
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

fn str_param(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| format!("缺少 {key} 字符串参数"))
}

fn to_string_vec(v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().map(String::from).ok_or_else(|| format!("{x} 不是字符串")))
                .collect()
        })
        .ok_or("components 必须是字符串数组")?
}

/// Entity parameter parsing: "e3v1" handle or "@name".
fn entity_param(world: &World, params: &Value, key: &str) -> Result<EntityId, String> {
    let s = str_param(params, key)?;
    let id = if let Some(name) = s.strip_prefix('@') {
        world.entity(name).map_err(|e| e.to_string())?
    } else {
        s.parse::<EntityId>()?
    };
    if !world.is_alive(id) {
        return Err(format!("实体 {id} 已不存在（句柄过期或已销毁）"));
    }
    Ok(id)
}

fn entity_json(world: &World, id: EntityId) -> Value {
    let mut comps = serde_json::Map::new();
    for name in world.components_of(id) {
        comps.insert(name.clone(), world.get_component(id, &name).expect("components_of 列出").clone());
    }
    let mut obj = json!({"id": id.to_string(), "components": comps});
    if let Some(name) = world.name_of(id) {
        obj["name"] = json!(name);
    }
    obj
}

/// Condition array parsing (same format as rules: [[left, operator, right], ...]).
fn parse_conditions(v: &Value) -> Result<Vec<(String, String, Value)>, String> {
    let arr = v.as_array().ok_or("if 必须是条件数组，如 [[\"@player.Health.hp\", \">=\", 0]]")?;
    let mut out = Vec::new();
    for (i, cond) in arr.iter().enumerate() {
        let parts = cond.as_array().filter(|p| p.len() == 2 || p.len() == 3);
        let parts = parts.ok_or_else(|| {
            format!("if[{i}] 必须是 [路径, 操作符, 值] 三元组（exists/!exists 可两元）")
        })?;
        let left = parts[0].as_str().ok_or_else(|| format!("if[{i}][0] 必须是路径字符串"))?;
        let op = parts[1].as_str().ok_or_else(|| format!("if[{i}][1] 必须是操作符字符串"))?;
        out.push((left.to_string(), op.to_string(), parts.get(2).cloned().unwrap_or(Value::Null)));
    }
    Ok(out)
}

/// Write into a component value by relative path (intermediate path must already exist).
fn set_in_value(root: &mut Value, rel_path: &str, value: Value) -> Result<(), String> {
    let mut cur = root;
    let segs: Vec<&str> = rel_path.split('.').collect();
    let (last, mids) = segs.split_last().ok_or("路径为空")?;
    for seg in mids {
        cur = match cur {
            Value::Object(m) => m.get_mut(*seg).ok_or_else(|| format!("没有字段 {seg:?}"))?,
            Value::Array(a) => {
                let i: usize = seg.parse().map_err(|_| format!("{seg:?} 不是数组下标"))?;
                a.get_mut(i).ok_or_else(|| format!("下标 {i} 越界"))?
            }
            _ => return Err(format!("无法在标量里继续取 {seg:?}")),
        };
    }
    match cur {
        Value::Object(m) => {
            if !m.contains_key(*last) {
                return Err(format!(
                    "没有字段 {last:?}，现有字段: [{}]",
                    m.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            m.insert(last.to_string(), value);
        }
        Value::Array(a) => {
            let i: usize = last.parse().map_err(|_| format!("{last:?} 不是数组下标"))?;
            let len = a.len();
            *a.get_mut(i).ok_or_else(|| format!("下标 {i} 越界（长度 {len}）"))? = value;
        }
        _ => return Err(format!("无法往标量里写 {last:?}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use vitric_data::Schema;

    use super::*;

    fn schema() -> Schema {
        Schema::parse(
            &json!({"components": {
                "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Velocity": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Sprite": {"fields": {"w": {"type":"number"}, "h": {"type":"number"}}},
                "Health": {"fields": {"hp": {"type":"int", "default": 100, "min": 0, "max": 100}}}
            }}),
            "schema.json",
        )
        .unwrap()
    }

    fn setup() -> (Dispatcher, Sim) {
        let mut sim = Sim::new(1);
        let p = sim.world.spawn_named("player").unwrap();
        sim.world.set_component(p, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(p, "Velocity", json!({"x": 60.0, "y": 0.0})).unwrap();
        sim.world.set_component(p, "Health", json!({"hp": 100})).unwrap();
        (Dispatcher::new(schema()), sim)
    }

    fn call(d: &mut Dispatcher, sim: &mut Sim, method: &str, params: Value) -> Value {
        let resp = d.handle(&json!({"method": method, "params": params}), sim, &mut ());
        assert_eq!(resp["ok"], json!(true), "{method} 应成功: {resp}");
        resp["result"].clone()
    }

    fn call_err(d: &mut Dispatcher, sim: &mut Sim, method: &str, params: Value) -> String {
        let resp = d.handle(&json!({"method": method, "params": params}), sim, &mut ());
        assert_eq!(resp["ok"], json!(false), "{method} 应失败: {resp}");
        resp["error"].as_str().expect("error 是字符串").to_string()
    }

    #[test]
    fn observe_world() {
        let (mut d, mut sim) = setup();
        let all = call(&mut d, &mut sim, "world/entities", json!({}));
        assert_eq!(all.as_array().unwrap().len(), 1);
        let got = call(&mut d, &mut sim, "world/get", json!({"entity": "@player"}));
        assert_eq!(got["name"], json!("player"));
        assert_eq!(got["components"]["Health"]["hp"], json!(100));
    }

    #[test]
    fn recording_rejects_out_of_band_mutation() {
        // Regression: the recording only captures the input stream; allowing world/set during
        // recording would produce a recording that is non-replayable by construction.
        let (mut d, mut sim) = setup();
        sim.start_recording();
        let e = call_err(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 50}));
        assert!(e.contains("录像"), "{e}");
        let e = call_err(&mut d, &mut sim, "world/spawn", json!({"components": {"Health": {}}}));
        assert!(e.contains("录像"), "{e}");
        let snap = sim.snapshot(&());
        let e = call_err(&mut d, &mut sim, "sim/restore", json!({"snapshot": snap}));
        assert!(e.contains("录像"), "{e}");
        // Input goes through the recording stream, allowed as usual.
        call(&mut d, &mut sim, "input/inject", json!({"action": "right"}));
        // The world is not polluted.
        let p = sim.world.entity("player").unwrap();
        assert_eq!(sim.world.get_field(p, "Health.hp").unwrap(), &json!(100));
        // After stopping recording, mutation is available again.
        sim.stop_recording();
        call(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 50}));
    }

    #[test]
    fn mutate_with_schema_enforcement() {
        let (mut d, mut sim) = setup();
        call(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 50}));
        let p = sim.world.entity("player").unwrap();
        assert_eq!(sim.world.get_field(p, "Health.hp").unwrap(), &json!(50));
        // Out-of-range writes are blocked by schema; the world is not polluted.
        let e = call_err(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 999}));
        assert!(e.contains("schema"), "{e}");
        assert_eq!(sim.world.get_field(p, "Health.hp").unwrap(), &json!(50));
        // Spawning an unknown component errors and lists known components.
        let e = call_err(&mut d, &mut sim, "world/spawn", json!({"components": {"Ghost": {}}}));
        assert!(e.contains("Health"), "{e}");
    }

    #[test]
    fn time_control_and_step() {
        let (mut d, mut sim) = setup();
        // Stepping without pausing → rejected.
        let e = call_err(&mut d, &mut sim, "sim/step", json!({"ticks": 10}));
        assert!(e.contains("sim/pause"), "{e}");
        call(&mut d, &mut sim, "sim/pause", json!({}));
        call(&mut d, &mut sim, "sim/step", json!({"ticks": 60}));
        let p = sim.world.entity("player").unwrap();
        let x = sim.world.get_field(p, "Position.x").unwrap().as_f64().unwrap();
        assert!((x - 60.0).abs() < 1e-9, "60 tick 后 x 应为 60，实际 {x}");
        // Speed multiplier parameter validation.
        let e = call_err(&mut d, &mut sim, "sim/speed", json!({"multiplier": -1}));
        assert!(e.contains("> 0"), "{e}");
    }

    #[test]
    fn snapshot_restore_via_rpc() {
        let (mut d, mut sim) = setup();
        call(&mut d, &mut sim, "sim/pause", json!({}));
        let snap = call(&mut d, &mut sim, "sim/snapshot", json!({}));
        let h0 = call(&mut d, &mut sim, "sim/hash", json!({}));
        call(&mut d, &mut sim, "sim/step", json!({"ticks": 30}));
        assert_ne!(call(&mut d, &mut sim, "sim/hash", json!({})), h0);
        call(&mut d, &mut sim, "sim/restore", json!({"snapshot": snap}));
        assert_eq!(call(&mut d, &mut sim, "sim/hash", json!({})), h0, "恢复后哈希必须一致");
    }

    #[test]
    fn assertions_catch_violation_once() {
        let (mut d, mut sim) = setup();
        call(&mut d, &mut sim, "assert/add", json!({
            "id": "player-not-too-far",
            "if": [["@player.Position.x", "<", 30.0]]
        }));
        call(&mut d, &mut sim, "sim/pause", json!({}));
        // 60 ticks at speed 60/s: after 30 ticks x=30, out of bounds.
        let result = call(&mut d, &mut sim, "sim/step", json!({"ticks": 60}));
        let failures = result["assert_failures"].as_array().unwrap();
        assert_eq!(failures.len(), 1, "持续违反只记一次（去抖）: {failures:?}");
        assert_eq!(failures[0]["id"], json!("player-not-too-far"));
        assert_eq!(failures[0]["tick"], json!(30), "应记录首次违反的 tick");
        let listed = call(&mut d, &mut sim, "assert/list", json!({}));
        assert_eq!(listed[0]["failing"], json!(true));
    }

    #[test]
    fn input_and_events_flow() {
        let (mut d, mut sim) = setup();
        call(&mut d, &mut sim, "input/inject", json!({"action": "jump"}));
        call(&mut d, &mut sim, "sim/pause", json!({}));
        call(&mut d, &mut sim, "sim/step", json!({}));
        let events = call(&mut d, &mut sim, "events/recent", json!({}));
        let arr = events.as_array().unwrap();
        assert!(
            arr.iter().any(|e| e["name"] == json!("input") && e["data"]["action"] == json!("jump")),
            "{arr:?}"
        );
        let e = call_err(&mut d, &mut sim, "input/inject", json!({"action": "x", "phase": "held"}));
        assert!(e.contains("pressed"), "{e}");
    }

    #[test]
    fn budget_violation_reports_once_like_assertion() {
        let (mut d, mut sim) = setup();
        d.set_budgets(serde_json::from_value(json!({"max_entities": 2})).unwrap());
        // 1 entity: healthy.
        assert!(d.check_assertions(&sim).is_empty());
        // Blow the budget.
        sim.world.spawn();
        sim.world.spawn();
        let fresh = d.check_assertions(&sim);
        assert_eq!(fresh.len(), 1, "{fresh:?}");
        assert_eq!(fresh[0]["kind"], json!("budget"));
        assert_eq!(fresh[0]["limit"], json!(2));
        assert_eq!(fresh[0]["actual"], json!(3));
        // Continued overage is not reported again (debounced).
        assert!(d.check_assertions(&sim).is_empty());
        // perf/stats is observable.
        let stats = call(&mut d, &mut sim, "perf/stats", json!({}));
        assert_eq!(stats["entities"], json!(3));
        assert_eq!(stats["budgets"]["max_entities"], json!(2));
    }

    #[test]
    fn inspect_selection_two_way() {
        let (mut d, mut sim) = setup();
        // No selection initially.
        let r = call(&mut d, &mut sim, "inspect/selection", json!({}));
        assert_eq!(r["selected"], json!(null));
        // AI sets the selection (equivalent to a human clicking in the window).
        let r = call(&mut d, &mut sim, "inspect/select", json!({"entity": "@player"}));
        assert_eq!(r["selected"]["name"], json!("player"));
        assert!(d.selection().is_some());
        // After the entity is destroyed, the selection auto-invalidates.
        let p = sim.world.entity("player").unwrap();
        sim.world.despawn(p).unwrap();
        let r = call(&mut d, &mut sim, "inspect/selection", json!({}));
        assert_eq!(r["selected"], json!(null));
        // Clear the selection.
        d.set_selection(None);
        let r = call(&mut d, &mut sim, "inspect/select", json!({"entity": null}));
        assert_eq!(r["selected"], json!(null));
    }

    fn temp_save_root(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vitric-dispatch-save-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_rpcs_roundtrip_and_clear_selection() {
        let (mut d, mut sim) = setup();
        let root = temp_save_root("roundtrip");
        d.set_save_store(SaveStore::new(&root, "rpc-test"));

        let written = call(&mut d, &mut sim, "save/write", json!({"slot": "s1"}));
        assert_eq!(written["slot"], json!("s1"));
        assert!(root.join("saves/s1.json").exists());
        assert_eq!(call(&mut d, &mut sim, "save/list", json!({})), json!(["s1"]));

        // Mutate world + select an entity; after load the state rolls back and selection clears.
        let h0 = call(&mut d, &mut sim, "sim/hash", json!({}));
        call(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 5}));
        call(&mut d, &mut sim, "inspect/select", json!({"entity": "@player"}));
        assert_ne!(call(&mut d, &mut sim, "sim/hash", json!({})), h0);
        call(&mut d, &mut sim, "save/load", json!({"slot": "s1"}));
        assert_eq!(call(&mut d, &mut sim, "sim/hash", json!({})), h0, "读档后哈希必须回到存档时刻");
        assert!(d.selection().is_none(), "restore 后旧句柄全部失效，选中必须清空");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_load_rejected_while_recording_but_write_allowed() {
        let (mut d, mut sim) = setup();
        let root = temp_save_root("recording");
        d.set_save_store(SaveStore::new(&root, "rpc-test"));
        call(&mut d, &mut sim, "save/write", json!({"slot": "s1"}));
        sim.start_recording();
        // Loading = timeline break; rejected during recording (same guard as sim/restore).
        let e = call_err(&mut d, &mut sim, "save/load", json!({"slot": "s1"}));
        assert!(e.contains("录像"), "{e}");
        assert!(sim.is_recording(), "拒绝后录像必须仍然有效");
        // Writing a save is a pure output side effect; allowed during recording.
        call(&mut d, &mut sim, "save/write", json!({"slot": "s2"}));
        assert!(root.join("saves/s2.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_rpcs_explicit_errors() {
        let (mut d, mut sim) = setup();
        // No save store mounted: explicit error rather than silently writing to nowhere.
        let e = call_err(&mut d, &mut sim, "save/write", json!({"slot": "s1"}));
        assert!(e.contains("存档目录"), "{e}");
        let root = temp_save_root("errors");
        d.set_save_store(SaveStore::new(&root, "rpc-test"));
        // Slot-name path traversal is rejected.
        let e = call_err(&mut d, &mut sim, "save/write", json!({"slot": "../evil"}));
        assert!(e.contains("不合法"), "{e}");
        // Reading a non-existent slot: error includes the existing save list.
        call(&mut d, &mut sim, "save/write", json!({"slot": "s1"}));
        let e = call_err(&mut d, &mut sim, "save/load", json!({"slot": "ghost"}));
        assert!(e.contains("ghost") && e.contains("s1"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn input_click_resolves_pick_and_injects_mouse_event() {
        let (mut d, mut sim) = setup();
        // Stationary player with a Sprite (2x2, centered at (0,0)); picking has a target.
        let p = sim.world.entity("player").unwrap();
        sim.world.set_component(p, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(p, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        call(&mut d, &mut sim, "sim/pause", json!({}));

        // Click on the player (world coordinates): the pick result is directly in the return value.
        let r = call(&mut d, &mut sim, "input/click", json!({"x": 0.5, "y": 0.5}));
        assert_eq!(r["event"], json!("mouse"));
        assert_eq!(r["entity"], json!("player"));
        // Click empty ground: entity = null.
        let r = call(&mut d, &mut sim, "input/click", json!({"x": 100.0, "y": 100.0}));
        assert_eq!(r["entity"], json!(null));
        // Right click → mouse-alt, same payload shape.
        let r = call(&mut d, &mut sim, "input/click", json!({"x": 0.0, "y": 0.0, "button": "right"}));
        assert_eq!(r["event"], json!("mouse-alt"));
        assert_eq!(r["entity"], json!("player"));

        // Next tick all three events arrive; payload is world coordinates + pick result.
        call(&mut d, &mut sim, "sim/step", json!({}));
        let events = call(&mut d, &mut sim, "events/recent", json!({}));
        let arr = events.as_array().unwrap();
        let hit = arr.iter().find(|e| e["name"] == json!("mouse") && e["data"]["entity"] == json!("player")).expect("命中事件");
        assert_eq!(hit["data"]["x"], json!(0.5));
        assert_eq!(hit["data"]["y"], json!(0.5));
        assert!(arr.iter().any(|e| e["name"] == json!("mouse") && e["data"]["entity"] == json!(null)));
        assert!(arr.iter().any(|e| e["name"] == json!("mouse-alt") && e["data"]["entity"] == json!("player")));

        // Parameter validation: missing coordinates / unknown buttons all explicitly error.
        let e = call_err(&mut d, &mut sim, "input/click", json!({"y": 1.0}));
        assert!(e.contains('x'), "{e}");
        let e = call_err(&mut d, &mut sim, "input/click", json!({"x": 0.0, "y": 0.0, "button": "middle"}));
        assert!(e.contains("left") && e.contains("right"), "{e}");
    }

    #[test]
    fn clicks_ride_reply_channel_recorded_and_replayed() {
        /// Logic that writes mouse events into the world — clicks really affect the state hash,
        /// and dropping a click on replay would necessarily diverge (the invariant to lock down).
        struct ApplyClick;
        impl GameLogic for ApplyClick {
            fn on_tick(&mut self, w: &mut World, ev: Vec<Event>, _: &mut vitric_sim::Pcg32, _: u64) -> Result<(), String> {
                for e in ev {
                    if e.name == "mouse" || e.name == "mouse-alt" {
                        let p = w.entity("player").map_err(|e| e.to_string())?;
                        w.set_component(p, "Clicked", json!({"at": Value::Object(e.data)}))
                            .map_err(|e| e.to_string())?;
                    }
                }
                Ok(())
            }
        }
        let build = || {
            let mut sim = Sim::new(11);
            let p = sim.world.spawn_named("player").unwrap();
            sim.world.set_component(p, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
            sim.world.set_component(p, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
            sim
        };
        let mut sim = build();
        sim.start_recording();
        for t in 0..90 {
            if t == 30 {
                inject_click(&mut sim, 0.0, 0.0, "left").unwrap();
            }
            if t == 60 {
                inject_click(&mut sim, 50.0, 50.0, "right").unwrap();
            }
            sim.step(&mut ApplyClick).unwrap();
        }
        let rec = sim.stop_recording().unwrap();
        // Clicks ride the reply channel: together with the tick + pick result they enter the recording.
        assert_eq!(rec.replies.len(), 2);
        assert_eq!((rec.replies[0].tick, rec.replies[0].name.as_str()), (30, "mouse"));
        assert_eq!(rec.replies[0].data["entity"], json!("player"));
        assert_eq!((rec.replies[1].tick, rec.replies[1].name.as_str()), (60, "mouse-alt"));
        assert_eq!(rec.replies[1].data["entity"], json!(null));
        // Replay: clicks are injected from the recording; check points + final hash match bit-for-bit.
        let mut sim2 = build();
        sim2.replay(&rec, &mut ApplyClick).unwrap();
        assert_eq!(sim2.world.state_hash(), rec.final_hash);
    }

    #[test]
    fn unknown_method_lists_available() {
        let (mut d, mut sim) = setup();
        let e = call_err(&mut d, &mut sim, "world/fly", json!({}));
        assert!(e.contains("world/get"), "{e}");
    }

    #[test]
    fn dead_handle_is_explicit() {
        let (mut d, mut sim) = setup();
        let p = sim.world.entity("player").unwrap();
        sim.world.despawn(p).unwrap();
        let e = call_err(&mut d, &mut sim, "world/get", json!({"entity": p.to_string()}));
        assert!(e.contains("不存在"), "{e}");
    }

    // ---- describe folds in actions + inter-frame delta ----

    /// Test logic: available_actions returns a fixed action vocabulary (simulating the Runtime
    /// introspecting the rules). The control plane holds a dyn GameLogic and cannot reach the
    /// rules, so this hook hands the actions into describe.
    struct WithActions;
    impl GameLogic for WithActions {
        fn on_tick(&mut self, _: &mut World, _: Vec<Event>, _: &mut vitric_sim::Pcg32, _: u64) -> Result<(), String> {
            Ok(())
        }
        fn available_actions(&self) -> Vec<(String, Vec<String>)> {
            vec![("left".into(), vec!["pressed".into(), "released".into()]), ("space".into(), vec!["pressed".into()])]
        }
    }

    fn call_with(
        d: &mut Dispatcher,
        sim: &mut Sim,
        logic: &mut dyn GameLogic,
        method: &str,
        params: Value,
    ) -> Value {
        let resp = d.handle(&json!({"method": method, "params": params}), sim, logic);
        assert_eq!(resp["ok"], json!(true), "{method} 应成功: {resp}");
        resp["result"].clone()
    }

    #[test]
    fn describe_carries_actions_from_logic() {
        // describe top level carries actions = the logic's action vocabulary × {pressed, released} flattened.
        let (mut d, mut sim) = setup();
        let mut logic = WithActions;
        let out = call_with(&mut d, &mut sim, &mut logic, "render/describe", json!({}));
        let acts = out["actions"].as_array().expect("describe 应有 actions");
        // 2 actions × 2 phases = 4 entries; action names come from the logic.
        assert_eq!(acts.len(), 4, "{acts:?}");
        assert_eq!(acts[0], json!({"action": "left", "phase": "pressed"}));
        assert_eq!(acts[1], json!({"action": "left", "phase": "released"}));
        assert_eq!(acts[2], json!({"action": "space", "phase": "pressed"}));
        assert_eq!(acts[3], json!({"action": "space", "phase": "released"}));
    }

    #[test]
    fn describe_actions_empty_for_logic_without_rules() {
        // The default GameLogic (()) has empty available_actions → actions is an empty array
        // (the key is still present, shape stays uniform).
        let (mut d, mut sim) = setup();
        let out = call(&mut d, &mut sim, "render/describe", json!({}));
        assert_eq!(out["actions"], json!([]), "纯逻辑无动作: {out}");
    }

    #[test]
    fn describe_changes_reflect_movement_across_two_frames() {
        // Two consecutive describes with the world mutated in between: first has no changes,
        // second's changes reflect the move/appearance.
        let (mut d, mut sim) = setup();
        // Give player a Sprite so it enters describe's visible.
        let p = sim.world.entity("player").unwrap();
        sim.world.set_component(p, "Sprite", json!({"w": 1.0, "h": 1.0})).unwrap();

        // First frame: no previous frame → no changes key (backward compatible).
        let first = call(&mut d, &mut sim, "render/describe", json!({}));
        assert!(first.get("changes").is_none(), "第一帧不该有 changes: {first}");
        // The original body fields are still present.
        assert!(first.get("visible").is_some() && first.get("camera").is_some());

        // Mutate the world: player moves right + spawn a new entity with a Sprite.
        sim.world.set_field(p, "Position.x", json!(50.0)).unwrap();
        let star = sim.world.spawn_named("star").unwrap();
        sim.world.set_component(star, "Position", json!({"x": 10.0, "y": 0.0})).unwrap();
        sim.world.set_component(star, "Sprite", json!({"w": 1.0, "h": 1.0})).unwrap();

        // Second frame: has changes, reflecting the player's move + star's appearance.
        let second = call(&mut d, &mut sim, "render/describe", json!({}));
        let changes = second.get("changes").expect("第二帧应有 changes");
        // The player's world changed (id is the handle string).
        let pid = p.to_string();
        let pchg = &changes["changed"][&pid];
        assert!(!pchg.is_null(), "player 应在 changed: {changes}");
        assert_eq!(pchg["world"][1], json!({"x": 50.0, "y": 0.0}), "新位置");
        // star appears in appeared.
        let appeared = changes["appeared"].as_array().unwrap();
        assert!(
            appeared.iter().any(|e| e["name"] == json!("star")),
            "star 应在 appeared: {appeared:?}"
        );
    }
}
