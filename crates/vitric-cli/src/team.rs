//! `vitric team` — the collaboration board for a multi-agent team.
//!
//! Stance: each role in the team (art/level/gameplay/audio/narrative/QA) delivers files
//! in the project directory, so "who delivered what, what's still stuck" can be read
//! **mechanically** from files, not from each role's self-report. This command only reads
//! and does not judge: it reports each role's deliverables' presence/health + contract
//! (GDD/schema) + gate declaration status, ending with "blocking" hints. It's a status
//! tool, not a gate — **always exits 0**; the actual delivery verdict belongs to
//! `vitric gate` (here we only report whether the gate declaration exists and whether the
//! recording files are present — we don't replay recordings, because double-judging would
//! cause the two sides to disagree).
//!
//! Constraint: the board must work even when the project is incomplete (day one, before
//! vitric.json even exists, it must still report status), so all counts derive directly
//! from files; parse failures degrade into explicit *_error fields in the report — a bad
//! file from one role never makes the whole command error out.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use vitric_data::{Project, Schema};

use crate::runtime::Runtime;

/// Produce the board report. Err is only used for "project directory itself doesn't exist" —
/// the board can face an incomplete project, but not a nonexistent directory.
pub fn run(dir: &Path) -> Result<Value, String> {
    if !dir.is_dir() {
        return Err(format!(
            "项目目录 {} 不存在或不是目录。提示：vitric team <项目目录>（含 vitric.json 的目录）",
            dir.display()
        ));
    }

    let mut blocking: Vec<String> = Vec::new();

    // ---- Contract: GDD + manifest + schema ----
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

    // Schema path follows the manifest (when manifest is unusable, fall back to schema.json by convention)
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

    // ---- Art: asset count / palette / normals ----
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

    // ---- Level + Narrative: scene entity count / Text entity count (two roles share scenes/, the board reports separately) ----
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

    // ---- Gameplay: rule count (direct file read) + script systems/fns (reuse check's assembly kernel) ----
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
    // systems/fns can only be known by actually evaluating the script (vitric.system
    // registration happens at execution time), so reuse the same assembly as check:
    // Project::load + Runtime::build.
    // Assembly failure doesn't block the board — recorded as load_error, counts set to
    // null (unknown, not 0).
    let (systems, fns, load_error) = match Project::load(dir)
        .map_err(|r| r.to_string())
        .and_then(|p| Runtime::build(&p))
    {
        // fns only counts author-written gameplay functions, excluding `__`-prefixed
        // engine built-ins (like the reply dispatcher __onReply for ctx.ask)
        Ok(rt) => (
            json!(rt.scripts.systems.len()),
            json!(rt.scripts.fns.iter().filter(|f| !f.starts_with("__")).count()),
            None,
        ),
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

    // ---- Audio: present files vs literal references in rules ----
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

    // ---- QA: assertion set + recording library (every .json in qa/ except asserts.json counts as a recording) ----
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

    // ---- Gate declaration (only reports declaration presence and recording file presence — replay verdict is vitric gate's job) ----
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

/// Recursively list all files under a directory (nonexistent directory = empty, not an error —
/// most directories don't exist yet early in a project).
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

/// Collect literal sound references to play-sound / play-music in rule documents (runtime
/// references self./other./event./@ don't count — same scan exemption rule as check).
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
