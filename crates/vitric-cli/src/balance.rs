//! `vitric balance`: auto-balancing. Tweak one numeric knob, run playtests repeatedly with an
//! agent swarm, and binary-search for the knob value that lands the clear rate in the target
//! band — upgrading the "playtest report" from merely flagging problems to **closed-loop balancing**.
//!
//! Three pieces fit together:
//! - **Knob addressing** ([`KnobAddr`]): `<relative file path>#<json-pointer>` points at a number
//!   in some project file (RFC6901 JSON Pointer). Parsing reads that file as JSON and resolves the
//!   pointer to that number.
//! - **Applying the knob** ([`apply_knob_to_temp`]): for each candidate value, **copy the whole
//!   project to a temp directory**, change only that one pointer in that one file, then run a
//!   playtest on the temp copy. **Never writes the user's project directory**; the temp copy is
//!   deleted after use ([`TempProject`]'s Drop handles cleanup).
//! - **Search** ([`search`]): first run each end of the range once to determine direction (does
//!   clear rate rise or fall with the knob), assume monotonic and binary-search; if the two ends
//!   disagree in direction / the midpoint violates the monotonic assumption, fall back to a
//!   **coarse line scan**, reporting the value closest to the target + the entire curve, and
//!   honestly noting "clear rate is non-monotonic for this knob".
//!
//! **Determinism**: playtests are already deterministic (same knob value → same clear rate), and
//! the search path is decided solely by project + parameters — same project, same parameters
//! yields the same found_value + the same samples. `search` abstracts "how to evaluate a candidate
//! value" into an injected closure, so the pure-algorithm part (binary search / line scan) can be
//! unit-tested without boot; in real runs the closure points to [`evaluate`] (copy temp → run
//! swarm → get win_rate).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use vitric_data::Project;
use vitric_playtest::{
    aggregate_with_endings_and_declared, default_plan, run_swarm_with_config, PlaytestConfig,
    TerminalSpec,
};

use crate::runtime::Runtime;

/// Knob address: some file in the project (relative path) + a JSON Pointer (RFC6901, e.g.
/// `/rules/3/do/0/to`) pointing at a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnobAddr {
    /// File path relative to the project root (e.g. `rules/game.json`).
    pub rel_file: String,
    /// RFC6901 JSON Pointer (e.g. `/rules/3/do/0/to`) pointing at a number in that file.
    pub pointer: String,
}

impl KnobAddr {
    /// Parse the `--knob` argument: `<relative file path>#<json-pointer>`. `#` splits only on the
    /// first occurrence (the pointer will not contain `#`, but a filename theoretically could — by
    /// convention "everything after the first # is the pointer", simple and predictable).
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
            // An empty pointer (which in RFC6901 refers to the whole document) is meaningless for
            // balancing — it must point at a number.
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

/// Read the knob's current value: read `rel_file` as JSON and resolve the pointer to that number.
/// Pointer unresolvable / pointing at a non-number both explicitly error (with the path,
/// vitric check style).
pub fn read_knob(root: &Path, addr: &KnobAddr) -> Result<f64, String> {
    let path = root.join(&addr.rel_file);
    let doc = load_json(&path)?;
    pointer_get_number(&doc, &addr.pointer)
        .ok_or_else(|| format!("{} 里 pointer {} 没指到一个数字（越界或非 number）", addr.rel_file, addr.pointer))
}

/// Change the number at the pointer to `value` in an in-memory JSON document. Pointer unresolvable
/// / original value not a number both explicitly error — balancing only tweaks knobs that "are
/// already numbers"; it never creates fields out of thin air or overwrites non-numbers.
pub fn set_pointer_number(doc: &mut Value, pointer: &str, value: f64) -> Result<(), String> {
    // First confirm the original value exists and is a number (out-of-bounds / non-number errors
    // immediately, no silent creation).
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

/// Render a candidate value as a JSON number: integral values (e.g. 8.0) are written as integer
/// `8` (not `8.0`), the rest as floats. Knobs are mostly integer thresholds ("enemy attack = 8"),
/// so writing integers keeps the temp copy's JSON identical in shape to the user's original, easy
/// to reconcile.
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9.007_199_254_740_992e15 {
        // Integral values falling within the i64 safe-integer range are written as integer literals
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// Get the number at the pointer (returns None if absent or not a number).
fn pointer_get_number(doc: &Value, pointer: &str) -> Option<f64> {
    doc.pointer(pointer).and_then(|v| v.as_f64())
}

fn load_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} 解析 JSON 失败: {e}", path.display()))
}

/// A temporary project copy: copies the whole project directory to the system temp area (isolated
/// by process id + counter). On Drop the entire directory is deleted. **Never touches the user's
/// project directory** — all balancing writes land in this copy.
struct TempProject {
    dir: PathBuf,
}

/// In-process global increasing counter: guarantees each temp copy directory name is **globally
/// unique** — even if multiple evaluations / multiple tests run concurrently in the same process
/// (same pid), they never collide on a directory (a collision would wipe each other's copies and
/// read half-finished data). `n` is just a reconciliation number passed in by the caller; it does
/// not participate in uniqueness.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl TempProject {
    /// Copy the whole `src` project tree to
    /// `temp_dir()/vitric-balance-<pid>-<global sequence>-<n>/`. The global sequence guarantees
    /// uniqueness, so there is no need to "delete the old directory first" (every run is a fresh path).
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
        // Best-effort delete; don't panic on failure (a panic in a destructor would mask the real
        // error), leftover temp directories are harmless.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Recursively copy a directory (only copies files and subdirectories; symlinks are copied as
/// ordinary files pointing to their target — a project should not contain links).
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

/// Parameters for one evaluation (how to run the swarm to get the clear rate).
#[derive(Debug, Clone)]
pub struct EvalParams {
    pub sessions: u64,
    pub max_ticks: u64,
    pub seed: u64,
}

/// Evaluate a candidate knob value → clear rate (0..1).
///
/// Flow: copy the project to a temp copy → change the number at the knob pointer in the copy to
/// `value` → boot the temp copy and run a swarm (default_plan default strategy group) → aggregate
/// to get `outcome_distribution.win_rate` → the temp copy is dropped and deleted.
/// `n` is used for temp directory name isolation (multiple evaluations in the same process do not
/// collide on directories). **Never writes the user's project directory**.
pub fn evaluate(src: &Path, addr: &KnobAddr, value: f64, params: &EvalParams, n: u64) -> Result<f64, String> {
    let temp = TempProject::clone_from(src, n)?;

    // Change that one pointer in that one file in the temp copy
    let target_file = temp.dir.join(&addr.rel_file);
    let mut doc = load_json(&target_file)?;
    set_pointer_number(&mut doc, &addr.pointer, value)?;
    let serialized = serde_json::to_string_pretty(&doc).expect("旋钮文档可序列化");
    std::fs::write(&target_file, serialized)
        .map_err(|e| format!("写临时旋钮文件 {} 失败: {e}", target_file.display()))?;

    // Run the swarm to get the clear rate (same calibration as gate's playtest gate:
    // default_plan default strategy group + config + manifest must_emit)
    let win_rate = run_swarm_win_rate(&temp.dir, params)?;
    // temp is dropped here, the temp copy is deleted
    Ok(win_rate)
}

/// Run a default_plan swarm on a (temp) project directory and aggregate the clear rate.
/// Calibration aligns with cmd_playtest / gate's playtest gate: playtest.json overrides the view,
/// manifest must_emit joins the win set, and a declared goal auto-injects lookahead. Each session
/// boots its own runtime (QuickJS is not Send, the runtime does not cross threads).
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

    // Default strategy group swarm (a declared goal auto-injects lookahead; no declaration leaves
    // it fully unchanged) — the "default swarm, default group" required by the task.
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

/// Target clear-rate band [lo, hi] (closed interval).
#[derive(Debug, Clone, Copy)]
pub struct TargetBand {
    pub lo: f64,
    pub hi: f64,
}

impl TargetBand {
    /// Parse `lo:hi` (e.g. `0.4:0.7`).
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
    /// Hit: the clear rate falls within [lo,hi] (endpoints included).
    pub fn contains(&self, rate: f64) -> bool {
        rate >= self.lo && rate <= self.hi
    }
    /// Distance from the band (the line scan's "closest" uses this as the distance reference —
    /// the closer to the band the better; inside the band the distance is 0).
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

/// Knob search range [min, max].
#[derive(Debug, Clone, Copy)]
pub struct KnobRange {
    pub min: f64,
    pub max: f64,
}

impl KnobRange {
    /// Parse `min:max` (e.g. `0:50`).
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

/// A sample point: knob value → clear rate.
/// (vitric-cli does not directly depend on serde derive; output uses [`Sample::to_json`] to
/// hand-serialize into the json! report.)
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub value: f64,
    pub clear_rate: f64,
}

impl Sample {
    /// Serialize to `{"value":.., "clear_rate":..}` (feeds the json! report's samples array).
    fn to_json(self) -> Value {
        serde_json::json!({ "value": self.value, "clear_rate": self.clear_rate })
    }
}

/// Search outcome (feeds JSON output).
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    /// The reported knob value (hit = a value landing in the target band; miss = the value closest
    /// to the target band in the line scan).
    pub found_value: f64,
    /// The clear rate corresponding to that value.
    pub found_clear_rate: f64,
    /// Whether the target band was hit.
    pub in_target: bool,
    /// Number of evaluations (each = one swarm playtest round).
    pub iterations: usize,
    /// All sample points (in evaluation order, deterministic and reproducible).
    pub samples: Vec<Sample>,
    /// A plain-language / honest note (e.g. "clear rate is non-monotonic for this knob, the line
    /// scan optimum is given").
    pub note: String,
}

/// Convergence threshold: if the binary-search interval width shrinks below this fraction of the
/// range span without a hit, stop (avoids infinite binary search).
const CONVERGE_FRAC: f64 = 1e-3;

/// Balancing search: binary search + non-monotonic line scan fallback.
///
/// `eval(value, n) -> clear_rate`: evaluates the clear rate for a candidate knob value (`n` is the
/// evaluation index, used for temp directory isolation / reconciliation). Abstracting evaluation
/// into a closure lets the pure algorithm (direction check / binary search / line scan) be
/// unit-tested without boot; in real runs the closure points to [`evaluate`].
///
/// Algorithm:
/// 1. First evaluate both ends of the range `min`, `max` to determine direction: does the clear
///    rate rise (rate(max) > rate(min)) or fall as the knob increases. If an endpoint itself hits
///    the target band, return immediately (saves one round).
/// 2. **Both ends have a clear direction** (one above the band, the other below; under the
///    monotonic assumption the target band is bracketed between them): binary search. Each step
///    evaluates the midpoint, stops on a hit, narrows the interval by direction; if the interval
///    width shrinks very small without a hit, also stop (report the closest endpoint sample).
/// 3. **The two ends disagree in direction / are on the same side of the band** (the monotonic
///    assumption does not hold, or the interval contains no solution): fall back to a **coarse line
///    scan** — sample `max_iters` evenly spaced points across the range, report the value closest
///    to the target band + the entire curve, with an honest note.
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
    // Evaluate a value and record it in samples (dedup: the same value is not re-evaluated, the
    // recorded result is reused — under determinism, same value means same rate). The closure
    // cannot borrow samples (it needs to be mutable), so this is hand-written functionally: check
    // the cache first, then eval if absent.
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

    // Precondition for binary search under the monotonic assumption: the two ends "bracket" the
    // target band — one end's clear rate is above the band, the other below. The band midpoint is
    // the reference: which end is higher / lower, and the two straddle the target.
    let min_above = rate_min > target.hi;
    let max_above = rate_max > target.hi;
    let min_below = rate_min < target.lo;
    let max_below = rate_max < target.lo;
    let bracketed = (min_above && max_below) || (min_below && max_above);

    if bracketed {
        // Direction: does the rate fall (min high, max low) or rise as the knob increases.
        let ascending = rate_max > rate_min;
        binary_search(range, target, max_iters, ascending, &mut samples, &mut n, eval)
    } else {
        // Monotonic assumption does not hold or the interval contains no solution: line scan
        // fallback. The already-evaluated ends are carried along (not wasted).
        line_scan(range, target, max_iters, samples, n, eval)
    }
}

/// Binary search core (given that both ends bracket the target band and direction = ascending).
/// Stops on a hit; also stops when the interval converges to a tiny width without a hit (in that
/// case reports the sample closest to the target band within the interval — usually the last midpoint).
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
    // Both ends are already in samples (search evaluated them); binary search starts from the
    // midpoint. The budget subtracts the two already-used endpoint evaluations.
    let budget = max_iters.saturating_sub(samples.len());
    for _ in 0..budget {
        if (hi - lo).abs() <= span * CONVERGE_FRAC {
            break;
        }
        let mid = lo + (hi - lo) / 2.0;
        // Reuse an already-evaluated value (under determinism, same value means same rate); don't
        // repeat a swarm round
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
        // rate too high → move toward "lower clear rate"; too low → move toward higher. When
        // ascending (rate rises with the knob): high rate pulls hi to mid (decrease the knob to
        // lower the clear rate). Descending is the opposite direction.
        let too_high = rate > target.hi;
        if ascending == too_high {
            // ascending && too high → decrease the knob (hi=mid); descending && too low → also
            // decrease the knob (hi=mid)
            hi = mid;
        } else {
            lo = mid;
        }
    }
    // No hit: report the value in the samples closest to the target band (honestly give the best
    // approximation).
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

/// Line scan fallback: sample `points` evenly spaced points across the range (including both
/// ends), report the value closest to the target band + the entire curve. The already-evaluated
/// ends are reused (they are in samples). Honestly notes "clear rate is non-monotonic for this
/// knob / the interval contains no solution, the line scan optimum is given".
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
        // Reuse already-evaluated values (the ends, or points that happen to be equal in floating point)
        let already = samples.iter().any(|s| s.value == v);
        if !already {
            let r = eval(v, n)?;
            n += 1;
            samples.push(Sample { value: v, clear_rate: r });
        }
        // Stop early on a hit (a line scan can also stumble into the target band)
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

/// The point in the samples closest to the target band (distance 0 inside the band; on a tie the
/// smaller knob value wins, deterministic).
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

/// CLI entry: `vitric balance <project> --knob ... --target-clear-rate ... --range ... [options]`.
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
    // --strategy currently only accepts "swarm" (default group) — a reserved slot, leaving room to
    // add lookahead / economy specialist profiles later.
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

    // Knob addressing self-check: confirm at load time that the knob points at a number
    // (out-of-bounds / non-number errors immediately, without waiting for the playtest to finish).
    let knob_initial = read_knob(&dir, &addr)?;

    let params = EvalParams { sessions, max_ticks, seed };
    // Real run: for each candidate value, copy a temp copy → change the knob → run the swarm → get
    // win_rate → delete the copy (all done inside evaluate).
    let src = dir.clone();
    let addr_eval = addr.clone();
    let params_eval = params.clone();
    let outcome = search(knob_range, target, max_iters, |value, n| {
        evaluate(&src, &addr_eval, value, &params_eval, n)
    })?;

    // The source project remaining byte-for-byte unchanged is a hard constraint of balancing — no
    // active assertion here (integration tests assert it), and the temp copies have been dropped
    // and cleaned up.
    let _ = strategy; // currently only swarm; reserved slot to avoid unused

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

    // A plain-language summary (stderr, separate from the JSON; humans read the summary, scripts
    // read stdout's JSON).
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

/// Plain-language display of a knob value: integral values drop the decimal tail.
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

    // ---- Knob addressing: parsing ----

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

    // ---- Knob addressing: read/change value + out-of-bounds errors ----

    #[test]
    fn set_pointer_changes_the_number() {
        let mut doc = serde_json::json!({"rules":[{"do":[{"to": 8}]}]});
        set_pointer_number(&mut doc, "/rules/0/do/0/to", 20.0).unwrap();
        assert_eq!(doc.pointer("/rules/0/do/0/to").unwrap().as_i64(), Some(20));
    }

    #[test]
    fn set_pointer_writes_integer_for_whole_values() {
        // Integral values like 8.0 must be written as integer 8 (not 8.0), so the temp copy's JSON
        // stays the same shape as the user's original
        let mut doc = serde_json::json!({"x": 1});
        set_pointer_number(&mut doc, "/x", 8.0).unwrap();
        assert!(doc.pointer("/x").unwrap().is_i64(), "整数值应写成整数: {:?}", doc.pointer("/x"));
        assert_eq!(doc.pointer("/x").unwrap().as_i64(), Some(8));
    }

    #[test]
    fn set_pointer_out_of_bounds_errors() {
        let mut doc = serde_json::json!({"rules":[{"do":[{"to": 8}]}]});
        // Index out of bounds
        assert!(set_pointer_number(&mut doc, "/rules/9/do/0/to", 1.0).is_err());
        // Path does not exist at all
        assert!(set_pointer_number(&mut doc, "/nope/0", 1.0).is_err());
    }

    #[test]
    fn set_pointer_non_number_errors() {
        // Pointer points at a non-number (a string) → error, do not overwrite
        let mut doc = serde_json::json!({"name": "hi"});
        assert!(set_pointer_number(&mut doc, "/name", 1.0).is_err());
    }

    // ---- Range/target parsing ----

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

    // ---- Search: binary-search convergence (monotonically descending synthetic function) ----

    /// Synthetic evaluator: clear rate = clamp(1 - value/100), monotonically descending with value.
    /// value≈30 → 0.7, value≈60 → 0.4.
    fn descending_eval(value: f64) -> f64 {
        (1.0 - value / 100.0).clamp(0.0, 1.0)
    }

    #[test]
    fn binary_search_converges_on_descending() {
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.4, hi: 0.7 }; // value ∈ [30,60] hits
        let mut calls = 0;
        let out = search(range, target, 20, |v, _| {
            calls += 1;
            Ok(descending_eval(v))
        })
        .unwrap();
        assert!(out.in_target, "单调下降应二分命中: {:?}", out);
        assert!(descending_eval(out.found_value) >= 0.4 && descending_eval(out.found_value) <= 0.7);
        assert!((out.found_clear_rate - descending_eval(out.found_value)).abs() < 1e-12);
        // Stops on a hit, won't run the full budget
        assert!(out.iterations <= 20);
    }

    #[test]
    fn binary_search_converges_on_ascending() {
        // Clear rate monotonically ascending with value: rate = clamp(value/100)
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
        // The range's lower end happens to hit (descending: value=0 → rate=1.0; set the target band
        // to include 1.0)
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

    // ---- Search: non-monotonic line scan fallback ----

    /// Non-monotonic evaluator: parabolic peak at value=50 (rate is highest 1.0 at 50, low at both
    /// ends). Both ends have low rate (same side), bracketed does not hold → line scan. Near the
    /// peak it can hit a high target band.
    fn hump_eval(value: f64) -> f64 {
        let d = (value - 50.0) / 50.0; // value∈[0,100] → d∈[-1,1]
        (1.0 - d * d).clamp(0.0, 1.0)
    }

    #[test]
    fn nonmonotonic_falls_back_to_line_scan_and_finds_best() {
        let range = KnobRange { min: 0.0, max: 100.0 };
        // Target band near the peak (rate≈0 at both ends, ≈1 in the middle). The line scan should
        // find a point near the peak that hits.
        let target = TargetBand { lo: 0.9, hi: 1.0 };
        let out = search(range, target, 11, |v, _| Ok(hump_eval(v))).unwrap();
        assert!(out.note.contains("线扫") || out.note.contains("单调"), "应标注线扫/非单调: {}", out.note);
        assert!(out.in_target, "线扫应在峰附近命中高目标带: {:?}", out);
        assert!((out.found_value - 50.0).abs() <= 10.0, "命中值应接近峰 50: {}", out.found_value);
    }

    #[test]
    fn line_scan_reports_closest_when_unreachable() {
        // The target band is entirely above any clear rate this knob can reach within the range →
        // no hit, report the closest + an honest note.
        let range = KnobRange { min: 0.0, max: 100.0 };
        let target = TargetBand { lo: 0.95, hi: 1.0 };
        // Evaluator: rate tops out at 0.5 (can never reach 0.95). Both ends are on the same side
        // (below the band) → line scan.
        let out = search(range, target, 8, |v, _| Ok((v / 200.0).clamp(0.0, 0.5))).unwrap();
        assert!(!out.in_target, "够不到的目标带应 in_target=false");
        assert!(out.note.contains("不单调") || out.note.contains("不在 range"), "诚实标注: {}", out.note);
        // The closest is the point with the highest rate (value=max)
        assert!(out.found_clear_rate > 0.49, "应报最接近目标带（rate 最高）的点: {:?}", out);
    }

    // ---- Temp copy: clone + Drop cleanup ----

    #[test]
    fn temp_project_clones_and_cleans_up() {
        // Build a minimal "project" directory
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
            // Modifying the copy does not affect the source
            std::fs::write(temp.dir.join("rules/game.json"), "{\"x\":999}").unwrap();
            assert_eq!(std::fs::read_to_string(src.join("rules/game.json")).unwrap(), "{\"x\":1}", "源文件不被改");
        } // temp Drop
        assert!(!temp_dir.exists(), "Drop 后临时副本应删干净");

        let _ = std::fs::remove_dir_all(&src);
    }
}
