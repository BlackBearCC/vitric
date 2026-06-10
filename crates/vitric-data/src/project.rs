use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::{Scene, Schema, ValidationReport};

/// 项目清单 `vitric.json`。
///
/// ```json
/// {
///   "name": "coin-run",
///   "schema": "schema.json",
///   "entry": "scenes/main.json",
///   "scenes": ["scenes/main.json"],
///   "rules": ["rules/game.json"],
///   "scripts": ["scripts/systems.js"],
///   "seed": 42
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectManifest {
    pub name: String,
    pub schema: String,
    /// 启动场景，必须出现在 scenes 列表里。
    pub entry: String,
    #[serde(default)]
    pub scenes: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    /// 世界随机种子；同种子同输入 = 同结果。
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_seed() -> u64 {
    0
}

/// 加载完成的项目：清单 + schema + 全部场景（已校验）+ 规则/脚本原文。
///
/// 规则的语义校验在 vitric-rules（它认识触发器/动作的结构）；
/// 这里只保证 JSON 能解析，职责分明。
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub schema: Schema,
    /// 相对路径 -> 场景
    pub scenes: BTreeMap<String, Scene>,
    /// (相对路径, 规则文档)
    pub rules: Vec<(String, Value)>,
    /// (相对路径, 脚本源码)
    pub scripts: Vec<(String, String)>,
}

impl Project {
    /// 从目录加载整个项目。所有问题（IO/解析/校验）汇总成一份报告一次给全。
    pub fn load(root: impl AsRef<Path>) -> Result<Project, ValidationReport> {
        let root = root.as_ref().to_path_buf();
        let mut report = ValidationReport::default();

        // 清单
        let manifest_path = root.join("vitric.json");
        let manifest_doc = match read_json(&manifest_path) {
            Ok(v) => v,
            Err(e) => {
                report.push("VD040", "vitric.json", e, "项目根目录必须有 vitric.json 清单");
                return Err(report);
            }
        };
        let manifest: ProjectManifest = match serde_json::from_value(manifest_doc) {
            Ok(m) => m,
            Err(e) => {
                report.push(
                    "VD041",
                    "vitric.json",
                    format!("清单解析失败: {e}"),
                    "必填字段: name(文本)、schema(路径)、entry(路径)。可选: scenes/rules/scripts(路径数组)、seed(整数)",
                );
                return Err(report);
            }
        };
        if !manifest.scenes.contains(&manifest.entry) {
            report.push(
                "VD042",
                "vitric.json#/entry",
                format!("入口场景 {:?} 不在 scenes 列表里", manifest.entry),
                "把它加进 scenes 数组",
            );
        }

        // schema
        let schema = match read_json(&root.join(&manifest.schema)) {
            Ok(doc) => match Schema::parse(&doc, &manifest.schema) {
                Ok(s) => s,
                Err(r) => {
                    report.merge(r);
                    Schema::default()
                }
            },
            Err(e) => {
                report.push("VD040", &manifest.schema, e, "清单 schema 字段指向的文件必须存在");
                Schema::default()
            }
        };

        // 场景
        let mut scenes = BTreeMap::new();
        for rel in &manifest.scenes {
            match read_json(&root.join(rel)) {
                Ok(doc) => match Scene::parse(doc, rel, &schema) {
                    Ok(s) => {
                        scenes.insert(rel.clone(), s);
                    }
                    Err(r) => report.merge(r),
                },
                Err(e) => report.push("VD040", rel, e, "清单 scenes 列表里的文件必须存在"),
            }
        }

        // 规则（仅解析 JSON，语义校验归 vitric-rules）
        let mut rules = Vec::new();
        for rel in &manifest.rules {
            match read_json(&root.join(rel)) {
                Ok(doc) => rules.push((rel.clone(), doc)),
                Err(e) => report.push("VD040", rel, e, "清单 rules 列表里的文件必须存在"),
            }
        }

        // 脚本（源码原文，执行归 vitric-script）
        let mut scripts = Vec::new();
        for rel in &manifest.scripts {
            match fs::read_to_string(root.join(rel)) {
                Ok(src) => scripts.push((rel.clone(), src)),
                Err(e) => report.push("VD040", rel, format!("读取失败: {e}"), "清单 scripts 列表里的文件必须存在"),
            }
        }

        report.into_result(Project { root, manifest, schema, scenes, rules, scripts })
    }

    pub fn entry_scene(&self) -> &Scene {
        &self.scenes[&self.manifest.entry]
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败（第 {} 行第 {} 列）: {e}", e.line(), e.column()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vitric-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_full_project() {
        let dir = temp_project("load");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"seed":7}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}}}}"#,
        );
        write(
            &dir.join("scenes/main.json"),
            r#"{"entities":[{"name":"player","components":{"Position":{"x":1,"y":2}}}]}"#,
        );
        let p = Project::load(&dir).unwrap();
        assert_eq!(p.manifest.name, "demo");
        assert_eq!(p.manifest.seed, 7);
        assert!(p.entry_scene().doc.get("entities").is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn all_problems_reported_in_one_pass() {
        let dir = temp_project("problems");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/missing.json",
                "scenes":["scenes/bad.json"],"rules":["rules/none.json"]}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"P":{"fields":{"x":{"type":"number"}}}}}"#,
        );
        write(&dir.join("scenes/bad.json"), r#"{"entities":[{"components":{"Nope":{}}}]}"#);
        let err = Project::load(&dir).unwrap_err();
        let codes: Vec<&str> = err.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VD042"), "入口不在列表: {err}");
        assert!(codes.contains(&"VD005"), "未知组件: {err}");
        assert!(codes.contains(&"VD040"), "规则文件缺失: {err}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
