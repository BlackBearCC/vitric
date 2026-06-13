//! 动画 demo（阶段 1 验收）：程序生成一组「滑动 + 静止段」占位帧（不需真 AI 出图），
//! 跑 vitric assets --frames 出图集 + 动画配置 + BC7 压缩，再启动引擎自检播放正确。
//!
//! **测试纯净**：全程在临时目录里建一份工作项目（从 examples/frame-anim 只读拷项目
//! 模板，产物在临时目录里生成），**绝不写源码树**——否则它和别的「读 examples」测试
//! 并行时会撞上半写产物（曾导致首跑 flaky 失败）。examples/frame-anim 是提交进 git 的
//! 稳定 fixture（预生成产物），由 `vitric assets --frames` 生成一次，不靠测试运行时重建。
//!
//! 验收对照（design-frame-animation.md 第五节阶段 1）：
//! - 一组序列图 → --frames 出图集 + 配置 ✓（本测试在临时目录生成）
//! - 引擎播放正确（帧序、停留、trim 摆位）✓（advance_animations 自检）
//! - 显存实测对比（RGBA8 vs BC7 倍数）✓（report 的 compression_ratio）
//! - check 校验产物 ✓（这里在临时项目跑一次 check 确认绿灯）

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use vitric_cli::frames::{self, FramesOptions};
use vitric_cli::runtime::{self, advance_animations};
use vitric_data::Clip;
use vitric_ecs::World;

/// examples/frame-anim 只读模板源（项目文件，不含产物）。
fn template_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/frame-anim")
}

/// 把 demo 项目的**项目文件**（不含 assets/ 和 animations.json 产物）拷进临时工作目录。
/// 产物由测试在工作目录里跑 --frames 生成——工作目录是临时的，源码树不被触碰。
fn setup_work_project(work: &Path) {
    let tpl = template_dir();
    for rel in ["vitric.json", "schema.json", "scenes/main.json", "rules/game.json"] {
        let dst = work.join(rel);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(tpl.join(rel), &dst).unwrap();
    }
    // --frames 把产物写进项目 assets/，目录要先在（原 examples 版隐含存在）
    std::fs::create_dir_all(work.join("assets")).unwrap();
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// 程序生成 10 帧：一个 8x8 方块从左滑到右，中间 3 帧（4,5,6）位置+颜色不变（静止段，
/// 测去重）；方块画在透明背景上（测 trim）。同尺寸 32x32 输入。
fn generate_source_frames(dir: &Path) {
    let (w, h) = (32u32, 32u32);
    for i in 0..10u32 {
        let mut px = vec![0u8; (w * h * 4) as usize]; // 透明背景
        let step = if i <= 4 {
            i
        } else if i <= 6 {
            4 // 静止段：位置不动
        } else {
            i - 2
        };
        let bx = 2 + step * 2;
        let by = 12u32;
        let cstep = if (4..=6).contains(&i) { 4 } else { i.min(9) };
        let r = (40 + cstep * 20) as u8;
        for yy in by..by + 8 {
            for xx in bx..bx + 8 {
                if xx < w && yy < h {
                    let o = ((yy * w + xx) * 4) as usize;
                    px[o..o + 4].copy_from_slice(&[r, 80, 200, 255]);
                }
            }
        }
        write_png(&dir.join(format!("frame{i:02}.png")), w, h, &px);
    }
}

/// 生成 demo 产物（临时目录）+ 自检引擎播放 + check 绿灯。
#[test]
fn frame_anim_demo_pipeline_and_playback() {
    // 工作项目 + 源帧都在临时目录（进程 id 隔离并行实例），不碰源码树
    let work = std::env::temp_dir().join(format!("vitric-frameanim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let project = work.join("project");
    setup_work_project(&project);

    // 1. 程序生成源序列帧到临时目录（源帧不进项目，只进产物）
    let seq = work.join("src/slide"); // 目录名 = 片段名
    std::fs::create_dir_all(&seq).unwrap();
    generate_source_frames(&seq);

    // 2. 跑 --frames：出图集 + 配置 + BC7
    let report =
        frames::run(&project, &seq, &FramesOptions { colors: 16, compress: true }).unwrap();
    assert_eq!(report.input_frames, 10);
    assert_eq!(report.kept_frames, 8, "中间 3 帧静止 → 去重砍到 8 帧");
    let ratio = report.atlas_raw_bytes as f64 / (report.bc7_bytes.unwrap() - 20) as f64;
    assert!((ratio - 4.0).abs() < 0.01, "BC7 块数据是 RGBA8 的 1/4（4×），实测 {ratio}");

    // trim 生效：每帧裁后宽高都是内容外接框（≤ 输入 32x32）
    for r in &report.records {
        assert!(r.atlas_rect.2 <= 32 && r.atlas_rect.3 <= 32, "trim 后不超原尺寸");
        assert!(r.atlas_rect.2 > 0 && r.atlas_rect.3 > 0, "内容帧裁后非空");
    }

    // 3. 引擎自检：用项目装配的运行时播放 slide，逐 tick 推进帧
    let (mut sim, mut rt) = runtime::Runtime::boot(&project).expect("demo 项目应能启动");
    let actor = sim.world.entity("actor").expect("场景里有 actor");
    let mut frames_seen = Vec::new();
    for _ in 0..report.records.iter().map(|r| r.stay).sum::<u32>() {
        advance_animations(&mut sim.world, &rt.animations).unwrap();
        frames_seen
            .push(sim.world.get_field(actor, "Sprite.image").unwrap().as_str().unwrap().to_string());
        let _ = &mut rt;
    }
    // 播放序列 = 配置里展开的帧序（停留 = 重复帧名），逐帧确定
    let anims: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(project.join("animations.json")).unwrap(),
    )
    .unwrap();
    let expanded: Vec<String> = anims["clips"]["slide"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(frames_seen, expanded, "引擎逐 tick 播放 = 配置展开帧序（帧序+停留对）");

    // 静止段在展开序列里体现为连续相同帧名（停留摆位）
    let has_run = expanded.windows(2).any(|w| w[0] == w[1]);
    assert!(has_run, "静止段应展开成连续相同帧（停留）");

    // 4. check 校验产物绿灯（图集存在、帧表合法、uv 不越界、帧图引用一致）
    runtime::check(&project).expect("demo 产物 check 该过");

    // 5. 确定性：再跑一次 --frames，animations.json 逐字节相同
    let before = std::fs::read(project.join("animations.json")).unwrap();
    frames::run(&project, &seq, &FramesOptions { colors: 16, compress: true }).unwrap();
    let after = std::fs::read(project.join("animations.json")).unwrap();
    assert_eq!(before, after, "同输入 --frames 产物逐字节一致");

    // 引擎也能用生成的 clip 跑（直接喂 advance_animations，独立于场景）
    let clip: Clip = serde_json::from_value(anims["clips"]["slide"].clone()).unwrap();
    let clips = BTreeMap::from([("slide".to_string(), clip)]);
    let mut w = World::new();
    let e = w.spawn();
    w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#fff", "image": ""})).unwrap();
    w.set_component(e, "Anim", json!({"clip": "slide", "prev": "", "t": 0, "done": false})).unwrap();
    advance_animations(&mut w, &clips).unwrap();
    assert_eq!(
        w.get_field(e, "Sprite.image").unwrap().as_str().unwrap(),
        expanded[0],
        "首 tick 播第一帧"
    );

    let _ = std::fs::remove_dir_all(&work);
}
