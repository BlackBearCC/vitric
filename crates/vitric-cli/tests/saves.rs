//! 玩家存档端到端：约定事件 save-game/load-game、`--load` 启动续玩、录像互斥。
//!
//! 用 coin-run 的临时副本（加三条存档规则），不污染示例项目。
//! in-process 部分逐步复刻 `vitric run` 主循环对存档事件的处理
//! （step → drain_observed → handle_save_load_events），CLI 部分跑真二进制。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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

/// coin-run 临时副本 + 存档规则：按 s 存 slot1、按 l 读 slot1、按 x 用非法槽名存。
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

/// in-process 装配：与 vitric run 同款（Runtime + Dispatcher + SaveStore）。
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
    /// 等价主循环的一步：step + 处理存档约定事件，返回存档错误记录。
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

    // 按 s：存档文件落盘，带引擎版本 + 项目名 + 完整快照
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

    // 改变世界：向右跑 60 tick（吃金币、动画推帧，哈希必然变）
    g.sim.inject_input("right", "pressed");
    for _ in 0..60 {
        assert!(g.step().is_empty());
    }
    assert_ne!(g.hash(), h_save);

    // 按 l：状态精确回到存档时刻
    g.sim.inject_input("l", "pressed");
    assert!(g.step().is_empty());
    assert_eq!(g.hash(), h_save, "load-game 后世界哈希必须等于存档时刻");
    assert_eq!(g.sim.tick, t_save);

    // 续玩照常（读档不是终点，继续跑不出错、世界继续演化）
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

    // 路径穿越槽名：拒绝 + 不落盘
    g.sim.inject_input("x", "pressed");
    let errs = g.step();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0]["event"], json!("save-game"));
    assert!(errs[0]["error"].as_str().unwrap().contains("不合法"), "{errs:?}");
    assert!(!dir.join("evil.json").exists() && !dir.join("saves").exists(), "非法槽名不许写出任何文件");

    // 读不存在的槽：显式报错，游戏继续跑
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
            g.sim.inject_input("s", "pressed"); // 录像中存档：纯输出副作用，放行
        }
        if t == 20 {
            g.sim.inject_input("l", "pressed"); // 录像中读档：拒绝
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

    // 录像仍可从冷启动逐位重放（读档被拒，时间线没断；重放中 save/load 事件无人执行）
    let rec = g.sim.stop_recording().unwrap();
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.replay(&rec, &mut rt2).expect("拒绝读档后的录像必须可重放");

    let _ = fs::remove_dir_all(&dir);
}

// ---- CLI：真二进制 ----

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
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let body_start = response.find("\r\n\r\n").expect("HTTP 响应有空行") + 4;
    serde_json::from_str(&response[body_start..]).expect("响应体是 JSON")
}

#[test]
fn cli_load_boots_into_saved_state_and_save_rpcs_roundtrip() {
    let dir = temp_copy("cli-load");
    // 先 in-process 造一份存档，记下存档时刻的哈希
    let mut g = boot(&dir);
    for _ in 0..30 {
        g.step();
    }
    g.sim.inject_input("s", "pressed");
    assert!(g.step().is_empty());
    let h_save = format!("{:#018x}", g.hash());
    let t_save = g.sim.tick;
    drop(g);

    // --load 续玩：极低倍速（几乎不前进），经控制面验证起点就是存档时刻
    let game = spawn_game(&dir, &["--load", "slot1", "--speed", "0.000001"]);
    let port = game.port;
    let r = rpc(port, "sim/pause", json!({}));
    assert_eq!(r["ok"], json!(true), "{r}");
    let pong = rpc(port, "ping", json!({}));
    assert_eq!(pong["result"]["tick"], json!(t_save), "--load 应恢复到存档 tick");
    assert_eq!(rpc(port, "sim/hash", json!({}))["result"], json!(h_save), "--load 应恢复到存档哈希");

    // RPC 薄封装回环：save/list / save/write 新槽 / 改世界 / save/load 回滚
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
    // 放一份存档，让报错有东西可列
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
