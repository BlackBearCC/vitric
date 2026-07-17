//! `vitric team` / `vitric turf` end-to-end: the collaboration blackboard and turf enforcement.
//!
//! Position lock: the blackboard reads state mechanically from files and always exits 0 (a state tool is not a gate);
//! the turf table is engine-defined discipline — a role crossing its boundary must exit 1 and name each violation, while the director may write everything.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// Copy coin-run into a temp directory (tests need to add/remove GDD/recordings and must not touch the shared example).
fn copy_example(tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run");
    let dst = std::env::temp_dir().join(format!("vitric-team-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    for sub in ["", "scenes", "rules", "scripts", "assets", "sounds"] {
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

fn vitric(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vitric")).args(args).output().unwrap()
}

fn report_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout 不是 JSON 报告: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

// ---- vitric team ----

#[test]
fn team_reports_sane_counts_and_always_exits_zero() {
    let dir = copy_example("counts");
    let out = vitric(&["team", dir.to_str().unwrap()]);
    // The blackboard is a state tool, not a gate: even with a pile of blockers (missing GDD/palette/gates) it must exit 0
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let r = report_of(&out);

    assert_eq!(r["project"], "coin-run");
    assert_eq!(r["contract"]["gdd"], false);
    assert_eq!(r["contract"]["manifest"], true);
    assert_eq!(r["contract"]["schema_parses"], true);

    // Counts derived from the real example (values locked — if coin-run content changes this must change too, that is the blackboard's contract)
    assert_eq!(r["roles"]["art"]["assets"], 8);
    assert_eq!(r["roles"]["art"]["palette"], false);
    assert_eq!(r["roles"]["art"]["normals"], 0);
    assert_eq!(r["roles"]["level"]["scenes"], 1);
    assert_eq!(r["roles"]["level"]["entities"], 6);
    assert_eq!(r["roles"]["gameplay"]["rules"], 4);
    assert_eq!(r["roles"]["gameplay"]["systems"], 1);
    assert_eq!(r["roles"]["gameplay"]["fns"], 1);
    assert_eq!(r["roles"]["audio"]["sounds"], 1);
    assert_eq!(r["roles"]["audio"]["referenced"], 1);
    assert_eq!(r["roles"]["audio"]["missing"].as_array().unwrap().len(), 0);
    assert_eq!(r["roles"]["narrative"]["text_entities"], 1);
    assert_eq!(r["roles"]["qa"]["asserts"], false);
    assert_eq!(r["roles"]["qa"]["recordings"], 0);

    // Blocker hints: no GDD, no palette, no gates must all be named
    let blocking = serde_json::to_string(&r["blocking"]).unwrap();
    assert!(blocking.contains("GDD.md"), "{blocking}");
    assert!(blocking.contains("palette.json"), "{blocking}");
    assert!(blocking.contains("gates"), "{blocking}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn team_sees_contract_gates_and_missing_recording_without_running_gate() {
    let dir = copy_example("gates");
    fs::write(dir.join("GDD.md"), "# 合同\n").unwrap();
    fs::write(dir.join("palette.json"), "{\"colors\": []}").unwrap();
    fs::create_dir_all(dir.join("qa")).unwrap();
    fs::write(dir.join("qa/asserts.json"), "[]").unwrap();
    // Declare a gate but the recording is not yet recorded: the blackboard must report "recording missing", but only checks file existence, no replay (the verdict belongs to gate)
    let manifest_path = dir.join("vitric.json");
    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["gates"] = serde_json::json!({
        "playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}],
        "assertions": "qa/asserts.json",
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = vitric(&["team", dir.to_str().unwrap()]);
    assert!(out.status.success(), "黑板永远退出 0");
    let r = report_of(&out);
    assert_eq!(r["contract"]["gdd"], true);
    assert_eq!(r["roles"]["art"]["palette"], true);
    assert_eq!(r["roles"]["qa"]["asserts"], true);
    assert_eq!(r["gates"]["declared"], true);
    assert_eq!(r["gates"]["playthroughs"], 1);
    assert_eq!(r["gates"]["recordings_missing"], serde_json::json!(["qa/clear.json"]));
    assert_eq!(r["gates"]["assertions_present"], true);
    let blocking = serde_json::to_string(&r["blocking"]).unwrap();
    assert!(blocking.contains("qa/clear.json"), "缺失录像要进卡点: {blocking}");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn team_degrades_gracefully_when_project_is_broken() {
    // Day one of a project, not even vitric.json exists: the blackboard still produces a report + an explicit error field, still exits 0
    let dir = std::env::temp_dir().join(format!("vitric-team-{}-broken", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let out = vitric(&["team", dir.to_str().unwrap()]);
    assert!(out.status.success(), "残缺项目不挡黑板");
    let r = report_of(&out);
    assert_eq!(r["project"], Value::Null);
    assert_eq!(r["contract"]["manifest"], false);
    assert!(r["contract"]["manifest_error"].is_string());
    assert_eq!(r["contract"]["schema_parses"], false);
    // Cannot be assembled: systems/fns are null (unknown) rather than 0 (misreported)
    assert!(r["roles"]["gameplay"]["systems"].is_null());
    assert!(r["roles"]["gameplay"]["load_error"].is_string());
    fs::remove_dir_all(&dir).unwrap();

    // A non-existent directory is a hard error (the blackboard can face a broken project, but not a non-existent directory)
    let out = vitric(&["team", "/nonexistent/vitric-team-dir"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("不存在"));
}

// ---- vitric turf ----

#[test]
fn turf_art_touching_scenes_is_a_violation_exit_1() {
    let dir = copy_example("turf-art-scenes");
    let out = vitric(&["turf", dir.to_str().unwrap(), "--role", "art", "scenes/main.json"]);
    assert_eq!(out.status.code(), Some(1), "越界必须退出 1");
    let r = report_of(&out);
    assert_eq!(r["pass"], false);
    let v = &r["violations"][0];
    assert_eq!(v["file"], "scenes/main.json");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("导演"), "处方要指路导演: {reason}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn turf_art_inside_own_turf_passes() {
    let dir = copy_example("turf-art-ok");
    let out = vitric(&[
        "turf",
        dir.to_str().unwrap(),
        "--role",
        "art",
        "assets/x.png",
        "animations.json",
        "palette.json",
    ]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let r = report_of(&out);
    assert_eq!(r["pass"], true);
    assert_eq!(r["violations"].as_array().unwrap().len(), 0);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn turf_director_may_touch_everything_inside_project() {
    let dir = copy_example("turf-director");
    let out = vitric(&[
        "turf",
        dir.to_str().unwrap(),
        "--role",
        "director",
        "GDD.md",
        "schema.json",
        "vitric.json",
        "scenes/main.json",
        "rules/game.json",
        "assets/x.png",
    ]);
    assert!(out.status.success(), "导演可写项目内一切: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(report_of(&out)["pass"], true);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn turf_files_outside_project_violate_even_for_director() {
    let dir = copy_example("turf-outside");
    // An absolute path outside the project + a relative .. escape: both are violations, even for the director
    let out = vitric(&[
        "turf",
        dir.to_str().unwrap(),
        "--role",
        "director",
        "/etc/passwd",
        "../escape.json",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let r = report_of(&out);
    assert_eq!(r["violations"].as_array().unwrap().len(), 2, "{r}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn turf_narrative_shares_scenes_and_audio_owns_sounds() {
    let dir = copy_example("turf-share");
    // Narrative and level share scenes/ (narrative lives in scene Text)
    let out = vitric(&["turf", dir.to_str().unwrap(), "--role", "narrative", "scenes/main.json"]);
    assert!(out.status.success(), "narrative 可写 scenes/");
    // Audio is its own lane: sounds/ belongs to audio, gameplay touching sounds/ is a boundary violation
    let out = vitric(&["turf", dir.to_str().unwrap(), "--role", "gameplay", "sounds/jump.wav"]);
    assert_eq!(out.status.code(), Some(1), "gameplay 不许碰 sounds/");
    let out = vitric(&["turf", dir.to_str().unwrap(), "--role", "audio", "sounds/jump.wav"]);
    assert!(out.status.success(), "audio 可写 sounds/");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn turf_usage_errors_are_explicit() {
    let dir = copy_example("turf-usage");
    // Unknown role: hard error and lists all available options
    let out = vitric(&["turf", dir.to_str().unwrap(), "--role", "hacker", "x.png"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("director") && err.contains("art") && err.contains("qa"), "{err}");
    // No changed files given: hard error, not an empty pass
    let out = vitric(&["turf", dir.to_str().unwrap(), "--role", "art"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("改动文件"));
    fs::remove_dir_all(&dir).unwrap();
}
