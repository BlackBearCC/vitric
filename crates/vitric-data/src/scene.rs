use serde_json::Value;

use vitric_ecs::World;

use crate::{Schema, ValidationReport};

/// 场景文件：实体列表，每个实体可命名、挂组件。
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
    /// 解析 + 按 schema 校验（不落地到 World，只验数据）。
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
    // 收集实体名：查重 + 给 entity 引用校验用
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
            // entity 类型字段引用的实体名必须在场景里存在
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
        }
    }
}

/// 把校验过的场景落地进 World：建实体、填归一化后的组件值。
///
/// 实体引用（entity 字段里的名字）在落地时解析成运行时句柄 "e<i>v<g>"——
/// 场景文件用名字（人/AI 好写），运行时用句柄（精确无歧义）。
pub fn instantiate_scene(scene: &Scene, schema: &Schema, world: &mut World) -> Result<(), ValidationReport> {
    let mut report = ValidationReport::default();
    let entities = scene
        .doc
        .get("entities")
        .and_then(|v| v.as_array())
        .expect("Scene::parse 已校验结构");

    // 第一遍：建实体（先全部建出来，引用解析才有目标）
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

    // 第二遍：填组件，entity 字段的名字换成句柄
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
                        // 不存在的引用 Scene::parse 已报过，这里不重复
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
        // 名字引用解析成了句柄
        assert_eq!(
            w.get_field(pet, "Follow.target").unwrap(),
            &json!(player.to_string())
        );
        // 默认值填上了
        let coins = w.query(&["Coin"]);
        assert_eq!(coins.len(), 1);
        assert_eq!(w.get_field(coins[0], "Coin.value").unwrap(), &json!(1));
    }
}
