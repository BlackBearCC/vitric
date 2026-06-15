//! LLM 档代理（设计稿二节「LLM 档」、十一节第 5 阶段）——少量 LLM 代理读同一份
//! Scene View **拟人玩** + 吐定性 note（清晰度/连续性/选择有效性）。
//!
//! 关键：**LLM 推理不确定，但它选的输入照样录进录像 → 可重放复现**。所以本模块只负责
//! 「把视图拼成提示词 → 问 LLM → 解析回的动作 + note」，选出的动作走和廉价策略档**完全
//! 一样**的注入/录像通道（session 的 `inject_input`）。那一局的录像因此可被 `Sim::replay`
//! 逐位复现——LLM 的不确定只影响「选了什么」，不影响「选过之后能不能重放」。
//!
//! **依赖方向**：抽象 `LlmClient` 定义在本 crate（vitric-playtest），真实现（包住运行时
//! 的 `llm::Llm` 发 HTTP）住在 vitric-cli。cli 已依赖 playtest，反过来不依赖——不成环。
//! 测试用 `FakeLlmClient`（按提示词返罐头 JSON）驱动，**不碰网络**。

use serde::Deserialize;
use vitric_sim::Pcg32;

use crate::scene_view::{Action, SceneView};
use crate::strategy::{PlaytestNote, Strategy};

/// LLM 客户端抽象：给一段提示词，返一段回复文本（或显式错误）。
///
/// 故意只有这一个最窄的方法——让 [`LlmStrategy`] 可测、不绑网络。真实现（vitric-cli）
/// 把它接到 OpenAI 兼容端点；测试用 [`FakeLlmClient`] 返罐头回复。失败用 `Err(String)`
/// 显式暴露（不静默吞），由策略转成一条 note 并退化为「本 tick 不操作」。
pub trait LlmClient: Send + Sync {
    /// 问一次 LLM。`Ok(回复文本)` / `Err(原因)`——错误显式，绝不静默。
    fn complete(&self, prompt: &str) -> Result<String, String>;
}

/// LLM 期望返回的结构化回复：选哪个动作 + 一条可选的定性 note。
/// 约定 JSON 形如 `{"action": "<actions 之一>", "note": "<可选>", "kind": "<可选>"}`。
/// `action` 为空串或省略 = 本 tick 不操作（拟人玩里「先观察、不动手」也是合法选择）。
#[derive(Debug, Deserialize)]
struct LlmReply {
    /// 选中的动作名（必须是 view.actions 里某个的 `action` 字段）。空/缺 = 不操作。
    #[serde(default)]
    action: String,
    /// 可选的 phase（pressed/released）。缺省按 pressed——拟人玩绝大多数是「按下」。
    #[serde(default)]
    phase: Option<String>,
    /// 可选的定性 note 正文。非空才记一条 note。
    #[serde(default)]
    note: Option<String>,
    /// 可选的 note 类型（clarity/continuity/choice）。缺省归一到 "other"。
    #[serde(default)]
    kind: Option<String>,
}

/// LLM 拟人玩策略：把 SceneView 拼成提示词 → 问 client → 解析动作 + note。
///
/// - 解析出的动作若**合法**（在 view.actions 里）就返回它注入；
/// - 解析失败 / 动作非法 / client 报错：**记一条 note 说明**，退化为 `None`（本 tick 不操作），
///   **绝不 panic**——LLM 不可控，本策略对它的任何坏输出都得稳稳兜住。
/// - note 累积在 `notes` 里，由 session 每 tick `drain_notes` 收走（取走即清空）。
///
/// `rng`/`tick` 只用来给 note 标 tick + 在「LLM 给了非法/空动作」时**不**乱选——拟人玩
/// 不靠随机数选动作（那是 random 策略的活），LLM 不给合法动作就老实不动手。`rng` 留作
/// 将来需要打散并列动作时用，当前不消费，保持确定的 tick 计数。
pub struct LlmStrategy {
    /// LLM 客户端（trait 对象——真实现发 HTTP，测试用罐头）。
    client: Box<dyn LlmClient>,
    /// 目标描述（拼进提示词的「你的目标」一行）。从 playtest 配置/规则来，默认通用一句。
    goal: String,
    /// 当前决策 tick（每 choose 一次 +1）——给 note 标「在哪一刻看的视图」。
    tick: u64,
    /// 累积的 note，等 session drain 走。
    notes: Vec<PlaytestNote>,
    /// 预留的 PCG（当前不消费，见类型注释）——带上它让构造签名和别的策略一致、将来好扩。
    #[allow(dead_code)]
    rng: Pcg32,
}

impl LlmStrategy {
    /// 造一个 LLM 策略。`client` 是 LLM 客户端，`goal` 是拼进提示词的目标描述
    /// （空串则用通用默认），`seed` 给预留 PCG 播种（当前不消费但保持确定）。
    pub fn new(client: Box<dyn LlmClient>, goal: &str, seed: u64) -> LlmStrategy {
        let goal = if goal.trim().is_empty() {
            "把这局玩到一个结局（通关或任意 ending），过程中留意哪里看不懂、哪里前后矛盾、哪个选项没意义。".to_string()
        } else {
            goal.to_string()
        };
        LlmStrategy { client, goal, tick: 0, notes: Vec::new(), rng: Pcg32::new(seed) }
    }

    /// 记一条 note（内部用）。`tick` 是这条 note 所属的决策 tick（由 choose 在自增前捕获，
    /// 不能用 self.tick——那已被 choose 自增过了）。kind 归一到 clarity/continuity/choice/other。
    fn push_note(&mut self, tick: u64, kind: &str, text: String) {
        self.notes.push(PlaytestNote { tick, kind: normalize_kind(kind), text });
    }
}

impl Strategy for LlmStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        let tick = self.tick;
        self.tick += 1;

        // 没有可选动作就别问 LLM（省一次外呼）——拟人也无从下手
        if view.actions.is_empty() {
            return None;
        }

        let prompt = build_prompt(&self.goal, view);
        let raw = match self.client.complete(&prompt) {
            Ok(text) => text,
            Err(e) => {
                // LLM 外呼失败：显式记一条 note，本 tick 不操作（绝不 panic）
                self.notes.push(PlaytestNote {
                    tick,
                    kind: "other".to_string(),
                    text: format!("LLM 调用失败：{e}"),
                });
                return None;
            }
        };

        // 解析回复：从文本里抠出 JSON 对象（容忍 LLM 在 JSON 前后裹了寒暄/```json 围栏）
        let reply = match parse_reply(&raw) {
            Ok(r) => r,
            Err(e) => {
                self.push_note(tick, "other", format!("LLM 回复解析失败（{e}）：{}", truncate(&raw, 120)));
                return None;
            }
        };

        // 先收 note（哪怕动作非法，note 也是有价值的定性反馈——别因为动作问题丢了 note）
        if let Some(text) = reply.note.as_deref().filter(|t| !t.trim().is_empty()) {
            self.push_note(tick, reply.kind.as_deref().unwrap_or("other"), text.to_string());
        }

        // 空动作 = 拟人「先观察不动手」，合法
        if reply.action.trim().is_empty() {
            return None;
        }
        let phase = reply.phase.as_deref().unwrap_or("pressed");
        let chosen = Action { action: reply.action.clone(), phase: phase.to_string() };

        // 动作必须在合法集合里（拟人玩不能凭空造词汇——那会注入引擎不认识的输入）
        if view.actions.contains(&chosen) {
            Some(chosen)
        } else {
            // LLM 选了个不在词汇里的动作：记一条 note，退化为不操作
            self.push_note(
                tick,
                "other",
                format!(
                    "LLM 选了非法动作「{} ({})」，不在可选清单里，本 tick 不操作。",
                    reply.action, phase
                ),
            );
            None
        }
    }

    fn drain_notes(&mut self) -> Vec<PlaytestNote> {
        std::mem::take(&mut self.notes)
    }
}

/// 把 kind 归一到四类（LLM 自报的 kind 可能五花八门，收敛到固定标签便于报告分组）。
fn normalize_kind(raw: &str) -> String {
    let k = raw.trim().to_ascii_lowercase();
    match k.as_str() {
        "clarity" | "clear" | "confusing" | "confusion" => "clarity",
        "continuity" | "consistency" | "contradiction" | "contradict" => "continuity",
        "choice" | "choices" | "meaningless" | "useless-choice" => "choice",
        _ => "other",
    }
    .to_string()
}

/// 喂给 LLM 的提示词（设计稿二节「拟人玩 + 吐定性 note」）。拼三块：目标、当前所见（观测+
/// 可选动作）、回复格式约定。要求 LLM 只返一个 JSON 对象，挑一个动作、可选附一条定性 note。
fn build_prompt(goal: &str, view: &SceneView) -> String {
    // 动作清单：编号列出来，让 LLM 照着挑（也方便它在 note 里引用「这个选项」）
    let actions: Vec<String> = view
        .actions
        .iter()
        .map(|a| format!("- {} ({})", a.action, a.phase))
        .collect();
    let observation =
        serde_json::to_string_pretty(&view.observation).unwrap_or_else(|_| "{}".to_string());
    format!(
        "你在像真人一样试玩一个游戏，判断它哪里有问题。\n\
         你的目标：{goal}\n\n\
         【当前所见（机器投影的世界状态，已剔除纯画面装饰）】\n{observation}\n\n\
         【本刻可做的动作】\n{actions}\n\n\
         请选一个动作推进游戏。如果你看不懂该干嘛、发现剧情前后矛盾、或觉得某个选项没意义，\
         在 note 里说出来（kind 用 clarity / continuity / choice 之一）。\n\
         只返回一个 JSON 对象，不要任何别的文字，格式：\n\
         {{\"action\": \"<上面动作名之一，留空表示先观察不动手>\", \"phase\": \"pressed\", \
         \"note\": \"<可选，定性感受>\", \"kind\": \"<可选 clarity|continuity|choice>\"}}",
        goal = goal,
        observation = observation,
        actions = actions.join("\n"),
    )
}

/// 从 LLM 回复文本里解析出 [`LlmReply`]。容忍回复在 JSON 前后裹了寒暄或 ```json 围栏——
/// 抠出第一个 `{` 到最后一个 `}` 之间的子串当 JSON 解析。抠不出对象/解析失败都返 Err
/// （显式，让 choose 转成 note，不静默）。
fn parse_reply(raw: &str) -> Result<LlmReply, String> {
    let start = raw.find('{').ok_or("回复里没有 JSON 对象")?;
    let end = raw.rfind('}').ok_or("回复里没有闭合的 JSON 对象")?;
    if end < start {
        return Err("JSON 括号不成对".to_string());
    }
    let slice = &raw[start..=end];
    serde_json::from_str::<LlmReply>(slice).map_err(|e| format!("JSON 解析错误: {e}"))
}

/// 截断字符串（按字符不按字节，别切碎多字节字）——note 里回显 LLM 原始回复时用。
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! 测试用假 LLM 客户端（脚本化罐头回复，不碰网络）。也给集成测试用（pub(crate)）。
    use std::sync::Mutex;

    use super::LlmClient;

    /// 按调用顺序返预设回复的假客户端。可脚本化多轮：第 i 次 complete 返 replies[i]，
    /// 超出范围循环用最后一条（或在精确脚本里就把每轮都填好）。
    pub struct FakeLlmClient {
        replies: Vec<Result<String, String>>,
        /// 已调用次数（内部可变 + Mutex，让 &self 的 complete 能记数，满足 Send+Sync）。
        calls: Mutex<usize>,
    }

    impl FakeLlmClient {
        /// 每轮都返同一条回复。
        pub fn always(reply: &str) -> FakeLlmClient {
            FakeLlmClient { replies: vec![Ok(reply.to_string())], calls: Mutex::new(0) }
        }

        /// 脚本化：第 i 次调用返 script[i]，调用数超出脚本则复用最后一条。
        pub fn scripted(script: Vec<&str>) -> FakeLlmClient {
            FakeLlmClient {
                replies: script.into_iter().map(|s| Ok(s.to_string())).collect(),
                calls: Mutex::new(0),
            }
        }

        /// 每轮都返错误（测「LLM 外呼失败 → 记 note + 不操作」）。
        pub fn always_err(msg: &str) -> FakeLlmClient {
            FakeLlmClient { replies: vec![Err(msg.to_string())], calls: Mutex::new(0) }
        }

        /// 这个假客户端被问了几次（断言外呼次数）。
        pub fn call_count(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl LlmClient for FakeLlmClient {
        fn complete(&self, _prompt: &str) -> Result<String, String> {
            let mut n = self.calls.lock().unwrap();
            let idx = (*n).min(self.replies.len().saturating_sub(1));
            *n += 1;
            self.replies[idx].clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeLlmClient;
    use super::*;
    use serde_json::json;

    fn view_with_actions(names: &[&str]) -> SceneView {
        let actions = names
            .iter()
            .map(|n| Action { action: n.to_string(), phase: "pressed".to_string() })
            .collect();
        SceneView { observation: json!({"entities": []}), actions, done: None }
    }

    // ---- 提示词构造 ----

    #[test]
    fn prompt_includes_goal_actions_and_observation() {
        let view = SceneView {
            observation: json!({"entities": [{"name": "hero", "components": {"Pos": {"x": 1}}}]}),
            actions: vec![
                Action { action: "left".into(), phase: "pressed".into() },
                Action { action: "jump".into(), phase: "pressed".into() },
            ],
            done: None,
        };
        let prompt = build_prompt("逃出房间", &view);
        assert!(prompt.contains("逃出房间"), "提示词应含目标");
        assert!(prompt.contains("left"), "提示词应列出动作 left");
        assert!(prompt.contains("jump"), "提示词应列出动作 jump");
        assert!(prompt.contains("hero"), "提示词应含观测里的实体");
        assert!(prompt.contains("\"action\""), "提示词应说明回复 JSON 格式");
        assert!(prompt.contains("clarity") && prompt.contains("continuity") && prompt.contains("choice"));
    }

    // ---- 回复解析（合法 / 非法 / 缺字段） ----

    #[test]
    fn parse_reply_extracts_action_and_note() {
        let r = parse_reply(r#"{"action": "left", "note": "看不懂", "kind": "clarity"}"#).unwrap();
        assert_eq!(r.action, "left");
        assert_eq!(r.note.as_deref(), Some("看不懂"));
        assert_eq!(r.kind.as_deref(), Some("clarity"));
    }

    #[test]
    fn parse_reply_tolerates_chatter_and_fences() {
        // LLM 常在 JSON 前后裹寒暄或 ```json 围栏——照样抠得出
        let raw = "好的，我选择：\n```json\n{\"action\": \"jump\"}\n```\n希望有帮助";
        let r = parse_reply(raw).unwrap();
        assert_eq!(r.action, "jump");
        assert!(r.note.is_none());
    }

    #[test]
    fn parse_reply_missing_action_defaults_empty() {
        // 缺 action 字段：默认空串（= 不操作），不报错
        let r = parse_reply(r#"{"note": "只是观察一下"}"#).unwrap();
        assert_eq!(r.action, "");
        assert_eq!(r.note.as_deref(), Some("只是观察一下"));
    }

    #[test]
    fn parse_reply_rejects_non_json() {
        assert!(parse_reply("我不知道该选什么").is_err(), "无 JSON 对象应报错");
        assert!(parse_reply("").is_err());
    }

    // ---- choose：合法动作 / 非法动作 / 解析失败 / 外呼失败，都不 panic ----

    #[test]
    fn choose_returns_legal_action_from_llm() {
        let client = Box::new(FakeLlmClient::always(r#"{"action": "left"}"#));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left", "right"]);
        let a = s.choose(&view).expect("合法动作应被选中");
        assert_eq!(a.action, "left");
        assert_eq!(a.phase, "pressed");
    }

    #[test]
    fn choose_illegal_action_degrades_to_none_with_note() {
        // LLM 选了个不在词汇里的动作 → 退化为 None + 记一条 note（不 panic）
        let client = Box::new(FakeLlmClient::always(r#"{"action": "teleport"}"#));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left", "right"]);
        assert_eq!(s.choose(&view), None, "非法动作退化为不操作");
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].text.contains("非法动作"), "{:?}", notes[0]);
    }

    #[test]
    fn choose_malformed_reply_degrades_to_none_with_note() {
        let client = Box::new(FakeLlmClient::always("我选左边"));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        assert_eq!(s.choose(&view), None, "解析失败退化为不操作");
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].text.contains("解析失败"), "{:?}", notes[0]);
    }

    #[test]
    fn choose_client_error_degrades_to_none_with_note() {
        let client = Box::new(FakeLlmClient::always_err("额度耗尽"));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        assert_eq!(s.choose(&view), None, "外呼失败退化为不操作");
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].text.contains("LLM 调用失败"), "{:?}", notes[0]);
        assert!(notes[0].text.contains("额度耗尽"));
    }

    #[test]
    fn choose_empty_actions_skips_llm_call() {
        // 没有可选动作就不问 LLM（省外呼）。用 Arc 共享 client 以便策略跑完读它的调用数。
        use std::sync::Arc;
        struct ArcClient(Arc<FakeLlmClient>);
        impl LlmClient for ArcClient {
            fn complete(&self, p: &str) -> Result<String, String> {
                self.0.complete(p)
            }
        }
        let inner = Arc::new(FakeLlmClient::always(r#"{"action": "x"}"#));
        let mut s = LlmStrategy::new(Box::new(ArcClient(inner.clone())), "", 0);
        // 空动作集：choose 应直接返 None，且根本没问 LLM
        assert_eq!(s.choose(&view_with_actions(&[])), None);
        assert_eq!(inner.call_count(), 0, "空动作不该外呼 LLM");
        // 有动作时才会真问一次
        let _ = s.choose(&view_with_actions(&["x"]));
        assert_eq!(inner.call_count(), 1, "有动作时问一次 LLM");
    }

    // ---- note 累积与归一 ----

    #[test]
    fn notes_accumulate_across_ticks_and_drain_clears() {
        // 两轮都吐 note：累积两条，drain 后清空
        let client = Box::new(FakeLlmClient::scripted(vec![
            r#"{"action": "left", "note": "这步看不懂", "kind": "clarity"}"#,
            r#"{"action": "right", "note": "管理员这句和上一幕矛盾", "kind": "continuity"}"#,
        ]));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left", "right"]);
        let _ = s.choose(&view); // tick 0
        let _ = s.choose(&view); // tick 1
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].tick, 0);
        assert_eq!(notes[0].kind, "clarity");
        assert_eq!(notes[1].tick, 1);
        assert_eq!(notes[1].kind, "continuity");
        assert!(notes[1].text.contains("矛盾"));
        // drain 取走即清空
        assert!(s.drain_notes().is_empty(), "drain 后应清空");
    }

    #[test]
    fn note_kind_is_normalized() {
        // LLM 自报五花八门的 kind → 归一到固定四类
        let client = Box::new(FakeLlmClient::always(
            r#"{"action": "left", "note": "x", "kind": "Contradiction"}"#,
        ));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        let _ = s.choose(&view);
        let notes = s.drain_notes();
        assert_eq!(notes[0].kind, "continuity", "Contradiction 应归一到 continuity");
    }

    #[test]
    fn note_kept_even_when_action_illegal() {
        // 动作非法但带了 note：note 仍要收（别因为动作问题丢了定性反馈）+ 一条非法动作 note
        let client = Box::new(FakeLlmClient::always(
            r#"{"action": "fly", "note": "这关没出口", "kind": "clarity"}"#,
        ));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        assert_eq!(s.choose(&view), None);
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 2, "一条定性 note + 一条非法动作 note");
        assert!(notes.iter().any(|n| n.text.contains("这关没出口")));
        assert!(notes.iter().any(|n| n.text.contains("非法动作")));
    }

    #[test]
    fn empty_note_is_not_recorded() {
        // note 为空串/全空白：不记
        let client = Box::new(FakeLlmClient::always(r#"{"action": "left", "note": "  "}"#));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        let _ = s.choose(&view);
        assert!(s.drain_notes().is_empty(), "空 note 不记");
    }

    #[test]
    fn empty_action_means_no_op_but_keeps_note() {
        // action 留空 = 拟人「先观察不动手」，note 照收
        let client = Box::new(FakeLlmClient::always(r#"{"action": "", "note": "先看看", "kind": "clarity"}"#));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        assert_eq!(s.choose(&view), None, "空动作 = 不操作");
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "先看看");
    }
}
