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
///
/// Resolution order:
/// 1. `VITRIC_MODULES` env var (explicit override — for release binaries or custom layouts)
/// 2. Walk up from the executable to find a `modules/` dir containing `registry.json`
///    (works in dev: `target/release/vitric` → walk up → `<repo>/modules/`)
fn find_engine_modules() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("VITRIC_MODULES") {
        let p = PathBuf::from(&dir);
        if p.join("registry.json").exists() {
            return Ok(p);
        }
        return Err(format!("VITRIC_MODULES={dir} 但该目录下没有 registry.json"));
    }

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
    Err("找不到引擎内置模块目录。请设置 VITRIC_MODULES 环境变量指向 modules/ 目录，或在仓库根目录下执行。".to_string())
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

    // Update vitric.json includes — text-based patching preserves original key order & formatting
    let include_path = format!("modules/{}", entry.name);
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读取 vitric.json 失败: {e}"))?;
    let updated_text = patch_includes(&manifest_text, &include_path)?;
    if updated_text != manifest_text {
        fs::write(&manifest_path, &updated_text)
            .map_err(|e| format!("写入 vitric.json 失败: {e}"))?;
    }

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

/// Patch the `includes` array in a vitric.json text, preserving original key order and formatting.
///
/// - If `includes` already contains the path → no-op (return original text unchanged).
/// - If `includes` exists but doesn't contain the path → append to the array (text-level insertion
///   before the closing `]`).
/// - If `includes` doesn't exist → insert a new `"includes": ["<path>"]` entry before the final `}`.
fn patch_includes(json_text: &str, include_path: &str) -> Result<String, String> {
    // Parse to check if already present
    let parsed: serde_json::Value = serde_json::from_str(json_text)
        .map_err(|e| format!("解析 vitric.json 失败: {e}"))?;
    let root = parsed
        .as_object()
        .ok_or("vitric.json 顶层不是对象")?;

    // Already present?
    if let Some(includes) = root.get("includes").and_then(|v| v.as_array()) {
        if includes.iter().any(|v| {
            v.as_str().map(|s| s == include_path).unwrap_or(false)
        }) {
            return Ok(json_text.to_string());
        }
    }

    let mut text = json_text.to_string();

    if root.get("includes").and_then(|v| v.as_array()).is_some() {
        // Append to existing includes array: find the closing "]" of the includes value
        let includes_key = find_includes_value_range(&text)?;
        let close_bracket = text[includes_key.clone()].rfind(']').ok_or("找不到 includes 数组的闭合 ]")?;
        let abs_pos = includes_key.start + close_bracket;
        // Check if the array is empty "[]" or has content
        let array_inner = text[includes_key.start + 1..includes_key.end - 1].trim();
        if array_inner.is_empty() {
            // Empty array — insert directly
            text.insert_str(abs_pos, &format!("\"{include_path}\""));
        } else {
            // Non-empty — insert before the closing bracket, with a comma
            // Walk backwards from ] to find last non-whitespace
            let mut insert_at = abs_pos;
            while insert_at > 0 && text.as_bytes()[insert_at - 1].is_ascii_whitespace() {
                insert_at -= 1;
            }
            text.insert_str(insert_at, &format!(", \"{include_path}\""));
        }
    } else {
        // No includes key — insert before the final closing brace
        let last_brace = text.rfind('}').ok_or("找不到 vitric.json 的闭合 }")?;
        // Check if there's content before the brace (trailing comma handling)
        let mut insert_at = last_brace;
        while insert_at > 0 && text.as_bytes()[insert_at - 1].is_ascii_whitespace() {
            insert_at -= 1;
        }
        let needs_comma = insert_at > 0 && text.as_bytes()[insert_at - 1] != b'{' && text.as_bytes()[insert_at - 1] != b',';
        let prefix = if needs_comma { "," } else { "" };
        let indent = detect_indent(&text);
        let insertion = format!(
            "{prefix}\n{indent}\"includes\": [\"{include_path}\"]\n",
        );
        text.insert_str(insert_at, &insertion);
    }

    Ok(text)
}

/// Find the byte range of the value following the `"includes"` key in JSON text.
fn find_includes_value_range(text: &str) -> Result<std::ops::Range<usize>, String> {
    let key_pos = text.find("\"includes\"").ok_or("找不到 includes 键")?;
    // Skip past the key and its following colon+whitespace
    let after_key = key_pos + "\"includes\"".len();
    let colon_pos = text[after_key..]
        .find(':')
        .ok_or("includes 键后缺少冒号")?;
    let value_start = after_key + colon_pos + 1;
    // Skip whitespace after colon
    let value_start = text[value_start..]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| value_start + i)
        .unwrap_or(value_start);
    // Find the matching closing bracket for the array
    let mut depth = 0i32;
    let mut value_end = value_start;
    for (i, b) in text[value_start..].bytes().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    value_end = value_start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if value_end == value_start {
        return Err("找不到 includes 数组的闭合 ]".to_string());
    }
    Ok(value_start..value_end)
}

/// Detect the indentation unit used in a JSON file (first indented line).
fn detect_indent(text: &str) -> &str {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed != line && !trimmed.is_empty() {
            let indent_len = line.len() - trimmed.len();
            return &line[..indent_len];
        }
    }
    "  "
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

    #[test]
    fn patch_appends_to_existing_includes() {
        let json = r#"{
  "name": "my-game",
  "includes": ["../../modules/inventory"]
}"#;
        let result = patch_includes(json, "modules/combat").unwrap();
        assert!(result.contains("\"../../modules/inventory\""));
        assert!(result.contains("\"modules/combat\""));
        // Key order preserved: "name" still before "includes"
        assert!(result.find("\"name\"").unwrap() < result.find("\"includes\"").unwrap());
    }

    #[test]
    fn patch_noop_if_already_present() {
        let json = r#"{"includes": ["modules/inventory"]}"#;
        let result = patch_includes(json, "modules/inventory").unwrap();
        assert_eq!(result, json);
    }

    #[test]
    fn patch_creates_includes_if_missing() {
        let json = r#"{
  "name": "my-game",
  "schema": "schema.json"
}"#;
        let result = patch_includes(json, "modules/inventory").unwrap();
        assert!(result.contains("\"includes\""));
        assert!(result.contains("\"modules/inventory\""));
        // Existing keys still present and in order
        assert!(result.find("\"name\"").unwrap() < result.find("\"includes\"").unwrap());
        // Valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["includes"][0], "modules/inventory");
    }

    #[test]
    fn patch_handles_empty_includes_array() {
        let json = r#"{"includes": []}"#;
        let result = patch_includes(json, "modules/inventory").unwrap();
        assert!(result.contains("\"modules/inventory\""));
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["includes"].as_array().unwrap().len(), 1);
    }
}
