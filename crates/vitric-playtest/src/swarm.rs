//! Swarm batch execution: spreads a "which sessions to run" plan across multiple worker threads in parallel,
//! each session producing a plain-data [`LabeledResult`] (design draft section 7 A2, section 9 perf budget).
//!
//! **Key parallel architecture (avoiding the QuickJS non-Send pitfall)**: the script runtime (QuickJS) is not `Send`,
//! and cannot be moved across threads. So the convention: the caller provides a **factory closure** `factory: Fn() -> (Sim, R, Engine)`,
//! each worker thread **inside its own thread** calls `factory()` to boot a fresh runtime, and only the plain-data
//! results (`SessionResult` — all-Send numbers/strings/recordings) are passed back. Runtime objects never cross
//! the thread boundary — only the "how to run" spec and the "what came out" result flow between threads.
//!
//! **Determinism rule**: each spec carries its own (strategy, seed, max_ticks, terminal); a session's result is
//! decided solely by the spec, not by thread scheduling — so `run_swarm` produces identical results whether run
//! serially or in parallel. Threads decide only "who finishes first", not "what gets produced"; results are
//! re-homed by the spec's original index in the plan.

use std::sync::Arc;
use std::thread;

use vitric_rules::Engine;
use vitric_sim::{GameLogic, ReplyRecord, Sim};

use crate::llm_agent::{LlmClient, LlmStrategy};
use crate::scene_view::{Outcome, TerminalSpec};
use crate::seed::Perturbation;
use crate::session::{run_session, run_session_lookahead, LookaheadConfig, SessionConfig, SessionResult};
use crate::strategy::{
    CoverageStrategy, EconomyStrategy, GreedyStrategy, RandomStrategy, ScriptedStrategy, Strategy,
};

/// Strategy kind (named in the spec; instantiated at run time based on this).
/// A serializable pure label — results carry it back, and the aggregator groups by strategy_kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StrategyKind {
    Random,
    Greedy,
    Coverage,
    /// Economy pressure strategy (sim-management games, finds numeric breakage): locks one action and repeats it R times before rotating (design draft section 4).
    Economy,
    /// Scripted replay session for seed exploration — used only as a **label** (for result grouping). The script itself isn't in SessionSpec
    /// (scripts are variable-length and each one differs); `run_seed_swarm` holds the Perturbation directly to build the strategy.
    Scripted,
    /// LLM human-like play session — used only as a **label** (design draft stage 5). The LLM strategy carries a client (a non-Send trait object
    /// in various shapes), isn't constructed from (kind, seed); `cmd_playtest --llm` holds the LlmStrategy directly to run it,
    /// then tags results with this label to merge them into the swarm result set (notes go into qualitative_notes).
    Llm,
    /// Lookahead planner session (skill/navigation-type specific). It isn't a `Strategy` — sim/logic themselves snapshot/restore
    /// to run beam-search speculative action selection (goes through `run_session_lookahead` not `run_session`), so the search depth
    /// `depth` is attached directly to the variant (beam width uses the default [`DEFAULT_SWARM_LOOKAHEAD_BEAM`]; swarm doesn't override it).
    /// For projects that declare a goal, the default swarm mixes a few of these sessions in (otherwise navigation-type games get misreported as unbeatable).
    /// `run_one` dispatches this variant to `run_session_lookahead` at each session execution point.
    Lookahead { depth: u64 },
}

impl StrategyKind {
    /// Full set of cheap strategy tiers (the CLI rotates through these by default). **Excludes Scripted** — scripted replay goes through
    /// the seed-exploration-specific path (`run_seed_swarm`), not the "breadth-coverage" strategy rotation. Includes Economy: sim-management games need it
    /// to catch economy runaway/collapse, so it's in the default rotation regardless of game type (in non-management games it's just another stress test).
    pub const ALL: [StrategyKind; 4] = [
        StrategyKind::Random,
        StrategyKind::Greedy,
        StrategyKind::Coverage,
        StrategyKind::Economy,
    ];

    /// Short name (for report/CLI display).
    pub fn name(self) -> &'static str {
        match self {
            StrategyKind::Random => "random",
            StrategyKind::Greedy => "greedy",
            StrategyKind::Coverage => "coverage",
            StrategyKind::Economy => "economy",
            StrategyKind::Scripted => "scripted",
            StrategyKind::Llm => "llm",
            StrategyKind::Lookahead { .. } => "lookahead",
        }
    }

    /// Build a strategy instance from kind + seed (PCG-seeded, deterministic and reproducible).
    /// `goal`: greedy's derived goal (declared in playtest.json) — with a goal greedy walks toward it; without one it degrades to random.
    /// Other strategies ignore goal. Scripted/Llm don't go through this path (script/client aren't in the seed) — they explicitly panic rather than silently degrade.
    fn build(self, seed: u64, goal: &Option<crate::config::GoalSpec>) -> Box<dyn Strategy> {
        match self {
            StrategyKind::Random => Box::new(RandomStrategy::new(seed)),
            StrategyKind::Greedy => match goal {
                Some(g) => Box::new(GreedyStrategy::with_goal(seed, g.clone())),
                None => Box::new(GreedyStrategy::new(seed)),
            },
            StrategyKind::Coverage => Box::new(CoverageStrategy::new(seed)),
            StrategyKind::Economy => Box::new(EconomyStrategy::new(seed)),
            StrategyKind::Scripted => {
                panic!("Scripted 策略要带脚本，必须走 run_seed_swarm，不能用 StrategyKind::build")
            }
            StrategyKind::Llm => {
                panic!("Llm 策略要带 client，必须由 cmd_playtest --llm 直接构造 LlmStrategy，不能用 StrategyKind::build")
            }
            StrategyKind::Lookahead { .. } => {
                panic!("Lookahead 不是 Strategy，要 sim/logic 自己投机，必须走 run_session_lookahead，不能用 StrategyKind::build")
            }
        }
    }
}

/// One session's spec: which strategy, which seed, how many ticks, which events count as terminal.
/// A session's result is decided **solely** by it (determinism rule).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionSpec {
    pub strategy_kind: StrategyKind,
    pub seed: u64,
    pub max_ticks: u64,
    pub terminal: TerminalSpec,
}

impl SessionSpec {
    /// Common constructor: default terminal spec.
    pub fn new(strategy_kind: StrategyKind, seed: u64, max_ticks: u64) -> SessionSpec {
        SessionSpec { strategy_kind, seed, max_ticks, terminal: TerminalSpec::default() }
    }
}

/// A session result with its spec label (which strategy/seed produced it + what it produced). The aggregator consumes this.
#[derive(Debug, Clone)]
pub struct LabeledResult {
    pub spec: SessionSpec,
    pub result: SessionResult,
}

impl LabeledResult {
    /// Convenience read: this session's outcome.
    pub fn outcome(&self) -> Outcome {
        self.result.outcome
    }
}

/// **Beam search depth** used by lookahead sessions mixed into the default swarm. Slightly larger than the default depth=8 of single-session `--strategy lookahead`,
/// leaving headroom: skill/navigation-type games need deep enough planning to evaluate the payoff of multi-tick maneuvers like "first jump over the wall, then move right" —
/// the payoff of a nav wall-jump (hero_x grows again after clearing the wall top) only shows up ~5 frames after takeoff, and a depth too shallow (empirically ≤6) misses it
/// and degenerates to marking time in place. depth 12 with beam width 4 is the empirically stable clear-depth on the nav fixture (minimum ~8, leaving 1.5× headroom).
/// Beam search is linear in depth (per real tick the speculative steps ≤ max(W, root-action count)×(B+1)×D, see the perf comment in session.rs);
/// navigation-type B is single-digit, so depth 12 is affordable. Single-session `--strategy lookahead` is unaffected (it uses `--horizon`→
/// depth, default 8; users can explicitly raise it to solve harder multi-tick maneuvers).
pub const DEFAULT_SWARM_LOOKAHEAD_DEPTH: u64 = 12;

/// **Beam width** used by lookahead sessions mixed into the default swarm. Navigation/skill types need to keep a few "first gets worse, then gets better" branches (hugging a wall,
/// taking off, lateral motion coexisting); beam width 1 = pure greedy, which easily prunes the wall-jump path; 4 gives enough exploration headroom without being too slow.
pub const DEFAULT_SWARM_LOOKAHEAD_BEAM: usize = 4;

/// Default swarm session plan: N sessions, cheap strategies (random/greedy/coverage/economy) rotated × incrementing seeds.
///
/// **When a goal is declared, mix in lookahead**: lookahead requires the project to declare a goal (`playtest.json`'s PlaytestConfig.goal)
/// to have a direction; otherwise it's meaningless. So only when `has_goal` is true, a fixed small number of sessions in the plan are swapped to `Lookahead` —
/// navigation/skill types (first jump onto a wall, then walk) get 0% clear rate with random, and would be misreported as unbeatable without mixing in lookahead. Ratio is restrained (beam search is slow:
/// per real tick ≤ W×(B+1)×D speculative steps): take `min(N, max(2, N/4))` sessions (about 25%, at least 2,
/// not exceeding N). **When no goal is declared, not a single session is swapped**, and the default set is entirely unchanged (backward compatible).
///
/// The mixed-in lookahead sessions use [`DEFAULT_SWARM_LOOKAHEAD_DEPTH`] (deeper than the single-session default, so multi-tick maneuvers show their payoff).
/// Which sessions get swapped: the **trailing** ones (the earlier rotation slots are untouched, minimizing the diff and easing reconciliation). Swapped sessions
/// retain their original seed/max_ticks/terminal, only their strategy_kind changes to `Lookahead{depth}`.
pub fn default_plan(
    sessions: u64,
    seed: u64,
    max_ticks: u64,
    terminal: TerminalSpec,
    has_goal: bool,
) -> Vec<SessionSpec> {
    let mut plan: Vec<SessionSpec> = Vec::with_capacity(sessions as usize);
    for k in 0..sessions {
        let kind = StrategyKind::ALL[(k as usize) % StrategyKind::ALL.len()];
        plan.push(SessionSpec { strategy_kind: kind, seed: seed + k, max_ticks, terminal: terminal.clone() });
    }
    if has_goal && sessions > 0 {
        // Number of mixed-in sessions: about 1/4, at least 2, but not exceeding the total session count.
        let look_n = ((sessions / 4).max(2)).min(sessions) as usize;
        // Swap the trailing look_n sessions (the earlier rotation slots are untouched).
        let start = plan.len() - look_n;
        for spec in &mut plan[start..] {
            spec.strategy_kind =
                StrategyKind::Lookahead { depth: DEFAULT_SWARM_LOOKAHEAD_DEPTH };
        }
    }
    plan
}

/// Run one spec (boot a runtime inside the calling thread, run a session, produce a labeled result).
/// Both the serial and parallel paths of swarm converge on this single function — guaranteeing "always the same result no matter how you run it".
/// `config`: per-game view overrides (include/exclude/relabel/derived quantities/goal/terminal). greedy uses its goal
/// to find the target; session uses it for derive_with_config. Default empty config = auto-derive, behavior identical to stages 1~5.
fn run_one<R, F>(
    factory: &F,
    spec: &SessionSpec,
    config: &crate::config::PlaytestConfig,
) -> Result<LabeledResult, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String>,
{
    // Each session boots its own runtime: runtimes aren't reused across sessions (a replayable recording requires a cold start), let alone across threads.
    let (mut sim, mut logic, engine) = factory()?;
    let cfg = SessionConfig {
        max_ticks: spec.max_ticks,
        seed: spec.seed,
        terminal: spec.terminal.clone(),
        playtest: config.clone(),
        // Normal swarm/single-session doesn't replay seed replies (that's a seed-exploration-specific concern for run_seed_swarm).
        seed_replies: Vec::new(),
    };
    // Dispatch: Lookahead isn't a Strategy — sim/logic themselves snapshot/restore to speculatively pick actions, going through
    // run_session_lookahead; the rest build a Strategy as before and go through run_session. Both paths produce SessionResult with the same shape
    // (state_trace/numeric_summary/fired_events/notes/recording all populated), so the aggregator doesn't lack data.
    let result = match spec.strategy_kind {
        StrategyKind::Lookahead { depth } => {
            // swarm-mixed lookahead sessions use the default beam width (DEFAULT_SWARM_LOOKAHEAD_BEAM); depth is carried by the variant.
            run_session_lookahead(
                &mut sim,
                &mut logic,
                &engine,
                &cfg,
                &LookaheadConfig { depth, beam_width: DEFAULT_SWARM_LOOKAHEAD_BEAM },
            )?
        }
        _ => {
            let mut strategy = spec.strategy_kind.build(spec.seed, &config.goal);
            run_session(&mut sim, &mut logic, &engine, strategy.as_mut(), &cfg)?
        }
    };
    Ok(LabeledResult { spec: spec.clone(), result })
}

/// Run a whole batch of strategy sessions. `factory` must be `Sync` (multiple threads share the same closure reference, each calling it once);
/// `threads` is the desired parallelism upper bound (the actual value is `min(threads, plan length, available_parallelism)`).
///
/// Result order matches `plan` (re-homed by original index), independent of thread scheduling — so serial and parallel results are
/// item-by-item identical. If any session errors during boot/run, the whole batch returns that error (fail-fast, not swallowed).
pub fn run_swarm<R, F>(
    factory: F,
    plan: &[SessionSpec],
    threads: usize,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String> + Sync,
{
    // Default empty config (auto-derive view, greedy degrades to random without a goal) — behavior identical to stages 1~5.
    run_swarm_with_config(factory, plan, &crate::config::PlaytestConfig::default(), threads)
}

/// Swarm with per-game view overrides (design draft section 1 "auto-derive + optional overrides", section 11 item 6).
/// `config` applies to the whole batch: strategies see the include/exclude/relabel/derived-quantity-adjusted view, greedy walks toward
/// config.goal. Other determinism/parallelism/re-homing guarantees are the same as [`run_swarm`].
pub fn run_swarm_with_config<R, F>(
    factory: F,
    plan: &[SessionSpec],
    config: &crate::config::PlaytestConfig,
    threads: usize,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String> + Sync,
{
    // Run by index: each spec boots a runtime inside the calling thread + builds a strategy (with config.goal), runs a session.
    run_indexed(plan.len(), threads, |i| run_one::<R, F>(&factory, &plan[i], config))
}

/// Seed-exploration batch runner (design draft section 3): spreads a set of **perturbation scripts** across parallel threads, each using
/// [`ScriptedStrategy`] to run a session — the script is fed back to reproduce/diverge, and truncated scripts hand off to random for divergence.
/// Reuses the same index-parallel core as `run_swarm` (`run_indexed`), so **serial/parallel results are item-by-item identical**,
/// and results can be fed directly to `aggregate_with_endings` (including unreachable endings).
///
/// `seed_replies`: external replies in the seed recording (LLM content, etc.). Perturbation only touches **inputs**; replies follow the seed and are
/// re-injected at their original ticks (same accounting as `Sim::replay`) — otherwise endings that require a reply to clear (e.g. echo) couldn't be
/// reproduced by the baseline. Truncated scripts only inject replies **before the truncation point** (after truncation it's random divergence, no more seed replies).
///
/// Each result's spec is tagged `StrategyKind::Scripted` with seed=that script's index in the plan
/// (so results can be reconciled to the specific perturbation; the script itself isn't in the spec — too long and variable-length).
/// Truncated scripts (`truncate_at=Some`) seed their divergence random with `explore_seed + index` — deterministic and reproducible.
pub fn run_seed_swarm<R, F>(
    factory: F,
    plan: &[Perturbation],
    seed_replies: &[ReplyRecord],
    max_ticks: u64,
    terminal: TerminalSpec,
    explore_seed: u64,
    threads: usize,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String> + Sync,
{
    run_indexed(plan.len(), threads, |i| {
        let pert = &plan[i];
        let (mut sim, mut logic, engine) = factory()?;
        // Truncated scripts hand off to random for divergence; non-truncated scripts stop after replay (None).
        // The divergence random is seeded with explore_seed + index: each script gets its own independent reproducible divergence sequence.
        let then_explore: Option<Box<dyn Strategy>> = if pert.truncate_at.is_some() {
            Some(Box::new(RandomStrategy::new(explore_seed.wrapping_add(i as u64))))
        } else {
            None
        };
        let mut strategy = ScriptedStrategy::new(pert.script.clone(), then_explore);
        // Seed replies to inject for this session: truncated scripts keep only those before the truncation point (after truncation there are no seed replies);
        // non-truncated scripts inject all. drop/swap/substitute don't change the tick axis, replies are injected at their original ticks.
        let replies: Vec<ReplyRecord> = match pert.truncate_at {
            Some(cut) => seed_replies.iter().filter(|r| r.tick < cut).cloned().collect(),
            None => seed_replies.to_vec(),
        };
        let cfg = SessionConfig {
            max_ticks,
            seed: i as u64,
            terminal: terminal.clone(),
            seed_replies: replies,
            ..Default::default()
        };
        let result = run_session(&mut sim, &mut logic, &engine, &mut strategy, &cfg)?;
        // spec is tagged Scripted + seed=index, to reconcile the result back to the specific perturbation
        let spec = SessionSpec {
            strategy_kind: StrategyKind::Scripted,
            seed: i as u64,
            max_ticks,
            terminal: terminal.clone(),
        };
        Ok(LabeledResult { spec, result })
    })
}

/// LLM-tier batch runner (design draft stage 5): a few LLM agents read the same Scene View to play human-like + emit qualitative notes.
///
/// **Why serial, not parallel**: LLM sessions are inherently slow, separately rate-limited, and don't drag down strategy tiers (design draft section 9); moreover LLM
/// inference is non-deterministic, so the outcome/notes of these sessions **don't require cross-run reproducibility** — parallelism has no "determinism" meaning for the result.
/// Serial is simplest: share one `client` (`Arc<dyn LlmClient>`, Send+Sync), boot a fresh runtime per session
/// (same cold-start constraint as strategy tiers, so the recording is replayable), run a LlmStrategy session, and tag with `StrategyKind::Llm`.
///
/// Each session's seed=`base_seed + index` (seeds LlmStrategy's reserved PCG; currently doesn't affect selection, kept for reconciliation).
/// `goal` becomes the goal description in the prompt. The returned `LabeledResult` can be **merged into the same result set** as strategy-tier results
/// and fed to the aggregator — LLM-session notes go into `qualitative_notes`, and the inputs it picks go into the recording (reproducible via `Sim::replay`).
pub fn run_llm_sessions<R, F>(
    factory: F,
    client: Arc<dyn LlmClient>,
    count: usize,
    goal: &str,
    base_seed: u64,
    max_ticks: u64,
    terminal: TerminalSpec,
) -> Result<Vec<LabeledResult>, String>
where
    R: GameLogic,
    F: Fn() -> Result<(Sim, R, Engine), String>,
{
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (mut sim, mut logic, engine) = factory()?;
        let seed = base_seed.wrapping_add(i as u64);
        // Each session gets a fresh LlmStrategy, sharing the same client (Arc wrapped as Box<dyn LlmClient> for the constructor)
        let mut strategy = LlmStrategy::new(Box::new(SharedClient(client.clone())), goal, seed);
        let cfg = SessionConfig { max_ticks, seed, terminal: terminal.clone(), ..Default::default() };
        let result = run_session(&mut sim, &mut logic, &engine, &mut strategy, &cfg)?;
        let spec = SessionSpec {
            strategy_kind: StrategyKind::Llm,
            seed,
            max_ticks,
            terminal: terminal.clone(),
        };
        out.push(LabeledResult { spec, result });
    }
    Ok(out)
}

/// Wraps `Arc<dyn LlmClient>` as an `LlmClient`, letting multiple sessions share the same underlying client
/// (Box requires exclusive ownership; Arc lets N sessions all get the same real client, saving N reconnections/reconfigurations).
struct SharedClient(Arc<dyn LlmClient>);

impl LlmClient for SharedClient {
    fn complete(&self, prompt: &str) -> Result<String, String> {
        self.0.complete(prompt)
    }
}

/// Index-parallel core: runs `len` tasks, the i-th defined by `task(i)`, results re-homed by index.
/// Shared by `run_swarm` and `run_seed_swarm` — thread splitting / re-homing / fail-fast lives in one place,
/// so the "serial/parallel item-by-item identical" determinism rule is the same guarantee for both paths.
fn run_indexed<T>(
    len: usize,
    threads: usize,
    task: T,
) -> Result<Vec<LabeledResult>, String>
where
    T: Fn(usize) -> Result<LabeledResult, String> + Sync,
{
    if len == 0 {
        return Ok(Vec::new());
    }

    // Actual thread count: no more than desired, no more than task count, no more than machine cores (default to available_parallelism)
    let cpu = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let n_threads = threads.max(1).min(len).min(cpu.max(1));

    // Single thread: just run serially, don't even open a scope (common case for small batches/single core, zero thread overhead)
    if n_threads <= 1 {
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            out.push(task(i)?);
        }
        return Ok(out);
    }

    // Multi-thread: round-robin indices into n_threads buckets, each thread runs its own batch,
    // results are collected back with their original indices, then re-homed by index. The splitting doesn't affect results (determinism doesn't depend on splitting).
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); n_threads];
    for i in 0..len {
        buckets[i % n_threads].push(i);
    }

    // scope lets worker threads borrow task (stack reference), no 'static / Arc needed
    let collected: Vec<Result<Vec<(usize, LabeledResult)>, String>> = thread::scope(|scope| {
        let task_ref = &task;
        let handles: Vec<_> = buckets
            .into_iter()
            .map(|idxs| {
                scope.spawn(move || {
                    let mut local = Vec::with_capacity(idxs.len());
                    for i in idxs {
                        // Any session error carries the error out (fail-fast, not silently dropped)
                        local.push((i, task_ref(i)?));
                    }
                    Ok(local)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("工作线程不应 panic")).collect()
    });

    // Aggregate: first collect all thread errors (return the first if any), then sort back by original index
    let mut indexed: Vec<(usize, LabeledResult)> = Vec::with_capacity(len);
    for chunk in collected {
        indexed.extend(chunk?);
    }
    indexed.sort_by_key(|(i, _)| *i);
    Ok(indexed.into_iter().map(|(_, lr)| lr).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::{Event, RuleSet};
    use vitric_sim::Pcg32;

    fn empty_engine() -> Engine {
        let schema = Schema::parse(&json!({"components": {}}), "s.json").unwrap();
        Engine::new(RuleSet::parse(&json!({"rules": []}), "r.json").unwrap(), schema)
    }

    /// Minimal logic that emits a terminal event at tick N (same as the session tests, but this file has its own copy).
    struct EmitAt {
        at: u64,
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitAt {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            tick: u64,
        ) -> Result<(), String> {
            if tick == self.at {
                self.pending.push(Event::new(&self.event, json!({})));
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    /// Factory: builds a fresh (sim, logic, engine) each call. win_at=Some(n) emits game-won at tick n.
    fn factory_winning(win_at: Option<u64>) -> impl Fn() -> Result<(Sim, EmitAt, Engine), String> {
        move || {
            let sim = Sim::new(1);
            let logic = match win_at {
                Some(at) => EmitAt { at, event: "game-won".to_string(), pending: vec![] },
                // Never emits a terminal event: use an unreachable at
                None => EmitAt { at: u64::MAX, event: "nope".to_string(), pending: vec![] },
            };
            Ok((sim, logic, empty_engine()))
        }
    }

    fn small_plan() -> Vec<SessionSpec> {
        let mut plan = Vec::new();
        for kind in StrategyKind::ALL {
            for seed in 0..4u64 {
                plan.push(SessionSpec::new(kind, seed, 50));
            }
        }
        plan
    }

    #[test]
    fn swarm_empty_plan_yields_empty() {
        let out = run_swarm(factory_winning(Some(3)), &[], 4).unwrap();
        assert!(out.is_empty());
    }

    // ---- default_plan: mix in lookahead when goal declared / unchanged without goal ----

    /// Number of lookahead sessions in a session plan.
    fn count_lookahead(plan: &[SessionSpec]) -> usize {
        plan.iter()
            .filter(|s| matches!(s.strategy_kind, StrategyKind::Lookahead { .. }))
            .count()
    }

    #[test]
    fn default_plan_no_goal_is_pure_rotation_unchanged() {
        // No goal: the plan must match "four-strategy rotation × incrementing seed" item-by-item, with zero lookahead sessions (backward compatible).
        let plan = default_plan(8, 0, 50, TerminalSpec::default(), false);
        assert_eq!(plan.len(), 8);
        assert_eq!(count_lookahead(&plan), 0, "无 goal 不该掺前瞻");
        for (k, spec) in plan.iter().enumerate() {
            assert_eq!(spec.strategy_kind, StrategyKind::ALL[k % StrategyKind::ALL.len()]);
            assert_eq!(spec.seed, k as u64);
        }
    }

    #[test]
    fn default_plan_with_goal_mixes_in_lookahead() {
        // With goal: about 1/4 of sessions swap to lookahead (8 → 2), the trailing ones are swapped, the rest of the rotation slots are untouched.
        let plan = default_plan(8, 0, 50, TerminalSpec::default(), true);
        assert_eq!(plan.len(), 8);
        assert_eq!(count_lookahead(&plan), 2, "8 局应掺 2 局前瞻（约 25%）: {plan:?}");
        // The trailing 2 are lookahead; the first 6 are still the original rotation
        assert!(matches!(plan[6].strategy_kind, StrategyKind::Lookahead { .. }));
        assert!(matches!(plan[7].strategy_kind, StrategyKind::Lookahead { .. }));
        for (k, spec) in plan.iter().enumerate().take(6) {
            assert_eq!(spec.strategy_kind, StrategyKind::ALL[k % StrategyKind::ALL.len()]);
        }
        // Lookahead sessions retain their original seed/max_ticks/terminal; only kind is swapped
        assert_eq!(plan[7].seed, 7);
        assert_eq!(plan[7].max_ticks, 50);
        // The mixed-in lookahead uses the default swarm search depth (deeper than the single-session default)
        assert_eq!(
            plan[7].strategy_kind,
            StrategyKind::Lookahead { depth: DEFAULT_SWARM_LOOKAHEAD_DEPTH }
        );
    }

    #[test]
    fn default_plan_with_goal_keeps_at_least_two_lookahead_for_small_n() {
        // Small N: at least 2 lookahead sessions, but not exceeding the total session count. N=1 → 1 session (min kicks in).
        assert_eq!(count_lookahead(&default_plan(1, 0, 50, TerminalSpec::default(), true)), 1);
        assert_eq!(count_lookahead(&default_plan(3, 0, 50, TerminalSpec::default(), true)), 2);
        // N=20 → 1/4=5 sessions
        assert_eq!(count_lookahead(&default_plan(20, 0, 50, TerminalSpec::default(), true)), 5);
    }

    /// A minimal logic that emits game-won, but only when "no input was injected" — to prove the lookahead session actually went through
    /// `run_session_lookahead` (it speculatively tries each candidate; with an empty action vocabulary "do nothing" is the only candidate, still clearing).
    /// Mainly verifies run_one's dispatch of the Lookahead variant: doesn't panic (build would panic), produces full telemetry.
    #[test]
    fn run_swarm_dispatches_lookahead_spec_to_lookahead_runner() {
        // Plan: one normal random + one Lookahead. Both should run through, both carrying full telemetry (state_trace non-empty, has recording).
        let plan = vec![
            SessionSpec::new(StrategyKind::Random, 0, 20),
            SessionSpec::new(StrategyKind::Lookahead { depth: 4 }, 1, 20),
        ];
        // factory_winning(Some(3)): emits game-won at tick 3, both paths should clear at tick 4
        let out = run_swarm(factory_winning(Some(3)), &plan, 2).unwrap();
        assert_eq!(out.len(), 2);
        // The lookahead session didn't panic (would panic if it mistakenly went through build); outcome/telemetry all populated
        let look = &out[1];
        assert!(matches!(look.spec.strategy_kind, StrategyKind::Lookahead { .. }));
        assert_eq!(look.outcome(), Outcome::Win, "前瞻局也该收到 tick3 的 game-won");
        assert_eq!(look.result.ticks, 4);
        // Telemetry has the same shape: one state_hash per tick + a serializable recording
        assert_eq!(look.result.state_trace.len(), look.result.ticks as usize);
        assert!(!look.result.recording.checkpoints.is_empty());
    }

    /// Mixed plan containing lookahead sessions; serial vs parallel are item-by-item identical (the determinism rule holds for lookahead too).
    #[test]
    fn swarm_mixed_lookahead_serial_and_parallel_identical() {
        let plan = vec![
            SessionSpec::new(StrategyKind::Random, 0, 40),
            SessionSpec::new(StrategyKind::Lookahead { depth: 5 }, 1, 40),
            SessionSpec::new(StrategyKind::Greedy, 2, 40),
            SessionSpec::new(StrategyKind::Lookahead { depth: 8 }, 3, 40),
        ];
        let serial = run_swarm(factory_winning(Some(10)), &plan, 1).unwrap();
        let parallel = run_swarm(factory_winning(Some(10)), &plan, 8).unwrap();
        assert_eq!(serial.len(), parallel.len());
        for (a, b) in serial.iter().zip(parallel.iter()) {
            assert_eq!(a.spec, b.spec, "spec 顺序一致");
            assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
            assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
            assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
            let ja = serde_json::to_string(&a.result.recording).unwrap();
            let jb = serde_json::to_string(&b.result.recording).unwrap();
            assert_eq!(ja, jb, "含前瞻局的混合计划串/并行录像逐字节一致");
        }
    }

    #[test]
    fn swarm_results_follow_plan_order() {
        let plan = small_plan();
        let out = run_swarm(factory_winning(Some(3)), &plan, 4).unwrap();
        assert_eq!(out.len(), plan.len());
        // Each result's spec must equal the spec at the corresponding position in plan (order wasn't shuffled by threads)
        for (lr, spec) in out.iter().zip(plan.iter()) {
            assert_eq!(&lr.spec, spec);
        }
    }

    #[test]
    fn swarm_serial_and_parallel_are_identical() {
        let plan = small_plan();
        // 1 thread (serial) vs 8 threads (parallel): outcome/ticks/state_trace must be item-by-item identical
        let serial = run_swarm(factory_winning(Some(5)), &plan, 1).unwrap();
        let parallel = run_swarm(factory_winning(Some(5)), &plan, 8).unwrap();
        assert_eq!(serial.len(), parallel.len());
        for (a, b) in serial.iter().zip(parallel.iter()) {
            assert_eq!(a.spec, b.spec, "spec 顺序一致");
            assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
            assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
            assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
            assert_eq!(a.result.fired_events, b.result.fired_events, "事件集一致");
        }
    }

    #[test]
    fn swarm_propagates_factory_error() {
        let factory = || -> Result<(Sim, EmitAt, Engine), String> { Err("boot 炸了".to_string()) };
        let plan = vec![SessionSpec::new(StrategyKind::Random, 0, 10)];
        let err = run_swarm(factory, &plan, 4).unwrap_err();
        assert!(err.contains("boot 炸了"), "{err}");
    }

    #[test]
    fn swarm_collects_win_outcomes() {
        // Emit game-won at tick 3 → every session should clear at tick 4
        let plan = small_plan();
        let out = run_swarm(factory_winning(Some(3)), &plan, 4).unwrap();
        assert!(out.iter().all(|lr| lr.outcome() == Outcome::Win), "全部应通关");
        assert!(out.iter().all(|lr| lr.result.ticks == 4));
    }

    use crate::scene_view::Action;
    use crate::seed::{PerturbOp, Perturbation};

    fn pert(op: PerturbOp, script: Vec<(u64, &str)>, trunc: Option<u64>) -> Perturbation {
        Perturbation {
            op,
            script: script
                .into_iter()
                .map(|(t, a)| (t, Action { action: a.to_string(), phase: "pressed".to_string() }))
                .collect(),
            truncate_at: trunc,
        }
    }

    fn seed_plan() -> Vec<Perturbation> {
        vec![
            pert(PerturbOp::Baseline, vec![(1, "go")], None),
            pert(PerturbOp::Drop, vec![], None),
            pert(PerturbOp::Truncate, vec![(1, "go")], Some(2)), // truncated, then random divergence
        ]
    }

    #[test]
    fn seed_swarm_results_follow_plan_order_and_label_scripted() {
        let plan = seed_plan();
        let out = run_seed_swarm(
            factory_winning(Some(3)),
            &plan,
            &[],
            50,
            TerminalSpec::default(),
            7,
            4,
        )
        .unwrap();
        assert_eq!(out.len(), plan.len());
        for (i, lr) in out.iter().enumerate() {
            assert_eq!(lr.spec.strategy_kind, StrategyKind::Scripted, "脚本局标 Scripted");
            assert_eq!(lr.spec.seed, i as u64, "seed=下标，便于对账");
        }
    }

    #[test]
    fn seed_swarm_serial_and_parallel_identical() {
        let plan = seed_plan();
        let serial =
            run_seed_swarm(factory_winning(Some(3)), &plan, &[], 80, TerminalSpec::default(), 11, 1)
                .unwrap();
        let parallel =
            run_seed_swarm(factory_winning(Some(3)), &plan, &[], 80, TerminalSpec::default(), 11, 8)
                .unwrap();
        assert_eq!(serial.len(), parallel.len());
        for (a, b) in serial.iter().zip(parallel.iter()) {
            assert_eq!(a.result.outcome, b.result.outcome, "结局一致");
            assert_eq!(a.result.ticks, b.result.ticks, "tick 数一致");
            assert_eq!(a.result.state_trace, b.result.state_trace, "状态轨迹逐项一致");
            let ja = serde_json::to_string(&a.result.recording).unwrap();
            let jb = serde_json::to_string(&b.result.recording).unwrap();
            assert_eq!(ja, jb, "种子探索串/并行录像逐字节一致");
        }
    }

    #[test]
    fn seed_swarm_empty_plan_yields_empty() {
        let out =
            run_seed_swarm(factory_winning(Some(3)), &[], &[], 50, TerminalSpec::default(), 0, 4)
                .unwrap();
        assert!(out.is_empty());
    }

    /// A logic that emits game-won only when it receives an "oracle-says" reply — clears only via the reply, not by pressing inputs.
    /// Used to verify that seed exploration really re-injects seed replies at their ticks (the baseline should reproduce the Win).
    struct WinOnReply {
        pending: Vec<Event>,
    }
    impl GameLogic for WinOnReply {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            events: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            for e in events {
                if e.name == "oracle-says" && e.data.get("answer") == Some(&json!("open")) {
                    self.pending.push(Event::new("game-won", json!({})));
                }
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    fn factory_reply_gated() -> impl Fn() -> Result<(Sim, WinOnReply, Engine), String> {
        || Ok((Sim::new(1), WinOnReply { pending: vec![] }, empty_engine()))
    }

    #[test]
    fn seed_swarm_injects_seed_replies_baseline_reaches_win() {
        // Seed: an oracle-says{answer:"open"} at tick 2 (required to clear). The script carries only inputs (here an empty script).
        let plan = vec![pert(PerturbOp::Baseline, vec![], None)];
        let replies = vec![ReplyRecord {
            tick: 2,
            name: "oracle-says".to_string(),
            data: json!({"answer": "open"}),
        }];
        // With seed replies injected → the baseline reproduces the Win
        let out = run_seed_swarm(
            factory_reply_gated(),
            &plan,
            &replies,
            50,
            TerminalSpec::default(),
            0,
            1,
        )
        .unwrap();
        assert_eq!(out[0].outcome(), Outcome::Win, "种子回复注回去了，基线该通关");

        // Counter-evidence: without seed replies → no oracle-says → never clears → Timeout
        let out_no = run_seed_swarm(
            factory_reply_gated(),
            &plan,
            &[],
            50,
            TerminalSpec::default(),
            0,
            1,
        )
        .unwrap();
        assert_eq!(out_no[0].outcome(), Outcome::Timeout, "没回复就通不了");
    }

    #[test]
    fn seed_swarm_truncate_drops_replies_after_cut() {
        // Seed reply at tick 5; truncation point at tick 3 → after truncation there are no seed replies → that reply can't be injected → can't clear.
        let plan = vec![pert(PerturbOp::Truncate, vec![], Some(3))];
        let replies = vec![ReplyRecord {
            tick: 5,
            name: "oracle-says".to_string(),
            data: json!({"answer": "open"}),
        }];
        let out = run_seed_swarm(
            factory_reply_gated(),
            &plan,
            &replies,
            50,
            TerminalSpec::default(),
            0,
            1,
        )
        .unwrap();
        assert_eq!(out[0].outcome(), Outcome::Timeout, "截断点之后的种子回复不该被注入");
    }
}
