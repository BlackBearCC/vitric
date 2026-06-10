use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};
use vitric_rules::{Engine, Event, RuleSet};
use vitric_sim::{GameLogic, Sim};

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
            ctl: LoopCtl::default(),
        }
    }

    /// 主循环每 step 后调用：记录事件进环形缓冲。
    pub fn record_events(&mut self, tick: u64, events: &[Event]) {
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
        fresh
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
                if let Some(cschema) = self.checker.schema.component(comp) {
                    let mut report = vitric_data::ValidationReport::default();
                    let normalized = cschema.normalize(&after, &format!("world/set/{comp}"), &mut report);
                    if !report.ok() {
                        return Err(format!("修改未通过 schema 校验:\n{report}"));
                    }
                    after = normalized;
                }
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

            // ---- 热重载（AI 改完规则/脚本，毫秒级生效，世界状态不动）----
            "project/reload" => logic.reload(),

            // ---- 快照 / 哈希 ----
            "sim/snapshot" => Ok(sim.snapshot()),
            "sim/restore" => {
                let snap = params.get("snapshot").ok_or("缺少 snapshot 参数")?;
                sim.restore(snap)?;
                Ok(json!({"tick": sim.tick}))
            }
            "sim/hash" => Ok(json!(format!("{:#018x}", sim.world.state_hash()))),

            // ---- 观察（语义描述是主通道，截图是兜底验证）----
            "render/describe" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(320) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(240) as u32;
                vitric_render::describe_world(&sim.world, width, height)
            }
            "render/screenshot" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(320) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(240) as u32;
                let png = vitric_render::screenshot_png(&sim.world, width, height)?;
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
                 world/spawn, world/despawn, input/inject, sim/pause, sim/resume, sim/step, \
                 sim/speed, sim/quit, sim/snapshot, sim/restore, sim/hash, project/reload, \
                 events/recent, render/describe, render/screenshot, \
                 assert/add, assert/remove, assert/list, assert/failures"
            )),
        }
    }
}

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
