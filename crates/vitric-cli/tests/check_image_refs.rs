//! `vitric check` 扫脚本/规则字面贴图引用的端到端：
//! 真实事故原型——脚本 ctx.spawn 用了不存在的 "dust.png"，check 绿灯、
//! 游戏跑到一半渲染硬炸。现在这类引用必须在 check 期就红灯。

use std::fs;
use std::path::{Path, PathBuf};

use vitric_cli::runtime;

/// 最小可 check 项目：一个组件、一个实体、一个 .js 脚本、一个规则文件。
/// `script` / `rule_doc` 由各测试注入要扫的内容。
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

/// 1x1 不透明白 PNG 写进 assets/（让缺失的引用变"存在"）。
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
    // 把图补进 assets/ 之后同一项目过 check
    write_png(&dir.join("assets/missing.png"));
    runtime::check(&dir).expect("图补上了 check 该过");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_ignores_dynamic_image_concatenation() {
    // 动态拼接是文档化局限：不报（误报会让 agent 学会无视 check）
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
