//! Seed-based exploration (design draft section 3): takes a gate-certificate recording as the **seed** —
//! a deterministic input sequence that "proves this session can be cleared". `perturb_plan` performs
//! **controlled mutation** on this sequence; each mutation is a new script fed to
//! [`crate::strategy::ScriptedStrategy`] to run as a session, to see whether it soft-locks or fails to reach an ending.
//!
//! Why "find flaws around a known solution" rather than "solve the puzzle from scratch": real hard-puzzle
//! solutions are specific N-step sequences that random/greedy cannot solve (solving arbitrary puzzles from
//! scratch is an intractable AI problem). So we take the developer-provided solution as the starting point
//! and perturb around it — reorder, diverge, change one choice, skip a step, truncate early to diverge — to
//! find flaws the developer did not wall off.
//!
//! **Determinism**: all random decisions go through one PCG (`Pcg32::new(rng_seed)`), so the same
//! `(seed, n, rng_seed)` produces the **same set** of mutations (byte-identical, item by item). Operators
//! rotate in a fixed order and do not depend on hash-set iteration order.

use vitric_sim::{InputRecord, Pcg32, Recording};

use crate::scene_view::Action;

/// Perturbation operator kinds — used in rotation, also carried in the result so reports/debugging can label "how this one was produced".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PerturbOp {
    /// Original seed, unperturbed (entry 0 is always this, as a baseline: it should reproduce the seed's ending).
    Baseline,
    /// Drops one input (skip a step — what if a key press was missed).
    Drop,
    /// Swaps the **actions** of two inputs (keeps each tick, only swaps content — what if the operation order changed).
    Swap,
    /// Replaces one input's action with **another** action from the vocabulary (take the other branch at a fork / change one choice).
    Substitute,
    /// Truncates at a tick, after which a random strategy diverges (follow the solution for the first half, wander for the second half to find side paths).
    Truncate,
}

impl PerturbOp {
    /// Short name (for report display).
    pub fn name(self) -> &'static str {
        match self {
            PerturbOp::Baseline => "baseline",
            PerturbOp::Drop => "drop",
            PerturbOp::Swap => "swap",
            PerturbOp::Substitute => "substitute",
            PerturbOp::Truncate => "truncate",
        }
    }
}

/// One mutation: the script (fed to ScriptedStrategy) + how it was produced (operator) + whether to truncate-and-diverge.
///
/// `truncate_at`: Some(tick) means this script only goes up to that tick, after which a random strategy
/// should diverge (the CLI/caller uses this to splice a `then_explore` onto ScriptedStrategy); None = play the script to the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Perturbation {
    /// Operator label (baseline / drop / swap / substitute / truncate).
    pub op: PerturbOp,
    /// Mutated script: (tick, action), sorted ascending by tick.
    pub script: Vec<(u64, Action)>,
    /// Truncation point: Some = script truncates here, then random diverges; None = play the script to the end.
    pub truncate_at: Option<u64>,
}

/// Generate n mutated scripts from the seed recording (including entry 0, the baseline).
///
/// - Entry 0 is always the **original seed** (`Baseline`, unperturbed) — it reproduces the seed's ending and is the comparison baseline.
/// - Entries 1..n: operators rotate in `drop / swap / substitute / truncate` order, each drawing parameters
///   from the same PCG (which one to drop, which two to swap, which action to substitute, where to truncate).
/// - Determinism: the same `(seed recording, n, rng_seed)` produces a byte-identical set (single-line PCG seeding, operator rotation does not depend on hash order).
///
/// Degenerate case: when the seed has no inputs (empty recording), only the baseline is returned (nothing to perturb; fabricating mutations is meaningless).
pub fn perturb_plan(seed: &Recording, n: usize, rng_seed: u64) -> Vec<Perturbation> {
    let base_script: Vec<(u64, Action)> = seed
        .inputs
        .iter()
        .map(|r| (r.tick, Action { action: r.action.clone(), phase: r.phase.clone() }))
        .collect();

    let mut out = Vec::with_capacity(n.max(1));
    // Entry 0: baseline (original seed, unperturbed)
    out.push(Perturbation { op: PerturbOp::Baseline, script: base_script.clone(), truncate_at: None });
    if n <= 1 {
        return out;
    }

    // Vocabulary: action names that appeared in the seed (deduped, preserving order of appearance) — substitute picks "another action" from here.
    // Use the seed's own vocabulary rather than the global rule vocabulary: perturbation is "swap a known operation near the known solution",
    // and does not introduce keys that never appeared in the seed (that would be closer to random thrashing than "finding flaws around the solution").
    let mut vocab: Vec<String> = Vec::new();
    for (_, a) in &base_script {
        if !vocab.iter().any(|v| v == &a.action) {
            vocab.push(a.action.clone());
        }
    }

    // Empty script: nothing to perturb, only return the baseline (no matter how large n is, no variations can be produced)
    if base_script.is_empty() {
        return out;
    }

    let mut rng = Pcg32::new(rng_seed);
    // Operator rotation order: fixed four, selected by (i-1) % 4 — entry 1 is drop, entry 2 is swap, entry 3 is substitute,
    // entry 4 is truncate, entry 5 wraps back to drop... ensuring all four operators are used evenly.
    const OPS: [PerturbOp; 4] =
        [PerturbOp::Drop, PerturbOp::Swap, PerturbOp::Substitute, PerturbOp::Truncate];
    for i in 1..n {
        let op = OPS[(i - 1) % OPS.len()];
        let pert = apply_op(op, &base_script, &vocab, &mut rng);
        out.push(pert);
    }
    out
}

/// Run one operator to produce a mutation (starting from the original base_script — each entry mutates one spot
/// based on the seed's original form, not stacked on the previous mutation, so perturbations are independent and do not pollute each other).
fn apply_op(
    op: PerturbOp,
    base: &[(u64, Action)],
    vocab: &[String],
    rng: &mut Pcg32,
) -> Perturbation {
    match op {
        PerturbOp::Drop => {
            // Drop a random one
            let idx = rng.range_i64(0, base.len() as i64 - 1) as usize;
            let mut script = base.to_vec();
            script.remove(idx);
            Perturbation { op, script, truncate_at: None }
        }
        PerturbOp::Swap => {
            let mut script = base.to_vec();
            // Need at least two to swap; with only one, degrade to "as-is" (still labeled Swap, for reconciliation)
            if script.len() >= 2 {
                let a = rng.range_i64(0, script.len() as i64 - 1) as usize;
                // Pick a b not equal to a: draw from [0, len-2], shift by one if >= a, ensuring b != a
                let mut b = rng.range_i64(0, script.len() as i64 - 2) as usize;
                if b >= a {
                    b += 1;
                }
                // Only swap action content, keep each tick (what's swapped is the order of "what to press at this moment")
                let action_a = script[a].1.clone();
                let action_b = script[b].1.clone();
                script[a].1 = action_b;
                script[b].1 = action_a;
            }
            Perturbation { op, script, truncate_at: None }
        }
        PerturbOp::Substitute => {
            let mut script = base.to_vec();
            let idx = rng.range_i64(0, script.len() as i64 - 1) as usize;
            // Pick a **different** action name from the vocabulary to substitute; with only one action in the vocabulary there's nothing to substitute (degrade to as-is)
            if vocab.len() >= 2 {
                let cur = &script[idx].1.action;
                // Pick from the vocabulary "excluding the current action": collect candidates first then draw (candidates are non-empty, since vocab>=2 and cur is in vocab)
                let candidates: Vec<&String> = vocab.iter().filter(|v| *v != cur).collect();
                let pick = rng.range_i64(0, candidates.len() as i64 - 1) as usize;
                script[idx].1.action = candidates[pick].clone();
            }
            Perturbation { op, script, truncate_at: None }
        }
        PerturbOp::Truncate => {
            // Truncate at one of the inputs: keep the prefix, after the truncation-point tick hand off to random divergence.
            // The truncation point is taken as "the tick of some input" — starting to diverge after a particular step of the solution is more meaningful than cutting at an absolute tick.
            let cut_idx = rng.range_i64(0, base.len() as i64 - 1) as usize;
            let cut_tick = base[cut_idx].0;
            // Keep inputs with tick < cut_tick (from cut_tick onward, no longer follow the script, switch to random)
            let script: Vec<(u64, Action)> =
                base.iter().filter(|(t, _)| *t < cut_tick).cloned().collect();
            Perturbation { op, script, truncate_at: Some(cut_tick) }
        }
        PerturbOp::Baseline => {
            Perturbation { op, script: base.to_vec(), truncate_at: None }
        }
    }
}

/// Convenience: copy a recording's inputs into a `Vec<InputRecord>` (for tests / external script construction).
pub fn inputs_of(rec: &Recording) -> Vec<InputRecord> {
    rec.inputs.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitric_sim::{InputRecord, Recording};

    fn seed_recording(actions: &[(u64, &str)]) -> Recording {
        let mut rec = Recording { ticks: 100, ..Default::default() };
        for (t, a) in actions {
            rec.inputs.push(InputRecord {
                tick: *t,
                action: a.to_string(),
                phase: "pressed".to_string(),
            });
        }
        rec
    }

    fn three_step_seed() -> Recording {
        seed_recording(&[(1, "left"), (3, "up"), (5, "right")])
    }

    #[test]
    fn first_perturbation_is_unchanged_baseline() {
        let seed = three_step_seed();
        let plan = perturb_plan(&seed, 6, 42);
        assert_eq!(plan[0].op, PerturbOp::Baseline);
        // Baseline script equals the seed inputs item by item
        let expect: Vec<(u64, Action)> = seed
            .inputs
            .iter()
            .map(|r| (r.tick, Action { action: r.action.clone(), phase: r.phase.clone() }))
            .collect();
        assert_eq!(plan[0].script, expect);
        assert_eq!(plan[0].truncate_at, None);
    }

    #[test]
    fn plan_is_deterministic_same_inputs_same_plan() {
        let seed = three_step_seed();
        let a = perturb_plan(&seed, 10, 7);
        let b = perturb_plan(&seed, 10, 7);
        assert_eq!(a, b, "同 (seed,n,rng_seed) 必须出同一组变异");
    }

    #[test]
    fn plan_differs_across_rng_seeds() {
        // Different rng_seed should give a different set (at least one script differs)
        let seed = three_step_seed();
        let a = perturb_plan(&seed, 10, 1);
        let b = perturb_plan(&seed, 10, 999);
        assert_ne!(a, b, "不同 rng_seed 应给出不同变异组");
    }

    #[test]
    fn plan_has_n_entries() {
        let seed = three_step_seed();
        assert_eq!(perturb_plan(&seed, 1, 0).len(), 1);
        assert_eq!(perturb_plan(&seed, 5, 0).len(), 5);
        assert_eq!(perturb_plan(&seed, 13, 0).len(), 13);
    }

    #[test]
    fn operators_rotate_drop_swap_substitute_truncate() {
        let seed = three_step_seed();
        let plan = perturb_plan(&seed, 5, 0);
        // Entry 0 is baseline, 1..=4 rotate drop/swap/substitute/truncate
        assert_eq!(plan[1].op, PerturbOp::Drop);
        assert_eq!(plan[2].op, PerturbOp::Swap);
        assert_eq!(plan[3].op, PerturbOp::Substitute);
        assert_eq!(plan[4].op, PerturbOp::Truncate);
    }

    #[test]
    fn drop_removes_exactly_one_input() {
        let seed = three_step_seed();
        let plan = perturb_plan(&seed, 2, 0);
        // plan[1] is drop: one fewer
        assert_eq!(plan[1].op, PerturbOp::Drop);
        assert_eq!(plan[1].script.len(), seed.inputs.len() - 1);
    }

    #[test]
    fn swap_keeps_ticks_and_set_of_actions_but_changes_order() {
        // Find an rng_seed that actually swaps the actions (different seeds draw different (a,b))
        let seed = three_step_seed();
        let base: Vec<(u64, String)> =
            seed.inputs.iter().map(|r| (r.tick, r.action.clone())).collect();
        let mut found = false;
        for rs in 0..50u64 {
            // n=3: plan[2] is the swap (plan[1]=drop)
            let plan = perturb_plan(&seed, 3, rs);
            let swap = &plan[2];
            assert_eq!(swap.op, PerturbOp::Swap);
            // Tick sequence unchanged
            let ticks: Vec<u64> = swap.script.iter().map(|(t, _)| *t).collect();
            assert_eq!(ticks, vec![1, 3, 5]);
            // The "multiset" of actions is unchanged (only reordered)
            let mut a: Vec<&str> = swap.script.iter().map(|(_, x)| x.action.as_str()).collect();
            let mut b: Vec<&str> = base.iter().map(|(_, x)| x.as_str()).collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b, "swap 不增不减动作，只换序");
            // Found one that actually reordered
            let ordered: Vec<&str> = swap.script.iter().map(|(_, x)| x.action.as_str()).collect();
            let orig: Vec<&str> = base.iter().map(|(_, x)| x.as_str()).collect();
            if ordered != orig {
                found = true;
            }
        }
        assert!(found, "至少有一个 rng_seed 真换了动作顺序");
    }

    #[test]
    fn substitute_replaces_one_action_with_another_from_vocab() {
        let seed = three_step_seed();
        let vocab = ["left", "up", "right"];
        for rs in 0..30u64 {
            let plan = perturb_plan(&seed, 4, rs);
            let sub = &plan[3];
            assert_eq!(sub.op, PerturbOp::Substitute);
            // Length unchanged; each action is still in the vocabulary
            assert_eq!(sub.script.len(), seed.inputs.len());
            for (_, a) in &sub.script {
                assert!(vocab.contains(&a.action.as_str()), "替换只用种子词汇: {a:?}");
            }
        }
    }

    #[test]
    fn truncate_keeps_prefix_and_marks_explore_point() {
        let seed = three_step_seed();
        let plan = perturb_plan(&seed, 5, 0);
        let trunc = &plan[4];
        assert_eq!(trunc.op, PerturbOp::Truncate);
        // Truncate: marked a divergence point, and all ticks in the script are < the truncation point
        let cut = trunc.truncate_at.expect("truncate 必须标发散点");
        for (t, _) in &trunc.script {
            assert!(*t < cut, "截断后脚本只剩发散点之前的输入: tick={t} cut={cut}");
        }
    }

    #[test]
    fn empty_seed_yields_only_baseline() {
        let seed = seed_recording(&[]);
        let plan = perturb_plan(&seed, 8, 0);
        assert_eq!(plan.len(), 1, "空种子无可扰动，只回基线");
        assert_eq!(plan[0].op, PerturbOp::Baseline);
        assert!(plan[0].script.is_empty());
    }

    #[test]
    fn single_action_vocab_substitute_degrades_gracefully() {
        // Vocabulary has only one action: substitute has nothing to substitute, degrades to as-is without panicking
        let seed = seed_recording(&[(1, "only"), (2, "only")]);
        let plan = perturb_plan(&seed, 4, 0);
        let sub = &plan[3];
        assert_eq!(sub.op, PerturbOp::Substitute);
        for (_, a) in &sub.script {
            assert_eq!(a.action, "only");
        }
    }
}
