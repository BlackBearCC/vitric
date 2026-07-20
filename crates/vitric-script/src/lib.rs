//! vitric-script — an embedded JS scripting layer (QuickJS).
//!
//! The 20% of complex logic that rules can't express lands here, but **with a seatbelt on**:
//! - Systems declare `query` (which components they read) and `writes` (which they write) at registration;
//!   out-of-scope writes error immediately, so the engine always knows what each piece of logic touches;
//! - `Math.random` / `Date.now` are disabled with a pointer to `ctx.random()` / `ctx.tick`;
//!   the random source shares the same PCG32 stream as the Rust side (JS reimplements the same algorithm with BigInt),
//!   so scripts don't break deterministic replay;
//! - All data crossing the boundary is JSON: the entities scripts see speak the same language as scene files and the control plane.

use std::fmt;

use rquickjs::{CatchResultExt, Context, Function, Runtime};
use serde_json::{json, Map, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};
use vitric_rules::Event;
use vitric_sim::Pcg32;

const PRELUDE: &str = include_str!("prelude.js");

thread_local! {
    /// Bare pointer to the World readable during the current call_js. Set before run_one_system/call_fn calls into JS, cleared after;
    /// null outside that window. QuickJS is single-threaded and synchronous; __getFieldRaw only reads it within the window.
    static WORLD_PTR: std::cell::Cell<*const World> = std::cell::Cell::new(std::ptr::null());
}

/// Resolution for ctx.getField: handle (may carry an @ prefix) or entity name → that field's value; returns None if entity/field is missing.
fn resolve_field(world: &World, reference: &str, path: &str) -> Option<Value> {
    let stripped = reference.strip_prefix('@').unwrap_or(reference);
    let id = match stripped.parse::<EntityId>() {
        Ok(id) => id,
        Err(_) => world.entity(stripped).ok()?,
    };
    world.get_field(id, path).ok().cloned()
}

/// Declaration of a registered system.
#[derive(Debug, Clone)]
pub struct SystemDecl {
    pub name: String,
    /// Entity filter + the set of readable components.
    pub query: Vec<String>,
    /// Set of writable components (⊆ query).
    pub writes: Vec<String>,
    /// Whether the system declared an optional `catch_up(entity, ctx, dormant_ticks)` fn.
    /// Set at registration time (4th arg to `vitric.system`); the engine uses this flag to
    /// skip systems without catch_up when iterating for region thaw catch-up.
    pub has_catch_up: bool,
}

/// Script execution output.
#[derive(Debug, Default)]
pub struct ScriptOutput {
    /// Events emitted by the script; the runtime layer feeds them back to the rule engine.
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptError {
    /// Script loading/evaluation failed (syntax error, bad registration args, ...).
    Load { file: String, message: String },
    /// Exception thrown while a system/function was running.
    Runtime { location: String, message: String },
    /// Wrote a component not declared in writes.
    UndeclaredWrite { system: String, entity: String, component: String },
    /// A produced operation (spawn/despawn/write-back) is invalid.
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

/// Script engine. Holds one QuickJS context; hot reload = rebuild the context with new source.
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
        engine.register_natives()?;
        engine.eval_file("<prelude>", PRELUDE)?;
        Ok(engine)
    }

    /// Register native functions for the script. Currently only __getFieldRaw(ref, path) → JSON string, called by prelude's
    /// ctx.getField: looks up a single field on the live World directly; missing returns the literal "undefined".
    fn register_natives(&self) -> Result<(), ScriptError> {
        self.context.with(|ctx| {
            let make_err = |e: rquickjs::Error| ScriptError::Load {
                file: "<natives>".into(),
                message: e.to_string(),
            };
            let f = Function::new(ctx.clone(), |reference: String, path: String| -> String {
                WORLD_PTR.with(|p| {
                    let ptr = p.get();
                    if ptr.is_null() {
                        return "undefined".to_string();
                    }
                    // Safety: the pointer is only non-null within the synchronous window when run_one_system/call_fn calls JS,
                    // during which world's &mut is not used; QuickJS is single-threaded, so there is no concurrency or read/write aliasing.
                    let world: &World = unsafe { &*ptr };
                    match resolve_field(world, &reference, &path) {
                        Some(v) => {
                            serde_json::to_string(&v).unwrap_or_else(|_| "undefined".to_string())
                        }
                        None => "undefined".to_string(),
                    }
                })
            })
            .map_err(make_err)?;
            ctx.globals().set("__getFieldRaw", f).map_err(make_err)
        })
    }

    /// Load a script file (evaluated in call order; systems execute in registration order).
    pub fn load(&mut self, file: &str, source: &str) -> Result<(), ScriptError> {
        self.eval_file(file, source)?;
        self.sources.push((file.to_string(), source.to_string()));
        self.refresh_decls()
    }

    /// Hot reload: rebuild wholesale with new source (registry reset from scratch; world state untouched).
    pub fn reload(&mut self, sources: Vec<(String, String)>) -> Result<(), ScriptError> {
        let mut fresh = ScriptEngine::new(self.schema.clone())?;
        for (file, src) in &sources {
            fresh.load(file, src)?;
        }
        *self = fresh;
        Ok(())
    }

    /// Run all systems (in registration order).
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

    /// Run each system's optional `catch_up` fn for entities in the given region. Invoked by
    /// the runtime when a region thaws after being dormant — each system that declared a
    /// catch_up fn fast-forwards entity state by the dormant tick budget (e.g. crop-grow
    /// advances Crop.timer/stage so a crop frozen for 60s of sim time doesn't stay at timer=0).
    ///
    /// Entity iteration uses `world.entities()` (NOT `query`) because `query` filters dormant
    /// entities — the just-thawed entities are now "active" at the region-entity level, but
    /// individual entities carrying a Region component may still have state="dormant" on their
    /// own Region component (the engine's thaw_region only updates the region entity named by
    /// `region_id`, not every entity in that region). The system's `query` filter is applied
    /// on top: an entity must have all query components to be matched.
    ///
    /// Each matching entity triggers one `__runCatchUp` call (per-entity, not per-batch). The
    /// catch_up fn reads/writes via ctx.getField/ctx.setField — the deferred-op channel — so
    /// writes are subject to schema validation but NOT to the system's `writes` declaration
    /// (catch_up is best-effort reconciliation, the writes declaration governs the main fn only).
    pub fn run_catch_up_for_region(
        &mut self,
        region_id: &str,
        dormant_ticks: u32,
        world: &mut World,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<ScriptOutput, ScriptError> {
        let mut out = ScriptOutput::default();
        for idx in 0..self.systems.len() {
            let decl = self.systems[idx].clone();
            if !decl.has_catch_up {
                continue;
            }
            let location = format!("系统 {:?} 的 catch_up", decl.name);
            let query: Vec<&str> = decl.query.iter().map(|s| s.as_str()).collect();

            // Find entities in this region (Region.id == region_id) that have all query components.
            // Use entities() (not query()) because query filters dormant — we want the just-thawed
            // entities even if their own Region component still says "dormant".
            let mut matching: Vec<EntityId> = Vec::new();
            for id in world.entities() {
                let Ok(region) = world.get_component(id, "Region") else { continue; };
                let Some(id_str) = region.get("id").and_then(|v| v.as_str()) else { continue; };
                if id_str != region_id {
                    continue;
                }
                if !query.iter().all(|c| world.has_component(id, c)) {
                    continue;
                }
                matching.push(id);
            }

            for id in matching {
                let payload = json!({
                    "dt": vitric_sim::DT,
                    "tick": tick,
                    "rng": rng_to_json(rng),
                });
                // Open the getField read-live-World window for this JS call, close it immediately after.
                WORLD_PTR.with(|p| p.set(world as *const World));
                let call = self.call_js(
                    "__runCatchUp",
                    (idx as i32, id.to_string(), dormant_ticks as i32, payload.to_string()),
                    &location,
                );
                WORLD_PTR.with(|p| p.set(std::ptr::null()));
                let result_str = call?;
                let mut result: Value = serde_json::from_str(&result_str).map_err(|e| {
                    ScriptError::Op {
                        location: location.clone(),
                        message: format!("返回值不是合法 JSON: {e}"),
                    }
                })?;
                revive_f64(&mut result, &location)?;
                *rng = rng_from_json(result.get("rng"), &location)?;
                self.apply_ops(result.get("ops"), world, &mut out, &location)?;
            }
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

        // In: entity snapshot (carrying only the components declared in query)
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

        // Open the getField read-live-World window for this JS call, close it immediately after.
        WORLD_PTR.with(|p| p.set(world as *const World));
        let call = self.call_js("__runSystem", (idx as i32, payload.to_string()), &location);
        WORLD_PTR.with(|p| p.set(std::ptr::null()));
        let result_str = call?;
        let mut result: Value = serde_json::from_str(&result_str).map_err(|e| ScriptError::Op {
            location: location.clone(),
            message: format!("返回值不是合法 JSON: {e}"),
        })?;
        revive_f64(&mut result, &location)?;

        // Write the RNG state back (it advances by however many draws the script took)
        *rng = rng_from_json(result.get("rng"), &location)?;

        // Out: write entities back (writes declaration enforced)
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
        // Two passes: first validate everything and collect changes, then commit only if the whole batch is legal.
        // When the Nth entity is illegal the world stays untouched, no half-applied state (commit-on-success).
        let mut pending: Vec<(vitric_ecs::EntityId, String, Value)> = Vec::new();
        for (i, (&id, ret)) in ids.iter().zip(returned).enumerate() {
            let ret_obj = ret.as_object().ok_or_else(|| ScriptError::Op {
                location: location.clone(),
                message: format!("entities[{i}] 被改成了非对象"),
            })?;
            // no components outside query allowed
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
                // JSON round-tripped through JS loses decimal-point shape (0.0 → 0); comparison must be by numeric semantics,
                // otherwise a read-only system would be misjudged as an out-of-scope write
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
                // write-back also goes through schema: scripts and scene files follow the same law
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

        // Out: operation stream
        self.apply_ops(result.get("ops"), world, out, &location)
    }

    /// Execute the script function targeted by a rule's `call` action.
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
        WORLD_PTR.with(|p| p.set(world as *const World));
        let call = self.call_js("__callFn", (function.to_string(), payload.to_string()), &location);
        WORLD_PTR.with(|p| p.set(std::ptr::null()));
        let result_str = call?;
        let mut result: Value = serde_json::from_str(&result_str).map_err(|e| ScriptError::Op {
            location: location.clone(),
            message: format!("返回值不是合法 JSON: {e}"),
        })?;
        revive_f64(&mut result, &location)?;
        *rng = rng_from_json(result.get("rng"), &location)?;
        let mut out = ScriptOutput::default();
        self.apply_ops(result.get("ops"), world, &mut out, &location)?;
        Ok(out)
    }

    // ---- internals ----

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
                Some("setField") => {
                    let r = op.get("ref").and_then(|v| v.as_str()).expect("prelude 已校验");
                    let path = op.get("path").and_then(|v| v.as_str()).expect("prelude 已校验");
                    let value = op.get("value").cloned().unwrap_or(Value::Null);
                    // ref is handle text (e.g. "e3v0") or entity name — handle first, fall back to name lookup on parse failure.
                    // Names may carry an @ prefix (in rules "@name" is the convention); strip it uniformly here.
                    let stripped = r.strip_prefix('@').unwrap_or(r);
                    let id: EntityId = match stripped.parse::<EntityId>() {
                        Ok(id) => id,
                        Err(_) => world.entity(stripped).map_err(|e| err(format!("setField: {e}")))?,
                    };
                    world
                        .set_field(id, path, value)
                        .map_err(|e| err(format!("setField: {e}")))?;
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
                // Prelude's __list reports `catch_up: true/false` per system; missing/false = no catch_up.
                has_catch_up: s.get("catch_up").and_then(|v| v.as_bool()).unwrap_or(false),
            })
            .collect();
        self.fns = to_string_vec(&v["fns"]);
        Ok(())
    }
}

/// Restore prelude's bit-string floats: `{"$f64":"<16hex>"}` → f64.
/// QuickJS's dtoa is not shortest-round-trip, so non-integer floats crossing the boundary must go through IEEE754 bit strings.
fn revive_f64(v: &mut Value, location: &str) -> Result<(), ScriptError> {
    match v {
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(Value::String(hex)) = map.get("$f64") {
                    let bits = u64::from_str_radix(hex, 16).map_err(|e| ScriptError::Op {
                        location: location.to_string(),
                        message: format!("$f64 位串 {hex:?} 不合法: {e}"),
                    })?;
                    *v = serde_json::Number::from_f64(f64::from_bits(bits))
                        .map(Value::Number)
                        .ok_or_else(|| ScriptError::Op {
                            location: location.to_string(),
                            message: format!("$f64 位串 {hex:?} 不是有限浮点数"),
                        })?;
                    return Ok(());
                }
            }
            for child in map.values_mut() {
                revive_f64(child, location)?;
            }
            Ok(())
        }
        Value::Array(arr) => {
            for child in arr {
                revive_f64(child, location)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Compare numbers by semantics (5.0 == 5); recurse through other structures. JS's JSON.stringify doesn't preserve
/// the decimal point on integer-valued floats, so the representation always drifts on round-trip; comparing by representation would yield a screenful of false positives.
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

/// Pcg32 → {"state": "u64-string", "inc": "u64-string"} (JSON number can't hold u64, go through strings).
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
        // JS's 5.0 serializes as 5 (JSON doesn't distinguish); compare by numeric value
        assert_eq!(w.get_field(e, "Velocity.x").unwrap().as_f64(), Some(5.0));
    }

    #[test]
    fn ctx_ask_emits_service_ask_with_callback_in_id() {
        let mut eng = engine_with(
            r#"
            vitric.system("brain", {query: ["Position"], writes: []}, (entities, ctx) => {
                for (const e of entities) { ctx.ask("llm", "hello", "onBrainReply"); }
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut rng = Pcg32::new(1);
        let out = eng.run_systems(&mut w, &mut rng, 7).unwrap();
        let ask = out.events.iter().find(|ev| ev.name == "llm-ask").expect("ctx.ask 应发出 llm-ask");
        assert_eq!(ask.data["prompt"], json!("hello"));
        let id = ask.data["id"].as_str().unwrap();
        assert!(id.starts_with("onBrainReply#7#"), "id 应把回调名和 tick 编进去，实际 {id}");
    }

    #[test]
    fn on_reply_dispatches_to_callback_named_in_id() {
        let mut eng = engine_with(
            r#"
            vitric.fn("onBrainReply", (reply, ctx) => { ctx.emit("handled", {text: reply.text}); });
            "#,
        );
        let mut w = World::new();
        let mut rng = Pcg32::new(1);
        let out = eng
            .call_fn(
                "__onReply",
                &json!({"id": "onBrainReply#7#0", "text": "hi there"}),
                None,
                &mut w,
                &mut rng,
                8,
            )
            .unwrap();
        let handled = out.events.iter().find(|ev| ev.name == "handled").expect("__onReply 应分发到 onBrainReply");
        assert_eq!(handled.data["text"], json!("hi there"));
    }

    #[test]
    fn on_reply_unregistered_callback_errors_loud() {
        let mut eng = engine_with(r#"vitric.fn("present", (a, c) => {});"#);
        let mut w = World::new();
        let mut rng = Pcg32::new(1);
        let err = eng
            .call_fn(
                "__onReply",
                &json!({"id": "ghost#1#0", "text": "x"}),
                None,
                &mut w,
                &mut rng,
                0,
            )
            .unwrap_err();
        assert!(format!("{err}").contains("ghost"), "应显式报未注册回调，实际 {err}");
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
        // the world was not poisoned
        assert_eq!(w.get_field(e, "Position.x").unwrap(), &json!(0.0));
    }

    #[test]
    fn readonly_system_with_whole_valued_floats_is_not_a_write() {
        // Regression: world holds 0.0/5.0, JS round-trip becomes 0/5 — different representation, same semantics.
        // A system that changes nothing must never be misjudged as an out-of-scope write (this once caused random stalls in a normal project).
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
        // write-back must commit only when the whole batch succeeds: when the second entity is out-of-scope, the first entity's legal write must not leak into the world either
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
    fn js_boundary_preserves_f64_precision_exactly() {
        // Regression: QuickJS's JSON.stringify is not shortest-round-trip (-7.3666666666666645 gets truncated to
        // -7.366666666666664, off by 1 ULP). prelude's __numStr must restore it exactly,
        // otherwise read-only systems get misjudged as out-of-scope writes and read-write systems silently lose precision.
        let tricky = -7.366_666_666_666_664_5_f64;
        let mut eng = engine_with(
            r#"
            vitric.system("mover", {query: ["Position", "Velocity"], writes: ["Velocity"]}, (entities) => {
                for (const e of entities) { e.Velocity.x = e.Position.y; } // copy the tricky value verbatim
            });
            "#,
        );
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": tricky})).unwrap();
        w.set_component(e, "Velocity", json!({"x": 0.0, "y": tricky})).unwrap();
        let mut rng = Pcg32::new(1);
        eng.run_systems(&mut w, &mut rng, 0).unwrap(); // Velocity.y is read-only: must not be misjudged as out-of-scope
        assert_eq!(w.get_field(e, "Velocity.x").unwrap().as_f64(), Some(tricky), "精度丢了");
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
        // Regression: only Date.now was once poisoned, but new Date().getTime() still leaked wall-clock into world state
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

        // Explicit argument passing is pure computation; it must be allowed through
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
        // After JS draws two f64s, the Rust side keeps drawing; the four consecutive draws must exactly match four pure-Rust draws
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
    fn system_despawn_of_named_entity_fully_removes_it() {
        // Reproduce the "suspected engine bug" logged in frontier: when a system calls ctx.despawn(named entity),
        // does it fully remove (both name and entity gone) or just unregister the name and leave the entity in queries?
        let mut eng = engine_with(
            r#"
            vitric.system("reaper", {query: ["Coin"], writes: []}, (entities, ctx) => {
                for (const e of entities) {
                    ctx.despawn(e.id);
                }
            });
            "#,
        );
        let mut w = World::new();
        let victim = w.spawn_named("victim").unwrap();
        w.set_component(victim, "Coin", json!({"value": 1})).unwrap();
        let mut rng = Pcg32::new(1);
        eng.run_systems(&mut w, &mut rng, 0).unwrap();
        assert!(!w.is_alive(victim), "命名实体 despawn 后应不存活");
        assert!(w.entity("victim").is_err(), "名字应注销");
        assert!(w.query(&["Coin"]).is_empty(), "实体应从查询消失(不只名字)");
    }

    #[test]
    fn ctx_set_field_writes_by_name_and_handle() {
        // The foundation of "do something to whatever you point at": ctx.setField writes one field of any entity, by name or handle.
        let mut eng = engine_with(
            r#"
            vitric.system("poke", {query: ["Coin"], writes: []}, (entities, ctx) => {
                ctx.setField("target", "Velocity.x", 7);                    // write another entity by name
                for (const e of entities) ctx.setField(e.id, "Coin.value", 9); // write a queried entity by handle
            });
            "#,
        );
        let mut w = World::new();
        let target = w.spawn_named("target").unwrap();
        w.set_component(target, "Velocity", json!({"x": 0.0, "y": 0.0})).unwrap();
        let coin = w.spawn();
        w.set_component(coin, "Coin", json!({"value": 1})).unwrap();
        let mut rng = Pcg32::new(1);
        eng.run_systems(&mut w, &mut rng, 0).unwrap();
        assert_eq!(w.get_field(target, "Velocity.x").unwrap().as_f64(), Some(7.0), "按名字 setField");
        assert_eq!(w.get_field(coin, "Coin.value").unwrap(), &json!(9), "按句柄 setField");
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
        // __onReply is prelude's built-in ctx.ask reply dispatcher; always registered (listed before user functions)
        assert_eq!(eng.fns, vec!["__onReply", "explode"]);
        let mut w = World::new();
        let mut rng = Pcg32::new(1);
        let out = eng
            .call_fn("explode", &json!({"count": 3, "where": "here"}), None, &mut w, &mut rng, 5)
            .unwrap();
        assert_eq!(w.query(&["Coin"]).len(), 3);
        assert_eq!(out.events[0].data["at"], json!("here"));
        // unknown function errors and lists the registered ones
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
        // A bad script failing to reload → error; the old behavior must not be left half-dead
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
