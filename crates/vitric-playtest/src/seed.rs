//! 种子式探索（设计稿三节）：拿 gate 证书录像当**种子**——它是一段「证明这局能通」的
//! 确定性输入序列。`perturb_plan` 在这条序列上做**受控变异**，每个变异是一条新脚本，
//! 喂给 [`crate::strategy::ScriptedStrategy`] 跑成一局，看是否软锁/到不了某结局。
//!
//! 为什么是「绕着已知解找破绽」而不是「从零解谜」：真正的难题谜解法是特定 N 步序列，
//! 随机/贪心解不出来（从零解任意谜是没解的 AI 难题）。所以拿开发者给的解法当起点，
//! 在它附近受控扰动——换序、走岔、改一个选择、跳一步、提前截断走岔——找开发者没堵死的破绽。
//!
//! **确定性**：所有随机决策走一个 PCG（`Pcg32::new(rng_seed)`），所以同 `(seed, n, rng_seed)`
//! 出**同一组**变异（逐条逐字节一致）。算子按固定顺序轮换，不依赖哈希集合迭代序。

use vitric_sim::{InputRecord, Pcg32, Recording};

use crate::scene_view::Action;

/// 扰动算子种类——轮换用，也带进结果供报告/调试标注「这条是怎么变出来的」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PerturbOp {
    /// 原种子，未扰动（第 0 条永远是它，作基线：应复现到种子那个结局）。
    Baseline,
    /// 去掉一条输入（跳过一步——少按了某个关键键会怎样）。
    Drop,
    /// 交换两条输入的**动作**（保留各自 tick，只换内容——换了操作顺序会怎样）。
    Swap,
    /// 把一条输入的动作换成词汇里的**另一个**动作（在分叉点走另一条/改一个选择）。
    Substitute,
    /// 在某 tick 截断，截断后交给随机策略发散（照解法走前半段，后半段乱走找岔路）。
    Truncate,
}

impl PerturbOp {
    /// 短名（报告显示用）。
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

/// 一条变异：脚本（喂 ScriptedStrategy）+ 它是怎么变出来的（算子）+ 是否截断发散。
///
/// `truncate_at`：Some(tick) 表示这条脚本只到该 tick，之后该交给随机策略发散
/// （CLI/调用方据此给 ScriptedStrategy 接一个 `then_explore`）；None=脚本放完即止。
#[derive(Debug, Clone, PartialEq)]
pub struct Perturbation {
    /// 算子标签（baseline / drop / swap / substitute / truncate）。
    pub op: PerturbOp,
    /// 变异后的脚本：(tick, 动作)，按 tick 升序。
    pub script: Vec<(u64, Action)>,
    /// 截断点：Some=脚本到此截断、之后随机发散；None=脚本放完即止。
    pub truncate_at: Option<u64>,
}

/// 从种子录像生成 n 条变异脚本（含第 0 条基线）。
///
/// - 第 0 条永远是**原种子**（`Baseline`，未扰动）——它复现到种子那个结局，是对照基准。
/// - 第 1..n 条：算子按 `drop / swap / substitute / truncate` 顺序轮换，每条用同一个 PCG
///   抽参数（选哪条 drop、换哪两条、替成哪个动作、在哪截断）。
/// - 确定性：同 `(seed 录像, n, rng_seed)` 出逐条一致的一组（PCG 单线播种，算子轮换不依赖哈希序）。
///
/// 退化：种子无输入（空录像）时只回基线一条（没东西可扰动，硬造变异没意义）。
pub fn perturb_plan(seed: &Recording, n: usize, rng_seed: u64) -> Vec<Perturbation> {
    let base_script: Vec<(u64, Action)> = seed
        .inputs
        .iter()
        .map(|r| (r.tick, Action { action: r.action.clone(), phase: r.phase.clone() }))
        .collect();

    let mut out = Vec::with_capacity(n.max(1));
    // 第 0 条：基线（原种子，未扰动）
    out.push(Perturbation { op: PerturbOp::Baseline, script: base_script.clone(), truncate_at: None });
    if n <= 1 {
        return out;
    }

    // 词汇：种子里出现过的动作名（去重，保出现序）——substitute 从这里挑「另一个动作」。
    // 用种子自带词汇而非全局规则词汇：扰动是「在已知解附近换个已知操作」，不引入种子里
    // 根本没出现过的键（那更像乱试而非「绕解法找破绽」）。
    let mut vocab: Vec<String> = Vec::new();
    for (_, a) in &base_script {
        if !vocab.iter().any(|v| v == &a.action) {
            vocab.push(a.action.clone());
        }
    }

    // 空脚本：没东西可扰动，只回基线（再多 n 也变不出花样）
    if base_script.is_empty() {
        return out;
    }

    let mut rng = Pcg32::new(rng_seed);
    // 算子轮换序：固定四个，按 (i-1) % 4 选——第 1 条 drop、第 2 条 swap、第 3 条 substitute、
    // 第 4 条 truncate，第 5 条又回 drop……保证四种算子都被均匀用到。
    const OPS: [PerturbOp; 4] =
        [PerturbOp::Drop, PerturbOp::Swap, PerturbOp::Substitute, PerturbOp::Truncate];
    for i in 1..n {
        let op = OPS[(i - 1) % OPS.len()];
        let pert = apply_op(op, &base_script, &vocab, &mut rng);
        out.push(pert);
    }
    out
}

/// 跑一个算子生成一条变异（从原始 base_script 出发——每条都基于种子原貌变一处，
/// 不在前一条变异上叠加，所以扰动彼此独立、互不污染）。
fn apply_op(
    op: PerturbOp,
    base: &[(u64, Action)],
    vocab: &[String],
    rng: &mut Pcg32,
) -> Perturbation {
    match op {
        PerturbOp::Drop => {
            // 去掉随机一条
            let idx = rng.range_i64(0, base.len() as i64 - 1) as usize;
            let mut script = base.to_vec();
            script.remove(idx);
            Perturbation { op, script, truncate_at: None }
        }
        PerturbOp::Swap => {
            let mut script = base.to_vec();
            // 至少两条才换得动；只有一条时退化成「原样」（仍标 Swap，便于对账）
            if script.len() >= 2 {
                let a = rng.range_i64(0, script.len() as i64 - 1) as usize;
                // 选一个不等于 a 的 b：从 [0, len-2] 抽，≥a 的偏移一位，确保 b != a
                let mut b = rng.range_i64(0, script.len() as i64 - 2) as usize;
                if b >= a {
                    b += 1;
                }
                // 只换动作内容，保留各自的 tick（换的是「这一刻按什么」的顺序）
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
            // 词汇里挑一个**不同**的动作名替上去；词汇只有一个动作时无从替（退化原样）
            if vocab.len() >= 2 {
                let cur = &script[idx].1.action;
                // 从「除当前动作外」的词汇里挑：先收候选再抽（候选非空，因 vocab≥2 且 cur 在 vocab 里）
                let candidates: Vec<&String> = vocab.iter().filter(|v| *v != cur).collect();
                let pick = rng.range_i64(0, candidates.len() as i64 - 1) as usize;
                script[idx].1.action = candidates[pick].clone();
            }
            Perturbation { op, script, truncate_at: None }
        }
        PerturbOp::Truncate => {
            // 在某条输入处截断：保留前缀，截断点 tick 之后交随机发散。
            // 截断点取「某条输入的 tick」——在解法的某一步之后开始走岔，比按绝对 tick 切更有意义。
            let cut_idx = rng.range_i64(0, base.len() as i64 - 1) as usize;
            let cut_tick = base[cut_idx].0;
            // 保留 tick < cut_tick 的输入（cut_tick 那一刻起就不再照脚本，改随机）
            let script: Vec<(u64, Action)> =
                base.iter().filter(|(t, _)| *t < cut_tick).cloned().collect();
            Perturbation { op, script, truncate_at: Some(cut_tick) }
        }
        PerturbOp::Baseline => {
            Perturbation { op, script: base.to_vec(), truncate_at: None }
        }
    }
}

/// 便捷：把一条录像的输入拷成 `Vec<InputRecord>`（给测试/外部构造脚本用）。
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
        // 基线脚本逐条等于种子输入
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
        // 不同 rng_seed 应给出不同的一组（至少有一条脚本不一样）
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
        // 第 0 基线，1..=4 是 drop/swap/substitute/truncate 轮换
        assert_eq!(plan[1].op, PerturbOp::Drop);
        assert_eq!(plan[2].op, PerturbOp::Swap);
        assert_eq!(plan[3].op, PerturbOp::Substitute);
        assert_eq!(plan[4].op, PerturbOp::Truncate);
    }

    #[test]
    fn drop_removes_exactly_one_input() {
        let seed = three_step_seed();
        let plan = perturb_plan(&seed, 2, 0);
        // plan[1] 是 drop：少一条
        assert_eq!(plan[1].op, PerturbOp::Drop);
        assert_eq!(plan[1].script.len(), seed.inputs.len() - 1);
    }

    #[test]
    fn swap_keeps_ticks_and_set_of_actions_but_changes_order() {
        // 找一个真把动作换了位的 rng_seed（不同 seed 抽不同 (a,b)）
        let seed = three_step_seed();
        let base: Vec<(u64, String)> =
            seed.inputs.iter().map(|r| (r.tick, r.action.clone())).collect();
        let mut found = false;
        for rs in 0..50u64 {
            // n=3：plan[2] 才是 swap（plan[1]=drop）
            let plan = perturb_plan(&seed, 3, rs);
            let swap = &plan[2];
            assert_eq!(swap.op, PerturbOp::Swap);
            // tick 序列不变
            let ticks: Vec<u64> = swap.script.iter().map(|(t, _)| *t).collect();
            assert_eq!(ticks, vec![1, 3, 5]);
            // 动作的「多重集合」不变（只是换了位）
            let mut a: Vec<&str> = swap.script.iter().map(|(_, x)| x.action.as_str()).collect();
            let mut b: Vec<&str> = base.iter().map(|(_, x)| x.as_str()).collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b, "swap 不增不减动作，只换序");
            // 找到一次真换了序的
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
            // 长度不变；每个动作仍在词汇里
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
        // 截断：标了一个发散点，且脚本里所有 tick 都 < 截断点
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
        // 词汇只有一个动作：substitute 无从替，退化原样但不 panic
        let seed = seed_recording(&[(1, "only"), (2, "only")]);
        let plan = perturb_plan(&seed, 4, 0);
        let sub = &plan[3];
        assert_eq!(sub.op, PerturbOp::Substitute);
        for (_, a) in &sub.script {
            assert_eq!(a.action, "only");
        }
    }
}
