//! Sequence (timeline) static data: declarative action track + validation.
//!
//! Positioning: the engine's generic timeline primitive, comparable to Unity Timeline / Godot AnimationPlayer —
//! an action track that advances by relative ticks and triggers **existing generic verbs of the engine** at specified moments.
//! The engine has no theme-specific concepts like "cutscene", "comic page", or "card"; comic-style cutscenes are **usages** built from these blocks
//! within a game project (see examples/intro's sequences/opening.json).
//!
//! File form (`sequences/<name>.json`):
//! ```json
//! {
//!   "id": "intro",
//!   "steps": [
//!     { "at": 0,   "do": { "spawn": { "name": "panel", "components": { ... } } } },
//!     { "at": 0,   "do": { "tween": { "target": "panel", "field": "Sprite.color_a",
//!                                       "from": 0, "to": 1, "duration": 30 } } },
//!     { "at": 30,  "do": { "set": "@subtitle.Text.content", "to": "He walks into the corridor" } },
//!     { "at": 150, "do": { "wait": "player-confirm" } },
//!     { "at": 151, "do": { "emit": "intro-done" } }
//!   ]
//! }
//! ```
//!
//! `at` is the tick relative to the sequence's start point (no matter when the same sequence starts, it always plays from its own t=0).
//! The action set is fixed in v1 (not Turing-complete, no embedded scripts), all mirroring existing engine verbs:
//! `tween` / `set` / `spawn` / `despawn` / `emit` / `sound` / `wait`.
//! Scene switching is not built in — sequences decouple via `emit "load-scene"`, which project rules pick up to load-scene.

use serde_json::{Map, Value};

use crate::{Schema, ValidationReport};

/// Fixed action set in a sequence (v1). Each mirrors an existing generic engine verb, introducing no new semantics.
pub const SEQ_ACTION_KINDS: &[&str] =
    &["tween", "set", "spawn", "despawn", "emit", "sound", "wait"];

/// One action in a sequence (already parsed into a form the engine recognizes).
#[derive(Debug, Clone)]
pub struct SeqStep {
    /// Tick relative to the sequence's start point (monotonically non-decreasing).
    pub at: u64,
    /// Action type name (within [`SEQ_ACTION_KINDS`]).
    pub kind: String,
    /// Raw action JSON object (the execution end reads fields by kind).
    pub action: Value,
}

/// A sequence (a static track after parsing + validation). At runtime a `Sequence` component references it by name and
/// holds only minimal playback state (cursor + start tick + barrier flag); the static track does not enter any snapshot.
#[derive(Debug, Clone)]
pub struct Sequence {
    /// Sequence name (referenced by the manifest / the component's `track` field; defaults to the file id).
    pub id: String,
    /// Source file relative path (for error localization).
    pub file: String,
    /// Ordered action entries (`at` monotonically non-decreasing).
    pub steps: Vec<SeqStep>,
}

impl Sequence {
    /// Parse + validate a sequence file against the schema.
    /// Validation items: `at` is monotonically non-decreasing, action name is in the fixed set, action fields pass schema,
    /// and referenced entity / texture / event names are well-formed. All problems are reported at once (does not stop at the first error).
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

/// Take the default sequence name from the file's relative path (`sequences/intro.json` -> `intro`).
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
        // at: non-negative integer, monotonically non-decreasing (a sequence is an ordered track; out-of-order entries have no defined playback order)
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
        // do: action object, type is in the fixed set, fields pass the corresponding schema
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

/// Validate an action object and return its type name. Unknown type / illegal fields all go into report.
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

/// tween: starts a tween component (reuses the already-shipped Tween). Camera pushes/pulls, fade in/out, translation, scaling, and color changes all rely on it.
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
    // ease is optional; if given, it must be text (the legality of the specific curve name is reported by the tween system at runtime;
    // here we only catch obvious type errors — same scope as the tween component's own parsing)
    if let Some(e) = t.get("ease") {
        if !e.is_string() {
            report.push("VD065", format!("{path}/ease"), "ease 必须是文本（缓动曲线名）", tween_hint());
        }
    }
}

fn tween_hint() -> String {
    "写法: {\"tween\": {\"target\": \"实体名\", \"field\": \"Sprite.color_a\", \"from\": 0, \"to\": 1, \"duration\": 30, \"ease\": \"ease-out\"}}".to_string()
}

/// set: instantaneously set a field (mirrors the rule set). target = "entity.field path" reference, to = value.
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

/// spawn: spawn an entity (mirrors the rule spawn). Component values pass schema.
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
        // Reference values (@/event. etc. runtime paths) skip strict schema validation, same scope as rule spawn:
        // only run the whole thing through schema when there are no reference strings (those with references are left for runtime parsing and then validation)
        if !contains_ref(cval) {
            cschema.normalize(cval, &cpath, report);
        }
    }
}

/// despawn: destroy an entity (mirrors the rule despawn). Value = entity reference text.
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

/// emit: emit an event for rules to chain on (the front door for sequence/scene decoupling). Value = event name.
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

/// sound: play a sound effect (mirrors audio; translated into a play-sound event at runtime). Value = sound file name.
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

/// wait: barrier — the cursor stops until a named event fires / skip input arrives. Value = the event name to wait for.
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

/// Whether the component value contains a runtime reference string (starting with @/self./other./event.) — those with references are left for
/// runtime parsing and then validation; static schema validation lets them through (same scope as rule spawn).
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
