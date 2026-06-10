//! vitric-script — 嵌入式 JS 脚本层（QuickJS）。
//!
//! 规则写不动的 20% 复杂逻辑落到这里，但**戴着安全带**：
//! - 系统注册时声明 `query`（读哪些组件）和 `writes`（写哪些组件），
//!   越权写入直接报错，引擎因此永远知道每段逻辑碰了什么；
//! - `Math.random` / `Date.now` 被禁用并指路 `ctx.random()` / `ctx.tick`，
//!   随机数与 Rust 侧共用同一条 PCG32 流（JS 侧 BigInt 实现同一算法），
//!   脚本不破坏确定性回放；
//! - 数据进出全是 JSON：脚本看到的实体和场景文件、控制面是同一种语言。

use std::fmt;

use rquickjs::{CatchResultExt, Context, Function, Runtime};
use serde_json::{json, Map, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};
use vitric_rules::Event;
use vitric_sim::Pcg32;

const PRELUDE: &str = include_str!("prelude.js");

/// 一个已注册系统的声明。
#[derive(Debug, Clone)]
pub struct SystemDecl {
    pub name: String,
    /// 实体筛选 + 可读组件集合。
    pub query: Vec<String>,
    /// 可写组件集合（⊆ query）。
    pub writes: Vec<String>,
}

/// 脚本执行产出。
#[derive(Debug, Default)]
pub struct ScriptOutput {
    /// 脚本 emit 的事件，由运行时层送回规则引擎。
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptError {
    /// 脚本加载/求值失败（语法错误、注册参数不对……）。
    Load { file: String, message: String },
    /// 系统/函数运行中抛异常。
    Runtime { location: String, message: String },
    /// 改了未声明 writes 的组件。
    UndeclaredWrite { system: String, entity: String, component: String },
    /// 产出的操作（spawn/despawn/写回）不合法。
    Op { location: String, message: String },
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Load { file, message } => {
                write!(f, "脚本 {file} 加载失败: {message}")
            }
            ScriptError::Runtime { location, message } => {
                write!(f, "脚本 {location} 运行报错: {message}")
            }
            ScriptError::UndeclaredWrite { system, entity, component } => write!(
                f,
                "系统 {system:?} 修改了实体 {entity} 的组件 {component:?}，\
                 但 writes 里没有声明它。提示：把 {component:?} 加进该系统的 writes，\
                 或者别改它——读写声明是引擎理解逻辑影响面的依据，不是摆设"
            ),
            ScriptError::Op { location, message } => {
                write!(f, "脚本 {location} 的操作不合法: {message}")
            }
        }
    }
}

impl std::error::Error for ScriptError {}

/// 脚本引擎。持有一个 QuickJS 上下文；热重载 = 换源码重建上下文。
pub struct ScriptEngine {
    _runtime: Runtime,
    context: Context,
    schema: Schema,
    sources: Vec<(String, String)>,
    pub systems: Vec<SystemDecl>,
    pub fns: Vec<String>,
}

impl ScriptEngine {
    pub fn new(schema: Schema) -> Result<ScriptEngine, ScriptError> {
        let runtime = Runtime::new().map_err(|e| ScriptError::Load {
            file: "<runtime>".into(),
            message: e.to_string(),
        })?;
        let context = Context::full(&runtime).map_err(|e| ScriptError::Load {
            file: "<context>".into(),
            message: e.to_string(),
        })?;
        let mut engine = ScriptEngine {
            _runtime: runtime,
            context,
            schema,
            sources: Vec::new(),
            systems: Vec::new(),
            fns: Vec::new(),
        };
        engine.eval_file("<prelude>", PRELUDE)?;
        Ok(engine)
    }

    /// 加载一个脚本文件（按调用顺序求值，系统按注册顺序执行）。
    pub fn load(&mut self, file: &str, source: &str) -> Result<(), ScriptError> {
        self.eval_file(file, source)?;
        self.sources.push((file.to_string(), source.to_string()));
        self.refresh_decls()
    }

    /// 热重载：用新源码整体重建（注册表清零重来，世界状态不动）。
    pub fn reload(&mut self, sources: Vec<(String, String)>) -> Result<(), ScriptError> {
        let mut fresh = ScriptEngine::new(self.schema.clone())?;
        for (file, src) in &sources {
            fresh.load(file, src)?;
        }
        *self = fresh;
        Ok(())
    }

    /// 跑全部系统（注册顺序）。
    pub fn run_systems(
        &mut self,
        world: &mut World,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<ScriptOutput, ScriptError> {
        let mut out = ScriptOutput::default();
        for idx in 0..self.systems.len() {
            self.run_one_system(idx, world, rng, tick, &mut out)?;
        }
        Ok(out)
    }

    fn run_one_system(
        &mut self,
        idx: usize,
        world: &mut World,
        rng: &mut Pcg32,
        tick: u64,
        out: &mut ScriptOutput,
    ) -> Result<(), ScriptError> {
        let decl = self.systems[idx].clone();
        let location = format!("系统 {:?}", decl.name);
        let query: Vec<&str> = decl.query.iter().map(|s| s.as_str()).collect();

        // 进：实体快照（只带 query 声明的组件）
        let ids = world.query(&query);
        let mut entities = Vec::with_capacity(ids.len());
        for &id in &ids {
            let mut obj = Map::new();
            obj.insert("id".into(), json!(id.to_string()));
            for comp in &decl.query {
                obj.insert(
                    comp.clone(),
                    world.get_component(id, comp).expect("query 已筛选").clone(),
                );
            }
            entities.push(Value::Object(obj));
        }
        let payload = json!({
            "entities": entities,
            "dt": vitric_sim::DT,
            "tick": tick,
            "rng": rng_to_json(rng),
        });

        let result_str = self.call_js("__runSystem", (idx as i32, payload.to_string()), &location)?;
        let result: Value = serde_json::from_str(&result_str).map_err(|e| ScriptError::Op {
            location: location.clone(),
            message: format!("返回值不是合法 JSON: {e}"),
        })?;

        // 随机数状态写回（脚本抽过几次就推进几步）
        *rng = rng_from_json(result.get("rng"), &location)?;

        // 出：写回实体（强制 writes 声明）
        let returned = result
            .get("entities")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ScriptError::Op {
                location: location.clone(),
                message: "返回缺少 entities 数组（不要整体替换 entities 参数）".into(),
            })?;
        if returned.len() != ids.len() {
            return Err(ScriptError::Op {
                location,
                message: format!(
                    "entities 数组长度从 {} 变成了 {}。不要增删数组元素；\
                     生成/销毁实体用 ctx.spawn / ctx.despawn",
                    ids.len(),
                    returned.len()
                ),
            });
        }
        // 两遍走：先全量校验攒齐变更，确认整批合法再落地。
        // 第 N 个实体非法时世界保持原样，不留半改状态（commit-on-success）。
        let mut pending: Vec<(vitric_ecs::EntityId, String, Value)> = Vec::new();
        for (i, (&id, ret)) in ids.iter().zip(returned).enumerate() {
            let ret_obj = ret.as_object().ok_or_else(|| ScriptError::Op {
                location: location.clone(),
                message: format!("entities[{i}] 被改成了非对象"),
            })?;
            // 不许夹带 query 之外的组件
            for key in ret_obj.keys() {
                if key != "id" && !decl.query.contains(key) {
                    return Err(ScriptError::UndeclaredWrite {
                        system: decl.name.clone(),
                        entity: id.to_string(),
                        component: key.clone(),
                    });
                }
            }
            for comp in &decl.query {
                let before = world.get_component(id, comp).expect("query 已筛选").clone();
                let after = ret_obj.get(comp).cloned().unwrap_or(Value::Null);
                // JSON 经 JS 往返会丢小数点形态（0.0 → 0），必须按数值语义比，
                // 否则只读系统会被误判成越权写
                if json_semantic_eq(&before, &after) {
                    continue;
                }
                if !decl.writes.contains(comp) {
                    return Err(ScriptError::UndeclaredWrite {
                        system: decl.name.clone(),
                        entity: id.to_string(),
                        component: comp.clone(),
                    });
                }
                // 写回也过 schema：脚本和场景文件遵守同一套法律
                let normalized = self.normalize(comp, &after, &location)?;
                pending.push((id, comp.clone(), normalized));
            }
        }
        for (id, comp, normalized) in pending {
            world
                .set_component(id, &comp, normalized)
                .map_err(|e| ScriptError::Op {
                    location: location.clone(),
                    message: e.to_string(),
                })?;
        }

        // 出：操作流
        self.apply_ops(result.get("ops"), world, out, &location)
    }

    /// 执行规则 `call` 动作指向的脚本函数。
    pub fn call_fn(
        &mut self,
        function: &str,
        args: &Value,
        self_entity: Option<EntityId>,
        world: &mut World,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<ScriptOutput, ScriptError> {
        let location = format!("函数 {function:?}");
        let payload = json!({
            "args": args,
            "self": self_entity.map(|e| e.to_string()),
            "dt": vitric_sim::DT,
            "tick": tick,
            "rng": rng_to_json(rng),
        });
        let result_str =
            self.call_js("__callFn", (function.to_string(), payload.to_string()), &location)?;
        let result: Value = serde_json::from_str(&result_str).map_err(|e| ScriptError::Op {
            location: location.clone(),
            message: format!("返回值不是合法 JSON: {e}"),
        })?;
        *rng = rng_from_json(result.get("rng"), &location)?;
        let mut out = ScriptOutput::default();
        self.apply_ops(result.get("ops"), world, &mut out, &location)?;
        Ok(out)
    }

    // ---- 内部 ----

    fn apply_ops(
        &self,
        ops: Option<&Value>,
        world: &mut World,
        out: &mut ScriptOutput,
        location: &str,
    ) -> Result<(), ScriptError> {
        let err = |message: String| ScriptError::Op {
            location: location.to_string(),
            message,
        };
        let Some(ops) = ops.and_then(|v| v.as_array()) else {
            return Ok(());
        };
        for op in ops {
            match op.get("op").and_then(|v| v.as_str()) {
                Some("emit") => {
                    let name = op.get("name").and_then(|v| v.as_str()).expect("prelude 已校验");
                    let data = op.get("data").cloned().unwrap_or_else(|| json!({}));
                    out.events.push(Event::new(name, data));
                }
                Some("spawn") => {
                    let comps = op
                        .get("components")
                        .and_then(|v| v.as_object())
                        .ok_or_else(|| err("spawn 的 components 必须是对象".into()))?;
                    let id = match op.get("name").and_then(|v| v.as_str()) {
                        Some(name) => world.spawn_named(name).map_err(|e| err(e.to_string()))?,
                        None => world.spawn(),
                    };
                    for (cname, cval) in comps {
                        let normalized = self.normalize(cname, cval, location)?;
                        world.set_component(id, cname, normalized).map_err(|e| err(e.to_string()))?;
                    }
                }
                Some("despawn") => {
                    let handle = op.get("id").and_then(|v| v.as_str()).expect("prelude 已校验");
                    let id: EntityId = handle
                        .parse()
                        .map_err(|e: String| err(format!("despawn: {e}")))?;
                    world.despawn(id).map_err(|e| err(e.to_string()))?;
                }
                other => return Err(err(format!("未知操作 {other:?}"))),
            }
        }
        Ok(())
    }

    fn normalize(&self, comp: &str, value: &Value, location: &str) -> Result<Value, ScriptError> {
        let cschema = self.schema.component(comp).ok_or_else(|| ScriptError::Op {
            location: location.to_string(),
            message: format!(
                "组件 {comp:?} 不在 schema 里。schema 定义的组件: [{}]",
                self.schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        })?;
        let mut report = vitric_data::ValidationReport::default();
        let normalized = cschema.normalize(value, &format!("{location}/{comp}"), &mut report);
        if !report.ok() {
            return Err(ScriptError::Op {
                location: location.to_string(),
                message: format!("组件值未通过 schema 校验:\n{report}"),
            });
        }
        Ok(normalized)
    }

    fn eval_file(&mut self, file: &str, source: &str) -> Result<(), ScriptError> {
        self.context.with(|ctx| {
            ctx.eval::<(), _>(source.as_bytes().to_vec())
                .catch(&ctx)
                .map_err(|e| ScriptError::Load {
                    file: file.to_string(),
                    message: e.to_string(),
                })
        })
    }

    fn call_js<A>(&mut self, entry: &str, args: A, location: &str) -> Result<String, ScriptError>
    where
        A: for<'js> rquickjs::function::IntoArgs<'js>,
    {
        self.context.with(|ctx| {
            let f: Function = ctx.globals().get(entry).map_err(|e| ScriptError::Runtime {
                location: location.to_string(),
                message: format!("找不到入口 {entry}: {e}"),
            })?;
            f.call::<_, String>(args).catch(&ctx).map_err(|e| ScriptError::Runtime {
                location: location.to_string(),
                message: e.to_string(),
            })
        })
    }

    fn refresh_decls(&mut self) -> Result<(), ScriptError> {
        let listing = self.call_js("__list", (), "<注册表>")?;
        let v: Value = serde_json::from_str(&listing).expect("prelude __list 输出合法 JSON");
        self.systems = v["systems"]
            .as_array()
            .expect("systems 是数组")
            .iter()
            .map(|s| SystemDecl {
                name: s["name"].as_str().expect("name").to_string(),
                query: to_string_vec(&s["query"]),
                writes: to_string_vec(&s["writes"]),
            })
            .collect();
        self.fns = to_string_vec(&v["fns"]);
        Ok(())
    }
}

/// 数值按语义比（5.0 == 5），其余结构递归。JS 的 JSON.stringify 不保留
/// 整值浮点的小数点，往返后表示必然漂移，按表示比会满屏假阳性。
fn json_semantic_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(ix), Some(iy)) = (x.as_i64(), y.as_i64()) {
                return ix == iy;
            }
            if let (Some(ux), Some(uy)) = (x.as_u64(), y.as_u64()) {
                return ux == uy;
            }
            match (x.as_f64(), y.as_f64()) {
                (Some(fx), Some(fy)) => fx == fy,
                _ => x == y,
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(va, vb)| json_semantic_eq(va, vb))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, va)| y.get(k).is_some_and(|vb| json_semantic_eq(va, vb)))
        }
        _ => a == b,
    }
}

fn to_string_vec(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Pcg32 → {"state": "u64串", "inc": "u64串"}（JSON number 装不下 u64，走字符串）。
fn rng_to_json(rng: &Pcg32) -> Value {
    let v = serde_json::to_value(rng).expect("Pcg32 可序列化");
    json!({
        "state": v["state"].as_u64().expect("state 是 u64").to_string(),
        "inc": v["inc"].as_u64().expect("inc 是 u64").to_string(),
    })
}

fn rng_from_json(v: Option<&Value>, location: &str) -> Result<Pcg32, ScriptError> {
    let err = |message: String| ScriptError::Op {
        location: location.to_string(),
        message,
    };
    let v = v.ok_or_else(|| err("返回缺少 rng 状态".into()))?;
    let parse = |key: &str| -> Result<u64, ScriptError> {
        v.get(key)
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| err(format!("rng.{key} 不是合法 u64 字符串")))
    };
    let state = parse("state")?;
    let inc = parse("inc")?;
    serde_json::from_value(json!({"state": state, "inc": inc}))
        .map_err(|e| err(format!("rng 状态重建失败: {e}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use vitric_data::Schema;
    use vitric_ecs::World;
    use vitric_sim::Pcg32;

    use super::*;

    fn schema() -> Schema {
        Schema::parse(
            &json!({"components": {
                "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Velocity": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Coin": {"fields": {"value": {"type": "int", "default": 1}}}
            }}),
            "schema.json",
        )
        .unwrap()
    }

    fn engine_with(src: &str) -> ScriptEngine {
        let mut eng = ScriptEngine::new(schema()).unwrap();
        eng.load("test.js", src).unwrap();
        eng
    }

    #[test]
    fn system_reads_and_writes_declared_components() {
        let mut eng = engine_with(
            r#"
            vitric.system("friction", {query: ["Velocity"], writes: ["Velocity"]}, (entities, ctx) => {
                for (const e of entities) {
                    e.Velocity.x *= 0.5;
                }
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Velocity", json!({"x": 10.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        eng.run_systems(&mut w, &mut rng, 0).unwrap();
        // JS 的 5.0 序列化成 5（JSON 不区分），按数值比较
        assert_eq!(w.get_field(e, "Velocity.x").unwrap().as_f64(), Some(5.0));
    }

    #[test]
    fn undeclared_write_is_rejected() {
        let mut eng = engine_with(
            r#"
            vitric.system("sneaky", {query: ["Position", "Velocity"], writes: ["Velocity"]}, (entities) => {
                for (const e of entities) { e.Position.x = 999; }
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        let err = eng.run_systems(&mut w, &mut rng, 0).unwrap_err();
        match &err {
            ScriptError::UndeclaredWrite { system, component, .. } => {
                assert_eq!(system, "sneaky");
                assert_eq!(component, "Position");
            }
            other => panic!("错误类型不对: {other}"),
        }
        // 世界没被污染
        assert_eq!(w.get_field(e, "Position.x").unwrap(), &json!(0.0));
    }

    #[test]
    fn readonly_system_with_whole_valued_floats_is_not_a_write() {
        // 回归：世界里存 0.0/5.0，JS 往返变成 0/5，表示不同但语义相同。
        // 一个什么都不改的系统绝不能被误判成越权写（曾导致正常项目随机停机）。
        let mut eng = engine_with(
            r#"
            vitric.system("observer", {query: ["Position", "Velocity"], writes: ["Velocity"]}, () => {});
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 5.0, "y": 0.0})).unwrap();
        w.set_component(e, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        eng.run_systems(&mut w, &mut rng, 0).unwrap();
        assert_eq!(w.get_field(e, "Position.x").unwrap().as_f64(), Some(5.0));
    }

    #[test]
    fn failed_validation_leaves_world_untouched_even_with_earlier_valid_writes() {
        // 写回必须整批成功才落地：第二个实体越权时，第一个实体的合法写也不能漏进世界
        let mut eng = engine_with(
            r#"
            vitric.system("mixed", {query: ["Position", "Velocity"], writes: ["Velocity"]}, (entities) => {
                entities[0].Velocity.x = 77;
                if (entities.length > 1) { entities[1].Position.x = 999; }
            });
            "#,
        );
        let mut w = World::new();
        let a = w.spawn();
        w.set_component(a, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(a, "Velocity", json!({"x": 1.0, "y": 0.0})).unwrap();
        let b = w.spawn();
        w.set_component(b, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(b, "Velocity", json!({"x": 2.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        let err = eng.run_systems(&mut w, &mut rng, 0).unwrap_err();
        assert!(matches!(err, ScriptError::UndeclaredWrite { .. }), "{err}");
        assert_eq!(w.get_field(a, "Velocity.x").unwrap().as_f64(), Some(1.0), "半改状态泄漏");
    }

    #[test]
    fn writes_must_be_subset_of_query() {
        let mut eng = ScriptEngine::new(schema()).unwrap();
        let err = eng
            .load(
                "bad.js",
                r#"vitric.system("s", {query: ["Position"], writes: ["Velocity"]}, () => {});"#,
            )
            .unwrap_err();
        assert!(err.to_string().contains("不在 query 里"), "{err}");
    }

    #[test]
    fn math_random_and_date_now_are_poisoned() {
        let mut eng = engine_with(
            r#"
            vitric.system("evil", {query: ["Position"], writes: []}, () => {
                Math.random();
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        let err = eng.run_systems(&mut w, &mut rng, 0).unwrap_err();
        assert!(err.to_string().contains("ctx.random"), "要指路正确用法: {err}");
    }

    #[test]
    fn new_date_is_poisoned_but_explicit_date_is_allowed() {
        // 回归：曾只毒化 Date.now，new Date().getTime() 照样泄漏墙钟进世界状态
        let mut eng = engine_with(
            r#"
            vitric.system("clock", {query: ["Position"], writes: ["Position"]}, (entities) => {
                for (const e of entities) { e.Position.x = new Date().getTime(); }
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        let err = eng.run_systems(&mut w, &mut rng, 0).unwrap_err();
        assert!(err.to_string().contains("ctx.tick"), "要指路正确用法: {err}");

        // 显式传参是纯计算，必须放行
        let mut eng2 = ScriptEngine::new(schema()).unwrap();
        eng2.load(
            "ok.js",
            r#"
            const epoch = new Date(0);
            if (epoch.getTime() !== 0) throw new Error("显式 Date 坏了");
            vitric.system("noop", {query: ["Position"], writes: []}, () => {});
            "#,
        )
        .unwrap();
    }

    #[test]
    fn ctx_random_continues_the_rust_stream() {
        // JS 抽两个 f64 后，Rust 侧继续抽，必须和纯 Rust 连续抽四个完全一致
        let mut pure = Pcg32::new(42);
        let expected: Vec<f64> = (0..4).map(|_| pure.next_f64()).collect();

        let mut eng = engine_with(
            r#"
            vitric.system("rand", {query: ["Position"], writes: ["Position"]}, (entities, ctx) => {
                entities[0].Position.x = ctx.random();
                entities[0].Position.y = ctx.random();
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(42);
        eng.run_systems(&mut w, &mut rng, 0).unwrap();
        let x = w.get_field(e, "Position.x").unwrap().as_f64().unwrap();
        let y = w.get_field(e, "Position.y").unwrap().as_f64().unwrap();
        assert_eq!(x, expected[0], "JS 第 1 抽必须等于 Rust 第 1 抽");
        assert_eq!(y, expected[1], "JS 第 2 抽必须等于 Rust 第 2 抽");
        assert_eq!(rng.next_f64(), expected[2], "Rust 续抽必须接上 JS 抽过的流");
        assert_eq!(rng.next_f64(), expected[3]);
    }

    #[test]
    fn spawn_despawn_emit_ops() {
        let mut eng = engine_with(
            r#"
            vitric.system("burst", {query: ["Coin"], writes: []}, (entities, ctx) => {
                for (const e of entities) {
                    ctx.spawn({Coin: {value: e.Coin.value + 1}});
                    ctx.despawn(e.id);
                    ctx.emit("burst", {from: e.id});
                }
            });
            "#,
        );
        let mut w = World::new();
        let c = w.spawn();
        w.set_component(c, "Coin", json!({"value": 1})).unwrap();
        let mut rng = Pcg32::new(1);
        let out = eng.run_systems(&mut w, &mut rng, 0).unwrap();
        assert!(!w.is_alive(c));
        let coins = w.query(&["Coin"]);
        assert_eq!(coins.len(), 1);
        assert_eq!(w.get_field(coins[0], "Coin.value").unwrap(), &json!(2));
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].name, "burst");
    }

    #[test]
    fn spawned_components_are_schema_checked() {
        let mut eng = engine_with(
            r#"
            vitric.system("bad-spawn", {query: ["Coin"], writes: []}, (entities, ctx) => {
                if (entities.length) ctx.spawn({Coin: {value: "not-a-number"}});
            });
            "#,
        );
        let mut w = World::new();
        let c = w.spawn();
        w.set_component(c, "Coin", json!({"value": 1})).unwrap();
        let mut rng = Pcg32::new(1);
        let err = eng.run_systems(&mut w, &mut rng, 0).unwrap_err();
        assert!(err.to_string().contains("schema"), "{err}");
    }

    #[test]
    fn call_fn_from_rules() {
        let mut eng = engine_with(
            r#"
            vitric.fn("explode", (args, ctx) => {
                for (let i = 0; i < args.count; i++) {
                    ctx.spawn({Coin: {value: 1}});
                }
                ctx.emit("exploded", {at: args.where});
            });
            "#,
        );
        assert_eq!(eng.fns, vec!["explode"]);
        let mut w = World::new();
        let mut rng = Pcg32::new(1);
        let out = eng
            .call_fn("explode", &json!({"count": 3, "where": "here"}), None, &mut w, &mut rng, 5)
            .unwrap();
        assert_eq!(w.query(&["Coin"]).len(), 3);
        assert_eq!(out.events[0].data["at"], json!("here"));
        // 未注册的函数报错并列出已注册的
        let err = eng.call_fn("nope", &json!({}), None, &mut w, &mut rng, 5).unwrap_err();
        assert!(err.to_string().contains("explode"), "{err}");
    }

    #[test]
    fn hot_reload_replaces_behavior() {
        let mut eng = engine_with(
            r#"vitric.system("a", {query: ["Coin"], writes: ["Coin"]}, (es) => {
                for (const e of es) e.Coin.value = 10;
            });"#,
        );
        let mut w = World::new();
        let c = w.spawn();
        w.set_component(c, "Coin", json!({"value": 1})).unwrap();
        let mut rng = Pcg32::new(1);
        eng.run_systems(&mut w, &mut rng, 0).unwrap();
        assert_eq!(w.get_field(c, "Coin.value").unwrap(), &json!(10));

        eng.reload(vec![(
            "test.js".into(),
            r#"vitric.system("a", {query: ["Coin"], writes: ["Coin"]}, (es) => {
                for (const e of es) e.Coin.value = 77;
            });"#
                .into(),
        )])
        .unwrap();
        eng.run_systems(&mut w, &mut rng, 1).unwrap();
        assert_eq!(w.get_field(c, "Coin.value").unwrap(), &json!(77));
        // 坏脚本重载失败 → 报错，旧行为不能半死不活
        let err = eng.reload(vec![("bad.js".into(), "syntax error (".into())]).unwrap_err();
        assert!(matches!(err, ScriptError::Load { .. }), "{err}");
    }

    #[test]
    fn syntax_error_reports_file() {
        let mut eng = ScriptEngine::new(schema()).unwrap();
        let err = eng.load("oops.js", "function {").unwrap_err();
        match &err {
            ScriptError::Load { file, .. } => assert_eq!(file, "oops.js"),
            other => panic!("错误类型不对: {other}"),
        }
    }
}
