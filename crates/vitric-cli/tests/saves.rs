//! Player saves end-to-end: convention events save-game/load-game, `--load` boot to resume, recording mutual exclusion.
//!
//! Uses a temp copy of coin-run (with three save rules added), not polluting the example project.
//! The in-process part incrementally replicates `vitric run`'s main-loop handling of save events
//! (step → drain_observed → handle_save_load_events); the CLI part runs the real binary.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use vitric_cli::runtime::Runtime;
use vitric_control::{saves::ENGINE_VERSION, Dispatcher, SaveStore};
use vitric_data::Project;
use vitric_sim::{GameLogic, Sim};

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// coin-run temp copy + save rules: press s to save slot1, press l to load slot1, press x to save with an illegal slot name.
fn temp_copy(tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run");
    let dst = std::env::temp_dir().join(format!("vitric-saves-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    copy_dir(&src, &dst);
    fs::write(
        dst.join("rules/saves.json"),
        json!({"rules": [
            {"id": "save-on-s", "on": {"event": "input", "filter": {"action": "s", "phase": "pressed"}},
             "do": [{"emit": "save-game", "data": {"slot": "slot1"}}]},
            {"id": "load-on-l", "on": {"event": "input", "filter": {"action": "l", "phase": "pressed"}},
             "do": [{"emit": "load-game", "data": {"slot": "slot1"}}]},
            {"id": "bad-slot-on-x", "on": {"event": "input", "filter": {"action": "x", "phase": "pressed"}},
             "do": [{"emit": "save-game", "data": {"slot": "../evil"}}]}
        ]})
        .to_string(),
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(dst.join("vitric.json")).unwrap()).unwrap();
    manifest["rules"].as_array_mut().unwrap().push(json!("rules/saves.json"));
    fs::write(dst.join("vitric.json"), manifest.to_string()).unwrap();
    dst
}

/// in-process assembly: same as vitric run (Runtime + Dispatcher + SaveStore).
struct Game {
    sim: Sim,
    rt: Runtime,
    d: Dispatcher,
}

fn boot(dir: &Path) -> Game {
    let project = Project::load(dir).unwrap();
    let (sim, rt) = Runtime::boot(dir).unwrap();
    let mut d = Dispatcher::new(project.schema.clone());
    d.set_save_store(SaveStore::new(dir, &project.manifest.name));
    Game { sim, rt, d }
}

impl Game {
    /// Equivalent to one step of the main loop: step + handle save convention events, returns save error records.
    fn step(&mut self) -> Vec<Value> {
        self.sim.step(&mut self.rt).unwrap();
        let observed = self.rt.drain_observed();
        self.d.handle_save_load_events(&observed, &mut self.sim, &mut self.rt)
    }

    fn hash(&self) -> u64 {
        self.sim.world.state_hash()
    }
}

#[test]
fn save_event_writes_file_and_load_event_restores_state() {
    let dir = temp_copy("roundtrip");
    let mut g = boot(&dir);
    for _ in 0..30 {
        assert!(g.step().is_empty());
    }

    // Press s: save file flushed to disk, carrying engine version + project name + full snapshot
    g.sim.inject_input("s", "pressed");
    assert!(g.step().is_empty());
    let save_path = dir.join("saves/slot1.json");
    assert!(save_path.exists(), "save-game 应写出 saves/slot1.json");
    let file: Value = serde_json::from_str(&fs::read_to_string(&save_path).unwrap()).unwrap();
    assert_eq!(file["engine_version"], json!(ENGINE_VERSION));
    assert_eq!(file["project"], json!("coin-run"));
    assert!(file["snapshot"]["world"].is_object(), "快照要含完整世界: {file}");
    let h_save = g.hash();
    let t_save = g.sim.tick;
    assert_eq!(file["snapshot"]["tick"], json!(t_save));

    // Change the world: run right for 60 ticks (eat coins, advance animation frames, the hash must change)
    g.sim.inject_input("right", "pressed");
    for _ in 0..60 {
        assert!(g.step().is_empty());
    }
    assert_ne!(g.hash(), h_save);

    // Press l: state returns exactly to the save moment
    g.sim.inject_input("l", "pressed");
    assert!(g.step().is_empty());
    assert_eq!(g.hash(), h_save, "load-game 后世界哈希必须等于存档时刻");
    assert_eq!(g.sim.tick, t_save);

    // Resume play as usual (loading is not the end; keep running without errors, the world keeps evolving)
    for _ in 0..30 {
        assert!(g.step().is_empty());
    }
    assert_ne!(g.hash(), h_save);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bad_slot_and_missing_slot_report_structured_errors_without_crashing() {
    let dir = temp_copy("errors");
    let mut g = boot(&dir);

    // Path-traversal slot name: rejected + nothing flushed to disk
    g.sim.inject_input("x", "pressed");
    let errs = g.step();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0]["event"], json!("save-game"));
    assert!(errs[0]["error"].as_str().unwrap().contains("不合法"), "{errs:?}");
    assert!(!dir.join("evil.json").exists() && !dir.join("saves").exists(), "非法槽名不许写出任何文件");

    // Load a non-existent slot: explicit error, the game keeps running
    g.sim.inject_input("l", "pressed");
    let errs = g.step();
    assert_eq!(errs.len(), 1, "{errs:?}");
    let msg = errs[0]["error"].as_str().unwrap();
    assert!(msg.contains("slot1") && msg.contains("不存在"), "{msg}");
    for _ in 0..10 {
        assert!(g.step().is_empty(), "存档错误不崩游戏");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn load_game_refused_while_recording_and_recording_stays_replayable() {
    let dir = temp_copy("recording");
    let mut g = boot(&dir);
    g.sim.start_recording();

    let mut errs = Vec::new();
    for t in 0..90 {
        if t == 5 {
            g.sim.inject_input("s", "pressed"); // Saving during a recording: pure output side effect, allowed
        }
        if t == 20 {
            g.sim.inject_input("l", "pressed"); // Loading during a recording: rejected
        }
        if t == 40 {
            g.sim.inject_input("right", "pressed");
        }
        errs.extend(g.step());
    }
    assert!(dir.join("saves/slot1.json").exists(), "录像中 save-game 照常写盘");
    assert_eq!(errs.len(), 1, "{errs:?}");
    let msg = errs[0]["error"].as_str().unwrap();
    assert!(msg.contains("录像") && msg.contains("互斥"), "{msg}");
    assert!(g.sim.is_recording(), "拒绝读档后录像必须仍然有效");

    // The recording can still be replayed bit-by-bit from a cold boot (loading was rejected, the timeline is unbroken; during replay save/load events are executed by no one)
    let rec = g.sim.stop_recording().unwrap();
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.replay(&rec, &mut rt2).expect("拒绝读档后的录像必须可重放");

    let _ = fs::remove_dir_all(&dir);
}

// ---- CLI: real binary ----

struct RunningGame {
    child: Child,
    port: u16,
}

impl Drop for RunningGame {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_game(dir: &Path, extra: &[&str]) -> RunningGame {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("run")
        .arg(dir)
        .args(["--port", "0"])
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.as_mut().unwrap();
    let mut banner = Vec::new();
    let mut byte = [0u8; 1];
    while stdout.read_exact(&mut byte).is_ok() {
        if byte[0] == b'\n' {
            break;
        }
        banner.push(byte[0]);
    }
    let banner: Value = serde_json::from_slice(&banner).expect("启动横幅是 JSON");
    let control = banner["control"].as_str().unwrap();
    let port: u16 = control.rsplit(':').next().unwrap().trim_end_matches("/rpc").parse().unwrap();
    RunningGame { child, port }
}

fn rpc(port: u16, method: &str, params: Value) -> Value {
    let body = json!({"method": method, "params": params}).to_string();
    // Retry the whole request: the OS can complete the TCP handshake while the freshly
    // spawned server is not yet accepting, which otherwise shows up as a refused or
    // empty response on loaded CI machines.
    let mut last_err = String::new();
    for _ in 0..40 {
        let attempt = (|| -> Result<String, String> {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;
            write!(
                stream,
                "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .map_err(|e| e.to_string())?;
            let mut response = String::new();
            stream.read_to_string(&mut response).map_err(|e| e.to_string())?;
            if !response.contains("\r\n\r\n") {
                return Err(format!("incomplete HTTP response: {response:?}"));
            }
            Ok(response)
        })();
        match attempt {
            Ok(response) => {
                let body_start = response.find("\r\n\r\n").expect("HTTP 响应有空行") + 4;
                return serde_json::from_str(&response[body_start..]).expect("响应体是 JSON");
            }
            Err(e) => {
                last_err = e;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    panic!("RPC {method} retried 40 times, last error: {last_err}");
}

#[test]
fn cli_load_boots_into_saved_state_and_save_rpcs_roundtrip() {
    let dir = temp_copy("cli-load");
    // First build a save in-process, recording the hash at the save moment
    let mut g = boot(&dir);
    for _ in 0..30 {
        g.step();
    }
    g.sim.inject_input("s", "pressed");
    assert!(g.step().is_empty());
    let h_save = format!("{:#018x}", g.hash());
    let t_save = g.sim.tick;
    drop(g);

    // --load resume: very low speed (barely advances), verify via the control plane that the starting point is the save moment
    let game = spawn_game(&dir, &["--load", "slot1", "--speed", "0.000001"]);
    let port = game.port;
    let r = rpc(port, "sim/pause", json!({}));
    assert_eq!(r["ok"], json!(true), "{r}");
    let pong = rpc(port, "ping", json!({}));
    assert_eq!(pong["result"]["tick"], json!(t_save), "--load 应恢复到存档 tick");
    assert_eq!(rpc(port, "sim/hash", json!({}))["result"], json!(h_save), "--load 应恢复到存档哈希");

    // Thin RPC wrapper round-trip: save/list / save/write a new slot / mutate the world / save/load to roll back
    assert_eq!(rpc(port, "save/list", json!({}))["result"], json!(["slot1"]));
    let r = rpc(port, "save/write", json!({"slot": "slot2"}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert!(dir.join("saves/slot2.json").exists());
    rpc(port, "world/set", json!({"entity": "@player", "path": "Score.value", "value": 2}));
    assert_ne!(rpc(port, "sim/hash", json!({}))["result"], json!(h_save));
    let r = rpc(port, "save/load", json!({"slot": "slot2"}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(rpc(port, "sim/hash", json!({}))["result"], json!(h_save), "save/load 应回滚到 slot2 时刻");

    rpc(port, "sim/quit", json!({}));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_load_missing_slot_is_boot_error_listing_saves() {
    let dir = temp_copy("cli-missing");
    // Drop a save so the error has something to list
    let g = boot(&dir);
    SaveStore::new(&dir, "coin-run").write("slot1", &g.sim, &()).unwrap();
    drop(g);

    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("run")
        .arg(&dir)
        .args(["--load", "ghost"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "缺失槽位必须是启动错误");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ghost") && stderr.contains("slot1"), "报错要列出现有存档: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_load_and_record_are_mutually_exclusive() {
    let dir = temp_copy("cli-exclusive");
    let g = boot(&dir);
    SaveStore::new(&dir, "coin-run").write("slot1", &g.sim, &()).unwrap();
    drop(g);

    let rec = std::env::temp_dir().join(format!("vitric-saves-rec-{}.json", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("run")
        .arg(&dir)
        .args(["--load", "slot1", "--record"])
        .arg(&rec)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("互斥"), "{stderr}");
    assert!(!rec.exists(), "互斥拒绝后不许产出录像文件");

    let _ = fs::remove_dir_all(&dir);
}
