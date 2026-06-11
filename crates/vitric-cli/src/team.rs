//! `vitric team` — 多 agent 班子的协同黑板。
//!
//! 立场：班子里每个角色（美术/关卡/玩法/音频/文案/QA）的交付物都是项目目录里的
//! 文件，所以"谁交了什么、还卡在哪"可以从文件**机械地**读出来，不靠各角色自述。
//! 本命令只读不判：报告每个角色交付物的在场/健康度 + 合同（GDD/schema）+ 门禁声明
//! 状态，结尾给"卡点提示"（blocking）。它是状态工具不是门——**永远退出 0**；
//! 真正的交付裁决归 `vitric gate`（这里只报门禁声明了没有、录像文件在不在，
//! 不重放录像——重复裁决会让两边打架）。
//!
//! 约束：黑板必须在项目残缺时也能用（立项第一天 vitric.json 都没有也要能报状态），
//! 所以一切计数从文件直接派生、解析失败降级成显式的 *_error 字段进报告，
//! 不会因为某个角色交了坏文件就整个命令报错。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use vitric_data::{Project, Schema};

use crate::runtime::Runtime;

/// 出黑板报告。Err 只用于"项目目录本身不存在"——黑板可以面对残缺项目，
/// 但不能面对不存在的目录。
pub fn run(dir: &Path) -> Result<Value, String> {
    if !dir.is_dir() {
        return Err(format!(
            "项目目录 {} 不存在或不是目录。提示：vitric team <项目目录>（含 vitric.json 的目录）",
            dir.display()
        ));
    }

    let mut blocking: Vec<String> = Vec::new();

    // ---- 合同：GDD + 清单 + schema ----
    let gdd = dir.join("GDD.md").is_file();
    if !gdd {
        blocking.push("缺 GDD.md——全队没有合同，先由导演立项（骨架在引擎仓库 team/templates/GDD-template.md）".to_string());
    }

    let (manifest_doc, manifest_error) = match read_json(&dir.join("vitric.json")) {
        Ok(doc) => (Some(doc), None),
        Err(e) => (None, Some(e)),
    };
    if let Some(e) = &manifest_error {
        blocking.push(format!("vitric.json 不可用（{e}）——项目跑不起来，归导演"));
    }

    // schema 路径以清单为准（清单不可用时按惯例 schema.json）
    let schema_rel = manifest_doc
        .as_ref()
        .and_then(|m| m.get("schema"))
        .and_then(|v| v.as_str())
        .unwrap_or("schema.json")
        .to_string();
    let schema_error: Option<String> = match read_json(&dir.join(&schema_rel)) {
        Ok(doc) => Schema::parse(&doc, &schema_rel).err().map(|r| r.to_string()),
        Err(e) => Some(e),
    };
    if let Some(e) = &schema_error {
        blocking.push(format!("schema（{schema_rel}）不可用——组件字段是全队接口，归导演修：{e}"));
    }
    let mut contract = json!({
        "gdd": gdd,
        "manifest": manifest_error.is_none(),
        "schema_parses": schema_error.is_none(),
    });
    if let Some(e) = manifest_error {
        contract["manifest_error"] = json!(e);
    }
    if let Some(e) = schema_error {
        contract["schema_error"] = json!(e);
    }

    // ---- 美术：素材数 / 色板 / 法线 ----
    let asset_files = files_under(&dir.join("assets"));
    let normals = asset_files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase().ends_with("_n.png"))
        })
        .count();
    let assets = asset_files
        .iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png"))
        })
        .count()
        - normals;
    let palette = dir.join("palette.json").is_file();
    if !palette {
        blocking.push("美术缺 palette.json，其他角色的视觉基调悬空（vitric assets <项目目录> --colors N 生成）".to_string());
    }

    // ---- 关卡 + 文案：场景实体数 / Text 实体数（两个角色共享 scenes/，黑板分开报）----
    let mut scene_files = 0usize;
    let mut entities = 0usize;
    let mut text_entities = 0usize;
    let mut scene_errors: Vec<String> = Vec::new();
    for path in files_under(&dir.join("scenes")) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        scene_files += 1;
        let rel = rel_display(dir, &path);
        match read_json(&path) {
            Ok(doc) => {
                let list = doc.get("entities").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                entities += list.len();
                text_entities += list
                    .iter()
                    .filter(|e| e.get("components").and_then(|c| c.get("Text")).is_some())
                    .count();
            }
            Err(e) => scene_errors.push(format!("{rel}: {e}")),
        }
    }
    if entities == 0 && scene_errors.is_empty() {
        blocking.push("关卡为空（scenes/ 里没有实体）——没有可玩骨架，灰盒先立起来".to_string());
    }

    // ---- 玩法：规则条数（文件直读）+ 脚本 systems/fns（复用 check 的装配内核）----
    let mut rule_count = 0usize;
    let mut rule_errors: Vec<String> = Vec::new();
    let mut rule_docs: Vec<Value> = Vec::new();
    for path in files_under(&dir.join("rules")) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let rel = rel_display(dir, &path);
        match read_json(&path) {
            Ok(doc) => {
                rule_count += doc.get("rules").and_then(|v| v.as_array()).map_or(0, |a| a.len());
                rule_docs.push(doc);
            }
            Err(e) => rule_errors.push(format!("{rel}: {e}")),
        }
    }
    // systems/fns 必须真的求值脚本才知道（vitric.system 注册发生在执行期），
    // 所以这里复用 check 同款装配：Project::load + Runtime::build。
    // 装配失败不挡黑板——记成 load_error，计数置 null（未知，不是 0）。
    let (systems, fns, load_error) = match Project::load(dir)
        .map_err(|r| r.to_string())
        .and_then(|p| Runtime::build(&p))
    {
        Ok(rt) => (json!(rt.scripts.systems.len()), json!(rt.scripts.fns.len()), None),
        Err(e) => (Value::Null, Value::Null, Some(e)),
    };
    if rule_count == 0 && load_error.is_none() && systems == json!(0) {
        blocking.push("玩法零规则零系统——游戏没有逻辑".to_string());
    }
    let mut gameplay = json!({"rules": rule_count, "systems": systems, "fns": fns});
    if !rule_errors.is_empty() {
        gameplay["rule_errors"] = json!(rule_errors);
    }
    if let Some(e) = load_error {
        gameplay["load_error"] = json!(e);
    }

    // ---- 音频：在场文件 vs 规则字面引用 ----
    let sounds_dir = dir.join("sounds");
    let sounds = files_under(&sounds_dir).len();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for doc in &rule_docs {
        collect_sound_refs(doc, &mut referenced);
    }
    let missing_sounds: Vec<&String> =
        referenced.iter().filter(|s| !sounds_dir.join(s.as_str()).is_file()).collect();
    if !missing_sounds.is_empty() {
        blocking.push(format!(
            "规则引用了 {} 个不存在的音效（{}）——音频补文件或玩法改引用",
            missing_sounds.len(),
            missing_sounds.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }

    // ---- QA：断言集 + 录像库（qa/ 里除 asserts.json 外的 .json 都按录像计）----
    let asserts = dir.join("qa/asserts.json").is_file();
    let recordings = files_under(&dir.join("qa"))
        .iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("json")
                && p.file_name().and_then(|n| n.to_str()) != Some("asserts.json")
        })
        .count()
        + files_under(&dir.join("recordings"))
            .iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .count();

    // ---- 门禁声明（只报声明与录像文件在场——重放裁决是 vitric gate 的事）----
    let gates_doc = manifest_doc.as_ref().and_then(|m| m.get("gates")).cloned();
    let gates = match gates_doc {
        Some(g) => {
            let playthroughs =
                g.get("playthroughs").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let recordings_missing: Vec<String> = playthroughs
                .iter()
                .filter_map(|p| p.get("recording").and_then(|v| v.as_str()))
                .filter(|rel| !dir.join(rel).is_file())
                .map(|s| s.to_string())
                .collect();
            if playthroughs.is_empty() {
                blocking.push("gates.playthroughs 为空——没有录像就没有证书，gate 必拒".to_string());
            }
            if !recordings_missing.is_empty() {
                blocking.push(format!(
                    "门禁录像缺失（{}）——gate 必红，QA/导演先录通关",
                    recordings_missing.join(", ")
                ));
            }
            let mut out = json!({
                "declared": true,
                "playthroughs": playthroughs.len(),
                "recordings_missing": recordings_missing,
            });
            if let Some(rel) = g.get("assertions").and_then(|v| v.as_str()) {
                let present = dir.join(rel).is_file();
                out["assertions"] = json!(rel);
                out["assertions_present"] = json!(present);
                if !present {
                    blocking.push(format!("gates.assertions 指向的 {rel} 不存在——断言门必红，归 QA"));
                }
            }
            out
        }
        None => {
            blocking.push("清单未声明 gates——vitric gate 不出证书，交付没有机器裁决".to_string());
            json!({"declared": false})
        }
    };

    let mut level = json!({"scenes": scene_files, "entities": entities});
    if !scene_errors.is_empty() {
        level["scene_errors"] = json!(scene_errors);
    }

    Ok(json!({
        "project": manifest_doc.as_ref().and_then(|m| m.get("name")).cloned().unwrap_or(Value::Null),
        "contract": contract,
        "roles": {
            "art": {"assets": assets, "palette": palette, "normals": normals},
            "level": level,
            "gameplay": gameplay,
            "audio": {"sounds": sounds, "referenced": referenced.len(), "missing": missing_sounds},
            "narrative": {"text_entities": text_entities},
            "qa": {"asserts": asserts, "recordings": recordings},
        },
        "gates": gates,
        "blocking": blocking,
    }))
}

/// 递归列目录下全部文件（目录不存在 = 空，不是错——立项早期大半目录都还没有）。
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn rel_display(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir).unwrap_or(path).display().to_string()
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败: {e}"))
}

/// 收集规则文档里 play-sound / play-music 的字面音效引用（运行时引用
/// self./other./event./@ 不算——和 check 的扫描豁免同一条规则）。
fn collect_sound_refs(doc: &Value, out: &mut BTreeSet<String>) {
    match doc {
        Value::Object(map) => {
            if matches!(map.get("emit").and_then(|v| v.as_str()), Some("play-sound" | "play-music")) {
                if let Some(sound) =
                    map.get("data").and_then(|d| d.get("sound")).and_then(|s| s.as_str())
                {
                    let is_ref = sound.starts_with("self.")
                        || sound.starts_with("other.")
                        || sound.starts_with("event.")
                        || sound.starts_with('@');
                    if !is_ref && !sound.is_empty() {
                        out.insert(sound.to_string());
                    }
                }
            }
            for v in map.values() {
                collect_sound_refs(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_sound_refs(v, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sound_ref_collector_takes_literals_and_skips_runtime_refs() {
        let mut out = BTreeSet::new();
        collect_sound_refs(
            &json!([
                {"emit": "play-sound", "data": {"sound": "coin.wav"}},
                {"emit": "play-music", "data": {"sound": "bgm.ogg"}},
                {"emit": "play-sound", "data": {"sound": "event.sfx"}},
                {"emit": "stop-music", "data": {}},
            ]),
            &mut out,
        );
        assert_eq!(
            out.iter().cloned().collect::<Vec<_>>(),
            vec!["bgm.ogg".to_string(), "coin.wav".to_string()]
        );
    }

    #[test]
    fn nonexistent_dir_is_an_explicit_error() {
        let err = run(Path::new("/nonexistent/vitric-team")).unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }
}
