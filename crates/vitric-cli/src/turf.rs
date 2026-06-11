//! `vitric turf` — 地盘执法。
//!
//! 立场：班子协议第一条是"文件即地盘"——每个角色只写自己的目录，这条纪律
//! 不能靠 subagent 自觉，要引擎机械执法。用法：
//!
//! ```text
//! vitric turf <项目目录> --role <角色> <改动文件...>
//! ```
//!
//! 改动文件路径**相对项目目录**解释（绝对路径也认，但必须落在项目内）。
//! 任何文件越出该角色的地盘 → 报告里逐条点名 + 退出 1；处方永远是同一句：
//! 跨地盘需求走事件约定提给导演，不直接改别人的文件。
//!
//! 地盘表是引擎定义的（与仓库 team/README.md 的表一字一句对齐）：
//! art=assets/+animations.json+palette.json | level=scenes/ | gameplay=rules/+scripts/
//! audio=sounds/ | narrative=scenes/（文案住在场景 Text 里，与 level 共享目录）
//! qa=qa/+recordings/ | director=一切（GDD.md/schema.json/vitric.json 只有导演能动）。

use std::path::Path;

use serde_json::{json, Value};

/// 角色 → (可写目录, 可写单文件)。director 不在表里——它可写项目内一切。
const TURF: &[(&str, &[&str], &[&str])] = &[
    ("art", &["assets"], &["animations.json", "palette.json"]),
    ("level", &["scenes"], &[]),
    ("gameplay", &["rules", "scripts"], &[]),
    ("audio", &["sounds"], &[]),
    ("narrative", &["scenes"], &[]),
    ("qa", &["qa", "recordings"], &[]),
];

/// 跑地盘执法。返回 (JSON 报告, 是否全在地盘内)；Err 只用于用法错误
/// （缺参数/未知角色/项目目录不存在）——这些是硬错误，不是一份 pass=false 的报告。
pub fn run(args: &[String]) -> Result<(Value, bool), String> {
    let dir = args.first().ok_or("turf 缺少项目目录参数。用法: vitric turf <项目目录> --role <角色> <改动文件...>")?;
    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        return Err(format!("项目目录 {dir} 不存在或不是目录"));
    }
    // 绝对路径的改动文件要靠规范化的项目根来折算成相对路径
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

    // 角色合法性：未知角色是硬错误并列出全部可选项，不静默当 pass/fail
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
                // director 可写项目内一切；其他角色查地盘表
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

/// 把一个改动文件路径折算成项目内的相对路径（'/' 分隔）。
/// 折算不动 = 违规：绝对路径不在项目下、相对路径 `..` 逃出项目目录，都给显式理由。
/// 纯词法处理（不碰文件系统）——改动文件可能已被删除，canonicalize 会失败。
fn normalize(file: &str, abs_dir: &str) -> Result<String, String> {
    let file = file.replace('\\', "/");
    // 绝对路径（Unix / Windows 盘符）：必须落在项目目录下
    let is_abs = file.starts_with('/')
        || (file.len() >= 3 && file.as_bytes()[1] == b':' && file.as_bytes()[2] == b'/');
    let rel = if is_abs {
        // 剩余部分必须以 '/' 开头才算真的在项目目录下（防 /proj 撞上 /proj-other 前缀）
        match file.strip_prefix(abs_dir) {
            Some(r) if r.starts_with('/') && !r.trim_start_matches('/').is_empty() => {
                r.trim_start_matches('/').to_string()
            }
            _ => return Err(format!("在项目目录 {abs_dir} 之外——别的项目/仓库的文件不归本班子管")),
        }
    } else {
        file
    };
    // 词法归一：消化 "." 与 ".."，弹空即逃逸
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
        // 项目根的前缀撞名不算项目内：/proj-other 不是 /proj 的子路径
        assert!(normalize("/proj-other/x.png", "/proj").unwrap_err().contains("之外"));
        // Windows 风格：反斜杠与盘符都折算
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
        // 前缀撞名不算：assets_original/ 不是 assets/
        assert!(!in_turf("assets_original/x.png", dirs, singles));
    }
}
