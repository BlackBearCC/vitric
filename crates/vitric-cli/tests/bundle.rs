//! `vitric bundle` end-to-end: all decision paths of release packaging.
//!
//! Stance locked: no certificate, no release — gate must PASS to ship; the release package is a
//! self-contained single file: play with no args (opens a window), `run-embedded` passes through
//! options for headless smoke, with args it is still a full CLI.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use vitric_cli::bundle;
use vitric_cli::runtime::Runtime;
use vitric_sim::Recording;

// ---- Test fixtures (same pattern as tests/gate.rs: copy coin-run, programmatically record a
// clear) ----

fn copy_example(tag: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/coin-run");
    let dst = std::env::temp_dir().join(format!("vitric-bundle-{}-{tag}", std::process::id()));
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

/// Programmatically record a clear run: hold right for 60 ticks to eat all three coins →
/// game-won.
fn record_win(dir: &Path) -> Recording {
    let (mut sim, mut rt) = Runtime::boot(dir).unwrap();
    sim.start_recording();
    sim.inject_input("right", "pressed");
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    sim.stop_recording().unwrap()
}

/// Record a non-winning run: pure idle for 10 ticks — gate must reject, bundle must follow.
fn record_idle(dir: &Path) -> Recording {
    let (mut sim, mut rt) = Runtime::boot(dir).unwrap();
    sim.start_recording();
    for _ in 0..10 {
        sim.step(&mut rt).unwrap();
    }
    sim.stop_recording().unwrap()
}

fn set_gates(dir: &Path, gates: Value) {
    let path = dir.join("vitric.json");
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    manifest["gates"] = gates;
    fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
}

/// Make the project all-green on gate (record a clear + declare the gate).
fn make_gated(dir: &Path) {
    fs::write(dir.join("qa/clear.json"), serde_json::to_string(&record_win(dir)).unwrap())
        .unwrap();
    set_gates(
        dir,
        json!({"playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}]}),
    );
}

fn vitric(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vitric")).args(args).output().unwrap()
}

fn last_stdout_json(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_else(|| {
        panic!("stdout 应有 JSON 输出\nstderr: {}", String::from_utf8_lossy(&out.stderr))
    });
    serde_json::from_str(line).unwrap_or_else(|e| panic!("stdout 末行应是 JSON: {e}\n{text}"))
}

// ---- Container format: pure-function level (pack/unpack/seal/open are inverse, corruption
// reports explicitly) ----

#[test]
fn container_roundtrip_is_byte_identical() {
    // Contains binary content (all 256 byte values), an empty file, nested paths — these are the
    // shapes inside a release package
    let mut files = BTreeMap::new();
    files.insert("vitric.json".to_string(), br#"{"name":"x"}"#.to_vec());
    files.insert("assets/bin.png".to_string(), (0..=255u8).cycle().take(4096).collect());
    files.insert("qa/empty.json".to_string(), Vec::new());

    let engine = b"\x7fELF-fake-engine-bytes".to_vec();
    let bundle_bytes = bundle::seal(engine.clone(), &files).unwrap();
    // Engine bytes verbatim at the front (the release package is also still that engine)
    assert_eq!(&bundle_bytes[..engine.len()], &engine[..]);

    let (hash, unpacked) = bundle::open(&bundle_bytes).unwrap().expect("应检出尾标");
    assert_eq!(unpacked, files, "解包必须逐字节还原");
    assert_ne!(hash, 0);

    // No trailer = a plain engine, not an error
    assert_eq!(bundle::open(&engine).unwrap(), None);
    // A truncated package: has the magic but length mismatches; must explicitly report corruption,
    // not silently treat it as a plain engine
    let truncated = [&bundle_bytes[engine.len() + 3..]].concat();
    let err = bundle::open(&truncated).unwrap_err();
    assert!(err.contains("损坏"), "{err}");
}

#[test]
fn archive_rejects_unsafe_paths_and_trailing_garbage() {
    // Unpacking writes files by path — out-of-bounds paths must be rejected (a tampered release
    // package must not write outside the directory)
    for bad in ["../evil", "/abs", "a/../b", "a//b", "a\\b", ""] {
        let mut files = BTreeMap::new();
        files.insert(bad.to_string(), vec![1u8]);
        let archive = bundle::pack_archive(&files).unwrap();
        let err = bundle::unpack_archive(&archive).unwrap_err();
        assert!(err.contains("安全相对路径"), "{bad:?} 应被拒: {err}");
    }
    // Trailing extra bytes = corrupted package, must not partially unpack
    let archive = bundle::pack_archive(&BTreeMap::new()).unwrap();
    let err = bundle::unpack_archive(&[archive, vec![0u8; 3]].concat()).unwrap_err();
    assert!(err.contains("多余"), "{err}");
}

// ---- Gate decision: no certificate, no release ----

#[test]
fn bundle_refuses_project_without_gates() {
    let dir = copy_example("nogates");
    let out = vitric(&["bundle", dir.to_str().unwrap()]);
    assert!(!out.status.success(), "无门禁项目不发行");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("gates"), "要指路声明门禁: {stderr}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bundle_refuses_failing_gate_with_the_gate_report() {
    let dir = copy_example("idle");
    // Non-winning recording: the gate's must_emit gate must fail
    fs::write(dir.join("qa/idle.json"), serde_json::to_string(&record_idle(&dir)).unwrap())
        .unwrap();
    set_gates(&dir, json!({"playthroughs": [{"recording": "qa/idle.json"}]}));

    let out = vitric(&["bundle", dir.to_str().unwrap()]);
    assert!(!out.status.success(), "gate 不过不出包");
    // On rejection, the gate report is given verbatim (visible where it fell short); the verdict
    // is in stderr
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["pass"], json!(false));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("无证书不发行"), "{stderr}");
    // No package file produced
    assert!(!Path::new("coin-run-linux").exists());
    fs::remove_dir_all(&dir).unwrap();
}

// ---- Packaging + release package behavior (unpack-run / option passthrough / still full CLI /
// refuse nesting) ----

#[test]
fn gated_project_bundles_into_a_self_contained_player() {
    let dir = copy_example("ship");
    make_gated(&dir);
    // Exclusions in place: player saves, hidden files, and a same-name previous package (when
    // re-packaging over itself, the old package must not be packed into the new one)
    fs::create_dir_all(dir.join("saves")).unwrap();
    fs::write(dir.join("saves/slot1.json"), "{}").unwrap();
    fs::write(dir.join(".hidden"), "x").unwrap();
    let out_file = dir.join("dist").join("game-bin");
    fs::create_dir_all(dir.join("dist")).unwrap();
    fs::write(&out_file, b"stale-previous-bundle").unwrap();

    let out = vitric(&["bundle", dir.to_str().unwrap(), "--out", out_file.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "全绿项目应出包\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let result = last_stdout_json(&out);
    assert_eq!(result["out"], json!(out_file.to_str().unwrap()));
    assert_eq!(result["project"], json!("coin-run"));
    let bytes = result["bytes"].as_u64().unwrap();
    assert_eq!(bytes, fs::metadata(&out_file).unwrap().len(), "报告的字节数要对账");

    // Package contents: the clear recording (certificate) is in, saves/ is not
    let bundle_bytes = fs::read(&out_file).unwrap();
    let (hash, files) = bundle::open(&bundle_bytes).unwrap().expect("出的包必须带尾标");
    assert!(files.contains_key("vitric.json"), "清单必须在包里");
    assert!(files.contains_key("qa/clear.json"), "通关录像是证书本体，必须随包");
    assert!(files.contains_key("assets/player.png"), "素材必须随包");
    assert!(
        !files.keys().any(|k| k.starts_with("saves/") || k.starts_with("dist/") || k.starts_with('.')),
        "saves/隐藏文件/输出文件自己不进包: {:?}",
        files.keys().collect::<Vec<_>>()
    );

    // run-embedded passes through --ticks: headless smoke, banner project name = embedded project
    let play = Command::new(&out_file).args(["run-embedded", "--ticks", "5"]).output().unwrap();
    assert!(
        play.status.success(),
        "发行包应能无头跑内嵌项目\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&play.stdout),
        String::from_utf8_lossy(&play.stderr)
    );
    let banner: Value =
        serde_json::from_str(String::from_utf8_lossy(&play.stdout).lines().next().unwrap())
            .unwrap();
    assert_eq!(banner["vitric"], json!("running"));
    assert_eq!(banner["project"], json!("coin-run"), "跑的必须是内嵌项目");

    // The unpack directory is unique by package hash; the project has landed (saves will later
    // live here too, persisted with the package)
    let extracted = std::env::temp_dir().join(format!("vitric-{hash:016x}"));
    assert!(extracted.join("vitric.json").is_file(), "{}", extracted.display());

    // With args = normal CLI: the release package is also a full engine (can gate other projects)
    let as_cli = Command::new(&out_file).args(["gate", dir.to_str().unwrap()]).output().unwrap();
    assert!(as_cli.status.success(), "发行包带参数应是完整 CLI: {}",
        String::from_utf8_lossy(&as_cli.stderr));

    // Nested packaging must be refused: a release package cannot be reused as bundle's engine
    let nested = Command::new(&out_file)
        .args(["bundle", dir.to_str().unwrap(), "--out", dir.join("dist/n2").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!nested.status.success());
    assert!(String::from_utf8_lossy(&nested.stderr).contains("已是发行包"));

    let _ = fs::remove_dir_all(&extracted);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn default_output_name_is_project_platform_and_engine_flag_targets_cross() {
    let dir = copy_example("name");
    make_gated(&dir);
    // Default output name is written to the current directory: pin cwd inside the temp project
    // directory to verify
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .args(["bundle", dir.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let expected = format!("coin-run-{}{}", std::env::consts::OS, std::env::consts::EXE_SUFFIX);
    assert_eq!(last_stdout_json(&out)["out"], json!(expected));
    assert!(dir.join(&expected).is_file());
    // Clear the first artifact first: it must not be swept into a second packaging (exclusion
    // only recognizes "this run's output")
    fs::remove_file(dir.join(&expected)).unwrap();

    // --engine specifies a .exe engine: the default name follows the engine platform (windows);
    // the trailer is appended after the given engine bytes
    let fake_engine = dir.join("engine-stub.exe");
    fs::write(&fake_engine, b"MZ-fake-windows-engine").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .args(["bundle", dir.to_str().unwrap(), "--engine", fake_engine.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(last_stdout_json(&out)["out"], json!("coin-run-windows.exe"));
    let bytes = fs::read(dir.join("coin-run-windows.exe")).unwrap();
    assert!(bytes.starts_with(b"MZ-fake-windows-engine"), "引擎字节原样在前");
    assert!(bundle::open(&bytes).unwrap().is_some());

    // A plain engine (not a release package) running run-embedded: explicitly reports "not a
    // release package", not silent
    let plain = vitric(&["run-embedded", "--ticks", "1"]);
    assert!(!plain.status.success());
    assert!(String::from_utf8_lossy(&plain.stderr).contains("不是发行包"));

    fs::remove_dir_all(&dir).unwrap();
}
