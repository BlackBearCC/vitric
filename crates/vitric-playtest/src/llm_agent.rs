//! LLM-tier agent (design draft section 2 "LLM tier", section 11 stage 5) — a small number of LLM
//! agents read the same Scene View and **play human-like** + emit qualitative notes (clarity /
//! continuity / choice effectiveness).
//!
//! Key point: **LLM inference is non-deterministic, but the input it chose is still recorded into
//! the recording → replayable and reproducible**. So this module only does "stitch the view into a
//! prompt → ask the LLM → parse the returned action + note"; the chosen action goes through the
//! **exact same** injection/recording channel as cheap strategy tiers (session's `inject_input`).
//! That session's recording can therefore be bitwise reproduced by `Sim::replay` — the LLM's
//! non-determinism only affects "what was chosen", not "whether it can be replayed after choosing".
//!
//! **Dependency direction**: the abstract `LlmClient` is defined in this crate (vitric-playtest);
//! the real implementation (wrapping the runtime's `llm::Llm` to send HTTP) lives in vitric-cli.
//! cli already depends on playtest, not the other way around — no cycle.
//! Tests use `FakeLlmClient` (returns canned JSON based on the prompt) and **don't touch the network**.

use serde::Deserialize;
use vitric_sim::Pcg32;

use crate::scene_view::{Action, SceneView};
use crate::strategy::{PlaytestNote, Strategy};

/// LLM client abstraction: given a prompt, return a reply text (or an explicit error).
///
/// Deliberately only this one narrowest method — keeps [`LlmStrategy`] testable and not tied to
/// the network. The real implementation (vitric-cli) plugs it into an OpenAI-compatible endpoint;
/// tests use [`FakeLlmClient`] to return canned replies. Failures are surfaced explicitly via
/// `Err(String)` (not silently swallowed), turned by the strategy into a note and degraded to
/// "no action this tick".
pub trait LlmClient: Send + Sync {
    /// Ask the LLM once. `Ok(reply text)` / `Err(reason)` — errors are explicit, never silent.
    fn complete(&self, prompt: &str) -> Result<String, String>;
}

/// The structured reply the LLM is expected to return: which action to pick + an optional
/// qualitative note.
/// The agreed JSON shape is `{"action": "<one of actions>", "note": "<optional>",
/// "kind": "<optional>"}`. An empty or omitted `action` = no action this tick (in human-like play,
/// "observe first, don't act" is also a legitimate choice).
#[derive(Debug, Deserialize)]
struct LlmReply {
    /// The chosen action name (must be one of the `action` fields in view.actions). Empty/missing
    /// = no action.
    #[serde(default)]
    action: String,
    /// Optional phase (pressed/released). Defaults to pressed — human-like play is mostly "press".
    #[serde(default)]
    phase: Option<String>,
    /// Optional qualitative note body. A note is only recorded when non-empty.
    #[serde(default)]
    note: Option<String>,
    /// Optional note kind (clarity/continuity/choice). Defaults to "other".
    #[serde(default)]
    kind: Option<String>,
}

/// LLM human-like-play strategy: stitch the SceneView into a prompt → ask the client → parse the
/// action + note.
///
/// - If the parsed action is **legal** (in view.actions), return it for injection;
/// - On parse failure / illegal action / client error: **record a note explaining it**, degrade to
///   `None` (no action this tick), **never panic** — the LLM is uncontrollable, this strategy must
///   robustly handle any bad output from it.
/// - Notes accumulate in `notes`, drained by session each tick via `drain_notes` (drain = clear).
///
/// `rng`/`tick` are only used to label notes with the tick + to **not** pick randomly when the LLM
/// gives an illegal/empty action — human-like play doesn't pick actions by random numbers (that's
/// the random strategy's job); if the LLM doesn't give a legal action, just honestly do nothing.
/// `rng` is reserved for future use spreading tied actions; currently unconsumed, keeping a
/// deterministic tick count.
pub struct LlmStrategy {
    /// LLM client (trait object — real impl sends HTTP, tests use canned).
    client: Box<dyn LlmClient>,
    /// Goal description (the "your goal" line spliced into the prompt). Comes from the playtest
    /// config/rules; defaults to a generic sentence.
    goal: String,
    /// Current decision tick (incremented by 1 each choose) — labels notes with "the view seen at
    /// that moment".
    tick: u64,
    /// Accumulated notes, waiting for session to drain them.
    notes: Vec<PlaytestNote>,
    /// Reserved PCG (currently unconsumed, see type comment) — kept so the constructor signature
    /// matches other strategies and is easy to extend later.
    #[allow(dead_code)]
    rng: Pcg32,
}

impl LlmStrategy {
    /// Build an LLM strategy. `client` is the LLM client, `goal` is the goal description spliced
    /// into the prompt (empty string uses a generic default), `seed` seeds the reserved PCG
    /// (currently unconsumed but kept deterministic).
    pub fn new(client: Box<dyn LlmClient>, goal: &str, seed: u64) -> LlmStrategy {
        let goal = if goal.trim().is_empty() {
            "把这局玩到一个结局（通关或任意 ending），过程中留意哪里看不懂、哪里前后矛盾、哪个选项没意义。".to_string()
        } else {
            goal.to_string()
        };
        LlmStrategy { client, goal, tick: 0, notes: Vec::new(), rng: Pcg32::new(seed) }
    }

    /// Record a note (internal use). `tick` is the decision tick this note belongs to (captured by
    /// choose before incrementing — don't use self.tick, that's already been incremented by
    /// choose). kind is normalized to clarity/continuity/choice/other.
    fn push_note(&mut self, tick: u64, kind: &str, text: String) {
        self.notes.push(PlaytestNote { tick, kind: normalize_kind(kind), text });
    }
}

impl Strategy for LlmStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        let tick = self.tick;
        self.tick += 1;

        // No actions to choose from → don't ask the LLM (save one external call) — even a
        // human-like player has nothing to do here
        if view.actions.is_empty() {
            return None;
        }

        let prompt = build_prompt(&self.goal, view);
        let raw = match self.client.complete(&prompt) {
            Ok(text) => text,
            Err(e) => {
                // LLM external call failed: explicitly record a note, no action this tick (never
                // panic)
                self.notes.push(PlaytestNote {
                    tick,
                    kind: "other".to_string(),
                    text: format!("LLM 调用失败：{e}"),
                });
                return None;
            }
        };

        // Parse the reply: extract the JSON object from the text (tolerating the LLM wrapping it
        // with chitchat / ```json fences)
        let reply = match parse_reply(&raw) {
            Ok(r) => r,
            Err(e) => {
                self.push_note(tick, "other", format!("LLM 回复解析失败（{e}）：{}", truncate(&raw, 120)));
                return None;
            }
        };

        // Collect the note first (even if the action is illegal, the note is valuable qualitative
        // feedback — don't lose the note because of an action problem)
        if let Some(text) = reply.note.as_deref().filter(|t| !t.trim().is_empty()) {
            self.push_note(tick, reply.kind.as_deref().unwrap_or("other"), text.to_string());
        }

        // Empty action = human-like "observe first, don't act", legal
        if reply.action.trim().is_empty() {
            return None;
        }
        let phase = reply.phase.as_deref().unwrap_or("pressed");
        let chosen = Action { action: reply.action.clone(), phase: phase.to_string() };

        // The action must be in the legal set (human-like play can't fabricate vocabulary — that
        // would inject input the engine doesn't recognize)
        if view.actions.contains(&chosen) {
            Some(chosen)
        } else {
            // The LLM picked an action not in the vocabulary: record a note, degrade to no action
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

/// Normalize kind to four categories (the kind the LLM self-reports may be all over the place;
/// collapse to fixed labels for easier report grouping).
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

/// The prompt fed to the LLM (design draft section 2 "human-like play + qualitative notes").
/// Stitches three parts: goal, what's currently seen (observation + available actions), reply
/// format agreement. Asks the LLM to return only one JSON object, picking an action and optionally
/// attaching a qualitative note.
fn build_prompt(goal: &str, view: &SceneView) -> String {
    // Action list: numbered so the LLM can pick from it (also makes it easy for the LLM to refer
    // to "this option" in a note)
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

/// Parse an [`LlmReply`] from the LLM reply text. Tolerates replies wrapping the JSON with
/// chitchat or ```json fences — extracts the substring from the first `{` to the last `}` and
/// parses it as JSON. Returns Err if no object can be extracted / parsing fails (explicit, so
/// choose can turn it into a note, not silent).
fn parse_reply(raw: &str) -> Result<LlmReply, String> {
    let start = raw.find('{').ok_or("回复里没有 JSON 对象")?;
    let end = raw.rfind('}').ok_or("回复里没有闭合的 JSON 对象")?;
    if end < start {
        return Err("JSON 括号不成对".to_string());
    }
    let slice = &raw[start..=end];
    serde_json::from_str::<LlmReply>(slice).map_err(|e| format!("JSON 解析错误: {e}"))
}

/// Truncate a string (by characters not bytes, don't chop up multi-byte characters) — used when
/// echoing the LLM's raw reply in a note.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! Fake LLM client for tests (scripted canned replies, no network). Also used by integration
    //! tests (pub(crate)).
    use std::sync::Mutex;

    use super::LlmClient;

    /// Fake client that returns preset replies in call order. Can be scripted over multiple rounds:
    /// the i-th complete returns replies[i]; past the range it reuses the last one (or in an exact
    /// script, fill in every round).
    pub struct FakeLlmClient {
        replies: Vec<Result<String, String>>,
        /// Number of calls so far (interior mutability + Mutex, so &self's complete can record
        /// counts, satisfying Send+Sync).
        calls: Mutex<usize>,
    }

    impl FakeLlmClient {
        /// Return the same reply every round.
        pub fn always(reply: &str) -> FakeLlmClient {
            FakeLlmClient { replies: vec![Ok(reply.to_string())], calls: Mutex::new(0) }
        }

        /// Scripted: the i-th call returns script[i]; when call count exceeds the script, reuses
        /// the last one.
        pub fn scripted(script: Vec<&str>) -> FakeLlmClient {
            FakeLlmClient {
                replies: script.into_iter().map(|s| Ok(s.to_string())).collect(),
                calls: Mutex::new(0),
            }
        }

        /// Return an error every round (tests "LLM external call failed → record note + no action").
        pub fn always_err(msg: &str) -> FakeLlmClient {
            FakeLlmClient { replies: vec![Err(msg.to_string())], calls: Mutex::new(0) }
        }

        /// How many times this fake client was asked (asserting external call counts).
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

    // ---- Prompt construction ----

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

    // ---- Reply parsing (legal / illegal / missing fields) ----

    #[test]
    fn parse_reply_extracts_action_and_note() {
        let r = parse_reply(r#"{"action": "left", "note": "看不懂", "kind": "clarity"}"#).unwrap();
        assert_eq!(r.action, "left");
        assert_eq!(r.note.as_deref(), Some("看不懂"));
        assert_eq!(r.kind.as_deref(), Some("clarity"));
    }

    #[test]
    fn parse_reply_tolerates_chatter_and_fences() {
        // The LLM often wraps JSON with chitchat or ```json fences — still extractable
        let raw = "好的，我选择：\n```json\n{\"action\": \"jump\"}\n```\n希望有帮助";
        let r = parse_reply(raw).unwrap();
        assert_eq!(r.action, "jump");
        assert!(r.note.is_none());
    }

    #[test]
    fn parse_reply_missing_action_defaults_empty() {
        // Missing action field: defaults to empty string (= no action), no error
        let r = parse_reply(r#"{"note": "只是观察一下"}"#).unwrap();
        assert_eq!(r.action, "");
        assert_eq!(r.note.as_deref(), Some("只是观察一下"));
    }

    #[test]
    fn parse_reply_rejects_non_json() {
        assert!(parse_reply("我不知道该选什么").is_err(), "无 JSON 对象应报错");
        assert!(parse_reply("").is_err());
    }

    // ---- choose: legal action / illegal action / parse failure / external call failure, none
    // panic ----

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
        // The LLM picked an action not in the vocabulary → degrade to None + record a note (no
        // panic)
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
        // No actions to choose from → don't ask the LLM (save the external call). Use Arc to share
        // the client so the strategy can read its call count after running.
        use std::sync::Arc;
        struct ArcClient(Arc<FakeLlmClient>);
        impl LlmClient for ArcClient {
            fn complete(&self, p: &str) -> Result<String, String> {
                self.0.complete(p)
            }
        }
        let inner = Arc::new(FakeLlmClient::always(r#"{"action": "x"}"#));
        let mut s = LlmStrategy::new(Box::new(ArcClient(inner.clone())), "", 0);
        // Empty action set: choose should return None directly and never ask the LLM
        assert_eq!(s.choose(&view_with_actions(&[])), None);
        assert_eq!(inner.call_count(), 0, "空动作不该外呼 LLM");
        // With actions it actually asks once
        let _ = s.choose(&view_with_actions(&["x"]));
        assert_eq!(inner.call_count(), 1, "有动作时问一次 LLM");
    }

    // ---- Note accumulation and normalization ----

    #[test]
    fn notes_accumulate_across_ticks_and_drain_clears() {
        // Both rounds emit notes: accumulate two, drain clears them
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
        // drain takes and clears
        assert!(s.drain_notes().is_empty(), "drain 后应清空");
    }

    #[test]
    fn note_kind_is_normalized() {
        // The LLM self-reports a wide variety of kinds → normalize to fixed four categories
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
        // Illegal action but carries a note: the note is still collected (don't lose qualitative
        // feedback due to an action problem) + one illegal-action note
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
        // Note is empty string / all whitespace: not recorded
        let client = Box::new(FakeLlmClient::always(r#"{"action": "left", "note": "  "}"#));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        let _ = s.choose(&view);
        assert!(s.drain_notes().is_empty(), "空 note 不记");
    }

    #[test]
    fn empty_action_means_no_op_but_keeps_note() {
        // Empty action = human-like "observe first, don't act", note is still collected
        let client = Box::new(FakeLlmClient::always(r#"{"action": "", "note": "先看看", "kind": "clarity"}"#));
        let mut s = LlmStrategy::new(client, "", 0);
        let view = view_with_actions(&["left"]);
        assert_eq!(s.choose(&view), None, "空动作 = 不操作");
        let notes = s.drain_notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "先看看");
    }
}
