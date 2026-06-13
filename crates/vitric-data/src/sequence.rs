//! 序列（时间轴）静态数据：声明式动作轨道 + 校验。
//!
//! 定位：引擎的通用时间轴原语，对标 Unity Timeline / Godot AnimationPlayer——
//! 一条按相对 tick 推进的动作轨道，在指定时刻发动**引擎已有的通用动词**。
//! 引擎里没有"过场""漫画页""卡牌"这类题材专属概念；漫画过场是用这套积木在
//! 游戏项目里拼出来的**用法**（见 examples/intro 的 sequences/opening.json）。
//!
//! 文件形态（`sequences/<名>.json`）：
//! ```json
//! {
//!   "id": "intro",
//!   "steps": [
//!     { "at": 0,   "do": { "spawn": { "name": "panel", "components": { ... } } } },
//!     { "at": 0,   "do": { "tween": { "target": "panel", "field": "Sprite.color_a",
//!                                       "from": 0, "to": 1, "duration": 30 } } },
//!     { "at": 30,  "do": { "set": "@subtitle.Text.content", "to": "他走进长廊" } },
//!     { "at": 150, "do": { "wait": "player-confirm" } },
//!     { "at": 151, "do": { "emit": "intro-done" } }
//!   ]
//! }
//! ```
//!
//! `at` 是相对序列起跑点的 tick（同一序列任何时刻起跑都从自己的 t=0 放）。
//! 动作集 v1 固定（不图灵完备、不嵌脚本），全部镜像引擎已有动词：
//! `tween` / `set` / `spawn` / `despawn` / `emit` / `sound` / `wait`。
//! 切场景不内置——序列靠 `emit "load-scene"` 解耦，由项目规则接去 load-scene。

use serde_json::{Map, Value};

use crate::{Schema, ValidationReport};

/// 序列里固定动作集（v1）。每个都镜像引擎已有通用动词，不新造语义。
pub const SEQ_ACTION_KINDS: &[&str] =
    &["tween", "set", "spawn", "despawn", "emit", "sound", "wait"];

/// 一个序列里的一条动作（已解析为引擎认得的形态）。
#[derive(Debug, Clone)]
pub struct SeqStep {
    /// 相对序列起跑点的 tick（单调不减）。
    pub at: u64,
    /// 动作类型名（在 [`SEQ_ACTION_KINDS`] 内）。
    pub kind: String,
    /// 动作原始 JSON 对象（执行端按 kind 取字段）。
    pub action: Value,
}

/// 一条序列（解析 + 校验后的静态轨道）。运行时一个 `Sequence` 组件引用它的名字、
/// 只持有最小播放状态（游标 + 起跑 tick + barrier 标志），静态轨道不进任何快照。
#[derive(Debug, Clone)]
pub struct Sequence {
    /// 序列名（清单/组件 `track` 字段按它引用，默认取文件 id）。
    pub id: String,
    /// 来源文件相对路径（错误定位用）。
    pub file: String,
    /// 有序动作条目（`at` 单调不减）。
    pub steps: Vec<SeqStep>,
}

impl Sequence {
    /// 解析 + 按 schema 校验一条序列文件。
    /// 校验项：`at` 单调不减、动作名在固定集合内、动作字段过 schema、
    /// 引用的实体/贴图/事件名形态合法。所有问题一次报全（不在第一个错误就停）。
    pub fn parse(doc: &Value, file: &str, schema: &Schema) -> Result<Sequence, ValidationReport> {
        let mut report = ValidationReport::default();
        let id = doc
            .get("id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| derive_name(file));
        let steps = parse_steps(doc, file, schema, &mut report);
        report.into_result(Sequence { id, file: file.to_string(), steps })
    }
}

/// 从文件相对路径取默认序列名（`sequences/intro.json` → `intro`）。
fn derive_name(file: &str) -> String {
    file.rsplit('/')
        .next()
        .unwrap_or(file)
        .strip_suffix(".json")
        .unwrap_or(file)
        .to_string()
}

fn parse_steps(doc: &Value, file: &str, schema: &Schema, report: &mut ValidationReport) -> Vec<SeqStep> {
    let Some(steps) = doc.get("steps").and_then(|v| v.as_array()) else {
        report.push(
            "VD060",
            format!("{file}#/steps"),
            "序列缺少 steps 数组",
            "顶层结构: {\"id\": \"...\", \"steps\": [ {\"at\": 0, \"do\": {...}} ]}",
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(steps.len());
    let mut last_at: Option<u64> = None;
    for (i, step) in steps.iter().enumerate() {
        let spath = format!("{file}#/steps/{i}");
        // at：非负整数，单调不减（序列是有序轨道，乱序无法定义播放顺序）
        let at = match step.get("at").and_then(|v| v.as_i64()) {
            Some(a) if a >= 0 => a as u64,
            _ => {
                report.push(
                    "VD061",
                    format!("{spath}/at"),
                    "条目缺少 at（非负整数：相对序列起跑点的 tick）",
                    "写法: {\"at\": 0, \"do\": {...}}，at 从 0 起、按时间单调不减",
                );
                continue;
            }
        };
        if let Some(prev) = last_at {
            if at < prev {
                report.push(
                    "VD062",
                    format!("{spath}/at"),
                    format!("at={at} 小于上一条的 at={prev}（序列条目必须按 at 单调不减）"),
                    "把条目按 at 从小到大排好；同一 tick 多个动作 at 写相同值即可",
                );
            }
        }
        last_at = Some(at);
        // do：动作对象，类型在固定集合内，字段过对应 schema
        let Some(action) = step.get("do") else {
            report.push(
                "VD063",
                format!("{spath}/do"),
                "条目缺少 do（动作对象）",
                format!("动作类型: [{}]", SEQ_ACTION_KINDS.join(", ")),
            );
            continue;
        };
        let Some(kind) = validate_action(action, &format!("{spath}/do"), schema, report) else {
            continue;
        };
        out.push(SeqStep { at, kind, action: action.clone() });
    }
    out
}

/// 校验一条动作对象，返回它的类型名。未知类型 / 字段非法都进 report。
fn validate_action(
    action: &Value,
    path: &str,
    schema: &Schema,
    report: &mut ValidationReport,
) -> Option<String> {
    let Some(obj) = action.as_object() else {
        report.push(
            "VD063",
            path,
            "动作必须是对象",
            format!("写法按类型定，类型: [{}]", SEQ_ACTION_KINDS.join(", ")),
        );
        return None;
    };
    let Some(kind) = SEQ_ACTION_KINDS.iter().find(|k| obj.contains_key(**k)) else {
        report.push(
            "VD064",
            path,
            format!("动作不属于任何已知类型（键: [{}]）", obj.keys().cloned().collect::<Vec<_>>().join(", ")),
            format!("序列动作类型: [{}]，按对象的键识别", SEQ_ACTION_KINDS.join(", ")),
        );
        return None;
    };
    match *kind {
        "tween" => validate_tween(obj, path, report),
        "set" => validate_set(obj, path, report),
        "spawn" => validate_spawn(obj, path, schema, report),
        "despawn" => validate_despawn(obj, path, report),
        "emit" => validate_emit(obj, path, report),
        "sound" => validate_sound(obj, path, report),
        "wait" => validate_wait(obj, path, report),
        _ => unreachable!("kind 来自 SEQ_ACTION_KINDS"),
    }
    Some(kind.to_string())
}

/// tween：起一个补间组件（复用已落地的 Tween）。镜头推拉/淡入淡出/位移缩放变色全靠它。
fn validate_tween(obj: &Map<String, Value>, path: &str, report: &mut ValidationReport) {
    let spec = &obj["tween"];
    let Some(t) = spec.as_object() else {
        report.push("VD065", path, "tween 值必须是对象", tween_hint());
        return;
    };
    require_text(t, "target", path, report, &|| tween_hint());
    let field = require_text(t, "field", path, report, &|| tween_hint());
    if let Some(f) = field {
        if !f.contains('.') {
            report.push(
                "VD065",
                format!("{path}/field"),
                format!("field {f:?} 缺少字段路径，写法 \"组件.字段\"（如 \"Position.x\"）"),
                tween_hint(),
            );
        }
    }
    require_number(t, "from", path, report, &|| tween_hint());
    require_number(t, "to", path, report, &|| tween_hint());
    match t.get("duration").and_then(|v| v.as_i64()) {
        Some(d) if d >= 1 => {}
        _ => report.push(
            "VD065",
            format!("{path}/duration"),
            "tween 缺少 duration（整数 ≥ 1：补间时长 tick 数）",
            tween_hint(),
        ),
    }
    // ease 可选，给了就必须是文本（具体曲线名的合法性由补间系统在运行时报，
    // 这里只挡明显的类型错——和补间组件本身的解析口径一致）
    if let Some(e) = t.get("ease") {
        if !e.is_string() {
            report.push("VD065", format!("{path}/ease"), "ease 必须是文本（缓动曲线名）", tween_hint());
        }
    }
}

fn tween_hint() -> String {
    "写法: {\"tween\": {\"target\": \"实体名\", \"field\": \"Sprite.color_a\", \"from\": 0, \"to\": 1, \"duration\": 30, \"ease\": \"ease-out\"}}".to_string()
}

/// set：瞬时设字段（镜像规则 set）。target = "实体.字段路径" 引用，to = 值。
fn validate_set(obj: &Map<String, Value>, path: &str, report: &mut ValidationReport) {
    match obj.get("set").and_then(|v| v.as_str()) {
        Some(t) if t.contains('.') => {}
        Some(_) => report.push(
            "VD065",
            format!("{path}/set"),
            "set 目标缺少字段路径，写法 \"@实体名.组件.字段\"（如 \"@subtitle.Text.content\"）",
            "写法: {\"set\": \"@subtitle.Text.content\", \"to\": \"...\"}",
        ),
        None => report.push(
            "VD065",
            format!("{path}/set"),
            "set 值必须是目标字段路径文本",
            "写法: {\"set\": \"@subtitle.Text.content\", \"to\": \"...\"}",
        ),
    }
    if !obj.contains_key("to") {
        report.push(
            "VD065",
            format!("{path}/to"),
            "set 动作缺少 to（要设的值）",
            "写法: {\"set\": \"@e.Comp.field\", \"to\": <值>}",
        );
    }
}

/// spawn：生成实体（镜像规则 spawn）。组件值过 schema。
fn validate_spawn(obj: &Map<String, Value>, path: &str, schema: &Schema, report: &mut ValidationReport) {
    let Some(comps) = obj.get("spawn").and_then(|s| s.get("components")).and_then(|v| v.as_object())
    else {
        report.push(
            "VD065",
            format!("{path}/spawn"),
            "spawn 缺少 components 对象",
            "写法: {\"spawn\": {\"name\": \"panel\", \"components\": {\"Sprite\": {...}}}}",
        );
        return;
    };
    for (cname, cval) in comps {
        let cpath = format!("{path}/spawn/components/{cname}");
        let Some(cschema) = schema.component(cname) else {
            report.push(
                "VD005",
                &cpath,
                format!("未知组件 {cname:?}"),
                format!(
                    "schema 里的组件: [{}]",
                    schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            );
            continue;
        };
        // 引用值（@/event. 等运行时路径）跳过 schema 严格校验，与规则 spawn 同口径：
        // 只在没有任何引用字符串时整体过 schema（含引用的留给运行时解析后再校验）
        if !contains_ref(cval) {
            cschema.normalize(cval, &cpath, report);
        }
    }
}

/// despawn：销毁实体（镜像规则 despawn）。值 = 实体引用文本。
fn validate_despawn(obj: &Map<String, Value>, path: &str, report: &mut ValidationReport) {
    if obj.get("despawn").and_then(|v| v.as_str()).is_none() {
        report.push(
            "VD065",
            format!("{path}/despawn"),
            "despawn 值必须是实体引用文本（实体名或 @名字 或句柄）",
            "写法: {\"despawn\": \"panel\"}",
        );
    }
}

/// emit：发事件让规则接龙（序列与场景解耦的正门）。值 = 事件名。
fn validate_emit(obj: &Map<String, Value>, path: &str, report: &mut ValidationReport) {
    match obj.get("emit").and_then(|v| v.as_str()) {
        Some(name) if !name.is_empty() => {}
        _ => report.push(
            "VD065",
            format!("{path}/emit"),
            "emit 值必须是非空事件名文本",
            "写法: {\"emit\": \"intro-done\", \"data\": {...}}；切场景就 emit \"load-scene\"",
        ),
    }
}

/// sound：播音效（镜像音频；运行时翻成 play-sound 事件）。值 = 音效文件名。
fn validate_sound(obj: &Map<String, Value>, path: &str, report: &mut ValidationReport) {
    match obj.get("sound").and_then(|v| v.as_str()) {
        Some(name) if !name.is_empty() => {}
        _ => report.push(
            "VD065",
            format!("{path}/sound"),
            "sound 值必须是非空音效文件名（sounds/ 目录内）",
            "写法: {\"sound\": \"chime.wav\", \"volume\": 0.6}",
        ),
    }
}

/// wait：barrier——游标停住直到某命名事件触发 / skip 输入到达。值 = 等待的事件名。
fn validate_wait(obj: &Map<String, Value>, path: &str, report: &mut ValidationReport) {
    match obj.get("wait").and_then(|v| v.as_str()) {
        Some(name) if !name.is_empty() => {}
        _ => report.push(
            "VD065",
            format!("{path}/wait"),
            "wait 值必须是非空事件名文本（要等的命名事件，或玩家输入对应的事件名）",
            "写法: {\"wait\": \"player-confirm\"}；序列停住直到该事件出现或 skip 跳过",
        ),
    }
}

fn require_text<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    path: &str,
    report: &mut ValidationReport,
    hint: &dyn Fn() -> String,
) -> Option<&'a str> {
    match obj.get(key).and_then(|v| v.as_str()) {
        Some(s) => Some(s),
        None => {
            report.push("VD065", format!("{path}/{key}"), format!("缺少 {key}（文本）"), hint());
            None
        }
    }
}

fn require_number(
    obj: &Map<String, Value>,
    key: &str,
    path: &str,
    report: &mut ValidationReport,
    hint: &dyn Fn() -> String,
) {
    if obj.get(key).and_then(|v| v.as_f64()).is_none() {
        report.push("VD065", format!("{path}/{key}"), format!("缺少 {key}（数字）"), hint());
    }
}

/// 组件值里是否含运行时引用字符串（@/self./other./event. 开头）——含引用的留给
/// 运行时解析后再校验，静态 schema 校验对它放行（与规则 spawn 同口径）。
fn contains_ref(v: &Value) -> bool {
    match v {
        Value::String(s) => {
            s.starts_with('@')
                || s.starts_with("self.")
                || s.starts_with("other.")
                || s.starts_with("event.")
        }
        Value::Array(a) => a.iter().any(contains_ref),
        Value::Object(o) => o.values().any(contains_ref),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Schema {
        Schema::parse(
            &json!({"components": {
                "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Sprite": {"fields": {"w": {"type":"number"}, "h": {"type":"number"},
                                       "color": {"type":"text", "default":"#ffffff"}}},
                "Text": {"fields": {"content": {"type":"text", "default":""},
                                     "reveal": {"type":"number", "default": 1}}}
            }}),
            "schema.json",
        )
        .unwrap()
    }

    #[test]
    fn parses_a_well_formed_sequence() {
        let seq = Sequence::parse(
            &json!({"id": "intro", "steps": [
                {"at": 0, "do": {"spawn": {"name": "panel", "components": {"Sprite": {"w": 1, "h": 1}}}}},
                {"at": 0, "do": {"tween": {"target": "panel", "field": "Position.x", "from": 0, "to": 5, "duration": 30, "ease": "ease-out"}}},
                {"at": 30, "do": {"set": "@panel.Text.content", "to": "hi"}},
                {"at": 30, "do": {"sound": "chime.wav", "volume": 0.5}},
                {"at": 150, "do": {"wait": "player-confirm"}},
                {"at": 151, "do": {"despawn": "panel"}},
                {"at": 151, "do": {"emit": "intro-done"}}
            ]}),
            "sequences/intro.json",
            &schema(),
        )
        .unwrap();
        assert_eq!(seq.id, "intro");
        assert_eq!(seq.steps.len(), 7);
        assert_eq!(seq.steps[0].kind, "spawn");
        assert_eq!(seq.steps[4].kind, "wait");
    }

    #[test]
    fn id_defaults_to_filename() {
        let seq = Sequence::parse(&json!({"steps": []}), "sequences/ending.json", &schema()).unwrap();
        assert_eq!(seq.id, "ending");
    }

    #[test]
    fn out_of_order_at_is_an_error() {
        let err = Sequence::parse(
            &json!({"id": "bad", "steps": [
                {"at": 30, "do": {"emit": "a"}},
                {"at": 10, "do": {"emit": "b"}}
            ]}),
            "sequences/bad.json",
            &schema(),
        )
        .unwrap_err();
        let codes: Vec<&str> = err.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VD062"), "at 乱序要报 VD062: {err}");
    }

    #[test]
    fn unknown_action_lists_valid_kinds() {
        let err = Sequence::parse(
            &json!({"id": "bad", "steps": [{"at": 0, "do": {"fly": "moon"}}]}),
            "sequences/bad.json",
            &schema(),
        )
        .unwrap_err();
        let e = &err.errors[0];
        assert_eq!(e.code, "VD064");
        assert!(e.hint.contains("tween") && e.hint.contains("wait"), "{e}");
    }

    #[test]
    fn spawn_unknown_component_is_reported() {
        let err = Sequence::parse(
            &json!({"id": "bad", "steps": [
                {"at": 0, "do": {"spawn": {"components": {"Ghost": {}}}}}
            ]}),
            "sequences/bad.json",
            &schema(),
        )
        .unwrap_err();
        assert!(err.errors.iter().any(|e| e.code == "VD005"), "未知组件: {err}");
    }

    #[test]
    fn malformed_action_fields_report_with_paths() {
        let err = Sequence::parse(
            &json!({"id": "bad", "steps": [
                {"at": 0, "do": {"tween": {"target": "p", "field": "noPath", "from": 0, "to": 1}}},
                {"at": 1, "do": {"set": "noPath", "to": 1}},
                {"at": 2, "do": {"wait": ""}}
            ]}),
            "sequences/bad.json",
            &schema(),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("VD065"), "字段错误码: {text}");
        assert!(text.contains("steps/0/do/field"), "缺字段路径要点到 tween field: {text}");
        assert!(text.contains("steps/0/do/duration"), "缺 duration: {text}");
        assert!(text.contains("steps/1/do/set"), "set 缺字段路径: {text}");
        assert!(text.contains("steps/2/do/wait"), "wait 空名: {text}");
    }

    #[test]
    fn all_problems_reported_in_one_pass() {
        let err = Sequence::parse(
            &json!({"steps": [
                {"do": {"emit": "a"}},
                {"at": 5, "do": {"nope": 1}},
                {"at": 3, "do": {"emit": "c"}}
            ]}),
            "sequences/x.json",
            &schema(),
        )
        .unwrap_err();
        let codes: Vec<&str> = err.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VD061"), "缺 at: {err}");
        assert!(codes.contains(&"VD064"), "未知动作: {err}");
        assert!(codes.contains(&"VD062"), "at 乱序: {err}");
    }
}
