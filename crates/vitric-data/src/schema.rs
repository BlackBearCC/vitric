use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::ValidationReport;

/// 字段类型。刻意保持很小：够描述 2D 游戏数据，又让每个值都能被精确校验。
#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    /// 浮点数
    Number,
    /// 整数
    Int,
    Bool,
    Text,
    /// {"x": f64, "y": f64}
    Vec2,
    /// 实体引用，值是实体名（场景内）或 "e<i>v<g>" 句柄（运行时）
    Entity,
    /// 枚举：值必须是 variants 之一
    Enum(Vec<String>),
    /// 同质列表
    List(Box<FieldType>),
}

impl FieldType {
    fn parse(spec: &Value, path: &str, report: &mut ValidationReport) -> Option<FieldType> {
        let ty = spec.get("type").and_then(|v| v.as_str());
        match ty {
            Some("number") => Some(FieldType::Number),
            Some("int") => Some(FieldType::Int),
            Some("bool") => Some(FieldType::Bool),
            Some("text") => Some(FieldType::Text),
            Some("vec2") => Some(FieldType::Vec2),
            Some("entity") => Some(FieldType::Entity),
            Some("enum") => {
                let variants: Option<Vec<String>> = spec
                    .get("variants")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
                match variants {
                    Some(v) if !v.is_empty() => Some(FieldType::Enum(v)),
                    _ => {
                        report.push(
                            "VD010",
                            path,
                            "enum 类型缺少非空的 variants 数组",
                            "写法: {\"type\":\"enum\",\"variants\":[\"idle\",\"run\"]}",
                        );
                        None
                    }
                }
            }
            Some("list") => {
                let of = spec.get("of");
                match of {
                    Some(inner) => FieldType::parse(inner, &format!("{path}/of"), report)
                        .map(|t| FieldType::List(Box::new(t))),
                    None => {
                        report.push(
                            "VD011",
                            path,
                            "list 类型缺少 of（元素类型）",
                            "写法: {\"type\":\"list\",\"of\":{\"type\":\"number\"}}",
                        );
                        None
                    }
                }
            }
            Some(other) => {
                report.push(
                    "VD012",
                    path,
                    format!("未知字段类型 {other:?}"),
                    "可用类型: number / int / bool / text / vec2 / entity / enum / list",
                );
                None
            }
            None => {
                report.push(
                    "VD013",
                    path,
                    "字段定义缺少 type",
                    "每个字段形如 {\"type\":\"number\",\"default\":0}",
                );
                None
            }
        }
    }

    pub fn name(&self) -> String {
        match self {
            FieldType::Number => "number".into(),
            FieldType::Int => "int".into(),
            FieldType::Bool => "bool".into(),
            FieldType::Text => "text".into(),
            FieldType::Vec2 => "vec2".into(),
            FieldType::Entity => "entity".into(),
            FieldType::Enum(v) => format!("enum[{}]", v.join("|")),
            FieldType::List(t) => format!("list<{}>", t.name()),
        }
    }

    /// 该类型的零值（字段没写 default 时用）。
    pub fn zero(&self) -> Value {
        match self {
            FieldType::Number | FieldType::Int => json!(0),
            FieldType::Bool => json!(false),
            FieldType::Text => json!(""),
            FieldType::Vec2 => json!({"x": 0.0, "y": 0.0}),
            FieldType::Entity => Value::Null,
            FieldType::Enum(v) => json!(v[0]),
            FieldType::List(_) => json!([]),
        }
    }

    /// 校验一个值是否符合本类型。
    pub fn check(&self, value: &Value, path: &str, report: &mut ValidationReport) {
        let ok = match self {
            FieldType::Number => value.is_number(),
            FieldType::Int => value.is_i64() || value.is_u64(),
            FieldType::Bool => value.is_boolean(),
            FieldType::Text => value.is_string(),
            FieldType::Vec2 => {
                value.is_object()
                    && value.get("x").is_some_and(Value::is_number)
                    && value.get("y").is_some_and(Value::is_number)
                    && value.as_object().map(|o| o.len()) == Some(2)
            }
            // entity 引用允许 null（"还没指向谁"），实体名存在性由场景校验做
            FieldType::Entity => value.is_null() || value.is_string(),
            FieldType::Enum(variants) => {
                if let Some(s) = value.as_str() {
                    if variants.iter().any(|v| v == s) {
                        true
                    } else {
                        report.push(
                            "VD022",
                            path,
                            format!("枚举值 {s:?} 不在可选项里"),
                            format!("可选: [{}]", variants.join(", ")),
                        );
                        return;
                    }
                } else {
                    false
                }
            }
            FieldType::List(inner) => {
                if let Some(arr) = value.as_array() {
                    for (i, item) in arr.iter().enumerate() {
                        inner.check(item, &format!("{path}/{i}"), report);
                    }
                    return;
                }
                false
            }
        };
        if !ok {
            report.push(
                "VD020",
                path,
                format!("类型不符：期望 {}，拿到 {}", self.name(), value_type(value)),
                expect_hint(self),
            );
        }
    }
}

impl FieldType {
    /// 统一数值表示：number 字段一律存成浮点形态（5 → 5.0）。
    /// 没有这一步，同一个值会因写入方不同（场景 JSON / JS 往返 / 规则动作）
    /// 在世界里出现 int/float 两种形态，状态哈希和相等判断都会被表示差异干扰。
    pub fn canonicalize(&self, value: &Value) -> Value {
        match self {
            FieldType::Number => match value.as_f64() {
                Some(f) => json!(f),
                None => value.clone(),
            },
            FieldType::Vec2 => {
                let (Some(x), Some(y)) = (
                    value.get("x").and_then(Value::as_f64),
                    value.get("y").and_then(Value::as_f64),
                ) else {
                    return value.clone();
                };
                json!({ "x": x, "y": y })
            }
            FieldType::List(inner) => match value.as_array() {
                Some(arr) => Value::Array(arr.iter().map(|v| inner.canonicalize(v)).collect()),
                None => value.clone(),
            },
            _ => value.clone(),
        }
    }
}

fn value_type(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(n) if n.is_i64() || n.is_u64() => "int".into(),
        Value::Number(_) => "number".into(),
        Value::String(_) => "text".into(),
        Value::Array(_) => "list".into(),
        Value::Object(_) => "object".into(),
    }
}

fn expect_hint(t: &FieldType) -> String {
    match t {
        FieldType::Vec2 => "vec2 写法: {\"x\": 1.0, \"y\": 2.0}，且只能有 x、y 两个键".into(),
        FieldType::Entity => "entity 写法: 实体名字符串（如 \"player\"）或 null".into(),
        FieldType::Int => "int 必须是不带小数点的整数".into(),
        other => format!("该字段类型是 {}", other.name()),
    }
}

/// 一个字段的完整定义。
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub ty: FieldType,
    /// 实例化时缺省用的值；None 则用类型零值。
    pub default: Option<Value>,
    /// true = 场景里必须显式给值。
    pub required: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl FieldDef {
    pub fn effective_default(&self) -> Value {
        let v = self.default.clone().unwrap_or_else(|| self.ty.zero());
        self.ty.canonicalize(&v)
    }

    fn check_range(&self, value: &Value, path: &str, report: &mut ValidationReport) {
        if let Some(n) = value.as_f64() {
            if let Some(min) = self.min {
                if n < min {
                    report.push(
                        "VD021",
                        path,
                        format!("值 {n} 小于下限 {min}"),
                        format!("取值范围 [{}, {}]", min, self.max.map_or("∞".into(), |m| m.to_string())),
                    );
                }
            }
            if let Some(max) = self.max {
                if n > max {
                    report.push(
                        "VD021",
                        path,
                        format!("值 {n} 大于上限 {max}"),
                        format!("取值范围 [{}, {}]", self.min.map_or("-∞".into(), |m| m.to_string()), max),
                    );
                }
            }
        }
    }
}

/// 一个组件的 schema。
#[derive(Debug, Clone)]
pub struct ComponentSchema {
    pub name: String,
    pub fields: BTreeMap<String, FieldDef>,
}

impl ComponentSchema {
    /// 校验并归一化一个组件值：填默认值、查未知字段、查类型和范围。
    /// 返回归一化后的值（即使有错也尽量归一化，让错误一次全暴露）。
    pub fn normalize(&self, value: &Value, path: &str, report: &mut ValidationReport) -> Value {
        let Some(obj) = value.as_object() else {
            report.push(
                "VD002",
                path,
                format!("组件值必须是对象，拿到 {}", value_type(value)),
                format!("写法: \"{}\": {{ ... 字段 ... }}", self.name),
            );
            return value.clone();
        };
        // 未知字段
        for key in obj.keys() {
            if !self.fields.contains_key(key) {
                report.push(
                    "VD003",
                    format!("{path}/{key}"),
                    format!("组件 {} 没有字段 {key:?}", self.name),
                    format!(
                        "该组件的字段: [{}]。字段集合由 schema 决定，需要新字段先改 schema",
                        self.fields.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                );
            }
        }
        // 逐字段：缺失补默认 / required 报错；存在则查类型范围
        let mut out = Map::new();
        for (fname, fdef) in &self.fields {
            let fpath = format!("{path}/{fname}");
            match obj.get(fname) {
                Some(v) => {
                    fdef.ty.check(v, &fpath, report);
                    fdef.check_range(v, &fpath, report);
                    out.insert(fname.clone(), fdef.ty.canonicalize(v));
                }
                None if fdef.required => {
                    report.push(
                        "VD004",
                        fpath,
                        format!("缺少必填字段 {fname:?}"),
                        format!("类型 {}，必须显式给值", fdef.ty.name()),
                    );
                    out.insert(fname.clone(), fdef.effective_default());
                }
                None => {
                    out.insert(fname.clone(), fdef.effective_default());
                }
            }
        }
        Value::Object(out)
    }
}

/// 整个项目的组件 schema 集合。
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub components: BTreeMap<String, ComponentSchema>,
}

impl Schema {
    /// 从 JSON 解析 schema 文件。格式:
    /// ```json
    /// { "components": { "Position": { "fields": { "x": {"type":"number"} } } } }
    /// ```
    pub fn parse(doc: &Value, file: &str) -> Result<Schema, ValidationReport> {
        let mut report = ValidationReport::default();
        let mut schema = Schema::default();
        let comps = doc.get("components").and_then(|v| v.as_object());
        let Some(comps) = comps else {
            report.push(
                "VD001",
                format!("{file}#/components"),
                "schema 文件缺少 components 对象",
                "顶层结构: {\"components\": { \"组件名\": {\"fields\": {...}} }}",
            );
            return Err(report);
        };
        for (cname, cdef) in comps {
            let cpath = format!("{file}#/components/{cname}");
            let fields_doc = cdef.get("fields").and_then(|v| v.as_object());
            let Some(fields_doc) = fields_doc else {
                report.push(
                    "VD001",
                    &cpath,
                    format!("组件 {cname} 缺少 fields 对象"),
                    "写法: {\"fields\": {\"x\": {\"type\":\"number\"}}}",
                );
                continue;
            };
            let mut fields = BTreeMap::new();
            for (fname, fspec) in fields_doc {
                let fpath = format!("{cpath}/fields/{fname}");
                let Some(ty) = FieldType::parse(fspec, &fpath, &mut report) else {
                    continue;
                };
                let default = fspec.get("default").cloned();
                if let Some(d) = &default {
                    // 默认值本身必须通过类型校验
                    ty.check(d, &format!("{fpath}/default"), &mut report);
                }
                fields.insert(
                    fname.clone(),
                    FieldDef {
                        ty,
                        default,
                        required: fspec.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                        min: fspec.get("min").and_then(|v| v.as_f64()),
                        max: fspec.get("max").and_then(|v| v.as_f64()),
                    },
                );
            }
            schema
                .components
                .insert(cname.clone(), ComponentSchema { name: cname.clone(), fields });
        }
        report.into_result(schema)
    }

    pub fn component(&self, name: &str) -> Option<&ComponentSchema> {
        self.components.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_schema() -> Schema {
        Schema::parse(
            &json!({
                "components": {
                    "Position": { "fields": {
                        "x": {"type": "number", "default": 0},
                        "y": {"type": "number", "default": 0}
                    }},
                    "Health": { "fields": {
                        "hp": {"type": "int", "default": 100, "min": 0, "max": 100},
                        "state": {"type": "enum", "variants": ["alive", "dead"], "default": "alive"}
                    }},
                    "Sprite": { "fields": {
                        "image": {"type": "text", "required": true}
                    }}
                }
            }),
            "schema.json",
        )
        .unwrap()
    }

    use serde_json::json;

    #[test]
    fn parse_and_normalize_with_defaults() {
        let s = demo_schema();
        let mut report = ValidationReport::default();
        let v = s.component("Position").unwrap().normalize(&json!({"x": 5}), "p", &mut report);
        assert!(report.ok(), "{report}");
        // number 字段统一存浮点形态：表示唯一，状态哈希不受写入方影响
        assert_eq!(v, json!({"x": 5.0, "y": 0.0}));
    }

    #[test]
    fn unknown_field_lists_valid_fields() {
        let s = demo_schema();
        let mut report = ValidationReport::default();
        s.component("Position").unwrap().normalize(&json!({"z": 1}), "p", &mut report);
        let err = &report.errors[0];
        assert_eq!(err.code, "VD003");
        assert!(err.hint.contains("x, y"), "{err}");
    }

    #[test]
    fn type_range_enum_required_checks() {
        let s = demo_schema();
        let mut report = ValidationReport::default();
        s.component("Health").unwrap().normalize(
            &json!({"hp": 999, "state": "zombie"}),
            "h",
            &mut report,
        );
        let codes: Vec<&str> = report.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VD021"), "超范围: {report}");
        assert!(codes.contains(&"VD022"), "枚举: {report}");

        let mut report = ValidationReport::default();
        s.component("Sprite").unwrap().normalize(&json!({}), "sp", &mut report);
        assert_eq!(report.errors[0].code, "VD004", "必填缺失: {report}");

        let mut report = ValidationReport::default();
        s.component("Health").unwrap().normalize(&json!({"hp": 1.5}), "h", &mut report);
        assert_eq!(report.errors[0].code, "VD020", "int 不收小数: {report}");
    }

    #[test]
    fn vec2_shape_enforced() {
        let s = Schema::parse(
            &json!({"components": {"T": {"fields": {"pos": {"type": "vec2"}}}}}),
            "s.json",
        )
        .unwrap();
        let mut report = ValidationReport::default();
        s.component("T").unwrap().normalize(&json!({"pos": {"x": 1, "y": 2, "z": 3}}), "t", &mut report);
        assert!(!report.ok(), "vec2 多余的键必须报错");
    }

    #[test]
    fn bad_schema_reports_all_problems_at_once() {
        let err = Schema::parse(
            &json!({"components": {
                "A": {"fields": {"f": {"type": "rocket"}}},
                "B": {"fields": {"g": {}}},
                "C": {"fields": {"h": {"type": "enum"}}}
            }}),
            "schema.json",
        )
        .unwrap_err();
        assert_eq!(err.errors.len(), 3, "三个问题一次全报: {err}");
    }
}
