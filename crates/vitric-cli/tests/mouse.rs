//! End-to-end verification of mouse clicks as game input:
//! `input/click` (world coordinates) → pick resolution → `mouse` / `mouse-alt` events → rule consumption,
//! the whole pipeline goes through the reply channel — clicks are recorded into the recording, `vitric replay` reproduces bit-by-bit offline.
//! This is the foundation for menu/card-type mouse games to be driven by a headless agent + produce a clear-recording certificate.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

use vitric_sim::Recording;

/// Click test project: one card (card, center (3,2), 2x2) + two mouse rules —
/// left-click on it flips it and records the click's world x (condition uses event.x, data filtered by event.entity),
/// right-click anywhere flips it back.
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

        // Left-click on the card (world coordinates): picking returns the entity name directly, the next tick the rule flips it
        let r = rpc(port, "input/click", json!({"x": 3.5, "y": 2.0}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["result"]["event"], json!("mouse"));
        assert_eq!(r["result"]["entity"], json!("card"));
        rpc(port, "sim/step", json!({}));
        let card = rpc(port, "world/get", json!({"entity": "@card"}));
        assert_eq!(card["result"]["components"]["Card"]["flipped"], json!(true), "{card}");
        assert_eq!(card["result"]["components"]["Card"]["last_x"], json!(3.5), "条件/取值都该来自 event.x");

        // Left-click on empty ground: entity null, filter does not match, state unchanged
        let r = rpc(port, "input/click", json!({"x": 50.0, "y": 50.0}));
        assert_eq!(r["result"]["entity"], json!(null), "{r}");
        rpc(port, "sim/step", json!({}));
        let card = rpc(port, "world/get", json!({"entity": "@card"}));
        assert_eq!(card["result"]["components"]["Card"]["flipped"], json!(true));

        // Right-click → mouse-alt → flip back
        let r = rpc(port, "input/click", json!({"x": 3.0, "y": 2.0, "button": "right"}));
        assert_eq!(r["result"]["event"], json!("mouse-alt"), "{r}");
        rpc(port, "sim/step", json!({}));
        let card = rpc(port, "world/get", json!({"entity": "@card"}));
        assert_eq!(card["result"]["components"]["Card"]["flipped"], json!(false), "{card}");

        // mouse / mouse-alt are visible in the event log (the agent observes them too, outside the rules)
        let events = rpc(port, "events/recent", json!({}));
        let names: Vec<&str> =
            events["result"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"mouse"), "{names:?}");
        assert!(names.contains(&"mouse-alt"), "{names:?}");

        // Clean exit so the recording is flushed to disk
        rpc(port, "sim/quit", json!({}));
        let mut game = game;
        let status = game.child.wait().unwrap();
        assert!(status.success());
    }

    // Clicks along with pick results are all in the recording's reply channel
    let rec: Recording = serde_json::from_str(&fs::read_to_string(&rec_path).unwrap()).unwrap();
    let names: Vec<&str> = rec.replies.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["mouse", "mouse", "mouse-alt"], "{names:?}");
    assert_eq!(rec.replies[0].data["entity"], json!("card"));
    assert_eq!(rec.replies[1].data["entity"], json!(null));

    // Offline replay: clicks are injected from the recording, reproduced checkpoint by checkpoint (the clear-recording certificate for mouse games is established this way)
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
