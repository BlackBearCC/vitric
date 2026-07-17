//! `vitric turf` — turf enforcement.
//!
//! Stance: the first rule of the team protocol is "files are turf" — each role only writes
//! its own directory, and this discipline can't rely on subagents' self-awareness; the
//! engine must enforce it mechanically. Usage:
//!
//! ```text
//! vitric turf <project_dir> --role <role> <changed_files...>
//! ```
//!
//! Changed-file paths are interpreted **relative to the project directory** (absolute paths
//! are accepted but must fall inside the project). Any file outside the role's turf → the
//! report names each violation + exits 1; the prescription is always the same line:
//! cross-turf needs go through the event convention to the director, don't directly edit
//! someone else's files.
//!
//! The turf table is engine-defined (aligned word-for-word with the table in the repo's
//! team/README.md):
//! art=assets/+animations.json+palette.json | level=scenes/ | gameplay=rules/+scripts/
//! audio=sounds/ | narrative=scenes/ (narrative lives in scene Text, sharing the directory
//! with level)
//! qa=qa/+recordings/ | director=everything (GDD.md/schema.json/vitric.json only the director
//! can edit).

use std::path::Path;

use serde_json::{json, Value};

/// Role → (writable directories, writable single files). director is not in the table —
/// it can write everything inside the project.
const TURF: &[(&str, &[&str], &[&str])] = &[
    ("art", &["assets"], &["animations.json", "palette.json"]),
    ("level", &["scenes"], &[]),
    ("gameplay", &["rules", "scripts"], &[]),
    ("audio", &["sounds"], &[]),
    ("narrative", &["scenes"], &[]),
    ("qa", &["qa", "recordings"], &[]),
];

/// Run turf enforcement. Returns (JSON report, whether all in turf); Err is only for usage
/// errors (missing args / unknown role / nonexistent project dir) — these are hard errors,
/// not a pass=false report.
pub fn run(args: &[String]) -> Result<(Value, bool), String> {
    let dir = args.first().ok_or("turf 缺少项目目录参数。用法: vitric turf <项目目录> --role <角色> <改动文件...>")?;
    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        return Err(format!("项目目录 {dir} 不存在或不是目录"));
    }
    // Absolute-path changed files need a canonicalized project root to convert to relative paths
    let abs_dir = std::fs::canonicalize(dir_path)
        .map_err(|e| format!("项目目录 {dir} 无法规范化: {e}"))?
        .to_string_lossy()
        .replace('\\', "/");

    let mut role: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--role" {
            role = Some(args.get(i + 1).ok_or("--role 缺少参数值")?.clone());
            i += 2;
        } else {
            files.push(args[i].clone());
            i += 1;
        }
    }
    let role = role.ok_or_else(|| {
        format!("turf 缺少 --role 参数。可选角色: director, {}", role_names().join(", "))
    })?;
    if files.is_empty() {
        return Err("turf 缺少改动文件参数——把本次改动的文件列在命令末尾（路径相对项目目录）".to_string());
    }

    // Role validity: unknown role is a hard error listing all options, not silently treated as pass/fail
    let turf: Option<(&[&str], &[&str])> = if role == "director" {
        None
    } else {
        Some(
            TURF.iter()
                .find(|(name, _, _)| *name == role)
                .map(|(_, dirs, single)| (*dirs, *single))
                .ok_or_else(|| {
                    format!("未知角色 {role:?}。可选: director, {}", role_names().join(", "))
                })?,
        )
    };

    let mut violations: Vec<Value> = Vec::new();
    for file in &files {
        match normalize(file, &abs_dir) {
            Err(reason) => violations.push(json!({"file": file, "reason": reason})),
            Ok(rel) => {
                // director can write everything inside the project; other roles check the turf table
                if let Some((dirs, singles)) = turf {
                    if !in_turf(&rel, dirs, singles) {
                        violations.push(json!({
                            "file": file,
                            "reason": format!(
                                "不在 {role} 的地盘（可写: {}）——跨地盘需求走事件约定提给导演，不直接改别人的文件",
                                turf_display(dirs, singles)
                            ),
                        }));
                    }
                }
            }
        }
    }

    let pass = violations.is_empty();
    let report = json!({
        "role": role,
        "project_dir": dir,
        "turf": match turf {
            Some((dirs, singles)) => json!(turf_display(dirs, singles)),
            None => json!("全部（项目内一切文件；GDD.md/schema.json/vitric.json 只有导演能动）"),
        },
        "files": files.len(),
        "pass": pass,
        "violations": violations,
    });
    Ok((report, pass))
}

fn role_names() -> Vec<&'static str> {
    TURF.iter().map(|(name, _, _)| *name).collect()
}

fn turf_display(dirs: &[&str], singles: &[&str]) -> String {
    dirs.iter()
        .map(|d| format!("{d}/"))
        .chain(singles.iter().map(|f| f.to_string()))
        .collect::<Vec<_>>()
        .join(" + ")
}

fn in_turf(rel: &str, dirs: &[&str], singles: &[&str]) -> bool {
    singles.contains(&rel)
        || dirs.iter().any(|d| rel == *d || rel.starts_with(&format!("{d}/")))
}

/// Convert a changed-file path to a relative path inside the project ('/' separated).
/// Failure to convert = violation: absolute path not under the project, or relative path
/// with `..` escaping the project dir — each gets an explicit reason.
/// Pure lexical processing (no filesystem access) — the changed file may already be deleted,
/// canonicalize would fail.
fn normalize(file: &str, abs_dir: &str) -> Result<String, String> {
    let file = file.replace('\\', "/");
    // Absolute path (Unix / Windows drive letter): must fall under the project directory
    let is_abs = file.starts_with('/')
        || (file.len() >= 3 && file.as_bytes()[1] == b':' && file.as_bytes()[2] == b'/');
    let rel = if is_abs {
        // The remainder must start with '/' to truly be under the project dir (prevents /proj
        // colliding with the /proj-other prefix)
        match file.strip_prefix(abs_dir) {
            Some(r) if r.starts_with('/') && !r.trim_start_matches('/').is_empty() => {
                r.trim_start_matches('/').to_string()
            }
            _ => return Err(format!("在项目目录 {abs_dir} 之外——别的项目/仓库的文件不归本班子管")),
        }
    } else {
        file
    };
    // Lexical normalization: consume "." and ".."; popping empty = escape
    let mut parts: Vec<&str> = Vec::new();
    for comp in rel.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("路径用 .. 逃出了项目目录".to_string());
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Err("不是项目内的文件路径".to_string());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dots_and_flags_escapes() {
        assert_eq!(normalize("./scenes/main.json", "/proj").unwrap(), "scenes/main.json");
        assert_eq!(normalize("assets//x.png", "/proj").unwrap(), "assets/x.png");
        assert_eq!(normalize("scenes/../rules/a.json", "/proj").unwrap(), "rules/a.json");
        assert!(normalize("../outside.json", "/proj").unwrap_err().contains("逃出"));
        assert!(normalize("/etc/passwd", "/proj").unwrap_err().contains("之外"));
        assert_eq!(normalize("/proj/assets/x.png", "/proj").unwrap(), "assets/x.png");
        // Project root prefix collision doesn't count as inside: /proj-other isn't a subpath of /proj
        assert!(normalize("/proj-other/x.png", "/proj").unwrap_err().contains("之外"));
        // Windows style: backslashes and drive letters both convert
        assert_eq!(normalize("assets\\x.png", "/proj").unwrap(), "assets/x.png");
        assert!(normalize("C:/other/x.png", "C:/proj").unwrap_err().contains("之外"));
    }

    #[test]
    fn in_turf_matches_dirs_and_single_files() {
        let dirs: &[&str] = &["assets"];
        let singles: &[&str] = &["animations.json", "palette.json"];
        assert!(in_turf("assets/x.png", dirs, singles));
        assert!(in_turf("assets/sub/y.png", dirs, singles));
        assert!(in_turf("palette.json", dirs, singles));
        assert!(!in_turf("scenes/main.json", dirs, singles));
        // Prefix collision doesn't count: assets_original/ is not assets/
        assert!(!in_turf("assets_original/x.png", dirs, singles));
    }
}
