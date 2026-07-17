use serde_json::{Map, Value};

use vitric_data::ValidationReport;

/// Runtime event: the fuel of rules. Built-in events include `input` ({"action","phase"}),
/// `collision` ({"a","b"}, entity handle strings); rules can use `emit` to produce arbitrary custom events.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub name: String,
    pub data: Map<String, Value>,
}

impl Event {
    pub fn new(name: &str, data: Value) -> Event {
        Event {
            name: name.to_string(),
            data: data.as_object().cloned().unwrap_or_default(),
        }
    }
}

/// Trigger.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    /// Fires once per tick; with `each`, fires once per entity.
    Tick,
    /// Event-triggered. `filter`: equality filter on event data fields.
    /// `between`: collision syntax sugar — binds `self` to the entity with the first component, `other` to the entity with the second component.
    Event {
        name: String,
        filter: Map<String, Value>,
        between: Option<(String, String)>,
    },
}

/// A single rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub trigger: Trigger,
    /// Used with Tick trigger: runs once for each entity that has these components, bound as `self`.
    pub each: Option<Vec<String>>,
    /// Condition triples [left, operator, right]; all must hold to execute.
    pub conditions: Vec<(String, String, Value)>,
    /// Actions, executed in order.
    pub actions: Vec<Value>,
}

/// All rules of a project (after parsing + static validation).
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
}

/// An "injectable action" introspected from rules: action name + the phases it appears with in rules.
/// This is the authoritative vocabulary of "what this game can do" — describe (control plane) and SceneView (playtest) both start from it,
/// aligned to the same source of truth. It lives in vitric-rules because it is pure introspection over the rule set (rules are the source of actions),
/// and consumers shouldn't each re-implement the rule-scanning logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputAction {
    /// Input action name (the value of the action field in an input trigger's filter).
    pub action: String,
    /// Phases this action appears with in rules ("pressed" / "released"), deduplicated in first-occurrence order.
    pub phases: Vec<String>,
}

/// Derive the "input action vocabulary" from a rule set: scan all `input` event triggers, collect distinct actions,
/// each with the phases it appeared with.
///
/// Order is deterministic (output must be reproducible):
/// - actions are recorded in the order of their first appearance within the rule set (later occurrences don't reorder);
/// - phases for each action are also collected in first-occurrence order, deduplicated.
///
/// An input trigger without a phase field (no phase in its filter) contributes only the action name, no phase —
/// it doesn't care about pressed/released, so phases is left empty for consumers to fill in as needed (SceneView defaults to
/// both {pressed, released}). Non-input triggers (tick / collision / custom events) are never counted.
pub fn input_actions(rules: &RuleSet) -> Vec<InputAction> {
    let mut out: Vec<InputAction> = Vec::new();
    for rule in &rules.rules {
        let Trigger::Event { name, filter, .. } = &rule.trigger else {
            continue; // only look at input event triggers
        };
        if name != "input" {
            continue;
        }
        let Some(action) = filter.get("action").and_then(|v| v.as_str()) else {
            continue; // an input trigger without a specific action name contributes nothing to the action vocabulary
        };
        // phase is optional: collected if present, skipped if absent (the action is still recorded; phases may be empty)
        let phase = filter.get("phase").and_then(|v| v.as_str());
        match out.iter_mut().find(|a| a.action == action) {
            Some(existing) => {
                if let Some(p) = phase {
                    if !existing.phases.iter().any(|x| x == p) {
                        existing.phases.push(p.to_string());
                    }
                }
            }
            None => {
                let phases = phase.map(|p| vec![p.to_string()]).unwrap_or_default();
                out.push(InputAction { action: action.to_string(), phases });
            }
        }
    }
    out
}

const OPS: &[&str] = &["==", "!=", "<", "<=", ">", ">=", "exists", "!exists"];
const ACTION_KINDS: &[&str] = &["set", "add", "spawn", "despawn", "emit", "call"];

impl RuleSet {
    /// Parse a rule file. Format: {"rules": [ {...}, ... ]}
    /// All structural problems are reported at once.
    pub fn parse(doc: &Value, file: &str) -> Result<RuleSet, ValidationReport> {
        let mut report = ValidationReport::default();
        let mut set = RuleSet::default();
        let Some(rules) = doc.get("rules").and_then(|v| v.as_array()) else {
            report.push(
                "VR001",
                format!("{file}#/rules"),
                "规则文件缺少 rules 数组",
                "顶层结构: {\"rules\": [ {\"id\":..., \"on\":..., \"do\":[...]} ]}",
            );
            return Err(report);
        };
        let mut seen_ids: Vec<String> = Vec::new();
        for (i, rdoc) in rules.iter().enumerate() {
            let rpath = format!("{file}#/rules/{i}");
            // id duplicate check happens before structural parsing: errors elsewhere in the rule shouldn't mask a duplicate id
            if let Some(id) = rdoc.get("id").and_then(|v| v.as_str()) {
                if seen_ids.iter().any(|s| s == id) {
                    report.push(
                        "VR002",
                        format!("{rpath}/id"),
                        format!("规则 id {id:?} 重复"),
                        "规则 id 全局唯一，错误信息和调试都靠它定位",
                    );
                }
                seen_ids.push(id.to_string());
            }
            if let Some(rule) = parse_rule(rdoc, &rpath, &mut report) {
                set.rules.push(rule);
            }
        }
        report.into_result(set)
    }
}

fn parse_rule(doc: &Value, rpath: &str, report: &mut ValidationReport) -> Option<Rule> {
    let mut broken = false;

    let id = match doc.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            report.push(
                "VR003",
                format!("{rpath}/id"),
                "规则缺少 id（非空文本）",
                "给规则起个能说明意图的 id，如 \"collect-coin\"",
            );
            broken = true;
            format!("<未命名规则 {rpath}>")
        }
    };

    // on
    let trigger = match doc.get("on") {
        Some(Value::String(s)) if s == "tick" => Some(Trigger::Tick),
        Some(Value::Object(o)) => {
            let name = o.get("event").and_then(|v| v.as_str());
            match name {
                Some(name) => {
                    let filter = o
                        .get("filter")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    let between = match o.get("between") {
                        None => None,
                        Some(Value::Array(arr)) if arr.len() == 2 => {
                            let a = arr[0].as_str();
                            let b = arr[1].as_str();
                            match (a, b) {
                                (Some(a), Some(b)) => Some((a.to_string(), b.to_string())),
                                _ => {
                                    report.push(
                                        "VR004",
                                        format!("{rpath}/on/between"),
                                        "between 必须是两个组件名",
                                        "写法: \"between\": [\"Player\", \"Coin\"]",
                                    );
                                    return None;
                                }
                            }
                        }
                        Some(_) => {
                            report.push(
                                "VR004",
                                format!("{rpath}/on/between"),
                                "between 必须是两个组件名的数组",
                                "写法: \"between\": [\"Player\", \"Coin\"]",
                            );
                            return None;
                        }
                    };
                    if between.is_some() && name != "collision" {
                        report.push(
                            "VR005",
                            format!("{rpath}/on"),
                            "between 只能配 collision 事件",
                            "其他事件用 filter 做字段过滤",
                        );
                        broken = true;
                    }
                    Some(Trigger::Event { name: name.to_string(), filter, between })
                }
                None => {
                    report.push(
                        "VR006",
                        format!("{rpath}/on"),
                        "on 对象缺少 event 字段",
                        "写法: \"on\": {\"event\": \"collision\"} 或 \"on\": \"tick\"",
                    );
                    None
                }
            }
        }
        _ => {
            report.push(
                "VR006",
                format!("{rpath}/on"),
                "规则缺少触发器 on",
                "写法: \"on\": \"tick\" 或 \"on\": {\"event\": \"事件名\"}",
            );
            None
        }
    };

    // each
    let each = match doc.get("each") {
        None => None,
        Some(Value::Array(arr)) => {
            let comps: Option<Vec<String>> =
                arr.iter().map(|v| v.as_str().map(String::from)).collect();
            match comps {
                Some(c) if !c.is_empty() => Some(c),
                _ => {
                    report.push(
                        "VR007",
                        format!("{rpath}/each"),
                        "each 必须是非空组件名数组",
                        "写法: \"each\": [\"Position\", \"Velocity\"]",
                    );
                    broken = true;
                    None
                }
            }
        }
        Some(_) => {
            report.push(
                "VR007",
                format!("{rpath}/each"),
                "each 必须是组件名数组",
                "写法: \"each\": [\"Position\", \"Velocity\"]",
            );
            broken = true;
            None
        }
    };

    // if
    let mut conditions = Vec::new();
    if let Some(ifs) = doc.get("if") {
        let Some(arr) = ifs.as_array() else {
            report.push(
                "VR008",
                format!("{rpath}/if"),
                "if 必须是条件数组",
                "每个条件是 [左, 操作符, 右] 三元组，如 [\"self.Health.hp\", \"<=\", 0]",
            );
            return None;
        };
        for (ci, cond) in arr.iter().enumerate() {
            let cpath = format!("{rpath}/if/{ci}");
            let parts = cond.as_array();
            let valid = parts.is_some_and(|p| {
                (p.len() == 3 && p[0].is_string() && p[1].is_string())
                    || (p.len() == 2 && p[0].is_string() && p[1].is_string())
            });
            if !valid {
                report.push(
                    "VR008",
                    &cpath,
                    "条件必须是 [路径, 操作符, 值] 三元组（exists/!exists 可省略第三项）",
                    "例: [\"self.Health.hp\", \"<\", 10] 或 [\"self.Shield\", \"exists\"]",
                );
                broken = true;
                continue;
            }
            let p = parts.expect("validated");
            let left = p[0].as_str().expect("validated").to_string();
            let op = p[1].as_str().expect("validated").to_string();
            if !OPS.contains(&op.as_str()) {
                report.push(
                    "VR009",
                    &cpath,
                    format!("未知操作符 {op:?}"),
                    format!("可用操作符: [{}]", OPS.join(", ")),
                );
                broken = true;
                continue;
            }
            let right = p.get(2).cloned().unwrap_or(Value::Null);
            conditions.push((left, op, right));
        }
    }

    // do
    let mut actions = Vec::new();
    match doc.get("do").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => {
            for (ai, action) in arr.iter().enumerate() {
                let apath = format!("{rpath}/do/{ai}");
                let Some(obj) = action.as_object() else {
                    report.push(
                        "VR010",
                        &apath,
                        "动作必须是对象",
                        "例: {\"set\": \"self.Health.hp\", \"to\": 100}",
                    );
                    broken = true;
                    continue;
                };
                let kind = ACTION_KINDS.iter().find(|k| obj.contains_key(**k));
                match kind {
                    Some(_) => actions.push(action.clone()),
                    None => {
                        report.push(
                            "VR010",
                            &apath,
                            "动作不属于任何已知类型",
                            format!("动作类型: [{}]，按对象的键识别", ACTION_KINDS.join(", ")),
                        );
                        broken = true;
                    }
                }
            }
        }
        _ => {
            report.push(
                "VR011",
                format!("{rpath}/do"),
                "规则缺少非空 do 数组",
                "没有动作的规则什么也不会发生；至少写一个动作",
            );
            broken = true;
        }
    }

    if broken || trigger.is_none() {
        return None;
    }
    Some(Rule { id, trigger: trigger.expect("checked above"), each, conditions, actions })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_good_rules() {
        let set = RuleSet::parse(
            &json!({"rules": [
                {"id": "gravity", "on": "tick", "each": ["Position", "Velocity"],
                 "do": [{"add": "self.Velocity.y", "by": -9.8}]},
                {"id": "collect", "on": {"event": "collision", "between": ["Player", "Coin"]},
                 "if": [["other.Coin.value", ">", 0]],
                 "do": [{"despawn": "other"}]},
                {"id": "quit", "on": {"event": "input", "filter": {"action": "quit", "phase": "pressed"}},
                 "do": [{"emit": "game-over", "data": {}}]}
            ]}),
            "rules/game.json",
        )
        .unwrap();
        assert_eq!(set.rules.len(), 3);
        assert_eq!(set.rules[0].trigger, Trigger::Tick);
        assert!(matches!(&set.rules[1].trigger, Trigger::Event { between: Some(_), .. }));
    }

    #[test]
    fn parse_reports_all_problems() {
        let err = RuleSet::parse(
            &json!({"rules": [
                {"on": "tick", "do": [{"set": "x", "to": 1}]},
                {"id": "a", "on": "tick", "do": [{"fly": "moon"}]},
                {"id": "a", "on": "tick", "do": [{"set": "x", "to": 1}],
                 "if": [["x", "~=", 1]]},
                {"id": "b", "on": {"event": "input", "between": ["A", "B"]},
                 "do": [{"set": "x", "to": 1}]}
            ]}),
            "rules/game.json",
        )
        .unwrap_err();
        let codes: Vec<&str> = err.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VR003"), "缺 id: {err}");
        assert!(codes.contains(&"VR010"), "未知动作: {err}");
        assert!(codes.contains(&"VR009"), "未知操作符: {err}");
        assert!(codes.contains(&"VR002"), "id 重复: {err}");
        assert!(codes.contains(&"VR005"), "between 配错事件: {err}");
    }

    #[test]
    fn input_actions_collects_distinct_actions_and_phases() {
        // left has pressed+released across two rules → one action with two phases; right only has pressed.
        // tick / collision / custom event triggers are never counted.
        let set = RuleSet::parse(
            &json!({"rules": [
                {"id": "left-go", "on": {"event": "input", "filter": {"action": "left", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.x", "to": -8}]},
                {"id": "left-stop", "on": {"event": "input", "filter": {"action": "left", "phase": "released"}},
                 "do": [{"set": "@hero.Velocity.x", "to": 0}]},
                {"id": "right-go", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.x", "to": 8}]},
                {"id": "tickrule", "on": "tick", "do": [{"emit": "noop", "data": {}}]},
                {"id": "hitrule", "on": {"event": "collision", "between": ["A", "B"]},
                 "do": [{"emit": "boom", "data": {}}]}
            ]}),
            "rules/game.json",
        )
        .unwrap();
        let acts = input_actions(&set);
        // distinct actions in first-seen order: left precedes right; non-input triggers contribute nothing
        assert_eq!(acts.len(), 2, "only left/right two actions: {acts:?}");
        assert_eq!(acts[0].action, "left");
        assert_eq!(acts[0].phases, vec!["pressed".to_string(), "released".to_string()]);
        assert_eq!(acts[1].action, "right");
        assert_eq!(acts[1].phases, vec!["pressed".to_string()]);
    }

    #[test]
    fn input_actions_is_deterministic() {
        let set = RuleSet::parse(
            &json!({"rules": [
                {"id": "a", "on": {"event": "input", "filter": {"action": "fire", "phase": "pressed"}},
                 "do": [{"emit": "shot", "data": {}}]},
                {"id": "b", "on": {"event": "input", "filter": {"action": "jump", "phase": "pressed"}},
                 "do": [{"emit": "hop", "data": {}}]}
            ]}),
            "rules/game.json",
        )
        .unwrap();
        assert_eq!(input_actions(&set), input_actions(&set));
    }

    #[test]
    fn input_actions_action_without_phase_has_empty_phases() {
        // an input trigger that declares only action, no phase (indifferent to both phases): the action is still recorded once, with empty phases.
        let set = RuleSet::parse(
            &json!({"rules": [
                {"id": "any", "on": {"event": "input", "filter": {"action": "menu"}},
                 "do": [{"emit": "open", "data": {}}]}
            ]}),
            "rules/game.json",
        )
        .unwrap();
        let acts = input_actions(&set);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].action, "menu");
        assert!(acts[0].phases.is_empty(), "没声明 phase → phases 空: {acts:?}");
    }

    #[test]
    fn input_actions_ignores_non_input_and_filterless_input() {
        // no input triggers at all → empty; an input trigger whose filter lacks an action field also contributes nothing.
        let set = RuleSet::parse(
            &json!({"rules": [
                {"id": "t", "on": "tick", "do": [{"emit": "x", "data": {}}]},
                {"id": "noaction", "on": {"event": "input", "filter": {"phase": "pressed"}},
                 "do": [{"emit": "y", "data": {}}]}
            ]}),
            "rules/game.json",
        )
        .unwrap();
        assert!(input_actions(&set).is_empty());
    }
}
