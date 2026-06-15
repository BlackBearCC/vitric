//! 运行时 LLM — 把模型回复变成「录下来的输入」。
//!
//! 约定事件（和 play-sound 一样是引擎层约定，模拟内核不认识 LLM）：
//! - 规则/脚本 emit `llm-ask`，data `{"id": "关联键", "prompt": "提示词"}`；
//!   id 由游戏逻辑自选，用来把回复对回提问方。
//! - 引擎在每 tick 的可观测事件里捕获 llm-ask，丢给**一个**后台工作线程排队发 HTTP
//!   （OpenAI 兼容 chat/completions）。模拟循环从不等网络。
//! - 回复到达后经 [`vitric_sim::Sim::inject_reply`] 注入事件
//!   `llm-reply {id, text}`；任何失败（未配置/网络/格式）注入 `llm-error {id, message}`，
//!   显式可见，绝不静默吞掉。
//!
//! 确定性边界：inject_reply 是和按键输入同级的**录制通道**——回复内容连同到达 tick
//! 一起进录像。`vitric replay` 重放时回复从录像注入、llm-ask 无人监听，
//! **重放永远不碰网络**（cmd_replay 根本不构造本模块）。
//!
//! 配置只认环境变量（不进项目数据，密钥不落盘）：
//! - `VITRIC_LLM_URL`   如 https://api.openai.com/v1/chat/completions（任何兼容端点）
//! - `VITRIC_LLM_KEY`   Bearer 密钥
//! - `VITRIC_LLM_MODEL` 模型名
//!
//! 三个缺一个 = disabled：横幅明说，llm-ask 立刻收到 llm-error 回复（也走录像通道，
//! 所以「没配 LLM 的那局」重放同样逐位一致）。

use std::sync::mpsc;

use serde_json::{json, Value};

use vitric_rules::Event;
use vitric_sim::Sim;

/// LLM 端点配置（全部来自环境变量）。
pub struct LlmConfig {
    pub url: String,
    pub key: String,
    pub model: String,
}

impl LlmConfig {
    /// 读环境变量。缺任何一个返回 Err（拿去当横幅的 disabled 原因）。
    pub fn from_env() -> Result<LlmConfig, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        match (get("VITRIC_LLM_URL"), get("VITRIC_LLM_KEY"), get("VITRIC_LLM_MODEL")) {
            (Some(url), Some(key), Some(model)) => Ok(LlmConfig { url, key, model }),
            _ => Err("未配置 VITRIC_LLM_URL/KEY/MODEL".to_string()),
        }
    }
}

/// 提交给工作线程的一次提问。
struct Ask {
    id: String,
    prompt: String,
}

/// 工作线程交回的一次结果。
struct Done {
    id: String,
    result: Result<String, String>,
}

/// LLM 通道。内部两种形态：
/// - 启用 = 一个后台工作线程顺序消化请求队列（请求多了排队，不并发轰端点）；
/// - 未启用 = 配置缺失，llm-ask 一律立刻回 llm-error。
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
    /// 按环境变量装配：配齐了起工作线程，没配齐降级为 disabled（合法状态，横幅明说）。
    pub fn from_env() -> Llm {
        match LlmConfig::from_env() {
            Ok(cfg) => Llm::start(cfg),
            Err(reason) => Llm::disabled(reason),
        }
    }

    /// 未启用形态（reason 进横幅和 llm-error 消息）。
    pub fn disabled(reason: String) -> Llm {
        Llm(Mode::Disabled { reason })
    }

    /// 用给定配置起后台工作线程（测试用桩服务器配置直接从这进，不碰环境变量）。
    pub fn start(cfg: LlmConfig) -> Llm {
        let (ask_tx, ask_rx) = mpsc::channel::<Ask>();
        let (done_tx, done_rx) = mpsc::channel::<Done>();
        let model = cfg.model.clone();
        // 单工作线程：请求按提交顺序串行执行；引擎侧只 try_recv，从不阻塞等它。
        // 引擎退出 → ask_tx 析构 → for 循环结束 → 线程自然收摊，不用 join。
        std::thread::spawn(move || {
            for ask in ask_rx {
                let result = call_endpoint(&cfg, &ask.prompt);
                if done_tx.send(Done { id: ask.id, result }).is_err() {
                    break; // 引擎侧已退出，没人收了
                }
            }
        });
        Llm(Mode::Enabled { model, tx: ask_tx, rx: done_rx })
    }

    /// 启动横幅的 llm 字段。
    pub fn banner(&self) -> String {
        match &self.0 {
            Mode::Enabled { model, .. } => format!("ok (model {model})"),
            Mode::Disabled { reason } => format!("disabled: {reason}"),
        }
    }
}

/// 从一帧的可观测事件里捕获 llm-ask：合法的提交给工作线程，
/// 不合法/未启用的**立刻**注入 llm-error 回复（走录像通道，显式不静默）。
pub fn handle_ask_events(llm: &mut Llm, events: &[Event], sim: &mut Sim) {
    for e in events {
        if e.name != "llm-ask" {
            continue;
        }
        // id 尽力取出来（哪怕出错也要让游戏逻辑对得上是哪次提问炸了）
        let id = e.data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let prompt = e.data.get("prompt").and_then(|v| v.as_str());
        let error = |message: String| json!({"id": id, "message": message});
        match (e.data.get("id").and_then(|v| v.as_str()), prompt) {
            (Some(_), Some(prompt)) => match &llm.0 {
                Mode::Enabled { tx, .. } => {
                    if tx.send(Ask { id: id.clone(), prompt: prompt.to_string() }).is_err() {
                        // 工作线程死了（只可能是 panic）——显式报给游戏，别让提问石沉大海
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

/// 收割工作线程已完成的回复，注入模拟（每帧/每 tick 调一次，只 try_recv 不阻塞）。
/// 注入即录像：回复内容连同消化它的 tick 一起被 Recording 记录。
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

/// 组装 OpenAI 兼容 chat/completions 请求体。
pub fn build_request_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    })
}

/// 解析 chat/completions 响应：取 choices[0].message.content。
/// 格式不对显式报错并附上拿到的内容（截断），别让游戏拿到一个空串猜半天。
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
            // 回显截断按字符不按字节，别把多字节字符切成半个
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

/// 同步问一次 LLM（直接阻塞当前线程，返回回复文本或显式错误）。
///
/// 运行时的游戏循环走 [`Llm`] 的异步队列（绝不阻塞主循环）；但 **playtest 的 LLM 档**是另一
/// 条路——它本就慢、单独限流、不在游戏帧里（设计稿九节），同步阻塞最直白，正好给
/// playtest 的 `LlmClient` 适配器包一层。复用同一套请求/解析（build_request_body/parse_completion），
/// 端点行为和运行时 LLM 完全一致。
pub fn complete_sync(cfg: &LlmConfig, prompt: &str) -> Result<String, String> {
    call_endpoint(cfg, prompt)
}

/// 在工作线程里执行一次 HTTP 调用（同步阻塞，主循环看不见这段等待）。
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
        // 合法 JSON 但不是 chat/completions 形状 → 指明期望格式并回显内容
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
        // 注入的 llm-error 在下一 step 以事件形式出现
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
        // 缺 prompt 字段
        let asks = vec![Event::new("llm-ask", json!({"id": "npc-1"}))];
        handle_ask_events(&mut llm, &asks, &mut sim);
        let events = collect_one_step(&mut sim);
        let err = events.iter().find(|e| e.name == "llm-error").expect("必须有 llm-error");
        let msg = err.data.get("message").and_then(|v| v.as_str()).unwrap();
        assert!(msg.contains("id 和 prompt"), "{msg}");
    }

    /// 推一步并收回事件（() 逻辑不消费，事件在 StepReport 里）。
    fn collect_one_step(sim: &mut Sim) -> Vec<Event> {
        sim.step(&mut ()).unwrap().events
    }

    #[test]
    fn worker_round_trip_against_local_stub() {
        // 本地桩端点：回一份 canned 的 chat/completions
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/chat/completions", server.server_addr());
        let handle = std::thread::spawn(move || {
            let mut req = server.recv().unwrap();
            // 顺手验证请求形状：模型名 + 用户消息进了请求体
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

        // 轮询收割（测试上限 5 秒，正常毫秒级到达）
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
        // 桩端点回 500：必须变成显式 llm-error，不是静默没下文
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
