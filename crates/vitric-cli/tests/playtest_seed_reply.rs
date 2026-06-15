//! 种子式探索重放外部回复（reply）的真 boot 集成验收。
//!
//! Bug：`vitric playtest --seed-recording` 种子探索只重放种子录像的 **inputs**，不重放它的
//! **replies**（LLM 等外部内容）。结果：靠 reply 才走到的结局连**基线**（未扰动的种子，本该原样
//! 复现通关）都复现不出来——和 `Sim::replay` 不同口径。
//!
//! 这里用 `reply-gated` 极简项目（仿 jump 结构、过 vitric check）验证修复：它**只有**收到一条
//! `oracle-says {answer:"open"}` 外部回复才会 emit `game-won`，光按 input（`ask`）通不了关。
//! 构造一条「到 win」的种子录像（含那条 reply）→ 种子探索 N 局 → 断言**基线那局（perturbation #0）
//! 复现到 Win**（证明 replies 被按原 tick 注回去了，和 Sim::replay 同口径）。
//! 反证：同一计划不传种子回复 → 基线 Timeout（reply 确实是通关的唯一来源）。

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{perturb_plan, run_seed_swarm, Outcome, TerminalSpec};
use vitric_sim::{InputRecord, Recording, ReplyRecord};

fn reply_gated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures/reply-gated")
}

/// 一条到 win 的种子录像：tick 1 按 `ask`（自身通不了关），tick 3 收一条
/// `oracle-says {answer:"open"}` 外部回复——靠它才 emit game-won。
fn seed_to_win() -> Recording {
    Recording {
        inputs: vec![InputRecord {
            tick: 1,
            action: "ask".into(),
            phase: "pressed".into(),
        }],
        replies: vec![ReplyRecord {
            tick: 3,
            name: "oracle-says".into(),
            data: serde_json::json!({ "answer": "open" }),
        }],
        ticks: 20,
        ..Default::default()
    }
}

/// 工厂：每线程自己 boot 一份运行时 + 复制只读 Engine（QuickJS 非 Send 同款约束）。
fn factory(
    dir: PathBuf,
) -> impl Fn() -> Result<(vitric_sim::Sim, Runtime, vitric_rules::Engine), String> + Sync {
    move || {
        let (sim, rt) = Runtime::boot(&dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    }
}

#[test]
fn seed_exploration_replays_reply_so_baseline_reaches_win() {
    let dir = reply_gated_dir();
    let seed = seed_to_win();
    // N 条扰动（含基线=第 0 条）。扰动只动 input，回复跟着种子走。
    let plan = perturb_plan(&seed, 8, 12345);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    // 带种子回复跑：基线（#0）该复现到 Win（回复被按原 tick 注回去了）
    let results = run_seed_swarm(
        factory(dir.clone()),
        &plan,
        &seed.replies,
        200,
        TerminalSpec::default(),
        777,
        threads,
    )
    .expect("种子探索 swarm 应跑通");
    assert_eq!(
        results[0].spec.seed, 0,
        "第 0 条是基线（未扰动种子）"
    );
    assert_eq!(
        results[0].outcome(),
        Outcome::Win,
        "基线注回了种子回复，必须复现到 Win，实际 {:?}",
        results[0].outcome()
    );

    // 反证：同一计划不传种子回复 → 没有 oracle-says → game-won 永不 emit → 基线 Timeout
    let no_reply = run_seed_swarm(
        factory(dir.clone()),
        &plan,
        &[],
        200,
        TerminalSpec::default(),
        777,
        threads,
    )
    .expect("种子探索 swarm 应跑通");
    assert_eq!(
        no_reply[0].outcome(),
        Outcome::Timeout,
        "不重放种子回复时基线复现不出 Win（这正是修复前的 bug），实际 {:?}",
        no_reply[0].outcome()
    );
}
