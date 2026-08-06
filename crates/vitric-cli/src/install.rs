//! `vitric install <module>` — install a gameplay module from the built-in registry into a project.
//!
//! Copies the module directory into `<project>/modules/<name>/` and appends the include path to
//! `vitric.json`'s `includes` array. Idempotent: if the module is already in includes, it is a no-op
//! (the directory is still refreshed). The built-in registry is the engine's own `modules/` directory;
//! remote registries are a future extension (the format is designed for it: registry.json maps names to sources).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
/// One entry in the module registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Module name (what the user types: `vitric install inventory`).
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Source: `"builtin"` = copy from the engine's bundled modules/ dir.
    /// Future: `"git"` = clone from a URL, `"path"` = copy from a local path.
    #[serde(default = "default_source")]
    pub source: String,
    /// For `source: "builtin"`: the directory name under the engine's `modules/`.
    /// For `source: "git"`: the clone URL.
    /// For `source: "path"`: an absolute or relative path.
    #[serde(default)]
    pub path: Option<String>,
}

fn default_source() -> String {
    "builtin".to_string()
}

/// The built-in registry (loaded from `modules/registry.json` next to the engine binary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub modules: Vec<RegistryEntry>,
}

impl Registry {
    /// Load the built-in registry from `<engine_modules>/registry.json`.
    /// The engine modules directory is resolved relative to the executable (same as how `team/` skills are found).
    pub fn load_builtin(engine_modules: &Path) -> Result<Self, String> {
        let registry_path = engine_modules.join("registry.json");
        let text = fs::read_to_string(&registry_path)
            .map_err(|e| format!("读取模块注册表 {} 失败: {e}", registry_path.display()))?;
        let registry: Registry = serde_json::from_str(&text)
            .map_err(|e| format!("解析模块注册表失败: {e}"))?;
        Ok(registry)
    }

    /// Look up a module by name.
    pub fn find(&self, name: &str) -> Option<&RegistryEntry> {
        self.modules.iter().find(|m| m.name == name)
    }

    /// List all available modules (for `vitric install --list`).
    pub fn list(&self) -> Vec<(&str, &str)> {
        self.modules
            .iter()
            .map(|m| (m.name.as_str(), m.description.as_str()))
            .collect()
    }
}

/// Find the engine's bundled modules directory.
/// The executable is at `<repo>/target/release/vitric` (or debug); modules are at `<repo>/modules/`.
/// We walk up from the executable to find a `modules/` directory containing `registry.json`.
fn find_engine_modules() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位引擎可执行文件: {e}"))?;
    let mut dir = exe.parent().ok_or("引擎可执行文件没有父目录")?;
    // Walk up at most 5 levels (target/release → target → repo)
    for _ in 0..5 {
        let candidate = dir.join("modules");
        if candidate.join("registry.json").exists() {
            return Ok(candidate);
        }
        dir = match dir.parent() {
            Some(p) => p,
            None => break,
        };
    }
    Err("找不到引擎内置模块目录（modules/registry.json）。如果你是从源码运行的，请确保在仓库根目录下执行。".to_string())
}

/// Run the install command.
///
/// ```bash
/// vitric install <module>        # install <module> into the project in the current directory
/// vitric install --list          # list available modules
/// vitric install <module> --project <dir>  # specify project directory
/// ```
pub fn run(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err(usage());
    }

    // Parse args
    let mut module_name: Option<String> = None;
    let mut project_dir: Option<PathBuf> = None;
    let mut list_mode = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--list" | "-l" => {
                list_mode = true;
                i += 1;
            }
            "--project" | "-p" => {
                project_dir = Some(PathBuf::from(
                    args.get(i + 1).ok_or("--project 缺少参数值")?,
                ));
                i += 2;
            }
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(());
            }
            other if !other.starts_with("--") => {
                module_name = Some(other.to_string());
                i += 1;
            }
            other => {
                return Err(format!("未知选项 {other:?}。{USAGE_TIP}"));
            }
        }
    }

    let engine_modules = find_engine_modules()?;
    let registry = Registry::load_builtin(&engine_modules)?;

    if list_mode {
        println!("{}", serde_json::to_string_pretty(&registry.list()).map_err(|e| e.to_string())?);
        return Ok(());
    }

    let name = module_name.ok_or_else(|| format!("缺少模块名。{USAGE_TIP}"))?;
    let entry = registry.find(&name).ok_or_else(|| {
        let available: Vec<&str> = registry.modules.iter().map(|m| m.name.as_str()).collect();
        format!("模块 {name:?} 不在注册表中。可用模块: {}", available.join(", "))
    })?;

    let project = project_dir.unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = project.join("vitric.json");
    if !manifest_path.exists() {
        return Err(
            "当前目录没有 vitric.json。请在项目根目录执行，或用 --project <dir> 指定。".to_string(),
        );
    }

    // Copy module directory
    let source_dir = match entry.source.as_str() {
        "builtin" => {
            let dir_name = entry.path.as_ref().unwrap_or(&entry.name);
            engine_modules.join(dir_name)
        }
        "path" => {
            let p = entry.path.as_ref().ok_or("path 类型模块缺少 path 字段")?;
            PathBuf::from(p)
        }
        other => return Err(format!("暂不支持的模块来源: {other}（当前仅支持 builtin）")),
    };

    if !source_dir.exists() {
        return Err(format!("模块源目录不存在: {}", source_dir.display()));
    }

    let dest_dir = project.join("modules").join(&entry.name);
    if dest_dir.exists() {
        // Refresh: remove old copy, then copy fresh
        fs::remove_dir_all(&dest_dir)
            .map_err(|e| format!("清理旧模块目录失败: {e}"))?;
    } else {
        fs::create_dir_all(dest_dir.parent().unwrap())
            .map_err(|e| format!("创建 modules 目录失败: {e}"))?;
    }

    copy_dir_recursive(&source_dir, &dest_dir)?;

    // Update vitric.json includes
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读取 vitric.json 失败: {e}"))?;
    let mut manifest: serde_json::Value = serde_json::from_str(&manifest_text)
        .map_err(|e| format!("解析 vitric.json 失败: {e}"))?;

    let include_path = format!("modules/{}", entry.name);
    let includes = manifest
        .get_mut("includes")
        .and_then(|v| v.as_array_mut());

    match includes {
        Some(arr) => {
            let already = arr.iter().any(|v| {
                v.as_str().map(|s| s == include_path).unwrap_or(false)
            });
            if !already {
                arr.push(serde_json::Value::String(include_path.clone()));
            }
        }
        None => {
            manifest["includes"] = serde_json::json!([include_path]);
        }
    }

    let updated = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("序列化 vitric.json 失败: {e}"))?;
    fs::write(&manifest_path, updated)
        .map_err(|e| format!("写入 vitric.json 失败: {e}"))?;

    println!(
        "{}",
        serde_json::json!({
            "installed": entry.name,
            "description": entry.description,
            "path": dest_dir.display().to_string(),
            "include": include_path,
            "next": format!("已自动加入 vitric.json 的 includes。运行 vitric check {} 验证。", project.display()),
        })
    );

    Ok(())
}

const USAGE_TIP: &str = "用法: vitric install <模块名> [--project <dir>] 或 vitric install --list";

fn usage() -> String {
    USAGE_TIP.to_string()
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("创建目录 {} 失败: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("读取目录 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("读取目录项失败: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("复制 {} → {} 失败: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_parse() {
        let json = r#"{
            "modules": [
                {"name": "inventory", "description": "拾取/堆叠/溢出/转移", "source": "builtin"},
                {"name": "combat", "description": "HP/攻击/伤害/死亡/治疗", "source": "builtin"}
            ]
        }"#;
        let r: Registry = serde_json::from_str(json).unwrap();
        assert_eq!(r.modules.len(), 2);
        assert_eq!(r.find("inventory").unwrap().description, "拾取/堆叠/溢出/转移");
        assert!(r.find("nonexistent").is_none());
    }

    #[test]
    fn registry_list() {
        let r = Registry {
            modules: vec![
                RegistryEntry { name: "a".into(), description: "desc a".into(), source: "builtin".into(), path: None },
                RegistryEntry { name: "b".into(), description: "desc b".into(), source: "builtin".into(), path: None },
            ],
        };
        let list = r.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], ("a", "desc a"));
    }
}
