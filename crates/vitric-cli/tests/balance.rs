//! `vitric balance` integration test (real boot): runs auto-balancing on a mine-laying fixture
//! with an adjustable difficulty knob, asserting binary search finds a knob value that drops the
//! clear rate into the target band, and that **after scanning the source fixture files they are
//! byte-for-byte unchanged**.
//!
//! Mine-laying fixture = `tests/fixtures/difficulty` (lives under the vitric-playtest crate, same
//! place as the other mine-laying projects):
//! action `step` increments `@world.Counter.value` by 1; the tick rule `if Counter.value >=
//! threshold` emits game-won.
//! Knob = that threshold (`rules/game.json#/rules/1/if/0/2`). The larger the threshold → the more
//! step presses needed to win → the lower the clear rate (not enough presses within max_ticks) →
//! **clear rate monotonically decreases as the knob grows**, exactly what binary search needs.

use std::path::PathBuf;

use vitric_cli::balance::{evaluate, search, EvalParams, KnobAddr, KnobRange, TargetBand};

/// Mine-laying fixture directory (under vitric-playtest's tests/fixtures; this test crate is at
/// crates/vitric-cli).
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures")
        .join(name)
}

/// Recursively read the whole directory into a sorted (relative path, bytes) list — used to
/// assert "after scanning, the source project is byte-for-byte unchanged".
fn snapshot_bytes(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let p = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// Knob address: in the tick rule (rules[1]) the 0th if clause
/// `["@world.Counter.value", ">=", 20]` — the 20 (index 2).
const KNOB: &str = "rules/game.json#/rules/1/if/0/2";

#[test]
fn balance_binary_search_hits_target_band_on_difficulty_fixture() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();

    // After scanning, the source files must be byte-for-byte unchanged — snapshot first.
    let before = snapshot_bytes(&dir);

    // Target band 0.4~0.6 (curve's measured threshold ~55 hits); range 20..120 brackets the
    // target band (threshold 20 → clear rate 1.0 above the band, threshold 120 → 0.0 below the
    // band); binary search will trigger.
    let range = KnobRange::parse("20:120").unwrap();
    let target = TargetBand::parse("0.4:0.6").unwrap();
    let params = EvalParams { sessions: 16, max_ticks: 120, seed: 0 };

    let src = dir.clone();
    let addr_eval = addr.clone();
    let params_eval = params.clone();
    let out = search(range, target, 12, |value, n| {
        evaluate(&src, &addr_eval, value, &params_eval, n)
    })
    .expect("配平搜索应跑通");

    // Hits the target band
    assert!(
        out.in_target,
        "二分应找到让通关率落进 [0.4,0.6] 的阈值，实际 found_value={} clear_rate={} note={} samples={:?}",
        out.found_value, out.found_clear_rate, out.note, out.samples
    );
    assert!(
        (0.4..=0.6).contains(&out.found_clear_rate),
        "found_clear_rate 应在目标带内: {}",
        out.found_clear_rate
    );
    // The found knob value falls in a reasonable range (the curve's measured hitting threshold is
    // between 45 and 65)
    assert!(
        out.found_value >= 40.0 && out.found_value <= 70.0,
        "找到的阈值应在合理区间 [40,70]，实际 {}",
        out.found_value
    );
    // At least evaluated both ends + a few midpoints
    assert!(out.iterations >= 3, "二分应评估两端 + 中点，iterations={}", out.iterations);

    // The source fixture is byte-for-byte unchanged (the hard constraint of balancing: never
    // modify the user's project directory)
    let after = snapshot_bytes(&dir);
    assert_eq!(before, after, "配平搜完源 fixture 文件必须逐字节不变（不许写用户项目目录）");
}

/// Determinism: same fixture, same params, run twice → found_value / samples are fully identical
/// (playtest deterministic + search path deterministic).
#[test]
fn balance_is_deterministic_on_fixture() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();
    let range = KnobRange::parse("20:120").unwrap();
    let target = TargetBand::parse("0.4:0.6").unwrap();
    let params = EvalParams { sessions: 16, max_ticks: 120, seed: 0 };

    let run = || {
        let src = dir.clone();
        let addr_eval = addr.clone();
        let params_eval = params.clone();
        search(range, target, 12, |value, n| evaluate(&src, &addr_eval, value, &params_eval, n)).unwrap()
    };
    let a = run();
    let b = run();
    assert_eq!(a.found_value, b.found_value, "同参 found_value 应一致");
    assert_eq!(a.in_target, b.in_target);
    assert_eq!(a.iterations, b.iterations);
    assert_eq!(a.samples.len(), b.samples.len(), "采样条数一致");
    for (x, y) in a.samples.iter().zip(b.samples.iter()) {
        assert_eq!(x.value, y.value, "采样旋钮值逐项一致");
        assert_eq!(x.clear_rate, y.clear_rate, "同旋钮值同通关率（试玩确定）");
    }
}

/// Knob addressing reads the correct initial value on the real fixture (the threshold's original
/// text is 20).
#[test]
fn balance_reads_initial_knob_value() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();
    let v = vitric_cli::balance::read_knob(&dir, &addr).unwrap();
    assert_eq!(v, 20.0, "旋钮初值应是规则里的 20");
}

/// A single evaluate runs a real swarm to get the clear rate, and the source files are unchanged
/// after evaluation (modified in a temp copy that is deleted afterwards).
#[test]
fn balance_evaluate_runs_real_swarm_without_touching_source() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();
    let before = snapshot_bytes(&dir);
    let params = EvalParams { sessions: 16, max_ticks: 120, seed: 0 };
    // Threshold pushed very high (120) → clear rate should be 0 (not enough presses); pushed very
    // low (5) → should be close to 1.
    let hard = evaluate(&dir, &addr, 120.0, &params, 0).unwrap();
    let easy = evaluate(&dir, &addr, 5.0, &params, 1).unwrap();
    assert!(hard < easy, "阈值高通关率应更低: hard(120)={hard} easy(5)={easy}");
    assert_eq!(hard, 0.0, "阈值 120 在 120 tick 内按不够，通关率 0");
    assert!(easy > 0.5, "阈值 5 很容易达成，通关率应高: {easy}");
    let after = snapshot_bytes(&dir);
    assert_eq!(before, after, "evaluate 不许改源文件");
}
