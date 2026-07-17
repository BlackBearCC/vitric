//! Seed-based exploration replays external replies: real-boot integration acceptance.
//!
//! Bug: `vitric playtest --seed-recording` seed exploration only replays the seed recording's
//! **inputs**, not its **replies** (external content such as LLM). Result: an ending reachable
//! only via a reply cannot be reproduced even by the **baseline** (the un-perturbed seed,
//! which should reproduce the clear verbatim) — a different channel from `Sim::replay`.
//!
//! Here we use the minimal `reply-gated` project (mimics jump structure, passes vitric check)
//! to verify the fix: it emits `game-won` **only** when it receives an `oracle-says
//! {answer:"open"}` external reply; pressing the input (`ask`) alone cannot clear. Construct a
//! seed recording "to win" (containing that reply) → run N sessions of seed exploration →
//! assert **the baseline session (perturbation #0) reproduces the Win** (proving replies were
//! re-injected at their original ticks, same channel as Sim::replay).
//! Counter-evidence: the same plan without passing the seed replies → baseline Timeout (the
//! reply is indeed the sole source of the clear).

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;
use vitric_playtest::{perturb_plan, run_seed_swarm, Outcome, TerminalSpec};
use vitric_sim::{InputRecord, Recording, ReplyRecord};

fn reply_gated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures/reply-gated")
}

/// A seed recording to win: tick 1 presses `ask` (alone cannot clear), tick 3 receives an
/// `oracle-says {answer:"open"}` external reply — game-won is emitted thanks to it.
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

/// Factory: each thread boots its own runtime + clones the read-only Engine (same QuickJS
/// non-Send constraint).
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
    // N perturbations (including the baseline = #0). Perturbation only touches inputs; replies
    // follow the seed.
    let plan = perturb_plan(&seed, 8, 12345);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    // Run with the seed replies: the baseline (#0) should reproduce the Win (replies were
    // re-injected at their original ticks)
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

    // Counter-evidence: the same plan without passing the seed replies → no oracle-says →
    // game-won never emits → baseline Timeout
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
