//! vitric assets — harmonize all PNGs in a project onto a single shared palette (asset harmonization).
//!
//! Problem solved: AI-generated images each have different tones/styles; spliced into one
//! scene they look incoherent.
//! Approach: extract one shared palette from all project pixels together (median-cut
//! quantization), then snap each image's opaque pixels to the nearest palette color —
//! one palette per project = visual harmony.
//!
//! Constraints (all deliberate):
//! - Determinism: traversal order sorted, frequency table in BTreeMap, no randomness in
//!   splitting — same input always produces the same palette and bytes.
//! - Safety: back up originals to assets_original/ before touching anything; if a backup
//!   already exists, refuse to run — never silently overwrite the previous originals.
//! - Transparency: alpha=0 pixels stay fully transparent (RGB zeroed); semi-transparent
//!   pixels keep alpha, only RGB is quantized.
//! - palette.json is the project's official palette: --palette-lock skips extraction and
//!   uses it directly, so newly added assets automatically join.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Decoded image (RGBA8), matching vitric-render's convention.
/// pub(crate): normal generation (normals.rs) and frame import (frames.rs) reuse the same
/// decode/encode — don't duplicate.
#[derive(Clone)]
pub(crate) struct Img {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

pub struct Options {
    /// Palette color count cap (target box count for median cut).
    pub colors: usize,
    /// If set, images taller than H are first nearest-neighbor downscaled to height H
    /// (aspect-preserving) — pixel-art path; scaling happens before palette extraction.
    pub height: Option<u32>,
    /// Skip extraction and directly use the project's existing palette.json — new assets
    /// join the old palette.
    pub palette_lock: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { colors: 32, height: None, palette_lock: false }
    }
}

#[derive(Debug)]
pub struct Report {
    pub images: usize,
    pub palette: Vec<[u8; 3]>,
    pub downscaled: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl Report {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "images": self.images,
            "palette_size": self.palette.len(),
            "palette": self.palette.iter().map(|c| hex(*c)).collect::<Vec<_>>(),
            "downscaled": self.downscaled,
            "bytes_before": self.bytes_before,
            "bytes_after": self.bytes_after,
        })
    }
}

/// CLI entry: `vitric assets <project_dir> [--colors N] [--height H] [--palette-lock]
/// | --normals [--normals-ai]`. Palette harmonization and normal generation are two
/// separate modes, never mixed (one thing at a time, so the report is unambiguous).
pub fn run(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("assets 缺少项目目录参数")?;
    let mut opts = Options::default();
    let mut normals = false;
    let mut normals_ai = false;
    let mut palette_opts_given = false;
    // Frame import mode: --frames <sequence_dir>, mutually exclusive with palette/normals
    let mut frames_dir: Option<String> = None;
    let mut frames_compress = true;
    let mut frames_colors: Option<usize> = None;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--frames" => {
                frames_dir = Some(args.get(i + 1).ok_or(need("--frames"))?.clone());
                i += 2;
            }
            "--no-compress" => {
                frames_compress = false;
                i += 1;
            }
            "--colors" => {
                opts.colors = args
                    .get(i + 1)
                    .ok_or(need("--colors"))?
                    .parse()
                    .map_err(|e| format!("--colors: {e}"))?;
                frames_colors = Some(opts.colors);
                palette_opts_given = true;
                i += 2;
            }
            "--height" => {
                opts.height = Some(
                    args.get(i + 1)
                        .ok_or(need("--height"))?
                        .parse()
                        .map_err(|e| format!("--height: {e}"))?,
                );
                palette_opts_given = true;
                i += 2;
            }
            "--palette-lock" => {
                opts.palette_lock = true;
                palette_opts_given = true;
                i += 1;
            }
            "--normals" => {
                normals = true;
                i += 1;
            }
            // --normals-ai implies --normals (it just swaps the generator)
            "--normals-ai" => {
                normals = true;
                normals_ai = true;
                i += 1;
            }
            other => {
                return Err(format!(
                    "未知选项 {other:?}。可用: --frames --no-compress --colors --height --palette-lock --normals --normals-ai"
                ))
            }
        }
    }
    // Frame import mode: mutually exclusive with palette/normals (one thing at a time, so
    // the report is unambiguous — same style as the existing mutual exclusion).
    // --frames itself accepts --colors (whole-group palette count) and --no-compress, so
    // those two don't count as conflicts.
    if let Some(fdir) = frames_dir {
        if normals {
            return Err("--frames 与 --normals 不能混用（一次只做一件事）。\
                        提示：先 --frames 出动画素材，再单独跑 --normals 生成法线".into());
        }
        if opts.height.is_some() || opts.palette_lock {
            return Err("--frames 与 --height/--palette-lock 不能混用。\
                        提示：trim 在引擎内做，色板由 --colors 控制（整组一套）".into());
        }
        let fopts = crate::frames::FramesOptions {
            colors: frames_colors.unwrap_or(32),
            compress: frames_compress,
        };
        let report = crate::frames::run(&PathBuf::from(dir), &PathBuf::from(fdir), &fopts)?;
        println!("{}", serde_json::to_string_pretty(&report.to_json()).expect("报告可序列化"));
        return Ok(());
    }
    if !frames_compress {
        return Err("--no-compress 只在 --frames 模式下有意义（它控制图集是否压 BC7）".into());
    }
    if normals && palette_opts_given {
        return Err("--normals 和色板选项（--colors/--height/--palette-lock）不能混用。\
                    提示：先 --normals 生成法线，再单独跑一次和谐化（_n 文件会被自动跳过）"
            .into());
    }
    if normals {
        let ai = if normals_ai { Some(crate::normals::AiConfig::from_env()?) } else { None };
        let report = crate::normals::generate(&PathBuf::from(dir), ai.as_ref())?;
        println!("{}", serde_json::to_string_pretty(&report.to_json()).expect("报告可序列化"));
        return Ok(());
    }
    let report = harmonize(&PathBuf::from(dir), &opts)?;
    println!("{}", serde_json::to_string_pretty(&report.to_json()).expect("报告可序列化"));
    Ok(())
}

/// Main flow: collect → (optional scale) → extract/read palette → back up originals →
/// quantize and write back → persist palette.json.
pub fn harmonize(project_dir: &Path, opts: &Options) -> Result<Report, String> {
    if opts.colors < 2 || opts.colors > 256 {
        return Err(format!(
            "--colors 要在 2..=256 之间，拿到 {}。提示：像素风常用 16/32/64",
            opts.colors
        ));
    }
    if opts.height == Some(0) {
        return Err("--height 不能是 0。提示：给目标像素高度，比如 64".into());
    }
    let assets_dir = project_dir.join("assets");
    if !assets_dir.is_dir() {
        return Err(format!(
            "{} 不存在。提示：vitric assets 处理项目的 assets/ 目录，先把图放进去",
            assets_dir.display()
        ));
    }
    // Safety gate: if a backup exists, refuse. Overwriting the backup = permanently losing
    // the true originals — this must never happen silently.
    let backup_dir = project_dir.join("assets_original");
    if backup_dir.exists() {
        return Err(format!(
            "{} 已有 assets_original 备份，确认后删掉它再跑——不静默覆盖上次的原件",
            project_dir.display()
        ));
    }

    let mut rels = collect_pngs(&assets_dir)?;
    // Normal maps (_n pairs, convention in vitric_render::is_normal_map_name) are entirely
    // excluded from harmonization: not in palette extraction, not quantized, not scaled, not
    // backed up — RGB encodes vectors not colors, snapping to the palette would destroy the
    // normal data, and every rerun would destroy it again.
    rels.retain(|r| !vitric_render::is_normal_map_name(r));
    if rels.is_empty() {
        return Err(format!(
            "{} 里没有 PNG（法线贴图 _n 文件不算，它们不参与和谐化）。\
             提示：先把 AI 出的图（PNG）放进 assets/ 再跑",
            assets_dir.display()
        ));
    }

    // Decode + (optional) scale. Scaling must happen before palette extraction: the scaled
    // pixels are the ones that ultimately participate in the palette.
    let mut images: Vec<(String, Img)> = Vec::new();
    let mut bytes_before = 0u64;
    let mut downscaled = 0usize;
    for rel in &rels {
        let path = assets_dir.join(rel);
        bytes_before += std::fs::metadata(&path)
            .map_err(|e| format!("素材 {rel}: 读元数据失败: {e}"))?
            .len();
        let mut img = load_png(&path).map_err(|e| format!("素材 {rel}: {e}"))?;
        if let Some(h) = opts.height {
            if img.height > h {
                img = downscale_to_height(&img, h);
                downscaled += 1;
            }
        }
        images.push((rel.clone(), img));
    }

    // Palette: lock mode uses the project's existing palette.json; otherwise extract from
    // all opaque pixels in the project (weighted by frequency).
    let palette_path = project_dir.join("palette.json");
    let palette: Vec<[u8; 3]> = if opts.palette_lock {
        read_palette(&palette_path)?
    } else {
        extract_palette(images.iter().map(|(_, img)| img), opts.colors)?
    };

    // Back up before touching anything: if the backup isn't fully written, error out —
    // assets/ hasn't been modified a single byte.
    for rel in &rels {
        let src = assets_dir.join(rel);
        let dst = backup_dir.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建备份目录 {} 失败: {e}", parent.display()))?;
        }
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("备份 {rel} 到 assets_original/ 失败: {e}"))?;
    }

    // Quantize and write back: opaque pixels snap to the nearest palette color; alpha is
    // preserved as-is.
    let mut bytes_after = 0u64;
    for (rel, img) in &images {
        let q = quantize_with(img, &palette);
        let path = assets_dir.join(rel);
        save_png(&path, &q).map_err(|e| format!("素材 {rel}: {e}"))?;
        bytes_after += std::fs::metadata(&path)
            .map_err(|e| format!("素材 {rel}: 读元数据失败: {e}"))?
            .len();
    }

    // palette.json = the project's official palette. Lock mode doesn't rewrite it (it's the input).
    if !opts.palette_lock {
        let value = serde_json::json!({
            "colors": palette.iter().map(|c| hex(*c)).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string_pretty(&value).expect("色板可序列化");
        std::fs::write(&palette_path, text + "\n")
            .map_err(|e| format!("写 {} 失败: {e}", palette_path.display()))?;
    }

    Ok(Report { images: images.len(), palette, downscaled, bytes_before, bytes_after })
}

/// Recursively collect relative paths (forward slashes) of all PNGs under assets/, sorted
/// for determinism.
pub(crate) fn collect_pngs(dir: &Path) -> Result<Vec<String>, String> {
    let mut rels = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries =
            std::fs::read_dir(&d).map_err(|e| format!("读素材目录 {} 失败: {e}", d.display()))?;
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
                let rel = path
                    .strip_prefix(dir)
                    .expect("walk 始于 dir")
                    .to_string_lossy()
                    .replace('\\', "/");
                rels.push(rel);
            }
        }
    }
    rels.sort();
    Ok(rels)
}

/// Median-cut quantization: split (color → frequency) into at most n boxes, taking the
/// weighted average color per box.
///
/// Determinism constraints: input is a BTreeMap (ordered iteration); box selection picks
/// the most pixels, breaking ties by smaller index; split channel ties break toward
/// R→G→B; in-box sorting carries the full tuple as a tiebreaker — no HashMap, no randomness
/// anywhere.
fn median_cut(freq: &BTreeMap<[u8; 3], u64>, n: usize) -> Vec<[u8; 3]> {
    let entries: Vec<([u8; 3], u64)> = freq.iter().map(|(c, &w)| (*c, w)).collect();
    if entries.len() <= n {
        // Few colors already: use originals as the palette, don't move a single pixel
        return entries.into_iter().map(|(c, _)| c).collect();
    }
    let mut boxes: Vec<Vec<([u8; 3], u64)>> = vec![entries];
    while boxes.len() < n {
        // Pick the box that can still be split (≥2 colors) and has the most pixels; ties
        // break by smaller index
        let mut pick: Option<(usize, u64)> = None;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            let weight: u64 = b.iter().map(|e| e.1).sum();
            if pick.is_none_or(|(_, pw)| weight > pw) {
                pick = Some((i, weight));
            }
        }
        let Some((idx, total)) = pick else { break };
        let b = &mut boxes[idx];
        // Sort along the widest-span channel, split at the weighted median
        let ch = widest_channel(b);
        b.sort_by_key(|e| (e.0[ch], e.0));
        let mut acc = 0u64;
        let mut split = 1usize;
        for (i, e) in b.iter().enumerate() {
            acc += e.1;
            if acc * 2 >= total {
                split = (i + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let hi = b.split_off(split);
        boxes.push(hi);
    }
    // Take the weighted average color per box; BTreeSet dedups + sorts, so palette order
    // and contents are both deterministic
    let mut out: BTreeSet<[u8; 3]> = BTreeSet::new();
    for b in &boxes {
        out.insert(box_average(b));
    }
    out.into_iter().collect()
}

/// Channel with the widest span in the box (0=R 1=G 2=B); ties break toward the earlier one.
fn widest_channel(entries: &[([u8; 3], u64)]) -> usize {
    let mut min = [u8::MAX; 3];
    let mut max = [u8::MIN; 3];
    for (c, _) in entries {
        for ch in 0..3 {
            min[ch] = min[ch].min(c[ch]);
            max[ch] = max[ch].max(c[ch]);
        }
    }
    let mut best = 0;
    for ch in 1..3 {
        if max[ch] - min[ch] > max[best] - min[best] {
            best = ch;
        }
    }
    best
}

/// Weighted average color of a box (rounded).
fn box_average(entries: &[([u8; 3], u64)]) -> [u8; 3] {
    let total: u64 = entries.iter().map(|e| e.1).sum();
    let mut out = [0u8; 3];
    for (ch, slot) in out.iter_mut().enumerate() {
        let sum: u64 = entries.iter().map(|(c, w)| c[ch] as u64 * w).sum();
        *slot = ((sum + total / 2) / total) as u8;
    }
    out
}

/// Extract a shared palette from opaque pixels across a set of images (frequency-weighted
/// median cut). Shared by harmonization and frame import (frames.rs) — one palette per
/// group = visual harmony + paves the way for compression.
/// Fully transparent (nothing to extract) is an explicit error — never silently returns an
/// empty palette.
pub(crate) fn extract_palette<'a>(
    imgs: impl IntoIterator<Item = &'a Img>,
    colors: usize,
) -> Result<Vec<[u8; 3]>, String> {
    let mut freq: BTreeMap<[u8; 3], u64> = BTreeMap::new();
    for img in imgs {
        for px in img.rgba.chunks_exact(4) {
            if px[3] > 0 {
                *freq.entry([px[0], px[1], px[2]]).or_insert(0) += 1;
            }
        }
    }
    if freq.is_empty() {
        return Err(
            "所有图片都是全透明，提取不出色板。提示：检查抠图这步是不是把内容抠没了".into(),
        );
    }
    Ok(median_cut(&freq, colors))
}

/// Snap each opaque pixel to the nearest palette color (RGB Euclidean distance; squared
/// comparison is enough).
/// alpha=0 → entire pixel zeroed (RGB is meaningless; zeroing helps compression);
/// 0<alpha<255 → keep alpha, only swap RGB.
pub(crate) fn quantize_with(img: &Img, palette: &[[u8; 3]]) -> Img {
    let mut rgba = Vec::with_capacity(img.rgba.len());
    for px in img.rgba.chunks_exact(4) {
        if px[3] == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let c = nearest(palette, [px[0], px[1], px[2]]);
        rgba.extend_from_slice(&[c[0], c[1], c[2], px[3]]);
    }
    Img { width: img.width, height: img.height, rgba }
}

/// Nearest color: update only on strictly-smaller distance → on distance ties, take the
/// earlier one in the palette (palette is ordered, result is deterministic).
fn nearest(palette: &[[u8; 3]], c: [u8; 3]) -> [u8; 3] {
    let mut best = palette[0];
    let mut best_d = u32::MAX;
    for p in palette {
        let d: u32 = (0..3)
            .map(|ch| {
                let diff = p[ch] as i32 - c[ch] as i32;
                (diff * diff) as u32
            })
            .sum();
        if d < best_d {
            best_d = d;
            best = *p;
        }
    }
    best
}

/// Nearest-neighbor downscale to height h (aspect-preserving; width rounded, at least 1).
/// Pixel-art scaling doesn't interpolate, edges stay hard.
fn downscale_to_height(img: &Img, h: u32) -> Img {
    let w = ((img.width as u64 * h as u64 + img.height as u64 / 2) / img.height as u64).max(1) as u32;
    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for y in 0..h {
        let sy = y as u64 * img.height as u64 / h as u64;
        for x in 0..w {
            let sx = x as u64 * img.width as u64 / w as u64;
            let i = ((sy * img.width as u64 + sx) * 4) as usize;
            rgba.extend_from_slice(&img.rgba[i..i + 4]);
        }
    }
    Img { width: w, height: h, rgba }
}

/// Read the project's official palette palette.json (format `{"colors": ["#rrggbb", ...]}`).
fn read_palette(path: &Path) -> Result<Vec<[u8; 3]>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "读 {} 失败: {e}。提示：--palette-lock 需要项目已有 palette.json，先不带锁跑一次 vitric assets 生成它",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} 解析失败: {e}", path.display()))?;
    let arr = value
        .get("colors")
        .and_then(|c| c.as_array())
        .ok_or(format!("{} 缺 colors 数组。提示：格式是 {{\"colors\": [\"#rrggbb\", ...]}}", path.display()))?;
    let mut out = Vec::new();
    for item in arr {
        let s = item
            .as_str()
            .ok_or(format!("{} 的 colors 里有非字符串项", path.display()))?;
        out.push(parse_hex(s).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    if out.is_empty() {
        return Err(format!("{} 的 colors 是空的。提示：删掉它重新跑一次 vitric assets", path.display()));
    }
    Ok(out)
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn parse_hex(s: &str) -> Result<[u8; 3], String> {
    let h = s
        .strip_prefix('#')
        .ok_or(format!("颜色 {s:?} 不是 #rrggbb 格式"))?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 {s:?} 不是 #rrggbb 格式"));
    }
    let v = u32::from_str_radix(h, 16).expect("已校验过是 6 位十六进制");
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Decode a PNG into RGBA8. Pattern mirrors vitric-render/src/assets.rs (errors carry the
/// same fix hints), but no 2048 cap — this tool is meant to shrink oversized images
/// (--height).
pub(crate) fn load_png(path: &Path) -> Result<Img, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().map_err(|e| format!("PNG 解码失败: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG 尺寸异常")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG 解码失败: {e}"))?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let src = &buf[..info.buffer_size()];
            let mut out = Vec::with_capacity(src.len() / 3 * 4);
            for px in src.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        other => {
            return Err(format!(
                "颜色类型 {other:?} 不支持。提示：用 RGBA 或 RGB 的 PNG（带不带透明都行）"
            ))
        }
    };
    Ok(Img { width: info.width, height: info.height, rgba })
}

/// Encode an RGBA8 PNG. Fixed encoding parameters → same pixels always produce the same
/// bytes (deterministic tests rely on this).
pub(crate) fn save_png(path: &Path, img: &Img) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("写入失败: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| format!("PNG 编码失败: {e}"))?;
    writer.write_image_data(&img.rgba).map_err(|e| format!("PNG 编码失败: {e}"))?;
    Ok(())
}
