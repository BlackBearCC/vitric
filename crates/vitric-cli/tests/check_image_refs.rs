//! `vitric check` scans literal texture references in scripts/rules end-to-end:
//! real incident prototype — the script ctx.spawn used a non-existent "dust.png", check was green,
//! and the game hard-crashed mid-render. Now such references must go red during check.

use std::fs;
use std::path::{Path, PathBuf};

use vitric_cli::runtime;

/// Minimal checkable project: one component, one entity, one .js script, one rule file.
/// `script` / `rule_doc` are injected by each test with the content to scan.
fn make_project(tag: &str, script: &str, rule_doc: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-imgref-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "scripts", "rules", "assets"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        r#"{"name":"imgref-demo","schema":"schema.json","entry":"scenes/main.json",
            "scenes":["scenes/main.json"],"scripts":["scripts/fx.js"],
            "rules":["rules/main.json"],"seed":1}"#,
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        r#"{"components":{"Counter":{"fields":{"value":{"type":"int","default":0}}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[{"name":"c","components":{"Counter":{}}}]}"#,
    )
    .unwrap();
    fs::write(dir.join("scripts/fx.js"), script).unwrap();
    fs::write(dir.join("rules/main.json"), rule_doc).unwrap();
    dir
}

const EMPTY_RULES: &str = r#"{"rules": []}"#;

/// Write a 1x1 opaque white PNG into assets/ (to turn a missing reference "present").
fn write_png(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 1, 1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(&[255, 255, 255, 255]).unwrap();
}

#[test]
fn check_fails_on_script_literal_missing_png_then_passes_with_it() {
    let dir = make_project(
        "script",
        r#"vitric.fn("boom", (ctx) => { ctx.spawn({ Sprite: { image: "missing.png", w: 2, h: 2 } }); });"#,
        EMPTY_RULES,
    );
    let err = runtime::check(&dir).expect_err("脚本字面引用缺图，check 必须红灯");
    assert!(err.contains("missing.png"), "报错点名贴图: {err}");
    assert!(err.contains("scripts/fx.js"), "报错点名脚本文件: {err}");
    assert!(err.contains("动态拼接"), "局限要在错误里说清: {err}");
    // After dropping the image into assets/, the same project passes check
    write_png(&dir.join("assets/missing.png"));
    runtime::check(&dir).expect("图补上了 check 该过");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_ignores_dynamic_image_concatenation() {
    // Dynamic concatenation is a documented limitation: not reported (false positives would teach the agent to ignore check)
    let dir = make_project(
        "dynamic",
        r#"vitric.fn("burst", (ctx, args) => {
            ctx.spawn({ Sprite: { image: "dust_" + args.i + ".png", w: 1, h: 1 } });
        });"#,
        EMPTY_RULES,
    );
    runtime::check(&dir).expect("动态拼接不在字面量扫描范围，不该误报");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_fails_on_rule_spawn_missing_png() {
    let dir = make_project(
        "rule",
        "// 没有贴图引用的脚本\n",
        r#"{"rules": [{
            "id": "emit-dust",
            "on": {"event": "boom"},
            "do": [{"spawn": {"components": {"Sprite": {"image": "puff.png", "w": 1, "h": 1}}}}]
        }]}"#,
    );
    let err = runtime::check(&dir).expect_err("规则 spawn 缺图，check 必须红灯");
    assert!(err.contains("puff.png"), "报错点名贴图: {err}");
    assert!(err.contains("rules/main.json"), "报错点名规则文件: {err}");
    write_png(&dir.join("assets/puff.png"));
    runtime::check(&dir).expect("图补上了 check 该过");
    fs::remove_dir_all(&dir).unwrap();
}
