//! Frame-to-frame delta (scene delta) — a pure comparison of two scene-view JSONs, reporting only "what changed".
//!
//! Why this lives in vitric-ecs: this is a pure JSON comparison done on the output of describe (pure JSON),
//! with no dependency on rules/render/simulation — both describe (vitric-render) and the control plane
//! (vitric-control) already depend on vitric-ecs, so placing it here is reachable by both, and ecs has only
//! serde as a dependency, so no cycle is created. Pure functions, deterministic, unit-testable: same two
//! frames -> same output forever.
//!
//! Input convention: both frames are describe's output shape — entities are spread across the top-level
//! `visible` / `offscreen` arrays, each entity object carries an `id` (string handle, stable cross-frame
//! identifier). Pair by id and compare:
//! - `appeared`: entities only in the new frame (with their new object);
//! - `disappeared`: ids of entities only in the old frame;
//! - `changed`: entities present in both frames but with changed fields, `id -> {field: [old value, new value]}`.
//!   Field comparison goes down to top-level keys (world / sprite / relative_to_focal / name / region / direction...
//!   compared as a whole block; if a block changed, the whole block goes into [old, new]) — most directly useful
//!   for an agent asking "has my position/visibility changed since last time". An entity moving from visible to
//!   offscreen counts as "still present, changed" (the same id is still paired), not as disappeared.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// Compute the frame-to-frame delta between two scene views (describe output). Pure function, deterministic.
///
/// Output shape:
/// ```json
/// {"appeared": [ {entity object...} ], "disappeared": ["e2v1", ...],
///  "changed": {"e1v1": {"world": [{"x":0,"y":0}, {"x":5,"y":0}]}}}
/// ```
/// All three sets are sorted by entity id in lexicographic order (BTreeMap / already sorted), so the output is reproducible.
pub fn scene_delta(prev: &Value, cur: &Value) -> Value {
    // Collect entities by id from each side (visible + offscreen merged: which bucket an id is in doesn't affect "is it there").
    // If the same id appears in both buckets (theoretically won't happen), the latter wins — describe never produces this, just a safe fallback.
    let prev_ents = collect_entities(prev);
    let cur_ents = collect_entities(cur);

    let mut appeared: Vec<Value> = Vec::new();
    let mut disappeared: Vec<String> = Vec::new();
    let mut changed: Map<String, Value> = Map::new();

    // For each entity in the new frame: not in old = appeared; in both = compare field by field, only enters changed if there's a change.
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
    // In old but not in new = disappeared (only the id is recorded, the object is gone).
    for id in prev_ents.keys() {
        if !cur_ents.contains_key(id) {
            disappeared.push(id.clone());
        }
    }
    // prev_ents/cur_ents are BTreeMaps, so keys/iter are already in lexicographic order; appeared is ordered accordingly.

    json!({
        "appeared": appeared,
        "disappeared": disappeared,
        "changed": Value::Object(changed),
    })
}

/// Collect entities by id from a describe output (merging the visible + offscreen arrays).
/// Entries without an id field are skipped (describe guarantees an id; without one, cross-frame pairing is impossible, so ignoring is safest).
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

/// Compare two entity objects top-level field by field, collecting changes: `field name -> [old value, new value]`.
/// The id field itself is not part of the diff (it's the pairing key, equal across frames by definition). New fields get null as the old value,
/// and disappeared fields get null as the new value — to an agent, "this block appeared/gone" is also "changed".
/// Comparison stops at top-level keys (each block is compared and emitted as a whole into [old, new]), without drilling deeper — position/sprite/relative-to
/// are all small objects; emitting the whole block is more readable than going leaf by leaf.
fn diff_fields(prev: &Value, cur: &Value) -> Map<String, Value> {
    let empty = Map::new();
    let prev_obj = prev.as_object().unwrap_or(&empty);
    let cur_obj = cur.as_object().unwrap_or(&empty);

    let mut out: Map<String, Value> = Map::new();
    // Fields in the new frame: compare with old, record [old, new] if different (including new fields)
    for (k, cv) in cur_obj {
        if k == "id" {
            continue;
        }
        let pv = prev_obj.get(k);
        if pv != Some(cv) {
            out.insert(k.clone(), json!([pv.cloned().unwrap_or(Value::Null), cv.clone()]));
        }
    }
    // Fields in old but gone in new: record [old, null]
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

    /// Build a minimal describe shape: put some entities into the visible bucket.
    fn view(visible: Vec<Value>) -> Value {
        json!({"visible": visible, "offscreen": []})
    }

    fn ent(id: &str, x: f64, y: f64) -> Value {
        json!({"id": id, "world": {"x": x, "y": y}, "sprite": {"w": 1.0, "h": 1.0}})
    }

    #[test]
    fn appeared_disappeared_changed_each() {
        // prev: a(0,0) b(1,1); cur: a(5,0 moved) c(2,2 new). b disappears.
        let prev = view(vec![ent("e1v1", 0.0, 0.0), ent("e2v1", 1.0, 1.0)]);
        let cur = view(vec![ent("e1v1", 5.0, 0.0), ent("e3v1", 2.0, 2.0)]);
        let d = scene_delta(&prev, &cur);

        // appeared: only c
        let app = d["appeared"].as_array().unwrap();
        assert_eq!(app.len(), 1);
        assert_eq!(app[0]["id"], json!("e3v1"));

        // disappeared: only b
        assert_eq!(d["disappeared"], json!(["e2v1"]));

        // changed: a's world changed from (0,0) to (5,0)
        let changed = d["changed"].as_object().unwrap();
        assert_eq!(changed.len(), 1);
        let a_world = &changed["e1v1"]["world"];
        assert_eq!(a_world[0], json!({"x": 0.0, "y": 0.0}), "旧值");
        assert_eq!(a_world[1], json!({"x": 5.0, "y": 0.0}), "新值");
    }

    #[test]
    fn unchanged_entity_not_in_changed() {
        // Completely unchanged -> changed empty, appeared/disappeared empty
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
        // Same id moved from visible to offscreen: still present, counts as changed (fields changed), not as disappeared.
        let prev = json!({"visible": [ent("e1v1", 0.0, 0.0)], "offscreen": []});
        let cur = json!({
            "visible": [],
            "offscreen": [json!({"id": "e1v1", "world": {"x": 999.0, "y": 0.0}, "direction": "右"})]
        });
        let d = scene_delta(&prev, &cur);
        assert!(d["disappeared"].as_array().unwrap().is_empty(), "没消失: {d}");
        assert!(d["appeared"].as_array().unwrap().is_empty());
        // world changed + gained direction, lost sprite — all count as field changes
        let ch = &d["changed"]["e1v1"];
        assert_eq!(ch["world"][1], json!({"x": 999.0, "y": 0.0}));
        assert_eq!(ch["direction"], json!([Value::Null, json!("右")]), "新增字段旧值 null");
        assert_eq!(ch["sprite"][1], Value::Null, "消失字段新值 null");
    }

    #[test]
    fn empty_views_yield_empty_delta() {
        // First frame (prev has no visible/offscreen keys) doesn't blow up either: all empty
        let d = scene_delta(&json!({}), &view(vec![ent("e1v1", 0.0, 0.0)]));
        // prev empty -> the entity in the new frame counts as appeared
        assert_eq!(d["appeared"].as_array().unwrap().len(), 1);
        assert!(d["disappeared"].as_array().unwrap().is_empty());
        assert!(d["changed"].as_object().unwrap().is_empty());
    }
}
