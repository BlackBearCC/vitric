//! LLM 档埋雷验收（设计稿十一节第 5 阶段）：在叙事埋雷项目 `branching` 上跑一局 **LLM 拟人玩**
//! （真 `Runtime::boot` + 假 LLM 客户端，不碰网络），断言两件事：
//!   1. **LLM 吐的定性 note 进报告 qualitative_notes**——脚本化假客户端在某 tick 回一条
//!      `{"action":..., "note":"管理员这句和上一幕矛盾"}`，断言这条矛盾 note 出现在聚合报告里；
//!   2. **LLM 那局录像可重放复现**——LLM 推理不确定，但它选的输入照样录进录像，
//!      `Sim::replay` 必须逐位复现通过（这是「可定位」的根基）。
//!
//! 还验：非 LLM 路径不受影响（不传 client 的策略档局没有 note、行为不变）。
//! 假客户端按提示词内容决定动作（读 observation 的世界态），不靠脆弱的调用计数。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use vitric_cli::runtime::Runtime;
use vitric_playtest::{
    aggregate_with_endings, run_llm_sessions, run_session, LlmClient, LlmStrategy, Outcome,
    SessionConfig, TerminalSpec,
};

fn branching_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vitric-playtest/tests/fixtures/branching")
}

/// 工厂：每次 boot 一份运行时 + 复制只读 Engine（和别的 playtest 集成测试同款）。
fn factory(
    dir: PathBuf,
) -> impl Fn() -> Result<(vitric_sim::Sim, Runtime, vitric_rules::Engine), String> {
    move || {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    }
}

/// 假 LLM 客户端：读提示词里的世界态，**像拟人一样**一步步把 branching 玩到 ending-bad
/// （take-key → open-door → go-bad），并在做 open-door 那一步顺手吐一条剧情矛盾 note。
///
/// 决策完全看提示词（不靠调用计数，免得 no-op tick 错位）：
/// - 还没拿钥匙（"key": false）→ take-key；
/// - 拿了钥匙、门还没开（"door_open": false）→ open-door（这一步附矛盾 note）；
/// - 门开了 → go-bad（奔 ending-bad）。
///
/// 全程不碰网络。`prompts` 记下每次收到的提示词，便于断言。
struct ScriptedFake {
    prompts: Mutex<Vec<String>>,
}

impl ScriptedFake {
    fn new() -> ScriptedFake {
        ScriptedFake { prompts: Mutex::new(Vec::new()) }
    }
}

impl LlmClient for ScriptedFake {
    fn complete(&self, prompt: &str) -> Result<String, String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        // 提示词里 observation 是 pretty JSON：直接子串判世界态推进到哪一步
        let key_taken = prompt.contains("\"key\": true");
        let door_open = prompt.contains("\"door_open\": true");
        let reply = if !key_taken {
            r#"{"action": "take-key", "phase": "pressed"}"#.to_string()
        } else if !door_open {
            // 开门这一步顺手吐一条剧情连续性矛盾 note（埋雷验收的核心断言对象）
            r#"{"action": "open-door", "phase": "pressed", "note": "管理员这句和上一幕矛盾", "kind": "continuity"}"#
                .to_string()
        } else {
            r#"{"action": "go-bad", "phase": "pressed"}"#.to_string()
        };
        Ok(reply)
    }
}

#[test]
fn llm_note_lands_in_report_and_its_recording_replays() {
    let dir = branching_dir();
    let client: Arc<dyn LlmClient> = Arc::new(ScriptedFake::new());

    // 跑 1 局 LLM 拟人玩（用真 boot，假 client）
    let terminal = TerminalSpec::default();
    let results = run_llm_sessions(
        factory(dir.clone()),
        client,
        1,
        "把这局玩到一个结局",
        0,
        50,
        terminal.clone(),
    )
    .expect("LLM 档应跑通");
    assert_eq!(results.len(), 1);
    let lr = &results[0];

    // LLM 一步步玩到了 ending-bad（拟人玩真推进了游戏，不是空转）
    assert_eq!(lr.result.outcome, Outcome::Win, "ending-bad 归 Win，LLM 应玩到");
    assert!(
        lr.result.fired_events.contains(&"ending-bad".to_string()),
        "LLM 应触达 ending-bad，实际 {:?}",
        lr.result.fired_events
    );
    // 它选的输入进了录像（take-key/open-door/go-bad）
    let actions: Vec<&str> = lr.result.recording.inputs.iter().map(|r| r.action.as_str()).collect();
    assert!(actions.contains(&"open-door"), "LLM 选的输入应进录像: {actions:?}");

    // 1) 聚合报告：那条矛盾 note 进 qualitative_notes
    let (_, rt) = Runtime::boot(&dir).unwrap();
    let report = aggregate_with_endings(&results, &rt.rules, &terminal);
    assert!(
        report.qualitative_notes.total >= 1,
        "应至少收到一条 LLM note，实际 {:?}",
        report.qualitative_notes
    );
    let contradiction = report
        .qualitative_notes
        .clusters
        .iter()
        .find(|c| c.text.contains("矛盾"))
        .expect("矛盾 note 必须出现在 qualitative_notes");
    assert_eq!(contradiction.kind, "continuity", "kind 应归一为 continuity");
    assert!(contradiction.representative.ticks > 0, "note 簇应挂得到可重放代表录像");
    // summary 诚实标「待人复核」
    assert!(report.summary.contains("待人复核"), "summary 应标 LLM note 待复核: {}", report.summary);

    // 2) LLM 那局录像可重放复现（LLM 非确定，但它选的输入录下来照样逐位重放）
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.replay(&lr.result.recording, &mut rt)
        .expect("LLM 局录像必须可 Sim::replay 逐位复现");
}

#[test]
fn llm_session_recording_replays_via_direct_strategy() {
    // 直接用 LlmStrategy + run_session 跑（不经 run_llm_sessions），同样断言录像可重放——
    // 验「LLM 策略选的输入走的就是普通 inject_input 录像通道」。
    let dir = branching_dir();
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    let engine = rt.rules.clone();
    let client: Box<dyn LlmClient> = Box::new(ScriptedFake::new());
    let mut strat = LlmStrategy::new(client, "玩到结局", 7);
    let cfg = SessionConfig { max_ticks: 50, seed: 7, terminal: TerminalSpec::default(), ..Default::default() };
    let res = run_session(&mut sim, &mut rt, &engine, &mut strat, &cfg).unwrap();

    assert_eq!(res.outcome, Outcome::Win, "LLM 应玩到 ending-bad");
    // 这局直接拿到的 notes 里有矛盾 note（session 收 note 通道通了）
    assert!(
        res.notes.iter().any(|n| n.text.contains("矛盾") && n.kind == "continuity"),
        "session.notes 应含矛盾 note，实际 {:?}",
        res.notes
    );
    // 录像可重放
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.replay(&res.recording, &mut rt2).expect("LLM 局录像必须可重放");
}

#[test]
fn non_llm_path_unchanged_no_notes() {
    // 非 LLM 路径不受影响：纯策略档（脚本回放）的局不产 note、行为不变。
    use vitric_playtest::ScriptedStrategy;
    use vitric_sim::{InputRecord, Recording};

    let dir = branching_dir();
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    let engine = rt.rules.clone();
    let seed = Recording {
        inputs: vec![
            InputRecord { tick: 1, action: "take-key".into(), phase: "pressed".into() },
            InputRecord { tick: 3, action: "open-door".into(), phase: "pressed".into() },
            InputRecord { tick: 5, action: "go-bad".into(), phase: "pressed".into() },
        ],
        ticks: 20,
        ..Default::default()
    };
    let mut strat = ScriptedStrategy::from_inputs(&seed.inputs, None);
    let cfg = SessionConfig { max_ticks: 50, seed: 0, terminal: TerminalSpec::default(), ..Default::default() };
    let res = run_session(&mut sim, &mut rt, &engine, &mut strat, &cfg).unwrap();
    assert_eq!(res.outcome, Outcome::Win, "脚本回放应到 ending-bad");
    assert!(res.notes.is_empty(), "非 LLM 策略不产 note，notes 必须空");
}

#[test]
fn llm_session_outcome_is_deterministic_with_deterministic_fake() {
    // 假 client 是确定的（同提示词同回复）→ 整局也确定。验「LLM 局录像确定与否取决于 client」：
    // 用确定的假 client 时，两次跑录像逐字节一致（真 LLM 非确定，那是 client 的事，本档兜不住）。
    let dir = branching_dir();
    let run = || {
        let client: Arc<dyn LlmClient> = Arc::new(ScriptedFake::new());
        run_llm_sessions(factory(dir.clone()), client, 1, "", 0, 50, TerminalSpec::default())
            .unwrap()
            .remove(0)
    };
    let a = run();
    let b = run();
    let ja = serde_json::to_string(&a.result.recording).unwrap();
    let jb = serde_json::to_string(&b.result.recording).unwrap();
    assert_eq!(ja, jb, "确定的假 client → LLM 局录像逐字节一致");
}
