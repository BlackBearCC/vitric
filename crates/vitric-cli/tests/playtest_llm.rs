//! LLM tier mine-laying acceptance (design draft section 11 stage 5): run one session of **LLM
//! persona play** on the narrative mine-laying project `branching` (real `Runtime::boot` + fake
//! LLM client, no network touched), asserting two things:
//!   1. **The qualitative note the LLM emits enters the report qualitative_notes** — the scripted
//!      fake client returns a `{"action":..., "note":"this line contradicts the previous scene"}` at some tick;
//!      assert this contradiction note appears in the aggregated report;
//!   2. **The LLM session's recording is replayable** — LLM inference is non-deterministic, but
//!      the inputs it chooses are still recorded; `Sim::replay` must reproduce bit-by-bit (this
//!      is the foundation of "locatability").
//!
//! Also verifies: the non-LLM path is unaffected (sessions of strategies without a client have
//! no notes, behavior unchanged).
//! The fake client decides actions based on prompt content (reads the world state from
//! observation), not on fragile call counting.

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

/// Factory: boot a runtime each time + clone the read-only Engine (same as the other playtest
/// integration tests).
fn factory(
    dir: PathBuf,
) -> impl Fn() -> Result<(vitric_sim::Sim, Runtime, vitric_rules::Engine), String> {
    move || {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    }
}

/// Fake LLM client: reads the world state from the prompt, **like a persona** step by step plays
/// branching to ending-bad (take-key → open-door → go-bad), and at the open-door step emits a
/// plot-contradiction note along the way.
///
/// Decisions are entirely based on the prompt (not call counting, to avoid no-op tick
/// misalignment):
/// - key not yet taken ("key": false) → take-key;
/// - key taken, door not yet open ("door_open": false) → open-door (this step attaches the
///   contradiction note);
/// - door open → go-bad (heading to ending-bad).
///
/// No network touched throughout. `prompts` records every received prompt for assertions.
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
        // observation in the prompt is pretty JSON: directly substring-check the world state to
        // see which step we are at
        let key_taken = prompt.contains("\"key\": true");
        let door_open = prompt.contains("\"door_open\": true");
        let reply = if !key_taken {
            r#"{"action": "take-key", "phase": "pressed"}"#.to_string()
        } else if !door_open {
            // Emit a plot-continuity contradiction note on the door-open step (the core assertion
            // target of mine-laying acceptance)
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

    // Run 1 session of LLM persona play (real boot, fake client)
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

    // The LLM step-by-step played to ending-bad (persona play really advanced the game, not
    // spinning)
    assert_eq!(lr.result.outcome, Outcome::Win, "ending-bad 归 Win，LLM 应玩到");
    assert!(
        lr.result.fired_events.contains(&"ending-bad".to_string()),
        "LLM 应触达 ending-bad，实际 {:?}",
        lr.result.fired_events
    );
    // The inputs it chose entered the recording (take-key/open-door/go-bad)
    let actions: Vec<&str> = lr.result.recording.inputs.iter().map(|r| r.action.as_str()).collect();
    assert!(actions.contains(&"open-door"), "LLM 选的输入应进录像: {actions:?}");

    // 1) Aggregated report: the contradiction note enters qualitative_notes
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
    // summary honestly marks "pending human review"
    assert!(report.summary.contains("待人复核"), "summary 应标 LLM note 待复核: {}", report.summary);

    // 2) The LLM session's recording is replayable (LLM is non-deterministic, but the inputs it
    // chose were recorded and still replay bit-by-bit)
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.replay(&lr.result.recording, &mut rt)
        .expect("LLM 局录像必须可 Sim::replay 逐位复现");
}

#[test]
fn llm_session_recording_replays_via_direct_strategy() {
    // Run directly with LlmStrategy + run_session (not via run_llm_sessions), same recording
    // replay assertion — verifies "the inputs chosen by the LLM strategy go through the normal
    // inject_input recording channel".
    let dir = branching_dir();
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    let engine = rt.rules.clone();
    let client: Box<dyn LlmClient> = Box::new(ScriptedFake::new());
    let mut strat = LlmStrategy::new(client, "玩到结局", 7);
    let cfg = SessionConfig { max_ticks: 50, seed: 7, terminal: TerminalSpec::default(), ..Default::default() };
    let res = run_session(&mut sim, &mut rt, &engine, &mut strat, &cfg).unwrap();

    assert_eq!(res.outcome, Outcome::Win, "LLM 应玩到 ending-bad");
    // The notes from this session include the contradiction note (the session note channel works)
    assert!(
        res.notes.iter().any(|n| n.text.contains("矛盾") && n.kind == "continuity"),
        "session.notes 应含矛盾 note，实际 {:?}",
        res.notes
    );
    // Recording is replayable
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.replay(&res.recording, &mut rt2).expect("LLM 局录像必须可重放");
}

#[test]
fn non_llm_path_unchanged_no_notes() {
    // Non-LLM path is unaffected: pure strategy sessions (script replay) produce no notes,
    // behavior unchanged.
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
    // The fake client is deterministic (same prompt → same reply) → the whole session is also
    // deterministic. Verify "whether an LLM session recording is deterministic depends on the
    // client": with a deterministic fake client, two runs produce byte-identical recordings (a
    // real LLM is non-deterministic, that is the client's business, this tier cannot help).
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
