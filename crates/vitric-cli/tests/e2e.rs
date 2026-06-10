//! 端到端测试：用 coin-run 示例游戏验证整条链路——
//! 数据加载 → 规则 → 脚本 → 模拟 → 控制面 → 录像重放。
//! 这就是「AI 自主闭环」的最小可信证明。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

use vitric_cli::runtime::Runtime;
use vitric_sim::{GameLogic, Recording};

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run")
}

#[test]
fn play_coin_run_to_victory() {
    let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
    sim.start_recording();

    // 按住右键跑 1 秒：速度 60/s，金币在 x=10/20/30，全部吃到
    sim.inject_input("right", "pressed");
    let mut all_events = Vec::new();
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
        all_events.extend(rt.drain_observed());
    }

    let player = sim.world.entity("player").unwrap();
    assert_eq!(
        sim.world.get_field(player, "Score.value").unwrap(),
        &json!(3),
        "三枚金币都该被吃掉"
    );
    assert!(sim.world.query(&["Coin"]).is_empty(), "金币实体应已销毁");
    // win-check 规则 → celebrate 脚本函数 → game-won 事件 + 5 个彩带实体
    assert!(
        all_events.iter().any(|e| e.name == "game-won"),
        "应观测到 game-won 事件，实际: {:?}",
        all_events.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    assert_eq!(
        sim.world.query(&["Position", "Velocity"]).len(),
        1 + 5,
        "玩家 + 5 个彩带粒子"
    );
    // 通关后玩家被规则停下
    assert_eq!(sim.world.get_field(player, "Velocity.x").unwrap().as_f64(), Some(0.0));

    // 录像重放：从头再来必须逐校验点一致
    let rec = sim.stop_recording().unwrap();
    let (mut sim2, mut rt2) = Runtime::boot(&example_dir()).unwrap();
    sim2.replay(&rec, &mut rt2).expect("重放必须逐帧一致");
}

#[test]
fn determinism_across_full_stack() {
    // 整条链路（规则+脚本+随机数）跑两遍，哈希必须一致
    let run = || {
        let (mut sim, mut rt) = Runtime::boot(&example_dir()).unwrap();
        sim.inject_input("right", "pressed");
        for t in 0..90 {
            if t == 45 {
                sim.inject_input("right", "released");
            }
            sim.step(&mut rt).unwrap();
        }
        sim.world.state_hash()
    };
    assert_eq!(run(), run());
}

#[test]
fn check_command_reports_project_shape() {
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("check")
        .arg(example_dir())
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["project"], json!("coin-run"));
    assert_eq!(report["entities"], json!(5));
    assert!(report["rules"].as_array().unwrap().iter().any(|r| r == "collect-coin"));
    assert!(report["systems"][0]["writes"].as_array().is_some());
}

#[test]
fn check_command_reports_broken_project_with_paths() {
    // 用一个坏项目验证报错质量：路径 + 错误码 + 修复提示一次给全
    let dir = std::env::temp_dir().join(format!("vitric-e2e-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(
        dir.join("vitric.json"),
        r#"{"name":"bad","schema":"schema.json","entry":"scenes/main.json","scenes":["scenes/main.json"]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("schema.json"),
        r#"{"components":{"Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[{"components":{"Position":{"x":1,"z":2}}}]}"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("check")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("VD003"), "要有错误码: {stderr}");
    assert!(stderr.contains("scenes/main.json#/entities/0"), "要有精确路径: {stderr}");
    assert!(stderr.contains("x, y"), "要列出可用字段: {stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- 控制面 HTTP 闭环：跑真二进制，像 AI agent 一样通过 HTTP 操作 ----

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

fn spawn_game() -> RunningGame {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("run")
        .arg(example_dir())
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // 第一行 stdout 是启动横幅 JSON，含控制面地址
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
fn agent_drives_game_over_http() {
    let game = spawn_game();
    let port = game.port;

    // 看：ping + 初始世界
    let pong = rpc(port, "ping", json!({}));
    assert_eq!(pong["ok"], json!(true), "{pong}");
    let entities = rpc(port, "world/entities", json!({"components": ["Coin"]}));
    assert_eq!(entities["result"].as_array().unwrap().len(), 3);

    // 测：注册断言「分数不超过 3」
    let r = rpc(port, "assert/add", json!({"id": "score-cap", "if": [["@player.Score.value", "<=", 3]]}));
    assert_eq!(r["ok"], json!(true), "{r}");

    // 控时间：暂停 → 注入输入 → 单步推进（确定性的逐帧控制）
    rpc(port, "sim/pause", json!({}));
    rpc(port, "input/inject", json!({"action": "right", "phase": "pressed"}));
    let r = rpc(port, "sim/step", json!({"ticks": 60}));
    assert_eq!(r["ok"], json!(true), "{r}");

    // 验证结果：分数 3、金币清零、game-won 事件可见
    let player = rpc(port, "world/get", json!({"entity": "@player"}));
    assert_eq!(player["result"]["components"]["Score"]["value"], json!(3));
    let coins = rpc(port, "world/entities", json!({"components": ["Coin"]}));
    assert_eq!(coins["result"].as_array().unwrap().len(), 0);
    let events = rpc(port, "events/recent", json!({}));
    let names: Vec<&str> = events["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"collision"), "{names:?}");
    assert!(names.contains(&"coin-collected"), "{names:?}");
    assert!(names.contains(&"game-won"), "{names:?}");

    // 改：直接改世界状态（过 schema），再读回来
    rpc(port, "world/set", json!({"entity": "@player", "path": "Score.value", "value": 0}));
    let player = rpc(port, "world/get", json!({"entity": "@player"}));
    assert_eq!(player["result"]["components"]["Score"]["value"], json!(0));

    // 断言一直健康
    let failures = rpc(port, "assert/failures", json!({}));
    assert_eq!(failures["result"].as_array().unwrap().len(), 0, "{failures}");

    // 看（语义级，主通道）：画面翻译成精确描述
    let desc = rpc(port, "render/describe", json!({}));
    assert_eq!(desc["ok"], json!(true), "{desc}");
    let visible = desc["result"]["visible"].as_array().unwrap();
    assert!(
        visible.iter().any(|v| v["name"] == json!("player")),
        "玩家应在画面里: {desc}"
    );
    assert!(desc["result"]["text"].as_str().unwrap().contains("相机"));

    // 看（像素级）：无头截图，PNG 直接回传 base64
    let shot = rpc(port, "render/screenshot", json!({"width": 320, "height": 240, "inline": true}));
    assert_eq!(shot["ok"], json!(true), "{shot}");
    assert_eq!(shot["result"]["width"], json!(320));
    let b64 = shot["result"]["png_base64"].as_str().unwrap();
    assert!(b64.starts_with("iVBORw0KGgo"), "base64 的 PNG 魔数");

    rpc(port, "sim/quit", json!({}));
}

#[test]
fn record_and_replay_via_cli() {
    let dir = example_dir();
    let rec_path = std::env::temp_dir().join(format!("vitric-rec-{}.json", std::process::id()));

    // 跑 120 tick 录像（无输入纯模拟也有金币漂浮的脚本运动）
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("run")
        .arg(&dir)
        .args(["--ticks", "120", "--record"])
        .arg(&rec_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let rec: Recording = serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec.ticks, 120);

    // 重放校验
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("replay")
        .arg(&dir)
        .arg(&rec_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verified"], json!(true));

    std::fs::remove_file(&rec_path).unwrap();
}
