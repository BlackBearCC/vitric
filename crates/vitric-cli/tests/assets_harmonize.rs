//! vitric assets (asset harmonization) end-to-end lock:
//! shared palette cap, determinism, transparency preservation, scaling, backup refusal,
//! --palette-lock joining.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use vitric_cli::assets_cmd::{harmonize, Options};

/// One independent temporary project directory per test, to avoid stepping on each other.
fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-assets-{name}-{}", std::process::id()));
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

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).unwrap();
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba, "输出必须是 RGBA8");
    (info.width, info.height, buf[..info.buffer_size()].to_vec())
}

/// Deduplicated RGB set of all opaque (alpha>0) pixels in the image.
fn opaque_colors(rgba: &[u8]) -> BTreeSet<[u8; 3]> {
    rgba.chunks_exact(4).filter(|px| px[3] > 0).map(|px| [px[0], px[1], px[2]]).collect()
}

fn read_palette_json(project: &Path) -> BTreeSet<[u8; 3]> {
    let text = std::fs::read_to_string(project.join("palette.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["colors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let h = s.as_str().unwrap().strip_prefix('#').unwrap();
            let n = u32::from_str_radix(h, 16).unwrap();
            [(n >> 16) as u8, (n >> 8) as u8, n as u8]
        })
        .collect()
}

/// 8x8 gradient image: 64 distinct colors (offset by seed; three images total 192 distinct
/// colors).
fn scattered_png(path: &Path, seed: u8) {
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for y in 0..8u32 {
        for x in 0..8u32 {
            rgba.extend_from_slice(&[
                (x * 32) as u8,
                (y * 32) as u8,
                seed.wrapping_add((x + y * 8) as u8),
                255,
            ]);
        }
    }
    write_png(path, 8, 8, &rgba);
}

#[test]
fn shared_palette_caps_union_across_images() {
    let dir = temp_project("union");
    scattered_png(&dir.join("assets/a.png"), 0);
    scattered_png(&dir.join("assets/b.png"), 90);
    scattered_png(&dir.join("assets/sub/c.png"), 180);

    let report =
        harmonize(&dir, &Options { colors: 16, ..Options::default() }).unwrap();
    assert_eq!(report.images, 3);
    assert!(report.palette.len() <= 16);

    // All images share the same palette: the union of opaque colors across the three images is
    // ≤ N, and each is a subset of palette.json
    let palette = read_palette_json(&dir);
    assert_eq!(
        palette,
        report.palette.iter().copied().collect::<BTreeSet<_>>(),
        "palette.json 必须和报告一致"
    );
    let mut union: BTreeSet<[u8; 3]> = BTreeSet::new();
    for rel in ["a.png", "b.png", "sub/c.png"] {
        let (_, _, rgba) = read_png(&dir.join("assets").join(rel));
        let colors = opaque_colors(&rgba);
        assert!(colors.is_subset(&palette), "{rel} 出现了色板外的颜色");
        union.extend(colors);
    }
    assert!(union.len() <= 16, "三张图颜色并集 {} 超过 16", union.len());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn deterministic_same_input_same_bytes() {
    let dir = temp_project("determinism");
    scattered_png(&dir.join("assets/a.png"), 7);
    scattered_png(&dir.join("assets/b.png"), 133);

    let opts = Options { colors: 8, ..Options::default() };
    harmonize(&dir, &opts).unwrap();
    let first: Vec<(String, Vec<u8>)> = ["assets/a.png", "assets/b.png", "palette.json"]
        .iter()
        .map(|rel| (rel.to_string(), std::fs::read(dir.join(rel)).unwrap()))
        .collect();

    // Restore originals from the backup, clear the backup dir and palette, then run again
    for rel in ["a.png", "b.png"] {
        std::fs::copy(dir.join("assets_original").join(rel), dir.join("assets").join(rel))
            .unwrap();
    }
    std::fs::remove_dir_all(dir.join("assets_original")).unwrap();
    std::fs::remove_file(dir.join("palette.json")).unwrap();

    harmonize(&dir, &opts).unwrap();
    for (rel, bytes) in &first {
        assert_eq!(
            &std::fs::read(dir.join(rel)).unwrap(),
            bytes,
            "{rel} 两次运行字节不一致——确定性被破坏"
        );
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn transparency_preserved() {
    let dir = temp_project("alpha");
    // 2x2: fully transparent (RGB is garbage) / semi-transparent / two opaque
    #[rustfmt::skip]
    let rgba = [
        123, 45, 67, 0,    // alpha=0, RGB does not matter
        200, 10, 10, 128,  // semi-transparent red
        10, 200, 10, 255,
        10, 10, 200, 255,
    ];
    write_png(&dir.join("assets/t.png"), 2, 2, &rgba);

    harmonize(&dir, &Options::default()).unwrap();
    let (_, _, out) = read_png(&dir.join("assets/t.png"));
    let palette = read_palette_json(&dir);

    assert_eq!(out[3], 0, "alpha=0 必须保持全透明");
    assert_eq!(out[7], 128, "半透明像素的 alpha 必须原样保留");
    assert!(palette.contains(&[out[4], out[5], out[6]]), "半透明像素的 RGB 也要量化进色板");
    for i in [2, 3] {
        assert_eq!(out[i * 4 + 3], 255);
        assert!(palette.contains(&[out[i * 4], out[i * 4 + 1], out[i * 4 + 2]]));
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn height_downscales_and_keeps_aspect() {
    let dir = temp_project("height");
    // 4x16 tall image (solid color, easy to verify content after scaling) + an already-short image
    // (must not be touched)
    write_png(&dir.join("assets/tall.png"), 4, 16, &[40, 80, 120, 255].repeat(4 * 16));
    write_png(&dir.join("assets/short.png"), 3, 2, &[200, 100, 50, 255].repeat(6));

    let report =
        harmonize(&dir, &Options { height: Some(8), ..Options::default() }).unwrap();
    assert_eq!(report.downscaled, 1, "只有高于 8 的那张被缩");

    let (w, h, _) = read_png(&dir.join("assets/tall.png"));
    assert_eq!((w, h), (2, 8), "16→8 高度减半，宽度 4→2 同比例");
    let (w, h, _) = read_png(&dir.join("assets/short.png"));
    assert_eq!((w, h), (3, 2), "不高于 8 的图尺寸不动");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn refuses_to_overwrite_existing_backup() {
    let dir = temp_project("backup");
    write_png(&dir.join("assets/a.png"), 1, 1, &[1, 2, 3, 255]);

    harmonize(&dir, &Options::default()).unwrap();
    assert!(dir.join("assets_original/a.png").exists(), "原件必须备份到 assets_original/");

    let err = harmonize(&dir, &Options::default()).unwrap_err();
    assert!(err.contains("assets_original"), "拒绝信息要点名备份目录: {err}");
    assert!(err.contains("不静默覆盖"), "拒绝信息要讲明原因: {err}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn palette_lock_quantizes_to_saved_palette() {
    let dir = temp_project("lock");
    std::fs::write(
        dir.join("palette.json"),
        r##"{"colors": ["#000000", "#ff0000"]}"##,
    )
    .unwrap();
    // New assets: two blackish and two reddish colors; after locking only the two colors from the
    // palette may appear
    #[rustfmt::skip]
    let rgba = [
        10, 0, 0, 255,
        30, 20, 20, 255,
        200, 30, 30, 255,
        255, 60, 0, 255,
    ];
    write_png(&dir.join("assets/new.png"), 2, 2, &rgba);

    let report =
        harmonize(&dir, &Options { palette_lock: true, ..Options::default() }).unwrap();
    assert_eq!(report.palette.len(), 2, "锁定模式不提取，色板就是 palette.json 那两色");

    let (_, _, out) = read_png(&dir.join("assets/new.png"));
    let allowed: BTreeSet<[u8; 3]> = [[0, 0, 0], [255, 0, 0]].into_iter().collect();
    assert!(opaque_colors(&out).is_subset(&allowed), "锁定模式下不许出现新颜色");
    // palette.json is an input, not an output: in lock mode it is not rewritten
    assert_eq!(
        std::fs::read_to_string(dir.join("palette.json")).unwrap(),
        r##"{"colors": ["#000000", "#ff0000"]}"##,
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn palette_lock_without_palette_json_is_explicit_error() {
    let dir = temp_project("lock-missing");
    write_png(&dir.join("assets/a.png"), 1, 1, &[1, 2, 3, 255]);
    let err = harmonize(&dir, &Options { palette_lock: true, ..Options::default() }).unwrap_err();
    assert!(err.contains("palette.json"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn empty_assets_dir_is_explicit_error() {
    let dir = temp_project("empty");
    let err = harmonize(&dir, &Options::default()).unwrap_err();
    assert!(err.contains("没有 PNG"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn normal_maps_are_excluded_from_harmonize() {
    // Normal maps (_n pairing) encode vectors, not colors: they must be entirely excluded from
    // quantization, otherwise every re-run of harmonization would trash the normal data
    let dir = temp_project("normals-guard");
    scattered_png(&dir.join("assets/hero.png"), 0);
    // The normal color is deliberately chosen to not be in hero.png (if quantization touched it,
    // it would be obvious at a glance)
    write_png(&dir.join("assets/hero_n.png"), 2, 2, &[3, 250, 99, 255].repeat(4));
    let n_before = std::fs::read(dir.join("assets/hero_n.png")).unwrap();

    let report = harmonize(&dir, &Options { colors: 4, ..Options::default() }).unwrap();
    assert_eq!(report.images, 1, "_n 不计入处理的图片数");
    // Not a byte changed, not backed up, the normal color does not enter the palette
    assert_eq!(std::fs::read(dir.join("assets/hero_n.png")).unwrap(), n_before);
    assert!(!dir.join("assets_original/hero_n.png").exists(), "_n 不参与 = 不备份");
    assert!(!read_palette_json(&dir).contains(&[3, 250, 99]), "法线色不许进色板");

    // A project with only _n files: explicit error (_n does not count as a processable PNG)
    let dir2 = temp_project("normals-only");
    write_png(&dir2.join("assets/x_n.png"), 1, 1, &[128, 128, 255, 255]);
    let err = harmonize(&dir2, &Options::default()).unwrap_err();
    assert!(err.contains("没有 PNG"), "{err}");

    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&dir2).unwrap();
}
