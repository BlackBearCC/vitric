use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};
use vitric_rules::{Engine, Event, RuleSet};
use vitric_sim::{GameLogic, Sim};

use crate::saves::SaveStore;

/// 主循环节奏控制（dispatcher 改，主循环读）。
#[derive(Debug, Clone, PartialEq)]
pub struct LoopCtl {
    pub paused: bool,
    /// 倍速（1.0 = 实时，0 上限不设——无头模式 AI 随便开快进）。
    pub speed: f64,
    pub quit: bool,
}

impl Default for LoopCtl {
    fn default() -> Self {
        LoopCtl { paused: false, speed: 1.0, quit: false }
    }
}

const EVENT_LOG_CAP: usize = 10_000;

/// 控制面命令执行器。主循环每帧调用 `handle` 处理积压请求。
pub struct Dispatcher {
    /// 只用于断言条件求值和 spawn 校验（空规则集 + 项目 schema）。
    checker: Engine,
    /// 断言：id -> 条件三元组列表（全部成立 = 健康）。
    assertions: BTreeMap<String, Vec<(String, String, Value)>>,
    /// 当前正在违反中的断言（去抖：从成立变违反才记一条）。
    failing: BTreeSet<String>,
    /// 断言失败记录。
    failures: Vec<Value>,
    /// 最近事件环形缓冲。
    events: VecDeque<(u64, Event)>,
    /// 素材仓库（render 方法用；项目无素材则为空仓库）。
    assets: vitric_render::Assets,
    /// 素材代次：每次（重新）加载 +1。GPU 呈现路径靠它判断图集是否需要重建——
    /// 比较内容太贵，比较 count 又抓不住"同名换图"。
    assets_generation: u64,
    /// 检查器选中的实体——人点的和 AI 设的是同一个状态，双向可见。
    selection: Option<EntityId>,
    /// 性能预算（0=不限）。
    budgets: vitric_data::Budgets,
    /// 最近一次 step 的事件数（预算检查用）。
    last_tick_events: u64,
    /// 事件计数对应的 tick。
    events_count_tick: u64,
    /// 玩家存档仓库（vitric run 挂 `<项目>/saves/`；嵌入式/测试装配可以不挂，
    /// 此时存档相关方法显式报错而不是悄悄写到不知道哪里）。
    saves: Option<SaveStore>,
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
            ctl: LoopCtl::default(),
        }
    }

    /// 挂载玩家存档仓库（save-game/load-game 约定事件与 save/* RPC 的执行端）。
    pub fn set_save_store(&mut self, store: SaveStore) {
        self.saves = Some(store);
    }

    pub fn set_budgets(&mut self, budgets: vitric_data::Budgets) {
        self.budgets = budgets;
    }

    /// 检查器选中态（窗口点选写、AI inspect/select 写，双方都读这里）。
    pub fn selection(&self) -> Option<EntityId> {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Option<EntityId>) {
        self.selection = selection;
    }

    /// 挂载项目素材目录（加载即校验，坏图/超预算立刻报错）。
    pub fn load_assets(&mut self, dir: &std::path::Path) -> Result<(), String> {
        self.assets = vitric_render::Assets::load_dir(dir)?;
        self.assets_generation += 1;
        Ok(())
    }

    /// 挂载清单 `font` 字段的 TTF 字体（缺失/损坏立刻报错——启动期失败，
    /// 不是跑起来文字消失）。挂上后所有 Text 走矢量路径。
    pub fn load_font(&mut self, path: &std::path::Path) -> Result<(), String> {
        self.assets.load_font(path)?;
        // 字体变化也要触发 GPU 侧重建（字形图集按字体栅格化）
        self.assets_generation += 1;
        Ok(())
    }

    /// 素材仓库只读访问（窗口呈现共用同一份）。
    pub fn assets(&self) -> &vitric_render::Assets {
        &self.assets
    }

    /// 素材代次（见字段注释）。
    pub fn assets_generation(&self) -> u64 {
        self.assets_generation
    }

    /// 主循环每 step 后调用：记录事件进环形缓冲（顺带按 tick 计数给预算用）。
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

    /// 主循环每 step 后调用：检查全部断言。
    /// 返回新发生的失败（从成立翻到违反的那一帧记录一条）。
    pub fn check_assertions(&mut self, sim: &Sim) -> Vec<Value> {
        let mut fresh = Vec::new();
        for (id, conds) in &self.assertions {
            let healthy = match self.checker.check(&sim.world, conds) {
                Ok(ok) => ok,
                Err(e) => {
                    // 断言本身求值失败（比如引用的实体没了）也算违反，但要说清原因
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

        // 性能预算：超了不是默默卡顿，是和断言同级的显式失败
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

    /// 运行循环每个 tick 调用：执行本 tick 规则/脚本 emit 的存档约定事件。
    ///
    /// 约束（与确定性的关系）：
    /// - `save-game` 是纯输出副作用（同 play-sound）：写盘不回流进模拟，放在哪个
    ///   时机执行都不影响轨迹；
    /// - `load-game` 会改写模拟，必须在帧边界（两个 tick 之间）执行；它是"会话
    ///   边界"操作——不进录像、回放不复现它，所以录像期间显式拒绝（同 sim/restore
    ///   的 RPC 守卫，时间线断裂后录像必然不可重放）。
    ///
    /// 返回结构化错误记录（槽名不合法/文件缺失/版本不符/录像互斥），调用方负责
    /// 打到 stderr——存档失败不崩游戏，但绝不静默。
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

    /// 单个 save-game / load-game 事件的执行（错误统一收口给上面包装）。
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
        // 旧世界的实体句柄随 restore 全部失效，选中态一并清掉（不留幽灵选中）
        self.selection = None;
        Ok(())
    }

    /// 处理一条控制请求，返回响应 JSON。
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
        // 录像只记输入流。这些方法绕过输入直接改世界/规则，录制中放行的话
        // 录出来的录像必然重放分歧，还会把排查方向误导到"非确定性"上——明确拒绝。
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

            // ---- 看 ----
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

            // ---- 动 ----
            "world/set" => {
                let id = entity_param(&sim.world, params, "entity")?;
                let path = str_param(params, "path")?;
                let value = params.get("value").cloned().ok_or("缺少 value 参数")?;
                // 改状态也过 schema：先改副本、整组件校验，再落地
                let comp = path.split('.').next().expect("split 至少一段");
                let mut after = sim.world.get_component(id, comp).map_err(|e| e.to_string())?.clone();
                if comp == path {
                    after = value.clone();
                } else {
                    set_in_value(&mut after, &path[comp.len() + 1..], value)?;
                }
                // 和 world/spawn 同一部法律：schema 外的组件直接拒，不存在静默跳过校验的后门
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
                // 无头 agent 的"鼠标"：世界坐标点击，拾取解析和窗口点选同一条路径。
                // 走回复通道所以录制中照常放行——点击会被录进录像、重放原样注入。
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

            // ---- 控时间 ----
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

            // ---- 热重载（AI 改完规则/脚本/素材，毫秒级生效，世界状态不动）----
            "project/reload" => {
                let mut summary = logic.reload()?;
                // 挂了素材目录就一起重载；失败要单独说清（规则/脚本已经换新了）
                match self.assets.reload() {
                    Ok(()) => self.assets_generation += 1,
                    Err(e) if e.contains("没有目录") => {} // 没挂素材目录，合法跳过
                    Err(e) => {
                        return Err(format!("规则/脚本已重载，但素材重载失败: {e}"));
                    }
                }
                summary["assets"] = json!(self.assets.count());
                Ok(summary)
            }

            // ---- 快照 / 哈希 ----
            "sim/snapshot" => Ok(sim.snapshot(logic)),
            "sim/restore" => {
                let snap = params.get("snapshot").ok_or("缺少 snapshot 参数")?;
                sim.restore(snap, logic)?;
                Ok(json!({"tick": sim.tick}))
            }
            "sim/hash" => Ok(json!(format!("{:#018x}", sim.world.state_hash()))),

            // ---- 玩家存档（saves/<slot>.json；与 save-game/load-game 约定事件同一条代码路径。
            //      save/write 是纯输出副作用录像期间放行；save/load 等价 sim/restore 受同一守卫）----
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
                // 旧世界的实体句柄随 restore 全部失效，选中态一并清掉
                self.selection = None;
                Ok(json!({"slot": slot, "tick": sim.tick}))
            }
            "save/list" => {
                let store = self.saves.as_ref().ok_or(NO_SAVE_STORE)?;
                Ok(json!(store.list()?))
            }

            // ---- 观察（语义描述是主通道，截图是兜底验证）----
            "render/describe" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(320) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(240) as u32;
                // 带上素材仓库：文字对比度测量按真贴图渲底色（缺图才退纯色近似）
                vitric_render::describe_world_with_assets(&sim.world, width, height, &self.assets)
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

            // ---- 性能观测 ----
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

            // ---- 检查器（人指哪 AI 看哪，反向也行）----
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

            // ---- 事件 ----
            "events/recent" => {
                let since = params.get("since").and_then(|v| v.as_u64()).unwrap_or(0);
                Ok(json!(self
                    .events
                    .iter()
                    .filter(|(t, _)| *t >= since)
                    .map(|(t, e)| json!({"tick": t, "name": e.name, "data": e.data}))
                    .collect::<Vec<_>>()))
            }

            // ---- 测 ----
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
                 world/spawn, world/despawn, input/inject, input/click, sim/pause, sim/resume, sim/step, \
                 sim/speed, sim/quit, sim/snapshot, sim/restore, sim/hash, project/reload, \
                 save/write, save/load, save/list, \
                 inspect/selection, inspect/select, events/recent, perf/stats, render/describe, \
                 render/screenshot, assert/add, assert/remove, assert/list, assert/failures"
            )),
        }
    }
}

/// 把一次鼠标点击注入模拟——窗口点击和 `input/click` RPC 共用的唯一路径。
///
/// 坐标是**世界坐标**；在 (x,y) 做拾取（同窗口点选的命中规则，见
/// `vitric_render::pick_world`），注入事件 data 为 `{x, y, entity}`：
/// entity = 命中实体的名字（无名实体用句柄文本），没命中 = null。
/// `button`: "left" → 事件 `mouse`，"right" → 事件 `mouse-alt`。
///
/// 走回复通道（`Sim::inject_reply`）：与 LLM 回复同级的录制通道——点击连同
/// tick、拾取结果一起进录像（`Recording.replies`）、重放原 tick 原样注入、
/// 快照含未消化的点击。所以点击驱动的游戏录像离线重放逐位一致，零新机制。
pub fn inject_click(sim: &mut Sim, x: f64, y: f64, button: &str) -> Result<Value, String> {
    let event = match button {
        "left" => "mouse",
        "right" => "mouse-alt",
        other => return Err(format!("button 必须是 left 或 right，拿到 {other:?}")),
    };
    let entity = match vitric_render::pick_world(&sim.world, x, y)? {
        Some(id) => match sim.world.name_of(id) {
            Some(name) => json!(name),
            None => json!(id.to_string()),
        },
        None => Value::Null,
    };
    sim.inject_reply(event, json!({"x": x, "y": y, "entity": entity}));
    Ok(json!({"event": event, "entity": entity}))
}

/// 没挂存档仓库时的统一报错（嵌入式/测试装配可能不挂；`vitric run` 一定会挂）。
const NO_SAVE_STORE: &str =
    "该运行时没有挂存档目录（嵌入式/测试装配）。vitric run 会自动挂 <项目>/saves/";

fn err_response(message: &str) -> Value {
    json!({"ok": false, "error": message})
}

/// 标准 base64（无填充依赖，20 行自实现省一个 crate）。
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

/// 实体参数解析："e3v1" 句柄或 "@名字"。
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

/// 条件数组解析（同规则格式: [[左, 操作符, 右], ...]）。
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

/// 在组件值内按相对路径写入（中间路径必须存在）。
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
        // 回归：录像只记输入流，录制中放行 world/set 等于产出天生不可重放的录像
        let (mut d, mut sim) = setup();
        sim.start_recording();
        let e = call_err(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 50}));
        assert!(e.contains("录像"), "{e}");
        let e = call_err(&mut d, &mut sim, "world/spawn", json!({"components": {"Health": {}}}));
        assert!(e.contains("录像"), "{e}");
        let snap = sim.snapshot(&());
        let e = call_err(&mut d, &mut sim, "sim/restore", json!({"snapshot": snap}));
        assert!(e.contains("录像"), "{e}");
        // 输入走录像流，照常放行
        call(&mut d, &mut sim, "input/inject", json!({"action": "right"}));
        // 世界没被污染
        let p = sim.world.entity("player").unwrap();
        assert_eq!(sim.world.get_field(p, "Health.hp").unwrap(), &json!(100));
        // 停止录制后恢复可改
        sim.stop_recording();
        call(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 50}));
    }

    #[test]
    fn mutate_with_schema_enforcement() {
        let (mut d, mut sim) = setup();
        call(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 50}));
        let p = sim.world.entity("player").unwrap();
        assert_eq!(sim.world.get_field(p, "Health.hp").unwrap(), &json!(50));
        // 越界写被 schema 拦住，世界不被污染
        let e = call_err(&mut d, &mut sim, "world/set", json!({"entity": "@player", "path": "Health.hp", "value": 999}));
        assert!(e.contains("schema"), "{e}");
        assert_eq!(sim.world.get_field(p, "Health.hp").unwrap(), &json!(50));
        // spawn 未知组件报错并列出已知组件
        let e = call_err(&mut d, &mut sim, "world/spawn", json!({"components": {"Ghost": {}}}));
        assert!(e.contains("Health"), "{e}");
    }

    #[test]
    fn time_control_and_step() {
        let (mut d, mut sim) = setup();
        // 不暂停就 step → 拒绝
        let e = call_err(&mut d, &mut sim, "sim/step", json!({"ticks": 10}));
        assert!(e.contains("sim/pause"), "{e}");
        call(&mut d, &mut sim, "sim/pause", json!({}));
        call(&mut d, &mut sim, "sim/step", json!({"ticks": 60}));
        let p = sim.world.entity("player").unwrap();
        let x = sim.world.get_field(p, "Position.x").unwrap().as_f64().unwrap();
        assert!((x - 60.0).abs() < 1e-9, "60 tick 后 x 应为 60，实际 {x}");
        // 倍速参数校验
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
        // 60 tick 速度 60/s：30 tick 后 x=30 越界
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
        // 1 个实体：健康
        assert!(d.check_assertions(&sim).is_empty());
        // 撑爆预算
        sim.world.spawn();
        sim.world.spawn();
        let fresh = d.check_assertions(&sim);
        assert_eq!(fresh.len(), 1, "{fresh:?}");
        assert_eq!(fresh[0]["kind"], json!("budget"));
        assert_eq!(fresh[0]["limit"], json!(2));
        assert_eq!(fresh[0]["actual"], json!(3));
        // 持续超标不重复报（去抖）
        assert!(d.check_assertions(&sim).is_empty());
        // perf/stats 可观测
        let stats = call(&mut d, &mut sim, "perf/stats", json!({}));
        assert_eq!(stats["entities"], json!(3));
        assert_eq!(stats["budgets"]["max_entities"], json!(2));
    }

    #[test]
    fn inspect_selection_two_way() {
        let (mut d, mut sim) = setup();
        // 初始无选中
        let r = call(&mut d, &mut sim, "inspect/selection", json!({}));
        assert_eq!(r["selected"], json!(null));
        // AI 设选中（等价于窗口里人点了一下）
        let r = call(&mut d, &mut sim, "inspect/select", json!({"entity": "@player"}));
        assert_eq!(r["selected"]["name"], json!("player"));
        assert!(d.selection().is_some());
        // 实体销毁后选中态自动失效
        let p = sim.world.entity("player").unwrap();
        sim.world.despawn(p).unwrap();
        let r = call(&mut d, &mut sim, "inspect/selection", json!({}));
        assert_eq!(r["selected"], json!(null));
        // 清空选中
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

        // 改世界 + 选中实体，读档后状态回滚、选中清空
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
        // 读档 = 时间线断裂，录像期间拒绝（同 sim/restore 守卫）
        let e = call_err(&mut d, &mut sim, "save/load", json!({"slot": "s1"}));
        assert!(e.contains("录像"), "{e}");
        assert!(sim.is_recording(), "拒绝后录像必须仍然有效");
        // 写存档是纯输出副作用，录像期间照常放行
        call(&mut d, &mut sim, "save/write", json!({"slot": "s2"}));
        assert!(root.join("saves/s2.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_rpcs_explicit_errors() {
        let (mut d, mut sim) = setup();
        // 没挂存档仓库：显式报错而不是悄悄写到不知道哪里
        let e = call_err(&mut d, &mut sim, "save/write", json!({"slot": "s1"}));
        assert!(e.contains("存档目录"), "{e}");
        let root = temp_save_root("errors");
        d.set_save_store(SaveStore::new(&root, "rpc-test"));
        // 槽名路径穿越被拒
        let e = call_err(&mut d, &mut sim, "save/write", json!({"slot": "../evil"}));
        assert!(e.contains("不合法"), "{e}");
        // 读不存在的槽：报错带现有存档列表
        call(&mut d, &mut sim, "save/write", json!({"slot": "s1"}));
        let e = call_err(&mut d, &mut sim, "save/load", json!({"slot": "ghost"}));
        assert!(e.contains("ghost") && e.contains("s1"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn input_click_resolves_pick_and_injects_mouse_event() {
        let (mut d, mut sim) = setup();
        // 静止的 player 挂 Sprite（2x2，中心 (0,0)），拾取有目标
        let p = sim.world.entity("player").unwrap();
        sim.world.set_component(p, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        sim.world.set_component(p, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        call(&mut d, &mut sim, "sim/pause", json!({}));

        // 点在 player 身上（世界坐标）：拾取结果直接在返回值里
        let r = call(&mut d, &mut sim, "input/click", json!({"x": 0.5, "y": 0.5}));
        assert_eq!(r["event"], json!("mouse"));
        assert_eq!(r["entity"], json!("player"));
        // 点空地：entity = null
        let r = call(&mut d, &mut sim, "input/click", json!({"x": 100.0, "y": 100.0}));
        assert_eq!(r["entity"], json!(null));
        // 右键 → mouse-alt，同一套 payload
        let r = call(&mut d, &mut sim, "input/click", json!({"x": 0.0, "y": 0.0, "button": "right"}));
        assert_eq!(r["event"], json!("mouse-alt"));
        assert_eq!(r["entity"], json!("player"));

        // 下一 tick 三个事件都到，payload 是世界坐标 + 拾取结果
        call(&mut d, &mut sim, "sim/step", json!({}));
        let events = call(&mut d, &mut sim, "events/recent", json!({}));
        let arr = events.as_array().unwrap();
        let hit = arr.iter().find(|e| e["name"] == json!("mouse") && e["data"]["entity"] == json!("player")).expect("命中事件");
        assert_eq!(hit["data"]["x"], json!(0.5));
        assert_eq!(hit["data"]["y"], json!(0.5));
        assert!(arr.iter().any(|e| e["name"] == json!("mouse") && e["data"]["entity"] == json!(null)));
        assert!(arr.iter().any(|e| e["name"] == json!("mouse-alt") && e["data"]["entity"] == json!("player")));

        // 参数校验：缺坐标 / 未知按键都显式报错
        let e = call_err(&mut d, &mut sim, "input/click", json!({"y": 1.0}));
        assert!(e.contains('x'), "{e}");
        let e = call_err(&mut d, &mut sim, "input/click", json!({"x": 0.0, "y": 0.0, "button": "middle"}));
        assert!(e.contains("left") && e.contains("right"), "{e}");
    }

    #[test]
    fn clicks_ride_reply_channel_recorded_and_replayed() {
        /// 把 mouse 事件写进世界的逻辑——点击真实影响状态哈希，
        /// 重放时丢了点击必然分歧（要锁死的不变量）。
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
        // 点击走回复通道：连同 tick + 拾取结果一起进录像
        assert_eq!(rec.replies.len(), 2);
        assert_eq!((rec.replies[0].tick, rec.replies[0].name.as_str()), (30, "mouse"));
        assert_eq!(rec.replies[0].data["entity"], json!("player"));
        assert_eq!((rec.replies[1].tick, rec.replies[1].name.as_str()), (60, "mouse-alt"));
        assert_eq!(rec.replies[1].data["entity"], json!(null));
        // 重放：点击从录像注入，逐校验点 + 终态哈希逐位一致
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
}
