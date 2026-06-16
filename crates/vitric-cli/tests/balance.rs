//! `vitric balance` 集成测试（真 boot）：在带可调难度旋钮的埋雷 fixture 上跑自动配平，
//! 断言二分能找到让通关率落进目标带的旋钮值，且**源 fixture 文件搜完逐字节不变**。
//!
//! 埋雷 fixture = `tests/fixtures/difficulty`（住在 vitric-playtest crate 下，和其他埋雷项目同处）：
//! 动作 `step` 把 `@world.Counter.value` +1，tick 规则 `if Counter.value >= 阈值` 就发 game-won。
//! 旋钮 = 那个阈值（`rules/game.json#/rules/1/if/0/2`）。阈值越大 → 要按越多次 step 才赢 →
//! 通关率越低（在 max_ticks 内按不够）→ **通关率随旋钮单调下降**，正好给二分用。

use std::path::PathBuf;

use vitric_cli::balance::{evaluate, search, EvalParams, KnobAddr, KnobRange, TargetBand};

/// 埋雷 fixture 目录（在 vitric-playtest 的 tests/fixtures 下，本测试 crate 在 crates/vitric-cli）。
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures")
        .join(name)
}

/// 整目录递归读成 (相对路径, 字节) 列表，排序——用于断言"搜完源项目逐字节不变"。
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

/// 旋钮地址：tick 规则（rules[1]）的 if 第 0 条 `["@world.Counter.value", ">=", 20]` 里的 20（下标 2）。
const KNOB: &str = "rules/game.json#/rules/1/if/0/2";

#[test]
fn balance_binary_search_hits_target_band_on_difficulty_fixture() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();

    // 搜完源文件必须逐字节不变——先拍快照。
    let before = snapshot_bytes(&dir);

    // 目标带 0.4~0.6（曲线实测阈值 ~55 命中）；range 20..120 把目标带夹住
    // （阈值 20 → 通关率 1.0 在带上方，阈值 120 → 0.0 在带下方），二分会触发。
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

    // 命中目标带
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
    // 找到的旋钮值落在合理区间（曲线实测命中阈值在 45~65 之间）
    assert!(
        out.found_value >= 40.0 && out.found_value <= 70.0,
        "找到的阈值应在合理区间 [40,70]，实际 {}",
        out.found_value
    );
    // 至少评估了两端 + 几个中点
    assert!(out.iterations >= 3, "二分应评估两端 + 中点，iterations={}", out.iterations);

    // 源 fixture 逐字节不变（配平的硬约束：绝不改用户项目目录）
    let after = snapshot_bytes(&dir);
    assert_eq!(before, after, "配平搜完源 fixture 文件必须逐字节不变（不许写用户项目目录）");
}

/// 确定性：同 fixture 同参跑两次，found_value / samples 完全一致（试玩确定 + 搜索路径确定）。
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

/// 旋钮寻址在真 fixture 上读到正确初值（阈值原文是 20）。
#[test]
fn balance_reads_initial_knob_value() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();
    let v = vitric_cli::balance::read_knob(&dir, &addr).unwrap();
    assert_eq!(v, 20.0, "旋钮初值应是规则里的 20");
}

/// 单次 evaluate 跑真 swarm 拿通关率，且评估完源文件不变（临时副本里改完即删）。
#[test]
fn balance_evaluate_runs_real_swarm_without_touching_source() {
    let dir = fixture("difficulty");
    let addr = KnobAddr::parse(KNOB).unwrap();
    let before = snapshot_bytes(&dir);
    let params = EvalParams { sessions: 16, max_ticks: 120, seed: 0 };
    // 阈值压到很高（120）→ 通关率应为 0（按不够）；压到很低（5）→ 应接近 1。
    let hard = evaluate(&dir, &addr, 120.0, &params, 0).unwrap();
    let easy = evaluate(&dir, &addr, 5.0, &params, 1).unwrap();
    assert!(hard < easy, "阈值高通关率应更低: hard(120)={hard} easy(5)={easy}");
    assert_eq!(hard, 0.0, "阈值 120 在 120 tick 内按不够，通关率 0");
    assert!(easy > 0.5, "阈值 5 很容易达成，通关率应高: {easy}");
    let after = snapshot_bytes(&dir);
    assert_eq!(before, after, "evaluate 不许改源文件");
}
