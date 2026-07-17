//! Strategy library — pure logic that consumes a Scene View and produces an action. Each strategy
//! uses an **independent Pcg32** seeded from the playtest seed (never touches sim.rng), so the same
//! seed yields the same sequence and is fully reproducible.

use serde::{Deserialize, Serialize};
use vitric_sim::{InputRecord, Pcg32};

use crate::scene_view::{Action, SceneView};

/// A qualitative note (subjective hint emitted by the LLM tier during human-like play, design draft section 2/5 "LLM qualitative note").
///
/// Only the LLM strategy produces notes (the human-language impressions like clarity/continuity/choice effectiveness); cheap strategy tiers don't.
/// Notes are **pure telemetry**: they don't enter the recording, don't enter the hash, and don't affect determinism — they are a bystander record of
/// "how this LLM session looked", on the same level as state_trace/fired_events. The report honestly labels them as "LLM subjective hints, pending human review".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaytestNote {
    /// On which decision tick this note surfaced (the view the LLM was looking at when it spoke).
    pub tick: u64,
    /// Note type (normalized label): commonly "clarity" (don't know what to do) / "continuity" (contradicts earlier) /
    /// "choice" (option is meaningless) / "other". Self-reported by the LLM, normalized at parse time into these categories.
    pub kind: String,
    /// Note body (the LLM's original words).
    pub text: String,
}

/// Strategy interface: look at a view, pick an action (or do nothing this tick).
pub trait Strategy {
    /// None = press nothing this tick. The returned action must come from `view.actions` (the legal set).
    fn choose(&mut self, view: &SceneView) -> Option<Action>;

    /// Take the qualitative notes accumulated by this strategy so far (draining clears them, to avoid duplicate collection).
    /// Default empty impl — only the LLM strategy overrides this to produce notes; cheap tiers (random/greedy/…) never produce any.
    /// session calls it once per tick to fold notes into SessionResult.notes (not entering recording/hash).
    fn drain_notes(&mut self) -> Vec<PlaytestNote> {
        Vec::new()
    }
}

/// Random strategy: pick uniformly from legal actions (including some probability of "do nothing").
/// Wide coverage, specifically looks for unexpected soft-locks (design draft section 2).
pub struct RandomStrategy {
    rng: Pcg32,
}

impl RandomStrategy {
    pub fn new(seed: u64) -> RandomStrategy {
        RandomStrategy { rng: Pcg32::new(seed) }
    }
}

impl Strategy for RandomStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        // [0, n]: n actions + 1 "do nothing" slot. Doing nothing is also a legal choice
        // (holding the button constantly vs occasionally releasing are different exploration paths).
        let n = view.actions.len() as i64;
        let pick = self.rng.range_i64(0, n);
        if pick == n {
            None
        } else {
            Some(view.actions[pick as usize].clone())
        }
    }
}

/// Greedy strategy: greedy toward a goal-derived quantity (design draft section 2, section 11 item 6).
///
/// **With a goal** (playtest.json declares `goal:{quantity, direction}`): each tick reads
/// `view.observation.derived[quantity]` and compares to the previous tick to decide whether this step "moved toward the goal".
///
/// **Approximation note (stage 6)**: this stage doesn't have the "one-step lookahead" infrastructure (would require cloning world + trial-injecting each action
/// then step to see which makes the goal better — that would borrow sim into the strategy, breaking the "strategy only sees Scene View" pure-logic boundary).
/// So we fall back to a **lightweight heuristic**: maintain "the previous action + the previous tick's goal value",
/// - if the previous action moved the goal in the desired direction → this tick **repeat** that action (lock the effective direction);
/// - if no improvement (flat/worse) → **switch action** (PCG random pick, jump out of the ineffective direction).
///
/// This isn't optimal greedy (no real lookahead), but it's enough to make greedy "walk toward the goal" rather than randomly wander — when something is consistently effective it
/// locks onto the action that advances the goal and holds it. Determinism: action selection is fully driven by an independent Pcg32, same seed + same goal trajectory
/// always produces the same sequence.
///
/// **Without a goal** it degrades to "PCG-driven random" (byte-identical to stages 1~5, backward compatibility locked by tests) —
/// without a goal quantity any "heuristic" is just blind guessing, so it honestly degrades to reproducible random.
pub struct GreedyStrategy {
    /// PCG for action selection (used directly as RandomStrategy when no goal; used for "switch action" when there is a goal).
    rng: Pcg32,
    /// Optimization goal (None=no goal, degrade to random).
    goal: Option<crate::config::GoalSpec>,
    /// The action chosen last tick (used to repeat an effective action). None=not chosen yet.
    last_action: Option<Action>,
    /// The goal quantity value observed last tick (for comparing improvement direction). None=not observed yet.
    last_value: Option<f64>,
}

impl GreedyStrategy {
    /// Goal-less greedy (degrades to reproducible random, behavior identical to RandomStrategy).
    pub fn new(seed: u64) -> GreedyStrategy {
        GreedyStrategy { rng: Pcg32::new(seed), goal: None, last_action: None, last_value: None }
    }

    /// Greedy with a derived goal (walks in the goal.direction of goal.quantity).
    pub fn with_goal(seed: u64, goal: crate::config::GoalSpec) -> GreedyStrategy {
        GreedyStrategy {
            rng: Pcg32::new(seed),
            goal: Some(goal),
            last_action: None,
            last_value: None,
        }
    }

    /// Read the goal-derived quantity's current value from view (None if unavailable / not a number).
    fn read_goal_value(&self, view: &SceneView) -> Option<f64> {
        let goal = self.goal.as_ref()?;
        view.observation
            .get("derived")
            .and_then(|d| d.get(&goal.quantity))
            .and_then(|v| v.as_f64())
    }

    /// Pick a random legal action (including the "do nothing" slot) — same semantics as RandomStrategy::choose,
    /// to guarantee that goal-less greedy matches RandomStrategy item by item under the same seed.
    fn random_pick(&mut self, view: &SceneView) -> Option<Action> {
        let n = view.actions.len() as i64;
        let pick = self.rng.range_i64(0, n);
        if pick == n {
            None
        } else {
            Some(view.actions[pick as usize].clone())
        }
    }
}

impl Strategy for GreedyStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        // no goal: pure random (backward compatible, behavior identical to RandomStrategy)
        if self.goal.is_none() {
            return self.random_pick(view);
        }

        let cur = self.read_goal_value(view);
        // decide whether the previous step moved the goal in the desired direction
        let improved = match (self.last_value, cur, &self.goal) {
            (Some(prev), Some(now), Some(g)) => match g.direction {
                crate::config::GoalDirection::Min => now < prev,
                crate::config::GoalDirection::Max => now > prev,
            },
            _ => false, // no comparable previous/current value → treat as "no improvement", go explore
        };

        let action = if improved {
            // previous action was effective: repeat it (lock the advancing direction). last_action should exist; defensively fall back to random if not.
            match &self.last_action {
                Some(a) => Some(a.clone()),
                None => self.random_pick(view),
            }
        } else {
            // no improvement: switch action to explore
            self.random_pick(view)
        };

        self.last_action = action.clone();
        self.last_value = cur;
        action
    }
}

/// Coverage strategy: systematically rotates to inject **every** action in the action vocabulary at least once (design draft section 2 coverage).
/// Specifically finds "dead actions never triggered before" — random strategy might go a whole session without touching some action, coverage guarantees it's touched.
///
/// How it rotates: maintains a rotation cursor, each tick picks `actions[cursor]` then advances; the cursor is mod the current action count,
/// so changes to the action set (vocabulary changes after a level switch) don't go out of bounds. The starting point is scattered by PCG from the seed — different seeds
/// start rotating from different positions in the vocabulary, different coverage orders but all traverse the full set; same seed is fully reproducible (determinism rule).
/// Deliberately **does not** insert "do nothing": coverage's job is to try every action; releasing-exploration is left to random.
pub struct CoverageStrategy {
    /// Rotation cursor (monotonically increasing, mod action count at use).
    cursor: usize,
    /// PCG-seeded start offset (deterministic and reproducible), added to cursor then mod.
    start_offset: u64,
}

impl CoverageStrategy {
    pub fn new(seed: u64) -> CoverageStrategy {
        // use a one-shot PCG to draw a start offset: same seed same offset, different seeds start from different positions
        let mut rng = Pcg32::new(seed);
        let start_offset = rng.next_u32() as u64;
        CoverageStrategy { cursor: 0, start_offset }
    }
}

impl Strategy for CoverageStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        let n = view.actions.len() as u64;
        // (start offset + cursor) mod action count = index of the action to try this tick
        let idx = ((self.start_offset + self.cursor as u64) % n) as usize;
        self.cursor = self.cursor.wrapping_add(1);
        Some(view.actions[idx].clone())
    }
}

/// Economy pressure strategy (design draft section 2 `economy` / section 6 main for simulation-management) — purpose-built **to find numeric breakage**:
/// resources **growing unboundedly (runaway)** or **hitting zero and stalling (collapse)**.
///
/// How it finds them: random/coverage spread actions out, so the cumulative effect of a single action is diluted and doesn't reach extremes; whereas economy breakage is
/// exposed only by **pressing one action many times in a row** (keep clicking "sell" to pile gold to overflow, keep clicking "buy" to drain resources to 0).
/// So this strategy **locks one action and repeats it R times before rotating to the next** — pushing each action's per-action cumulative effect
/// to the extreme, so that numeric telemetry's max/min/monotonic flags surface runaway/collapse.
///
/// R and rotation are PCG-seeded (from the playtest seed, never touches sim.rng): same seed same sequence, fully reproducible
/// (determinism rule). R is drawn randomly from a range (to avoid overly uniform "every action pressed the same number of times"); the rotation cursor
/// advances and is mod the action count, so changes to the action set (vocabulary changes after a level switch) don't go out of bounds. The start point is PCG-scattered,
/// different seeds start pressing from different actions. Deliberately **does not** insert "do nothing": economy pressure wants to press hard to the extreme.
pub struct EconomyStrategy {
    /// PCG (seeds R and start point).
    rng: Pcg32,
    /// Currently locked action index (used after mod the action count).
    cursor: usize,
    /// How many more times the current action should repeat (remaining count, rotate to next when it hits 0).
    remaining: u64,
}

/// The closed range from which the per-action repeat count R is drawn. Large enough to push cumulative effects to extremes
/// (doubling-style runaway reaches ~1e9 in ~30 presses; draining-style collapse hits zero in a few tens of presses), but not so large that one action fills a whole session.
const ECONOMY_REPEAT_MIN: i64 = 24;
const ECONOMY_REPEAT_MAX: i64 = 64;

impl EconomyStrategy {
    pub fn new(seed: u64) -> EconomyStrategy {
        let mut rng = Pcg32::new(seed);
        // start point: which position in the vocabulary to start pressing from (different per seed), drawn first and stored as cursor's base offset.
        // here cursor is recorded as a large offset, mod the current action count at choose time — the action count isn't known at construction.
        let cursor = rng.next_u32() as usize;
        EconomyStrategy { rng, cursor, remaining: 0 }
    }

    /// Draw a new repeat count R (random in range via PCG).
    fn roll_repeat(&mut self) -> u64 {
        self.rng.range_i64(ECONOMY_REPEAT_MIN, ECONOMY_REPEAT_MAX) as u64
    }
}

impl Strategy for EconomyStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        if view.actions.is_empty() {
            return None;
        }
        let n = view.actions.len();
        // current locked action's repeat count exhausted: rotate to the next action, redraw R
        if self.remaining == 0 {
            self.cursor = self.cursor.wrapping_add(1);
            self.remaining = self.roll_repeat();
        }
        self.remaining -= 1;
        // locked action is the one at cursor mod current action count (press the same one each tick until remaining hits 0)
        Some(view.actions[self.cursor % n].clone())
    }
}

/// Scripted strategy: injects actions from a fixed input sequence at **recorded ticks**, **without looking at SceneView**
/// (the replay part of design draft section 3 "seed-based exploration"). A seed recording is itself a "what was pressed on tick N" script —
/// feeding it back as-is reproduces that session; feeding a perturbed script back is "take one divergent step near the original solution".
///
/// How it tracks ticks: `Strategy::choose` is called once per tick (in the session loop); the strategy maintains its own
/// counter `cur_tick`, incrementing each call. When `cur_tick` hits some `InputRecord.tick` in the script, it emits that record's
/// action. **Multiple inputs on the same tick**: seed recordings allow multiple actions to be injected on one tick (e.g. left+space same frame),
/// but `choose` can only return one action per tick — so this strategy injects only the first (in script order) for "multiple on same tick".
/// This is a known limitation: same-frame multi-input in a seed recording gets truncated to one. The vast majority of puzzle/story scripts are "one action per frame"
/// sequences and are unaffected; to truly reproduce same-frame multi-input byte-for-byte you'd go through `sim.replay` (a different path, not the strategy path).
///
/// `then_explore`: after the script finishes, switch to this strategy to continue (truncation + divergence) — the seed-exploration truncate operator relies on it,
/// "follow the script up to step K, then hand off to random to wander". None = after the script finishes, press nothing (pure replay / pure prefix).
pub struct ScriptedStrategy {
    /// Script: (tick, action), sorted ascending by tick. When cur_tick hits an entry, emit it.
    script: Vec<(u64, Action)>,
    /// Current tick (incremented each choose call) — session calls choose once per tick, in sync with sim.tick.
    cur_tick: u64,
    /// How far the script has been consumed (script is sorted by tick, cursor advances monotonically, avoiding a full scan each tick).
    cursor: usize,
    /// Relay strategy after the script finishes (truncation+divergence). None=silent after finish.
    then_explore: Option<Box<dyn Strategy>>,
}

impl ScriptedStrategy {
    /// Build a scripted strategy from a list of (tick, Action). The script is stable-sorted by tick (tolerates caller passing unordered input).
    /// `then_explore`: relay strategy after the script finishes, None=silent after finish.
    pub fn new(
        mut script: Vec<(u64, Action)>,
        then_explore: Option<Box<dyn Strategy>>,
    ) -> ScriptedStrategy {
        // stable sort: same-tick entries keep their original relative order (only matters when taking the first)
        script.sort_by_key(|(t, _)| *t);
        ScriptedStrategy { script, cur_tick: 0, cursor: 0, then_explore }
    }

    /// Build a script from a recording's input sequence (seed recording → script). phase is carried along (pressed/released).
    pub fn from_inputs(
        inputs: &[InputRecord],
        then_explore: Option<Box<dyn Strategy>>,
    ) -> ScriptedStrategy {
        let script = inputs
            .iter()
            .map(|r| (r.tick, Action { action: r.action.clone(), phase: r.phase.clone() }))
            .collect();
        ScriptedStrategy::new(script, then_explore)
    }
}

impl Strategy for ScriptedStrategy {
    fn choose(&mut self, view: &SceneView) -> Option<Action> {
        let tick = self.cur_tick;
        self.cur_tick += 1;

        // script cursor past the current tick: script is done, hand off to the relay strategy (or stay silent)
        if self.cursor >= self.script.len() {
            return match &mut self.then_explore {
                // the relay strategy still needs to look at view (it's a reactive strategy like random/coverage)
                Some(s) => s.choose(view),
                None => None,
            };
        }
        // script is sorted by tick: the cursor's tick is "the next tick that should fire"
        let (script_tick, action) = &self.script[self.cursor];
        if *script_tick == tick {
            let out = action.clone();
            self.cursor += 1;
            // skip other entries on the same tick (only inject one action per tick, see the limitation in the type doc)
            while self.cursor < self.script.len() && self.script[self.cursor].0 == tick {
                self.cursor += 1;
            }
            Some(out)
        } else {
            // not yet at this entry's tick (the in-between ticks have no scheduled action) — do nothing this tick.
            // note: don't relay — the script isn't done yet; the in-between empty ticks are meant to "hold still",
            // and letting then_explore insert mid-script would break script reproduction.
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_view::Action;

    fn view_with_actions(names: &[&str]) -> SceneView {
        let actions = names
            .iter()
            .map(|n| Action { action: n.to_string(), phase: "pressed".to_string() })
            .collect();
        SceneView { observation: serde_json::json!({}), actions, done: None }
    }

    #[test]
    fn non_llm_strategies_drain_no_notes() {
        // cheap strategy tiers default to producing no notes (drain_notes default empty impl) — the note channel is LLM-tier only
        let view = view_with_actions(&["a", "b"]);
        let mut r = RandomStrategy::new(1);
        let mut c = CoverageStrategy::new(1);
        let mut e = EconomyStrategy::new(1);
        for s in [&mut r as &mut dyn Strategy, &mut c, &mut e] {
            let _ = s.choose(&view);
            assert!(s.drain_notes().is_empty(), "非 LLM 策略不产 note");
        }
    }

    #[test]
    fn random_only_picks_legal_actions() {
        let view = view_with_actions(&["left", "right", "space"]);
        let mut s = RandomStrategy::new(7);
        for _ in 0..500 {
            if let Some(a) = s.choose(&view) {
                assert!(view.actions.contains(&a), "选出的动作必须在合法集合里: {a:?}");
            }
        }
    }

    #[test]
    fn random_is_deterministic_same_seed_same_sequence() {
        let view = view_with_actions(&["left", "right", "space"]);
        let mut a = RandomStrategy::new(42);
        let mut b = RandomStrategy::new(42);
        let seq_a: Vec<_> = (0..1000).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..1000).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b, "同 seed 必须出同一序列");
    }

    #[test]
    fn random_differs_across_seeds() {
        let view = view_with_actions(&["left", "right", "space"]);
        let mut a = RandomStrategy::new(1);
        let mut b = RandomStrategy::new(2);
        let seq_a: Vec<_> = (0..200).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..200).map(|_| b.choose(&view)).collect();
        assert_ne!(seq_a, seq_b, "不同 seed 应给出不同序列");
    }

    #[test]
    fn empty_actions_yields_no_op() {
        let view = view_with_actions(&[]);
        let mut s = RandomStrategy::new(3);
        assert_eq!(s.choose(&view), None);
    }

    #[test]
    fn greedy_is_deterministic() {
        let view = view_with_actions(&["a", "b"]);
        let mut a = GreedyStrategy::new(99);
        let mut b = GreedyStrategy::new(99);
        let seq_a: Vec<_> = (0..300).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..300).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b);
    }

    // ---- stage 6: greedy with a derived goal ----

    use crate::config::{GoalDirection, GoalSpec};

    /// Build a view with derived[quantity]=val (simulates the derived quantity injected by SceneView).
    fn view_with_goal(names: &[&str], quantity: &str, val: f64) -> SceneView {
        let actions = names
            .iter()
            .map(|n| Action { action: n.to_string(), phase: "pressed".to_string() })
            .collect();
        SceneView {
            observation: serde_json::json!({ "entities": [], "derived": { quantity: val } }),
            actions,
            done: None,
        }
    }

    #[test]
    fn greedy_with_goal_is_deterministic() {
        let goal = GoalSpec { quantity: "d".to_string(), direction: GoalDirection::Min };
        let mut a = GreedyStrategy::with_goal(5, goal.clone());
        let mut b = GreedyStrategy::with_goal(5, goal);
        let mut sa = Vec::new();
        let mut sb = Vec::new();
        let mut d = 100.0;
        for _ in 0..200 {
            sa.push(a.choose(&view_with_goal(&["x", "y", "z"], "d", d)));
            sb.push(b.choose(&view_with_goal(&["x", "y", "z"], "d", d)));
            d -= 1.0;
        }
        assert_eq!(sa, sb, "同 seed + 同目标轨迹必出同一序列");
    }

    #[test]
    fn greedy_with_goal_repeats_action_when_improving() {
        // goal min: each tick the distance decreases (action effective) → greedy should tend to repeat the same effective action,
        // i.e. "the longest run of consecutive identical actions" is clearly longer than goal-less random.
        let goal = GoalSpec { quantity: "d".to_string(), direction: GoalDirection::Min };
        let mut g = GreedyStrategy::with_goal(7, goal);
        let mut seq: Vec<String> = Vec::new();
        let mut d = 100.0;
        for _ in 0..60 {
            if let Some(a) = g.choose(&view_with_goal(&["x", "y", "z"], "d", d)) {
                seq.push(a.action);
            }
            d -= 1.0; // improving all the time
        }
        // longest run of the same action
        let mut max_run = 1usize;
        let mut run = 1usize;
        for w in seq.windows(2) {
            if w[0] == w[1] {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 1;
            }
        }
        assert!(max_run >= 10, "持续改善时应锁住有效动作连按，最长连段 {max_run}");
    }

    #[test]
    fn greedy_with_goal_differs_from_no_goal() {
        // with goal vs without goal: on the same "always-improving" trajectory, the two action sequences should differ
        // (with goal locks the action, without goal is uniform random) — proves goal really changed behavior.
        let goal = GoalSpec { quantity: "d".to_string(), direction: GoalDirection::Min };
        let mut with = GreedyStrategy::with_goal(3, goal);
        let mut without = GreedyStrategy::new(3);
        let mut sw = Vec::new();
        let mut swo = Vec::new();
        let mut d = 100.0;
        for _ in 0..80 {
            let v = view_with_goal(&["x", "y", "z"], "d", d);
            sw.push(with.choose(&v).map(|a| a.action));
            swo.push(without.choose(&v).map(|a| a.action));
            d -= 1.0;
        }
        assert_ne!(sw, swo, "有 goal 的 greedy 行为必须和无 goal（随机）不同");
    }

    #[test]
    fn greedy_with_goal_only_picks_legal_actions() {
        let goal = GoalSpec { quantity: "d".to_string(), direction: GoalDirection::Max };
        let mut g = GreedyStrategy::with_goal(11, goal);
        let mut d = 0.0;
        for _ in 0..300 {
            let v = view_with_goal(&["a", "b"], "d", d);
            if let Some(act) = g.choose(&v) {
                assert!(v.actions.contains(&act), "只选合法动作: {act:?}");
            }
            d += 0.5;
        }
    }

    #[test]
    fn greedy_without_goal_still_random() {
        // goal-less greedy degrades to random: same seed same sequence as RandomStrategy (behavior invariance rule)
        let view = view_with_actions(&["a", "b", "c"]);
        let mut g = GreedyStrategy::new(42);
        let mut r = RandomStrategy::new(42);
        let sg: Vec<_> = (0..200).map(|_| g.choose(&view)).collect();
        let sr: Vec<_> = (0..200).map(|_| r.choose(&view)).collect();
        assert_eq!(sg, sr, "无目标 greedy 必须和 random 同 seed 逐项一致");
    }

    #[test]
    fn coverage_visits_every_action_within_a_full_cycle() {
        let view = view_with_actions(&["a", "b", "c", "d"]);
        let mut s = CoverageStrategy::new(7);
        // running a full cycle (as many ticks as there are actions) should try each action once
        let mut hit: std::collections::HashSet<String> = std::collections::HashSet::new();
        for _ in 0..view.actions.len() {
            let a = s.choose(&view).expect("非空词汇必出动作");
            hit.insert(a.action);
        }
        assert_eq!(hit.len(), 4, "一整轮必须覆盖全部 4 个动作: {hit:?}");
    }

    #[test]
    fn coverage_is_deterministic_same_seed() {
        let view = view_with_actions(&["a", "b", "c"]);
        let mut a = CoverageStrategy::new(11);
        let mut b = CoverageStrategy::new(11);
        let seq_a: Vec<_> = (0..50).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..50).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b, "同 seed coverage 必出同一序列");
    }

    #[test]
    fn coverage_start_offset_differs_across_seeds() {
        // Different seeds start at different offsets: at least one adjacent tick pair should differ (otherwise they aren't spread out)
        let view = view_with_actions(&["a", "b", "c", "d", "e"]);
        let mut a = CoverageStrategy::new(1);
        let mut b = CoverageStrategy::new(999);
        let seq_a: Vec<_> = (0..5).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..5).map(|_| b.choose(&view)).collect();
        assert_ne!(seq_a, seq_b, "不同 seed 应从不同起点起轮");
    }

    #[test]
    fn coverage_empty_actions_yields_no_op() {
        let view = view_with_actions(&[]);
        let mut s = CoverageStrategy::new(3);
        assert_eq!(s.choose(&view), None);
    }

    #[test]
    fn coverage_never_picks_illegal_action() {
        let view = view_with_actions(&["x", "y"]);
        let mut s = CoverageStrategy::new(5);
        for _ in 0..100 {
            let a = s.choose(&view).unwrap();
            assert!(view.actions.contains(&a), "{a:?}");
        }
    }

    #[test]
    fn economy_locks_one_action_for_a_run_then_rotates() {
        // economy strategy should **press the same action many times** before switching, not switch every tick (this is its distinction from coverage)
        let view = view_with_actions(&["buy", "sell", "wait"]);
        let mut s = EconomyStrategy::new(3);
        let seq: Vec<String> = (0..200).map(|_| s.choose(&view).unwrap().action).collect();
        // there should exist a run of "the same action ≥ 20 times in a row" (the lock-and-repeat signature)
        let mut max_run = 1usize;
        let mut run = 1usize;
        for w in seq.windows(2) {
            if w[0] == w[1] {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 1;
            }
        }
        assert!(max_run >= 20, "应有一段连按 ≥20 次同一动作，实际最长连段 {max_run}");
        // within 200 ticks it should have rotated through more than one action (not deadlocked on one)
        let distinct: std::collections::HashSet<&String> = seq.iter().collect();
        assert!(distinct.len() >= 2, "应轮转过多个动作: {distinct:?}");
    }

    #[test]
    fn economy_only_picks_legal_actions() {
        let view = view_with_actions(&["a", "b"]);
        let mut s = EconomyStrategy::new(1);
        for _ in 0..300 {
            let a = s.choose(&view).unwrap();
            assert!(view.actions.contains(&a), "经济策略只选合法动作: {a:?}");
        }
    }

    #[test]
    fn economy_is_deterministic_same_seed() {
        let view = view_with_actions(&["buy", "sell", "hold"]);
        let mut a = EconomyStrategy::new(77);
        let mut b = EconomyStrategy::new(77);
        let seq_a: Vec<_> = (0..500).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..500).map(|_| b.choose(&view)).collect();
        assert_eq!(seq_a, seq_b, "同 seed economy 必出同一序列");
    }

    #[test]
    fn economy_differs_across_seeds() {
        let view = view_with_actions(&["buy", "sell", "hold", "tax", "spend"]);
        let mut a = EconomyStrategy::new(1);
        let mut b = EconomyStrategy::new(424242);
        let seq_a: Vec<_> = (0..50).map(|_| a.choose(&view)).collect();
        let seq_b: Vec<_> = (0..50).map(|_| b.choose(&view)).collect();
        assert_ne!(seq_a, seq_b, "不同 seed 应从不同动作/不同 R 起压");
    }

    #[test]
    fn economy_empty_actions_yields_no_op() {
        let view = view_with_actions(&[]);
        let mut s = EconomyStrategy::new(2);
        assert_eq!(s.choose(&view), None);
    }

    fn act(name: &str) -> Action {
        Action { action: name.to_string(), phase: "pressed".to_string() }
    }

    #[test]
    fn scripted_injects_action_on_its_recorded_tick() {
        // Script: tick 2 presses a, tick 5 presses b. Other ticks do nothing.
        let script = vec![(2u64, act("a")), (5u64, act("b"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["a", "b", "c"]);
        let mut got: Vec<Option<String>> = Vec::new();
        for _ in 0..8 {
            got.push(s.choose(&view).map(|a| a.action));
        }
        // tick 0,1 none; tick2=a; tick3,4 none; tick5=b; tick6,7 none
        assert_eq!(
            got,
            vec![
                None,
                None,
                Some("a".to_string()),
                None,
                None,
                Some("b".to_string()),
                None,
                None,
            ]
        );
    }

    #[test]
    fn scripted_ignores_scene_view() {
        // scripted strategy emits its action even if it isn't in view.actions (script isn't constrained by the legal set — it's a replay)
        let script = vec![(0u64, act("offscreen"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["only-this"]);
        assert_eq!(s.choose(&view).map(|a| a.action), Some("offscreen".to_string()));
    }

    #[test]
    fn scripted_then_explore_takes_over_after_script_ends() {
        // script only goes to tick 0; after that hand off to random relay (truncation+divergence)
        let script = vec![(0u64, act("scripted"))];
        let mut s = ScriptedStrategy::new(script, Some(Box::new(RandomStrategy::new(7))));
        let view = view_with_actions(&["x", "y", "z"]);
        // tick0 = script's scripted
        assert_eq!(s.choose(&view).map(|a| a.action), Some("scripted".to_string()));
        // tick1 onwards hand off to random: chosen action (if any) must come from view's legal set
        let mut saw_explore = false;
        for _ in 0..200 {
            if let Some(a) = s.choose(&view) {
                assert!(view.actions.contains(&a), "接力策略只选合法动作: {a:?}");
                saw_explore = true;
            }
        }
        assert!(saw_explore, "脚本放完后接力策略应注入过动作");
    }

    #[test]
    fn scripted_no_explore_goes_silent_after_script() {
        let script = vec![(0u64, act("a"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["a"]);
        assert_eq!(s.choose(&view).map(|a| a.action), Some("a".to_string()));
        // script done, no relay: from here on never press anything
        for _ in 0..50 {
            assert_eq!(s.choose(&view), None);
        }
    }

    #[test]
    fn scripted_from_inputs_round_trips_ticks_and_phases() {
        use vitric_sim::InputRecord;
        let inputs = vec![
            InputRecord { tick: 1, action: "left".to_string(), phase: "pressed".to_string() },
            InputRecord { tick: 3, action: "left".to_string(), phase: "released".to_string() },
        ];
        let mut s = ScriptedStrategy::from_inputs(&inputs, None);
        let view = view_with_actions(&["left"]);
        let mut seq = Vec::new();
        for _ in 0..5 {
            seq.push(s.choose(&view).map(|a| (a.action, a.phase)));
        }
        assert_eq!(seq[1], Some(("left".to_string(), "pressed".to_string())));
        assert_eq!(seq[3], Some(("left".to_string(), "released".to_string())));
        assert_eq!(seq[0], None);
        assert_eq!(seq[2], None);
        assert_eq!(seq[4], None);
    }

    #[test]
    fn scripted_same_tick_multiple_keeps_first_only() {
        // two entries on the same tick: only the first is injected (known limitation), the second is skipped
        let script = vec![(2u64, act("first")), (2u64, act("second"))];
        let mut s = ScriptedStrategy::new(script, None);
        let view = view_with_actions(&["first", "second"]);
        let mut seq = Vec::new();
        for _ in 0..4 {
            seq.push(s.choose(&view).map(|a| a.action));
        }
        assert_eq!(seq, vec![None, None, Some("first".to_string()), None]);
    }
}
