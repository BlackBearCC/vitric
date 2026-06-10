use serde_json::{Map, Value};

use vitric_data::ValidationReport;

/// 运行时事件：规则的燃料。内建事件有 `input`（{"action","phase"}）、
/// `collision`（{"a","b"}，实体句柄字符串）；规则用 `emit` 可以造任意自定义事件。
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

/// 触发器。
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    /// 每 tick 触发一次；配 `each` 则按实体逐个触发。
    Tick,
    /// 事件触发。`filter`：事件 data 字段的等值过滤。
    /// `between`：碰撞语法糖——绑定 `self`=有第一个组件的实体、`other`=有第二个组件的实体。
    Event {
        name: String,
        filter: Map<String, Value>,
        between: Option<(String, String)>,
    },
}

/// 一条规则。
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub trigger: Trigger,
    /// 配合 Tick 触发：对拥有这些组件的每个实体各跑一次，绑定为 `self`。
    pub each: Option<Vec<String>>,
    /// 条件三元组 [左, 操作符, 右]，全部成立才执行。
    pub conditions: Vec<(String, String, Value)>,
    /// 动作，按序执行。
    pub actions: Vec<Value>,
}

/// 一个项目的全部规则（解析+静态校验后）。
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
}

const OPS: &[&str] = &["==", "!=", "<", "<=", ">", ">=", "exists", "!exists"];
const ACTION_KINDS: &[&str] = &["set", "add", "spawn", "despawn", "emit", "call"];

impl RuleSet {
    /// 解析规则文件。格式: {"rules": [ {...}, ... ]}
    /// 所有结构问题一次报全。
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
            // id 查重在结构解析之前做：规则其他部分写错不该掩盖重复 id
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
            let p = parts.expect("已校验");
            let left = p[0].as_str().expect("已校验").to_string();
            let op = p[1].as_str().expect("已校验").to_string();
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
    Some(Rule { id, trigger: trigger.expect("已检查"), each, conditions, actions })
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
}
