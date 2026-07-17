//! Runtime LLM — turns model replies into "recorded input".
//!
//! Conventional events (like play-sound, an engine-layer convention; the simulation core does not
//! know about LLM):
//! - Rules/scripts emit `llm-ask`, data `{"id": "correlation key", "prompt": "prompt text"}`;
//!   the id is chosen by game logic, used to route the reply back to the asker.
//! - The engine captures llm-ask from each tick's observable events and hands it to **one**
//!   background worker thread that queues HTTP calls (OpenAI-compatible chat/completions). The
//!   simulation loop never waits on the network.
//! - When a reply arrives it is injected via [`vitric_sim::Sim::inject_reply`] as an event
//!   `llm-reply {id, text}`; any failure (unconfigured / network / format) is injected as
//!   `llm-error {id, message}`, explicitly visible, never silently swallowed.
//!
//! Determinism boundary: inject_reply is a **recording channel** on the same level as keypress
//! input — the reply content along with the tick it arrives at go into the recording. During
//! `vitric replay`, replies are injected from the recording and llm-ask has no listener, so
//! **replay never touches the network** (cmd_replay does not construct this module at all).
//!
//! Configuration only reads environment variables (does not go into project data, keys never
//! touch disk):
//! - `VITRIC_LLM_URL`   e.g. https://api.openai.com/v1/chat/completions (any compatible endpoint)
//! - `VITRIC_LLM_KEY`   Bearer secret
//! - `VITRIC_LLM_MODEL` model name
//!
//! Missing any one of the three = disabled: the banner says so explicitly, and llm-ask immediately
//! receives an llm-error reply (also via the recording channel, so "the session with no LLM
//! configured" replays bit-for-bit identically).

use std::sync::mpsc;

use serde_json::{json, Value};

use vitric_rules::Event;
use vitric_sim::Sim;

/// LLM endpoint configuration (all from environment variables).
pub struct LlmConfig {
    pub url: String,
    pub key: String,
    pub model: String,
}

impl LlmConfig {
    /// Read environment variables. Missing any one returns Err (used as the banner's disabled reason).
    pub fn from_env() -> Result<LlmConfig, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        match (get("VITRIC_LLM_URL"), get("VITRIC_LLM_KEY"), get("VITRIC_LLM_MODEL")) {
            (Some(url), Some(key), Some(model)) => Ok(LlmConfig { url, key, model }),
            _ => Err("未配置 VITRIC_LLM_URL/KEY/MODEL".to_string()),
        }
    }
}

/// One question submitted to the worker thread.
struct Ask {
    id: String,
    prompt: String,
}

/// One result handed back by the worker thread.
struct Done {
    id: String,
    result: Result<String, String>,
}

/// LLM channel. Two internal forms:
/// - Enabled = one background worker thread drains the request queue in order (requests queue up,
///   no concurrent hammering of the endpoint);
/// - Disabled = configuration missing; llm-ask always gets an immediate llm-error reply.
pub struct Llm(Mode);

enum Mode {
    Enabled {
        model: String,
        tx: mpsc::Sender<Ask>,
        rx: mpsc::Receiver<Done>,
    },
    Disabled {
        reason: String,
    },
}

impl Llm {
    /// Assemble from environment variables: if all are present, start the worker thread; if not,
    /// degrade to disabled (a legal state, the banner says so explicitly).
    pub fn from_env() -> Llm {
        match LlmConfig::from_env() {
            Ok(cfg) => Llm::start(cfg),
            Err(reason) => Llm::disabled(reason),
        }
    }

    /// Disabled form (reason goes into the banner and llm-error messages).
    pub fn disabled(reason: String) -> Llm {
        Llm(Mode::Disabled { reason })
    }

    /// Start a background worker thread with the given config (a stub server config for tests goes
    /// in directly here, without touching environment variables).
    pub fn start(cfg: LlmConfig) -> Llm {
        let (ask_tx, ask_rx) = mpsc::channel::<Ask>();
        let (done_tx, done_rx) = mpsc::channel::<Done>();
        let model = cfg.model.clone();
        // Single worker thread: requests execute serially in submission order; the engine side
        // only try_recv, never blocks waiting on it. Engine exit → ask_tx dropped → for loop ends
        // → thread winds down naturally, no join needed.
        std::thread::spawn(move || {
            for ask in ask_rx {
                let result = call_endpoint(&cfg, &ask.prompt);
                if done_tx.send(Done { id: ask.id, result }).is_err() {
                    break; // the engine side has exited, no one is receiving
                }
            }
        });
        Llm(Mode::Enabled { model, tx: ask_tx, rx: done_rx })
    }

    /// The llm field of the startup banner.
    pub fn banner(&self) -> String {
        match &self.0 {
            Mode::Enabled { model, .. } => format!("ok (model {model})"),
            Mode::Disabled { reason } => format!("disabled: {reason}"),
        }
    }
}

/// Capture llm-ask from a frame's observable events: valid ones are submitted to the worker
/// thread; invalid / disabled ones **immediately** inject an llm-error reply (via the recording
/// channel, explicit, not silent).
pub fn handle_ask_events(llm: &mut Llm, events: &[Event], sim: &mut Sim) {
    for e in events {
        if e.name != "llm-ask" {
            continue;
        }
        // Best-effort extract of id (even on error, game logic must be able to tell which ask blew up)
        let id = e.data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let prompt = e.data.get("prompt").and_then(|v| v.as_str());
        let error = |message: String| json!({"id": id, "message": message});
        match (e.data.get("id").and_then(|v| v.as_str()), prompt) {
            (Some(_), Some(prompt)) => match &llm.0 {
                Mode::Enabled { tx, .. } => {
                    if tx.send(Ask { id: id.clone(), prompt: prompt.to_string() }).is_err() {
                        // Worker thread died (can only be a panic) — explicitly report to the game,
                        // don't let the ask disappear into the void
                        sim.inject_reply("llm-error", error("LLM 工作线程已退出".to_string()));
                    }
                }
                Mode::Disabled { reason } => {
                    sim.inject_reply("llm-error", error(format!("LLM 未启用：{reason}")));
                }
            },
            _ => {
                sim.inject_reply(
                    "llm-error",
                    error(
                        "llm-ask 事件的 data 必须含文本字段 id 和 prompt，\
                         写法 {\"emit\": \"llm-ask\", \"data\": {\"id\": \"npc-1\", \"prompt\": \"...\"}}"
                            .to_string(),
                    ),
                );
            }
        }
    }
}

/// Reap replies already completed by the worker thread and inject them into the simulation
/// (called once per frame / per tick; only try_recv, never blocks). Injection is recording: the
/// reply content along with the tick that consumes it are recorded by Recording.
pub fn pump_replies(llm: &mut Llm, sim: &mut Sim) {
    let Mode::Enabled { rx, .. } = &llm.0 else { return };
    while let Ok(done) = rx.try_recv() {
        match done.result {
            Ok(text) => sim.inject_reply("llm-reply", json!({"id": done.id, "text": text})),
            Err(message) => {
                sim.inject_reply("llm-error", json!({"id": done.id, "message": message}))
            }
        }
    }
}

/// Assemble an OpenAI-compatible chat/completions request body.
pub fn build_request_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    })
}

/// Parse a chat/completions response: take choices[0].message.content. On a format mismatch,
/// explicitly error with the received content (truncated) attached — don't hand the game an empty
/// string to puzzle over.
pub fn parse_completion(body: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| format!("LLM 响应不是合法 JSON: {e}"))?;
    v.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            // Truncate the echo by character count, not byte count, so multi-byte characters are
            // not split in half
            let shown = if body.chars().count() > 200 {
                format!("{}…", body.chars().take(200).collect::<String>())
            } else {
                body.to_string()
            };
            format!(
                "LLM 响应缺 choices[0].message.content（应为 OpenAI chat/completions 格式），\
                 实际响应: {shown}"
            )
        })
}

/// Synchronously ask the LLM once (blocks the current thread directly, returns the reply text or
/// an explicit error).
///
/// The runtime game loop goes through [`Llm`]'s async queue (never blocks the main loop); but
/// **playtest's LLM profile** is a different path — it is inherently slow, rate-limited separately,
/// and not inside a game frame (design doc section nine), so synchronous blocking is the most
/// straightforward, and is exactly what playtest's `LlmClient` adapter wraps. It reuses the same
/// request/parse code (build_request_body/parse_completion), so the endpoint behavior is identical
/// to the runtime LLM.
pub fn complete_sync(cfg: &LlmConfig, prompt: &str) -> Result<String, String> {
    call_endpoint(cfg, prompt)
}

/// Execute one HTTP call inside the worker thread (synchronous blocking; the main loop never sees
/// this wait).
fn call_endpoint(cfg: &LlmConfig, prompt: &str) -> Result<String, String> {
    let body = build_request_body(&cfg.model, prompt);
    let mut resp = ureq::post(&cfg.url)
        .header("Authorization", &format!("Bearer {}", cfg.key))
        .send_json(&body)
        .map_err(|e| format!("LLM 请求 {} 失败: {e}", cfg.url))?;
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("LLM 响应读取失败: {e}"))?;
    parse_completion(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_is_openai_chat_shape() {
        let body = build_request_body("gpt-x", "你是谁");
        assert_eq!(body["model"], json!("gpt-x"));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][0]["content"], json!("你是谁"));
    }

    #[test]
    fn parse_completion_extracts_text() {
        let body = r#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"宝箱在东边"},"finish_reason":"stop"}]}"#;
        assert_eq!(parse_completion(body).unwrap(), "宝箱在东边");
    }

    #[test]
    fn parse_completion_rejects_malformed_explicitly() {
        let err = parse_completion("not json at all").unwrap_err();
        assert!(err.contains("不是合法 JSON"), "{err}");
        // Valid JSON but not chat/completions shape → point out the expected format and echo the content
        let err = parse_completion(r#"{"error": {"message": "quota exceeded"}}"#).unwrap_err();
        assert!(err.contains("choices[0].message.content"), "{err}");
        assert!(err.contains("quota exceeded"), "{err}");
    }

    #[test]
    fn disabled_llm_replies_with_explicit_error() {
        let mut llm = Llm::disabled("未配置 VITRIC_LLM_URL/KEY/MODEL".to_string());
        let mut sim = Sim::new(1);
        let asks =
            vec![Event::new("llm-ask", json!({"id": "npc-1", "prompt": "hi"}))];
        handle_ask_events(&mut llm, &asks, &mut sim);
        // The injected llm-error appears as an event in the next step
        let report_events = collect_one_step(&mut sim);
        let errs: Vec<_> = report_events.iter().filter(|e| e.name == "llm-error").collect();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].data.get("id"), Some(&json!("npc-1")));
        let msg = errs[0].data.get("message").and_then(|v| v.as_str()).unwrap();
        assert!(msg.contains("未配置 VITRIC_LLM_URL/KEY/MODEL"), "{msg}");
    }

    #[test]
    fn malformed_ask_replies_with_explicit_error() {
        let mut llm = Llm::disabled("x".to_string());
        let mut sim = Sim::new(1);
        // Missing the prompt field
        let asks = vec![Event::new("llm-ask", json!({"id": "npc-1"}))];
        handle_ask_events(&mut llm, &asks, &mut sim);
        let events = collect_one_step(&mut sim);
        let err = events.iter().find(|e| e.name == "llm-error").expect("必须有 llm-error");
        let msg = err.data.get("message").and_then(|v| v.as_str()).unwrap();
        assert!(msg.contains("id 和 prompt"), "{msg}");
    }

    /// Step once and collect the events (the () logic consumes nothing; events are in StepReport).
    fn collect_one_step(sim: &mut Sim) -> Vec<Event> {
        sim.step(&mut ()).unwrap().events
    }

    #[test]
    fn worker_round_trip_against_local_stub() {
        // Local stub endpoint: return a canned chat/completions response
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/chat/completions", server.server_addr());
        let handle = std::thread::spawn(move || {
            let mut req = server.recv().unwrap();
            // Also verify the request shape: model name + user message went into the request body
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body).unwrap();
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["model"], json!("stub-model"));
            assert_eq!(v["messages"][0]["content"], json!("说一句欢迎词"));
            let resp = r#"{"choices":[{"message":{"role":"assistant","content":"旅人，欢迎来到玻璃镇"}}]}"#;
            req.respond(
                tiny_http::Response::from_string(resp).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                ),
            )
            .unwrap();
        });

        let mut llm = Llm::start(LlmConfig {
            url,
            key: "test-key".to_string(),
            model: "stub-model".to_string(),
        });
        let mut sim = Sim::new(1);
        let asks = vec![Event::new("llm-ask", json!({"id": "npc-1", "prompt": "说一句欢迎词"}))];
        handle_ask_events(&mut llm, &asks, &mut sim);

        // Poll and reap (test limit 5 seconds; normally arrives in milliseconds)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let reply = loop {
            pump_replies(&mut llm, &mut sim);
            let events = collect_one_step(&mut sim);
            if let Some(e) = events.into_iter().find(|e| e.name == "llm-reply") {
                break e;
            }
            assert!(std::time::Instant::now() < deadline, "5 秒内未收到 llm-reply");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(reply.data.get("id"), Some(&json!("npc-1")));
        assert_eq!(reply.data.get("text"), Some(&json!("旅人，欢迎来到玻璃镇")));
        handle.join().unwrap();
    }

    #[test]
    fn http_error_becomes_llm_error_reply() {
        // Stub endpoint returns 500: must become an explicit llm-error, not silence with no follow-up
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/chat/completions", server.server_addr());
        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            req.respond(tiny_http::Response::from_string("boom").with_status_code(500)).unwrap();
        });

        let mut llm = Llm::start(LlmConfig {
            url,
            key: "k".to_string(),
            model: "m".to_string(),
        });
        let mut sim = Sim::new(1);
        let asks = vec![Event::new("llm-ask", json!({"id": "q1", "prompt": "hi"}))];
        handle_ask_events(&mut llm, &asks, &mut sim);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let err = loop {
            pump_replies(&mut llm, &mut sim);
            let events = collect_one_step(&mut sim);
            if let Some(e) = events.into_iter().find(|e| e.name == "llm-error") {
                break e;
            }
            assert!(std::time::Instant::now() < deadline, "5 秒内未收到 llm-error");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(err.data.get("id"), Some(&json!("q1")));
        let msg = err.data.get("message").and_then(|v| v.as_str()).unwrap();
        assert!(msg.contains("失败"), "{msg}");
        handle.join().unwrap();
    }
}
