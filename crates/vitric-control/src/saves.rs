//! 玩家存档 — "存档随时续玩"，建立在 `Sim::snapshot` / `Sim::restore` 之上。
//!
//! 谁住在确定性边界的哪一侧（这是本模块全部设计的出发点）：
//! - **存档（save-game / save/write）是纯输出副作用**，和 play-sound 一个待遇：
//!   在模拟之外执行，写不写文件都不回流进世界，确定性回放不受影响。
//! - **读档（load-game / save/load / --load）会改写模拟**，等价 sim/restore：
//!   时间线断裂，正在录的录像必然不可重放——录像期间一律显式拒绝（守卫在
//!   Dispatcher，那里能同时看到 sim 和录像状态）。
//! - **槽名是文件名的一部分**：[a-z0-9-]{1,32} 之外全拒，路径穿越在这里堵死。
//!
//! 存档文件格式（`<项目>/saves/<slot>.json`）：
//! `{"engine_version", "project", "slot", "snapshot"}`——snapshot 就是
//! `Sim::snapshot` 的原样输出（世界/tick/随机数/未消化输入与回复/逻辑层暂存事件）。
//! 版本不匹配读档显式报错，不做静默兼容尝试。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use vitric_sim::{GameLogic, Sim};

/// 写进存档文件的引擎版本。读档时不匹配 = 显式报错（快照格式跨版本不保证兼容，
/// 静默尝试要么悄悄成功掩盖问题、要么在恢复中途炸出难懂的字段错误）。
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 槽名校验：`[a-z0-9-]{1,32}`。槽名直接拼进文件路径，这条规则同时堵死路径穿越。
pub fn validate_slot(slot: &str) -> Result<(), String> {
    let chars_ok =
        slot.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if slot.is_empty() || slot.len() > 32 || !chars_ok {
        return Err(format!(
            "存档槽名 {slot:?} 不合法：只允许小写字母/数字/连字符（[a-z0-9-]），长度 1..=32。\
             槽名会直接成为 saves/ 目录下的文件名，这条规则同时堵死 ../ 之类的路径穿越"
        ));
    }
    Ok(())
}

/// 一个项目的存档仓库：`<项目根>/saves/` 目录的全部读写都从这里走——
/// 约定事件（save-game/load-game）、控制面 RPC（save/*）、CLI（--load）共用同一条代码路径。
pub struct SaveStore {
    dir: PathBuf,
    /// 清单里的项目名，写进存档文件（人看文件能知道这是谁的存档）。
    project: String,
}

impl SaveStore {
    pub fn new(project_root: &Path, project: &str) -> SaveStore {
        SaveStore { dir: project_root.join("saves"), project: project.to_string() }
    }

    /// 写一个槽位（saves/ 目录自动创建）。返回 `{"slot", "path", "tick"}`。
    pub fn write(&self, slot: &str, sim: &Sim, logic: &dyn GameLogic) -> Result<Value, String> {
        validate_slot(slot)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("创建存档目录 {} 失败: {e}", self.dir.display()))?;
        let path = self.slot_path(slot);
        let file = json!({
            "engine_version": ENGINE_VERSION,
            "project": self.project,
            "slot": slot,
            "snapshot": sim.snapshot(logic),
        });
        // 先写临时文件再原子改名：写一半崩溃/断电不会留下半截 JSON 顶替旧存档
        let tmp = self.dir.join(format!(".{slot}.json.tmp"));
        std::fs::write(&tmp, serde_json::to_string(&file).expect("存档可序列化"))
            .map_err(|e| format!("写存档 {} 失败: {e}", path.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("落盘存档 {} 失败: {e}", path.display()))?;
        Ok(json!({"slot": slot, "path": path.display().to_string(), "tick": sim.tick}))
    }

    /// 读一个槽位，返回可直接交给 `Sim::restore` 的快照。
    /// 缺文件/坏 JSON/版本不符全部显式报错（缺文件时列出现有存档）。
    pub fn read(&self, slot: &str) -> Result<Value, String> {
        validate_slot(slot)?;
        let path = self.slot_path(slot);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let existing = self.list()?;
                let listing = if existing.is_empty() {
                    format!("{} 目录里还没有任何存档", self.dir.display())
                } else {
                    format!("现有存档: [{}]", existing.join(", "))
                };
                return Err(format!("存档 {slot:?} 不存在（找的是 {}）。{listing}", path.display()));
            }
            Err(e) => return Err(format!("读存档 {} 失败: {e}", path.display())),
        };
        let file: Value = serde_json::from_str(&text).map_err(|e| {
            format!("存档 {} 不是合法 JSON（文件损坏或被改坏了）: {e}", path.display())
        })?;
        let version = file
            .get("engine_version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!("存档 {} 缺 engine_version 字段——不是 vitric 写出的存档文件", path.display())
            })?;
        if version != ENGINE_VERSION {
            return Err(format!(
                "存档来自引擎 v{version}，当前 v{ENGINE_VERSION}，不保证兼容——\
                 确认要读就把 {} 里的 engine_version 改成 {ENGINE_VERSION:?} 再试",
                path.display()
            ));
        }
        file.get("snapshot").cloned().ok_or_else(|| {
            format!("存档 {} 缺 snapshot 字段——不是 vitric 写出的存档文件", path.display())
        })
    }

    /// 列出全部存档槽名（字典序）。saves/ 目录不存在 = 还没存过 = 空列表（合法状态）；
    /// 槽名规则之外的文件（README、临时文件）不算存档，跳过。
    pub fn list(&self) -> Result<Vec<String>, String> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("读存档目录 {} 失败: {e}", self.dir.display())),
        };
        let mut slots = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("读存档目录 {} 失败: {e}", self.dir.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(stem) = name.strip_suffix(".json") {
                if validate_slot(stem).is_ok() {
                    slots.push(stem.to_string());
                }
            }
        }
        slots.sort();
        Ok(slots)
    }

    fn slot_path(&self, slot: &str) -> PathBuf {
        self.dir.join(format!("{slot}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vitric-savestore-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(root: &Path) -> SaveStore {
        SaveStore::new(root, "test-game")
    }

    #[test]
    fn slot_validation_blocks_traversal_and_bad_names() {
        // 路径穿越是头号要堵的口子
        assert!(validate_slot("../x").is_err());
        assert!(validate_slot("a/b").is_err());
        assert!(validate_slot("a\\b").is_err());
        // 大写/空/超长也不合法
        assert!(validate_slot("Slot1").is_err());
        assert!(validate_slot("").is_err());
        assert!(validate_slot(&"a".repeat(33)).is_err());
        // 合法形态
        assert!(validate_slot("slot1").is_ok());
        assert!(validate_slot("auto-save-3").is_ok());
        assert!(validate_slot(&"a".repeat(32)).is_ok());
        // 错误信息要讲清规则和动机
        let e = validate_slot("../x").unwrap_err();
        assert!(e.contains("a-z0-9-") && e.contains("路径穿越"), "{e}");
    }

    #[test]
    fn write_then_read_roundtrips_snapshot() {
        let root = temp_root("roundtrip");
        let mut sim = Sim::new(7);
        sim.world.spawn_named("hero").unwrap();
        for _ in 0..5 {
            sim.step(&mut ()).unwrap();
        }
        let result = store(&root).write("slot1", &sim, &()).unwrap();
        assert_eq!(result["slot"], json!("slot1"));
        assert_eq!(result["tick"], json!(5));
        let path = root.join("saves/slot1.json");
        assert!(path.exists(), "存档文件应落在 saves/slot1.json");
        // 文件头部字段齐全
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["engine_version"], json!(ENGINE_VERSION));
        assert_eq!(file["project"], json!("test-game"));
        // 读回来的就是 Sim::snapshot 的原样输出
        assert_eq!(store(&root).read("slot1").unwrap(), sim.snapshot(&()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_missing_slot_lists_existing_saves() {
        let root = temp_root("missing");
        let sim = Sim::new(1);
        let s = store(&root);
        // 目录都还没有：明说"还没有任何存档"
        let e = s.read("ghost").unwrap_err();
        assert!(e.contains("ghost") && e.contains("还没有任何存档"), "{e}");
        // 有别的存档：列出来给玩家/agent 选
        s.write("alpha", &sim, &()).unwrap();
        s.write("beta", &sim, &()).unwrap();
        let e = s.read("ghost").unwrap_err();
        assert!(e.contains("alpha") && e.contains("beta"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn version_mismatch_is_explicit_no_silent_attempt() {
        let root = temp_root("version");
        let sim = Sim::new(1);
        let s = store(&root);
        s.write("slot1", &sim, &()).unwrap();
        // 手改版本号模拟旧引擎写的存档
        let path = root.join("saves/slot1.json");
        let mut file: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        file["engine_version"] = json!("0.0.0-old");
        std::fs::write(&path, file.to_string()).unwrap();
        let e = s.read("slot1").unwrap_err();
        assert!(
            e.contains("0.0.0-old") && e.contains(ENGINE_VERSION) && e.contains("不保证兼容"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_json_is_explicit() {
        let root = temp_root("corrupt");
        std::fs::create_dir_all(root.join("saves")).unwrap();
        std::fs::write(root.join("saves/slot1.json"), "{这不是 json").unwrap();
        let e = store(&root).read("slot1").unwrap_err();
        assert!(e.contains("JSON") && e.contains("slot1.json"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_is_sorted_and_skips_non_slot_files() {
        let root = temp_root("list");
        let sim = Sim::new(1);
        let s = store(&root);
        assert_eq!(s.list().unwrap(), Vec::<String>::new(), "没存过 = 空列表");
        s.write("beta", &sim, &()).unwrap();
        s.write("alpha", &sim, &()).unwrap();
        // 槽名规则之外的文件不算存档
        std::fs::write(root.join("saves/README.txt"), "x").unwrap();
        std::fs::write(root.join("saves/Bad_Name.json"), "{}").unwrap();
        assert_eq!(s.list().unwrap(), vec!["alpha".to_string(), "beta".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
