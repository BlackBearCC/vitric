//! Player saves — "save anytime, resume anytime", built on `Sim::snapshot` / `Sim::restore`.
//!
//! Who lives on which side of the determinism boundary (this is the starting point for the
//! entire design of this module):
//! - **Saving (save-game / save/write) is a pure output side effect**, treated the same as
//!   play-sound: executed outside the simulation, and whether the file is written or not does
//!   not flow back into the world, so deterministic replay is unaffected.
//! - **Loading (load-game / save/load / --load) rewrites the simulation**, equivalent to
//!   sim/restore: the timeline breaks, and any recording in progress becomes unreplayable —
//!   so during recording it is always explicitly rejected (the guard lives in the Dispatcher,
//!   which can see both sim and recording state).
//! - **The slot name is part of the file name**: anything outside `[a-z0-9-]{1,32}` is rejected,
//!   and path traversal is closed off here.
//!
//! Save file format (`<project>/saves/<slot>.json`):
//! `{"engine_version", "project", "slot", "snapshot"}` — snapshot is the raw output of
//! `Sim::snapshot` (world / tick / RNG / undigested input and replies / logic-layer buffered events).
//! Version mismatch on load is an explicit error; no silent compatibility attempt is made.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use vitric_sim::{GameLogic, Sim};

/// Engine version written into save files. A mismatch on load = an explicit error (the snapshot
/// format is not guaranteed to be compatible across versions; a silent attempt either quietly
/// succeeds and hides the problem, or blows up mid-restore with an incomprehensible field error).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Slot name validation: `[a-z0-9-]{1,32}`. The slot name is concatenated directly into the file
/// path, so this rule also closes off path traversal.
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

/// A project's save store: all reads and writes for the `<project root>/saves/` directory go
/// through here — convention events (save-game/load-game), control plane RPCs (save/*), and the
/// CLI (--load) share the same code path.
pub struct SaveStore {
    dir: PathBuf,
    /// Project name from the manifest, written into save files (a human reading the file can
    /// tell whose save it is).
    project: String,
}

impl SaveStore {
    pub fn new(project_root: &Path, project: &str) -> SaveStore {
        SaveStore { dir: project_root.join("saves"), project: project.to_string() }
    }

    /// Write a slot (the saves/ directory is auto-created). Returns `{"slot", "path", "tick"}`.
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
        // Write to a temp file then atomically rename: a half-written crash / power loss cannot
        // leave a half-baked JSON that overwrites the old save.
        let tmp = self.dir.join(format!(".{slot}.json.tmp"));
        std::fs::write(&tmp, serde_json::to_string(&file).expect("存档可序列化"))
            .map_err(|e| format!("写存档 {} 失败: {e}", path.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("落盘存档 {} 失败: {e}", path.display()))?;
        Ok(json!({"slot": slot, "path": path.display().to_string(), "tick": sim.tick}))
    }

    /// Read a slot, returning a snapshot that can be handed directly to `Sim::restore`.
    /// Missing file / bad JSON / version mismatch all explicitly error (a missing file lists
    /// existing saves).
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

    /// List all save slot names (lexicographic). A missing saves/ directory = nothing saved yet =
    /// an empty list (a legal state); files outside the slot-name rules (README, temp files) do
    /// not count as saves and are skipped.
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
        // Path traversal is the number-one hole to plug.
        assert!(validate_slot("../x").is_err());
        assert!(validate_slot("a/b").is_err());
        assert!(validate_slot("a\\b").is_err());
        // Uppercase / empty / over-length are also illegal.
        assert!(validate_slot("Slot1").is_err());
        assert!(validate_slot("").is_err());
        assert!(validate_slot(&"a".repeat(33)).is_err());
        // Legal forms.
        assert!(validate_slot("slot1").is_ok());
        assert!(validate_slot("auto-save-3").is_ok());
        assert!(validate_slot(&"a".repeat(32)).is_ok());
        // The error message must explain the rule and the motivation.
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
        // File header fields are all present.
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["engine_version"], json!(ENGINE_VERSION));
        assert_eq!(file["project"], json!("test-game"));
        // What comes back is the raw output of Sim::snapshot.
        assert_eq!(store(&root).read("slot1").unwrap(), sim.snapshot(&()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_missing_slot_lists_existing_saves() {
        let root = temp_root("missing");
        let sim = Sim::new(1);
        let s = store(&root);
        // Directory does not even exist yet: explicitly say "no saves yet".
        let e = s.read("ghost").unwrap_err();
        assert!(e.contains("ghost") && e.contains("还没有任何存档"), "{e}");
        // Other saves exist: list them for the player/agent to choose.
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
        // Hand-edit the version to simulate a save written by an older engine.
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
        // Files outside the slot-name rules do not count as saves.
        std::fs::write(root.join("saves/README.txt"), "x").unwrap();
        std::fs::write(root.join("saves/Bad_Name.json"), "{}").unwrap();
        assert_eq!(s.list().unwrap(), vec!["alpha".to_string(), "beta".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
