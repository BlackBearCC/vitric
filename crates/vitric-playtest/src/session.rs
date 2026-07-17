//! Single-session playtest: derive Scene View → strategy picks an action → inject → step, until clear/death/timeout,
//! recording throughout. One session = one replayable, verifiable deterministic recording (design draft section 4).
//!
//! **Interface orientation note**: the design draft says `run_session(project_dir, ...)`, but the runtime
//! assembly `Runtime::boot` lives in vitric-cli (cli depends on playtest; playtest cannot depend back on cli, or it'd form a cycle).
//! So boot is left to the caller (CLI's cmd_playtest); run_session takes an already-booted
//! `(Sim, GameLogic, Engine)` — purer responsibility: playtest only does "feed the view, run the loop, produce a recording",
//! it doesn't know how the project directory is assembled. Engine is passed separately because deriving the action vocabulary
//! needs to read the rules, and it's privately held by the GameLogic assembly, so we can't borrow it.

use std::collections::BTreeMap;

use vitric_sim::{GameLogic, InputRecord, Recording, ReplyRecord, Sim, TICKS_PER_SECOND};
use vitric_rules::Engine;
use serde_json::Value;

use crate::scene_view::{Outcome, SceneView, TerminalSpec};
use crate::strategy::{PlaytestNote, Strategy};

/// A numeric field's trajectory summary across a whole session (**incremental stats**, doesn't store per-tick full history).
///
/// key (in `numeric_summary`'s BTreeMap) = the path of a numeric leaf, shaped like
/// `hero/Resources.gold` (entity name or id + "component.field..."). Each tick reads all numeric leaves
/// from the current observation and does an O(1) update on their NumericStat — no per-tick history retained.
///
/// Why this design: finding economy breakage in simulation-management (design draft section 5 "numeric breakage") relies on
/// "how large did this field end up / did it hit zero / does it only-ever-grow" these **summary** signals, not on per-tick curves.
/// Incremental stats compress memory to O(number of numeric fields) rather than O(fields × ticks), surviving thousands of sessions
/// (design draft section 9 performance budget).
///
/// **Doesn't enter the hash or recording**: like state_trace/fired_events, it's a bystander record of "how this session ran",
/// a pure-function derivative of the recording (same recording ⇒ same summary), so it naturally satisfies the determinism rule.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NumericStat {
    /// This field's value the first time it was observed (first-frame baseline).
    pub first: f64,
    /// The last observed value (final frame — "how big did it get / how small did it collapse" reads this).
    pub last: f64,
    /// The minimum value observed across the session.
    pub min: f64,
    /// The maximum value observed across the session.
    pub max: f64,
    /// **Only-ever-grew** across the session (each new observation ≥ the previous one) — a signature of economy runaway.
    pub monotonic_up: bool,
    /// Hit zero at some point in the session (resource drained to zero — a signature of collapse soft-lock).
    pub hit_zero: bool,
    /// A non-finite value (inf / nan) was observed at some point — a hard signal of numeric overflow / divide-by-zero, flagged separately.
    pub non_finite: bool,
}

impl NumericStat {
    /// Initialize with the first observed value.
    fn start(v: f64) -> NumericStat {
        let finite = v.is_finite();
        NumericStat {
            first: v,
            last: v,
            min: v,
            max: v,
            monotonic_up: true,
            hit_zero: finite && v == 0.0,
            non_finite: !finite,
        }
    }

    /// Incrementally fold in a new observation (O(1)): refresh last/min/max, maintain monotonic/zero/non-finite flags.
    fn observe(&mut self, v: f64) {
        if !v.is_finite() {
            // non-finite values are flagged separately; don't pollute min/max (NaN comparisons are all false, which would break monotonic detection)
            self.non_finite = true;
            self.last = v;
            return;
        }
        // monotonic: as soon as some observation is smaller than the previous, it's no longer "only-ever-grows"
        if v < self.last {
            self.monotonic_up = false;
        }
        self.last = v;
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        if v == 0.0 {
            self.hit_zero = true;
        }
    }
}

/// Extract all numeric leaves from an observation (the SceneView projection JSON) and fold them into summary (incrementally).
///
/// key path: `<entity name or id>/<component>.<field>[.<subfield>...]`. Iterates observation.entities,
/// takes each entity's name (or falls back to id for unnamed), then drills its components tree, and on encountering a number
/// (bool folded to 0/1? No — only true numbers are collected; bool is a flag, not a number, doesn't enter numeric-breakage analysis)
/// updates the corresponding NumericStat.
///
/// **Determinism**: observation's entities are in slot order, components/fields are serde_json Maps (BTreeMap order),
/// so iteration order is fixed; the BTreeMap aggregation output is also fixed. O(number of numeric leaves)/tick, no history stored (design draft section 9).
fn collect_numeric_leaves(observation: &Value, summary: &mut BTreeMap<String, NumericStat>) {
    let Some(entities) = observation.get("entities").and_then(|v| v.as_array()) else {
        return;
    };
    for ent in entities {
        // entity identifier: prefer human-readable name, fall back to id for unnamed (scene_view guarantees id is always present)
        let label = ent
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| ent.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()));
        let Some(label) = label else { continue };
        let Some(comps) = ent.get("components").and_then(|v| v.as_object()) else {
            continue;
        };
        for (cname, cval) in comps {
            // path prefix: entity/component, field name appended in recursion
            let prefix = format!("{label}/{cname}");
            walk_numeric(&prefix, cval, summary);
        }
    }
}

/// Recursively drill a component value, folding numeric leaves into summary. path is the path prefix "up to this layer".
fn walk_numeric(path: &str, value: &Value, summary: &mut BTreeMap<String, NumericStat>) {
    match value {
        Value::Number(n) => {
            // only collect true numbers; integers also convert to f64 (numeric breakage looks at magnitude, i64/f64 unified onto one scale)
            if let Some(f) = n.as_f64() {
                upsert(summary, path, f);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                walk_numeric(&child, v, summary);
            }
        }
        Value::Array(arr) => {
            // arrays are path-indexed by subscript (e.g. inventory.0 / inventory.1), recursing with the same rules
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{path}.{i}");
                walk_numeric(&child, v, summary);
            }
        }
        // bool/string/null aren't numbers, skip (bool is a flag, doesn't enter numeric-breakage analysis)
        _ => {}
    }
}

/// Incrementally fold in if present, initialize with first value if absent (the upsert of incremental stats).
fn upsert(summary: &mut BTreeMap<String, NumericStat>, key: &str, v: f64) {
    summary
        .entry(key.to_string())
        .and_modify(|s| s.observe(v))
        .or_insert_with(|| NumericStat::start(v));
}

/// Configuration for a single session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Run this many ticks; if no termination by then, classify as Timeout.
    pub max_ticks: u64,
    /// Seed for the strategy's PCG (same seed + same strategy + same start = same session).
    pub seed: u64,
    /// Which events count as termination.
    pub terminal: TerminalSpec,
    /// Per-game view overrides (design draft section 1 "auto-derive + optional overrides"). Default=empty config (auto-derive, behavior unchanged).
    /// session uses this to drive `SceneView::derive_with_config` — strategy/LLM sees the include/exclude/relabel/
    /// derived-quantity-adjusted view (greedy in particular uses derived quantities to find the goal). **Still pure projection**, doesn't enter hash/recording.
    pub playtest: crate::config::PlaytestConfig,
    /// External replies (LLM content etc.) from the seed recording to be replayed by tick. Default empty=this session has no external replies (behavior unchanged).
    /// Seed-based exploration only: the strategy only reproduces/perturbs **inputs**, but endings reachable only via replies need the replies injected back at their original ticks
    /// — same semantics as `Sim::replay` (inputs first, then replies, then step), otherwise the baseline can't reproduce reply-gated endings.
    /// On truncate+divergence, the caller only passes replies before the truncation point (after truncation it's random divergence, no seed replies).
    pub seed_replies: Vec<ReplyRecord>,
}

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        SessionConfig {
            max_ticks: 600,
            seed: 0,
            terminal: TerminalSpec::default(),
            playtest: crate::config::PlaytestConfig::default(),
            seed_replies: Vec::new(),
        }
    }
}

/// A session's result: outcome + how many ticks it took + this session's recording (replayable) + lightweight telemetry.
///
/// Telemetry (state_trace/fired_events) is only for aggregator analysis, **doesn't enter the recording or hash** —
/// it's a bystander record of "how this session ran", not the authoritative state of "what this session is". Determinism rule:
/// the same (strategy, seed, start) produces byte-identical recordings, and the telemetry is a pure-function derivative of the recording, so it's naturally identical too.
#[derive(Debug, Clone)]
pub struct SessionResult {
    pub outcome: Outcome,
    pub ticks: u64,
    pub recording: Recording,
    /// The world state hash after each tick's step (`world.state_hash()`, optimized and cheap).
    /// Length = the actual number of ticks run; the authoritative signal for "did the state change", soft-lock clustering uses it to detect freezing.
    pub state_trace: Vec<u64>,
    /// Event names that appeared across the session (deduped, in first-appearance order): StepReport.events + logic.drain_observed().
    /// Used to decide "which terminal/milestone events were triggered" / "which input actions provoked a rule response".
    pub fired_events: Vec<String>,
    /// Numeric telemetry: each numeric field path → its whole-session trajectory summary (incremental stats, no per-tick full history).
    /// Feeds the aggregator to catch economy runaway/collapse/overflow (design draft section 5 "numeric breakage"). Doesn't enter hash/recording.
    pub numeric_summary: BTreeMap<String, NumericStat>,
    /// LLM-tier qualitative notes (clarity/continuity/choice effectiveness, design draft section 5 "LLM qualitative note").
    /// Only the LLM strategy produces these (cheap tiers' drain_notes is empty by default). **Doesn't enter hash/recording** —
    /// it's a bystander subjective hint of "how this LLM session looked", on the same level as other telemetry, doesn't affect determinism.
    pub notes: Vec<PlaytestNote>,
}

/// Run one session. `sim`/`logic` must be a fresh pair just booted, still at tick 0 (to record a replayable recording,
/// you must start recording from a cold boot — same constraint as vitric run's --record). `engine` is used to derive the action vocabulary.
///
/// Loop (per tick): derive Scene View → if done, break out → strategy picks an action → inject_input →
/// step → scan this tick's events for a terminal hit, record outcome and break → at max_ticks, record Timeout.
pub fn run_session(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    strategy: &mut dyn Strategy,
    cfg: &SessionConfig,
) -> Result<SessionResult, String> {
    if sim.is_recording() {
        return Err("run_session 要自己开录：传进来的 sim 不该已在录像中".to_string());
    }
    sim.start_recording();

    // telemetry accumulators: state_trace gets one entry per tick, fired_events deduped (set for dedup + vec for order preservation),
    // numeric_summary uses incremental stats (each tick reads numeric leaves from observation and updates, no history stored)
    let mut state_trace: Vec<u64> = Vec::new();
    let mut numeric_summary: BTreeMap<String, NumericStat> = BTreeMap::new();
    let mut fired_events: Vec<String> = Vec::new();
    let mut seen_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut note_events = |names: &mut dyn Iterator<Item = &str>| {
        for name in names {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
    };

    // LLM-tier qualitative note accumulator (collected from the strategy after each tick's decision; non-LLM strategies always drain empty).
    // Doesn't enter hash/recording — it's a bystander subjective hint, the determinism rule doesn't constrain it (LLM sessions aren't required to be reproducible across runs).
    let mut notes: Vec<PlaytestNote> = Vec::new();

    let mut outcome = Outcome::Timeout;
    while sim.tick < cfg.max_ticks {
        // Scene View is a pure projection: read-only on world/rules, never modifies world, doesn't enter hash.
        // config-aware derivation: the strategy sees the include/exclude/relabel/derived-quantity-adjusted view (greedy uses derived quantities to find the goal).
        let view = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        if let Some(done) = view.done {
            // in theory done is decided by scanning events after step (see SceneView::derive doc),
            // this branch is kept in case later stages let derive also detect static termination.
            outcome = done;
            break;
        }

        // strategy picks an action → inject (None=do nothing this tick)
        if let Some(action) = strategy.choose(&view) {
            sim.inject_input(&action.action, &action.phase);
        }
        // seed replies: same semantics as Sim::replay — inject this tick's seed replies after inputs (fixed order for byte-identical reproduction).
        // the non-seed path has empty seed_replies, this layer is a zero-cost no-op, behavior unchanged.
        for reply in &cfg.seed_replies {
            if reply.tick == sim.tick {
                sim.inject_reply(&reply.name, reply.data.clone());
            }
        }
        // collect notes after the decision: the LLM strategy may have produced a note in choose, drained and folded in each tick
        // (draining clears, to avoid duplicate collection). non-LLM strategies drain empty by default, zero cost.
        notes.append(&mut strategy.drain_notes());

        let report = sim.step(logic).map_err(|e| e.to_string())?;

        // telemetry (sampled after step): state_hash is the state fingerprint (optimized, cheap, doesn't serialize the whole world itself),
        // event names folded into the dedup set. Telemetry is read-only, doesn't write back to world, doesn't affect recording/hash.
        state_trace.push(sim.world.state_hash());
        // numeric telemetry: after step, project an observation of the current world, extract numeric leaves and incrementally fold into the summary.
        // uses the same config-aware projection (decoration-stripped, slot order, with derived quantities) so key paths match what the strategy saw;
        // only takes observation (not actions), incremental update O(number of numeric leaves), no per-tick history stored.
        let post = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        collect_numeric_leaves(&post.observation, &mut numeric_summary);
        note_events(&mut report.events.iter().map(|e| e.name.as_str()));

        // scan this tick's events sent to the logic layer + events emitted by the logic layer; on terminal hit, record outcome
        if let Some(o) = scan_terminal(report.events.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
        let emitted = logic.drain_observed();
        note_events(&mut emitted.iter().map(|e| e.name.as_str()));
        if let Some(o) = scan_terminal(emitted.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
    }

    // after exiting the loop (done/Timeout) collect notes one more time: the LLM may have emitted an undrained note
    // on its final decision tick (e.g. "the session gets stuck here, no idea what's next").
    notes.append(&mut strategy.drain_notes());

    let recording = sim.stop_recording().expect("刚 start_recording 过");
    Ok(SessionResult {
        outcome,
        ticks: sim.tick,
        recording,
        state_trace,
        fired_events,
        numeric_summary,
        notes,
    })
}

/// Lookahead planner config (design draft: the "smart but slow" tier for skill-based games).
///
/// This is a **depth-D, beam-W directed beam-search** rolling planner (MPC). Compared to the old one-step lookahead (1-ply:
/// each real tick only picks "one action for this instant", then rolls horizon frames scoring), beam search builds a search tree
/// between snapshot/restore: each node is a `Sim::snapshot`, expanding a node = for each candidate action restore
/// back to that node, inject, `step` **one frame**, then snapshot into a child node. This way "press up first then press right"
/// "after hitting a wall, re-press right to climb up" these **multi-tick combinations / continuous maneuvers** naturally emerge
/// from the optimal path — because each layer of the search re-picks an action, the optimal sequence is "right-right-right" or "up-right",
/// rather than one-step lookahead which can only roll the same action.
///
/// This is unique to Vitric: `Sim::snapshot`/`Sim::restore` can precisely save/restore the full state (world+rng+tick+
/// logic state + undigested inputs/replies); other engines are non-deterministic and can't precisely roll back and retry,
/// so they can't do this kind of per-layer speculative search.
///
/// **Degradation relation**: at `depth=1` this tree has only one layer of expansion from the root (one frame per candidate, pick the best),
/// equivalent to the old 1-ply one-step lookahead — kept as a degraded tier. The `horizon` field has been merged into `depth`'s semantics
/// ("how many frames to look ahead" = search depth); CLI's `--horizon` maps directly to `depth` (see cmd_playtest).
#[derive(Debug, Clone)]
pub struct LookaheadConfig {
    /// Search depth D: how many layers to expand from the root downward = how many frames to plan ahead. Larger = "more far-sighted" and slower.
    /// `depth=1` degrades to one-step lookahead (1-ply). Default 8.
    pub depth: u64,
    /// Beam width W: at each layer keep only the top-W highest-scoring nodes to expand further (beam pruning, to avoid B^D explosion).
    /// Larger = less likely to miss "needs to get worse before it gets better" maneuvers due to greedy pruning, but slower. Default 4.
    pub beam_width: usize,
}

impl Default for LookaheadConfig {
    fn default() -> LookaheadConfig {
        // depth=8 / beam=4: navigation/skill games usually have single-digit candidates B; the speculative-step upper bound per real tick ≈ W×B×D
        // (see run_session_lookahead's perf comment); default values pick "enough to solve multi-tick combos but not too slow".
        LookaheadConfig { depth: 8, beam_width: 4 }
    }
}

/// A search node's score (larger is better). Beam search uses this to score each expanded child node: first by terminal
/// signal (Win > neutral > Lose), then by earliness of reaching Win, then by the goal-derived quantity (with goal) or new-state exploration
/// (without goal). Carried by a totally-orderable struct — beam pruning sorts by it to take the top W, and the final best-leaf selection also relies on it.
/// Deterministic tie-breaking is left to the caller (the node carries "root first-action index + expansion order"; on ties take the earlier one).
#[derive(Debug, Clone, Copy, PartialEq)]
struct NodeScore {
    /// Terminal signal: +1=this path has reached Win, 0=no termination, -1=has reached Lose (Win has the highest priority).
    terminal: i8,
    /// Earliness of reaching Win: earlier is better. Expressed as "upper-bound depth - the depth of the hit layer" (0 if not hit); larger is better.
    /// Makes a "win in 3 steps" path beat an "win in 8 steps" one; the planner prefers the shortest clear.
    win_earliness: i64,
    /// Goal score (with goal): direction=min takes -distance, max takes value (larger is better); always 0 without goal.
    /// This is the node's **current state's** goal-derived quantity (not the rollout end state) — beam search recomputes each layer, climbing toward the goal.
    goal_score: f64,
    /// Exploration score (weak progress signal without goal): how many new state_hashes (change count) were traversed on the path from root to this node.
    /// With goal, this is only a secondary tiebreaker.
    explore: u64,
}

impl NodeScore {
    /// Total order comparison: terminal > win_earliness > goal_score > explore (item by item, only look at the next if the former is equal).
    /// Without goal, goal_score is always 0, naturally degrading to explore-dominant; NaN goal scores are treated as worst (shouldn't happen,
    /// upstream folds an unreachable derived quantity into a floor value, this is an extra safeguard against comparison panics).
    fn cmp(&self, other: &NodeScore) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.terminal
            .cmp(&other.terminal)
            .then(self.win_earliness.cmp(&other.win_earliness))
            .then(
                self.goal_score
                    .partial_cmp(&other.goal_score)
                    .unwrap_or(Ordering::Equal),
            )
            .then(self.explore.cmp(&other.explore))
    }
}

/// An **active node** in the beam-search tree: a snapshot + its score + the index of the first action it took at the root layer.
///
/// `root_action` is the candidate index chosen at the root layer when this node's ancestor chain is traced back to the root (`view.actions[i]`,
/// or = candidate count n meaning "do nothing") — MPC only executes this first step, so every node must carry it all the way down.
/// `terminated` marks that this path reached a terminal (Win/Lose) at some layer, no further expansion (pruned).
/// `explore` is the cumulative new-state count from root to here (progress signal without goal), accumulating as expansion goes down.
///
/// **`best_goal` is the key design**: a node's goal score uses "the **best** goal value seen on the path from root to here",
/// not "the current state's goal value". Why — the optimal path for skill games is often **non-monotonic** (must go up first to go right: when jumping,
/// the Manhattan distance to the exit actually grows, and only drops sharply when landing on the wall's other side). If scored by current state,
/// beam pruning would prune away the "currently getting over the wall, temporarily worse" branch, and beam search would degrade to greedy and get stuck
/// before the wall. Scoring by "best on path" = optimistic estimate: a path is credited as long as it touched a closer-to-exit position at **any frame**,
/// so the beam keeps this "worse-then-better" wall-overcoming path; once it lands on the wall's other side best_goal jumps up, and the planner picks
/// the root-layer first step as "that jump" accordingly. This is exactly the capability that 1-ply can't do and that needs planning depth.
/// Without goal, best_goal is always 0, degrading to explore-dominant.
#[derive(Debug, Clone)]
struct BeamNode {
    /// This node's full sim-state snapshot (world+rng+tick+logic state+undigested inputs/replies).
    snapshot: Value,
    /// Score (used for beam-pruning sort / final best selection). Its goal_score = best goal value on path (see struct doc).
    score: NodeScore,
    /// The index of the first action this node took at the root layer (0..n=view.actions[i], n=do nothing).
    root_action: usize,
    /// Whether this path has reached a terminal (terminated nodes aren't expanded, kept as best candidates).
    terminated: bool,
    /// How many new state_hashes were traversed from root to here (no-goal exploration signal, accumulates downward).
    explore: u64,
    /// **Best goal value seen on the path from root to here** (larger is better; min-goal takes -distance, max takes value).
    /// When expanding downward, take max with the child node's current-state goal value and propagate — monotonically non-decreasing,
    /// recording "this path got geometrically closest to the exit at some point".
    best_goal: f64,
}

/// Run one session of the **directed beam-search rolling planner** (design draft: let skill-based games — platformer/navigation/puzzle — be
/// played by swarm rather than misreported as unbeatable by random strategy). Same replayable recording output as [`run_session`], same telemetry,
/// the only difference is "how to pick an action each real tick": replace the old one-step lookahead with **depth-D, beam-W beam search**.
///
/// Each real tick (rolling horizon / MPC: re-plan every real tick, only execute the first step):
///
/// - **Root**: `root = sim.snapshot(logic)` saves the current full state as the root of the search tree.
/// - **Build tree**: expand layer by layer from the root. Expanding a node = for each candidate action (each of `view.actions` + a trailing
///   "do nothing") `restore` back to that node, inject, `sim.step` **one frame**, then `snapshot` into a child node; the child node records
///   "its first action at the root layer" + a score computed from the goal-derived quantity (a Win hit gets the top score and prunes; a Lose hit gets the bottom score and prunes).
/// - **Beam pruning**: at each layer sort the expanded children by score, keep only the best few for further expansion (beam search, to avoid
///   B^D explosion; root-action diversity preserved via [`prune_beam_diverse`]); terminated nodes aren't expanded, kept as best candidates.
/// - **Pick first step**: after expanding to depth **D** (or all beam nodes terminated), select the best-scoring node among all explored,
///   take its `root_action` — that's the action to execute this real tick.
/// - **Execute**: really inject the chosen action, normal `sim.step` advances one real tick (**only this step enters the real recording**).
///
/// "Press up first then press right" "after hitting a wall, re-press right to climb up" these **multi-tick combinations / continuous maneuvers**
/// naturally emerge from the optimal path: each search layer re-picks an action, the optimal sequence is "up-right" or "right-right-right",
/// and the root layer's first action is exactly the first step of that maneuver. `depth=1` degrades to one-step lookahead (only one root layer,
/// pick the best frame), kept as a degraded tier.
///
/// **Determinism rule**: node expansion order (candidates ascending by index), pruning sort (by `NodeScore::cmp`, ties take
/// "smaller root-action index, then earlier expansion"), and the final best-selection tiebreak are all deterministic ⇒ same (project, seed, depth,
/// beam, start) yields the same decision sequence. Speculative steps roll back precisely between snapshot/restore throughout and **never enter the real recording**.
/// `restore` clears sim's in-flight recorder — and every real tick needs to restore for speculation, so **can't use
/// `sim.start_recording`/`stop_recording`** (the 2nd real tick's restore would wipe the 1st tick's recording).
/// Instead we **manually accumulate a `Recording`**: record input/reply by "the sim.tick at the moment of injection",
/// record a checkpoint every 60 real ticks (same cadence as sim's internal CHECKPOINT_INTERVAL), record one at the start too
/// `(0, initial hash)`, and fill final_hash/ticks at the end. The semantics are identical to `Sim::step`'s internal recording, so this
/// hand-built recording is still byte-reproducible via `Sim::replay`. Only real injected actions are recorded.
///
/// **Performance upper bound**: per real tick, the number of speculative `sim.step` calls ≤ **W × (B+1) × D** (B=candidate count, +1 is
/// "do nothing", W=beam width, D=depth; the first layer has only 1 root, after that each layer has at most W nodes each expanding B+1 children) —
/// **linear in depth** (beam search compresses the (B+1)^D exponential explosion into linear). Navigation/skill games have single-digit B; it's an opt-in
/// "smart but slow" tier. The default swarm-mixed lookahead uses a small depth (see swarm.rs `DEFAULT_SWARM_LOOKAHEAD_DEPTH`).
///
/// **Goal-quantity source**: `cfg.playtest.goal` (derived quantity + min/max). **No-goal degradation**: node score first looks at
/// "whether Win was reached earlier"; without Win, only the weak progress signal "how many new states were traversed to reach this node" remains —
/// without a goal quantity there's no way to decide "which direction is closer to the end", inherently much weaker than with goal.
pub fn run_session_lookahead(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    cfg: &SessionConfig,
    look: &LookaheadConfig,
) -> Result<SessionResult, String> {
    if sim.is_recording() {
        return Err("run_session_lookahead 要自己开录：传进来的 sim 不该已在录像中".to_string());
    }
    if look.depth == 0 {
        return Err("lookahead depth 必须 ≥ 1".to_string());
    }
    if look.beam_width == 0 {
        return Err("lookahead beam_width 必须 ≥ 1".to_string());
    }
    // Speculative search (expand_one) doesn't replay seed_replies. If the seed recording carries external replies and the speculative path doesn't inject them,
    // the search tree is built on the wrong future where "that reply never happened", and the chosen action plans against the wrong world (doesn't affect replay
    // safety — real stepping injects as usual and the recording still matches — but plan quality is wrong). No caller currently passes seed_replies to lookahead
    // (seed exploration goes through run_session, not lookahead), so this hard-errors to turn the silent landmine into a loud one: anyone wanting to seed
    // lookahead replies must first inject them per-tick inside expand_one's speculative step, rather than silently running a wrong plan.
    if !cfg.seed_replies.is_empty() {
        return Err("run_session_lookahead 暂不支持 seed_replies：投机搜索不会重放它们，\
             规划会建在错误的世界上。要给前瞻接种子回复，先在 expand_one 的投机步里按 tick 注入"
            .to_string());
    }

    // telemetry accumulators (same semantics as run_session)
    let mut state_trace: Vec<u64> = Vec::new();
    let mut numeric_summary: BTreeMap<String, NumericStat> = BTreeMap::new();
    let mut fired_events: Vec<String> = Vec::new();
    let mut seen_events: std::collections::HashSet<String> = std::collections::HashSet::new();

    // ---- Hand-assemble the recording (don't use sim's recorder, because every real tick's restore would wipe it) ----
    // Accounting must match Sim::step / Sim::start_recording exactly, so replay can verify it (see the function doc).
    let mut recording = Recording {
        seed: sim.seed(),
        checkpoints: vec![(sim.tick, sim.world.state_hash())],
        ..Default::default()
    };

    let mut outcome = Outcome::Timeout;

    while sim.tick < cfg.max_ticks {
        let view = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        if let Some(done) = view.done {
            outcome = done;
            break;
        }

        // ---- beam-search planning (entirely between snapshot/restore, doesn't enter the manual recording) ----
        // root = snapshot of the current real state. Speculation stays entirely between it and subsequent restores, never polluting the real trajectory.
        let root = sim.snapshot(logic);
        let best_root_action =
            plan_beam(sim, logic, engine, cfg, look, &view, &root)?;
        // back to the real state: speculation done, what follows is the real tick.
        sim.restore(&root, logic)?;

        let n = view.actions.len();
        // really inject the chosen action (do nothing=no inject), manually record it (tick = sim.tick at the moment of injection).
        let inject_tick = sim.tick;
        if best_root_action < n {
            let a = &view.actions[best_root_action];
            sim.inject_input(&a.action, &a.phase);
            recording.inputs.push(InputRecord {
                tick: inject_tick,
                action: a.action.clone(),
                phase: a.phase.clone(),
            });
        }
        // seed replies (same semantics as run_session; lookahead usually doesn't carry them, kept for channel consistency). Also manually recorded.
        for reply in &cfg.seed_replies {
            if reply.tick == inject_tick {
                sim.inject_reply(&reply.name, reply.data.clone());
                recording.replies.push(ReplyRecord {
                    tick: inject_tick,
                    name: reply.name.clone(),
                    data: reply.data.clone(),
                });
            }
        }
        let report = sim.step(logic).map_err(|e| e.to_string())?;

        // periodic checkpoint: same semantics as Sim::step (after step, if tick is a multiple of 60, record one).
        if sim.tick.is_multiple_of(TICKS_PER_SECOND) {
            recording.checkpoints.push((sim.tick, sim.world.state_hash()));
        }

        // telemetry (sampled after the real step)
        state_trace.push(sim.world.state_hash());
        let post = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        collect_numeric_leaves(&post.observation, &mut numeric_summary);
        for name in report.events.iter().map(|e| e.name.as_str()) {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
        if let Some(o) = scan_terminal(report.events.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
        let emitted = logic.drain_observed();
        for name in emitted.iter().map(|e| e.name.as_str()) {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
        if let Some(o) = scan_terminal(emitted.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
    }

    // Wrap up: fill the end-state with the same accounting as Sim::stop_recording.
    recording.ticks = sim.tick;
    recording.final_hash = sim.world.state_hash();
    Ok(SessionResult {
        outcome,
        ticks: sim.tick,
        recording,
        state_trace,
        fired_events,
        numeric_summary,
        // Lookahead is a cheap (non-LLM) strategy tier: produces no qualitative notes
        notes: Vec::new(),
    })
}

/// Run one round of **depth-D, beam-W beam search** on the given root state (`root` snapshot), returning the index of the
/// first action to take at the root layer (0..n=`view.actions[i]`, n=do nothing). MPC uses only this step.
///
/// Calling convention: on entry sim is at some real state, `root` is its snapshot; this function speculates entirely between snapshot/restore
/// (on exit sim is left at some speculative end state, the caller is responsible for `restore(root)` afterward to return to the real state).
///
/// Tree expansion (deterministic order):
/// - Layer 0: the root is the only active node; expanding it = for each candidate action (ascending index, trailing "do nothing") restore to root,
///   inject, step one frame, snapshot into a child node; each root-layer candidate fixes its own `root_action` (= that candidate's index).
/// - Layer k (k≥1): expand each active node in the previous layer's beam the same way (children inherit the parent's `root_action`).
/// - After each layer's expansion, sort all new children by `NodeScore::cmp` descending (ties take smaller root-action index, then earlier expansion),
///   take the top W as the next layer's beam. Terminated (Win/Lose hit) nodes aren't expanded, go directly into the "best candidate pool".
/// - Run D layers or until the beam empties (all terminated). Finally pick the best-scoring node from "best candidate pool + last-layer beam" and return its `root_action`.
fn plan_beam(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    cfg: &SessionConfig,
    look: &LookaheadConfig,
    view: &SceneView,
    root: &Value,
) -> Result<usize, String> {
    let n = view.actions.len(); // candidates = n actions + 1 "do nothing" (index n)
    // best candidate pool: collects "best along the way" nodes — all children after each layer's expansion participate in "pick the final best",
    // so even if no one clears at layer D, we can still pick "the intermediate node with the highest goal score"'s root action (climb toward the goal).
    // accumulate via a single best (replace only when strictly better + tiebreak conservatively takes the earlier); no need to keep all nodes.
    let mut best: Option<BeamNode> = None;
    // current layer's beam (active nodes, next layer expands from them). Initial = "root" as a virtual node (no action taken yet).
    // root snapshot + placeholder score/root_action for the root; root's root_action is never selected
    // (each candidate at root-layer expansion fixes the real root_action), here n is a placeholder.
    // root's best_goal starts with the root current-state goal value (child nodes' path-max then accumulates monotonically).
    // sim is at the root state right now (plan_beam hasn't restored elsewhere on entry), compute directly.
    let root_goal = node_goal_score(sim, engine, cfg);
    let mut beam: Vec<BeamNode> = vec![BeamNode {
        snapshot: root.clone(),
        score: NodeScore { terminal: 0, win_earliness: 0, goal_score: root_goal, explore: 0 },
        root_action: n,
        terminated: false,
        explore: 0,
        best_goal: root_goal,
    }];

    let depth = look.depth;
    for layer in 0..depth {
        // all children expanded at this layer (pending pruning).
        let mut children: Vec<BeamNode> = Vec::new();
        for node in &beam {
            // terminated nodes aren't expanded — but they themselves are legal best candidates (e.g. "this path wins in 3 steps").
            if node.terminated {
                consider_best(&mut best, node);
                continue;
            }
            for cand in 0..=n {
                // restore to this parent node, inject the candidate action (cand==n is do nothing), step one frame, snapshot into a child.
                sim.restore(&node.snapshot, logic)?;
                if cand < n {
                    let a = &view.actions[cand];
                    sim.inject_input(&a.action, &a.phase);
                }
                // root layer (layer==0) each candidate fixes its own root_action; deeper layers inherit the parent's.
                let root_action = if layer == 0 { cand } else { node.root_action };
                let ctx = ExpandCtx {
                    root_action,
                    parent_explore: node.explore,
                    parent_best_goal: node.best_goal,
                    layer,
                };
                let child = expand_one(sim, logic, engine, cfg, &ctx)?;
                consider_best(&mut best, &child);
                children.push(child);
            }
        }
        // beam pruning (preserving root-action diversity): see prune_beam_diverse for the algorithm/why.
        beam = prune_beam_diverse(children, look.beam_width);
        if beam.is_empty() {
            break; // Whole beam terminated (already in best), no need to expand further
        }
    }
    // Nodes in the last beam layer also participate in "picking the final best" (they are the "most promising after looking D layers" states).
    for node in &beam {
        consider_best(&mut best, node);
    }

    // best always has a value: the root layer expands at least 1 child (n≥0, candidates always include "do nothing").
    let best = best.ok_or("束搜索未展开出任何节点（不应发生：至少有「不操作」候选）")?;
    Ok(best.root_action)
}

/// Beam pruning (**diverse beam search**): pick the next layer's beam from all children expanded at this layer.
///
/// Not a naive "global top-W by score", but **first guarantee each root action keeps one best line, then fill up to W with global runners-up**:
/// - Pass 1: iterate by score descending; the first time each `root_action` appears, take it (= that root action's best successor line).
///   This guarantees a root action that **branches early and is currently worse** (jump) won't be crowded out by 4 "walk-right-into-wall"
///   same-root-action lines — otherwise the greedy heuristic (Manhattan distance) would prune all jump lines at layer 1, beam search degrades to greedy,
///   stuck forever before the wall (measured: naive top-W never jumps even at depth=40).
/// - Pass 2: if slots still aren't filled to W, fill in the remaining uncollected children by score descending (giving the most promising root actions a few more lines
///   to explore deeper).
///
/// This is the standard "diverse beam search" approach: bucket by root action to enforce diversity, preventing the best line from being drowned by homogeneous branches.
/// The cost upper bound becomes `≤ max(W, root-action count) × (B+1) × D` (root-action count = B+1, so ≈ (B+1)²×D, still linear in
/// depth). Navigation/skill games have single-digit B, manageable.
///
/// **Determinism**: the input `children` is generated by (parent order in the upper beam, candidate index ascending); this function first stable-sorts
/// (score descending, ties take smaller root-action index first, then preserve generation order), then buckets — same input yields same beam, fully deterministic.
fn prune_beam_diverse(mut children: Vec<BeamNode>, beam_width: usize) -> Vec<BeamNode> {
    if children.is_empty() {
        return children;
    }
    // stable sort: score descending; ties take smaller root-action index first; further ties preserve generation order (earlier expansion first).
    children.sort_by(|a, b| b.score.cmp(&a.score).then(a.root_action.cmp(&b.root_action)));

    let mut chosen: Vec<BeamNode> = Vec::with_capacity(beam_width.max(1));
    let mut seen_roots: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut taken = vec![false; children.len()];

    // pass 1: each root action keeps its best successor line (in sorted score order, take on first sight).
    for (i, c) in children.iter().enumerate() {
        if seen_roots.insert(c.root_action) {
            chosen.push(c.clone());
            taken[i] = true;
        }
    }
    // pass 2: if not filled to W, fill in global runners-up by score order (skip already-taken).
    if beam_width > chosen.len() {
        for (i, c) in children.iter().enumerate() {
            if chosen.len() >= beam_width {
                break;
            }
            if !taken[i] {
                chosen.push(c.clone());
                taken[i] = true;
            }
        }
    }
    chosen
}

/// Context inherited from the parent / determined by position when expanding a child node (packed into a struct, to keep expand_one's param list from exploding).
struct ExpandCtx {
    /// This child's first action index at the root layer (root layer=the candidate itself, deeper layers=inherited from parent).
    root_action: usize,
    /// Cumulative new-state count from root to parent (this frame adds 1 if it produces a new state_hash, then passes to child).
    parent_explore: u64,
    /// Best goal value seen on the path from root to parent (take max with this frame's current goal value, pass to child).
    parent_best_goal: f64,
    /// Which layer is being expanded (0-based): used to compute `win_earliness` on a Win hit (earlier = larger).
    layer: u64,
}

/// Starting from the "restored to parent node + candidate action injected" state, take **one frame** of `sim.step`, pack the result
/// into a child [`BeamNode`]: scan this frame's terminal events, compute the node score, snapshot into the child state.
/// Caller must have already restore + inject before calling; this function advances one frame (the caller is responsible for subsequent restore to other nodes).
fn expand_one(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    cfg: &SessionConfig,
    ctx: &ExpandCtx,
) -> Result<BeamNode, String> {
    let before = sim.world.state_hash();
    let report = sim.step(logic).map_err(|e| e.to_string())?;
    // terminal scan: step events + logic-emitted events (same semantics as run_session, Win takes priority over Lose).
    let mut hit: Option<Outcome> = scan_terminal(report.events.iter(), &cfg.terminal);
    let emitted = logic.drain_observed();
    if hit.is_none() {
        hit = scan_terminal(emitted.iter(), &cfg.terminal);
    }

    // weak exploration signal: if this frame walked to a new state_hash, add 1 to the parent's cumulative count.
    let after = sim.world.state_hash();
    let explore = ctx.parent_explore + if after != before { 1 } else { 0 };

    let mut terminal: i8 = 0;
    let mut win_earliness: i64 = 0;
    let mut terminated = false;
    match hit {
        Some(Outcome::Win) => {
            terminal = 1;
            terminated = true;
            // earlier hit (smaller layer) → larger win_earliness. The upper bound uses a large constant to keep it always positive and monotonic.
            // here layer is "the layer index at which Win was reached"; +1 makes a layer-0 hit also < full marks.
            win_earliness = (cfg_depth_bound(cfg) as i64).max(ctx.layer as i64 + 1) - ctx.layer as i64;
        }
        Some(Outcome::Lose) => {
            terminal = -1;
            terminated = true;
        }
        Some(Outcome::Timeout) | None => {}
    }

    // best goal value on path: max of parent's best and this frame's current-state goal value (monotonically non-decreasing). Use it as the node's goal_score
    // — the optimal path for skill games is non-monotonic (current distance temporarily grows while getting over a wall), scoring by "best seen on path" prevents
    // mis-pruning the mid-wall-overcoming branch (see BeamNode doc). Without goal, node_goal_score is always 0, best_goal also always 0.
    let cur_goal = node_goal_score(sim, engine, cfg);
    let best_goal = ctx.parent_best_goal.max(cur_goal);

    let score = NodeScore { terminal, win_earliness, goal_score: best_goal, explore };
    let snapshot = sim.snapshot(logic);
    Ok(BeamNode { snapshot, score, root_action: ctx.root_action, terminated, explore, best_goal })
}

/// The upper-bound baseline for `win_earliness`: a constant "at least as large as the deepest layer", so that an earlier Win hit scores higher.
/// Simple use of a large enough fixed upper bound (search depth is usually ≤ a few tens); as long as it's ≥ any possible layer.
fn cfg_depth_bound(_cfg: &SessionConfig) -> u64 {
    // 1<<20 is far larger than any realistic search depth — guarantees (bound - layer) is monotonically decreasing and always positive, so earlier wins score higher.
    1 << 20
}

/// Compute a node's current-state goal score (larger is better): with goal, take -distance / value per direction;
/// if the derived quantity can't be read, give the worst floor; without goal, always 0 (degrade to explore-dominant).
fn node_goal_score(sim: &Sim, engine: &Engine, cfg: &SessionConfig) -> f64 {
    match &cfg.playtest.goal {
        Some(g) => {
            let view = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
            let val = view
                .observation
                .get("derived")
                .and_then(|d| d.get(&g.quantity))
                .and_then(|v| v.as_f64());
            match val {
                Some(v) => match g.direction {
                    // min: smaller distance is better → take -distance (larger is better, consistent with NodeScore semantics)
                    crate::config::GoalDirection::Min => -v,
                    crate::config::GoalDirection::Max => v,
                },
                // Goal quantity unavailable (derived quantity is null etc.): give the worst floor so this node isn't selected
                None => f64::NEG_INFINITY,
            }
        }
        None => 0.0,
    }
}

/// Update the "global best" with a candidate node: replace only when strictly better; **tiebreak conservatively takes the earlier**
/// (smaller root-action index first; on further equality, keep the existing one = earlier expansion). Single point of the deterministic tiebreak rule.
fn consider_best(best: &mut Option<BeamNode>, node: &BeamNode) {
    let replace = match best {
        None => true,
        Some(b) => {
            use std::cmp::Ordering;
            match node.score.cmp(&b.score) {
                Ordering::Greater => true,
                Ordering::Less => false,
                // Score tie: smaller root-action index first (deterministic); if still equal, keep the existing one (first come first served)
                Ordering::Equal => node.root_action < b.root_action,
            }
        }
    };
    if replace {
        *best = Some(node.clone());
    }
}

/// The first outcome in a group of events that hits a terminal (in event order, deterministic).
fn scan_terminal<'a>(
    events: impl Iterator<Item = &'a vitric_rules::Event>,
    terminal: &TerminalSpec,
) -> Option<Outcome> {
    for e in events {
        if let Some(o) = terminal.classify(&e.name) {
            return Some(o);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::{Event, RuleSet};
    use vitric_sim::Pcg32;

    use crate::scene_view::Action;
    use crate::strategy::RandomStrategy;

    fn empty_engine() -> Engine {
        let schema = Schema::parse(&json!({"components": {}}), "s.json").unwrap();
        Engine::new(RuleSet::parse(&json!({"rules": []}), "r.json").unwrap(), schema)
    }

    /// Minimal logic that emits a terminal event at tick N (drain_observed hands it to the session scanner).
    struct EmitAt {
        at: u64,
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitAt {
        fn on_tick(
            &mut self,
            _world: &mut vitric_ecs::World,
            _events: Vec<Event>,
            _rng: &mut Pcg32,
            tick: u64,
        ) -> Result<(), String> {
            // step increments +1 only after on_tick, so this tick is the index of "the frame about to complete"
            if tick == self.at {
                self.pending.push(Event::new(&self.event, json!({})));
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    /// Logic that never terminates: used to verify Timeout.
    struct NeverEnds;
    impl GameLogic for NeverEnds {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn run_session_collects_win_on_terminal_event() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 3, event: "game-won".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 100, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Win);
        assert_eq!(res.ticks, 4, "tick 3 那帧发的 game-won，step 后 tick=4 时收到");
    }

    #[test]
    fn run_session_collects_lose_on_terminal_event() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 2, event: "game-over".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 100, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Lose);
    }

    #[test]
    fn lookahead_rejects_seed_replies() {
        // Lookahead speculative search doesn't replay seed_replies, would plan on a wrong future — so it hard-errors on non-empty input,
        // rather than silently producing a wrong plan (code review #1: turn a silent landmine into a loud one).
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig {
            max_ticks: 100,
            seed: 0,
            seed_replies: vec![ReplyRecord { tick: 1, name: "oracle".to_string(), data: json!({}) }],
            ..Default::default()
        };
        let err = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig::default())
            .unwrap_err();
        assert!(err.contains("seed_replies"), "应明确报 seed_replies 不支持：{err}");
    }

    #[test]
    fn run_session_times_out_and_records() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 50, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Timeout);
        assert_eq!(res.ticks, 50);
        assert_eq!(res.recording.ticks, 50);
        // Empty engine has no action vocabulary, the strategy can't pick an action → recording has no inputs, but checkpoints still exist
        assert!(!res.recording.checkpoints.is_empty());
    }

    #[test]
    fn run_session_is_deterministic_byte_for_byte() {
        let run = || {
            let mut sim = Sim::new(1);
            let mut logic = NeverEnds;
            let eng = action_engine();
            let mut strat = RandomStrategy::new(123);
            let cfg = SessionConfig { max_ticks: 80, seed: 123, ..Default::default() };
            run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap()
        };
        let a = run();
        let b = run();
        assert_eq!(a.outcome, b.outcome);
        assert_eq!(a.ticks, b.ticks);
        // Recording is byte-for-byte identical
        let ja = serde_json::to_string(&a.recording).unwrap();
        let jb = serde_json::to_string(&b.recording).unwrap();
        assert_eq!(ja, jb, "同 (策略,seed,起点) 两次跑录像必须逐字节一致");
    }

    /// Engine with an input vocabulary: lets the random strategy actually inject actions, so the recording has inputs.
    fn action_engine() -> Engine {
        let schema = Schema::parse(
            &json!({"components": {"Velocity": {"fields": {"x": {"type": "number"}}}}}),
            "s.json",
        )
        .unwrap();
        let rules = RuleSet::parse(
            &json!({"rules": [
                {"id": "left", "on": {"event": "input", "filter": {"action": "left", "phase": "pressed"}},
                 "do": [{"emit": "noop", "data": {}}]},
                {"id": "right", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
                 "do": [{"emit": "noop", "data": {}}]}
            ]}),
            "r.json",
        )
        .unwrap();
        Engine::new(rules, schema)
    }

    /// Fake strategy that emits one note per tick (doesn't touch LLM, only verifies session's note collection channel).
    struct NotingStrategy {
        tick: u64,
        pending: Vec<crate::strategy::PlaytestNote>,
    }
    impl crate::strategy::Strategy for NotingStrategy {
        fn choose(&mut self, _view: &SceneView) -> Option<Action> {
            // Stash one note per decision tick, simulating how the LLM strategy produces notes inside choose
            self.pending.push(crate::strategy::PlaytestNote {
                tick: self.tick,
                kind: "clarity".to_string(),
                text: format!("第 {} tick 看不懂", self.tick),
            });
            self.tick += 1;
            None
        }
        fn drain_notes(&mut self) -> Vec<crate::strategy::PlaytestNote> {
            std::mem::take(&mut self.pending)
        }
    }

    #[test]
    fn run_session_collects_notes_from_strategy() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = NotingStrategy { tick: 0, pending: vec![] };
        let cfg = SessionConfig { max_ticks: 5, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // Each of the 5 decision ticks produces one note; all are collected into notes by session
        assert_eq!(res.notes.len(), 5, "每 tick 一条 note 应全部收齐: {:?}", res.notes);
        assert_eq!(res.notes[0].tick, 0);
        assert_eq!(res.notes[4].tick, 4);
        assert!(res.notes[0].text.contains("看不懂"));
    }

    #[test]
    fn run_session_notes_empty_for_non_noting_strategy() {
        // Ordinary strategies (random) don't produce notes → notes is empty (the note channel is LLM-tier only)
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert!(res.notes.is_empty(), "非 LLM 策略不产 note");
    }

    #[test]
    fn run_session_collects_state_trace_one_per_tick() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 40, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // Sample one state_hash per tick run; the length must equal the actual tick count
        assert_eq!(res.state_trace.len(), res.ticks as usize);
        assert_eq!(res.state_trace.len(), 40);
    }

    /// Emits the same non-terminal event every tick — used to verify fired_events deduplication.
    struct EmitEveryTick {
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitEveryTick {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            self.pending.push(Event::new(&self.event, json!({})));
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    #[test]
    fn run_session_collects_fired_events_deduped() {
        let mut sim = Sim::new(1);
        // Emit a same-named event 20 times per tick; fired_events should collect it only once (dedup)
        let mut logic = EmitEveryTick { event: "milestone".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        // milestone is not a terminal event → won't stop, runs to the limit
        let cfg = SessionConfig { max_ticks: 20, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert!(res.fired_events.contains(&"milestone".to_string()), "{:?}", res.fired_events);
        // Dedup: emitted 20 times, but only one copy in fired_events
        assert_eq!(res.fired_events.iter().filter(|n| *n == "milestone").count(), 1);
    }

    #[test]
    fn run_session_with_actions_records_inputs() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = action_engine();
        let mut strat = RandomStrategy::new(5);
        let cfg = SessionConfig { max_ticks: 200, seed: 5, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Timeout);
        assert!(!res.recording.inputs.is_empty(), "随机策略应注入过一些输入");
        // Injected actions must all be in the vocabulary
        for inp in &res.recording.inputs {
            assert!(["left", "right"].contains(&inp.action.as_str()), "意外动作 {inp:?}");
        }
        let _ = Action { action: "x".into(), phase: "pressed".into() }; // reference the Action type
    }

    // ---- Numeric telemetry (NumericStat / numeric_summary) unit tests ----

    #[test]
    fn numeric_stat_observe_tracks_min_max_last_monotonic_zero() {
        // 100 → 50 → 200 → 0: min=0 max=200 last=0; dipped mid-way → not monotonic; hit 0
        let mut s = NumericStat::start(100.0);
        s.observe(50.0);
        s.observe(200.0);
        s.observe(0.0);
        assert_eq!(s.first, 100.0);
        assert_eq!(s.last, 0.0);
        assert_eq!(s.min, 0.0);
        assert_eq!(s.max, 200.0);
        assert!(!s.monotonic_up, "中途降过就不是只增不减");
        assert!(s.hit_zero, "触达过 0");
        assert!(!s.non_finite);
    }

    #[test]
    fn numeric_stat_monotonic_up_stays_true_when_only_grows() {
        let mut s = NumericStat::start(1.0);
        for v in [2.0, 4.0, 8.0, 16.0] {
            s.observe(v);
        }
        assert!(s.monotonic_up, "只增不减");
        assert_eq!(s.max, 16.0);
        assert!(!s.hit_zero);
    }

    #[test]
    fn numeric_stat_flags_non_finite() {
        let mut s = NumericStat::start(1.0);
        s.observe(f64::INFINITY);
        assert!(s.non_finite, "inf 必须标 non_finite");
        // Non-finite values don't pollute min/max (still the range of finite observations)
        assert_eq!(s.max, 1.0);
    }

    #[test]
    fn collect_numeric_leaves_extracts_nested_paths() {
        // observation mirrors the SceneView projection structure: entities[].{name,components.{Comp.{field}}}
        let obs = json!({"entities": [
            {"name": "hero", "id": "0v0", "components": {
                "Resources": {"gold": 12.0, "wood": 3},
                "Stats": {"hp": 100}
            }},
            {"id": "1v0", "components": {"Tally": {"n": 7}}}  // unnamed → use id as label
        ]});
        let mut sum: BTreeMap<String, NumericStat> = BTreeMap::new();
        collect_numeric_leaves(&obs, &mut sum);
        assert!(sum.contains_key("hero/Resources.gold"), "{:?}", sum.keys().collect::<Vec<_>>());
        assert!(sum.contains_key("hero/Resources.wood"));
        assert!(sum.contains_key("hero/Stats.hp"));
        assert!(sum.contains_key("1v0/Tally.n"), "无名实体用 id 当 label");
        assert_eq!(sum["hero/Resources.gold"].first, 12.0);
    }

    #[test]
    fn collect_numeric_leaves_skips_non_numeric() {
        // bool/string don't enter the numeric summary (a flag is not a number)
        let obs = json!({"entities": [
            {"name": "w", "components": {"State": {"sealed": true, "label": "x", "count": 5}}}
        ]});
        let mut sum: BTreeMap<String, NumericStat> = BTreeMap::new();
        collect_numeric_leaves(&obs, &mut sum);
        assert!(sum.contains_key("w/State.count"));
        assert!(!sum.contains_key("w/State.sealed"), "bool 不收");
        assert!(!sum.contains_key("w/State.label"), "string 不收");
    }

    /// Logic that doubles some entity field each tick (mutates world directly, simulating economy runaway).
    struct DoublerLogic {
        ent: String,
    }
    impl GameLogic for DoublerLogic {
        fn on_tick(
            &mut self,
            world: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            // Find the named entity and double Bank.gold (float, to avoid i64 checked_add overflow errors)
            let id = world.entity_names().find(|(n, _)| *n == self.ent).map(|(_, id)| id);
            if let Some(id) = id {
                if let Ok(v) = world.get_component(id, "Bank") {
                    let cur = v.get("gold").and_then(|g| g.as_f64()).unwrap_or(0.0);
                    let _ = world.set_component(id, "Bank", json!({"gold": cur * 2.0}));
                }
            }
            Ok(())
        }
    }

    fn bank_world_sim() -> Sim {
        let mut sim = Sim::new(1);
        let id = sim.world.spawn_named("hero").unwrap();
        sim.world.set_component(id, "Bank", json!({"gold": 1.0})).unwrap();
        sim
    }

    #[test]
    fn run_session_numeric_summary_catches_runaway_growth() {
        let mut sim = bank_world_sim();
        let mut logic = DoublerLogic { ent: "hero".to_string() };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 30, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        let stat = res.numeric_summary.get("hero/Bank.gold").expect("应采到 hero/Bank.gold");
        assert!(stat.monotonic_up, "只 ×2 → 只增不减");
        // 30 ticks of doubling: far exceeds a thousand times the initial value (2^30 ≈ 1e9)
        assert!(stat.max > 1e6, "翻倍 30 次应跑飞到 >1e6，实际 {}", stat.max);
        assert!(stat.last > stat.first * 1000.0, "末值 ≫ 首值");
    }

    #[test]
    fn run_session_numeric_summary_is_incremental_not_full_history() {
        // The summary stores only one NumericStat per field, independent of tick count (incremental, no history stored)
        let mut sim = bank_world_sim();
        let mut logic = DoublerLogic { ent: "hero".to_string() };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 20, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // Only one numeric field → exactly one entry in the summary, not bloated by tick count
        assert_eq!(res.numeric_summary.len(), 1);
    }

    #[test]
    fn run_session_numeric_summary_is_deterministic() {
        let run = || {
            let mut sim = bank_world_sim();
            let mut logic = DoublerLogic { ent: "hero".to_string() };
            let eng = empty_engine();
            let mut strat = RandomStrategy::new(9);
            let cfg = SessionConfig { max_ticks: 15, seed: 9, ..Default::default() };
            run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap().numeric_summary
        };
        assert_eq!(run(), run(), "同输入两次跑数值摘要必须逐项一致");
    }

    // ---- Lookahead search (run_session_lookahead) unit tests ----

    #[test]
    fn lookahead_rejects_zero_depth() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let err = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 0, beam_width: 4 }).unwrap_err();
        assert!(err.contains("depth"), "{err}");
    }

    #[test]
    fn lookahead_rejects_zero_beam_width() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let err = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 4, beam_width: 0 }).unwrap_err();
        assert!(err.contains("beam_width"), "{err}");
    }

    /// A hand-assembled recording must be reproducible bit-for-bit by Sim::replay — locking down two things:
    /// "speculative restore doesn't pollute the recording" and "tick/checkpoint accounting matches sim's internal state".
    /// Uses action_engine (has left/right vocabulary, candidates non-empty → speculative path restores multiple times per real tick)
    /// + EmitAt (emits game-won at tick 70, crossing the 60 boundary to force a periodic checkpoint) to expose checkpoint misalignment.
    #[test]
    fn lookahead_recording_replays_bit_for_bit() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 70, event: "game-won".to_string(), pending: vec![] };
        let eng = action_engine();
        let cfg = SessionConfig { max_ticks: 300, seed: 0, ..Default::default() };
        let res = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 5, beam_width: 4 }).unwrap();
        assert_eq!(res.outcome, Outcome::Win);
        assert_eq!(res.ticks, 71, "tick 70 发的 game-won，step 后 tick=71 收到");
        // Crossed tick 60 → must have both a start (0,_) and a periodic (60,_) checkpoint
        assert!(res.recording.checkpoints.len() >= 2, "应有起点+周期 checkpoint: {:?}", res.recording.checkpoints);
        assert_eq!(res.recording.checkpoints[0].0, 0, "起点 checkpoint tick=0");
        assert_eq!(res.recording.ticks, 71);
        assert_eq!(res.recording.seed, 1, "录像 seed = sim.seed");

        // Key: cold-restart replay of this hand-assembled recording must match every checkpoint + final_hash
        let mut sim2 = Sim::new(1);
        let mut logic2 = EmitAt { at: 70, event: "game-won".to_string(), pending: vec![] };
        sim2.replay(&res.recording, &mut logic2).expect("手工攒的前瞻录像必须可重放且逐位一致");
        assert_eq!(sim2.world.state_hash(), res.recording.final_hash);
    }

    #[test]
    fn lookahead_is_deterministic_byte_for_byte() {
        let run = || {
            let mut sim = Sim::new(2);
            let mut logic = EmitAt { at: 50, event: "game-won".to_string(), pending: vec![] };
            let eng = action_engine();
            let cfg = SessionConfig { max_ticks: 200, seed: 2, ..Default::default() };
            run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 6, beam_width: 4 }).unwrap()
        };
        let a = run();
        let b = run();
        assert_eq!(a.outcome, b.outcome);
        assert_eq!(a.ticks, b.ticks);
        let ja = serde_json::to_string(&a.recording).unwrap();
        let jb = serde_json::to_string(&b.recording).unwrap();
        assert_eq!(ja, jb, "同 (项目,seed,depth,beam) 两次前瞻录像必须逐字节一致");
    }

    /// Done from the start / max_ticks=0: not a single real tick runs, but still produces a valid empty recording (start checkpoint + ticks=0).
    #[test]
    fn lookahead_zero_ticks_yields_valid_empty_recording() {
        let mut sim = Sim::new(3);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig { max_ticks: 0, seed: 0, ..Default::default() };
        let res = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 4, beam_width: 4 }).unwrap();
        assert_eq!(res.ticks, 0);
        assert_eq!(res.recording.ticks, 0);
        assert!(res.recording.inputs.is_empty());
        assert_eq!(res.recording.checkpoints, vec![(0, res.recording.final_hash)]);
        // Empty recording is also replayable (start is the end)
        let mut sim2 = Sim::new(3);
        sim2.replay(&res.recording, &mut NeverEnds).expect("空前瞻录像也可重放");
    }

    /// depth=1 degenerates to single-ply lookahead: each candidate looks ahead 1 frame to pick the best. Here we use a "emit game-won at tick 2" logic,
    /// depth=1 at the tick 1 frame (after step, tick=2 receives the win) can also receive the terminal — verifying the degenerate setting still runs and produces a recording.
    #[test]
    fn lookahead_depth_one_is_degenerate_one_ply() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 2, event: "game-won".to_string(), pending: vec![] };
        let eng = action_engine();
        let cfg = SessionConfig { max_ticks: 50, seed: 0, ..Default::default() };
        let res = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 1, beam_width: 4 }).unwrap();
        assert_eq!(res.outcome, Outcome::Win, "depth=1 退化档也该收到终止事件");
        assert_eq!(res.ticks, 3, "tick 2 发的 game-won，step 后 tick=3 收到");
        // depth=1 also hand-assembles a replayable recording
        let mut sim2 = Sim::new(1);
        let mut logic2 = EmitAt { at: 2, event: "game-won".to_string(), pending: vec![] };
        sim2.replay(&res.recording, &mut logic2).expect("depth=1 录像可重放");
    }

    // ---- prune_beam_diverse: preserve root-action diversity + determinism ----

    /// Build a minimal BeamNode (snapshot is Null placeholder — prune only reads score/root_action).
    fn node(root_action: usize, goal: f64, explore: u64) -> BeamNode {
        BeamNode {
            snapshot: Value::Null,
            score: NodeScore { terminal: 0, win_earliness: 0, goal_score: goal, explore },
            root_action,
            terminated: false,
            explore,
            best_goal: goal,
        }
    }

    #[test]
    fn prune_keeps_one_line_per_root_action_even_when_width_smaller() {
        // 6 children: root 0 has 3 (including the global best), root 1 and root 2 each have one (lower scores).
        // beam_width=2 < distinct root count 3 → the first pass still keeps each root's best one (3 entries), not crowded out by root 0's three.
        // Proves diversity: root 1 and root 2 lines aren't pruned to zero just because root 0 scores higher.
        let children = vec![
            node(0, 10.0, 5), // root0 best
            node(0, 9.0, 4),
            node(0, 8.0, 3),
            node(1, 2.0, 9), // root1 only
            node(2, 1.0, 1), // root2 only
        ];
        let kept = prune_beam_diverse(children, 2);
        let roots: std::collections::BTreeSet<usize> = kept.iter().map(|n| n.root_action).collect();
        assert!(roots.contains(&0) && roots.contains(&1) && roots.contains(&2),
            "每个根动作都该留一条线，实际根 {roots:?}");
        // Each root keeps only its best one (root0 keeps the 10.0 one)
        let root0: Vec<_> = kept.iter().filter(|n| n.root_action == 0).collect();
        assert_eq!(root0.len(), 1, "每根第 1 趟只留一条");
        assert_eq!(root0[0].score.goal_score, 10.0, "root0 留的是它的最优");
    }

    #[test]
    fn prune_fills_remaining_slots_with_global_best_when_width_allows() {
        // 3 roots, beam_width=5 > root count: first pass keeps 3 (each root's best), second pass fills 2 more with global runners-up.
        let children = vec![
            node(0, 10.0, 0),
            node(0, 9.0, 0), // root0 runner-up — should be filled in the 2nd pass
            node(1, 8.0, 0),
            node(1, 7.0, 0), // root1 runner-up — should be filled in the 2nd pass
            node(2, 1.0, 0),
        ];
        let kept = prune_beam_diverse(children, 5);
        assert_eq!(kept.len(), 5, "名额够就补满");
        // The fill-ins are global runners-up (9.0, 8.0, not lower-than-1.0) — filled by score order
        let goals: Vec<f64> = kept.iter().map(|n| n.score.goal_score).collect();
        assert!(goals.contains(&9.0) && goals.contains(&7.0), "第 2 趟按分序补次优: {goals:?}");
    }

    #[test]
    fn prune_is_deterministic_and_tiebreaks_by_root_action() {
        // All-equal tie: stable sort + smaller root action first → output order is deterministic (root ascending).
        let mk = || vec![node(2, 5.0, 0), node(0, 5.0, 0), node(1, 5.0, 0)];
        let a = prune_beam_diverse(mk(), 3);
        let b = prune_beam_diverse(mk(), 3);
        let ra: Vec<usize> = a.iter().map(|n| n.root_action).collect();
        let rb: Vec<usize> = b.iter().map(|n| n.root_action).collect();
        assert_eq!(ra, rb, "同输入两次剪枝顺序必须一致");
        assert_eq!(ra, vec![0, 1, 2], "平手按根动作下标升序");
    }
}
