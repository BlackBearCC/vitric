//! vitric check 对帧进口图集产物（*-atlas.json sidecar）的校验：
//! 图集缺失 / 帧表非法 / uv（rect）越界 / 帧图引用缺失，每条都带路径 + VDxxx 码红灯；
//! 合法产物 check 绿灯。

use std::fs;
use std::path::PathBuf;

use vitric_cli::runtime;

/// 最小可 check 项目（带 assets/）。
fn make_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-framescheck-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "assets"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        r#"{"name":"fc","schema":"schema.json","entry":"scenes/main.json",
            "scenes":["scenes/main.json"],"seed":1}"#,
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        r#"{"components":{"Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[{"name":"stage","components":{"Position":{"x":0,"y":0}}}]}"#,
    )
    .unwrap();
    dir
}

fn write_png(dir: &std::path::Path, rel: &str) {
    let p = dir.join("assets").join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let file = fs::File::create(&p).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&[255u8; 2 * 2 * 4]).unwrap();
}

fn write_sidecar(dir: &std::path::Path, body: &str) {
    fs::write(dir.join("assets").join("clip-atlas.json"), body).unwrap();
}

/// 合法产物：图集 png 在、帧表合法、rect 不越界、帧图都在 → check 绿灯。
#[test]
fn valid_atlas_products_pass() {
    let dir = make_project("valid");
    write_png(&dir, "clip-atlas.png");
    write_png(&dir, "clip/frame000.png");
    write_sidecar(
        &dir,
        r#"{"clip":"clip","atlas":"clip-atlas.png","compressed":null,"atlas_size":[2,2],
            "frames":[{"image":"clip/frame000.png","rect":[0,0,2,2],"uv":[0,0,1,1],"trim_offset":[0,0],"stay":1}]}"#,
    );
    runtime::check(&dir).expect("合法图集产物 check 该过");
}

/// 图集 png 缺失 → 红灯，点名图集名。
#[test]
fn missing_atlas_png_fails() {
    let dir = make_project("noatlas");
    write_png(&dir, "clip/frame000.png");
    // atlas 字段指向不存在的 clip-atlas.png（没 write_png 它）
    write_sidecar(
        &dir,
        r#"{"clip":"clip","atlas":"clip-atlas.png","compressed":null,"atlas_size":[2,2],
            "frames":[{"image":"clip/frame000.png","rect":[0,0,2,2]}]}"#,
    );
    let err = runtime::check(&dir).expect_err("图集缺失 check 必须红灯");
    assert!(err.contains("VD0A3"), "图集缺失错误码: {err}");
    assert!(err.contains("clip-atlas.png"), "点名图集: {err}");
}

/// 帧表非法（缺 frames 数组）→ 红灯。
#[test]
fn missing_frame_table_fails() {
    let dir = make_project("noframes");
    write_png(&dir, "clip-atlas.png");
    write_sidecar(
        &dir,
        r#"{"clip":"clip","atlas":"clip-atlas.png","compressed":null,"atlas_size":[2,2]}"#,
    );
    let err = runtime::check(&dir).expect_err("缺帧表 check 必须红灯");
    assert!(err.contains("VD0A6"), "缺帧表错误码: {err}");
}

/// uv（rect）越界 → 红灯，带路径。
#[test]
fn rect_out_of_bounds_fails() {
    let dir = make_project("oob");
    write_png(&dir, "clip-atlas.png");
    write_png(&dir, "clip/frame000.png");
    // rect [0,0,4,4] 越出 atlas_size [2,2]
    write_sidecar(
        &dir,
        r#"{"clip":"clip","atlas":"clip-atlas.png","compressed":null,"atlas_size":[2,2],
            "frames":[{"image":"clip/frame000.png","rect":[0,0,4,4]}]}"#,
    );
    let err = runtime::check(&dir).expect_err("rect 越界 check 必须红灯");
    assert!(err.contains("VD0A8") && err.contains("越出"), "越界错误码 + 措辞: {err}");
    assert!(err.contains("clip-atlas.json"), "带 sidecar 路径: {err}");
}

/// 帧图引用缺失 → 红灯，点名帧图。
#[test]
fn missing_frame_image_fails() {
    let dir = make_project("noframeimg");
    write_png(&dir, "clip-atlas.png");
    // 不写 frame000.png
    write_sidecar(
        &dir,
        r#"{"clip":"clip","atlas":"clip-atlas.png","compressed":null,"atlas_size":[2,2],
            "frames":[{"image":"clip/frame000.png","rect":[0,0,2,2]}]}"#,
    );
    let err = runtime::check(&dir).expect_err("帧图缺失 check 必须红灯");
    assert!(err.contains("VD0AA"), "帧图缺失错误码: {err}");
    assert!(err.contains("frame000.png"), "点名帧图: {err}");
}
