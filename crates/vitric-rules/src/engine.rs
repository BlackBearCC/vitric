use std::collections::VecDeque;
use std::fmt;

use serde_json::{json, Map, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};

use crate::model::{Event, Rule, RuleSet, Trigger};

/// A function call the rule system asks the script layer to perform (the product of the `call` action).
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptCall {
    pub function: String,
    pub args: Value,
    /// The self entity bound at trigger time (if any).
    pub self_entity: Option<EntityId>,
}

/// The output of rule execution within a single tick.
#[derive(Debug, Default)]
pub struct TickOutput {
    /// Calls pending script-layer execution.
    pub calls: Vec<ScriptCall>,
    /// Rule ids fired during this tick (with trigger-count order); for debugging / replays.
    pub fired: Vec<String>,
    /// Copies of events emitted by rules — the control plane's event log depends on it, so AI can see the causal chain.
    pub emitted: Vec<Event>,
}

/// Rule runtime error. The rule system writes no fallback: on error it stops and explains.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleError {
    /// Event cascade too deep (rule emits event that triggers rule...).
    CascadeOverflow { depth: usize, chain: Vec<String> },
    /// A rule's action / condition execution failed.
    Exec { rule: String, at: String, message: String },
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleError::CascadeOverflow { depth, chain } => write!(
                f,
                "事件级联超过 {depth} 层，疑似规则死循环。触发链: {}。\
                 提示：检查链上的规则是否互相 emit 形成回路",
                chain.join(" -> ")
            ),
            RuleError::Exec { rule, at, message } => write!(
                f,
                "规则 {rule:?} 在 {at} 执行失败: {message}"
            ),
        }
    }
}

impl std::error::Error for RuleError {}

const MAX_CASCADE_DEPTH: usize = 8;

/// The rule engine. Stateless internally: each tick takes events in, mutates the world, returns output.
/// Clone-able (rules/schema are immutable copies fixed at assembly time): playtest derives a scene view that needs to read rules,
/// and the engine is privately held by the Runtime GameLogic assembly — the borrow checker forbids simultaneously borrowing logic mutably and engine immutably,
/// so copying a read-only snapshot is the cleanest approach; the engine itself is stateless, so copying carries no semantic burden.
#[derive(Clone)]
pub struct Engine {
    pub rules: RuleSet,
    pub schema: Schema,
}

/// Binding context active while a rule fires.
#[derive(Clone, Copy, Default)]
struct Ctx<'a> {
    self_e: Option<EntityId>,
    other: Option<EntityId>,
    event: Option<&'a Event>,
}

impl Engine {
    pub fn new(rules: RuleSet, schema: Schema) -> Engine {
        Engine { rules, schema }
    }

    /// Evaluate a set of conditions standalone (no event context). The assertion system uses it:
    /// returns true when all conditions hold. Path syntax matches rules (@entity-name / e3v1 handle).
    pub fn check(
        &self,
        world: &World,
        conditions: &[(String, String, Value)],
    ) -> Result<bool, RuleError> {
        for (i, (left, op, right)) in conditions.iter().enumerate() {
            let at = format!("check/{i}");
            if !self.eval_condition(world, Ctx::default(), left, op, right, "<assertion>", &at)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Run one tick: first run tick rules, then drain events FIFO (including cascades).
    /// `inbox`: events produced externally this tick (input, collisions, etc.).
    pub fn process_tick(
        &self,
        world: &mut World,
        inbox: Vec<Event>,
    ) -> Result<TickOutput, RuleError> {
        let mut out = TickOutput::default();
        let mut queue: VecDeque<(Event, usize, Vec<String>)> =
            inbox.into_iter().map(|e| (e, 0, Vec::new())).collect();

        // tick-triggered rules (one round per tick, rules in file order)
        for rule in &self.rules.rules {
            if matches!(rule.trigger, Trigger::Tick) {
                self.run_rule_bound(rule, world, Ctx::default(), &mut out, &mut queue, 0, &[])?;
            }
        }

        // Event queue (FIFO, cascade depth limited)
        while let Some((event, depth, chain)) = queue.pop_front() {
            if depth > MAX_CASCADE_DEPTH {
                return Err(RuleError::CascadeOverflow { depth: MAX_CASCADE_DEPTH, chain });
            }
            for rule in &self.rules.rules {
                let Trigger::Event { name, filter, between } = &rule.trigger else {
                    continue;
                };
                if *name != event.name || !filter_matches(filter, &event.data) {
                    continue;
                }
                let mut ctx = Ctx { event: Some(&event), ..Default::default() };
                if let Some((comp_a, comp_b)) = between {
                    match bind_between(world, &event, comp_a, comp_b) {
                        Some((s, o)) => {
                            ctx.self_e = Some(s);
                            ctx.other = Some(o);
                        }
                        None => continue, // collision parties don't satisfy the component requirements, rule doesn't apply
                    }
                }
                self.run_rule_bound(rule, world, ctx, &mut out, &mut queue, depth, &chain)?;
            }
        }
        Ok(out)
    }

    /// Handle `each` expansion then execute per entity.
    /// The parameters are the full context threaded down by the cascade execution (output/event queue/depth/call chain); splitting them would hide intent.
    #[allow(clippy::too_many_arguments)]
    fn run_rule_bound(
        &self,
        rule: &Rule,
        world: &mut World,
        ctx: Ctx,
        out: &mut TickOutput,
        queue: &mut VecDeque<(Event, usize, Vec<String>)>,
        depth: usize,
        chain: &[String],
    ) -> Result<(), RuleError> {
        match &rule.each {
            Some(comps) => {
                let required: Vec<&str> = comps.iter().map(|s| s.as_str()).collect();
                for id in world.query(&required) {
                    if !world.is_alive(id) {
                        continue; // a prior entity's action may have despawned it
                    }
                    let bound = Ctx { self_e: Some(id), ..ctx };
                    self.run_rule_once(rule, world, bound, out, queue, depth, chain)?;
                }
                Ok(())
            }
            None => self.run_rule_once(rule, world, ctx, out, queue, depth, chain),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_rule_once(
        &self,
        rule: &Rule,
        world: &mut World,
        ctx: Ctx,
        out: &mut TickOutput,
        queue: &mut VecDeque<(Event, usize, Vec<String>)>,
        depth: usize,
        chain: &[String],
    ) -> Result<(), RuleError> {
        // execute only when all conditions hold
        for (i, (left, op, right)) in rule.conditions.iter().enumerate() {
            let at = format!("if/{i}");
            if !self.eval_condition(world, ctx, left, op, right, &rule.id, &at)? {
                return Ok(());
            }
        }
        out.fired.push(rule.id.clone());
        for (i, action) in rule.actions.iter().enumerate() {
            let at = format!("do/{i}");
            self.exec_action(rule, world, ctx, action, &at, out, queue, depth, chain)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_condition(
        &self,
        world: &World,
        ctx: Ctx,
        left: &str,
        op: &str,
        right: &Value,
        rule_id: &str,
        at: &str,
    ) -> Result<bool, RuleError> {
        let err = |message: String| RuleError::Exec {
            rule: rule_id.to_string(),
            at: at.to_string(),
            message,
        };
        if op == "exists" || op == "!exists" {
            let present = self.ref_exists(world, ctx, left).map_err(err)?;
            return Ok(if op == "exists" { present } else { !present });
        }
        let lv = self.resolve(world, ctx, &Value::String(left.to_string())).map_err(err)?;
        let rv = self.resolve(world, ctx, right).map_err(err)?;
        match op {
            "==" => Ok(lv == rv),
            "!=" => Ok(lv != rv),
            "<" | "<=" | ">" | ">=" => {
                let (a, b) = match (lv.as_f64(), rv.as_f64()) {
                    (Some(a), Some(b)) => (a, b),
                    _ => {
                        return Err(err(format!(
                            "操作符 {op} 只能比较数字，拿到 {lv} 和 {rv}。\
                             文本/布尔请用 == 或 !="
                        )))
                    }
                };
                Ok(match op {
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    _ => a >= b,
                })
            }
            other => Err(err(format!("未知操作符 {other:?}（解析层应已拦截）"))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_action(
        &self,
        rule: &Rule,
        world: &mut World,
        ctx: Ctx,
        action: &Value,
        at: &str,
        out: &mut TickOutput,
        queue: &mut VecDeque<(Event, usize, Vec<String>)>,
        depth: usize,
        chain: &[String],
    ) -> Result<(), RuleError> {
        let err = |message: String| RuleError::Exec {
            rule: rule.id.clone(),
            at: at.to_string(),
            message,
        };
        let obj = action.as_object().expect("parse layer validated action is an object");

        if let Some(target) = obj.get("set").and_then(|v| v.as_str()) {
            let to = obj
                .get("to")
                .ok_or_else(|| err("set 动作缺少 to".into()))?;
            let value = self.resolve(world, ctx, to).map_err(&err)?;
            let (id, path) = self.entity_field(world, ctx, target).map_err(&err)?;
            world.set_field(id, &path, value).map_err(|e| err(e.to_string()))?;
            return Ok(());
        }

        if let Some(target) = obj.get("add").and_then(|v| v.as_str()) {
            let by = obj.get("by").ok_or_else(|| err("add 动作缺少 by".into()))?;
            let delta = self.resolve(world, ctx, by).map_err(&err)?;
            let (id, path) = self.entity_field(world, ctx, target).map_err(&err)?;
            let cur = world.get_field(id, &path).map_err(|e| err(e.to_string()))?.clone();
            let sum = match (cur.as_i64(), delta.as_i64()) {
                // explicit checked: debug panic / release wraparound would make the same replay diverge across two build modes
                (Some(a), Some(b)) => json!(a.checked_add(b).ok_or_else(|| err(format!(
                    "add 整数溢出：{a} + {b} 超出 i64 范围"
                )))?),
                _ => match (cur.as_f64(), delta.as_f64()) {
                    (Some(a), Some(b)) => json!(a + b),
                    _ => {
                        return Err(err(format!(
                            "add 只能加数字：字段当前值 {cur}，增量 {delta}"
                        )))
                    }
                },
            };
            world.set_field(id, &path, sum).map_err(|e| err(e.to_string()))?;
            return Ok(());
        }

        if let Some(spec) = obj.get("spawn") {
            let comps = spec
                .get("components")
                .and_then(|v| v.as_object())
                .ok_or_else(|| {
                    err("spawn 动作缺少 components 对象。写法: {\"spawn\": {\"components\": {...}}}".into())
                })?;
            let id = match spec.get("name").and_then(|v| v.as_str()) {
                Some(name) => world.spawn_named(name).map_err(|e| err(e.to_string()))?,
                None => world.spawn(),
            };
            for (cname, cval) in comps {
                let cschema = self.schema.component(cname).ok_or_else(|| {
                    err(format!(
                        "spawn 引用了未知组件 {cname:?}。schema 里的组件: [{}]",
                        self.schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
                    ))
                })?;
                let resolved = self.resolve(world, ctx, cval).map_err(&err)?;
                let mut report = vitric_data::ValidationReport::default();
                let normalized = cschema.normalize(&resolved, &format!("spawn/{cname}"), &mut report);
                if !report.ok() {
                    return Err(err(format!("spawn 的组件值未通过 schema 校验:\n{report}")));
                }
                world.set_component(id, cname, normalized).map_err(|e| err(e.to_string()))?;
            }
            return Ok(());
        }

        if let Some(target) = obj.get("despawn").and_then(|v| v.as_str()) {
            let id = self.entity_ref(world, ctx, target).map_err(&err)?;
            world.despawn(id).map_err(|e| err(e.to_string()))?;
            return Ok(());
        }

        if let Some(name) = obj.get("emit").and_then(|v| v.as_str()) {
            let data = obj.get("data").cloned().unwrap_or_else(|| json!({}));
            let resolved = self.resolve(world, ctx, &data).map_err(&err)?;
            let mut new_chain = chain.to_vec();
            new_chain.push(format!("{}→{}", rule.id, name));
            let event = Event::new(name, resolved);
            out.emitted.push(event.clone());
            queue.push_back((event, depth + 1, new_chain));
            return Ok(());
        }

        if let Some(function) = obj.get("call").and_then(|v| v.as_str()) {
            let with = obj.get("with").cloned().unwrap_or_else(|| json!({}));
            let args = self.resolve(world, ctx, &with).map_err(&err)?;
            out.calls.push(ScriptCall {
                function: function.to_string(),
                args,
                self_entity: ctx.self_e,
            });
            return Ok(());
        }

        Err(err("action is not any known kind (parse layer should have rejected it)".into()))
    }

    /// Recursively resolve a value: a string that is a reference (self./other./@/event.) is replaced with the actual value,
    /// objects/arrays are processed item by item, everything else is returned as-is. A standalone "self"/"other" resolves to an entity handle string.
    fn resolve(&self, world: &World, ctx: Ctx, value: &Value) -> Result<Value, String> {
        match value {
            Value::String(s) => match self.try_ref(world, ctx, s)? {
                Some(v) => Ok(v),
                None => Ok(value.clone()),
            },
            Value::Array(arr) => arr
                .iter()
                .map(|v| self.resolve(world, ctx, v))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Object(map) => {
                // string template {"format": "SCORE {}", "args": [paths...]} —
                // the canonical way to turn numeric state into on-screen text (Text.content); there is no other way in rules
                if let Some(fmt) = map.get("format").and_then(|v| v.as_str()) {
                    if map.len() == 1 || (map.len() == 2 && map.contains_key("args")) {
                        return self.format_template(world, ctx, fmt, map.get("args"));
                    }
                }
                let mut out = Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), self.resolve(world, ctx, v)?);
                }
                Ok(Value::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    /// Replace each `{}` with the resolved args; the count must match exactly.
    fn format_template(
        &self,
        world: &World,
        ctx: Ctx,
        fmt: &str,
        args: Option<&Value>,
    ) -> Result<Value, String> {
        let args = match args {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| self.resolve(world, ctx, v))
                .collect::<Result<Vec<_>, _>>()?,
            Some(other) => return Err(format!("format 的 args 必须是数组，拿到 {other}")),
        };
        let slots = fmt.matches("{}").count();
        if slots != args.len() {
            return Err(format!(
                "format 模板 {fmt:?} 有 {slots} 个 {{}}，但 args 给了 {} 个",
                args.len()
            ));
        }
        let mut parts = fmt.split("{}");
        let mut out = String::from(parts.next().unwrap_or(""));
        for (arg, part) in args.iter().zip(parts) {
            match arg {
                Value::String(s) => out.push_str(s),
                other => out.push_str(&other.to_string()),
            }
            out.push_str(part);
        }
        Ok(Value::String(out))
    }

    /// If the string is a reference, evaluate it; otherwise return None (treat as a literal).
    fn try_ref(&self, world: &World, ctx: Ctx, s: &str) -> Result<Option<Value>, String> {
        // the entity binding itself → a handle string
        if s == "self" || s == "other" {
            let id = self.entity_ref(world, ctx, s)?;
            return Ok(Some(json!(id.to_string())));
        }
        if let Some(rest) = s.strip_prefix("event.") {
            let event = ctx.event.ok_or_else(|| {
                format!("引用了 {s:?}，但该规则不是事件触发的（tick 规则没有 event 上下文）")
            })?;
            let v = event.data.get(rest).ok_or_else(|| {
                format!(
                    "事件 {:?} 没有字段 {rest:?}，现有字段: [{}]",
                    event.name,
                    event.data.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            return Ok(Some(v.clone()));
        }
        for prefix in ["self.", "other."] {
            if let Some(rest) = s.strip_prefix(prefix) {
                let id = self.entity_ref(world, ctx, &prefix[..prefix.len() - 1])?;
                return world
                    .get_field(id, rest)
                    .map(|v| Some(v.clone()))
                    .map_err(|e| e.to_string());
            }
        }
        if let Some(rest) = s.strip_prefix('@') {
            let (name, path) = match rest.split_once('.') {
                Some((n, p)) => (n, Some(p)),
                None => (rest, None),
            };
            let id = world.entity(name).map_err(|e| e.to_string())?;
            return match path {
                Some(p) => world
                    .get_field(id, p)
                    .map(|v| Some(v.clone()))
                    .map_err(|e| e.to_string()),
                None => Ok(Some(json!(id.to_string()))),
            };
        }
        Ok(None)
    }

    /// exists check: whether a component or field is present. exists is fundamentally about "is it there",
    /// so every form of entity absence — @name with no such entity (despawn unregisters the name), a stale handle whose generation has moved on,
    /// unbound self/other — answers false instead of erroring. The relaxation applies only to exists/!exists:
    /// other operators still error when referencing a missing entity (comparing against a non-existent field is a genuine rule bug).
    /// A reference whose syntax itself is invalid (neither self/other/@name nor a handle) still errors.
    fn ref_exists(&self, world: &World, ctx: Ctx, path: &str) -> Result<bool, String> {
        let (ent_part, rest) = match path.split_once('.') {
            Some((e, r)) => (e, Some(r)),
            None => (path, None),
        };
        // @name handled specially: entity_ref_opt errors on unknown names (which is what other operators want),
        // but for exists "the name is gone" is exactly the legitimate answer being probed
        let id = if let Some(name) = ent_part.strip_prefix('@') {
            match world.entity(name) {
                Ok(id) => id,
                Err(_) => return Ok(false),
            }
        } else {
            match self.entity_ref_opt(world, ctx, ent_part)? {
                Some(id) => id,
                None => return Ok(false),
            }
        };
        if !world.is_alive(id) {
            return Ok(false);
        }
        match rest {
            None => Ok(true),
            Some(r) => match r.split_once('.') {
                None => Ok(world.has_component(id, r)),
                Some(_) => Ok(world.get_field(id, r).is_ok()),
            },
        }
    }

    /// Parse an "entity.field-path" target, e.g. "self.Score.value".
    fn entity_field(&self, world: &World, ctx: Ctx, target: &str) -> Result<(EntityId, String), String> {
        let (ent_part, path) = target.split_once('.').ok_or_else(|| {
            format!(
                "目标 {target:?} 缺少字段路径。写法: \"self.组件.字段\"，如 \"self.Score.value\""
            )
        })?;
        let id = self.entity_ref(world, ctx, ent_part)?;
        Ok((id, path.to_string()))
    }

    /// Parse an entity reference: "self" / "other" / "@name" / "e3v1".
    fn entity_ref(&self, world: &World, ctx: Ctx, s: &str) -> Result<EntityId, String> {
        self.entity_ref_opt(world, ctx, s)?.ok_or_else(|| {
            format!(
                "{s:?} 没有绑定。self/other 只在对应触发器下有值\
                （between 碰撞绑定 self+other，each 绑定 self）"
            )
        })
    }

    fn entity_ref_opt(&self, world: &World, ctx: Ctx, s: &str) -> Result<Option<EntityId>, String> {
        match s {
            "self" => Ok(ctx.self_e),
            "other" => Ok(ctx.other),
            _ => {
                if let Some(name) = s.strip_prefix('@') {
                    return world.entity(name).map(Some).map_err(|e| e.to_string());
                }
                if s.starts_with('e') && s.contains('v') {
                    if let Ok(id) = s.parse::<EntityId>() {
                        return Ok(Some(id));
                    }
                }
                Err(format!(
                    "无法识别实体引用 {s:?}。可用写法: self / other / @实体名 / e3v1 句柄"
                ))
            }
        }
    }
}

/// Equality filter on event data.
fn filter_matches(filter: &Map<String, Value>, data: &Map<String, Value>) -> bool {
    filter.iter().all(|(k, v)| data.get(k) == Some(v))
}

/// between binding: of the a/b entities in the collision event data, the one that has comp_a becomes self.
fn bind_between(
    world: &World,
    event: &Event,
    comp_a: &str,
    comp_b: &str,
) -> Option<(EntityId, EntityId)> {
    let parse = |key: &str| -> Option<EntityId> {
        event.data.get(key)?.as_str()?.parse().ok()
    };
    let a = parse("a")?;
    let b = parse("b")?;
    if world.has_component(a, comp_a) && world.has_component(b, comp_b) {
        Some((a, b))
    } else if world.has_component(b, comp_a) && world.has_component(a, comp_b) {
        Some((b, a))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use vitric_data::Schema;

    use super::*;

    fn schema() -> Schema {
        Schema::parse(
            &json!({"components": {
                "Player": {"fields": {}},
                "Coin": {"fields": {"value": {"type": "int", "default": 1}}},
                "Score": {"fields": {"value": {"type": "int", "default": 0}}},
                "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Velocity": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}}
            }}),
            "schema.json",
        )
        .unwrap()
    }

    fn engine(rules: Value) -> Engine {
        Engine::new(RuleSet::parse(&rules, "rules.json").unwrap(), schema())
    }

    fn world_with_player_and_coin() -> (World, EntityId, EntityId) {
        let mut w = World::new();
        let p = w.spawn_named("player").unwrap();
        w.set_component(p, "Player", json!({})).unwrap();
        w.set_component(p, "Score", json!({"value": 0})).unwrap();
        let c = w.spawn().to_owned();
        w.set_component(c, "Coin", json!({"value": 5})).unwrap();
        (w, p, c)
    }

    #[test]
    fn format_template_renders_numbers_into_text() {
        let eng = engine(json!({"rules": [{
            "id": "show-score",
            "on": {"event": "refresh"},
            "do": [{"set": "@player.Label.text",
                    "to": {"format": "SCORE {} / {}", "args": ["@player.Score.value", 3]}}]
        }]}));
        let (mut w, p, _) = world_with_player_and_coin();
        w.set_component(p, "Label", json!({"text": ""})).unwrap();
        w.set_field(p, "Score.value", json!(2)).unwrap();
        eng.process_tick(&mut w, vec![Event::new("refresh", json!({}))]).unwrap();
        assert_eq!(w.get_field(p, "Label.text").unwrap(), &json!("SCORE 2 / 3"));
    }

    #[test]
    fn format_template_arity_mismatch_is_reported() {
        let eng = engine(json!({"rules": [{
            "id": "bad-fmt",
            "on": {"event": "refresh"},
            "do": [{"set": "@player.Label.text",
                    "to": {"format": "A {} B {}", "args": ["@player.Score.value"]}}]
        }]}));
        let (mut w, p, _) = world_with_player_and_coin();
        w.set_component(p, "Label", json!({"text": ""})).unwrap();
        let err = eng
            .process_tick(&mut w, vec![Event::new("refresh", json!({}))])
            .unwrap_err();
        assert!(err.to_string().contains("format"), "{err}");
    }

    #[test]
    fn collision_between_collects_coin() {
        let eng = engine(json!({"rules": [{
            "id": "collect",
            "on": {"event": "collision", "between": ["Player", "Coin"]},
            "do": [
                {"add": "self.Score.value", "by": "other.Coin.value"},
                {"despawn": "other"}
            ]
        }]}));
        let (mut w, p, c) = world_with_player_and_coin();
        // a/b order reversed: binding should still work
        let out = eng
            .process_tick(
                &mut w,
                vec![Event::new("collision", json!({"a": c.to_string(), "b": p.to_string()}))],
            )
            .unwrap();
        assert_eq!(out.fired, vec!["collect"]);
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(5));
        assert!(!w.is_alive(c));
    }

    #[test]
    fn tick_each_runs_per_entity_in_order() {
        let eng = engine(json!({"rules": [{
            "id": "move",
            "on": "tick",
            "each": ["Position", "Velocity"],
            "do": [{"add": "self.Position.x", "by": "self.Velocity.x"}]
        }]}));
        let mut w = World::new();
        for i in 0..3 {
            let e = w.spawn();
            w.set_component(e, "Position", json!({"x": i as f64, "y": 0.0})).unwrap();
            w.set_component(e, "Velocity", json!({"x": 10.0, "y": 0.0})).unwrap();
        }
        let out = eng.process_tick(&mut w, vec![]).unwrap();
        assert_eq!(out.fired.len(), 3);
        let xs: Vec<Value> = w
            .query(&["Position"])
            .into_iter()
            .map(|e| w.get_field(e, "Position.x").unwrap().clone())
            .collect();
        assert_eq!(xs, vec![json!(10.0), json!(11.0), json!(12.0)]);
    }

    #[test]
    fn conditions_gate_execution() {
        let eng = engine(json!({"rules": [{
            "id": "low-hp-warning",
            "on": "tick",
            "each": ["Score"],
            "if": [["self.Score.value", ">=", 10]],
            "do": [{"emit": "rich", "data": {"who": "self"}}]
        }, {
            "id": "on-rich",
            "on": {"event": "rich"},
            "do": [{"set": "@player.Score.value", "to": 0}]
        }]}));
        let (mut w, p, _) = world_with_player_and_coin();
        // score too low: doesn't fire
        let out = eng.process_tick(&mut w, vec![]).unwrap();
        assert!(out.fired.is_empty());
        // score high enough: rich cascades to fire on-rich, which zeroes it
        w.set_field(p, "Score.value", json!(10)).unwrap();
        let out = eng.process_tick(&mut w, vec![]).unwrap();
        assert_eq!(out.fired, vec!["low-hp-warning", "on-rich"]);
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(0));
    }

    #[test]
    fn spawn_resolves_refs_and_validates() {
        let eng = engine(json!({"rules": [{
            "id": "drop-coin",
            "on": {"event": "drop"},
            "do": [{"spawn": {"components": {
                "Coin": {"value": "event.amount"},
                "Position": {"x": "@player.Position.x", "y": 0.0}
            }}}]
        }]}));
        let mut w = World::new();
        let p = w.spawn_named("player").unwrap();
        w.set_component(p, "Position", json!({"x": 7.0, "y": 1.0})).unwrap();
        eng.process_tick(&mut w, vec![Event::new("drop", json!({"amount": 3}))]).unwrap();
        let coins = w.query(&["Coin"]);
        assert_eq!(coins.len(), 1);
        assert_eq!(w.get_field(coins[0], "Coin.value").unwrap(), &json!(3));
        assert_eq!(w.get_field(coins[0], "Position.x").unwrap(), &json!(7.0));
    }

    #[test]
    fn cascade_overflow_reports_chain() {
        let eng = engine(json!({"rules": [
            {"id": "ping", "on": {"event": "ping"}, "do": [{"emit": "pong", "data": {}}]},
            {"id": "pong", "on": {"event": "pong"}, "do": [{"emit": "ping", "data": {}}]}
        ]}));
        let mut w = World::new();
        let err = eng
            .process_tick(&mut w, vec![Event::new("ping", json!({}))])
            .unwrap_err();
        match &err {
            RuleError::CascadeOverflow { chain, .. } => {
                assert!(chain.iter().any(|c| c.contains("ping")), "{err}");
            }
            other => panic!("错误类型不对: {other:?}"),
        }
    }

    #[test]
    fn exec_errors_are_explicit_and_helpful() {
        let eng = engine(json!({"rules": [{
            "id": "bad",
            "on": {"event": "go"},
            "do": [{"set": "self.Score.value", "to": 1}]
        }]}));
        let mut w = World::new();
        let err = eng.process_tick(&mut w, vec![Event::new("go", json!({}))]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad") && msg.contains("self"), "错误要带规则 id 和原因: {msg}");
    }

    /// Regression (exists-erroring bug): once an entity is despawned its name is unregistered,
    /// `["@name", "exists"]` must evaluate to false rather than error with "no entity named X" —
    /// otherwise every "do something after detecting X was destroyed" rule halts the simulation exactly when X is destroyed.
    #[test]
    fn exists_on_despawned_named_entity_is_false_not_error() {
        let eng = engine(json!({"rules": [
            {
                "id": "door-still-there",
                "on": {"event": "check"},
                "if": [["@door", "exists"]],
                "do": [{"set": "@player.Score.value", "to": 1}]
            },
            {
                "id": "door-destroyed",
                "on": {"event": "check"},
                "if": [["@door", "!exists"]],
                "do": [{"set": "@player.Score.value", "to": 2}]
            }
        ]}));
        let (mut w, p, _) = world_with_player_and_coin();
        let door = w.spawn_named("door").unwrap();
        w.set_component(door, "Coin", json!({"value": 1})).unwrap();

        // door still present: exists hits
        let out = eng.process_tick(&mut w, vec![Event::new("check", json!({}))]).unwrap();
        assert_eq!(out.fired, vec!["door-still-there"]);
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(1));

        // door gone: no error, !exists hits, simulation continues
        w.despawn(door).unwrap();
        let out = eng.process_tick(&mut w, vec![Event::new("check", json!({}))]).unwrap();
        assert_eq!(out.fired, vec!["door-destroyed"]);
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(2));
        // another round stays smooth (not a one-time exemption)
        eng.process_tick(&mut w, vec![Event::new("check", json!({}))]).unwrap();
    }

    /// All forms of entity absence for exists: references with field paths, stale handles, unbound self/other,
    /// all answer false without erroring.
    #[test]
    fn exists_handles_all_missing_entity_forms() {
        let eng = engine(json!({"rules": []}));
        let (mut w, _, _) = world_with_player_and_coin();
        let door = w.spawn_named("door").unwrap();
        w.set_component(door, "Coin", json!({"value": 1})).unwrap();
        let handle = door.to_string();
        w.despawn(door).unwrap();

        let f = json!(false);
        // @name + component path / field path: entity absent → false
        assert!(eng.check(&w, &[("@door".into(), "!exists".into(), f.clone())]).unwrap());
        assert!(!eng.check(&w, &[("@door.Coin".into(), "exists".into(), f.clone())]).unwrap());
        assert!(!eng.check(&w, &[("@door.Coin.value".into(), "exists".into(), f.clone())]).unwrap());
        // stale handle (generation has moved on) → false
        assert!(!eng.check(&w, &[(handle, "exists".into(), f.clone())]).unwrap());
        // self/other in an unbound context → false
        assert!(!eng.check(&w, &[("self".into(), "exists".into(), f.clone())]).unwrap());
        assert!(!eng.check(&w, &[("other.Coin".into(), "exists".into(), f.clone())]).unwrap());
        // missing component/field on a live entity is still false (existing semantics unchanged)
        assert!(!eng.check(&w, &[("@player.Coin".into(), "exists".into(), f.clone())]).unwrap());
        assert!(eng.check(&w, &[("@player.Score".into(), "exists".into(), f)]).unwrap());
    }

    /// Absence exemption is only for exists/!exists: other operators reading a missing entity still error explicitly —
    /// that's a real rule bug, and silently passing would hide it.
    #[test]
    fn non_exists_operators_still_error_on_missing_entity() {
        let eng = engine(json!({"rules": [{
            "id": "read-the-dead",
            "on": {"event": "check"},
            "if": [["@door.Coin.value", "==", 1]],
            "do": [{"set": "@player.Score.value", "to": 9}]
        }]}));
        let (mut w, _, _) = world_with_player_and_coin();
        let err = eng
            .process_tick(&mut w, vec![Event::new("check", json!({}))])
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("door"), "错误要点名缺失的实体: {msg}");
        // a reference whose syntax itself is invalid also errors (exists does not exempt syntax errors)
        let err = eng
            .check(&w, &[("door".into(), "exists".into(), json!(false))])
            .unwrap_err();
        assert!(err.to_string().contains("无法识别"), "{err}");
    }

    #[test]
    fn input_filter_matches_exactly() {
        let eng = engine(json!({"rules": [{
            "id": "jump",
            "on": {"event": "input", "filter": {"action": "jump", "phase": "pressed"}},
            "do": [{"set": "@player.Score.value", "to": 99}]
        }]}));
        let (mut w, p, _) = world_with_player_and_coin();
        eng.process_tick(
            &mut w,
            vec![Event::new("input", json!({"action": "jump", "phase": "released"}))],
        )
        .unwrap();
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(0), "phase 不匹配不触发");
        eng.process_tick(
            &mut w,
            vec![Event::new("input", json!({"action": "jump", "phase": "pressed"}))],
        )
        .unwrap();
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(99));
    }
}
