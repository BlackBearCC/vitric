//! 鼠标点击作为游戏输入的端到端验证：
//! `input/click`（世界坐标）→ 拾取解析 → `mouse` / `mouse-alt` 事件 → 规则消化，
//! 整条链路走回复通道——点击被录进录像、`vitric replay` 离线逐位复现。
//! 这是菜单/卡牌类鼠标游戏可被无头 agent 驱动 + 可出通关录像证书的根基。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

use vitric_sim::Recording;

/// 点击测试项目：一张牌（card，中心 (3,2)，2x2）+ 两条鼠标规则——
/// 左键点中翻面并记下点击的世界 x（条件用 event.x，data 用 event.entity 过滤），
/// 右键任意位置盖回去。
fn write_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-mouse-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "rules"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        json!({
            "name": "click-cards",
            "schema": "schema.json",
            "entry": "scenes/main.json",
            "scenes": ["scenes/main.json"],
            "rules": ["rules/mouse.json"],
            "seed": 9
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        json!({"components": {
            "Position": {"fields": {"x": {"type": "number"}, "y": {"type": "number"}}},
            "Sprite": {"fields": {
                "w": {"type": "number", "default": 1},
                "h": {"type": "number", "default": 1},
                "color": {"type": "text", "default": "#ffffff"}
            }},
            "Card": {"fields": {
                "flipped": {"type": "bool", "default": false},
                "last_x": {"type": "number", "default": 0}
            }}
        }})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        json!({"entities": [
            {"name": "card", "components": {
                "Position": {"x": 3.0, "y": 2.0},
                "Sprite": {"w": 2.0, "h": 2.0, "color": "#3366ff"},
                "Card": {}
            }}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("rules/mouse.json"),
        json!({"rules": [
            {
                "id": "flip-on-click",
                "on": {"event": "mouse", "filter": {"entity": "card"}},
                "if": [["event.x", ">", 0]],
                "do": [
                    {"set": "@card.Card.flipped", "to": true},
                    {"set": "@card.Card.last_x", "to": "event.x"}
                ]
            },
            {
                "id": "unflip-on-alt",
                "on": {"event": "mouse-alt"},
                "do": [{"set": "@card.Card.flipped", "to": false}]
            }
        ]})
        .to_string(),
    )
    .unwrap();
    dir
}

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

fn spawn_game(dir: &PathBuf, extra: &[&str]) -> RunningGame {
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
fn agent_clicks_over_http_and_recording_replays() {
    let dir = write_project("e2e");
    let rec_path = dir.join("clicks.json");

    {
        let game = spawn_game(&dir, &["--record", rec_path.to_str().unwrap()]);
        let port = game.port;
        rpc(port, "sim/pause", json!({}));

        // 左键点在牌上（世界坐标）：拾取直接回实体名，下一 tick 规则翻面
        let r = rpc(port, "input/click", json!({"x": 3.5, "y": 2.0}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["result"]["event"], json!("mouse"));
        assert_eq!(r["result"]["entity"], json!("card"));
        rpc(port, "sim/step", json!({}));
        let card = rpc(port, "world/get", json!({"entity": "@card"}));
        assert_eq!(card["result"]["components"]["Card"]["flipped"], json!(true), "{card}");
        assert_eq!(card["result"]["components"]["Card"]["last_x"], json!(3.5), "条件/取值都该来自 event.x");

        // 空地左键：entity null，filter 不匹配，状态不变
        let r = rpc(port, "input/click", json!({"x": 50.0, "y": 50.0}));
        assert_eq!(r["result"]["entity"], json!(null), "{r}");
        rpc(port, "sim/step", json!({}));
        let card = rpc(port, "world/get", json!({"entity": "@card"}));
        assert_eq!(card["result"]["components"]["Card"]["flipped"], json!(true));

        // 右键 → mouse-alt → 盖回去
        let r = rpc(port, "input/click", json!({"x": 3.0, "y": 2.0, "button": "right"}));
        assert_eq!(r["result"]["event"], json!("mouse-alt"), "{r}");
        rpc(port, "sim/step", json!({}));
        let card = rpc(port, "world/get", json!({"entity": "@card"}));
        assert_eq!(card["result"]["components"]["Card"]["flipped"], json!(false), "{card}");

        // 事件日志里 mouse / mouse-alt 可见（规则之外 agent 也观测得到）
        let events = rpc(port, "events/recent", json!({}));
        let names: Vec<&str> =
            events["result"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"mouse"), "{names:?}");
        assert!(names.contains(&"mouse-alt"), "{names:?}");

        // 干净退出让录像落盘
        rpc(port, "sim/quit", json!({}));
        let mut game = game;
        let status = game.child.wait().unwrap();
        assert!(status.success());
    }

    // 点击连同拾取结果都在录像的回复通道里
    let rec: Recording = serde_json::from_str(&fs::read_to_string(&rec_path).unwrap()).unwrap();
    let names: Vec<&str> = rec.replies.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["mouse", "mouse", "mouse-alt"], "{names:?}");
    assert_eq!(rec.replies[0].data["entity"], json!("card"));
    assert_eq!(rec.replies[1].data["entity"], json!(null));

    // 离线重放：点击从录像注入，逐校验点复现（鼠标游戏的通关录像证书由此成立）
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .arg("replay")
        .arg(&dir)
        .arg(&rec_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verified"], json!(true), "{report}");

    fs::remove_dir_all(&dir).unwrap();
}
