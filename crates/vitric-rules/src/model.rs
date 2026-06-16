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

/// 规则自省出来的一个「可注入动作」：动作名 + 它在规则里出现过的 phase。
/// 这是「这局能干啥」的权威词汇——describe（控制面）和 SceneView（试玩）都从它出发，
/// 两边对齐到同一个真相。放在 vitric-rules 因为它纯是对规则集的内省（规则是动作的来源），
/// 不该让消费方各自重抄一遍扫规则的逻辑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputAction {
    /// 输入动作名（input 触发器 filter 里的 action 字段值）。
    pub action: String,
    /// 这个动作在规则里出现过的 phase（"pressed" / "released"），按首次出现序去重。
    pub phases: Vec<String>,
}

/// 从规则集派生「输入动作词汇」：扫所有 `input` 事件触发器，收 distinct action，
/// 每个动作带上它出现过的 phase。
///
/// 顺序确定（输出必须可复现）：
/// - action 按规则在规则集里的出现序首次见到时记下（后面再出现不挪位）；
/// - 每个 action 的 phases 同样按首次出现序去重收集。
///
/// 不带 phase 字段的 input 触发器（filter 里没写 phase）只贡献动作名、不贡献 phase——
/// 它对 pressed/released 都不挑，phases 留空由消费方按需补全（SceneView 默认补
/// {pressed, released} 两相）。非 input 触发器（tick / collision / 自定义事件）一概不算。
pub fn input_actions(rules: &RuleSet) -> Vec<InputAction> {
    let mut out: Vec<InputAction> = Vec::new();
    for rule in &rules.rules {
        let Trigger::Event { name, filter, .. } = &rule.trigger else {
            continue; // 只看 input 事件触发器
        };
        if name != "input" {
            continue;
        }
        let Some(action) = filter.get("action").and_then(|v| v.as_str()) else {
            continue; // input 触发器没声明具体 action 名 = 不贡献动作词汇
        };
        // phase 是可选的：写了就收，没写不收（动作仍记一份，phases 可能为空）
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

    #[test]
    fn input_actions_collects_distinct_actions_and_phases() {
        // left 有 pressed+released 两条规则 → 一个动作两 phase；right 只有 pressed。
        // tick / collision / 自定义事件触发器一概不算。
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
        // distinct action 按出现序：left 先于 right；非 input 触发器不贡献
        assert_eq!(acts.len(), 2, "只 left/right 两个动作: {acts:?}");
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
        // input 触发器只声明 action、没写 phase（对两相都不挑）：动作仍记一份，phases 为空。
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
        // 没有任何 input 触发器 → 空；input 触发器但 filter 没 action 字段也不贡献。
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
