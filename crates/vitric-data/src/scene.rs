use serde_json::Value;

use vitric_ecs::World;

use crate::{Schema, ValidationReport};

/// Scene file: a list of entities, each of which can be named and have components attached.
///
/// ```json
/// { "entities": [
///   { "name": "player", "components": { "Position": {"x": 10, "y": 20} } }
/// ]}
/// ```
#[derive(Debug, Clone)]
pub struct Scene {
    pub file: String,
    pub doc: Value,
}

impl Scene {
    /// Parse + validate against the schema (does not land in World; only validates the data).
    pub fn parse(doc: Value, file: &str, schema: &Schema) -> Result<Scene, ValidationReport> {
        let mut report = ValidationReport::default();
        validate_scene(&doc, file, schema, &mut report);
        report.into_result(Scene { file: file.to_string(), doc })
    }
}

fn validate_scene(doc: &Value, file: &str, schema: &Schema, report: &mut ValidationReport) {
    let Some(entities) = doc.get("entities").and_then(|v| v.as_array()) else {
        report.push(
            "VD030",
            format!("{file}#/entities"),
            "场景缺少 entities 数组",
            "顶层结构: {\"entities\": [ {\"name\": \"...\", \"components\": {...}} ]}",
        );
        return;
    };
    // Collect entity names: for duplicate detection + for entity reference validation
    let mut names: Vec<&str> = Vec::new();
    for (i, ent) in entities.iter().enumerate() {
        if let Some(name) = ent.get("name").and_then(|v| v.as_str()) {
            if names.contains(&name) {
                report.push(
                    "VD031",
                    format!("{file}#/entities/{i}/name"),
                    format!("实体名 {name:?} 重复"),
                    "实体名在场景内必须唯一",
                );
            }
            names.push(name);
        }
    }
    for (i, ent) in entities.iter().enumerate() {
        let epath = format!("{file}#/entities/{i}");
        let Some(comps) = ent.get("components").and_then(|v| v.as_object()) else {
            report.push(
                "VD032",
                format!("{epath}/components"),
                "实体缺少 components 对象",
                "每个实体形如 {\"components\": {\"Position\": {...}}}（components 可为空对象）",
            );
            continue;
        };
        // Collect a copy of the normalized component values, for UI value-level cross-field validation (anchor / container kind / Grid columns)
        let mut normalized_comps = serde_json::Map::new();
        for (cname, cval) in comps {
            let cpath = format!("{epath}/components/{cname}");
            let Some(cschema) = schema.component(cname) else {
                report.push(
                    "VD005",
                    &cpath,
                    format!("未知组件 {cname:?}"),
                    format!(
                        "schema 里定义的组件: [{}]。需要新组件先加进 schema",
                        schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                );
                continue;
            };
            let normalized = cschema.normalize(cval, &cpath, report);
            // entity-typed fields must reference entity names that exist in the scene
            for (fname, fdef) in &cschema.fields {
                if matches!(fdef.ty, crate::FieldType::Entity) {
                    if let Some(refname) = normalized.get(fname).and_then(|v| v.as_str()) {
                        if !names.contains(&refname) {
                            report.push(
                                "VD033",
                                format!("{cpath}/{fname}"),
                                format!("引用了不存在的实体 {refname:?}"),
                                format!(
                                    "场景里的命名实体: [{}]",
                                    names.join(", ")
                                ),
                            );
                        }
                    }
                }
            }
            normalized_comps.insert(cname.clone(), normalized);
        }
        // UI value-level semantic validation (anchor preset name / container kind / Grid columns / alignment name) — engine backstop,
        // does not rely on the author declaring fields as enums. Component existence is already ensured by the schema validation above.
        crate::ui_check::validate_ui_components(&normalized_comps, &epath, report);
    }
}

/// Land the validated scene into World: create entities, fill in the normalized component values.
///
/// Entity references (names in entity fields) are resolved into runtime handles "e<i>v<g>" at landing —
/// scene files use names (easy for humans/AI to write), runtime uses handles (precise and unambiguous).
pub fn instantiate_scene(scene: &Scene, schema: &Schema, world: &mut World) -> Result<(), ValidationReport> {
    let mut report = ValidationReport::default();
    let entities = scene
        .doc
        .get("entities")
        .and_then(|v| v.as_array())
        .expect("Scene::parse 已校验结构");

    // First pass: create entities (build them all first, so reference resolution has targets)
    let mut ids = Vec::with_capacity(entities.len());
    for (i, ent) in entities.iter().enumerate() {
        let id = match ent.get("name").and_then(|v| v.as_str()) {
            Some(name) => match world.spawn_named(name) {
                Ok(id) => id,
                Err(e) => {
                    report.push(
                        "VD034",
                        format!("{}#/entities/{i}/name", scene.file),
                        e.to_string(),
                        "场景实例化到的 World 里已有同名实体",
                    );
                    world.spawn()
                }
            },
            None => world.spawn(),
        };
        ids.push(id);
    }

    // Second pass: fill components, swap names in entity fields for handles
    for (i, ent) in entities.iter().enumerate() {
        let comps = ent
            .get("components")
            .and_then(|v| v.as_object())
            .expect("Scene::parse 已校验结构");
        for (cname, cval) in comps {
            let cschema = schema.component(cname).expect("Scene::parse 已校验组件存在");
            let mut sub = ValidationReport::default();
            let mut normalized =
                cschema.normalize(cval, &format!("{}#/entities/{i}/components/{cname}", scene.file), &mut sub);
            report.merge(sub);
            for (fname, fdef) in &cschema.fields {
                if matches!(fdef.ty, crate::FieldType::Entity) {
                    if let Some(refname) = normalized.get(fname).and_then(|v| v.as_str()).map(String::from) {
                        if let Ok(target) = world.entity(&refname) {
                            normalized[fname] = Value::String(target.to_string());
                        }
                        // Non-existent references were already reported by Scene::parse; not duplicated here
                    }
                }
            }
            world
                .set_component(ids[i], cname, normalized)
                .expect("实体刚创建必然存活");
        }
    }
    report.into_result(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Schema {
        Schema::parse(
            &json!({"components": {
                "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Follow": {"fields": {"target": {"type":"entity"}}},
                "Coin": {"fields": {"value": {"type":"int", "default": 1}}}
            }}),
            "schema.json",
        )
        .unwrap()
    }

    #[test]
    fn scene_validation_catches_everything_at_once() {
        let err = Scene::parse(
            json!({"entities": [
                {"name": "a", "components": {"Position": {"x": 1, "y": 2}}},
                {"name": "a", "components": {"Ghost": {}}},
                {"components": {"Follow": {"target": "nobody"}}}
            ]}),
            "scenes/main.json",
            &schema(),
        )
        .unwrap_err();
        let codes: Vec<&str> = err.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VD031"), "重名: {err}");
        assert!(codes.contains(&"VD005"), "未知组件: {err}");
        assert!(codes.contains(&"VD033"), "悬空引用: {err}");
    }

    #[test]
    fn emitter_schema_rejects_bad_fields_with_paths() {
        // The emitter component goes through exactly the same schema validation as other components: illegal kind (enum),
        // out-of-range spread, and unknown fields all get reported with full paths — vitric check's red lights come from here
        let s = Schema::parse(
            &json!({"components": {
                "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
                "Emitter": {"fields": {
                    "kind": {"type": "enum", "variants": ["stream", "burst"]},
                    "rate": {"type": "number", "default": 0, "min": 0},
                    "count": {"type": "int", "default": 0, "min": 0},
                    "burst": {"type": "int", "default": -1},
                    "lifetime": {"type": "int", "default": 30, "min": 1},
                    "speed_min": {"type": "number", "default": 0, "min": 0},
                    "speed_max": {"type": "number", "default": 0, "min": 0},
                    "dir": {"type": "number", "default": 0},
                    "spread": {"type": "number", "default": 360, "min": 0, "max": 360},
                    "gravity": {"type": "number", "default": 0},
                    "color": {"type": "text", "default": "#ffffff"},
                    "color_end": {"type": "text", "default": ""},
                    "size": {"type": "number", "default": 0.3, "min": 0},
                    "size_end": {"type": "number", "default": 0, "min": 0},
                    "active": {"type": "bool", "default": true}
                }}
            }}),
            "schema.json",
        )
        .unwrap();
        let err = Scene::parse(
            json!({"entities": [
                {"name": "sparks", "components": {
                    "Position": {"x": 0, "y": 0},
                    "Emitter": {"kind": "fountain", "spread": 720, "ttl": 9}
                }}
            ]}),
            "scenes/main.json",
            &s,
        )
        .unwrap_err();
        let text = err.to_string();
        // Illegal enum value (VD022): path points directly at the kind field
        assert!(
            text.contains("VD022") && text.contains("scenes/main.json#/entities/0/components/Emitter/kind"),
            "{text}"
        );
        // Out of range (VD021): spread > 360
        assert!(
            text.contains("VD021") && text.contains("Emitter/spread"),
            "{text}"
        );
        // Unknown field (VD003): ttl does not belong to Emitter
        assert!(
            text.contains("VD003") && text.contains("Emitter/ttl"),
            "{text}"
        );
        // A legal form still passes
        Scene::parse(
            json!({"entities": [
                {"name": "sparks", "components": {
                    "Position": {"x": 0, "y": 0},
                    "Emitter": {"kind": "stream", "rate": 20, "lifetime": 40, "size": 0.3}
                }}
            ]}),
            "scenes/main.json",
            &s,
        )
        .unwrap();
    }

    #[test]
    fn ui_semantic_errors_surface_with_paths() {
        // UI value-level validation goes through the full scene path: illegal anchor name / unknown container kind / Grid columns 0 all get
        // reported with path + VD07x code (schema fields pass type first, then UI semantics).
        let s = Schema::parse(
            &json!({"components": {
                "UiRoot": {"fields": {}},
                "Ui": {"fields": {
                    "anchor": {"type":"text", "default":"manual"},
                    "parent": {"type":"entity"},
                    "w": {"type":"number", "default":0}, "h": {"type":"number", "default":0}
                }},
                "Container": {"fields": {
                    "kind": {"type":"text", "default":"VBox"},
                    "columns": {"type":"int", "default":1}
                }}
            }}),
            "schema.json",
        )
        .unwrap();
        let err = Scene::parse(
            json!({"entities": [
                {"name": "ui", "components": {"UiRoot": {}}},
                {"name": "bad-anchor", "components": {"Ui": {"anchor": "top-middle"}}},
                {"name": "bad-box", "components": {"Container": {"kind": "Flex"}}},
                {"name": "bad-grid", "components": {"Container": {"kind": "Grid", "columns": 0}}}
            ]}),
            "scenes/main.json",
            &s,
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("VD070") && text.contains("Ui/anchor"), "非法锚点: {text}");
        assert!(text.contains("VD071") && text.contains("Container/kind"), "未知容器: {text}");
        assert!(text.contains("VD072") && text.contains("Container/columns"), "Grid 列数 0: {text}");
        // Legal UI still passes
        Scene::parse(
            json!({"entities": [
                {"name": "ui", "components": {"UiRoot": {}}},
                {"name": "p", "components": {
                    "Ui": {"anchor": "center", "parent": "ui", "w": 100, "h": 50},
                    "Container": {"kind": "Grid", "columns": 3}
                }}
            ]}),
            "scenes/main.json",
            &s,
        )
        .unwrap();
    }

    #[test]
    fn instantiate_resolves_refs_and_defaults() {
        let s = schema();
        let scene = Scene::parse(
            json!({"entities": [
                {"name": "player", "components": {"Position": {"x": 10, "y": 20}}},
                {"name": "pet", "components": {"Follow": {"target": "player"}}},
                {"components": {"Coin": {}}}
            ]}),
            "scenes/main.json",
            &s,
        )
        .unwrap();
        let mut w = World::new();
        instantiate_scene(&scene, &s, &mut w).unwrap();

        let player = w.entity("player").unwrap();
        let pet = w.entity("pet").unwrap();
        // Name reference resolved into a handle
        assert_eq!(
            w.get_field(pet, "Follow.target").unwrap(),
            &json!(player.to_string())
        );
        // Default values filled in
        let coins = w.query(&["Coin"]);
        assert_eq!(coins.len(), 1);
        assert_eq!(w.get_field(coins[0], "Coin.value").unwrap(), &json!(1));
    }
}
