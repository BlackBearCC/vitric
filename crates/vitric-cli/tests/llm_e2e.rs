//! 运行时 LLM 端到端：规则 emit llm-ask → 桩 HTTP 端点 → 回复经 inject_reply
//! 变成 llm-reply 事件 → 规则把 event.text 写进世界；全程录像，离线重放逐位一致。
//! 不碰真实网络（桩服务器在本进程），不碰环境变量（配置直接构造，测试间不串）。

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use vitric_cli::llm::{self, Llm, LlmConfig};
use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

/// 最小 NPC 对话项目：start 时提问，llm-reply 写进 Text，llm-error 也写进 Text（显式可见）。
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

/// 模拟主循环的一个 tick（与 main.rs 的 step_once 同序：step → 观测事件 → ask → pump）。
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
    // 桩端点：回一份 canned 的 chat/completions
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

    // 录像从 tick 0 开始：llm-ask 的发起和回复的注入全程被录
    sim.start_recording();
    let deadline = Instant::now() + Duration::from_secs(5);
    while npc_text(&sim).is_empty() {
        assert!(Instant::now() < deadline, "5 秒内 NPC 没拿到台词");
        tick_once(&mut sim, &mut rt, &mut llm);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(npc_text(&sim), "旅人，欢迎来到玻璃镇");
    // 回复落地后再跑一段，让录像里有回复之后的轨迹
    for _ in 0..30 {
        tick_once(&mut sim, &mut rt, &mut llm);
    }
    let rec = sim.stop_recording().unwrap();
    assert_eq!(rec.replies.len(), 1, "录像必须记下这条 LLM 回复");
    assert_eq!(rec.replies[0].name, "llm-reply");
    assert_eq!(rec.replies[0].data.get("text"), Some(&json!("旅人，欢迎来到玻璃镇")));

    // 离线重放：不装配 LLM、桩服务器已收摊——回复全部来自录像，逐校验点一致
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

    // tick 0 规则发问 → 立刻注入 llm-error → tick 1 规则消化、写进 Text
    tick_once(&mut sim, &mut rt, &mut llm);
    tick_once(&mut sim, &mut rt, &mut llm);
    let text = npc_text(&sim);
    assert!(text.contains("未配置 VITRIC_LLM_URL/KEY/MODEL"), "NPC 应拿到显式错误，实际: {text}");

    fs::remove_dir_all(&dir).unwrap();
}
