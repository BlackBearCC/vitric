//! Aggregator + floor report (design draft section 5 "Aggregation and Report", section 9 perf budget).
//!
//! Consumes a batch of [`LabeledResult`] (sessions run with multiple strategies × multiple seeds),
//! aggregating them in **one O(sessions × ticks) pass** into a serializable [`Report`]: machine-readable
//! JSON + human-readable `summary`. Stage 2 only does a few **solid, accurately-measured** dimensions —
//! clear rate / reachability / stuck candidates / pacing / inert actions / dominant strategy — and does
//! not force in dimensions that can't be measured accurately (numeric breakage, unreachable content
//! require derived quantities / semantics, left to later stages).
//!
//! Honest labeling: `stuck_clusters` (soft-locks) and `inert_actions` (dead actions) are both
//! **heuristic candidates**, not conclusions — some games legitimately stay still, some actions
//! legitimately produce no events. The report labels them as "candidates" and hands them to humans
//! for review (each one carries a replayable session recording, you can replay it directly to see).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;
use vitric_rules::{Engine, Trigger};
use vitric_sim::Recording;

use crate::scene_view::{Outcome, TerminalSpec};
use crate::swarm::{LabeledResult, StrategyKind};

/// A lightweight reference to a "representative recording" (design draft section 5 "each conclusion
/// carries a replayable recording", section 11 item 6 report polish).
///
/// **The report body no longer inlines whole recordings**: previously each dimension's
/// (stuck/runaway/note…) `sample_recording` embedded an entire recording JSON into the report; with
/// dozens of dimensions stacked together the report became "one giant blob of recordings", drowning
/// the human-readable body. Now each dimension only carries this lightweight reference — only `path`
/// (the relative path after being written to disk) + `ticks` + `outcome` are serialized into the
/// report JSON, keeping the body clean and readable.
///
/// The actual recording bytes live in `recording` for the caller to write to a file / replay offline,
/// but `#[serde(skip)]` — **not in the report JSON**. At aggregation time `path=None` (not yet on
/// disk); `cmd_playtest`/the caller writes it into `--report-dir` and back-fills the relative path.
/// `key` is the stable filename for this representative recording (spliced from dimension + field +
/// strategy + seed, deterministically reproducible).
#[derive(Debug, Clone, Serialize)]
pub struct RecordingRef {
    /// Relative path after being written to disk (relative to report-dir). None at aggregation time;
    /// the caller back-fills it after writing the file.
    pub path: Option<String>,
    /// How many ticks this recording ran (you can see a rough picture without reading the file).
    pub ticks: u64,
    /// The outcome of this session.
    pub outcome: Outcome,
    /// Stable filename key (deterministically reproducible) — the caller writes it to disk as
    /// `<key>.json`. Not in the report JSON.
    #[serde(skip)]
    pub key: String,
    /// The actual recording bytes (for offline replay / writing to file). **Not in the report JSON**
    /// (the body only carries the path).
    #[serde(skip)]
    pub recording: Recording,
}

impl RecordingRef {
    /// Build a reference from a representative session result: key is spliced from the dimension
    /// label + the representative strategy/seed (stable, deterministic).
    fn from_sample(dimension: &str, rep: &LabeledResult) -> RecordingRef {
        let key = format!(
            "{dimension}-{}-{}",
            rep.spec.strategy_kind.name(),
            rep.spec.seed
        );
        RecordingRef {
            path: None,
            ticks: rep.result.recording.ticks,
            outcome: rep.result.outcome,
            key,
            recording: rep.result.recording.clone(),
        }
    }
}

/// Threshold for considering a session a freeze candidate based on "how many consecutive trailing
/// ticks have a completely unchanged state hash" (design draft: K defaults to 60).
pub const DEFAULT_FREEZE_K: usize = 60;

/// Runaway detection thresholds (design draft stage 4 "runaway: max > 1000× the initial value, or >1e6").
/// Annotated here, configurable later:
/// - end value / peak value ≫ first value (`> first × RUNAWAY_RATIO`) means this field grows unboundedly
///   — looking at the absolute value alone would falsely flag fields that were already large; the
///   relative ratio is more stable;
/// - **or** peak value `> RUNAWAY_ABS` absolute ceiling (when the first value is 0/negative the ratio
///   is invalid, so the absolute threshold acts as a fallback);
/// - and `monotonic_up` (only ever grows, never decreases) — true runaway is a one-way explosion;
///   up-and-down oscillation doesn't count.
///
/// Hitting either makes the field a candidate (honestly labeled candidate; legitimate strong growth
/// curves may also hit, leave for human review).
const RUNAWAY_RATIO: f64 = 1000.0;
const RUNAWAY_ABS: f64 = 1e6;

/// The relative runaway threshold additionally requires the peak to reach this absolute lower bound —
/// otherwise growth like "0.008 → 8.116" with an inflated ratio but actually tiny magnitude (physical
/// micro-movement / tiny accumulation) would be falsely flagged. Real runaway always grows to a
/// meaningfully large number.
const RUNAWAY_RATIO_MIN_ABS: f64 = 1000.0;

/// Non-economy components: position/velocity/camera/lighting aren't economy resources — a coordinate
/// moving monotonically isn't "unbounded growth", a light angle returning to zero isn't "collapse".
/// numeric_breakage excludes all fields of these components entirely (dogfood empirical: gravity
/// falling caused Position.y to be misreported as runaway; menu light Light.angle=0 + stuck was
/// misreported as collapse).
const NON_ECONOMY_COMPONENTS: &[&str] =
    &["Position", "Velocity", "Camera", "Shake", "Light", "Ambient"];

/// Field keys look like `<entity>/<component>.<field>`; extract the component name. When the input
/// isn't in this format, degrades to taking the first segment of the whole string.
fn field_component(field: &str) -> Option<&str> {
    let comp_field = field.rsplit_once('/').map_or(field, |(_, cf)| cf);
    comp_field.split('.').next()
}

/// Whether this field belongs to a non-economy component (economy-breakage detection skips it
/// entirely).
fn is_non_economy_field(field: &str) -> bool {
    field_component(field).is_some_and(|c| NON_ECONOMY_COMPONENTS.contains(&c))
}

/// Dominant-action criterion: the share threshold of an action among injections in winning sessions
/// (≥ this ratio → candidate). 0.8 = wins rely on this one action for over 80% of presses, other
/// actions barely used — choice meaningfulness is doubtful.
const DOMINANT_ACTION_SHARE: f64 = 0.8;
/// Dominant action requires at least this many winning sessions as a baseline before concluding
/// (too small a sample isn't "domination").
const DOMINANT_ACTION_MIN_WINS: usize = 3;

/// Built-in event names: these don't count as "an input action triggered a rule response" — they're
/// engine mechanics, independent of whether an input action is caught by a rule:
/// - `start`: a lifecycle event sim emits unconditionally at tick 0 (every session has it, unrelated
///   to actions);
/// - `input`: the action's own event, not a "response";
/// - `collision`: emitted by the built-in collision system, not a product of an input rule;
/// - the rest are common verbs of engine subsystems like sequence/animation/UI/scene.
///
/// They are excluded when detecting inert actions — only events **custom-emitted by rules** count
/// as "this action triggered a response".
const BUILTIN_EVENTS: &[&str] = &[
    "start",
    "input",
    "collision",
    "scene-loaded",
    "sequence-finished",
    "anim-finished",
    "ui-activate",
];

fn is_builtin_event(name: &str) -> bool {
    BUILTIN_EVENTS.contains(&name)
}

/// Replace non-filename characters in field paths / labels (`/`, `.`, etc.) with `_`, used to build
/// stable filenames.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}

/// Outcome distribution + clear rate.
#[derive(Debug, Clone, Serialize)]
pub struct OutcomeDistribution {
    pub win: usize,
    pub lose: usize,
    pub timeout: usize,
    pub total: usize,
    /// Clear rate = win / total (0 when total=0).
    pub win_rate: f64,
}

/// Reachability: terminal / milestone events fired across the union of sessions + the "swarm can't
/// beat it" signal.
#[derive(Debug, Clone, Serialize)]
pub struct Reachability {
    /// Names of terminal / milestone events fired in the union of all sessions (sorted, deterministic).
    pub reached_events: Vec<String>,
    /// 0 sessions Win → true (one of the strongest signals: declared winnable but no swarm session won).
    pub unbeatable_by_swarm: bool,
}

/// Ending coverage (the core of design draft section 3 seed-exploration acceptance: unreachable endings).
///
/// **Where the "declared ending set" comes from** (annotated, see task 3): scan all `emit` action
/// event names in the rules; any that match `TerminalSpec` (win/lose named sets or `ending-*` prefix)
/// are the endings this game **declares it can produce**. This step is a static rule scan — so even
/// if an ending is **never emitted** in any session, we still know it "was declared", and can judge
/// it **unreachable** (declared but reached by 0 sessions). Looking at `fired_events` alone (events
/// actually fired at runtime) can't do this: never-fired events aren't in there at all, conflating
/// "not declared" with "declared but never reached".
#[derive(Debug, Clone, Serialize)]
pub struct EndingCoverage {
    /// All ending event names this game declares it can produce (rules' emit ∩ TerminalSpec, sorted
    /// and deduped).
    pub declared_endings: Vec<String>,
    /// Endings reached by at least one session (declared ∩ any session's fired_events, sorted).
    pub reached_endings: Vec<String>,
    /// **Declared but reached by 0 sessions** — the prime target of seed exploration (design draft
    /// section 3 "unreachable endings").
    pub unreachable_endings: Vec<String>,
}

/// A cluster of soft-lock candidates: a batch of sessions stuck on the same "frozen state hash"
/// without reaching termination.
#[derive(Debug, Clone, Serialize)]
pub struct StuckCluster {
    /// State hash at freeze time (hex literal) — sessions in the same dead state cluster into one
    /// bucket.
    pub frozen_hash: String,
    /// How many sessions hit this dead state.
    pub hits: usize,
    /// The strategy/seed for this dead state (take one session as representative, can replay from it).
    pub sample_strategy: String,
    pub sample_seed: u64,
    /// Representative recording reference (after being written to disk only the path is attached,
    /// no whole recording inlined) — "each conclusion carries a replayable recording".
    pub representative: RecordingRef,
}

/// Pacing: tick distribution up to termination (Timeout sessions listed separately, not mixed into
/// "how long until termination").
#[derive(Debug, Clone, Serialize)]
pub struct Pacing {
    /// Ticks of sessions that terminated (Win/Lose): min / median / max. None when no terminating
    /// sessions.
    pub terminated_min: Option<u64>,
    pub terminated_median: Option<u64>,
    pub terminated_max: Option<u64>,
    /// Histogram buckets of termination ticks (fixed 5 buckets, equal-width cuts over [min, max];
    /// labels are bucket upper bounds).
    pub histogram: Vec<HistogramBucket>,
    /// Number of Timeout sessions (didn't terminate, listed separately and not in the distribution
    /// above).
    pub timeout_count: usize,
}

/// One histogram bucket.
#[derive(Debug, Clone, Serialize)]
pub struct HistogramBucket {
    /// Bucket upper bound (tick).
    pub upper: u64,
    pub count: usize,
}

/// One strategy's performance (grouped aggregation).
#[derive(Debug, Clone, Serialize)]
pub struct StrategyStats {
    pub strategy: String,
    pub sessions: usize,
    pub win_rate: f64,
    /// Median tick of this strategy's winning sessions (None when no winning sessions).
    pub median_win_ticks: Option<u64>,
}

/// Dominant strategy: per-strategy performance + a "some strategy dominates" flag + a "one-trick"
/// action flag.
#[derive(Debug, Clone, Serialize)]
pub struct DominantStrategy {
    pub per_strategy: Vec<StrategyStats>,
    /// If some strategy's win rate is ≥2× the runner-up and the sample is sufficient (≥4 sessions
    /// per strategy), flag its name; otherwise None.
    pub dominant: Option<String>,
    /// **One-trick** candidate (design draft section 5 "one-trick/dominant strategy", section 11
    /// stage 4 "dominant strategy deepening"): in **winning sessions** a single action appears
    /// frequently while other actions barely appear → this one action dominates other playstyles,
    /// other choices are meaningless. Honestly labeled "candidate" (high frequency ≠ the only way
    /// to win, but worth human review of the choice design). None = no such phenomenon.
    pub dominant_action: Option<DominantAction>,
}

/// One-trick action candidate: a single action dominates injections in winning sessions.
#[derive(Debug, Clone, Serialize)]
pub struct DominantAction {
    /// The action that tops the chart.
    pub action: String,
    /// Its share of the total injection count across all winning sessions (0..1).
    pub share: f64,
    /// How many winning sessions the statistics are based on.
    pub winning_sessions: usize,
}

/// Numeric breakage dimension (design draft section 5 "numeric breakage", section 11 stage 4
/// acceptance). All three candidate kinds are reported **clustered by field name**, honestly labeled
/// "candidate" (legitimate strong growth curves may also look like runaway, leave for human review;
/// each carries a replayable recording).
#[derive(Debug, Clone, Serialize)]
pub struct NumericBreakage {
    /// Runaway candidates: a field with very large max in multiple sessions, end value ≫ first value
    /// and only ever grows (unbounded economy growth).
    pub runaway: Vec<RunawayField>,
    /// Collapse soft-lock candidates: a field hit 0 and **that session fell into a stuck cluster**
    /// (the world froze after the resource was drained).
    pub collapse: Vec<CollapseField>,
    /// Overflow candidates: a field had inf/nan at some point (a hard signal of numeric overflow /
    /// divide-by-zero).
    pub non_finite: Vec<NonFiniteField>,
}

/// One runaway field (several sessions clustered by field name).
#[derive(Debug, Clone, Serialize)]
pub struct RunawayField {
    /// Numeric field path (e.g. `treasury/Resources.gold`).
    pub field: String,
    /// Number of sessions hitting the runaway criterion.
    pub hits: usize,
    /// The largest max observed across these sessions (how big it ran away to).
    pub peak_max: f64,
    /// Representative session (take one session's strategy/seed/recording, can replay to watch the
    /// runaway process).
    pub sample_strategy: String,
    pub sample_seed: u64,
    pub representative: RecordingRef,
}

/// One collapse field (several sessions clustered by field name).
#[derive(Debug, Clone, Serialize)]
pub struct CollapseField {
    pub field: String,
    /// Number of sessions hitting "returned to zero + that session got stuck".
    pub hits: usize,
    pub sample_strategy: String,
    pub sample_seed: u64,
    pub representative: RecordingRef,
}

/// One field that has taken a non-finite value.
#[derive(Debug, Clone, Serialize)]
pub struct NonFiniteField {
    pub field: String,
    pub hits: usize,
    pub sample_strategy: String,
    pub sample_seed: u64,
    pub representative: RecordingRef,
}

/// LLM qualitative note aggregation (design draft section 5 "LLM qualitative note", section 11
/// stage 5).
///
/// **Honest positioning**: this whole block is **LLM subjective hint, not a real-person verdict,
/// pending human review** — the LLM thinks something "unclear / contradictory / meaningless choice",
/// it may be right or it may be the LLM itself not understanding. The report places this separately
/// from the "mechanically-caught structural breakage" (the deterministic conclusions of soft-lock /
/// unreachable / numeric breakage), not conflating them.
///
/// LLM session notes don't require cross-run reproducibility anyway (LLM is non-deterministic), so
/// this block does **not** enter the determinism guarantee; but each note carries its session's
/// replayable recording (`representative`), so a human can scrub to that moment and see what scene
/// the LLM was talking about.
#[derive(Debug, Clone, Serialize)]
pub struct QualitativeNotes {
    /// Total number of notes received (including duplicates).
    pub total: usize,
    /// Note clusters grouped by kind and deduped by text within each group (sorted deterministically).
    pub clusters: Vec<NoteCluster>,
}

/// One cluster of qualitative notes: same kind + same text normalized into one entry, recording the
/// hit count + a representative session recording.
#[derive(Debug, Clone, Serialize)]
pub struct NoteCluster {
    /// Note kind (clarity/continuity/choice/other).
    pub kind: String,
    /// Note body (the representative text after normalization).
    pub text: String,
    /// How many times this note appeared (summed across sessions + multiple ticks in the same session).
    pub count: usize,
    /// The decision tick where it was first seen (representative tick, easy for replay to locate that
    /// moment).
    pub sample_tick: u64,
    /// The representative session's strategy/seed (identifies which session the LLM said it in).
    pub sample_strategy: String,
    pub sample_seed: u64,
    /// Representative recording reference — scrub to that scene to see what the LLM was saying
    /// (conclusions carry evidence; after being written to disk only the path is attached).
    pub representative: RecordingRef,
}

/// Floor report (machine JSON + human-readable summary). The few solid dimensions of stage 2.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub sessions: usize,
    pub outcome_distribution: OutcomeDistribution,
    pub reachability: Reachability,
    /// Ending coverage: which endings are declared, which were reached, which were reached by 0
    /// sessions (unreachable endings). None when no engine was passed in (the declared set is
    /// unknown) — an empty declared set is different from "not computed".
    pub ending_coverage: Option<EndingCoverage>,
    /// Soft-lock candidates (honestly labeled: candidates, not conclusions).
    pub stuck_clusters: Vec<StuckCluster>,
    pub pacing: Pacing,
    /// Suspected inert actions (lightweight heuristic candidates).
    pub inert_actions: Vec<String>,
    pub dominant_strategy: DominantStrategy,
    /// Numeric breakage: economy runaway / collapse soft-lock / overflow (design draft stage 4).
    /// Honestly labeled candidate.
    pub numeric_breakage: NumericBreakage,
    /// LLM qualitative note aggregation (design draft stage 5): clarity / continuity / choice
    /// effectiveness, grouped by kind and deduped. Honestly labeled "LLM subjective hint, not a
    /// real-person verdict, pending human review" — placed separately from the mechanical structural
    /// conclusions above.
    pub qualitative_notes: QualitativeNotes,
    /// Human-readable summary: one or two sentences stating the most critical findings above.
    pub summary: String,
}

impl Report {
    /// Walk all dimension representative recording references in the report (mutable borrows) — the
    /// caller uses this to write recordings into report-dir and back-fill relative paths. Order is
    /// deterministic (by dimension declaration order), each appears only once. The report doesn't
    /// hold duplicate references to the same RecordingRef anywhere, so the traversal is the full set.
    pub fn representatives_mut(&mut self) -> Vec<&mut RecordingRef> {
        let mut refs: Vec<&mut RecordingRef> = Vec::new();
        for c in &mut self.stuck_clusters {
            refs.push(&mut c.representative);
        }
        for r in &mut self.numeric_breakage.runaway {
            refs.push(&mut r.representative);
        }
        for c in &mut self.numeric_breakage.collapse {
            refs.push(&mut c.representative);
        }
        for n in &mut self.numeric_breakage.non_finite {
            refs.push(&mut n.representative);
        }
        for c in &mut self.qualitative_notes.clusters {
            refs.push(&mut c.representative);
        }
        refs
    }

    /// Write all representative recordings into separate json files under `report_dir`, and back-fill
    /// each reference's `path` with the relative path (relative to report_dir). The report body JSON
    /// thus only carries paths, no whole recordings inlined. Filename = `<key>.json` (key is
    /// deterministically reproducible). Creates report_dir if missing. Returns the number of files
    /// written.
    pub fn externalize_recordings(&mut self, report_dir: &std::path::Path) -> Result<usize, String> {
        let refs = self.representatives_mut();
        if refs.is_empty() {
            return Ok(0);
        }
        std::fs::create_dir_all(report_dir)
            .map_err(|e| format!("创建 report-dir {} 失败: {e}", report_dir.display()))?;
        let mut written = 0usize;
        for r in refs {
            let filename = format!("{}.json", r.key);
            let full = report_dir.join(&filename);
            let json = serde_json::to_string(&r.recording).expect("录像可序列化");
            std::fs::write(&full, json)
                .map_err(|e| format!("写代表录像 {} 失败: {e}", full.display()))?;
            r.path = Some(filename);
            written += 1;
        }
        Ok(written)
    }
}

/// Aggregation entry: a batch of labeled results → a report. Uses the default freeze threshold K and
/// doesn't compute ending coverage (no engine passed in = the declared ending set is unknown,
/// `ending_coverage` is None).
/// No engine also means `inert_actions` degrades to the old runtime heuristic (no rules to do a
/// static criterion from).
pub fn aggregate(results: &[LabeledResult]) -> Report {
    aggregate_inner(results, DEFAULT_FREEZE_K, None, None)
}

/// Aggregate (with adjustable freeze threshold K, for tests). One-pass scan, no quadratic
/// comparisons.
pub fn aggregate_with_freeze_k(results: &[LabeledResult], freeze_k: usize) -> Report {
    aggregate_inner(results, freeze_k, None, None)
}

/// Aggregate + ending coverage (seed-exploration-specific). `engine`/`terminal` are used to scan the
/// rules' declared ending set; the report includes `ending_coverage` (which declared endings are
/// unreachable). This is the entry point for design draft section 3 acceptance.
///
/// With the engine passed in, `inert_actions` also switches to the **static criterion** (scan the
/// rules' `do` to see if there's a real effect), instead of the old runtime "did any event get
/// emitted" heuristic — the latter would falsely flag movement keys (left/right/space/up) that "only
/// set state but don't emit" as inert. The default CLI path goes through this entry, so the default
/// is the static criterion.
pub fn aggregate_with_endings(
    results: &[LabeledResult],
    engine: &Engine,
    terminal: &TerminalSpec,
) -> Report {
    let declared = declared_endings(engine, terminal, &[] as &[String]);
    aggregate_inner(results, DEFAULT_FREEZE_K, Some(declared), Some(engine))
}

/// Same as [`aggregate_with_endings`], but additionally merges manifest-declared ending names
/// (`gates.playthroughs[].must_emit`) into the declared ending set. The static rule scan only looks
/// at `do`'s `emit`, so it can't catch events emitted by scripts/LLMs (echo's `run-complete` is sent
/// by the JS system, the rules don't have this emit at all), and ending_coverage would miss them.
/// Merging in the manifest's authoritative win-event makes the endings of script/LLM games
/// recognizable (unioned with the rule scan result, deduped).
pub fn aggregate_with_endings_and_declared(
    results: &[LabeledResult],
    engine: &Engine,
    terminal: &TerminalSpec,
    manifest_declared: &[String],
) -> Report {
    let declared = declared_endings(engine, terminal, manifest_declared);
    aggregate_inner(results, DEFAULT_FREEZE_K, Some(declared), Some(engine))
}

/// Aggregation kernel: when declared=Some, compute ending coverage; when None, skip.
/// When `engine=Some`, `inert_actions` uses the static criterion; when None, degrades to the runtime
/// heuristic.
fn aggregate_inner(
    results: &[LabeledResult],
    freeze_k: usize,
    declared: Option<Vec<String>>,
    engine: Option<&Engine>,
) -> Report {
    let outcome_distribution = aggregate_outcomes(results);
    let reachability = aggregate_reachability(results, &outcome_distribution);
    let ending_coverage = declared.map(|d| aggregate_ending_coverage(d, results));
    let stuck_clusters = aggregate_stuck(results, freeze_k);
    let pacing = aggregate_pacing(results);
    let inert_actions = aggregate_inert(results, engine);
    let dominant_strategy = aggregate_dominant(results);
    // Numeric breakage needs to know "which sessions got stuck" to judge collapse — use the same
    // freeze criterion to compute the set of stuck session indices.
    let stuck_idx = stuck_session_indices(results, freeze_k);
    let numeric_breakage = aggregate_numeric_breakage(results, &stuck_idx);
    let qualitative_notes = aggregate_notes(results);
    let summary = build_summary(
        &outcome_distribution,
        &reachability,
        &ending_coverage,
        &stuck_clusters,
        &inert_actions,
        &dominant_strategy,
        &numeric_breakage,
        &qualitative_notes,
    );
    Report {
        sessions: results.len(),
        outcome_distribution,
        reachability,
        ending_coverage,
        stuck_clusters,
        pacing,
        inert_actions,
        dominant_strategy,
        numeric_breakage,
        qualitative_notes,
        summary,
    }
}

/// Scan all `emit` action event names in the rules; any matching TerminalSpec (win/lose names or
/// ending prefix) is a "declared ending". Sorted and deduped — deterministic and easy to reconcile.
/// **Static rule scan**, doesn't look at whether it was actually fired at runtime, so an ending that
/// has never been emitted can still be identified as "declared" (this is the premise for judging
/// unreachability, see the EndingCoverage comment).
fn declared_endings(
    engine: &Engine,
    terminal: &TerminalSpec,
    manifest_declared: &[String],
) -> Vec<String> {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for rule in &engine.rules.rules {
        for action in &rule.actions {
            // Action is a JSON object; an emit action looks like {"emit": "<event name>", "data": {...}}
            if let Some(name) = action.get("emit").and_then(|v| v.as_str()) {
                if terminal.classify(name).is_some() {
                    declared.insert(name.to_string());
                }
            }
        }
    }
    // Merge in manifest-declared ending names (gates.playthroughs[].must_emit) — events fired by
    // scripts/LLM aren't in the rules' emit, can only be supplemented via manifest declaration
    // (unioned with the rule scan, BTreeSet auto-dedupes and sorts).
    for name in manifest_declared {
        declared.insert(name.clone());
    }
    declared.into_iter().collect()
}

/// Ending coverage: declared ∩ any session's fired_events = reached; declared − reached = unreachable.
fn aggregate_ending_coverage(declared: Vec<String>, results: &[LabeledResult]) -> EndingCoverage {
    // Union of all events fired across sessions (including terminal and milestone)
    let mut fired: BTreeSet<&str> = BTreeSet::new();
    for lr in results {
        for ev in &lr.result.fired_events {
            fired.insert(ev.as_str());
        }
    }
    let mut reached: Vec<String> = Vec::new();
    let mut unreachable: Vec<String> = Vec::new();
    for end in &declared {
        if fired.contains(end.as_str()) {
            reached.push(end.clone());
        } else {
            unreachable.push(end.clone());
        }
    }
    EndingCoverage { declared_endings: declared, reached_endings: reached, unreachable_endings: unreachable }
}

fn aggregate_outcomes(results: &[LabeledResult]) -> OutcomeDistribution {
    let mut win = 0;
    let mut lose = 0;
    let mut timeout = 0;
    for lr in results {
        match lr.result.outcome {
            Outcome::Win => win += 1,
            Outcome::Lose => lose += 1,
            Outcome::Timeout => timeout += 1,
        }
    }
    let total = results.len();
    let win_rate = if total == 0 { 0.0 } else { win as f64 / total as f64 };
    OutcomeDistribution { win, lose, timeout, total, win_rate }
}

fn aggregate_reachability(results: &[LabeledResult], dist: &OutcomeDistribution) -> Reachability {
    // Union: event names fired across all sessions (BTreeSet auto-sorts and dedupes = deterministic)
    let mut reached: BTreeSet<String> = BTreeSet::new();
    for lr in results {
        for ev in &lr.result.fired_events {
            reached.insert(ev.clone());
        }
    }
    // unbeatable: there are sessions but 0 Win (when there are no sessions we don't make this
    // claim — no data isn't "can't be beaten")
    let unbeatable_by_swarm = dist.total > 0 && dist.win == 0;
    Reachability { reached_events: reached.into_iter().collect(), unbeatable_by_swarm }
}

/// Whether a session is "stuck": Timeout + the trailing identical state_hash runs for ≥ K. Returns
/// Some(frozen hash) if stuck, None otherwise.
/// Soft-lock clustering and numeric breakage's collapse judgment share it — same freeze criterion,
/// not duplicated to avoid threshold drift.
fn frozen_tail_hash(lr: &LabeledResult, freeze_k: usize) -> Option<u64> {
    // Only Timeout can be a soft-lock; Win/Lose reached the end normally, not stuck
    if lr.result.outcome != Outcome::Timeout {
        return None;
    }
    let trace = &lr.result.state_trace;
    let last = *trace.last()?;
    // Count backward from the end: how many consecutive trailing ticks have the end value (the
    // trailing freeze length)
    let mut run = 0usize;
    for &h in trace.iter().rev() {
        if h == last {
            run += 1;
        } else {
            break;
        }
    }
    if run >= freeze_k {
        Some(last)
    } else {
        None
    }
}

/// Set of stuck session indices (numeric breakage's collapse judgment intersects it with
/// "returned-to-zero fields").
fn stuck_session_indices(results: &[LabeledResult], freeze_k: usize) -> BTreeSet<usize> {
    results
        .iter()
        .enumerate()
        .filter(|(_, lr)| frozen_tail_hash(lr, freeze_k).is_some())
        .map(|(i, _)| i)
        .collect()
}

/// Soft-lock candidates: for each session, look at how long the trailing identical state_hash runs.
/// ≥K and didn't terminate → freeze candidate.
/// Bucketed by "hash at freeze time"; each bucket records hit count + one representative recording.
fn aggregate_stuck(results: &[LabeledResult], freeze_k: usize) -> Vec<StuckCluster> {
    // Bucket: frozen_hash -> (hit count, representative session). BTreeMap guarantees deterministic
    // output order.
    let mut buckets: BTreeMap<u64, (usize, &LabeledResult)> = BTreeMap::new();
    for lr in results {
        if let Some(last) = frozen_tail_hash(lr, freeze_k) {
            let entry = buckets.entry(last).or_insert((0, lr));
            entry.0 += 1;
            // Representative session keeps the first one encountered (BTreeMap output order is
            // deterministic, hit count accumulates)
        }
    }
    buckets
        .into_iter()
        .map(|(hash, (hits, rep))| StuckCluster {
            frozen_hash: format!("{hash:#018x}"),
            hits,
            sample_strategy: rep.spec.strategy_kind.name().to_string(),
            sample_seed: rep.spec.seed,
            representative: RecordingRef::from_sample(&format!("stuck-{hash:#018x}"), rep),
        })
        .collect()
}

/// Numeric breakage aggregation (design draft stage 4): cluster each session's `numeric_summary` by
/// **field name**, catching three kinds —
/// - runaway: the field is monotonic_up in some session AND (end value ≫ first value OR peak exceeds
///   the absolute threshold) → unbounded growth;
/// - collapse: the field hit_zero in some session AND **that session got stuck** (in stuck_idx) →
///   soft-lock after returning to zero;
/// - non_finite: the field had inf/nan in some session → overflow / divide-by-zero.
///
/// One pass over all sessions (O(sessions × field count), no history stored, no quadratic
/// comparison); each kind is bucketed by field name, accumulating hit count + keeping one
/// representative session (taking its strategy/seed/recording). Output sorted by field name
/// (BTreeMap) = deterministic.
fn aggregate_numeric_breakage(
    results: &[LabeledResult],
    stuck_idx: &BTreeSet<usize>,
) -> NumericBreakage {
    // One bucket map per kind: field -> (hit session count, peak max, representative session).
    // BTreeMap guarantees deterministic output.
    let mut runaway: BTreeMap<&str, (usize, f64, &LabeledResult)> = BTreeMap::new();
    let mut collapse: BTreeMap<&str, (usize, &LabeledResult)> = BTreeMap::new();
    let mut non_finite: BTreeMap<&str, (usize, &LabeledResult)> = BTreeMap::new();

    for (i, lr) in results.iter().enumerate() {
        let is_stuck = stuck_idx.contains(&i);
        for (field, stat) in &lr.result.numeric_summary {
            // Non-economy fields (position/velocity/lighting etc.) aren't economy resources; skip
            // them entirely in economy-breakage detection (avoid movement/lighting false reports)
            if is_non_economy_field(field) {
                continue;
            }
            // Overflow: a non-finite value was observed
            if stat.non_finite {
                let e = non_finite.entry(field.as_str()).or_insert((0, lr));
                e.0 += 1;
            }
            // Runaway: only-ever-grows AND (end/peak value far exceeds first value OR peak exceeds
            // the absolute threshold)
            if is_runaway(stat) {
                let e = runaway.entry(field.as_str()).or_insert((0, stat.max, lr));
                e.0 += 1;
                if stat.max > e.1 {
                    e.1 = stat.max; // Keep the largest peak in the bucket (most telling of runaway)
                }
            }
            // Collapse soft-lock: was >0 then returned to zero + that session got stuck. Requires
            // first>0 (dogfood: Run.node/Light.angle start at 0, that's not "collapse" but rather
            // "never grew", which is a stuck signal not collapse);
            // returning to zero alone doesn't count either (many resources legitimately hit 0 and
            // come back), must be combined with that session getting stuck.
            if stat.first > 0.0 && stat.hit_zero && is_stuck {
                let e = collapse.entry(field.as_str()).or_insert((0, lr));
                e.0 += 1;
            }
        }
    }

    NumericBreakage {
        runaway: runaway
            .into_iter()
            .map(|(field, (hits, peak, rep))| RunawayField {
                field: field.to_string(),
                hits,
                peak_max: peak,
                sample_strategy: rep.spec.strategy_kind.name().to_string(),
                sample_seed: rep.spec.seed,
                representative: RecordingRef::from_sample(
                    &format!("runaway-{}", sanitize(field)),
                    rep,
                ),
            })
            .collect(),
        collapse: collapse
            .into_iter()
            .map(|(field, (hits, rep))| CollapseField {
                field: field.to_string(),
                hits,
                sample_strategy: rep.spec.strategy_kind.name().to_string(),
                sample_seed: rep.spec.seed,
                representative: RecordingRef::from_sample(
                    &format!("collapse-{}", sanitize(field)),
                    rep,
                ),
            })
            .collect(),
        non_finite: non_finite
            .into_iter()
            .map(|(field, (hits, rep))| NonFiniteField {
                field: field.to_string(),
                hits,
                sample_strategy: rep.spec.strategy_kind.name().to_string(),
                sample_seed: rep.spec.seed,
                representative: RecordingRef::from_sample(
                    &format!("nonfinite-{}", sanitize(field)),
                    rep,
                ),
            })
            .collect(),
    }
}

/// Whether a field's whole-session summary hits the runaway criterion. Non-finite values go to
/// non_finite separately; this only looks at explosion within the finite range.
fn is_runaway(stat: &crate::session::NumericStat) -> bool {
    if !stat.monotonic_up || stat.non_finite {
        return false;
    }
    // Absolute threshold: peak exceeds 1e6 (when the first value is 0/negative the ratio is invalid,
    // this is the fallback)
    if stat.max > RUNAWAY_ABS {
        return true;
    }
    // Relative threshold: peak ≫ first value (only meaningful as a ratio when first > 0, to avoid
    // dividing by 0); and the peak must reach the absolute lower bound, otherwise growth like
    // "0.008 → 8.116" with an inflated ratio but actually tiny magnitude would be falsely flagged
    // (dogfood lesson).
    stat.first > 0.0 && stat.max > stat.first * RUNAWAY_RATIO && stat.max >= RUNAWAY_RATIO_MIN_ABS
}

/// LLM qualitative note aggregation (design draft stage 5): summarize each session's `notes`,
/// normalize-dedupe by (kind, text), accumulate hit counts, and attach the tick where it was first
/// seen / a representative session recording.
///
/// Dedupe key = (kind, normalized text): the same sentence is normalized into one entry count++
/// regardless of which session/tick it appears in, to avoid "the LLM says the same contradiction in
/// every session" flooding the report. Normalized text = trimmed original (no semantic clustering,
/// that would need another layer of LLM, beyond this stage's scope; honestly only literal dedupe).
/// Output sorted by (kind, text) (BTreeMap) = deterministic.
///
/// Note: notes themselves are non-deterministic products of LLM sessions, this aggregation does
/// **not** enter the determinism guarantee (design draft section 8 "LLM tier excluded"); but the
/// aggregation logic itself is a pure function — given the same batch of notes it always produces
/// the same summary.
fn aggregate_notes(results: &[LabeledResult]) -> QualitativeNotes {
    // Bucket: (kind, text) -> (hit count, representative tick, representative session). BTreeMap
    // guarantees deterministic output order.
    let mut buckets: BTreeMap<(String, String), (usize, u64, &LabeledResult)> = BTreeMap::new();
    let mut total = 0usize;
    for lr in results {
        for note in &lr.result.notes {
            total += 1;
            let key = (note.kind.clone(), note.text.trim().to_string());
            let entry = buckets.entry(key).or_insert((0, note.tick, lr));
            entry.0 += 1;
            // Representative tick/session keeps the first one encountered (BTreeMap output order is
            // deterministic, count accumulates)
        }
    }
    let clusters = buckets
        .into_iter()
        .map(|((kind, text), (count, tick, rep))| {
            let representative =
                RecordingRef::from_sample(&format!("note-{}", sanitize(&kind)), rep);
            NoteCluster {
                kind,
                text,
                count,
                sample_tick: tick,
                sample_strategy: rep.spec.strategy_kind.name().to_string(),
                sample_seed: rep.spec.seed,
                representative,
            }
        })
        .collect();
    QualitativeNotes { total, clusters }
}

fn aggregate_pacing(results: &[LabeledResult]) -> Pacing {
    // Ticks of terminating sessions (Win/Lose), sorted for min/median/max + histogram
    let mut term_ticks: Vec<u64> = results
        .iter()
        .filter(|lr| lr.result.outcome != Outcome::Timeout)
        .map(|lr| lr.result.ticks)
        .collect();
    let timeout_count = results.iter().filter(|lr| lr.result.outcome == Outcome::Timeout).count();
    term_ticks.sort_unstable();

    if term_ticks.is_empty() {
        return Pacing {
            terminated_min: None,
            terminated_median: None,
            terminated_max: None,
            histogram: Vec::new(),
            timeout_count,
        };
    }
    let min = term_ticks[0];
    let max = *term_ticks.last().expect("非空");
    let median = term_ticks[term_ticks.len() / 2];
    let histogram = build_histogram(&term_ticks, min, max);
    Pacing {
        terminated_min: Some(min),
        terminated_median: Some(median),
        terminated_max: Some(max),
        histogram,
        timeout_count,
    }
}

/// Fixed 5-bucket equal-width histogram. When min==max (all values the same) it degenerates into a
/// single bucket.
fn build_histogram(sorted: &[u64], min: u64, max: u64) -> Vec<HistogramBucket> {
    const N_BUCKETS: u64 = 5;
    if min == max {
        return vec![HistogramBucket { upper: max, count: sorted.len() }];
    }
    let span = max - min;
    let mut counts = vec![0usize; N_BUCKETS as usize];
    for &t in sorted {
        // Bucket: (t-min)/span maps to [0, N_BUCKETS); max goes to the last bucket
        let b = ((t - min) * N_BUCKETS / (span + 1)).min(N_BUCKETS - 1) as usize;
        counts[b] += 1;
    }
    (0..N_BUCKETS)
        .map(|i| {
            // Bucket upper bound: bucket i covers [min + i*step, min + (i+1)*step)
            let upper = min + (i + 1) * (span + 1) / N_BUCKETS;
            HistogramBucket { upper, count: counts[i as usize] }
        })
        .collect()
}

/// Inert action candidates: input actions that are declared but **no rule produces a real effect on
/// them**.
///
/// **Criterion changed from runtime to static** (root cause of false positives observed dogfooding
/// examples/jump): the old criterion looked at "did this action trigger any non-built-in event
/// (emit) across all sessions", but platformer movement keys (left/right/space/up) only
/// `set Velocity` (modify state) and **don't emit events**, so they got falsely flagged as inert —
/// any action that "only sets/modifies state without emitting" was hit, a high-frequency false
/// positive. The new criterion doesn't look at whether events fired at runtime, but **statically
/// scans the rules**: an input action is truly inert if and only if all rules it triggers have `do`s
/// that produce no real effect (empty `do`, or only self-nullifying operations like setting a field
/// to the initial value it already has). As long as one triggered rule contains a real-effect action
/// (spawn/despawn/emit/call, non-zero add, or setting a field to a new value ≠ initial) → not inert.
/// This is a conservative criterion: better to miss inert actions than to falsely flag commonly-used
/// ones.
///
/// `engine=Some` uses the static criterion (the default CLI path); `engine=None` (bare `aggregate`,
/// no rule info) degrades to the old runtime heuristic — actions "injected, but never co-occurring
/// with a non-built-in event in any session". The honest "candidate / pending review" wording is
/// unchanged: actions that legitimately produce no observable effect may also be hit, leave for
/// human review.
fn aggregate_inert(results: &[LabeledResult], engine: Option<&Engine>) -> Vec<String> {
    // Vocabulary: union of all actions injected across sessions (candidates only come from actions
    // actually injected, no speculating about actions that never ran).
    let mut vocab: BTreeSet<String> = BTreeSet::new();
    for lr in results {
        for r in &lr.result.recording.inputs {
            vocab.insert(r.action.clone());
        }
    }

    // Static criterion (when engine is present): scan rules, check for each input action whether
    // the rules it triggers have any real effect.
    if let Some(engine) = engine {
        let inert_set = static_inert_actions(engine);
        // Intersect with the injection vocabulary: only report actions that actually ran and were
        // judged to have no effect statically (avoid reporting actions that were never injected).
        return vocab.intersection(&inert_set).cloned().collect();
    }

    // Degraded path (no engine): the old runtime heuristic. The set of actions "that triggered a
    // non-built-in event" — if a session had a non-built-in event → all actions injected in that
    // session get a "possibly responsive" mark; inert = actions in the vocabulary that never
    // appeared in a responsive session.
    let mut responsive: BTreeSet<String> = BTreeSet::new();
    for lr in results {
        let actions_this: BTreeSet<&str> =
            lr.result.recording.inputs.iter().map(|r| r.action.as_str()).collect();
        let has_non_builtin = lr.result.fired_events.iter().any(|e| !is_builtin_event(e));
        if has_non_builtin {
            for a in &actions_this {
                responsive.insert(a.to_string());
            }
        }
    }
    vocab.difference(&responsive).cloned().collect()
}

/// Static criterion: scan the engine rules and return the set of input actions "whose triggered
/// rules all have no real effect".
///
/// For each action declared by an input rule (collected via `filter.action`), look at all rules
/// attached to it: as long as **one** rule's `do` contains a real-effect action → this action is
/// not inert; if all rules have no real effect → inert. (An action with no rule attached at all —
/// theoretically derive_actions wouldn't list it in the vocabulary, but if it happens it also
/// counts as inert, because "no rule attached = no effect".) Output BTreeSet auto-sorts and
/// dedupes, deterministic.
fn static_inert_actions(engine: &Engine) -> BTreeSet<String> {
    // action -> whether any "real-effect" rule is attached to it
    let mut has_effect: BTreeMap<&str, bool> = BTreeMap::new();
    for rule in &engine.rules.rules {
        let Trigger::Event { name, filter, .. } = &rule.trigger else {
            continue; // Only look at input-triggered rules
        };
        if name != "input" {
            continue;
        }
        let Some(action) = filter.get("action").and_then(|v| v.as_str()) else {
            continue;
        };
        let effectful = rule.actions.iter().any(|a| action_has_effect(a, &engine.schema));
        let entry = has_effect.entry(action).or_insert(false);
        *entry = *entry || effectful;
    }
    // Inert = actions with no effectful rule attached
    has_effect
        .into_iter()
        .filter(|(_, eff)| !*eff)
        .map(|(a, _)| a.to_string())
        .collect()
}

/// Whether a `do` action (JSON object) **could produce a real effect** statically. Conservative:
/// when in doubt, treat it as "has effect" (better to miss inert actions than to falsely flag
/// commonly-used ones).
/// - `spawn`/`despawn`/`emit`/`call`: definitely have effect (create/destroy entities, fire events,
///   call scripts).
/// - `add`: `by` not being literal 0 counts as having effect (adding 0 is a no-op). When `by`
///   can't be fetched / isn't a literal number → treat as having effect.
/// - `set`: setting a target field to its schema initial value (effective_default) = self-nullifying
///   no-op, no effect; setting to ≠ initial, or `to` being a dynamic reference / format string, or
///   the target not resolving to a schema field → treat as having effect.
fn action_has_effect(action: &Value, schema: &vitric_data::Schema) -> bool {
    let Some(obj) = action.as_object() else {
        return true; // Not an object (theoretically blocked at parse time), conservatively treat as having effect
    };
    // State create/destroy/fire-event/call-script — always have effect
    if obj.contains_key("spawn")
        || obj.contains_key("despawn")
        || obj.contains_key("emit")
        || obj.contains_key("call")
    {
        return true;
    }
    // add: by being literal 0 is the only no-op; everything else (non-zero, reference, missing)
    // counts as having effect
    if obj.contains_key("add") {
        return match obj.get("by").and_then(|v| v.as_f64()) {
            Some(by) => by != 0.0,
            None => true, // by is a dynamic value like a reference/format string → treat as having effect (conservative)
        };
    }
    // set: judge whether it's "setting a field to the initial value it already had" — a self-nullifying no-op
    if let Some(target) = obj.get("set").and_then(|v| v.as_str()) {
        let Some(to) = obj.get("to") else {
            return true; // Missing to (blocked at parse time), conservative
        };
        // to is a dynamic reference (string referencing self./other./@/event.) or a format object → treat as having effect
        if !is_static_literal(to) {
            return true;
        }
        match field_def(schema, target) {
            // Resolved to a schema field: normalize `to` by field type and compare to the initial
            // value (to bypass int/float representation differences like 0 vs 0.0 — the initial
            // value is already canonicalized to float, `to` must be normalized the same way to
            // compare correctly). After normalization = initial value → self-nullifying no-op, no
            // effect; ≠ initial value → has effect.
            Some(fdef) => fdef.ty.canonicalize(to) != fdef.effective_default(),
            // Target path doesn't resolve to a schema field (custom path / undeclared component) →
            // conservatively treat as having effect
            None => true,
        }
    } else {
        // Reaching here means it's an unknown action type (blocked at parse time), conservatively
        // treat as having effect
        true
    }
}

/// Whether `to`'s value is a "static literal constant" (can be directly compared to the initial
/// value). Strings containing reference prefixes (self./other./@/event.) or being objects/arrays
/// (like {"format":...}) all count as dynamic, not static literals.
fn is_static_literal(v: &Value) -> bool {
    match v {
        Value::String(s) => {
            !(s.starts_with("self.")
                || s.starts_with("other.")
                || s.starts_with('@')
                || s.starts_with("event."))
        }
        // Objects/arrays may be format/reference structures, not treated as static literals
        Value::Object(_) | Value::Array(_) => false,
        // Numbers/booleans/null are all static literals
        _ => true,
    }
}

/// Parse a set target path `<entity reference>.<component>.<field>` and look up its field definition
/// in the schema.
/// The entity reference is the first segment (@name / self / other / handle), the component is the
/// second, the rest is the field path. Only supports top-level fields (component.field, no deeper
/// nesting) — enough to cover landmine/movement-key usage; if the component/field can't be parsed,
/// returns None (the caller conservatively treats it as "has effect").
fn field_def<'a>(schema: &'a vitric_data::Schema, target: &str) -> Option<&'a vitric_data::FieldDef> {
    let mut segs = target.split('.');
    let _entity = segs.next()?; // Entity reference segment, skip
    let component = segs.next()?;
    let field = segs.next()?;
    // Deeper nested fields (like vec2.x) — don't judge the initial value; conservatively let the
    // caller treat as having effect
    if segs.next().is_some() {
        return None;
    }
    schema.components.get(component)?.fields.get(field)
}

fn aggregate_dominant(results: &[LabeledResult]) -> DominantStrategy {
    // Grouping: strategy_kind -> (session count, win count, list of winning-session ticks)
    let mut groups: BTreeMap<&'static str, (usize, usize, Vec<u64>)> = BTreeMap::new();
    // First ensure all three strategies have a bucket (for stable output even when a strategy has
    // 0 sessions)
    for kind in StrategyKind::ALL {
        groups.entry(kind.name()).or_insert((0, 0, Vec::new()));
    }
    for lr in results {
        let entry = groups.entry(lr.spec.strategy_kind.name()).or_insert((0, 0, Vec::new()));
        entry.0 += 1;
        if lr.result.outcome == Outcome::Win {
            entry.1 += 1;
            entry.2.push(lr.result.ticks);
        }
    }
    let mut per_strategy: Vec<StrategyStats> = Vec::new();
    for (name, (sessions, wins, mut win_ticks)) in groups {
        let win_rate = if sessions == 0 { 0.0 } else { wins as f64 / sessions as f64 };
        win_ticks.sort_unstable();
        let median_win_ticks =
            if win_ticks.is_empty() { None } else { Some(win_ticks[win_ticks.len() / 2]) };
        per_strategy.push(StrategyStats {
            strategy: name.to_string(),
            sessions,
            win_rate,
            median_win_ticks,
        });
    }
    // Dominance decision: the strategy with the highest win rate ≥2× the runner-up, and both sides
    // have ≥4 sessions (a sufficient sample to make a claim)
    let dominant = find_dominant(&per_strategy);
    // One-trick: in winning sessions some action highly dominates other actions (design draft stage
    // 4 "dominant strategy deepening")
    let dominant_action = find_dominant_action(results);
    DominantStrategy { per_strategy, dominant, dominant_action }
}

/// One-trick action: count injections per action in **winning sessions**; if some action accounts
/// for ≥80% of total injections, and there are enough winning sessions as a baseline, flag it as a
/// candidate (this one action dominates other playstyles, other choices are meaningless).
/// Data source = winning sessions' recording inputs (injected actions = the recording,
/// deterministically replayable).
fn find_dominant_action(results: &[LabeledResult]) -> Option<DominantAction> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut total = 0usize;
    let mut winning_sessions = 0usize;
    for lr in results {
        if lr.result.outcome != Outcome::Win {
            continue;
        }
        winning_sessions += 1;
        for inp in &lr.result.recording.inputs {
            *counts.entry(inp.action.as_str()).or_insert(0) += 1;
            total += 1;
        }
    }
    if winning_sessions < DOMINANT_ACTION_MIN_WINS || total == 0 {
        return None; // Too few winning sessions / winning without pressing actions (instant clear),
        // don't make a claim
    }
    // Take the most-injected action; on ties take the lexicographically smallest field name
    // (BTreeMap order, deterministic)
    let (action, &top) = counts.iter().max_by_key(|(name, c)| (**c, std::cmp::Reverse(**name)))?;
    let share = top as f64 / total as f64;
    if share >= DOMINANT_ACTION_SHARE {
        Some(DominantAction { action: action.to_string(), share, winning_sessions })
    } else {
        None
    }
}

/// Dominant: sort by win_rate descending; the top one is ≥2× the runner-up and the top one's sample
/// is ≥4 sessions; when the runner-up's win_rate=0, as long as the top one actually has wins and a
/// sufficient sample it counts as domination (2× 0 is still 0, handled separately).
fn find_dominant(stats: &[StrategyStats]) -> Option<String> {
    const MIN_SAMPLE: usize = 4;
    let mut ranked: Vec<&StrategyStats> =
        stats.iter().filter(|s| s.sessions >= MIN_SAMPLE).collect();
    if ranked.len() < 2 {
        return None; // Fewer than two strategies with sufficient samples, can't compare "domination"
    }
    ranked.sort_by(|a, b| b.win_rate.partial_cmp(&a.win_rate).expect("win_rate 非 NaN"));
    let top = ranked[0];
    let second = ranked[1];
    if top.win_rate <= 0.0 {
        return None; // Top one didn't win either, no dominance to speak of
    }
    if top.win_rate >= 2.0 * second.win_rate {
        Some(top.strategy.clone())
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn build_summary(
    dist: &OutcomeDistribution,
    reach: &Reachability,
    ending: &Option<EndingCoverage>,
    stuck: &[StuckCluster],
    inert: &[String],
    dominant: &DominantStrategy,
    numeric: &NumericBreakage,
    notes: &QualitativeNotes,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "跑了 {} 局：通关 {}、失败 {}、超时 {}，通关率 {:.0}%。",
        dist.total,
        dist.win,
        dist.lose,
        dist.timeout,
        dist.win_rate * 100.0
    ));
    if reach.unbeatable_by_swarm {
        parts.push("⚠ swarm 一局都没通关——声明了能赢但谁也赢不到，疑似不可达通关条件。".to_string());
    }
    if let Some(ec) = ending {
        if !ec.declared_endings.is_empty() {
            if ec.unreachable_endings.is_empty() {
                parts.push(format!(
                    "结局覆盖：声明 {} 个结局，全部被触达。",
                    ec.declared_endings.len()
                ));
            } else {
                parts.push(format!(
                    "⚠ 不可达结局 {} 个：{}（声明了但任何扰动都到不了，疑似 flag bug）。",
                    ec.unreachable_endings.len(),
                    ec.unreachable_endings.join("/")
                ));
            }
        }
    }
    if !stuck.is_empty() {
        let hits: usize = stuck.iter().map(|c| c.hits).sum();
        parts.push(format!(
            "发现 {} 簇卡死候选（共 {} 局末尾状态冻结未到终止，可重放复核）。",
            stuck.len(),
            hits
        ));
    }
    if !inert.is_empty() {
        parts.push(format!("疑似惰性动作候选 {} 个：{}（声明了输入但没引发响应，待复核）。", inert.len(), inert.join("/")));
    }
    if let Some(d) = &dominant.dominant {
        parts.push(format!("策略 {d} 通关率碾压其他（疑似一招鲜，选择意义存疑）。"));
    }
    if let Some(da) = &dominant.dominant_action {
        parts.push(format!(
            "⚠ 通关几乎全靠动作「{}」（占注入 {:.0}%，{} 局通关），疑似一招鲜，其他选择没意义。",
            da.action,
            da.share * 100.0,
            da.winning_sessions
        ));
    }
    if !numeric.runaway.is_empty() {
        let fields: Vec<&str> = numeric.runaway.iter().map(|r| r.field.as_str()).collect();
        parts.push(format!(
            "⚠ 经济跑飞候选 {} 个字段：{}（无界增长，最高峰值 {:.3e}，可重放复核）。",
            numeric.runaway.len(),
            fields.join("/"),
            numeric.runaway.iter().map(|r| r.peak_max).fold(0.0_f64, f64::max)
        ));
    }
    if !numeric.collapse.is_empty() {
        let fields: Vec<&str> = numeric.collapse.iter().map(|c| c.field.as_str()).collect();
        parts.push(format!(
            "⚠ 经济崩盘软锁候选 {} 个字段：{}（资源归零后世界冻结，可重放复核）。",
            numeric.collapse.len(),
            fields.join("/")
        ));
    }
    if !numeric.non_finite.is_empty() {
        let fields: Vec<&str> = numeric.non_finite.iter().map(|c| c.field.as_str()).collect();
        parts.push(format!(
            "⚠ 数值溢出候选 {} 个字段：{}（出现 inf/nan）。",
            numeric.non_finite.len(),
            fields.join("/")
        ));
    }
    if !notes.clusters.is_empty() {
        // Count how many deduped entries per kind (a continuity/clarity/choice overview)
        let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
        for c in &notes.clusters {
            *by_kind.entry(c.kind.as_str()).or_insert(0) += 1;
        }
        let breakdown: Vec<String> =
            by_kind.iter().map(|(k, n)| format!("{k} {n}")).collect();
        parts.push(format!(
            "LLM 定性提示 {} 条（去重 {} 条：{}）——主观感受，非真人判，待人复核。",
            notes.total,
            notes.clusters.len(),
            breakdown.join("/")
        ));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_view::Outcome;
    use crate::session::{NumericStat, SessionResult};
    use crate::swarm::{SessionSpec, StrategyKind};
    use vitric_sim::{InputRecord, Recording};

    /// Build a LabeledResult with telemetry (to feed the aggregator).
    fn labeled(
        kind: StrategyKind,
        seed: u64,
        outcome: Outcome,
        ticks: u64,
        state_trace: Vec<u64>,
        fired_events: Vec<&str>,
        injected: Vec<&str>,
    ) -> LabeledResult {
        let mut recording = Recording { ticks, ..Default::default() };
        for (i, a) in injected.iter().enumerate() {
            recording.inputs.push(InputRecord {
                tick: i as u64,
                action: a.to_string(),
                phase: "pressed".to_string(),
            });
        }
        LabeledResult {
            spec: SessionSpec::new(kind, seed, ticks + 100),
            result: SessionResult {
                outcome,
                ticks,
                recording,
                state_trace,
                fired_events: fired_events.iter().map(|s| s.to_string()).collect(),
                numeric_summary: std::collections::BTreeMap::new(),
                notes: Vec::new(),
            },
        }
    }

    /// Build a LabeledResult with LLM notes (to feed the note aggregator).
    fn labeled_with_notes(
        kind: StrategyKind,
        seed: u64,
        notes: Vec<crate::strategy::PlaytestNote>,
    ) -> LabeledResult {
        let mut lr = labeled(kind, seed, Outcome::Timeout, 30, vec![], vec![], vec![]);
        lr.result.notes = notes;
        lr
    }

    fn note(tick: u64, kind: &str, text: &str) -> crate::strategy::PlaytestNote {
        crate::strategy::PlaytestNote { tick, kind: kind.to_string(), text: text.to_string() }
    }

    /// Build a LabeledResult with a numeric summary (to feed the numeric-breakage aggregator).
    fn labeled_numeric(
        kind: StrategyKind,
        seed: u64,
        outcome: Outcome,
        ticks: u64,
        state_trace: Vec<u64>,
        numeric: Vec<(&str, NumericStat)>,
    ) -> LabeledResult {
        let mut lr = labeled(kind, seed, outcome, ticks, state_trace, vec![], vec![]);
        lr.result.numeric_summary =
            numeric.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        lr
    }

    #[test]
    fn outcome_distribution_counts_and_rate() {
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 1, Outcome::Lose, 5, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 2, Outcome::Timeout, 50, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 3, Outcome::Win, 8, vec![], vec![], vec![]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.outcome_distribution.win, 2);
        assert_eq!(rep.outcome_distribution.lose, 1);
        assert_eq!(rep.outcome_distribution.timeout, 1);
        assert_eq!(rep.outcome_distribution.total, 4);
        assert!((rep.outcome_distribution.win_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn reachability_unbeatable_when_zero_wins() {
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Timeout, 50, vec![], vec!["near-win"], vec![]),
            labeled(StrategyKind::Random, 1, Outcome::Lose, 30, vec![], vec!["game-over"], vec![]),
        ];
        let rep = aggregate(&r);
        assert!(rep.reachability.unbeatable_by_swarm, "0 局 win 必须标 unbeatable");
        assert!(rep.reachability.reached_events.contains(&"game-over".to_string()));
        assert!(rep.reachability.reached_events.contains(&"near-win".to_string()));
    }

    #[test]
    fn reachability_not_unbeatable_when_some_win() {
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec!["game-won"], vec![]),
            labeled(StrategyKind::Random, 1, Outcome::Timeout, 50, vec![], vec![], vec![]),
        ];
        let rep = aggregate(&r);
        assert!(!rep.reachability.unbeatable_by_swarm);
    }

    #[test]
    fn reachability_empty_is_not_unbeatable() {
        // No sessions = no data, don't make a "can't be beaten" claim
        let rep = aggregate(&[]);
        assert!(!rep.reachability.unbeatable_by_swarm);
    }

    #[test]
    fn stuck_clusters_groups_frozen_tail() {
        // Both sessions freeze on the same hash (=999) for >K consecutive trailing ticks, and both
        // are Timeout
        let mut trace_a = vec![1, 2, 3];
        trace_a.extend(std::iter::repeat_n(999u64, 70)); // trailing 70 ticks frozen at 999
        let mut trace_b = vec![5, 6];
        trace_b.extend(std::iter::repeat_n(999u64, 65));
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Timeout, trace_a.len() as u64, trace_a, vec![], vec![]),
            labeled(StrategyKind::Greedy, 1, Outcome::Timeout, trace_b.len() as u64, trace_b, vec![], vec![]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.stuck_clusters.len(), 1, "同 hash 聚成一簇");
        assert_eq!(rep.stuck_clusters[0].hits, 2);
        assert_eq!(rep.stuck_clusters[0].frozen_hash, format!("{:#018x}", 999u64));
    }

    #[test]
    fn stuck_ignores_short_freeze_and_terminated() {
        // Freeze shorter than K, doesn't count
        let mut short = vec![1, 2];
        short.extend(std::iter::repeat_n(7u64, 10));
        // Freeze long enough but already Win (reached the end normally, not a soft-lock)
        let mut won = vec![1u64];
        won.extend(std::iter::repeat_n(8u64, 80));
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Timeout, short.len() as u64, short, vec![], vec![]),
            labeled(StrategyKind::Random, 1, Outcome::Win, won.len() as u64, won, vec![], vec![]),
        ];
        let rep = aggregate(&r);
        assert!(rep.stuck_clusters.is_empty(), "短冻结/已终止都不算软锁: {:?}", rep.stuck_clusters);
    }

    #[test]
    fn pacing_min_median_max_and_timeout_split() {
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 1, Outcome::Win, 20, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 2, Outcome::Lose, 30, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 3, Outcome::Timeout, 99, vec![], vec![], vec![]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.pacing.terminated_min, Some(10));
        assert_eq!(rep.pacing.terminated_max, Some(30));
        assert_eq!(rep.pacing.terminated_median, Some(20));
        assert_eq!(rep.pacing.timeout_count, 1, "timeout 单列不进 min/max");
        let total_in_hist: usize = rep.pacing.histogram.iter().map(|b| b.count).sum();
        assert_eq!(total_in_hist, 3, "直方只装 3 个终止局");
    }

    #[test]
    fn inert_actions_flags_action_with_no_response() {
        // dead action: injected but its session had no non-built-in events
        // live action: its session had a non-built-in event noop
        let r = vec![
            // One session injects left, fires noop (non-built-in) → left is responsive
            labeled(StrategyKind::Coverage, 0, Outcome::Timeout, 5, vec![], vec!["noop"], vec!["left"]),
            // One session injects dead, only has the built-in input event → dead is unresponsive
            labeled(StrategyKind::Coverage, 1, Outcome::Timeout, 5, vec![], vec!["input"], vec!["dead"]),
        ];
        let rep = aggregate(&r);
        assert!(rep.inert_actions.contains(&"dead".to_string()), "{:?}", rep.inert_actions);
        assert!(!rep.inert_actions.contains(&"left".to_string()));
    }

    /// Build an engine: given several input rules (action, do action list) + schema.
    fn inert_engine(rules: serde_json::Value, schema: serde_json::Value) -> Engine {
        let schema = Schema::parse(&schema, "s.json").unwrap();
        Engine::new(RuleSet::parse(&rules, "r.json").unwrap(), schema)
    }

    #[test]
    fn static_inert_set_to_default_is_inert_but_set_to_new_value_is_not() {
        // useless: set Scratch.v to its initial value 0 (self-nullifying no-op) → inert
        // live:    set Vel.x to -8 (≠ initial 0, a real state change) → not inert
        let eng = inert_engine(
            serde_json::json!({"rules": [
                {"id": "useless", "on": {"event": "input", "filter": {"action": "useless", "phase": "pressed"}},
                 "do": [{"set": "@s.Scratch.v", "to": 0}]},
                {"id": "live", "on": {"event": "input", "filter": {"action": "live", "phase": "pressed"}},
                 "do": [{"set": "@h.Vel.x", "to": -8}]}
            ]}),
            serde_json::json!({"components": {
                "Scratch": {"fields": {"v": {"type": "number", "default": 0}}},
                "Vel": {"fields": {"x": {"type": "number"}}}
            }}),
        );
        // Both actions were injected (so they're in the candidate vocabulary)
        let r = vec![
            labeled(StrategyKind::Coverage, 0, Outcome::Timeout, 5, vec![], vec![], vec!["useless"]),
            labeled(StrategyKind::Coverage, 1, Outcome::Timeout, 5, vec![], vec![], vec!["live"]),
        ];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        assert!(rep.inert_actions.contains(&"useless".to_string()), "set 成初值=惰性: {:?}", rep.inert_actions);
        assert!(!rep.inert_actions.contains(&"live".to_string()), "set 成新值=不惰性: {:?}", rep.inert_actions);
    }

    #[test]
    fn static_inert_emit_and_spawn_are_effectful() {
        // Rules with emit / spawn always have a real effect → corresponding actions aren't inert
        let eng = inert_engine(
            serde_json::json!({"rules": [
                {"id": "fire", "on": {"event": "input", "filter": {"action": "fire", "phase": "pressed"}},
                 "do": [{"emit": "boom", "data": {}}]},
                {"id": "make", "on": {"event": "input", "filter": {"action": "make", "phase": "pressed"}},
                 "do": [{"spawn": "Bullet", "components": {}}]}
            ]}),
            serde_json::json!({"components": {}}),
        );
        let r = vec![
            labeled(StrategyKind::Coverage, 0, Outcome::Timeout, 5, vec![], vec![], vec!["fire"]),
            labeled(StrategyKind::Coverage, 1, Outcome::Timeout, 5, vec![], vec![], vec!["make"]),
        ];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        assert!(rep.inert_actions.is_empty(), "emit/spawn 都有效果，无惰性: {:?}", rep.inert_actions);
    }

    #[test]
    fn static_inert_multi_rule_one_effectful_wins() {
        // Same action with two rules: one self-nullifying (set to initial value), one with effect
        // (emit) → overall not inert
        let eng = inert_engine(
            serde_json::json!({"rules": [
                {"id": "noop", "on": {"event": "input", "filter": {"action": "act", "phase": "pressed"}},
                 "do": [{"set": "@s.Scratch.v", "to": 0}]},
                {"id": "real", "on": {"event": "input", "filter": {"action": "act", "phase": "pressed"}},
                 "do": [{"emit": "ping", "data": {}}]}
            ]}),
            serde_json::json!({"components": {
                "Scratch": {"fields": {"v": {"type": "number", "default": 0}}}
            }}),
        );
        let r = vec![labeled(StrategyKind::Coverage, 0, Outcome::Timeout, 5, vec![], vec![], vec!["act"])];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        assert!(!rep.inert_actions.contains(&"act".to_string()), "有一条有效果就不惰性: {:?}", rep.inert_actions);
    }

    #[test]
    fn static_inert_only_reports_injected_actions() {
        // Statically useless is inert, but if it was never injected → not reported (candidates only
        // come from actions that actually ran)
        let eng = inert_engine(
            serde_json::json!({"rules": [
                {"id": "useless", "on": {"event": "input", "filter": {"action": "useless", "phase": "pressed"}},
                 "do": [{"set": "@s.Scratch.v", "to": 0}]}
            ]}),
            serde_json::json!({"components": {
                "Scratch": {"fields": {"v": {"type": "number", "default": 0}}}
            }}),
        );
        // No actions injected
        let r = vec![labeled(StrategyKind::Coverage, 0, Outcome::Timeout, 5, vec![], vec![], vec![])];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        assert!(rep.inert_actions.is_empty(), "没注入过的动作不进候选: {:?}", rep.inert_actions);
    }

    #[test]
    fn dominant_strategy_flagged_when_one_crushes() {
        // coverage 4 sessions all win (win_rate 1.0), random 4 sessions all timeout (0.0) → coverage
        // dominates
        let mut r = Vec::new();
        for seed in 0..4u64 {
            r.push(labeled(StrategyKind::Coverage, seed, Outcome::Win, 10, vec![], vec![], vec![]));
            r.push(labeled(StrategyKind::Random, seed, Outcome::Timeout, 99, vec![], vec![], vec![]));
        }
        let rep = aggregate(&r);
        assert_eq!(rep.dominant_strategy.dominant, Some("coverage".to_string()));
    }

    #[test]
    fn dominant_none_when_close() {
        // Two strategies' win rates are close (less than 2×) → no dominant flagged
        let mut r = Vec::new();
        for seed in 0..4u64 {
            // coverage 3/4 win, random 2/4 win: 1.5× < 2×
            r.push(labeled(StrategyKind::Coverage, seed, if seed < 3 { Outcome::Win } else { Outcome::Timeout }, 10, vec![], vec![], vec![]));
            r.push(labeled(StrategyKind::Random, seed, if seed < 2 { Outcome::Win } else { Outcome::Timeout }, 10, vec![], vec![], vec![]));
        }
        let rep = aggregate(&r);
        assert_eq!(rep.dominant_strategy.dominant, None);
    }

    #[test]
    fn dominant_none_when_sample_too_small() {
        // Sample smaller than 4 sessions: don't make a dominance claim
        let r = vec![
            labeled(StrategyKind::Coverage, 0, Outcome::Win, 10, vec![], vec![], vec![]),
            labeled(StrategyKind::Random, 0, Outcome::Timeout, 99, vec![], vec![], vec![]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.dominant_strategy.dominant, None);
    }

    use crate::scene_view::TerminalSpec;
    use vitric_data::Schema;
    use vitric_rules::{Engine, RuleSet};

    /// Build an engine that declares several ending events (via emit in rules).
    fn engine_with_emits(emit_events: &[&str]) -> Engine {
        let rules: Vec<serde_json::Value> = emit_events
            .iter()
            .enumerate()
            .map(|(i, ev)| {
                serde_json::json!({
                    "id": format!("end-{i}"),
                    "on": {"event": "input", "filter": {"action": format!("a{i}"), "phase": "pressed"}},
                    "do": [{"emit": ev, "data": {}}]
                })
            })
            .collect();
        let schema = Schema::parse(&serde_json::json!({"components": {}}), "s.json").unwrap();
        Engine::new(
            RuleSet::parse(&serde_json::json!({"rules": rules}), "r.json").unwrap(),
            schema,
        )
    }

    #[test]
    fn ending_coverage_flags_declared_but_unreached() {
        // The engine declares it can emit ending-good and ending-bad; at runtime only ending-bad was
        // reached
        let eng = engine_with_emits(&["ending-good", "ending-bad"]);
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec!["ending-bad"], vec![]),
        ];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        let ec = rep.ending_coverage.expect("传了引擎应算结局覆盖");
        assert_eq!(ec.declared_endings, vec!["ending-bad".to_string(), "ending-good".to_string()]);
        assert_eq!(ec.reached_endings, vec!["ending-bad".to_string()]);
        assert_eq!(ec.unreachable_endings, vec!["ending-good".to_string()], "声明了没到的算不可达");
    }

    #[test]
    fn ending_coverage_all_reached_is_empty_unreachable() {
        let eng = engine_with_emits(&["ending-a", "ending-b"]);
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec!["ending-a"], vec![]),
            labeled(StrategyKind::Random, 1, Outcome::Win, 12, vec![], vec!["ending-b"], vec![]),
        ];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        let ec = rep.ending_coverage.unwrap();
        assert!(ec.unreachable_endings.is_empty(), "两个都到了，无不可达: {ec:?}");
        assert_eq!(ec.reached_endings.len(), 2);
    }

    #[test]
    fn ending_coverage_only_counts_terminal_emits() {
        // The engine emits a non-ending event milestone and an ending ending-x: only ending-x counts
        // as a declared ending
        let eng = engine_with_emits(&["milestone", "ending-x"]);
        let r = vec![labeled(StrategyKind::Random, 0, Outcome::Timeout, 50, vec![], vec!["milestone"], vec![])];
        let rep = aggregate_with_endings(&r, &eng, &TerminalSpec::default());
        let ec = rep.ending_coverage.unwrap();
        assert_eq!(ec.declared_endings, vec!["ending-x".to_string()], "milestone 不是结局，不算声明");
        assert_eq!(ec.unreachable_endings, vec!["ending-x".to_string()]);
    }

    #[test]
    fn ending_coverage_none_without_engine() {
        let r = vec![labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec!["game-won"], vec![])];
        let rep = aggregate(&r);
        assert!(rep.ending_coverage.is_none(), "不传引擎=不算结局覆盖");
    }

    #[test]
    fn report_is_serializable_and_has_summary() {
        let r = vec![labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec!["game-won"], vec![])];
        let rep = aggregate(&r);
        assert!(!rep.summary.is_empty());
        let json = serde_json::to_string(&rep).expect("报告可序列化");
        assert!(json.contains("outcome_distribution"));
        assert!(json.contains("summary"));
    }

    #[test]
    fn aggregate_is_deterministic() {
        let build = || {
            vec![
                labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![1, 2, 3], vec!["game-won"], vec!["a"]),
                labeled(StrategyKind::Coverage, 1, Outcome::Timeout, 50, vec![9; 70], vec!["input"], vec!["b"]),
            ]
        };
        let a = aggregate(&build());
        let b = aggregate(&build());
        assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
    }

    // ---- Numeric breakage dimension (numeric_breakage) unit tests ----

    /// Build a NumericStat with scenario-specific values (bypassing private start/observe).
    fn nstat(
        first: f64,
        last: f64,
        min: f64,
        max: f64,
        monotonic_up: bool,
        hit_zero: bool,
        non_finite: bool,
    ) -> NumericStat {
        NumericStat { first, last, min, max, monotonic_up, hit_zero, non_finite }
    }

    #[test]
    fn numeric_breakage_flags_runaway_by_absolute_threshold() {
        // gold grows monotonically from 100 to 5e6 (> 1e6 absolute threshold) → runaway
        let r = vec![labeled_numeric(
            StrategyKind::Economy,
            0,
            Outcome::Timeout,
            300,
            vec![],
            vec![("treasury/Resources.gold", nstat(100.0, 5e6, 100.0, 5e6, true, false, false))],
        )];
        let rep = aggregate(&r);
        assert_eq!(rep.numeric_breakage.runaway.len(), 1, "应逮到一个跑飞字段");
        assert_eq!(rep.numeric_breakage.runaway[0].field, "treasury/Resources.gold");
        assert!(rep.numeric_breakage.runaway[0].peak_max >= 5e6);
    }

    #[test]
    fn numeric_breakage_flags_runaway_by_ratio() {
        // gold 50 → 80000 (1600× > 1000× ratio threshold), peak <1e6 but a relative explosion
        let r = vec![labeled_numeric(
            StrategyKind::Economy,
            0,
            Outcome::Timeout,
            100,
            vec![],
            vec![("bank/R.gold", nstat(50.0, 80000.0, 50.0, 80000.0, true, false, false))],
        )];
        let rep = aggregate(&r);
        assert_eq!(rep.numeric_breakage.runaway.len(), 1, "倍率阈也应逮到");
    }

    #[test]
    fn numeric_breakage_excludes_spatial_fields() {
        // dogfood lesson: gravity falling made Position.y grow monotonically from near 0 to tens,
        // with an inflated ratio but not economy runaway. The same statistics on Position (spatial)
        // must be excluded, and on economy fields also not falsely flagged due to a too-small peak.
        let stats = nstat(0.008, 8.116, 0.008, 8.116, true, false, false);
        let r = vec![labeled_numeric(
            StrategyKind::Random,
            0,
            Outcome::Timeout,
            300,
            vec![],
            vec![
                ("hero/Position.y", stats.clone()),
                ("hero/Velocity.x", stats.clone()),
                ("hero/Resources.coins", stats), // economy field but peak 8.116 < absolute lower bound → also not flagged
            ],
        )];
        let rep = aggregate(&r);
        assert!(
            rep.numeric_breakage.runaway.is_empty(),
            "Position/Velocity 空间字段 + 微小经济增长都不该报跑飞: {:?}",
            rep.numeric_breakage.runaway
        );
    }

    #[test]
    fn numeric_breakage_ratio_needs_min_abs() {
        // Economy field ratio exceeds 1000× but peak only reaches 8.116 (< absolute lower bound) →
        // not runaway (avoids false positives from small starting values)
        let r = vec![labeled_numeric(
            StrategyKind::Economy,
            0,
            Outcome::Timeout,
            100,
            vec![],
            vec![("shop/R.coins", nstat(0.008, 8.116, 0.008, 8.116, true, false, false))],
        )];
        let rep = aggregate(&r);
        assert!(rep.numeric_breakage.runaway.is_empty(), "倍率虚高但峰值太小不算跑飞");
    }

    #[test]
    fn numeric_breakage_runaway_needs_monotonic() {
        // Peak exceeds 1e6 but dropped midway (non-monotonic) → not runaway (up-and-down
        // oscillation isn't unbounded growth)
        let r = vec![labeled_numeric(
            StrategyKind::Economy,
            0,
            Outcome::Timeout,
            100,
            vec![],
            vec![("x/Y.z", nstat(100.0, 200.0, 50.0, 2e6, false, false, false))],
        )];
        let rep = aggregate(&r);
        assert!(rep.numeric_breakage.runaway.is_empty(), "非单调不算跑飞");
    }

    #[test]
    fn numeric_breakage_clusters_runaway_by_field_name() {
        // Same field name hit in multiple sessions → cluster into one bucket, accumulate hits, take
        // the max peak
        let r = vec![
            labeled_numeric(StrategyKind::Economy, 0, Outcome::Timeout, 100, vec![],
                vec![("v/R.gold", nstat(1.0, 2e6, 1.0, 2e6, true, false, false))]),
            labeled_numeric(StrategyKind::Economy, 1, Outcome::Timeout, 100, vec![],
                vec![("v/R.gold", nstat(1.0, 9e6, 1.0, 9e6, true, false, false))]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.numeric_breakage.runaway.len(), 1, "同字段聚一桶");
        assert_eq!(rep.numeric_breakage.runaway[0].hits, 2);
        assert!((rep.numeric_breakage.runaway[0].peak_max - 9e6).abs() < 1.0, "峰值取最大");
    }

    #[test]
    fn numeric_breakage_flags_collapse_only_when_stuck() {
        // Both sessions return to zero; but only the stuck one (trailing freeze ≥K) counts as a
        // collapse soft-lock
        let mut frozen = vec![1u64, 2];
        frozen.extend(std::iter::repeat_n(7u64, 70)); // trailing freeze → stuck
        let r = vec![
            // Returned to zero + stuck → collapse
            labeled_numeric(StrategyKind::Economy, 0, Outcome::Timeout, frozen.len() as u64, frozen,
                vec![("base/Res.fuel", nstat(100.0, 0.0, 0.0, 100.0, false, true, false))]),
            // Returned to zero but not stuck (state still changing) → doesn't count (resources
            // legitimately hit 0 and come back)
            labeled_numeric(StrategyKind::Economy, 1, Outcome::Timeout, 5, vec![1, 2, 3, 4, 5],
                vec![("base/Res.fuel", nstat(100.0, 0.0, 0.0, 100.0, false, true, false))]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.numeric_breakage.collapse.len(), 1, "只卡死那局算崩盘");
        assert_eq!(rep.numeric_breakage.collapse[0].field, "base/Res.fuel");
        assert_eq!(rep.numeric_breakage.collapse[0].hits, 1);
    }

    #[test]
    fn numeric_breakage_collapse_excludes_light_and_zero_start() {
        // dogfood echo lesson: when stuck on the menu state, any field that was 0 at the time got
        // misreported as collapse. Non-economy fields (Light) are excluded entirely; fields whose
        // initial value is 0 (Run.node never grew) aren't "collapse" but a stuck signal; only real
        // economy resources that "were >0 then returned to 0 + stuck" count as collapse.
        let mut frozen = vec![1u64, 2];
        frozen.extend(std::iter::repeat_n(7u64, 70)); // trailing freeze → stuck
        let r = vec![labeled_numeric(
            StrategyKind::Random,
            0,
            Outcome::Timeout,
            frozen.len() as u64,
            frozen,
            vec![
                // Light returned to zero (non-economy) → excluded
                ("menu-lamp/Light.angle", nstat(0.5, 0.0, 0.0, 0.5, false, true, false)),
                // Progress's initial value is 0 (never grew) → not collapse, it's a stuck signal
                ("progress/Run.node", nstat(0.0, 0.0, 0.0, 0.0, false, true, false)),
                // Real economy resource: was >0 then returned to 0 + stuck → only this counts as
                // collapse (regression protection)
                ("base/Res.fuel", nstat(100.0, 0.0, 0.0, 100.0, false, true, false)),
            ],
        )];
        let rep = aggregate(&r);
        let fields: Vec<&str> =
            rep.numeric_breakage.collapse.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(fields, vec!["base/Res.fuel"], "只真经济崩盘算，灯光/初值0 不冤判: {fields:?}");
    }

    #[test]
    fn numeric_breakage_flags_non_finite() {
        let r = vec![labeled_numeric(
            StrategyKind::Economy,
            0,
            Outcome::Timeout,
            10,
            vec![],
            vec![("x/Y.ratio", nstat(1.0, f64::NAN, 1.0, 1.0, false, false, true))],
        )];
        let rep = aggregate(&r);
        assert_eq!(rep.numeric_breakage.non_finite.len(), 1);
        assert_eq!(rep.numeric_breakage.non_finite[0].field, "x/Y.ratio");
    }

    #[test]
    fn numeric_breakage_empty_when_healthy() {
        // Healthy field: small fluctuations, doesn't return to zero, no overflow → all three kinds
        // empty
        let r = vec![labeled_numeric(
            StrategyKind::Random,
            0,
            Outcome::Win,
            10,
            vec![],
            vec![("p/Stats.hp", nstat(100.0, 90.0, 80.0, 110.0, false, false, false))],
        )];
        let rep = aggregate(&r);
        assert!(rep.numeric_breakage.runaway.is_empty());
        assert!(rep.numeric_breakage.collapse.is_empty());
        assert!(rep.numeric_breakage.non_finite.is_empty());
    }

    // ---- Dominant action (dominant_action one-trick) unit tests ----

    #[test]
    fn dominant_action_flagged_when_one_action_dominates_wins() {
        // 4 winning sessions, each spamming "cheese" (9 times) + 1 other action → cheese accounts
        // for 90% > 80%
        let mut r = Vec::new();
        for seed in 0..4u64 {
            let mut injected = vec!["cheese"; 9];
            injected.push("other");
            r.push(labeled(StrategyKind::Random, seed, Outcome::Win, 20, vec![], vec![], injected));
        }
        let rep = aggregate(&r);
        let da = rep.dominant_strategy.dominant_action.expect("应标一招鲜");
        assert_eq!(da.action, "cheese");
        assert!(da.share >= 0.8, "占比 {}", da.share);
        assert_eq!(da.winning_sessions, 4);
    }

    #[test]
    fn dominant_action_none_when_balanced() {
        // Winning-session actions are balanced (a/b half each) → no one-trick
        let mut r = Vec::new();
        for seed in 0..4u64 {
            r.push(labeled(StrategyKind::Random, seed, Outcome::Win, 20, vec![], vec![],
                vec!["a", "b", "a", "b"]));
        }
        let rep = aggregate(&r);
        assert!(rep.dominant_strategy.dominant_action.is_none(), "均衡不算一招鲜");
    }

    #[test]
    fn dominant_action_ignores_non_winning_sessions() {
        // Timeout sessions spamming cheese don't count; only winning sessions are looked at — too
        // few winning sessions → None
        let r = vec![
            labeled(StrategyKind::Random, 0, Outcome::Timeout, 20, vec![], vec![], vec!["cheese"; 20]),
            labeled(StrategyKind::Random, 1, Outcome::Win, 5, vec![], vec![], vec!["a", "b"]),
        ];
        let rep = aggregate(&r);
        assert!(rep.dominant_strategy.dominant_action.is_none(), "通关局不足/不偏，不下论断");
    }

    // ---- LLM qualitative note (qualitative_notes) unit tests ----

    #[test]
    fn qualitative_notes_empty_when_no_llm() {
        // Pure cheap-strategy sessions have no notes → empty summary
        let r = vec![labeled(StrategyKind::Random, 0, Outcome::Win, 10, vec![], vec![], vec![])];
        let rep = aggregate(&r);
        assert_eq!(rep.qualitative_notes.total, 0);
        assert!(rep.qualitative_notes.clusters.is_empty());
    }

    #[test]
    fn qualitative_notes_groups_by_kind_and_dedups_text() {
        // The same contradiction sentence appears in two sessions → deduped into one entry
        // count=2; another different text is listed separately
        let r = vec![
            labeled_with_notes(StrategyKind::Scripted, 0, vec![
                note(3, "continuity", "管理员这句和上一幕矛盾"),
                note(5, "clarity", "看不懂该干嘛"),
            ]),
            labeled_with_notes(StrategyKind::Scripted, 1, vec![
                note(4, "continuity", "管理员这句和上一幕矛盾"),
            ]),
        ];
        let rep = aggregate(&r);
        assert_eq!(rep.qualitative_notes.total, 3, "原始 note 共 3 条");
        assert_eq!(rep.qualitative_notes.clusters.len(), 2, "去重后 2 条");
        let contradiction = rep
            .qualitative_notes
            .clusters
            .iter()
            .find(|c| c.text.contains("矛盾"))
            .expect("应有矛盾簇");
        assert_eq!(contradiction.kind, "continuity");
        assert_eq!(contradiction.count, 2, "同句矛盾跨局归一 count=2");
        assert_eq!(contradiction.sample_tick, 3, "代表 tick = 第一次见到");
    }

    #[test]
    fn qualitative_notes_same_text_different_kind_not_merged() {
        // Same text but different kind → not merged (dedupe key includes kind)
        let r = vec![labeled_with_notes(StrategyKind::Scripted, 0, vec![
            note(1, "clarity", "这步没意义"),
            note(2, "choice", "这步没意义"),
        ])];
        let rep = aggregate(&r);
        assert_eq!(rep.qualitative_notes.clusters.len(), 2, "kind 不同不归一");
    }

    #[test]
    fn qualitative_notes_summary_and_serialization() {
        let r = vec![labeled_with_notes(StrategyKind::Scripted, 0, vec![
            note(3, "continuity", "前后矛盾"),
        ])];
        let rep = aggregate(&r);
        assert!(rep.summary.contains("LLM 定性提示"), "summary 应提 LLM note 概况: {}", rep.summary);
        assert!(rep.summary.contains("待人复核"), "summary 应诚实标待复核");
        let json = serde_json::to_string(&rep).unwrap();
        assert!(json.contains("qualitative_notes"));
        assert!(json.contains("前后矛盾"));
    }

    #[test]
    fn qualitative_notes_is_deterministic_aggregation() {
        // The aggregation itself is a pure function: the same batch of notes produces the same
        // summary twice (the non-determinism of notes themselves doesn't affect this)
        let build = || {
            vec![labeled_with_notes(StrategyKind::Scripted, 0, vec![
                note(2, "clarity", "b"),
                note(1, "continuity", "a"),
            ])]
        };
        let a = aggregate(&build());
        let b = aggregate(&build());
        assert_eq!(
            serde_json::to_string(&a.qualitative_notes).unwrap(),
            serde_json::to_string(&b.qualitative_notes).unwrap()
        );
    }

    #[test]
    fn numeric_breakage_serializes_in_report() {
        let r = vec![labeled_numeric(
            StrategyKind::Economy, 0, Outcome::Timeout, 100, vec![],
            vec![("t/R.gold", nstat(1.0, 5e6, 1.0, 5e6, true, false, false))],
        )];
        let rep = aggregate(&r);
        let json = serde_json::to_string(&rep).unwrap();
        assert!(json.contains("numeric_breakage"));
        assert!(json.contains("runaway"));
        assert!(rep.summary.contains("跑飞"), "summary 应提跑飞: {}", rep.summary);
    }

    // ---- Stage 6: representative recording externalization (no longer inlined into report JSON) ----

    /// Build a stuck session with a trailing freeze ≥K (so the report has a stuck representative
    /// recording).
    fn stuck_labeled(kind: StrategyKind, seed: u64) -> LabeledResult {
        let mut trace = vec![1u64, 2, 3];
        trace.extend(std::iter::repeat_n(42u64, 70));
        let n = trace.len() as u64;
        labeled(kind, seed, Outcome::Timeout, n, trace, vec![], vec!["left", "left"])
    }

    #[test]
    fn report_json_does_not_inline_recordings() {
        // The report body JSON shouldn't contain inlined recording fields (checkpoints/inputs etc.,
        // those big chunks)
        let rep = aggregate(&[stuck_labeled(StrategyKind::Random, 0)]);
        assert_eq!(rep.stuck_clusters.len(), 1, "应有一簇卡死");
        let json = serde_json::to_string(&rep).unwrap();
        assert!(!json.contains("checkpoints"), "报告 JSON 不该内联录像 checkpoints: {json}");
        // The representative only serializes path/ticks/outcome, no recording/key
        assert!(json.contains("representative"));
        assert!(json.contains("\"ticks\""));
        assert!(!json.contains("\"recording\""), "recording 字段必须 serde skip");
        assert!(!json.contains("\"key\""), "key 字段必须 serde skip");
    }

    #[test]
    fn representative_path_is_none_before_externalize() {
        let rep = aggregate(&[stuck_labeled(StrategyKind::Random, 0)]);
        assert!(rep.stuck_clusters[0].representative.path.is_none(), "落盘前 path=None");
        // But in memory the recording and ticks/outcome are still there (for replay / writing to
        // file)
        assert!(rep.stuck_clusters[0].representative.ticks > 0);
        assert_eq!(rep.stuck_clusters[0].representative.outcome, Outcome::Timeout);
    }

    #[test]
    fn externalize_writes_files_and_fills_paths() {
        let mut rep = aggregate(&[stuck_labeled(StrategyKind::Random, 7)]);
        // Write to a temp directory (cleaned up after the test, doesn't pollute the repo)
        let dir = std::env::temp_dir()
            .join(format!("vitric-playtest-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = rep.externalize_recordings(&dir).unwrap();
        assert_eq!(written, 1, "应写出 1 个代表录像文件");
        let r = &rep.stuck_clusters[0].representative;
        let path = r.path.as_ref().expect("落盘后 path 应被回填");
        assert!(path.ends_with(".json"), "路径是 json 文件: {path}");
        // The file actually exists and can be parsed back into a recording
        let full = dir.join(path);
        assert!(full.exists(), "代表录像文件应真写出: {}", full.display());
        let text = std::fs::read_to_string(&full).unwrap();
        let rec: vitric_sim::Recording = serde_json::from_str(&text).unwrap();
        assert_eq!(rec.ticks, r.ticks, "落盘录像与引用 ticks 一致");
        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn externalize_key_is_deterministic_same_input() {
        // Aggregating the same input twice produces the same representative recording key
        // (deterministically reproducible filename)
        let a = aggregate(&[stuck_labeled(StrategyKind::Random, 3)]);
        let b = aggregate(&[stuck_labeled(StrategyKind::Random, 3)]);
        assert_eq!(
            a.stuck_clusters[0].representative.key,
            b.stuck_clusters[0].representative.key,
            "代表录像文件名 key 必须确定可复现"
        );
    }

    #[test]
    fn externalize_no_op_when_no_representatives() {
        // When there are no representative-recording dimensions, externalize writes 0 files and
        // doesn't create the directory
        let rep_results =
            vec![labeled(StrategyKind::Random, 0, Outcome::Win, 5, vec![], vec![], vec![])];
        let mut rep = aggregate(&rep_results);
        assert!(rep.stuck_clusters.is_empty());
        let dir = std::env::temp_dir()
            .join(format!("vitric-playtest-noop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = rep.externalize_recordings(&dir).unwrap();
        assert_eq!(written, 0);
        assert!(!dir.exists(), "无录像时不该建目录");
    }
}
