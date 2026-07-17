//! vitric-playtest — the in-process foundation for agent swarm playtests (design draft stage 1).
//!
//! Three pieces compose a replayable automated playtest session:
//! - [`scene_view`]: derives an "agent view" (observation/action/terminate) from the world/rules, a pure projection;
//! - [`strategy`]: pure-logic strategies that consume the view and produce actions (random/greedy/coverage/scripted), PCG-seeded = deterministic;
//! - [`session`]: loops "derive view → choose action → inject → step" until clear/death/timeout, producing a recording.
//!
//! Stage 3 adds **seed-based exploration** (design draft section 3):
//! - [`seed`]: takes a gate-certificate recording as a seed and perturbs its input sequence to generate a set of mutated scripts;
//! - [`strategy::ScriptedStrategy`]: injects at recorded ticks per the script (can be spliced with random for truncation divergence);
//! - [`report`] adds `ending_coverage`: which declared endings were reached, and which are reachable in 0 sessions (unreachable endings).
//!
//! Stage 5 adds the **LLM tier** (design draft section 2 / section 11 item 5):
//! - [`llm_agent::LlmStrategy`]: a few LLM agents read the same Scene View to play human-likely + emit qualitative notes
//!   (clarity/continuity/choice validity). LLM inference is non-deterministic, but the inputs it picks are still recorded → replayable;
//! - [`strategy::PlaytestNote`] + `Strategy::drain_notes`: the note channel (empty by default, only produced by LLM);
//! - [`session::SessionResult`] collects notes (not hashed/recorded), and [`report`] aggregates them into `qualitative_notes`
//!   (grouped and deduped by kind, honestly labeled "LLM subjective hint, awaits human review").
//!
//! The assembly runtime (`Runtime::boot`) lives in vitric-cli; the dependency direction is cli → playtest, so this crate
//! does not boot the project — the caller boots it and hands `(Sim, GameLogic, Engine)` to [`session::run_session`].

pub mod config;
pub mod html;
pub mod llm_agent;
pub mod report;
pub mod scene_view;
pub mod seed;
pub mod session;
pub mod strategy;
pub mod swarm;

pub use config::{
    DerivedSpec, DistanceMetric, GoalDirection, GoalSpec, ObservationConfig, PlaytestConfig,
    Relabel, TerminalOverride,
};
pub use html::report_to_html;
pub use llm_agent::{LlmClient, LlmStrategy};
pub use report::{
    aggregate, aggregate_with_endings, aggregate_with_endings_and_declared, CollapseField,
    DominantAction, EndingCoverage,
    NonFiniteField, NoteCluster, NumericBreakage, QualitativeNotes, RecordingRef, Report,
    RunawayField,
};
pub use scene_view::{Action, Outcome, SceneView, TerminalSpec};
pub use seed::{perturb_plan, PerturbOp, Perturbation};
pub use session::{
    run_session, run_session_lookahead, LookaheadConfig, NumericStat, SessionConfig, SessionResult,
};
pub use strategy::{
    CoverageStrategy, EconomyStrategy, GreedyStrategy, PlaytestNote, RandomStrategy,
    ScriptedStrategy, Strategy,
};
pub use swarm::{
    default_plan, run_llm_sessions, run_seed_swarm, run_swarm, run_swarm_with_config,
    LabeledResult, SessionSpec, StrategyKind, DEFAULT_SWARM_LOOKAHEAD_BEAM,
    DEFAULT_SWARM_LOOKAHEAD_DEPTH,
};
