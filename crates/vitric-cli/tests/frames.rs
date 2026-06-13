//! vitric assets --frames（帧进口流水线）端到端锁定：
//! 去重 + trim + atlas + 统一色板 + 动画配置，全程确定性；配置喂
//! advance_animations 播放正确；BC7 字节数压缩比；check 校验坏产物。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use vitric_cli::frames::{self, FramesOptions};
use vitric_cli::runtime::advance_animations;
use vitric_data::Clip;
use vitric_ecs::World;

/// 每个测试一个独立临时项目，含空 assets/。
fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-frames-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    dir
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// 纯色帧。
fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
    c.to_vec().repeat((w * h) as usize)
}

/// 在透明背景上画一个不透明色块（用来测 trim）。
fn boxed(w: u32, h: u32, bx: u32, by: u32, bw: u32, bh: u32, c: [u8; 4]) -> Vec<u8> {
    let mut px = solid(w, h, [0, 0, 0, 0]);
    for y in by..by + bh {
        for x in bx..bx + bw {
            let o = ((y * w + x) * 4) as usize;
            px[o..o + 4].copy_from_slice(&c);
        }
    }
    px
}

/// 写一组序列帧到 <project>/seq/<name>/frameNNN.png，返回序列目录。
fn write_sequence(project: &Path, name: &str, frames: &[(u32, u32, Vec<u8>)]) -> PathBuf {
    let dir = project.join("seq").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for (i, (w, h, px)) in frames.iter().enumerate() {
        write_png(&dir.join(format!("frame{i:03}.png")), *w, *h, px);
    }
    dir
}

fn read_atlas_sidecar(project: &Path, clip: &str) -> serde_json::Value {
    let text =
        std::fs::read_to_string(project.join("assets").join(format!("{clip}-atlas.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn read_animations(project: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(project.join("animations.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// 去重：含重复相邻帧的序列，去重后帧数 + 停留计数对。
#[test]
fn dedup_collapses_and_counts_stays() {
    let project = temp_project("dedup");
    // A A A B A —— 三张 A 相邻塌成一张（停留 3），B 一张，末尾 A 一张
    let a = solid(8, 8, [200, 30, 30, 255]);
    let b = solid(8, 8, [30, 30, 200, 255]);
    let seq = write_sequence(
        &project,
        "spin",
        &[(8, 8, a.clone()), (8, 8, a.clone()), (8, 8, a.clone()), (8, 8, b.clone()), (8, 8, a.clone())],
    );
    let opts = FramesOptions { colors: 32, compress: false };
    let report = frames::run(&project, &seq, &opts).unwrap();
    assert_eq!(report.input_frames, 5);
    assert_eq!(report.kept_frames, 3, "AAA→1, B→1, A→1");
    let stays: Vec<u32> = report.records.iter().map(|r| r.stay).collect();
    assert_eq!(stays, vec![3, 1, 1], "停留计数 3,1,1");
}

/// trim：带透明边的帧裁切 + 偏移记录正确。
#[test]
fn trim_records_offset() {
    let project = temp_project("trim");
    // 16x16 透明，内容块在 (5,4) 大小 3x6
    let f = boxed(16, 16, 5, 4, 3, 6, [10, 200, 50, 255]);
    let seq = write_sequence(&project, "blob", &[(16, 16, f)]);
    let opts = FramesOptions { colors: 0, compress: false };
    let report = frames::run(&project, &seq, &opts).unwrap();
    let rec = &report.records[0];
    assert_eq!(rec.trim_offset, (5, 4), "偏移 = 内容左上角");
    assert_eq!(rec.atlas_rect.2, 3, "裁后宽 = 内容宽");
    assert_eq!(rec.atlas_rect.3, 6, "裁后高 = 内容高");
}

/// atlas：每帧 uv 矩形不越界，sidecar 帧表能还原每帧位置。
/// 同尺寸输入帧 + 不同位置/大小的内容块 → trim 后尺寸不同，考验装箱。
#[test]
fn atlas_uv_rects_valid() {
    let project = temp_project("atlas");
    // 都是 16x16 输入，内容块不同 → trim 后 6x4 与 4x8
    let f0 = boxed(16, 16, 1, 1, 6, 4, [255, 0, 0, 255]);
    let f1 = boxed(16, 16, 2, 2, 4, 8, [0, 255, 0, 255]);
    let seq = write_sequence(&project, "two", &[(16, 16, f0), (16, 16, f1)]);
    let opts = FramesOptions { colors: 0, compress: false };
    let report = frames::run(&project, &seq, &opts).unwrap();
    let (aw, ah) = report.atlas_size;
    let sidecar = read_atlas_sidecar(&project, "two");
    let fr = sidecar["frames"].as_array().unwrap();
    assert_eq!(fr.len(), 2);
    for f in fr {
        let r = f["rect"].as_array().unwrap();
        let (x, y, w, h) = (
            r[0].as_u64().unwrap() as u32,
            r[1].as_u64().unwrap() as u32,
            r[2].as_u64().unwrap() as u32,
            r[3].as_u64().unwrap() as u32,
        );
        assert!(x + w <= aw && y + h <= ah, "rect 不越界");
        let uv = f["uv"].as_array().unwrap();
        for v in uv {
            let val = v.as_f64().unwrap();
            assert!((0.0..=1.0).contains(&val), "uv 在 [0,1]");
        }
    }
}

/// 确定性：同输入跑两次，全部产物逐字节一致。
#[test]
fn deterministic_byte_for_byte() {
    let frames_set: Vec<(u32, u32, Vec<u8>)> = vec![
        (12, 12, boxed(12, 12, 2, 2, 5, 5, [180, 60, 200, 255])),
        (12, 12, boxed(12, 12, 2, 2, 5, 5, [180, 60, 200, 255])), // 重复（测去重也确定）
        (12, 12, boxed(12, 12, 4, 3, 6, 4, [60, 200, 90, 255])),
    ];
    let run_once = |tag: &str| -> BTreeMap<String, Vec<u8>> {
        let project = temp_project(tag);
        let seq = write_sequence(&project, "fx", &frames_set);
        frames::run(&project, &seq, &FramesOptions::default()).unwrap();
        // 收集所有产物字节（assets/ 下全部文件 + animations.json）
        let mut out = BTreeMap::new();
        collect_files(&project.join("assets"), &project, &mut out);
        out.insert(
            "animations.json".into(),
            std::fs::read(project.join("animations.json")).unwrap(),
        );
        out
    };
    let a = run_once("det-a");
    let b = run_once("det-b");
    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>(), "产物文件集一致");
    for (k, va) in &a {
        assert_eq!(va, &b[k], "产物 {k} 逐字节一致");
    }
}

fn collect_files(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, root, out);
        } else {
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.insert(rel, std::fs::read(&p).unwrap());
        }
    }
}

/// 配置喂 advance_animations 播放正确：帧序 + 停留（重复帧名）确定推进。
#[test]
fn animations_config_plays_with_stays() {
    let project = temp_project("play");
    let a = solid(8, 8, [200, 30, 30, 255]);
    let b = solid(8, 8, [30, 200, 30, 255]);
    // A A B —— A 停留 2，B 停留 1
    let seq = write_sequence(&project, "anim", &[(8, 8, a.clone()), (8, 8, a.clone()), (8, 8, b)]);
    frames::run(&project, &seq, &FramesOptions { colors: 0, compress: false }).unwrap();

    // 读回生成的 clip，喂 advance_animations
    let anims = read_animations(&project);
    let clip_val = &anims["clips"]["anim"];
    let clip: Clip = serde_json::from_value(clip_val.clone()).unwrap();
    assert_eq!(clip.fps, 60, "fps=60 → 每 tick 一帧");
    // frames 列表：A 重复 2 次（停留 2）+ B 1 次
    let frame_names: Vec<String> =
        clip_val["frames"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(frame_names.len(), 3, "停留展开后总帧 = 2+1");
    assert_eq!(frame_names[0], frame_names[1], "前两帧同名（A 停留）");
    assert_ne!(frame_names[1], frame_names[2], "第三帧是 B");

    // 真喂引擎播放：逐 tick 推进 Sprite.image，序列应等于展开后的帧名
    let clips = BTreeMap::from([("anim".to_string(), clip)]);
    let mut w = World::new();
    let e = w.spawn();
    w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#fff", "image": ""})).unwrap();
    w.set_component(e, "Anim", json!({"clip": "anim", "prev": "", "t": 0, "done": false})).unwrap();
    let mut played = Vec::new();
    for _ in 0..3 {
        advance_animations(&mut w, &clips).unwrap();
        played.push(w.get_field(e, "Sprite.image").unwrap().as_str().unwrap().to_string());
    }
    assert_eq!(played, frame_names, "引擎逐 tick 播放序列 = 配置展开的帧序（含停留）");
}

/// BC7 字节数对比：压缩产物 ≈ 图集 RGBA8 raw 的 1/4（4×），加去重额外省。
#[test]
fn bc7_compression_ratio_4x_plus_dedup() {
    let project = temp_project("bc7");
    // 10 帧，其中 8 帧相同（大量静止段）→ 去重砍到 3 帧，atlas 更小
    let a = solid(16, 16, [120, 80, 200, 255]);
    let b = solid(16, 16, [200, 120, 80, 255]);
    let c = solid(16, 16, [80, 200, 120, 255]);
    let mut set = Vec::new();
    for _ in 0..5 {
        set.push((16, 16, a.clone()));
    }
    set.push((16, 16, b.clone()));
    for _ in 0..3 {
        set.push((16, 16, c.clone()));
    }
    let seq = write_sequence(&project, "char", &set);
    let report = frames::run(&project, &seq, &FramesOptions { colors: 32, compress: true }).unwrap();

    assert_eq!(report.input_frames, 9);
    assert_eq!(report.kept_frames, 3, "去重 9→3（静止段砍掉）");
    let bc7 = report.bc7_bytes.expect("compress=true 应有 BC7 字节");
    // BC7 块数据（去掉 20 字节头）= 图集 RGBA8 raw 的 1/4（atlas 48x16，4 的倍数 → 精确 4×）
    const HEADER: u64 = 20;
    let block_bytes = bc7 - HEADER;
    assert_eq!(report.atlas_raw_bytes, block_bytes * 4, "BC7 块数据 = RGBA8 raw 的 1/4（4×）");
    // 去重额外省：原始 9 帧不去重的 raw 显存（桌宠那种全驻留）vs 去重 3 帧 atlas 的 BC7
    let raw_all_frames = 9u64 * 16 * 16 * 4;
    assert!(
        bc7 * 4 < raw_all_frames,
        "去重+BC7 总省应远超单纯 4×：bc7={bc7} vs 全帧 raw={raw_all_frames}"
    );
}

/// 没有 PNG（视频文件）→ 明确提示先用 ffmpeg 转，不静默失败。
#[test]
fn video_input_errors_with_ffmpeg_hint() {
    let project = temp_project("video");
    let dir = project.join("seq").join("clip");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("anim.mp4"), b"not really a video").unwrap();
    let err = frames::run(&project, &dir, &FramesOptions::default()).unwrap_err();
    assert!(err.contains("ffmpeg"), "应提示用 ffmpeg 转: {err}");
    assert!(err.contains("VD092"), "应带错误码: {err}");
}

/// 尺寸不一致的序列 → 显式报错（不静默）。
#[test]
fn mismatched_frame_size_errors() {
    let project = temp_project("mismatch");
    let seq = write_sequence(
        &project,
        "bad",
        &[(8, 8, solid(8, 8, [1, 2, 3, 255])), (10, 8, solid(10, 8, [1, 2, 3, 255]))],
    );
    let err = frames::run(&project, &seq, &FramesOptions::default()).unwrap_err();
    assert!(err.contains("VD094") && err.contains("尺寸"), "应报尺寸不一致: {err}");
}
