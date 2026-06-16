//! `vitric gate` 端到端：交付门禁的全部裁决路径。
//!
//! 立场锁定：通关录像是不可伪造的交付证书——
//! 真通关录像放行；篡改一帧拒发；没赢的录像拒发；没录像/没门禁直接拒绝。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use vitric_cli::runtime::Runtime;
use vitric_sim::Recording;

/// 把 coin-run 复制一份到临时目录（测试要改清单/录像，不能动共享示例）。
fn copy_example(tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run");
    let dst = std::env::temp_dir().join(format!("vitric-gate-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    for sub in ["", "scenes", "rules", "scripts", "assets", "sounds", "qa"] {
        fs::create_dir_all(dst.join(sub)).unwrap();
    }
    for rel in [
        "vitric.json",
        "schema.json",
        "animations.json",
        "scenes/main.json",
        "rules/game.json",
        "scripts/systems.js",
        "sounds/coin.wav",
    ] {
        fs::copy(src.join(rel), dst.join(rel)).unwrap();
    }
    for entry in fs::read_dir(src.join("assets")).unwrap() {
        let p = entry.unwrap().path();
        fs::copy(&p, dst.join("assets").join(p.file_name().unwrap())).unwrap();
    }
    dst
}

/// 程序化录一局：按住右键 60 tick 吃满三枚金币 → game-won（即 QA 真打通关的等价物）。
fn record_win(dir: &Path) -> Recording {
    let (mut sim, mut rt) = Runtime::boot(dir).unwrap();
    sim.start_recording();
    sim.inject_input("right", "pressed");
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    sim.stop_recording().unwrap()
}

/// 录一局没赢的：纯挂机 10 tick，没有任何输入，game-won 不可能触发。
fn record_idle(dir: &Path) -> Recording {
    let (mut sim, mut rt) = Runtime::boot(dir).unwrap();
    sim.start_recording();
    for _ in 0..10 {
        sim.step(&mut rt).unwrap();
    }
    sim.stop_recording().unwrap()
}

fn write_recording(dir: &Path, rel: &str, rec: &Recording) {
    fs::write(dir.join(rel), serde_json::to_string(rec).unwrap()).unwrap();
}

/// 往清单里写 gates 声明（导演的动作）。
fn set_gates(dir: &Path, gates: Value) {
    let path = dir.join("vitric.json");
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    manifest["gates"] = gates;
    fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
}

fn run_gate(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vitric")).arg("gate").arg(dir).output().unwrap()
}

/// stdout 的 JSON 报告（没有报告时 panic，带上下文好排查）。
fn report_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout 应是 JSON 报告: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// 报告里找指定门的结果。
fn gate_entry<'a>(report: &'a Value, name_prefix: &str) -> &'a Value {
    report["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"].as_str().unwrap().starts_with(name_prefix))
        .unwrap_or_else(|| panic!("报告里应有 {name_prefix} 门: {report}"))
}

#[test]
fn winning_recording_earns_the_certificate() {
    let dir = copy_example("win");
    write_recording(&dir, "qa/clear.json", &record_win(&dir));
    set_gates(
        &dir,
        json!({
            "playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}],
            "check": true,
            "max_ticks": 100000
        }),
    );

    let out = run_gate(&dir);
    let report = report_of(&out);
    assert!(out.status.success(), "真通关录像应过门禁: {report}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(report["pass"], json!(true));
    assert_eq!(gate_entry(&report, "check")["status"], json!("pass"));
    let play = gate_entry(&report, "playthrough:qa/clear.json");
    assert_eq!(play["status"], json!("pass"), "{report}");
    assert_eq!(play["detail"]["must_emit"], json!("game-won"));
    assert_eq!(play["detail"]["ticks"], json!(60));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tampered_checkpoint_is_rejected_as_divergence() {
    let dir = copy_example("tamper");
    let rec = record_win(&dir);
    write_recording(&dir, "qa/clear.json", &rec);
    set_gates(&dir, json!({"playthroughs": [{"recording": "qa/clear.json"}]}));

    // 篡改一个校验点哈希 = 伪造证书。重放必须在该校验点报跑偏。
    let path = dir.join("qa/clear.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let h = doc["checkpoints"][1][1].as_u64().unwrap();
    doc["checkpoints"][1][1] = json!(h ^ 1);
    fs::write(&path, doc.to_string()).unwrap();

    let out = run_gate(&dir);
    assert!(!out.status.success(), "篡改的录像不能拿证书");
    let report = report_of(&out);
    assert_eq!(report["pass"], json!(false));
    let play = gate_entry(&report, "playthrough:");
    assert_eq!(play["status"], json!("fail"));
    let detail = play["detail"].as_str().unwrap();
    assert!(detail.contains("跑偏"), "要用现成的重放跑偏报错精确定位: {detail}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn recording_without_win_event_fails_must_emit() {
    let dir = copy_example("idle");
    write_recording(&dir, "qa/idle.json", &record_idle(&dir));
    set_gates(&dir, json!({"playthroughs": [{"recording": "qa/idle.json"}]}));

    let out = run_gate(&dir);
    assert!(!out.status.success(), "没赢的录像不是通关证书");
    let report = report_of(&out);
    let play = gate_entry(&report, "playthrough:");
    assert_eq!(play["status"], json!("fail"));
    let detail = play["detail"].as_str().unwrap();
    assert!(detail.contains("game-won"), "要点名缺的事件（默认 game-won）: {detail}");
    assert!(detail.contains("逐位一致"), "要说明重放本身是成立的，差的只是终局事件: {detail}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_recording_file_fails_explicitly() {
    let dir = copy_example("missing");
    set_gates(&dir, json!({"playthroughs": [{"recording": "qa/ghost.json"}]}));

    let out = run_gate(&dir);
    assert!(!out.status.success());
    let report = report_of(&out);
    let play = gate_entry(&report, "playthrough:qa/ghost.json");
    assert_eq!(play["status"], json!("fail"));
    let detail = play["detail"].as_str().unwrap();
    assert!(detail.contains("qa/ghost.json") && detail.contains("--record"), "要点名文件并指路录制方法: {detail}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn manifest_without_gates_is_rejected_not_passed() {
    let dir = copy_example("nogates");
    let out = run_gate(&dir);
    assert!(!out.status.success(), "无门禁项目不出证书");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("清单未声明 gates——无门禁项目不出证书"), "{stderr}");

    // 声明了 gates 但 playthroughs 为空 = 同款后门，一样拒绝
    set_gates(&dir, json!({"check": true}));
    let out = run_gate(&dir);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("playthroughs 为空"), "{stderr}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn assertion_violated_midway_fails_with_id_and_tick() {
    let dir = copy_example("asserts");
    write_recording(&dir, "qa/clear.json", &record_win(&dir));
    // 分数 ≤1 的断言在第二枚金币（约 tick 20）被吃时必然违反——重放中途抓现行
    fs::write(
        dir.join("qa/asserts.json"),
        json!([{"id": "score-cap", "if": [["@player.Score.value", "<=", 1]]}]).to_string(),
    )
    .unwrap();
    set_gates(
        &dir,
        json!({
            "playthroughs": [{"recording": "qa/clear.json"}],
            "assertions": "qa/asserts.json"
        }),
    );

    let out = run_gate(&dir);
    assert!(!out.status.success(), "重放中途违反断言不能拿证书");
    let report = report_of(&out);
    assert_eq!(report["pass"], json!(false));
    // 录像本身仍然是逐位一致的通关局——失败的只是断言门
    assert_eq!(gate_entry(&report, "playthrough:")["status"], json!("pass"));
    let asserts = gate_entry(&report, "assertions");
    assert_eq!(asserts["status"], json!("fail"));
    let violations = asserts["detail"]["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "持续违反只记首次（去抖）: {violations:?}");
    assert_eq!(violations[0]["id"], json!("score-cap"));
    let tick = violations[0]["tick"].as_u64().unwrap();
    assert!(tick > 0 && tick < 60, "要报首次违反的 tick: {tick}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn healthy_assertions_pass_and_recording_over_max_ticks_fails() {
    let dir = copy_example("maxticks");
    write_recording(&dir, "qa/clear.json", &record_win(&dir));
    fs::write(
        dir.join("qa/asserts.json"),
        json!([{"id": "score-sane", "if": [["@player.Score.value", "<=", 3]]}]).to_string(),
    )
    .unwrap();
    // 断言健康 + 长度合规：全绿
    set_gates(
        &dir,
        json!({
            "playthroughs": [{"recording": "qa/clear.json"}],
            "assertions": "qa/asserts.json",
            "max_ticks": 60
        }),
    );
    let out = run_gate(&dir);
    let report = report_of(&out);
    assert!(out.status.success(), "{report}");
    assert_eq!(gate_entry(&report, "assertions")["status"], json!("pass"));

    // 上限收紧到 30：60 tick 的录像被拒（防挂机注水证书）
    set_gates(&dir, json!({"playthroughs": [{"recording": "qa/clear.json"}], "max_ticks": 30}));
    let out = run_gate(&dir);
    assert!(!out.status.success());
    let detail = gate_entry(&report_of(&out), "playthrough:")["detail"].as_str().unwrap().to_string();
    assert!(detail.contains("max_ticks") && detail.contains("60"), "{detail}");

    fs::remove_dir_all(&dir).unwrap();
}

/// playtest 门：声明 require_clearable + max_soft_locks:0 的可通关、无软锁项目（coin-run）
/// → swarm 100% 通关、零软锁 → 该门 pass、gate 整体 pass。
#[test]
fn playtest_gate_passes_when_swarm_clears_with_no_softlock() {
    let dir = copy_example("pt-pass");
    write_recording(&dir, "qa/clear.json", &record_win(&dir));
    set_gates(
        &dir,
        json!({
            "playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}],
            "check": true,
            "max_ticks": 100000,
            "playtest": {
                "sessions": 16,
                "max_ticks": 200,
                "require_clearable": true,
                "max_soft_locks": 0,
                "forbid_numeric_breakage": true
            }
        }),
    );

    // 直接调库（任务要求的入口），不经子进程
    let (report, pass) = vitric_cli::gate::run(&dir).unwrap();
    assert!(pass, "可通关无软锁项目应整体 pass: {report}");
    let pt = gate_entry(&report, "playtest");
    assert_eq!(pt["status"], json!("pass"), "playtest 门应 pass: {report}");
    // pass 的 detail 带关键指标
    assert!(pt["detail"]["win_rate"].as_f64().unwrap() > 0.0);
    assert_eq!(pt["detail"]["soft_locks"], json!(0));

    fs::remove_dir_all(&dir).unwrap();
}

/// playtest 门：声明同款门槛但 swarm 会撞出软锁的项目（gate-softlock fixture：按 key 通关、
/// 按 seal 永久封死再也赢不了 → 部分局冻结成软锁）→ playthrough 仍 pass（可通关），但
/// playtest 门 fail（软锁数超 max_soft_locks:0）→ gate 整体 fail，detail 含违反的断言名。
#[test]
fn playtest_gate_fails_on_softlock_with_violated_assertion() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vitric-playtest/tests/fixtures/gate-softlock");

    let (report, pass) = vitric_cli::gate::run(&dir).unwrap();
    assert!(!pass, "撞出软锁应整体 fail: {report}");
    // 录像门仍 pass：游戏确实可通关，差的只是 playtest 契约
    assert_eq!(gate_entry(&report, "playthrough:")["status"], json!("pass"), "{report}");
    let pt = gate_entry(&report, "playtest");
    assert_eq!(pt["status"], json!("fail"), "playtest 门应 fail: {report}");
    let violations = pt["detail"]["violations"].as_array().unwrap();
    let names: Vec<&str> =
        violations.iter().map(|v| v["assertion"].as_str().unwrap()).collect();
    assert!(names.contains(&"max_soft_locks"), "应点名违反的断言 max_soft_locks: {names:?}");
    // 实际值带出来便于对账
    assert!(pt["detail"]["metrics"]["soft_locks"].as_u64().unwrap() >= 1);
}

/// 向后兼容：没声明 gates.playtest 的项目，报告里不出现 playtest 门，行为不变。
#[test]
fn no_playtest_gate_means_no_playtest_door() {
    let dir = copy_example("no-pt");
    write_recording(&dir, "qa/clear.json", &record_win(&dir));
    set_gates(
        &dir,
        json!({"playthroughs": [{"recording": "qa/clear.json"}], "check": true}),
    );
    let (report, pass) = vitric_cli::gate::run(&dir).unwrap();
    assert!(pass, "{report}");
    let has_playtest = report["gates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["name"] == json!("playtest"));
    assert!(!has_playtest, "没声明 gates.playtest 就不该有 playtest 门: {report}");

    fs::remove_dir_all(&dir).unwrap();
}
