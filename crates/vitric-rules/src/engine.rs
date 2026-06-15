use std::collections::VecDeque;
use std::fmt;

use serde_json::{json, Map, Value};

use vitric_data::Schema;
use vitric_ecs::{EntityId, World};

use crate::model::{Event, Rule, RuleSet, Trigger};

/// 规则要求脚本层执行的函数调用（`call` 动作的产物）。
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptCall {
    pub function: String,
    pub args: Value,
    /// 触发时绑定的 self 实体（如果有）。
    pub self_entity: Option<EntityId>,
}

/// 一个 tick 内规则执行的产出。
#[derive(Debug, Default)]
pub struct TickOutput {
    /// 待脚本层执行的调用。
    pub calls: Vec<ScriptCall>,
    /// 本 tick 触发过的规则 id（含触发次数顺序），调试/录像用。
    pub fired: Vec<String>,
    /// 规则 emit 过的事件副本——控制面事件日志靠它，AI 才看得见因果链。
    pub emitted: Vec<Event>,
}

/// 规则运行时错误。规则系统不写 fallback：错了就停下来把话说清楚。
#[derive(Debug, Clone, PartialEq)]
pub enum RuleError {
    /// 事件级联超深（规则 emit 事件又触发规则……）。
    CascadeOverflow { depth: usize, chain: Vec<String> },
    /// 某条规则的某个动作/条件执行失败。
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

/// 规则引擎。无内部状态：每 tick 拿事件进来、改世界、吐输出。
/// 可 Clone（rules/schema 都是装配期不可变副本）：playtest 派生场景视图要读规则，
/// 而它被 Runtime 这个 GameLogic 装配体私有持有——同时可变借 logic + 不可变借 engine
/// 借用检查器不允许，复制一份只读副本最干净，引擎本身无状态、复制零语义负担。
#[derive(Clone)]
pub struct Engine {
    pub rules: RuleSet,
    pub schema: Schema,
}

/// 规则触发时的绑定上下文。
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

    /// 独立求值一组条件（无事件上下文）。断言系统用它：
    /// 条件全部成立返回 true。路径写法同规则（@实体名 / e3v1 句柄）。
    pub fn check(
        &self,
        world: &World,
        conditions: &[(String, String, Value)],
    ) -> Result<bool, RuleError> {
        for (i, (left, op, right)) in conditions.iter().enumerate() {
            let at = format!("check/{i}");
            if !self.eval_condition(world, Ctx::default(), left, op, right, "<断言>", &at)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// 跑一个 tick：先跑 tick 规则，再按 FIFO 消化事件（含级联）。
    /// `inbox`：本 tick 外部产生的事件（输入、碰撞等）。
    pub fn process_tick(
        &self,
        world: &mut World,
        inbox: Vec<Event>,
    ) -> Result<TickOutput, RuleError> {
        let mut out = TickOutput::default();
        let mut queue: VecDeque<(Event, usize, Vec<String>)> =
            inbox.into_iter().map(|e| (e, 0, Vec::new())).collect();

        // tick 触发的规则（每 tick 一轮，规则按文件顺序）
        for rule in &self.rules.rules {
            if matches!(rule.trigger, Trigger::Tick) {
                self.run_rule_bound(rule, world, Ctx::default(), &mut out, &mut queue, 0, &[])?;
            }
        }

        // 事件队列（FIFO，级联深度受限）
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
                        None => continue, // 碰撞双方不满足组件要求，规则不适用
                    }
                }
                self.run_rule_bound(rule, world, ctx, &mut out, &mut queue, depth, &chain)?;
            }
        }
        Ok(out)
    }

    /// 处理 each 展开后逐实体执行。
    /// 参数是级联执行线程过来的全套上下文（输出/事件队列/深度/调用链），拆开反而藏语义。
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
                        continue; // 前一个实体的动作可能销毁了它
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
        // 条件全部成立才执行
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
        let obj = action.as_object().expect("解析层已校验动作是对象");

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
                // 显式 checked：debug panic / release 回绕会让同一录像在两种构建下分歧
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

        Err(err("动作不属于任何已知类型（解析层应已拦截）".into()))
    }

    /// 递归解析值：字符串若是引用（self./other./@/event.）换成实际值，
    /// 对象/数组逐项处理，其余原样。单独的 "self"/"other" 解析为实体句柄字符串。
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
                // 字符串模板 {"format": "SCORE {}", "args": [路径...]}——
                // 数字状态转屏上文字（Text.content）的正路，规则里别无他法
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

    /// `{}` 逐个换成 resolve 后的 args；个数必须严格对上。
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

    /// 字符串是引用则求值；不是引用返回 None（按字面量处理）。
    fn try_ref(&self, world: &World, ctx: Ctx, s: &str) -> Result<Option<Value>, String> {
        // 实体绑定本身 → 句柄字符串
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

    /// exists 检查：组件或字段是否存在。exists 的本职就是问「在不在」，
    /// 所以实体缺席的所有形态——@名字查无此人（despawn 会注销名字）、句柄代数过期、
    /// self/other 未绑定——都回答 false，不报错。约束只对 exists/!exists 放宽：
    /// 其他操作符引用缺失实体仍然报错（拿不存在的字段去比较是规则真写错了）。
    /// 引用语法本身不合法（既不是 self/other/@名字也不是句柄）照常报错。
    fn ref_exists(&self, world: &World, ctx: Ctx, path: &str) -> Result<bool, String> {
        let (ent_part, rest) = match path.split_once('.') {
            Some((e, r)) => (e, Some(r)),
            None => (path, None),
        };
        // @名字单独处理：entity_ref_opt 对查无此名是报错（其他操作符要的就是这个），
        // 但对 exists 来说「名字不在了」恰恰是要检测的合法答案
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

    /// 解析「实体.字段路径」目标，如 "self.Score.value"。
    fn entity_field(&self, world: &World, ctx: Ctx, target: &str) -> Result<(EntityId, String), String> {
        let (ent_part, path) = target.split_once('.').ok_or_else(|| {
            format!(
                "目标 {target:?} 缺少字段路径。写法: \"self.组件.字段\"，如 \"self.Score.value\""
            )
        })?;
        let id = self.entity_ref(world, ctx, ent_part)?;
        Ok((id, path.to_string()))
    }

    /// 解析实体引用："self" / "other" / "@名字" / "e3v1"。
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

/// 事件 data 等值过滤。
fn filter_matches(filter: &Map<String, Value>, data: &Map<String, Value>) -> bool {
    filter.iter().all(|(k, v)| data.get(k) == Some(v))
}

/// between 绑定：碰撞事件 data 里的 a/b 实体，谁有 comp_a 谁当 self。
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
        // a/b 顺序反着也要能绑定
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
        // 分数不够：不触发
        let out = eng.process_tick(&mut w, vec![]).unwrap();
        assert!(out.fired.is_empty());
        // 分数够：rich 级联触发 on-rich 清零
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

    /// 回归（exists 报错 bug）：实体被 despawn 后名字注销，
    /// `["@名字", "exists"]` 必须求值为 false 而不是报「没有名为 X 的实体」——
    /// 否则所有「检测某东西被摧毁后做事」的规则在它真被摧毁那刻把模拟干停。
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

        // 门还在：exists 命中
        let out = eng.process_tick(&mut w, vec![Event::new("check", json!({}))]).unwrap();
        assert_eq!(out.fired, vec!["door-still-there"]);
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(1));

        // 门没了：不报错，!exists 命中，模拟继续跑
        w.despawn(door).unwrap();
        let out = eng.process_tick(&mut w, vec![Event::new("check", json!({}))]).unwrap();
        assert_eq!(out.fired, vec!["door-destroyed"]);
        assert_eq!(w.get_field(p, "Score.value").unwrap(), &json!(2));
        // 再跑一轮也照样平稳（不是只豁免一次）
        eng.process_tick(&mut w, vec![Event::new("check", json!({}))]).unwrap();
    }

    /// exists 的实体缺席各形态：带字段路径的引用、过期句柄、未绑定的 self/other，
    /// 全部回答 false 不报错。
    #[test]
    fn exists_handles_all_missing_entity_forms() {
        let eng = engine(json!({"rules": []}));
        let (mut w, _, _) = world_with_player_and_coin();
        let door = w.spawn_named("door").unwrap();
        w.set_component(door, "Coin", json!({"value": 1})).unwrap();
        let handle = door.to_string();
        w.despawn(door).unwrap();

        let f = json!(false);
        // @名字 + 组件路径 / 字段路径：实体不在 → false
        assert!(eng.check(&w, &[("@door".into(), "!exists".into(), f.clone())]).unwrap());
        assert!(!eng.check(&w, &[("@door.Coin".into(), "exists".into(), f.clone())]).unwrap());
        assert!(!eng.check(&w, &[("@door.Coin.value".into(), "exists".into(), f.clone())]).unwrap());
        // 过期句柄（代数已翻篇）→ false
        assert!(!eng.check(&w, &[(handle, "exists".into(), f.clone())]).unwrap());
        // self/other 在无绑定上下文里 → false
        assert!(!eng.check(&w, &[("self".into(), "exists".into(), f.clone())]).unwrap());
        assert!(!eng.check(&w, &[("other.Coin".into(), "exists".into(), f.clone())]).unwrap());
        // 活实体上缺组件/缺字段仍是 false（原有语义不变）
        assert!(!eng.check(&w, &[("@player.Coin".into(), "exists".into(), f.clone())]).unwrap());
        assert!(eng.check(&w, &[("@player.Score".into(), "exists".into(), f)]).unwrap());
    }

    /// 缺席豁免只给 exists/!exists：其他操作符读缺失实体仍然显式报错——
    /// 那是规则真写错了，静默放过会把 bug 藏起来。
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
        // 引用语法本身不合法也照常报错（exists 也不豁免语法错误）
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
