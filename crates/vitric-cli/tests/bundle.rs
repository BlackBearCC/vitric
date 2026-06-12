//! `vitric bundle` 端到端：发行打包的全部裁决路径。
//!
//! 立场锁定：无证书不发行——gate 不 PASS 不出包；发行包是自包含单文件：
//! 无参数即玩（开窗），`run-embedded` 透传选项可无头冒烟，带参数仍是完整 CLI。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use vitric_cli::bundle;
use vitric_cli::runtime::Runtime;
use vitric_sim::Recording;

// ---- 测试夹具（同 tests/gate.rs 的套路：复制 coin-run，程序化录通关）----

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

/// 程序化录一局通关：按住右键 60 tick 吃满三枚金币 → game-won。
fn record_win(dir: &Path) -> Recording {
    let (mut sim, mut rt) = Runtime::boot(dir).unwrap();
    sim.start_recording();
    sim.inject_input("right", "pressed");
    for _ in 0..60 {
        sim.step(&mut rt).unwrap();
    }
    sim.stop_recording().unwrap()
}

/// 录一局没赢的：纯挂机 10 tick——gate 必拒，bundle 必须跟着拒。
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

/// 把项目调成 gate 全绿（录通关 + 声明门禁）。
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

// ---- 容器格式：纯函数级（pack/unpack/seal/open 互逆，损坏显式报错）----

#[test]
fn container_roundtrip_is_byte_identical() {
    // 含二进制内容（全 256 字节值）、空文件、嵌套路径——发行包里就是这些形态
    let mut files = BTreeMap::new();
    files.insert("vitric.json".to_string(), br#"{"name":"x"}"#.to_vec());
    files.insert("assets/bin.png".to_string(), (0..=255u8).cycle().take(4096).collect());
    files.insert("qa/empty.json".to_string(), Vec::new());

    let engine = b"\x7fELF-fake-engine-bytes".to_vec();
    let bundle_bytes = bundle::seal(engine.clone(), &files).unwrap();
    // 引擎字节原样在前（发行包同时仍是那个引擎）
    assert_eq!(&bundle_bytes[..engine.len()], &engine[..]);

    let (hash, unpacked) = bundle::open(&bundle_bytes).unwrap().expect("应检出尾标");
    assert_eq!(unpacked, files, "解包必须逐字节还原");
    assert_ne!(hash, 0);

    // 无尾标 = 普通引擎，不是错误
    assert_eq!(bundle::open(&engine).unwrap(), None);
    // 截断的包：有魔数但长度对不上，必须显式报损坏，不能静默当普通引擎
    let truncated = [&bundle_bytes[engine.len() + 3..]].concat();
    let err = bundle::open(&truncated).unwrap_err();
    assert!(err.contains("损坏"), "{err}");
}

#[test]
fn archive_rejects_unsafe_paths_and_trailing_garbage() {
    // 解包按路径写文件——越界路径必须拒（被篡改的发行包不能写到目录外）
    for bad in ["../evil", "/abs", "a/../b", "a//b", "a\\b", ""] {
        let mut files = BTreeMap::new();
        files.insert(bad.to_string(), vec![1u8]);
        let archive = bundle::pack_archive(&files).unwrap();
        let err = bundle::unpack_archive(&archive).unwrap_err();
        assert!(err.contains("安全相对路径"), "{bad:?} 应被拒: {err}");
    }
    // 尾部多余字节 = 包损坏，不能半解
    let archive = bundle::pack_archive(&BTreeMap::new()).unwrap();
    let err = bundle::unpack_archive(&[archive, vec![0u8; 3]].concat()).unwrap_err();
    assert!(err.contains("多余"), "{err}");
}

// ---- 门禁裁决：无证书不发行 ----

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
    // 没赢的录像：gate 的 must_emit 门必 fail
    fs::write(dir.join("qa/idle.json"), serde_json::to_string(&record_idle(&dir)).unwrap())
        .unwrap();
    set_gates(&dir, json!({"playthroughs": [{"recording": "qa/idle.json"}]}));

    let out = vitric(&["bundle", dir.to_str().unwrap()]);
    assert!(!out.status.success(), "gate 不过不出包");
    // 拒绝时把 gate 报告原样给出来（差在哪看得见），结论在 stderr
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["pass"], json!(false));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("无证书不发行"), "{stderr}");
    // 没出包文件
    assert!(!Path::new("coin-run-linux").exists());
    fs::remove_dir_all(&dir).unwrap();
}

// ---- 出包 + 发行包行为（解包运行/选项透传/仍是完整 CLI/拒套娃）----

#[test]
fn gated_project_bundles_into_a_self_contained_player() {
    let dir = copy_example("ship");
    make_gated(&dir);
    // 排除项就位：玩家存档、隐藏文件、以及同名旧包（重打包覆盖自己时不能把旧包打进新包）
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

    // 包内容：通关录像（证书）在，saves/ 不在
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

    // run-embedded 透传 --ticks：无头冒烟，横幅项目名 = 内嵌项目
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

    // 解包目录按包哈希唯一，项目落了地（saves 将来也长在这里随包持久）
    let extracted = std::env::temp_dir().join(format!("vitric-{hash:016x}"));
    assert!(extracted.join("vitric.json").is_file(), "{}", extracted.display());

    // 带参数 = 正常 CLI：发行包同时也是完整引擎（能 gate 别的项目）
    let as_cli = Command::new(&out_file).args(["gate", dir.to_str().unwrap()]).output().unwrap();
    assert!(as_cli.status.success(), "发行包带参数应是完整 CLI: {}",
        String::from_utf8_lossy(&as_cli.stderr));

    // 套娃打包必须拒：发行包不能再当 bundle 的引擎
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
    // 缺省输出名写进当前目录：把 cwd 钉在临时项目目录里验证
    let out = Command::new(env!("CARGO_BIN_EXE_vitric"))
        .args(["bundle", dir.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let expected = format!("coin-run-{}{}", std::env::consts::OS, std::env::consts::EXE_SUFFIX);
    assert_eq!(last_stdout_json(&out)["out"], json!(expected));
    assert!(dir.join(&expected).is_file());
    // 第一份产物先清掉：它不该被第二次打包卷进去（排除只认"本次输出"）
    fs::remove_file(dir.join(&expected)).unwrap();

    // --engine 指定 .exe 引擎：缺省名跟引擎平台走（windows），尾标附在给定引擎字节后
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

    // 普通引擎（非发行包）跑 run-embedded：显式报"不是发行包"，不静默
    let plain = vitric(&["run-embedded", "--ticks", "1"]);
    assert!(!plain.status.success());
    assert!(String::from_utf8_lossy(&plain.stderr).contains("不是发行包"));

    fs::remove_dir_all(&dir).unwrap();
}
