//! `vitric check` 对坏序列的逐项报错（端到端）：at 乱序 / 未知动作 / spawn 缺图 /
//! sound 缺音效 / emit load-scene 指向未声明场景——每条都带路径，check 红灯。

use std::fs;
use std::path::{Path, PathBuf};

use vitric_cli::runtime;

/// 最小可 check 项目：一个组件、一个空场景、可注入的序列文件。
fn make_project(tag: &str, sequence: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-seqcheck-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "sequences", "assets", "sounds"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        r#"{"name":"seqcheck","schema":"schema.json","entry":"scenes/main.json",
            "scenes":["scenes/main.json"],"sequences":["sequences/s.json"],"seed":1}"#,
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        r#"{"components":{
            "Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}},
            "Sprite":{"fields":{"w":{"type":"number"},"h":{"type":"number"},
                                 "image":{"type":"text","default":""}}}
        }}"#,
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[{"name":"stage","components":{"Position":{"x":0,"y":0}}}]}"#,
    )
    .unwrap();
    fs::write(dir.join("sequences/s.json"), sequence).unwrap();
    dir
}

fn write_png(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 1, 1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(&[255, 255, 255, 255]).unwrap();
}

#[test]
fn out_of_order_at_fails_check_with_path() {
    let dir = make_project(
        "order",
        r#"{"id":"s","steps":[
            {"at":30,"do":{"emit":"a"}},
            {"at":10,"do":{"emit":"b"}}
        ]}"#,
    );
    let err = runtime::check(&dir).expect_err("at 乱序 check 必须红灯");
    assert!(err.contains("VD062"), "at 乱序错误码: {err}");
    assert!(err.contains("sequences/s.json#/steps/1/at"), "报错点到具体条目: {err}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unknown_action_fails_check_listing_kinds() {
    let dir = make_project("action", r#"{"id":"s","steps":[{"at":0,"do":{"teleport":"home"}}]}"#);
    let err = runtime::check(&dir).expect_err("未知动作 check 必须红灯");
    assert!(err.contains("VD064"), "未知动作错误码: {err}");
    assert!(err.contains("tween") && err.contains("wait"), "列出可用动作: {err}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn spawn_missing_image_fails_then_passes_with_it() {
    let dir = make_project(
        "image",
        r#"{"id":"s","steps":[{"at":0,"do":{"spawn":{"components":{
            "Sprite":{"image":"ghost.png","w":1,"h":1}
        }}}}]}"#,
    );
    let err = runtime::check(&dir).expect_err("序列 spawn 缺图 check 必须红灯");
    assert!(err.contains("ghost.png"), "报错点名贴图: {err}");
    assert!(err.contains("sequences/s.json"), "报错点名序列文件: {err}");
    // 把图补进 assets/ 后过 check
    write_png(&dir.join("assets/ghost.png"));
    runtime::check(&dir).expect("图补上了 check 该过");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sound_action_missing_file_fails_check() {
    let dir = make_project("sound", r#"{"id":"s","steps":[{"at":0,"do":{"sound":"nope.wav"}}]}"#);
    let err = runtime::check(&dir).expect_err("序列 sound 缺音效 check 必须红灯");
    assert!(err.contains("nope.wav"), "报错点名音效: {err}");
    // 放一个同名文件后过
    fs::write(dir.join("sounds/nope.wav"), b"RIFF").unwrap();
    runtime::check(&dir).expect("音效补上了 check 该过");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn emit_load_scene_to_undeclared_scene_fails_check() {
    let dir = make_project(
        "scene",
        r#"{"id":"s","steps":[{"at":0,"do":{"emit":"load-scene","data":{"scene":"scenes/void.json"}}}]}"#,
    );
    let err = runtime::check(&dir).expect_err("emit load-scene 指向未声明场景 check 必须红灯");
    assert!(err.contains("scenes/void.json"), "报错点名目标场景: {err}");
    fs::remove_dir_all(&dir).unwrap();
}
