//! Runtime LLM end-to-end: rule emits llm-ask → stub HTTP endpoint → reply goes through
//! inject_reply to become an llm-reply event → rule writes event.text into the world; recorded
//! throughout, offline replay bit-identical.
//! No real network touched (stub server is in-process), no env vars touched (config constructed
//! directly, no cross-test bleed).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use vitric_cli::llm::{self, Llm, LlmConfig};
use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

/// Minimal NPC dialogue project: ask on start, llm-reply writes into Text, llm-error also writes
/// into Text (explicitly visible).
fn write_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-llm-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("scenes")).unwrap();
    fs::create_dir_all(dir.join("rules")).unwrap();
    fs::write(
        dir.join("vitric.json"),
        json!({
            "name": "llm-npc",
            "schema": "schema.json",
            "entry": "scenes/main.json",
            "scenes": ["scenes/main.json"],
            "rules": ["rules/dialogue.json"],
            "scripts": [],
            "seed": 11
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        json!({
            "components": {
                "Npc": {"fields": {}},
                "Text": {"fields": {"content": {"type": "text", "default": ""}}}
            }
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        json!({
            "entities": [
                {"name": "npc", "components": {"Npc": {}, "Text": {"content": ""}}}
            ]
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("rules/dialogue.json"),
        json!({
            "rules": [
                {
                    "id": "greet-ask",
                    "on": {"event": "start"},
                    "do": [{"emit": "llm-ask", "data": {"id": "npc-1", "prompt": "说一句欢迎词"}}]
                },
                {
                    "id": "greet-reply",
                    "on": {"event": "llm-reply", "filter": {"id": "npc-1"}},
                    "do": [{"set": "@npc.Text.content", "to": "event.text"}]
                },
                {
                    "id": "greet-error",
                    "on": {"event": "llm-error"},
                    "do": [{"set": "@npc.Text.content", "to": "event.message"}]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
    dir
}

/// Simulate one tick of the main loop (same order as main.rs's step_once: step → observe events
/// → ask → pump).
fn tick_once(sim: &mut vitric_sim::Sim, rt: &mut Runtime, llm: &mut Llm) {
    sim.step(rt).unwrap();
    let observed = rt.drain_observed();
    llm::handle_ask_events(llm, &observed, sim);
    llm::pump_replies(llm, sim);
}

fn npc_text(sim: &vitric_sim::Sim) -> String {
    let npc = sim.world.entity("npc").unwrap();
    sim.world.get_field(npc, "Text.content").unwrap().as_str().unwrap().to_string()
}

#[test]
fn llm_reply_drives_rules_and_recording_replays_offline() {
    // Stub endpoint: returns a canned chat/completions
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/chat/completions", server.server_addr());
    let stub = std::thread::spawn(move || {
        let req = server.recv().unwrap();
        let resp = r#"{"choices":[{"message":{"role":"assistant","content":"旅人，欢迎来到玻璃镇"}}]}"#;
        req.respond(
            tiny_http::Response::from_string(resp).with_header(
                tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
            ),
        )
        .unwrap();
    });

    let dir = write_project("e2e");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    let mut llm = Llm::start(LlmConfig {
        url,
        key: "test-key".to_string(),
        model: "stub-model".to_string(),
    });

    // Recording starts at tick 0: the llm-ask dispatch and reply injection are recorded
    // throughout
    sim.start_recording();
    let deadline = Instant::now() + Duration::from_secs(5);
    while npc_text(&sim).is_empty() {
        assert!(Instant::now() < deadline, "5 秒内 NPC 没拿到台词");
        tick_once(&mut sim, &mut rt, &mut llm);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(npc_text(&sim), "旅人，欢迎来到玻璃镇");
    // After the reply lands, run a bit more so the recording has the post-reply trajectory
    for _ in 0..30 {
        tick_once(&mut sim, &mut rt, &mut llm);
    }
    let rec = sim.stop_recording().unwrap();
    assert_eq!(rec.replies.len(), 1, "录像必须记下这条 LLM 回复");
    assert_eq!(rec.replies[0].name, "llm-reply");
    assert_eq!(rec.replies[0].data.get("text"), Some(&json!("旅人，欢迎来到玻璃镇")));

    // Offline replay: no LLM assembled, the stub server has packed up — replies all come from
    // the recording, every checkpoint consistent
    stub.join().unwrap();
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.replay(&rec, &mut rt2).expect("带 LLM 内容的录像必须离线逐位重放");
    assert_eq!(sim2.world.state_hash(), rec.final_hash);
    assert_eq!(npc_text(&sim2), "旅人，欢迎来到玻璃镇");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn disabled_llm_injects_explicit_error_reply() {
    let dir = write_project("disabled");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    let mut llm = Llm::disabled("未配置 VITRIC_LLM_URL/KEY/MODEL".to_string());

    // tick 0 rule asks → immediately injects llm-error → tick 1 rule digests, writes into Text
    tick_once(&mut sim, &mut rt, &mut llm);
    tick_once(&mut sim, &mut rt, &mut llm);
    let text = npc_text(&sim);
    assert!(text.contains("未配置 VITRIC_LLM_URL/KEY/MODEL"), "NPC 应拿到显式错误，实际: {text}");

    fs::remove_dir_all(&dir).unwrap();
}
