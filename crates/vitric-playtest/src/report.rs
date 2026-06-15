//! 聚合器 + 地板报告（设计稿五节「聚合与报告」、九节性能预算）。
//!
//! 吃一批 [`LabeledResult`]（多策略 × 多 seed 跑出来的局），**一趟 O(局 × tick)** 聚合成
//! 一份可序列化的 [`Report`]：机器读 JSON + 人话读 `summary`。第 2 阶段只做几个**扎实、
//! 测得准**的维度——通关率/可达性/卡死候选/节奏/惰性动作/主导策略——不硬塞测不准的
//! （数值崩、不可达内容这些要派生量/语义，留给后续阶段）。
//!
//! 诚实标注：`stuck_clusters`（软锁）和 `inert_actions`（废动作）都是**启发式候选**，
//! 不是定论——有些游戏合法静止、有些动作合法地不产生事件。报告把它们标成「候选」，
//! 交给人复核（每条都挂得到一局可重放录像，能直接重放看）。

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use vitric_rules::Engine;
use vitric_sim::Recording;

use crate::scene_view::{Outcome, TerminalSpec};
use crate::swarm::{LabeledResult, StrategyKind};

/// 「末尾连续多少 tick 状态哈希完全不变」算冻结候选的阈值（设计稿：K 默认 60）。
pub const DEFAULT_FREEZE_K: usize = 60;

/// 内建事件名：这些事件不算「某个 input 动作引发了规则响应」——它们是引擎机制，
/// 跟某个 input 动作有没有被规则接住无关：
/// - `start`：sim 在 tick 0 无条件发的生命周期事件（每局都有，与动作无关）；
/// - `input`：动作本身的事件，不是「响应」；
/// - `collision`：内建碰撞系统发的，不是 input 规则的产物；
/// - 其余是序列/动画/UI/场景这些引擎子系统的通用动词。
///
/// 判惰性动作时把它们排除——只有规则**自定义 emit** 的事件才算「这个动作引发了响应」。
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

/// 结局分布 + 通关率。
#[derive(Debug, Clone, Serialize)]
pub struct OutcomeDistribution {
    pub win: usize,
    pub lose: usize,
    pub timeout: usize,
    pub total: usize,
    /// 通关率 = win / total（total=0 时为 0）。
    pub win_rate: f64,
}

/// 可达性：并起来触发过的终止/里程碑事件 + 「swarm 通不了」信号。
#[derive(Debug, Clone, Serialize)]
pub struct Reachability {
    /// 所有局并集里触发过的终止/里程碑事件名（排序，确定）。
    pub reached_events: Vec<String>,
    /// 0 局 Win → true（最强信号之一：声明了能赢但 swarm 谁都没赢到）。
    pub unbeatable_by_swarm: bool,
}

/// 结局覆盖（设计稿三节种子探索验收的核心：不可达结局）。
///
/// **「声明结局集合」从哪来**（注释说明，见任务三）：扫规则里所有 `emit` 动作的事件名，
/// 凡命中 `TerminalSpec`（win/lose 命名集合或 `ending-*` 前缀）的，就是这游戏**声明它能产出**
/// 的结局。这一步是静态扫规则——所以即便某个结局在所有局里**一次都没被 emit 过**，我们照样
/// 知道它「被声明了」，从而能判它**不可达**（声明了但 0 局触达）。光看 `fired_events`（运行时
/// 真发过的事件）做不到这点：没发过的事件压根不在里面，会被「没声明」和「声明了但没到」混为一谈。
#[derive(Debug, Clone, Serialize)]
pub struct EndingCoverage {
    /// 这游戏声明能产出的全部结局事件名（扫规则 emit ∩ TerminalSpec，排序去重）。
    pub declared_endings: Vec<String>,
    /// 至少被一局触达过的结局（declared ∩ 任意局 fired_events，排序）。
    pub reached_endings: Vec<String>,
    /// **声明了但 0 局可达的结局**——种子探试的头号靶子（设计稿三节「不可达结局」）。
    pub unreachable_endings: Vec<String>,
}

/// 一簇软锁候选：一批局在同一个「冻结状态哈希」上卡死且没到终止。
#[derive(Debug, Clone, Serialize)]
pub struct StuckCluster {
    /// 冻结时的状态哈希（十六进制字面）——同一死态的局聚到同一桶。
    pub frozen_hash: String,
    /// 命中这个死态的局数。
    pub hits: usize,
    /// 该死态对应策略/seed（拿一局当代表，能据此重放）。
    pub sample_strategy: String,
    pub sample_seed: u64,
    /// 一条可重放录像（该桶里的代表局）——「每条结论挂可重放录像」。
    pub sample_recording: Recording,
}

/// 节奏：到终止的 tick 分布（Timeout 局单列，不混进「到终止用了多久」）。
#[derive(Debug, Clone, Serialize)]
pub struct Pacing {
    /// 到终止（Win/Lose）的局的 tick：min / 中位 / max。无终止局时为 None。
    pub terminated_min: Option<u64>,
    pub terminated_median: Option<u64>,
    pub terminated_max: Option<u64>,
    /// 终止 tick 的直方桶（固定 5 桶，按 [min,max] 等宽切；标签是桶上界）。
    pub histogram: Vec<HistogramBucket>,
    /// Timeout 局数（没到终止，单列不进上面的分布）。
    pub timeout_count: usize,
}

/// 一个直方桶。
#[derive(Debug, Clone, Serialize)]
pub struct HistogramBucket {
    /// 桶上界（tick）。
    pub upper: u64,
    pub count: usize,
}

/// 单个策略的表现（分组聚合）。
#[derive(Debug, Clone, Serialize)]
pub struct StrategyStats {
    pub strategy: String,
    pub sessions: usize,
    pub win_rate: f64,
    /// 该策略通关局的中位 tick（无通关局为 None）。
    pub median_win_ticks: Option<u64>,
}

/// 主导策略：分策略表现 + 「某策略碾压」标记。
#[derive(Debug, Clone, Serialize)]
pub struct DominantStrategy {
    pub per_strategy: Vec<StrategyStats>,
    /// 若某策略通关率 ≥2× 次优且样本足（每策略 ≥4 局），标出它的名字；否则 None。
    pub dominant: Option<String>,
}

/// 地板报告（机器 JSON + 人话 summary）。第 2 阶段的几个扎实维度。
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub sessions: usize,
    pub outcome_distribution: OutcomeDistribution,
    pub reachability: Reachability,
    /// 结局覆盖：声明了哪些结局、到了哪些、哪些 0 局可达（不可达结局）。
    /// 没传引擎（不知道声明集合）时为 None——空 declared 和「没算过」是两回事。
    pub ending_coverage: Option<EndingCoverage>,
    /// 软锁候选（诚实标注：候选，不是定论）。
    pub stuck_clusters: Vec<StuckCluster>,
    pub pacing: Pacing,
    /// 疑似惰性动作（轻量启发式候选）。
    pub inert_actions: Vec<String>,
    pub dominant_strategy: DominantStrategy,
    /// 人话摘要：一两句把上面最关键的几条说清。
    pub summary: String,
}

/// 聚合入口：一批标签结果 → 一份报告。用默认冻结阈值 K，不算结局覆盖
/// （不传引擎=不知道声明结局集合，`ending_coverage` 为 None）。
pub fn aggregate(results: &[LabeledResult]) -> Report {
    aggregate_inner(results, DEFAULT_FREEZE_K, None)
}

/// 聚合（可调冻结阈值 K，给测试用）。一趟扫，不做平方比对。
pub fn aggregate_with_freeze_k(results: &[LabeledResult], freeze_k: usize) -> Report {
    aggregate_inner(results, freeze_k, None)
}

/// 聚合 + 结局覆盖（种子探索专用）。`engine`/`terminal` 用来扫规则声明的结局集合，
/// 报告含 `ending_coverage`（哪些声明结局不可达）。这是设计稿三节验收的入口。
pub fn aggregate_with_endings(
    results: &[LabeledResult],
    engine: &Engine,
    terminal: &TerminalSpec,
) -> Report {
    let declared = declared_endings(engine, terminal);
    aggregate_inner(results, DEFAULT_FREEZE_K, Some(declared))
}

/// 聚合内核：declared=Some 时算结局覆盖，None 时跳过。
fn aggregate_inner(
    results: &[LabeledResult],
    freeze_k: usize,
    declared: Option<Vec<String>>,
) -> Report {
    let outcome_distribution = aggregate_outcomes(results);
    let reachability = aggregate_reachability(results, &outcome_distribution);
    let ending_coverage = declared.map(|d| aggregate_ending_coverage(d, results));
    let stuck_clusters = aggregate_stuck(results, freeze_k);
    let pacing = aggregate_pacing(results);
    let inert_actions = aggregate_inert(results);
    let dominant_strategy = aggregate_dominant(results);
    let summary = build_summary(
        &outcome_distribution,
        &reachability,
        &ending_coverage,
        &stuck_clusters,
        &inert_actions,
        &dominant_strategy,
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
        summary,
    }
}

/// 扫规则里所有 `emit` 动作的事件名，凡命中 TerminalSpec（win/lose 命名或 ending 前缀）的
/// 就是「声明的结局」。排序去重——确定且便于对账。**静态扫规则**，不看运行时是否真发过，
/// 所以从没被 emit 的结局也能被认定为「声明了」（这正是判不可达的前提，见 EndingCoverage 注释）。
fn declared_endings(engine: &Engine, terminal: &TerminalSpec) -> Vec<String> {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for rule in &engine.rules.rules {
        for action in &rule.actions {
            // 动作是 JSON 对象；emit 动作形如 {"emit": "<事件名>", "data": {...}}
            if let Some(name) = action.get("emit").and_then(|v| v.as_str()) {
                if terminal.classify(name).is_some() {
                    declared.insert(name.to_string());
                }
            }
        }
    }
    declared.into_iter().collect()
}

/// 结局覆盖：declared ∩ 任意局 fired_events = 触达；declared − 触达 = 不可达。
fn aggregate_ending_coverage(declared: Vec<String>, results: &[LabeledResult]) -> EndingCoverage {
    // 所有局触发过的事件并集（含终止与里程碑）
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
    // 并集：所有局触发过的事件名（BTreeSet 自动排序去重 = 确定）
    let mut reached: BTreeSet<String> = BTreeSet::new();
    for lr in results {
        for ev in &lr.result.fired_events {
            reached.insert(ev.clone());
        }
    }
    // unbeatable：有局但 0 局 Win（没局时不下这个论断——没数据不是「打不过」）
    let unbeatable_by_swarm = dist.total > 0 && dist.win == 0;
    Reachability { reached_events: reached.into_iter().collect(), unbeatable_by_swarm }
}

/// 软锁候选：每局看末尾连续相同的 state_hash 跑了多长。≥K 且没到终止 → 冻结候选。
/// 按「冻结时的 hash」分桶，每桶命中局数 + 一条代表录像。
fn aggregate_stuck(results: &[LabeledResult], freeze_k: usize) -> Vec<StuckCluster> {
    // 桶：frozen_hash -> (命中数, 代表局)。BTreeMap 保证输出顺序确定。
    let mut buckets: BTreeMap<u64, (usize, &LabeledResult)> = BTreeMap::new();
    for lr in results {
        // Timeout 才可能是软锁；Win/Lose 是正常到了尽头，不算卡死
        if lr.result.outcome != Outcome::Timeout {
            continue;
        }
        let trace = &lr.result.state_trace;
        if trace.is_empty() {
            continue;
        }
        // 从末尾往前数：末值连续重复了多少 tick（末尾冻结长度）
        let last = *trace.last().expect("非空");
        let mut run = 0usize;
        for &h in trace.iter().rev() {
            if h == last {
                run += 1;
            } else {
                break;
            }
        }
        if run >= freeze_k {
            let entry = buckets.entry(last).or_insert((0, lr));
            entry.0 += 1;
            // 代表局保第一个遇到的（BTreeMap 输出序确定，命中数累加）
        }
    }
    buckets
        .into_iter()
        .map(|(hash, (hits, rep))| StuckCluster {
            frozen_hash: format!("{hash:#018x}"),
            hits,
            sample_strategy: rep.spec.strategy_kind.name().to_string(),
            sample_seed: rep.spec.seed,
            sample_recording: rep.result.recording.clone(),
        })
        .collect()
}

fn aggregate_pacing(results: &[LabeledResult]) -> Pacing {
    // 终止局（Win/Lose）的 tick，排序求 min/median/max + 直方
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

/// 固定 5 桶等宽直方。min==max（全同值）时退化成单桶。
fn build_histogram(sorted: &[u64], min: u64, max: u64) -> Vec<HistogramBucket> {
    const N_BUCKETS: u64 = 5;
    if min == max {
        return vec![HistogramBucket { upper: max, count: sorted.len() }];
    }
    let span = max - min;
    let mut counts = vec![0usize; N_BUCKETS as usize];
    for &t in sorted {
        // 落桶：(t-min)/span 映射到 [0, N_BUCKETS)，max 归到最后一桶
        let b = ((t - min) * N_BUCKETS / (span + 1)).min(N_BUCKETS - 1) as usize;
        counts[b] += 1;
    }
    (0..N_BUCKETS)
        .map(|i| {
            // 桶上界：第 i 桶覆盖 [min + i*step, min + (i+1)*step)
            let upper = min + (i + 1) * (span + 1) / N_BUCKETS;
            HistogramBucket { upper, count: counts[i as usize] }
        })
        .collect()
}

/// 惰性动作候选：在所有局里被注入过、但从没和任何**非内建**事件同局出现的 input 动作。
///
/// 诚实的局限（轻量启发式，不是数据流分析）：
/// - 动作词汇取「所有局录像里实际注入过的 action 并集」——coverage 策略保证每个声明的
///   动作都被注入到，所以这个并集 ≈ 完整词汇；但若某动作连一次都没被任何策略注入（理论上
///   coverage 会注入），它不会出现在这里。
/// - 「同局出现非内建事件」是粗判：只要那局有任何非内建事件，就认为该局的动作「可能」引发了
///   响应——不追因到具体哪个动作。所以一个真废动作只有在「它**单独**被注入、且那些局没有
///   任何非内建事件」时才会被逮到。dead-action 埋雷正是这种构造（声明输入但 rules 没人接）。
fn aggregate_inert(results: &[LabeledResult]) -> Vec<String> {
    // 词汇：所有局注入过的动作并集
    let mut vocab: BTreeSet<String> = BTreeSet::new();
    // 「引发过非内建事件」的动作集合：某局有非内建事件 → 该局注入过的动作都记一笔可能有响应
    let mut responsive: BTreeSet<String> = BTreeSet::new();
    for lr in results {
        let actions_this: BTreeSet<&str> =
            lr.result.recording.inputs.iter().map(|r| r.action.as_str()).collect();
        for a in &actions_this {
            vocab.insert(a.to_string());
        }
        let has_non_builtin =
            lr.result.fired_events.iter().any(|e| !is_builtin_event(e));
        if has_non_builtin {
            for a in &actions_this {
                responsive.insert(a.to_string());
            }
        }
    }
    // 惰性 = 词汇里、从没出现在「有响应的局」里的动作
    vocab.difference(&responsive).cloned().collect()
}

fn aggregate_dominant(results: &[LabeledResult]) -> DominantStrategy {
    // 分组：strategy_kind -> (局数, win 数, win 局的 tick 列表)
    let mut groups: BTreeMap<&'static str, (usize, usize, Vec<u64>)> = BTreeMap::new();
    // 先确保三种策略都有桶（便于稳定输出，即便某策略 0 局）
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
    // 主导判定：通关率最高的策略 ≥2× 次优，且双方样本都 ≥4 局（样本足才下论断）
    let dominant = find_dominant(&per_strategy);
    DominantStrategy { per_strategy, dominant }
}

/// 主导：按 win_rate 降序，头名 ≥2× 次名且头名样本 ≥4 局；次名 win_rate=0 时只要头名
/// 真有通关且样本足也算碾压（0 的 2 倍还是 0，单独处理）。
fn find_dominant(stats: &[StrategyStats]) -> Option<String> {
    const MIN_SAMPLE: usize = 4;
    let mut ranked: Vec<&StrategyStats> =
        stats.iter().filter(|s| s.sessions >= MIN_SAMPLE).collect();
    if ranked.len() < 2 {
        return None; // 不足两个够样本的策略，没法比「碾压」
    }
    ranked.sort_by(|a, b| b.win_rate.partial_cmp(&a.win_rate).expect("win_rate 非 NaN"));
    let top = ranked[0];
    let second = ranked[1];
    if top.win_rate <= 0.0 {
        return None; // 头名都没赢，谈不上主导
    }
    if top.win_rate >= 2.0 * second.win_rate {
        Some(top.strategy.clone())
    } else {
        None
    }
}

fn build_summary(
    dist: &OutcomeDistribution,
    reach: &Reachability,
    ending: &Option<EndingCoverage>,
    stuck: &[StuckCluster],
    inert: &[String],
    dominant: &DominantStrategy,
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
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_view::Outcome;
    use crate::session::SessionResult;
    use crate::swarm::{SessionSpec, StrategyKind};
    use vitric_sim::{InputRecord, Recording};

    /// 造一条带 telemetry 的 LabeledResult（喂聚合器）。
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
            },
        }
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
        // 没局 = 没数据，不下「打不过」论断
        let rep = aggregate(&[]);
        assert!(!rep.reachability.unbeatable_by_swarm);
    }

    #[test]
    fn stuck_clusters_groups_frozen_tail() {
        // 两局都在末尾连续 >K tick 哈希冻结成同一个 hash（=999），且都 Timeout
        let mut trace_a = vec![1, 2, 3];
        trace_a.extend(std::iter::repeat_n(999u64, 70)); // 末尾 70 tick 冻在 999
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
        // 冻结不够 K，不算
        let mut short = vec![1, 2];
        short.extend(std::iter::repeat_n(7u64, 10));
        // 冻结够长但已 Win（正常到尽头，不算软锁）
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
        // dead 动作：被注入但所在局没有任何非内建事件
        // live 动作：所在局有非内建事件 noop
        let r = vec![
            // 一局注入 left，触发了 noop（非内建）→ left 有响应
            labeled(StrategyKind::Coverage, 0, Outcome::Timeout, 5, vec![], vec!["noop"], vec!["left"]),
            // 一局注入 dead，只有内建 input 事件 → dead 无响应
            labeled(StrategyKind::Coverage, 1, Outcome::Timeout, 5, vec![], vec!["input"], vec!["dead"]),
        ];
        let rep = aggregate(&r);
        assert!(rep.inert_actions.contains(&"dead".to_string()), "{:?}", rep.inert_actions);
        assert!(!rep.inert_actions.contains(&"left".to_string()));
    }

    #[test]
    fn dominant_strategy_flagged_when_one_crushes() {
        // coverage 4 局全通关（win_rate 1.0），random 4 局全超时（0.0）→ coverage 碾压
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
        // 两策略通关率接近（不到 2×）→ 不标主导
        let mut r = Vec::new();
        for seed in 0..4u64 {
            // coverage 3/4 win, random 2/4 win：1.5× < 2×
            r.push(labeled(StrategyKind::Coverage, seed, if seed < 3 { Outcome::Win } else { Outcome::Timeout }, 10, vec![], vec![], vec![]));
            r.push(labeled(StrategyKind::Random, seed, if seed < 2 { Outcome::Win } else { Outcome::Timeout }, 10, vec![], vec![], vec![]));
        }
        let rep = aggregate(&r);
        assert_eq!(rep.dominant_strategy.dominant, None);
    }

    #[test]
    fn dominant_none_when_sample_too_small() {
        // 样本不足 4 局：不下主导论断
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

    /// 造一个声明了若干结局事件（rules 里 emit）的引擎。
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
        // 引擎声明能 emit ending-good 和 ending-bad；运行里只触达过 ending-bad
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
        // 引擎 emit 了一个非结局事件 milestone 和一个结局 ending-x：只 ending-x 算声明结局
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
}
