//! 帧间差量（scene delta）——两份场景视图 JSON 的纯比对，只说「变了啥」。
//!
//! 为什么放在 vitric-ecs：这是对 describe 输出（纯 JSON）做的纯 JSON 比对，
//! 不依赖规则/渲染/模拟——describe（vitric-render）和控制面（vitric-control）
//! 都已经依赖 vitric-ecs，放这里两边都够得到，且 ecs 只有 serde 依赖、不会造环。
//! 纯函数、确定、可单测：同两帧永远同输出。
//!
//! 输入约定：两帧都是 describe 的输出形态——实体散在顶层 `visible` / `offscreen`
//! 两个数组里，每个实体对象带 `id`（字符串句柄，跨帧稳定标识）。按 id 配对比较：
//! - `appeared`：只在新帧出现的实体（带它的新对象）；
//! - `disappeared`：只在旧帧出现的实体的 id；
//! - `changed`：两帧都在、但有字段变了的实体，`id → {字段: [旧值, 新值]}`。
//!   字段比对到顶层键（world / sprite / relative_to_focal / name / region / direction…
//!   整块比，块内变了就整块进 [旧,新]）——对 agent「自上次以来位置/可见性变没变」
//!   最直接。实体从 visible 挪到 offscreen 算「还在、变了」（同一 id 仍配上对），
//!   不算 disappeared。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// 比两份场景视图（describe 输出）求帧间差量。纯函数，确定。
///
/// 输出形如：
/// ```json
/// {"appeared": [ {实体对象...} ], "disappeared": ["e2v1", ...],
///  "changed": {"e1v1": {"world": [{"x":0,"y":0}, {"x":5,"y":0}]}}}
/// ```
/// 三个集合都按实体 id 的字典序排列（BTreeMap/已排序），输出可复现。
pub fn scene_delta(prev: &Value, cur: &Value) -> Value {
    // 各自按 id 收实体（visible + offscreen 合一：一个 id 在哪个桶不影响「它在不在」）。
    // 同 id 在两个桶里重复出现（理论上不会）以后者为准——describe 不会这么产，稳妥兜底。
    let prev_ents = collect_entities(prev);
    let cur_ents = collect_entities(cur);

    let mut appeared: Vec<Value> = Vec::new();
    let mut disappeared: Vec<String> = Vec::new();
    let mut changed: Map<String, Value> = Map::new();

    // 新帧里的每个实体：旧帧没有 = appeared；都有 = 逐字段比，有变化才进 changed。
    for (id, cur_obj) in &cur_ents {
        match prev_ents.get(id) {
            None => appeared.push((*cur_obj).clone()),
            Some(prev_obj) => {
                let field_diff = diff_fields(prev_obj, cur_obj);
                if !field_diff.is_empty() {
                    changed.insert(id.clone(), Value::Object(field_diff));
                }
            }
        }
    }
    // 旧帧有、新帧没有 = disappeared（只记 id，对象已经不在了）。
    for id in prev_ents.keys() {
        if !cur_ents.contains_key(id) {
            disappeared.push(id.clone());
        }
    }
    // prev_ents/cur_ents 是 BTreeMap，keys/iter 已是字典序；appeared 也随之有序。

    json!({
        "appeared": appeared,
        "disappeared": disappeared,
        "changed": Value::Object(changed),
    })
}

/// 从一份 describe 输出里按 id 收实体（visible + offscreen 两个数组合并）。
/// 没有 id 字段的条目跳过（describe 保证有 id；缺了无法跨帧配对，忽略最稳妥）。
fn collect_entities(view: &Value) -> BTreeMap<String, &Value> {
    let mut out: BTreeMap<String, &Value> = BTreeMap::new();
    for bucket in ["visible", "offscreen"] {
        if let Some(arr) = view.get(bucket).and_then(|v| v.as_array()) {
            for ent in arr {
                if let Some(id) = ent.get("id").and_then(|v| v.as_str()) {
                    out.insert(id.to_string(), ent);
                }
            }
        }
    }
    out
}

/// 逐顶层字段比两个实体对象，收变了的：`字段名 → [旧值, 新值]`。
/// id 字段本身不进 diff（它是配对键，按定义两帧相等）。出现的新字段旧值记 null、
/// 消失的旧字段新值记 null——对 agent「这块出现了/没了」同样是「变了」。
/// 比到顶层键即止（块内容整块比、整块进 [旧,新]），不再往下钻——位置/精灵/相对关系
/// 都是小对象，整块给反而比逐叶子更好读。
fn diff_fields(prev: &Value, cur: &Value) -> Map<String, Value> {
    let empty = Map::new();
    let prev_obj = prev.as_object().unwrap_or(&empty);
    let cur_obj = cur.as_object().unwrap_or(&empty);

    let mut out: Map<String, Value> = Map::new();
    // 新帧的字段：和旧帧比，不同（含新增）就记 [旧, 新]
    for (k, cv) in cur_obj {
        if k == "id" {
            continue;
        }
        let pv = prev_obj.get(k);
        if pv != Some(cv) {
            out.insert(k.clone(), json!([pv.cloned().unwrap_or(Value::Null), cv.clone()]));
        }
    }
    // 旧帧有、新帧没了的字段：记 [旧, null]
    for (k, pv) in prev_obj {
        if k == "id" || cur_obj.contains_key(k) {
            continue;
        }
        out.insert(k.clone(), json!([pv.clone(), Value::Null]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 攒一份最小 describe 形态：把若干实体放进 visible 桶。
    fn view(visible: Vec<Value>) -> Value {
        json!({"visible": visible, "offscreen": []})
    }

    fn ent(id: &str, x: f64, y: f64) -> Value {
        json!({"id": id, "world": {"x": x, "y": y}, "sprite": {"w": 1.0, "h": 1.0}})
    }

    #[test]
    fn appeared_disappeared_changed_each() {
        // prev: a(0,0) b(1,1)；cur: a(5,0 移动) c(2,2 新)。b 消失。
        let prev = view(vec![ent("e1v1", 0.0, 0.0), ent("e2v1", 1.0, 1.0)]);
        let cur = view(vec![ent("e1v1", 5.0, 0.0), ent("e3v1", 2.0, 2.0)]);
        let d = scene_delta(&prev, &cur);

        // appeared: 只有 c
        let app = d["appeared"].as_array().unwrap();
        assert_eq!(app.len(), 1);
        assert_eq!(app[0]["id"], json!("e3v1"));

        // disappeared: 只有 b
        assert_eq!(d["disappeared"], json!(["e2v1"]));

        // changed: a 的 world 从 (0,0) 变 (5,0)
        let changed = d["changed"].as_object().unwrap();
        assert_eq!(changed.len(), 1);
        let a_world = &changed["e1v1"]["world"];
        assert_eq!(a_world[0], json!({"x": 0.0, "y": 0.0}), "旧值");
        assert_eq!(a_world[1], json!({"x": 5.0, "y": 0.0}), "新值");
    }

    #[test]
    fn unchanged_entity_not_in_changed() {
        // 完全没动 → changed 空、appeared/disappeared 空
        let prev = view(vec![ent("e1v1", 3.0, 4.0)]);
        let cur = view(vec![ent("e1v1", 3.0, 4.0)]);
        let d = scene_delta(&prev, &cur);
        assert!(d["appeared"].as_array().unwrap().is_empty());
        assert!(d["disappeared"].as_array().unwrap().is_empty());
        assert!(d["changed"].as_object().unwrap().is_empty(), "没变就不进 changed: {d}");
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let prev = view(vec![ent("e1v1", 0.0, 0.0), ent("e2v1", 1.0, 1.0)]);
        let cur = view(vec![ent("e1v1", 9.0, 0.0), ent("e9v1", 2.0, 2.0)]);
        assert_eq!(scene_delta(&prev, &cur), scene_delta(&prev, &cur));
    }

    #[test]
    fn entity_moving_visible_to_offscreen_is_changed_not_disappeared() {
        // 同一 id 从 visible 挪到 offscreen：还在、算 changed（字段变了），不算消失。
        let prev = json!({"visible": [ent("e1v1", 0.0, 0.0)], "offscreen": []});
        let cur = json!({
            "visible": [],
            "offscreen": [json!({"id": "e1v1", "world": {"x": 999.0, "y": 0.0}, "direction": "右"})]
        });
        let d = scene_delta(&prev, &cur);
        assert!(d["disappeared"].as_array().unwrap().is_empty(), "没消失: {d}");
        assert!(d["appeared"].as_array().unwrap().is_empty());
        // world 变了 + 多出 direction、少了 sprite —— 都算字段变化
        let ch = &d["changed"]["e1v1"];
        assert_eq!(ch["world"][1], json!({"x": 999.0, "y": 0.0}));
        assert_eq!(ch["direction"], json!([Value::Null, json!("右")]), "新增字段旧值 null");
        assert_eq!(ch["sprite"][1], Value::Null, "消失字段新值 null");
    }

    #[test]
    fn empty_views_yield_empty_delta() {
        // 第一帧场景（prev 没有 visible/offscreen 键）也不炸：全空
        let d = scene_delta(&json!({}), &view(vec![ent("e1v1", 0.0, 0.0)]));
        // prev 空 → 新帧那个实体算 appeared
        assert_eq!(d["appeared"].as_array().unwrap().len(), 1);
        assert!(d["disappeared"].as_array().unwrap().is_empty());
        assert!(d["changed"].as_object().unwrap().is_empty());
    }
}
