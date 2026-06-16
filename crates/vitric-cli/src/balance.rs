//! `vitric balance`：自动配平。调一个数值旋钮，用 agent 集群试玩反复跑，二分搜索到
//! 让通关率落进目标区间的旋钮值——把"试玩报告"从只报问题升级成**闭环配平**。
//!
//! 三块拼起来：
//! - **旋钮寻址**（[`KnobAddr`]）：`<相对文件路径>#<json-pointer>` 指向项目某文件里的一个数字
//!   （RFC6901 JSON Pointer）。解析时把该文件读成 JSON、按 pointer 取到那个 number。
//! - **应用旋钮**（[`apply_knob_to_temp`]）：每个候选值，**把整个项目拷到临时目录**、只改那一个
//!   文件的那一个 pointer、对临时副本跑试玩。**绝不写用户项目目录**；临时副本用完即删
//!   （[`TempProject`] 的 Drop 负责清理）。
//! - **搜索**（[`search`]）：先在 range 两端各跑一次定方向（通关率随旋钮是升是降），假设单调
//!   做二分；两端方向不一致 / 中点不符合单调预期就退化成**粗线扫**，报最接近目标的值 + 整条曲线，
//!   并诚实标注"通关率对这个旋钮不单调"。
//!
//! **确定性**：试玩本就确定（同旋钮值 → 同通关率），搜索路径只由项目+参数决定——同项目同参
//! 出同 found_value + 同 samples。`search` 把"怎么评估一个候选值"抽象成注入的闭包，纯算法
//! 部分（二分/线扫）能脱离 boot 单测；真跑时闭包指向 [`evaluate`]（拷临时副本→跑 swarm→拿
//! win_rate）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use vitric_data::Project;
use vitric_playtest::{
    aggregate_with_endings_and_declared, default_plan, run_swarm_with_config, PlaytestConfig,
    TerminalSpec,
};

use crate::runtime::Runtime;

/// 旋钮地址：项目里某文件（相对路径）+ 指向一个数字的 JSON Pointer（RFC6901，如 `/rules/3/do/0/to`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnobAddr {
    /// 相对项目根的文件路径（如 `rules/game.json`）。
    pub rel_file: String,
    /// RFC6901 JSON Pointer（如 `/rules/3/do/0/to`），指向该文件里的一个 number。
    pub pointer: String,
}

impl KnobAddr {
    /// 解析 `--knob` 参数：`<相对文件路径>#<json-pointer>`。`#` 只切第一个（pointer 里不会有 `#`，
    /// 但文件名理论上可能有 `#`——按"第一个 # 之后全是 pointer"约定，简单可预测）。
    pub fn parse(spec: &str) -> Result<KnobAddr, String> {
        let (rel_file, pointer) = spec.split_once('#').ok_or_else(|| {
            format!(
                "--knob 格式应为 <相对文件路径>#<json-pointer>，拿到 {spec:?}\
                 （例：rules/game.json#/rules/3/do/0/to）"
            )
        })?;
        if rel_file.is_empty() {
            return Err(format!("--knob 缺少文件路径：{spec:?}"));
        }
        if pointer.is_empty() {
            // 空 pointer（RFC6901 里指整个文档）对配平没意义——必须指到一个数字。
            return Err(format!("--knob 缺少 JSON Pointer（# 后为空）：{spec:?}"));
        }
        if !pointer.starts_with('/') {
            return Err(format!(
                "JSON Pointer 必须以 / 开头（RFC6901），拿到 {pointer:?}"
            ));
        }
        Ok(KnobAddr { rel_file: rel_file.to_string(), pointer: pointer.to_string() })
    }
}

/// 读出旋钮当前值：把 `rel_file` 读成 JSON，按 pointer 取到那个数字。
/// pointer 解析不到 / 指到的不是数字都明确报错（带路径，vitric check 风格）。
pub fn read_knob(root: &Path, addr: &KnobAddr) -> Result<f64, String> {
    let path = root.join(&addr.rel_file);
    let doc = load_json(&path)?;
    pointer_get_number(&doc, &addr.pointer)
        .ok_or_else(|| format!("{} 里 pointer {} 没指到一个数字（越界或非 number）", addr.rel_file, addr.pointer))
}

/// 在内存的 JSON 文档里把 pointer 指向的数字改成 `value`。pointer 解析不到 / 原值不是数字
/// 都明确报错——配平只改"本来就是数字"的旋钮，不凭空建字段、不覆盖非数字。
pub fn set_pointer_number(doc: &mut Value, pointer: &str, value: f64) -> Result<(), String> {
    // 先确认原值存在且是数字（越界/非数字立刻报错，不静默新建）。
    let cur = doc
        .pointer(pointer)
        .ok_or_else(|| format!("JSON Pointer {pointer} 越界（指不到任何值）"))?;
    if !cur.is_number() {
        return Err(format!("JSON Pointer {pointer} 指到的不是数字（是 {cur}）——配平只调数值旋钮"));
    }
    let slot = doc
        .pointer_mut(pointer)
        .ok_or_else(|| format!("JSON Pointer {pointer} 可读不可写（结构异常）"))?;
    *slot = number_value(value);
    Ok(())
}

/// 把候选值落成 JSON 数字：整数值（如 8.0）写成整数 `8`（不写 `8.0`），其余写浮点。
/// 旋钮多半是整数阈值（"敌人攻击=8"），写整数能让临时副本的 JSON 跟用户原文同形、好对账。
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9.007_199_254_740_992e15 {
        // 落进 i64 安全整数范围的整数值写成整数字面
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// 取 pointer 指向的数字（不存在或非数字返回 None）。
fn pointer_get_number(doc: &Value, pointer: &str) -> Option<f64> {
    doc.pointer(pointer).and_then(|v| v.as_f64())
}

fn load_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} 解析 JSON 失败: {e}", path.display()))
}

/// 一个临时项目副本：拷整个项目目录到系统临时区（按进程 id + 计数隔离），
/// Drop 时整目录删掉。**绝不动用户项目目录**——配平的所有改写都落在这份副本里。
struct TempProject {
    dir: PathBuf,
}

/// 进程内全局递增计数：保证每份临时副本目录名**全局唯一**——即便多个评估/多个测试在同进程
/// 并发跑（同 pid），也绝不撞目录（撞了会互相清掉对方的副本，读到半成品）。`n` 只是调用方传来
/// 的对账编号，不参与唯一性。
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl TempProject {
    /// 把 `src` 项目整树拷到 `temp_dir()/vitric-balance-<pid>-<全局序号>-<n>/`。
    /// 全局序号保证唯一，所以不需要"先删旧目录"（每次都是全新路径）。
    fn clone_from(src: &Path, n: u64) -> Result<TempProject, String> {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("vitric-balance-{}-{}-{}", std::process::id(), seq, n));
        copy_dir_recursive(src, &dir)?;
        Ok(TempProject { dir })
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        // 尽力删除；删不掉不 panic（析构里 panic 会掩盖真错），临时目录残留无害。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 递归拷贝目录（只拷文件和子目录，符号链接按其指向当普通文件拷——项目里不该有链接）。
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("建临时目录 {} 失败: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读目录 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("遍历 {} 失败: {e}", src.display()))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| format!("读类型 {} 失败: {e}", from.display()))?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("拷文件 {} -> {} 失败: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// 一次评估的参数（怎么跑 swarm 拿通关率）。
#[derive(Debug, Clone)]
pub struct EvalParams {
    pub sessions: u64,
    pub max_ticks: u64,
    pub seed: u64,
}

/// 评估一个候选旋钮值 → 通关率（0..1）。
///
/// 流程：把项目拷到临时副本 → 改副本里 knob 指向的那个数字为 `value` → boot 临时副本跑
/// swarm（default_plan 默认策略组）→ 聚合拿 `outcome_distribution.win_rate` → 临时副本 Drop 删掉。
/// `n` 用于临时目录命名隔离（同进程多次评估不撞目录）。**绝不写用户项目目录**。
pub fn evaluate(src: &Path, addr: &KnobAddr, value: f64, params: &EvalParams, n: u64) -> Result<f64, String> {
    let temp = TempProject::clone_from(src, n)?;

    // 改临时副本里那一个文件的那一个 pointer
    let target_file = temp.dir.join(&addr.rel_file);
    let mut doc = load_json(&target_file)?;
    set_pointer_number(&mut doc, &addr.pointer, value)?;
    let serialized = serde_json::to_string_pretty(&doc).expect("旋钮文档可序列化");
    std::fs::write(&target_file, serialized)
        .map_err(|e| format!("写临时旋钮文件 {} 失败: {e}", target_file.display()))?;

    // 跑 swarm 拿通关率（和 gate 的 playtest 门同口径：default_plan 默认策略组 + config + 清单 must_emit）
    let win_rate = run_swarm_win_rate(&temp.dir, params)?;
    // temp 在这里 Drop，临时副本删掉
    Ok(win_rate)
}

/// 对一个（临时）项目目录跑 default_plan swarm，聚合出通关率。
/// 口径对齐 cmd_playtest / gate 的 playtest 门：playtest.json 覆盖视图、清单 must_emit 并进 win 集合、
/// 声明了 goal 自动掺前瞻。每局自己 boot（QuickJS 非 Send，运行时不跨线程）。
fn run_swarm_win_rate(dir: &Path, params: &EvalParams) -> Result<f64, String> {
    let config = PlaytestConfig::load(dir)?.unwrap_or_default();
    let manifest_must_emit: Vec<String> = match Project::load(dir) {
        Ok(project) => project
            .manifest
            .gates
            .as_ref()
            .map(|g| g.playthroughs.iter().map(|p| p.must_emit.clone()).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    }
    .with_manifest_must_emit(&manifest_must_emit);

    // 默认策略组 swarm（声明了 goal 自动掺前瞻；没声明完全不变）——任务要求的"默认 swarm 默认组"。
    let plan = default_plan(params.sessions, params.seed, params.max_ticks, terminal.clone(), config.goal.is_some());
    let factory = || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let results = run_swarm_with_config(factory, &plan, &config, threads)?;
    let (_, rt) = Runtime::boot(dir)?;
    let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
    Ok(report.outcome_distribution.win_rate)
}

/// 目标通关率区间 [lo, hi]（闭区间）。
#[derive(Debug, Clone, Copy)]
pub struct TargetBand {
    pub lo: f64,
    pub hi: f64,
}

impl TargetBand {
    /// 解析 `lo:hi`（如 `0.4:0.7`）。
    pub fn parse(s: &str) -> Result<TargetBand, String> {
        let (lo, hi) = parse_pair(s, "--target-clear-rate")?;
        if lo > hi {
            return Err(format!("--target-clear-rate 下限 {lo} 不能大于上限 {hi}"));
        }
        if !(0.0..=1.0).contains(&lo) || !(0.0..=1.0).contains(&hi) {
            return Err(format!("--target-clear-rate 必须在 [0,1] 内，拿到 {lo}:{hi}"));
        }
        Ok(TargetBand { lo, hi })
    }
    /// 命中：通关率落进 [lo,hi]（含端点）。
    pub fn contains(&self, rate: f64) -> bool {
        rate >= self.lo && rate <= self.hi
    }
    /// 区间中点（线扫"最接近"用它当距离参照——离带子越近越好；带内距离 0）。
    fn distance(&self, rate: f64) -> f64 {
        if self.contains(rate) {
            0.0
        } else if rate < self.lo {
            self.lo - rate
        } else {
            rate - self.hi
        }
    }
}

/// 旋钮搜索区间 [min, max]。
#[derive(Debug, Clone, Copy)]
pub struct KnobRange {
    pub min: f64,
    pub max: f64,
}

impl KnobRange {
    /// 解析 `min:max`（如 `0:50`）。
    pub fn parse(s: &str) -> Result<KnobRange, String> {
        let (min, max) = parse_pair(s, "--range")?;
        if min >= max {
            return Err(format!("--range 下限 {min} 必须严格小于上限 {max}"));
        }
        Ok(KnobRange { min, max })
    }
}

fn parse_pair(s: &str, flag: &str) -> Result<(f64, f64), String> {
    let (a, b) = s
        .split_once(':')
        .ok_or_else(|| format!("{flag} 格式应为 <下限>:<上限>，拿到 {s:?}"))?;
    let a: f64 = a.trim().parse().map_err(|e| format!("{flag} 下限解析失败: {e}"))?;
    let b: f64 = b.trim().parse().map_err(|e| format!("{flag} 上限解析失败: {e}"))?;
    Ok((a, b))
}

/// 一个采样点：旋钮值 → 通关率。
/// （vitric-cli 不直接依赖 serde derive，输出时用 [`Sample::to_json`] 手工序列化进 json! 报告。）
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub value: f64,
    pub clear_rate: f64,
}

impl Sample {
    /// 序列化成 `{"value":.., "clear_rate":..}`（喂 json! 报告的 samples 数组）。
    fn to_json(self) -> Value {
        serde_json::json!({ "value": self.value, "clear_rate": self.clear_rate })
    }
}

/// 搜索结果（喂 JSON 输出）。
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    /// 报出来的旋钮值（命中=落进目标带的值；没命中=线扫里最接近目标带的值）。
    pub found_value: f64,
    /// 该值对应的通关率。
    pub found_clear_rate: f64,
    /// 是否命中目标带。
    pub in_target: bool,
    /// 评估次数（每次 = 一轮 swarm 试玩）。
    pub iterations: usize,
    /// 所有采样点（按评估顺序，确定可复现）。
    pub samples: Vec<Sample>,
    /// 一句人话/诚实标注（如"通关率对这个旋钮不单调，给的是线扫最优"）。
    pub note: String,
}

/// 收敛阈值：二分区间宽度缩到 range 跨度的这个比例以下仍没命中，就停（避免无限二分）。
const CONVERGE_FRAC: f64 = 1e-3;

/// 二分 + 非单调线扫兜底的配平搜索。
///
/// `eval(value, n) -> clear_rate`：评估一个候选旋钮值的通关率（`n` 是第几次评估，用于临时目录隔离/对账）。
/// 把评估抽象成闭包，让纯算法（方向判定/二分/线扫）能脱离 boot 单测；真跑时闭包指向 [`evaluate`]。
///
/// 算法：
/// 1. 先评估 range 两端 `min`、`max` 定方向：通关率随旋钮增大是升（rate(max) > rate(min)）还是降。
///    端点本身命中目标带就直接返回（少跑一轮）。
/// 2. **两端方向明确**（一端在带上方、另一端在带下方，按单调假设目标带夹在中间）：二分。
///    每步取中点评估，命中即停；按方向收窄区间；区间宽度缩到很小仍没命中也停（报最接近的端点采样）。
/// 3. **两端方向不一致 / 都在带同侧**（单调假设不成立或区间不含解）：退化**粗线扫**——在 range 上
///    等距采 `max_iters` 个点，报最接近目标带的那个值 + 整条曲线，note 诚实标注。
pub fn search<F>(
    range: KnobRange,
    target: TargetBand,
    max_iters: usize,
    mut eval: F,
) -> Result<SearchOutcome, String>
where
    F: FnMut(f64, u64) -> Result<f64, String>,
{
    let mut samples: Vec<Sample> = Vec::new();
    let mut n: u64 = 0;
    // 评估一个值，记进 samples（去重：同值不重复评估，复用已记结果——确定性下同值同率）。
    // 闭包借不动 samples（要可变），所以用函数式手写：先查缓存，没有再 eval。
    macro_rules! eval_cached {
        ($val:expr) => {{
            let v: f64 = $val;
            if let Some(s) = samples.iter().find(|s| s.value == v) {
                s.clear_rate
            } else {
                let r = eval(v, n)?;
                n += 1;
                samples.push(Sample { value: v, clear_rate: r });
                r
            }
        }};
    }

    let rate_min = eval_cached!(range.min);
    if target.contains(rate_min) {
        return Ok(finish(range.min, rate_min, true, samples, "range 下端即命中目标带".to_string()));
    }
    let rate_max = eval_cached!(range.max);
    if target.contains(rate_max) {
        return Ok(finish(range.max, rate_max, true, samples, "range 上端即命中目标带".to_string()));
    }

    // 单调假设下能二分的前提：两端把目标带"夹住"——一端的通关率在带上方、另一端在带下方。
    // band 中点当参照：哪端 rate 高、哪端低，且二者分居 target 两侧。
    let min_above = rate_min > target.hi;
    let max_above = rate_max > target.hi;
    let min_below = rate_min < target.lo;
    let max_below = rate_max < target.lo;
    let bracketed = (min_above && max_below) || (min_below && max_above);

    if bracketed {
        // 方向：rate 随旋钮增大是降（min 高 max 低）还是升。
        let ascending = rate_max > rate_min;
        binary_search(range, target, max_iters, ascending, &mut samples, &mut n, eval)
    } else {
        // 单调假设不成立或区间不含解：线扫兜底。把已评估的两端也带进去（不浪费）。
        line_scan(range, target, max_iters, samples, n, eval)
    }
}

/// 二分核（已知两端把目标带夹住、方向 = ascending）。命中即停；区间收敛到极小仍没命中也停
/// （此时报区间内最接近目标带的采样——通常是最后一个中点）。
#[allow(clippy::too_many_arguments)]
fn binary_search<F>(
    range: KnobRange,
    target: TargetBand,
    max_iters: usize,
    ascending: bool,
    samples: &mut Vec<Sample>,
    n: &mut u64,
    mut eval: F,
) -> Result<SearchOutcome, String>
where
    F: FnMut(f64, u64) -> Result<f64, String>,
{
    let span = range.max - range.min;
    let mut lo = range.min;
    let mut hi = range.max;
    // 两端已在 samples 里（search 评估过），二分从中点开始。预算扣掉已用的两端评估。
    let budget = max_iters.saturating_sub(samples.len());
    for _ in 0..budget {
        if (hi - lo).abs() <= span * CONVERGE_FRAC {
            break;
        }
        let mid = lo + (hi - lo) / 2.0;
        // 同值已评估过就复用（确定性下同值同率），不重复跑一轮 swarm
        let rate = if let Some(s) = samples.iter().find(|s| s.value == mid) {
            s.clear_rate
        } else {
            let r = eval(mid, *n)?;
            *n += 1;
            samples.push(Sample { value: mid, clear_rate: r });
            r
        };
        if target.contains(rate) {
            return Ok(finish(mid, rate, true, std::mem::take(samples), "二分命中目标带".to_string()));
        }
        // rate 太高 → 要往"通关率更低"的方向走；太低 → 往更高的方向走。
        // ascending（rate 随旋钮升）时：rate 高就把 hi 拉到 mid（减小旋钮降低通关率）。
        // descending 时方向相反。
        let too_high = rate > target.hi;
        if ascending == too_high {
            // ascending && 太高 → 降旋钮（hi=mid）；descending && 太低 → 也降旋钮（hi=mid）
            hi = mid;
        } else {
            lo = mid;
        }
    }
    // 没命中：报采样里离目标带最近的那个值（诚实给最优近似）。
    let best = closest(samples, target);
    Ok(finish(
        best.value,
        best.clear_rate,
        false,
        std::mem::take(samples),
        format!(
            "二分收敛到区间 [{lo:.6},{hi:.6}] 仍无值落进目标带 [{:.3},{:.3}]，给的是最接近的旋钮值",
            target.lo, target.hi
        ),
    ))
}

/// 线扫兜底：在 range 上等距采 `points` 个点（含两端），报最接近目标带的值 + 整条曲线。
/// 已评估的两端复用（在 samples 里）。诚实标注"通关率对这个旋钮不单调/区间不含解，给的是线扫最优"。
fn line_scan<F>(
    range: KnobRange,
    target: TargetBand,
    points: usize,
    mut samples: Vec<Sample>,
    mut n: u64,
    mut eval: F,
) -> Result<SearchOutcome, String>
where
    F: FnMut(f64, u64) -> Result<f64, String>,
{
    let points = points.max(2);
    let span = range.max - range.min;
    for k in 0..points {
        let frac = k as f64 / (points - 1) as f64;
        let v = range.min + span * frac;
        // 复用已评估值（两端、或浮点上正好相等的点）
        let already = samples.iter().any(|s| s.value == v);
        if !already {
            let r = eval(v, n)?;
            n += 1;
            samples.push(Sample { value: v, clear_rate: r });
        }
        // 扫到命中就提前停（线扫也可能撞上目标带）
        if let Some(s) = samples.iter().find(|s| s.value == v) {
            if target.contains(s.clear_rate) {
                let hit = *s;
                return Ok(finish(
                    hit.value,
                    hit.clear_rate,
                    true,
                    samples,
                    "线扫命中目标带（旋钮对通关率非单调，二分不适用，改线扫）".to_string(),
                ));
            }
        }
    }
    let best = closest(&samples, target);
    let in_target = target.contains(best.clear_rate);
    let note = if in_target {
        "线扫命中目标带（旋钮对通关率非单调）".to_string()
    } else {
        "通关率对这个旋钮不单调（或目标带不在 range 内），给的是线扫最接近目标带的值，曲线见 samples"
            .to_string()
    };
    Ok(finish(best.value, best.clear_rate, in_target, samples, note))
}

/// 采样里离目标带最近的点（带内距离 0；多点同距离取旋钮值小的，确定）。
fn closest(samples: &[Sample], target: TargetBand) -> Sample {
    samples
        .iter()
        .copied()
        .min_by(|a, b| {
            let da = target.distance(a.clear_rate);
            let db = target.distance(b.clear_rate);
            da.partial_cmp(&db)
                .expect("通关率非 NaN")
                .then(a.value.partial_cmp(&b.value).expect("旋钮值非 NaN"))
        })
        .expect("samples 至少有两端两个点")
}

fn finish(found_value: f64, found_clear_rate: f64, in_target: bool, samples: Vec<Sample>, note: String) -> SearchOutcome {
    SearchOutcome { found_value, found_clear_rate, in_target, iterations: samples.len(), samples, note }
}

/// CLI 入口：`vitric balance <项目> --knob ... --target-clear-rate ... --range ... [选项]`。
pub fn run(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("balance 缺少项目目录参数")?;
    let dir = PathBuf::from(dir);

    let mut knob: Option<String> = None;
    let mut target: Option<String> = None;
    let mut range: Option<String> = None;
    let mut sessions: u64 = 16;
    let mut max_ticks: u64 = 600;
    let mut max_iters: usize = 12;
    let mut seed: u64 = 0;
    let mut out_path: Option<PathBuf> = None;
    // --strategy 当前只接受 "swarm"（默认组）——保留位，给将来扩展前瞻/经济等专档留口。
    let mut strategy = "swarm".to_string();

    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--knob" => {
                knob = Some(args.get(i + 1).ok_or(need("--knob"))?.clone());
                i += 2;
            }
            "--target-clear-rate" => {
                target = Some(args.get(i + 1).ok_or(need("--target-clear-rate"))?.clone());
                i += 2;
            }
            "--range" => {
                range = Some(args.get(i + 1).ok_or(need("--range"))?.clone());
                i += 2;
            }
            "--sessions" => {
                sessions = args.get(i + 1).ok_or(need("--sessions"))?.parse().map_err(|e| format!("--sessions: {e}"))?;
                if sessions == 0 {
                    return Err("--sessions 至少为 1".to_string());
                }
                i += 2;
            }
            "--max-ticks" => {
                max_ticks = args.get(i + 1).ok_or(need("--max-ticks"))?.parse().map_err(|e| format!("--max-ticks: {e}"))?;
                i += 2;
            }
            "--max-iters" => {
                max_iters = args.get(i + 1).ok_or(need("--max-iters"))?.parse().map_err(|e| format!("--max-iters: {e}"))?;
                if max_iters < 2 {
                    return Err("--max-iters 至少为 2（要先评估 range 两端定方向）".to_string());
                }
                i += 2;
            }
            "--seed" => {
                seed = args.get(i + 1).ok_or(need("--seed"))?.parse().map_err(|e| format!("--seed: {e}"))?;
                i += 2;
            }
            "--strategy" => {
                strategy = args.get(i + 1).ok_or(need("--strategy"))?.clone();
                if strategy != "swarm" {
                    return Err(format!("--strategy 当前只支持 swarm（默认组），拿到 {strategy:?}"));
                }
                i += 2;
            }
            "--out" => {
                out_path = Some(PathBuf::from(args.get(i + 1).ok_or(need("--out"))?));
                i += 2;
            }
            other => {
                return Err(format!(
                    "未知选项 {other:?}。可用: --knob --target-clear-rate --range --sessions --max-ticks --max-iters --seed --strategy --out"
                ))
            }
        }
    }

    let knob = knob.ok_or("balance 缺少 --knob <相对文件路径>#<json-pointer>")?;
    let target = target.ok_or("balance 缺少 --target-clear-rate <下限>:<上限>")?;
    let range = range.ok_or("balance 缺少 --range <min>:<max>")?;

    let addr = KnobAddr::parse(&knob)?;
    let target = TargetBand::parse(&target)?;
    let knob_range = KnobRange::parse(&range)?;

    // 旋钮寻址自检：加载期就确认 knob 指到一个数字（越界/非数字立刻报错，不等跑完试玩）。
    let knob_initial = read_knob(&dir, &addr)?;

    let params = EvalParams { sessions, max_ticks, seed };
    // 真跑：每个候选值拷临时副本→改 knob→跑 swarm→拿 win_rate→删副本（evaluate 内部完成）。
    let src = dir.clone();
    let addr_eval = addr.clone();
    let params_eval = params.clone();
    let outcome = search(knob_range, target, max_iters, |value, n| {
        evaluate(&src, &addr_eval, value, &params_eval, n)
    })?;

    // 源项目逐字节不变是配平的硬约束——这里不主动断言（集成测试断言），但临时副本已 Drop 删干净。
    let _ = strategy; // 当前只 swarm，保留位避免 unused

    let report = serde_json::json!({
        "knob": {
            "file": addr.rel_file,
            "pointer": addr.pointer,
            "initial_value": knob_initial,
        },
        "target": { "lo": target.lo, "hi": target.hi },
        "range": { "min": knob_range.min, "max": knob_range.max },
        "found_value": outcome.found_value,
        "found_clear_rate": outcome.found_clear_rate,
        "in_target": outcome.in_target,
        "iterations": outcome.iterations,
        "samples": outcome.samples.iter().map(|s| s.to_json()).collect::<Vec<_>>(),
        "note": outcome.note,
    });
    let json = serde_json::to_string_pretty(&report).expect("配平报告可序列化");
    if let Some(out) = &out_path {
        std::fs::write(out, &json).map_err(|e| format!("写配平报告 {} 失败: {e}", out.display()))?;
    }
    println!("{json}");

    // 一句人话总结（stderr，和 JSON 分流；人看总结、脚本读 stdout 的 JSON）。
    if outcome.in_target {
        eprintln!(
            "把 {}#{} 调到 {}，通关率 {:.1}%，达标（目标 {:.0}%~{:.0}%）。",
            addr.rel_file, addr.pointer, fmt_knob(outcome.found_value),
            outcome.found_clear_rate * 100.0, target.lo * 100.0, target.hi * 100.0
        );
    } else {
        eprintln!(
            "range [{}, {}] 内没有值能让通关率落进 {:.0}%~{:.0}%，最接近的是 {}→{:.1}%，曲线见 samples。",
            fmt_knob(knob_range.min), fmt_knob(knob_range.max),
            target.lo * 100.0, target.hi * 100.0,
            fmt_knob(outcome.found_value), outcome.found_clear_rate * 100.0
        );
    }
    Ok(())
}

/// 旋钮值人话展示：整数值去掉小数尾巴。
fn fmt_knob(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 旋钮寻址：解析 ----

    #[test]
    fn knob_addr_parses_file_and_pointer() {
        let a = KnobAddr::parse("rules/game.json#/rules/3/do/0/to").unwrap();
        assert_eq!(a.rel_file, "rules/game.json");
        assert_eq!(a.pointer, "/rules/3/do/0/to");
    }

    #[test]
    fn knob_addr_rejects_missing_hash() {
        assert!(KnobAddr::parse("rules/game.json").is_err());
    }

    #[test]
    fn knob_addr_rejects_empty_pointer() {
        assert!(KnobAddr::parse("rules/game.json#").is_err());
    }

    #[test]
    fn knob_addr_rejects_pointer_without_leading_slash() {
        assert!(KnobAddr::parse("rules/game.json#rules/3").is_err());
    }

    // ---- 旋钮寻址：读/改值 + 越界报错 ----

    #[test]
    fn set_pointer_changes_the_number() {
        let mut doc = serde_json::json!({"rules":[{"do":[{"to": 8}]}]});
        set_pointer_number(&mut doc, "/rules/0/do/0/to", 20.0).unwrap();
        assert_eq!(doc.pointer("/rules/0/do/0/to").unwrap().as_i64(), Some(20));
    }

    #[test]
    fn set_pointer_writes_integer_for_whole_values() {
        // 8.0 这种整数值要写成整数 8（不写 8.0），临时副本 JSON 跟用户原文同形
        let mut doc = serde_json::json!({"x": 1});
        set_pointer_number(&mut doc, "/x", 8.0).unwrap();
        assert!(doc.pointer("/x").unwrap().is_i64(), "整数值应写成整数: {:?}", doc.pointer("/x"));
        assert_eq!(doc.pointer("/x").unwrap().as_i64(), Some(8));
    }

    #[test]
    fn set_pointer_out_of_bounds_errors() {
        let mut doc = serde_json::json!({"rules":[{"do":[{"to": 8}]}]});
        // 下标越界
        assert!(set_pointer_number(&mut doc, "/rules/9/do/0/to", 1.0).is_err());
        // 路径根本不存在
        assert!(set_pointer_number(&mut doc, "/nope/0", 1.0).is_err());
    }

    #[test]
    fn set_pointer_non_number_errors() {
        // pointer 指到的不是数字（是字符串）→ 报错，不覆盖
        let mut doc = serde_json::json!({"name": "hi"});
        assert!(set_pointer_number(&mut doc, "/name", 1.0).is_err());
    }

    // ---- 区间/目标解析 ----

    #[test]
    fn target_band_parse_and_contains() {
        let t = TargetBand::parse("0.4:0.7").unwrap();
        assert!(t.contains(0.4) && t.contains(0.7) && t.contains(0.55));
        assert!(!t.contains(0.39) && !t.contains(0.71));
    }

    #[test]
    fn target_band_rejects_inverted_or_out_of_range() {
        assert!(TargetBand::parse("0.7:0.4").is_err());
        assert!(TargetBand::parse("0.4:1.5").is_err());
    }

    #[test]
    fn knob_range_rejects_degenerate() {
        assert!(KnobRange::parse("5:5").is_err());
        assert!(KnobRange::parse("0:50").is_ok());
    }

    // ---- 搜索：二分收敛（单调下降的合成函数）----

    /// 合成评估器：通关率 = clamp(1 - value/100)，随 value 单调下降。value≈30 → 0.7，value≈60 → 0.4。
    fn descending_eval(value: f64) -> f64 {
        (1.0 - value / 100.0).clamp(0.0, 1.0)
    }

    #[test]
    fn binary_search_converges_on_descending() {
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.4, hi: 0.7 }; // value ∈ [30,60] 命中
        let mut calls = 0;
        let out = search(range, target, 20, |v, _| {
            calls += 1;
            Ok(descending_eval(v))
        })
        .unwrap();
        assert!(out.in_target, "单调下降应二分命中: {:?}", out);
        assert!(descending_eval(out.found_value) >= 0.4 && descending_eval(out.found_value) <= 0.7);
        assert!((out.found_clear_rate - descending_eval(out.found_value)).abs() < 1e-12);
        // 命中后即停，不会跑满预算
        assert!(out.iterations <= 20);
    }

    #[test]
    fn binary_search_converges_on_ascending() {
        // 通关率随 value 单调上升：rate = clamp(value/100)
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.4, hi: 0.7 };
        let out = search(range, target, 20, |v, _| Ok((v / 100.0).clamp(0.0, 1.0))).unwrap();
        assert!(out.in_target, "单调上升也应二分命中: {:?}", out);
        let r = out.found_clear_rate;
        assert!((0.4..=0.7).contains(&r));
    }

    #[test]
    fn search_is_deterministic_same_inputs_same_samples() {
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.4, hi: 0.7 };
        let run = || search(range, target, 20, |v, _| Ok(descending_eval(v))).unwrap();
        let a = run();
        let b = run();
        assert_eq!(a.found_value, b.found_value);
        assert_eq!(a.iterations, b.iterations);
        assert_eq!(a.samples.len(), b.samples.len());
        for (x, y) in a.samples.iter().zip(b.samples.iter()) {
            assert_eq!(x.value, y.value);
            assert_eq!(x.clear_rate, y.clear_rate);
        }
    }

    #[test]
    fn endpoint_hit_returns_immediately() {
        // range 下端正好命中（descending: value=0 → rate=1.0；改目标带含 1.0）
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.9, hi: 1.0 };
        let mut calls = 0;
        let out = search(range, target, 20, |v, _| {
            calls += 1;
            Ok(descending_eval(v))
        })
        .unwrap();
        assert!(out.in_target);
        assert_eq!(out.found_value, 0.0, "下端 rate=1.0 命中目标带");
        assert_eq!(calls, 1, "下端命中应只评估一次");
    }

    // ---- 搜索：非单调线扫兜底 ----

    /// 非单调评估器：抛物线峰在 value=50（rate 在 50 处最高 1.0，两端低）。
    /// 两端 rate 都低（同侧），bracketed 不成立 → 走线扫。峰附近能命中高目标带。
    fn hump_eval(value: f64) -> f64 {
        let d = (value - 50.0) / 50.0; // value∈[0,100] → d∈[-1,1]
        (1.0 - d * d).clamp(0.0, 1.0)
    }

    #[test]
    fn nonmonotonic_falls_back_to_line_scan_and_finds_best() {
        let range = KnobRange { min: 0.0, max: 100.0 };
        // 目标带在峰附近（两端 rate≈0，中间≈1）。线扫应找到接近峰的点命中。
        let target = TargetBand { lo: 0.9, hi: 1.0 };
        let out = search(range, target, 11, |v, _| Ok(hump_eval(v))).unwrap();
        assert!(out.note.contains("线扫") || out.note.contains("单调"), "应标注线扫/非单调: {}", out.note);
        assert!(out.in_target, "线扫应在峰附近命中高目标带: {:?}", out);
        assert!((out.found_value - 50.0).abs() <= 10.0, "命中值应接近峰 50: {}", out.found_value);
    }

    #[test]
    fn line_scan_reports_closest_when_unreachable() {
        // 目标带整体高于该旋钮在 range 内能达到的任何通关率 → 没命中，报最接近 + 诚实 note。
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.95, hi: 1.0 };
        // 评估器：rate 最高只到 0.5（永远够不到 0.95）。两端同侧（都在带下方）→ 线扫。
        let out = search(range, target, 8, |v, _| Ok((v / 200.0).clamp(0.0, 0.5))).unwrap();
        assert!(!out.in_target, "够不到的目标带应 in_target=false");
        assert!(out.note.contains("不单调") || out.note.contains("不在 range"), "诚实标注: {}", out.note);
        // 最接近的是 rate 最高那个点（value=max）
        assert!(out.found_clear_rate > 0.49, "应报最接近目标带（rate 最高）的点: {:?}", out);
    }

    // ---- 临时副本：拷贝 + Drop 清理 ----

    #[test]
    fn temp_project_clones_and_cleans_up() {
        // 造一个最小"项目"目录
        let src = std::env::temp_dir().join(format!("vitric-balance-srctest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("rules")).unwrap();
        std::fs::write(src.join("vitric.json"), "{}").unwrap();
        std::fs::write(src.join("rules/game.json"), "{\"x\":1}").unwrap();

        let temp_dir;
        {
            let temp = TempProject::clone_from(&src, 777).unwrap();
            temp_dir = temp.dir.clone();
            assert!(temp.dir.join("rules/game.json").exists(), "副本应含子目录文件");
            // 改副本不影响源
            std::fs::write(temp.dir.join("rules/game.json"), "{\"x\":999}").unwrap();
            assert_eq!(std::fs::read_to_string(src.join("rules/game.json")).unwrap(), "{\"x\":1}", "源文件不被改");
        } // temp Drop
        assert!(!temp_dir.exists(), "Drop 后临时副本应删干净");

        let _ = std::fs::remove_dir_all(&src);
    }
}
