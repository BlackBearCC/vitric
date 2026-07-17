//! vitric assets --frames — turn a batch of AI-generated sequence images into optimized animation assets in one click.
//!
//! Reality path: AI generates video/sequence → cut frames (skeletal binding is what AI is worst at), so doing
//! "frame import + minimal memory" right suits individual developers better than chasing skeletal animation. Lesson learned: a
//! desktop pet with 2400 uncompressed RGBA8 frames fully resident → 8.5G VRAM.
//!
//! Pipeline (fully deterministic, same input → byte-identical output):
//!   1. Adjacent frame dedup: keep only one frame among pixel-wise nearly-identical adjacent frames, recording "stay for N frames" (AI animation often has static segments).
//!   2. Trim transparent edges: crop transparent edges per frame, record offset (playback repositions by offset, visual unchanged).
//!   3. Pack atlas: stitch all frames into one large image (fewer GPU texture switches), record per-frame uv rectangle.
//!   4. Unified palette: reuse median_cut, one palette for the whole set (assets_cmd::quantize_with).
//!   5. Write animation config:
//!      - `animations.json` standard clip (deduped frame names, stay = repeated frame names) — fed to advance_animations
//!        for deterministic playback, render core unchanged (still Sprite.image = full frame name).
//!      - `<clip>-atlas.png` + `<clip>-atlas.json` (uv/trim/anchor/stay/fps/loop) —
//!        products of the minimal-memory path, GPU BC7 upload reads it (gpu.rs), check verifies it.
//!
//! No built-in video decoder (minimal dependencies, no fallback): only optionally invoke ffmpeg for frame conversion when detected on the system;
//! if missing, explicitly prompt the user to convert first, never silently fail.

use std::path::{Path, PathBuf};

use crate::assets_cmd::{load_png, save_png, Img};

/// VBC7 product header byte count: magic(4) + width(4) + height(4) + blocks_x(4) + blocks_y(4).
pub const BC7_HEADER_BYTES: usize = 20;

/// Per-pixel difference threshold for adjacent frame dedup: two frames are "nearly identical" only when the
/// **per-channel** difference of corresponding pixels is ≤ this value AND the ratio of differing pixels is
/// ≤ [`DEDUP_PIXEL_RATIO`]. 0 = dedup only byte-identical frames.
const DEDUP_CHANNEL_TOL: u8 = 2;

/// Upper bound on the ratio of differing pixels (per mille). AI frames often have sporadic jitter noise; exact equality is too strict.
const DEDUP_PIXEL_RATIO_PERMILLE: u64 = 2;

/// A frame's record in the animation config (uv + stay + trim offset + anchor).
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRec {
    /// Relative name of this image in assets/ after dedup (Sprite.image of advance_animations).
    pub image: String,
    /// Pixel rectangle (x, y, w, h) in the atlas, top-left origin.
    pub atlas_rect: (u32, u32, u32, u32),
    /// trim offset: number of left/top blank pixels cropped (used to reposition during playback).
    pub trim_offset: (u32, u32),
    /// Number of original frames this deduped frame represents (stay for N frames).
    pub stay: u32,
}

/// `--frames` product manifest (after writing to disk, for reports/test assertions).
#[derive(Debug)]
pub struct FramesReport {
    /// Clip name (taken from input directory name).
    pub clip: String,
    /// Number of input sequence images.
    pub input_frames: usize,
    /// Number of independent frames remaining after dedup.
    pub kept_frames: usize,
    /// Per-frame records (after dedup + trim + atlas).
    pub records: Vec<FrameRec>,
    /// Atlas size in pixels.
    pub atlas_size: (u32, u32),
    /// Atlas RGBA8 raw byte count (= w*h*4, uncompressed VRAM baseline).
    pub atlas_raw_bytes: u64,
    /// BC7 compressed byte count (None = uncompressed/encoding blocked).
    pub bc7_bytes: Option<u64>,
    /// Relative paths of the produced files (relative to project root).
    pub products: Vec<String>,
}

impl FramesReport {
    pub fn to_json(&self) -> serde_json::Value {
        let ratio = self.bc7_bytes.map(|b| {
            if b == 0 { 0.0 } else { self.atlas_raw_bytes as f64 / b as f64 }
        });
        serde_json::json!({
            "clip": self.clip,
            "input_frames": self.input_frames,
            "kept_frames": self.kept_frames,
            "deduped": self.input_frames.saturating_sub(self.kept_frames),
            "atlas_size": [self.atlas_size.0, self.atlas_size.1],
            "atlas_raw_bytes": self.atlas_raw_bytes,
            "bc7_bytes": self.bc7_bytes,
            "compression_ratio": ratio,
            "products": self.products,
        })
    }
}

/// `--frames` options.
pub struct FramesOptions {
    /// Number of palette colors (reuses assets median_cut). 0 = no unified palette (keep original colors).
    pub colors: usize,
    /// Whether to offline-compress BC7 (product gains a `<clip>-atlas.bc7`).
    pub compress: bool,
}

impl Default for FramesOptions {
    fn default() -> FramesOptions {
        FramesOptions { colors: 32, compress: true }
    }
}

/// CLI entry: `vitric assets <project_dir> --frames <sequence_dir> [--colors N] [--no-compress]`.
///
/// `<sequence_dir>` can be a PNG sequence directory inside or outside the project; products are written into the project's assets/ and
/// alongside the manifest. Clip name is taken from the sequence directory's name.
pub fn run(project_dir: &Path, frames_dir: &Path, opts: &FramesOptions) -> Result<FramesReport, String> {
    let assets_dir = project_dir.join("assets");
    if !assets_dir.is_dir() {
        return Err(format!(
            "[VD090] {} 不存在。提示：--frames 把产物写进项目 assets/，先建好项目再跑",
            assets_dir.display()
        ));
    }
    // Clip name = sequence directory name (deterministic, easy to reference)
    let clip = frames_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("[VD091] 无法从 {} 取片段名（目录名为空？）", frames_dir.display()))?
        .to_string();

    // 1. Collect sequence images (natural sort), detect video files and give explicit prompts (no built-in decoder)
    let paths = collect_sequence(frames_dir)?;
    if paths.is_empty() {
        return Err(format!(
            "[VD092] {} 里没有 PNG 序列。提示：AI 出的是视频就先用 ffmpeg 转序列图——\
             `ffmpeg -i in.mp4 frame%04d.png`（引擎不内置视频解码器）",
            frames_dir.display()
        ));
    }
    let input_frames = paths.len();
    let mut imgs: Vec<Img> = Vec::with_capacity(paths.len());
    let (mut fw, mut fh) = (0u32, 0u32);
    for p in &paths {
        let img = load_png(p).map_err(|e| format!("[VD093] 帧 {}: {e}", p.display()))?;
        if fw == 0 {
            (fw, fh) = (img.width, img.height);
        } else if img.width != fw || img.height != fh {
            return Err(format!(
                "[VD094] 帧 {} 尺寸 {}x{} 与首帧 {fw}x{fh} 不一致。\
                 提示：序列图必须同尺寸（trim 在引擎内做，别预裁）",
                p.display(),
                img.width,
                img.height
            ));
        }
        imgs.push(img);
    }

    // 2. Adjacent frame dedup: keep independent frames + stay count per frame
    let (kept, stays) = dedup_adjacent(&imgs);

    // 3. Unified palette (one set for the whole group, reuses median_cut). colors=0 skips.
    let kept_imgs: Vec<Img> = if opts.colors >= 2 {
        let palette = crate::assets_cmd::extract_palette(kept.iter().map(|&i| &imgs[i]), opts.colors)?;
        kept.iter().map(|&i| crate::assets_cmd::quantize_with(&imgs[i], &palette)).collect()
    } else {
        kept.iter().map(|&i| imgs[i].clone()).collect()
    };

    // 4. trim: crop transparent edges per frame, record offset
    let trimmed: Vec<(Img, (u32, u32))> = kept_imgs.iter().map(trim_transparent).collect();

    // 5. Pack atlas: shelf bin-packing (deterministic, input order), record per-frame uv rectangle
    let (atlas, rects) = pack_atlas(&trimmed.iter().map(|(im, _)| im).collect::<Vec<_>>());

    // 6. Write products
    let mut products = Vec::new();
    let mut records = Vec::new();
    // 6a. Deduped frames written to assets/<clip>/frameNNN.png (real names referenced by advance_animations)
    let clip_dir = assets_dir.join(&clip);
    std::fs::create_dir_all(&clip_dir)
        .map_err(|e| format!("[VD095] 建帧目录 {} 失败: {e}", clip_dir.display()))?;
    let mut frame_names = Vec::new();
    for (i, ((im, trim), rect)) in trimmed.iter().zip(&rects).enumerate() {
        let rel = format!("{clip}/frame{i:03}.png");
        save_png(&assets_dir.join(&rel), im).map_err(|e| format!("[VD096] 写帧 {rel}: {e}"))?;
        frame_names.push(rel.clone());
        records.push(FrameRec {
            image: rel.clone(),
            atlas_rect: *rect,
            trim_offset: *trim,
            stay: stays[i],
        });
        products.push(format!("assets/{rel}"));
    }
    // 6b. animations.json standard clip: stay = repeat frame name in frames list (advance_animations
    //     advances per-frame by fps, repeating N times = stay N frames, render core unchanged, deterministic playback)
    let mut expanded: Vec<&String> = Vec::new();
    for (name, stay) in frame_names.iter().zip(&stays) {
        for _ in 0..(*stay).max(1) {
            expanded.push(name);
        }
    }
    let anim_path = project_dir.join("animations.json");
    merge_clip_into_animations(&anim_path, &clip, &expanded)?;
    products.push("animations.json".to_string());

    // 6c. Atlas png + sidecar json (products of the minimal-memory path, read by gpu.rs/check)
    let atlas_png = format!("{clip}-atlas.png");
    save_png(&assets_dir.join(&atlas_png), &atlas)
        .map_err(|e| format!("[VD097] 写图集 {atlas_png}: {e}"))?;
    products.push(format!("assets/{atlas_png}"));
    let atlas_raw_bytes = atlas.width as u64 * atlas.height as u64 * 4;

    // 6d. BC7 offline compression (optional). Container has no GPU: only encode + persist + byte compare, no upload.
    let bc7_bytes = if opts.compress {
        let (bx, by, data) = crate::bc7::encode_rgba8(atlas.width, atlas.height, &atlas.rgba)?;
        let bc7_rel = format!("{clip}-atlas.bc7");
        write_bc7(&assets_dir.join(&bc7_rel), atlas.width, atlas.height, bx, by, &data)?;
        products.push(format!("assets/{bc7_rel}"));
        Some(BC7_HEADER_BYTES as u64 + data.len() as u64) // Real file size = header + block data
    } else {
        None
    };

    // 6e. atlas sidecar json (frame table: uv + stay + trim offset + anchor + fps/loop)
    let sidecar = atlas_sidecar_json(&clip, atlas.width, atlas.height, &records, bc7_bytes.is_some());
    let sidecar_rel = format!("{clip}-atlas.json");
    std::fs::write(
        assets_dir.join(&sidecar_rel),
        serde_json::to_string_pretty(&sidecar).expect("sidecar 可序列化") + "\n",
    )
    .map_err(|e| format!("[VD098] 写图集配置 {sidecar_rel}: {e}"))?;
    products.push(format!("assets/{sidecar_rel}"));

    Ok(FramesReport {
        clip,
        input_frames,
        kept_frames: kept.len(),
        records,
        atlas_size: (atlas.width, atlas.height),
        atlas_raw_bytes,
        bc7_bytes,
        products,
    })
}

/// Collect PNGs from the sequence directory, sorted by filename natural order (frame2 < frame10).
/// Also detects common video extensions and prompts explicitly to convert first (no built-in decoder, no silent failure).
pub fn collect_sequence(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Err(format!("[VD099] 序列图目录 {} 不存在", dir.display()));
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读序列图目录 {} 失败: {e}", dir.display()))?;
    let mut pngs: Vec<PathBuf> = Vec::new();
    let mut saw_video: Option<String> = None;
    for e in entries {
        let path = e.map_err(|e| format!("读目录项失败: {e}"))?.path();
        if path.is_dir() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" => pngs.push(path),
            "mp4" | "mov" | "webm" | "avi" | "mkv" | "gif" => {
                saw_video.get_or_insert_with(|| path.display().to_string());
            }
            _ => {}
        }
    }
    if pngs.is_empty() {
        if let Some(v) = saw_video {
            return Err(format!(
                "[VD092] {} 里是视频/动图（{v}）不是 PNG 序列。引擎不内置视频解码器，\
                 先转：`ffmpeg -i {v} {}/frame%04d.png` 再跑 --frames",
                dir.display(),
                dir.display()
            ));
        }
    }
    pngs.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    Ok(pngs)
}

/// Natural-order comparison: treat consecutive digit runs as integers (frame2 < frame10), otherwise byte-wise.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.bytes().peekable(), b.bytes().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                // Read digit runs on both sides (skip leading zeros: compare effective length then bytes)
                let xs = take_digits(&mut ai);
                let ys = take_digits(&mut bi);
                let xt = xs.trim_start_matches('0');
                let yt = ys.trim_start_matches('0');
                match xt.len().cmp(&yt.len()).then_with(|| xt.cmp(yt)) {
                    std::cmp::Ordering::Equal => {}
                    ord => return ord,
                }
            }
            (Some(x), Some(y)) => {
                ai.next();
                bi.next();
                match x.cmp(&y) {
                    std::cmp::Ordering::Equal => {}
                    ord => return ord,
                }
            }
        }
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Bytes>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            s.push(c as char);
            it.next();
        } else {
            break;
        }
    }
    s
}

/// Adjacent frame dedup: returns (kept frame indices in original sequence, stay count per kept frame).
/// Stay count = this frame + the number of subsequent frames judged "nearly identical".
pub(crate) fn dedup_adjacent(imgs: &[Img]) -> (Vec<usize>, Vec<u32>) {
    let mut kept: Vec<usize> = Vec::new();
    let mut stays: Vec<u32> = Vec::new();
    for (i, img) in imgs.iter().enumerate() {
        if let Some(&last) = kept.last() {
            if nearly_same(&imgs[last], img) {
                *stays.last_mut().expect("kept 非空时 stays 必非空") += 1;
                continue;
            }
        }
        kept.push(i);
        stays.push(1);
    }
    (kept, stays)
}

/// Whether two frames are "nearly identical": same size + ratio of differing pixels ≤ threshold + per-channel diff of each differing pixel ≤ tolerance.
fn nearly_same(a: &Img, b: &Img) -> bool {
    if a.width != b.width || a.height != b.height {
        return false;
    }
    let total = (a.width as u64) * (a.height as u64);
    if total == 0 {
        return true;
    }
    let allow = (total * DEDUP_PIXEL_RATIO_PERMILLE).div_ceil(1000);
    let mut diff = 0u64;
    for (pa, pb) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)) {
        let over = (0..4).any(|c| pa[c].abs_diff(pb[c]) > DEDUP_CHANNEL_TOL);
        if over {
            diff += 1;
            if diff > allow {
                return false;
            }
        }
    }
    true
}

/// Crop a frame's transparent edges (alpha=0 outer ring), returns (cropped image, (left offset, top offset)).
/// Fully transparent frame is cropped to 1×1 transparent (avoid zero size), offset (0,0).
pub(crate) fn trim_transparent(img: &Img) -> (Img, (u32, u32)) {
    let (w, h) = (img.width, img.height);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            let a = img.rgba[((y * w + x) * 4 + 3) as usize];
            if a > 0 {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        return (Img { width: 1, height: 1, rgba: vec![0, 0, 0, 0] }, (0, 0));
    }
    let nw = max_x - min_x + 1;
    let nh = max_y - min_y + 1;
    let mut rgba = Vec::with_capacity((nw * nh * 4) as usize);
    for y in min_y..=max_y {
        let row = ((y * w + min_x) * 4) as usize;
        rgba.extend_from_slice(&img.rgba[row..row + (nw * 4) as usize]);
    }
    (Img { width: nw, height: nh, rgba }, (min_x, min_y))
}

/// Shelf bin-packing atlas: stitch all frames into one large image, returns (atlas image, per-frame (x,y,w,h) rectangle).
/// Order = input order (deterministic); row width upper bound [`ATLAS_MAX_W`], wrap to new row when exceeded.
pub(crate) fn pack_atlas(frames: &[&Img]) -> (Img, Vec<(u32, u32, u32, u32)>) {
    const ATLAS_MAX_W: u32 = 2048;
    let max_frame_w = frames.iter().map(|f| f.width).max().unwrap_or(1);
    let row_w = ATLAS_MAX_W.max(max_frame_w);
    // Bin-packing: left to right, wrap on wall hit, record per-frame top-left corner
    let mut rects = Vec::with_capacity(frames.len());
    let (mut x, mut y, mut row_h, mut used_w) = (0u32, 0u32, 0u32, 0u32);
    for f in frames {
        if x + f.width > row_w && x > 0 {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        rects.push((x, y, f.width, f.height));
        x += f.width;
        row_h = row_h.max(f.height);
        used_w = used_w.max(x);
    }
    let atlas_w = used_w.max(1);
    let atlas_h = (y + row_h).max(1);
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];
    for (f, &(rx, ry, rw, rh)) in frames.iter().zip(&rects) {
        for fy in 0..rh {
            let src = ((fy * rw) * 4) as usize;
            let dst = (((ry + fy) * atlas_w + rx) * 4) as usize;
            rgba[dst..dst + (rw * 4) as usize].copy_from_slice(&f.rgba[src..src + (rw * 4) as usize]);
        }
    }
    (Img { width: atlas_w, height: atlas_h, rgba }, rects)
}

/// JSON of the atlas sidecar (frame table = normalized uv rectangle + pixel rectangle + trim offset + stay + anchor).
/// Anchor convention: center of cropped frame + trim offset = original frame center, playback repositions to original, visual unchanged.
fn atlas_sidecar_json(
    clip: &str,
    aw: u32,
    ah: u32,
    records: &[FrameRec],
    compressed: bool,
) -> serde_json::Value {
    let frames: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            let (x, y, w, h) = r.atlas_rect;
            serde_json::json!({
                "image": r.image,
                "rect": [x, y, w, h],
                "uv": [
                    x as f64 / aw as f64,
                    y as f64 / ah as f64,
                    (x + w) as f64 / aw as f64,
                    (y + h) as f64 / ah as f64,
                ],
                "trim_offset": [r.trim_offset.0, r.trim_offset.1],
                "stay": r.stay,
            })
        })
        .collect();
    serde_json::json!({
        "clip": clip,
        "atlas": format!("{clip}-atlas.png"),
        "compressed": if compressed { serde_json::json!(format!("{clip}-atlas.bc7")) } else { serde_json::Value::Null },
        "atlas_size": [aw, ah],
        "frames": frames,
    })
}

/// BC7 product file format (self-describing header + block data), read by gpu.rs on upload:
///   magic "VBC7"(4) | width u32 | height u32 | blocks_x u32 | blocks_y u32 | block data
/// All little-endian. Header lets the GPU side know the real texture size (block grid may have been rounded up).
fn write_bc7(
    path: &Path,
    width: u32,
    height: u32,
    bx: u32,
    by: u32,
    data: &[u8],
) -> Result<(), String> {
    let mut out = Vec::with_capacity(BC7_HEADER_BYTES + data.len());
    out.extend_from_slice(b"VBC7");
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&bx.to_le_bytes());
    out.extend_from_slice(&by.to_le_bytes());
    out.extend_from_slice(data);
    std::fs::write(path, out).map_err(|e| format!("[VD09A] 写 BC7 {} 失败: {e}", path.display()))
}

/// Parse the BC7 product header (shared by gpu.rs upload / check verification).
/// Returns (width, height, blocks_x, blocks_y, block data start offset).
pub fn parse_bc7_header(bytes: &[u8]) -> Result<(u32, u32, u32, u32, usize), String> {
    if bytes.len() < BC7_HEADER_BYTES || &bytes[0..4] != b"VBC7" {
        return Err("[VD09B] 不是合法的 VBC7 文件（魔数/长度不符）".to_string());
    }
    let rd = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 字节"));
    let (w, h, bx, by) = (rd(4), rd(8), rd(12), rd(16));
    let want = bx as usize * by as usize * crate::bc7::BLOCK_BYTES;
    if bytes.len() - BC7_HEADER_BYTES != want {
        return Err(format!(
            "[VD09C] VBC7 块数据长度 {} 与 {bx}x{by} 块×16 的 {want} 不符",
            bytes.len() - BC7_HEADER_BYTES
        ));
    }
    Ok((w, h, bx, by, BC7_HEADER_BYTES))
}

/// Merge a clip into animations.json (preserving existing clips, same-name overwrite). Creates the file if missing.
fn merge_clip_into_animations(path: &Path, clip: &str, frames: &[&String]) -> Result<(), String> {
    let mut doc: serde_json::Value = if path.exists() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("[VD09D] 读 {} 失败: {e}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|e| format!("[VD09E] {} 解析失败: {e}", path.display()))?
    } else {
        serde_json::json!({ "clips": {} })
    };
    let clips = doc
        .get_mut("clips")
        .and_then(|c| c.as_object_mut())
        .ok_or_else(|| format!("[VD09F] {} 缺 clips 对象", path.display()))?;
    // fps fixed at 60 (one frame per tick): stay is already expanded as repeated frames in the frame sequence, so playback rate
    // at one frame per tick is most intuitive (advance_animations: t*fps/60 → when fps=60, idx=t).
    clips.insert(
        clip.to_string(),
        serde_json::json!({
            "frames": frames,
            "fps": vitric_sim::TICKS_PER_SECOND,
            "loop": true,
        }),
    );
    std::fs::write(path, serde_json::to_string_pretty(&doc).expect("animations 可序列化") + "\n")
        .map_err(|e| format!("[VD0A0] 写 {} 失败: {e}", path.display()))?;
    Ok(())
}

/// Whether ffmpeg is available on the system (found on PATH). Used by `--frames` to decide whether to auto-convert when video is detected.
/// Currently only detects, does not auto-invoke — keeps the "no built-in, prompt user" minimal-dependency stance.
pub fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Verify that a clip's atlas products are complete and valid (called by vitric check):
/// atlas png exists, sidecar frame table is valid, uv is in [0,1] and points back inside the atlas, and referenced frame images all exist.
/// Errors carry path + VDxxx code, all reported at once.
pub fn check_atlas_products(
    project_dir: &Path,
    sidecar_rel: &str,
    problems: &mut Vec<String>,
) {
    let assets_dir = project_dir.join("assets");
    let path = assets_dir.join(sidecar_rel);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("[VD0A1] 图集配置 {} 读取失败: {e}", path.display()));
            return;
        }
    };
    let doc: serde_json::Value = match serde_json::from_str(&text) {
        Ok(d) => d,
        Err(e) => {
            problems.push(format!("[VD0A2] 图集配置 {} 解析失败: {e}", path.display()));
            return;
        }
    };
    // Atlas png exists
    let atlas_name = doc.get("atlas").and_then(|v| v.as_str());
    match atlas_name {
        Some(name) if assets_dir.join(name).exists() => {}
        Some(name) => problems.push(format!(
            "[VD0A3] 图集 {} 的 atlas 字段指向不存在的图集 {name:?}",
            path.display()
        )),
        None => problems.push(format!("[VD0A4] 图集 {} 缺 atlas 字段", path.display())),
    }
    // Atlas size
    let size = doc.get("atlas_size").and_then(|v| v.as_array());
    let (aw, ah) = match size.and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?))) {
        Some((w, h)) if w > 0 && h > 0 => (w, h),
        _ => {
            problems.push(format!("[VD0A5] 图集 {} 的 atlas_size 非法（要 [w>0, h>0]）", path.display()));
            return;
        }
    };
    // Frame table valid + uv in bounds + frame image references all present
    let Some(frames) = doc.get("frames").and_then(|v| v.as_array()) else {
        problems.push(format!("[VD0A6] 图集 {} 缺 frames 数组", path.display()));
        return;
    };
    if frames.is_empty() {
        problems.push(format!("[VD0A7] 图集 {} 的 frames 为空（至少一帧）", path.display()));
    }
    for (i, f) in frames.iter().enumerate() {
        let rect = f.get("rect").and_then(|v| v.as_array());
        match rect.and_then(|r| {
            Some((r.first()?.as_u64()?, r.get(1)?.as_u64()?, r.get(2)?.as_u64()?, r.get(3)?.as_u64()?))
        }) {
            Some((x, y, w, h)) => {
                if x + w > aw || y + h > ah {
                    problems.push(format!(
                        "[VD0A8] 图集 {} 第 {i} 帧 rect [{x},{y},{w},{h}] 越出图集 {aw}x{ah}",
                        path.display()
                    ));
                }
            }
            None => problems.push(format!(
                "[VD0A9] 图集 {} 第 {i} 帧 rect 非法（要 [x,y,w,h] 四个非负整数）",
                path.display()
            )),
        }
        if let Some(name) = f.get("image").and_then(|v| v.as_str()) {
            if !assets_dir.join(name).exists() {
                problems.push(format!(
                    "[VD0AA] 图集 {} 第 {i} 帧引用了不存在的帧图 {name:?}",
                    path.display()
                ));
            }
        } else {
            problems.push(format!("[VD0AB] 图集 {} 第 {i} 帧缺 image 字段", path.display()));
        }
    }
    // Compressed product (if declared, must be present)
    if let Some(bc7) = doc.get("compressed").and_then(|v| v.as_str()) {
        let bp = assets_dir.join(bc7);
        match std::fs::read(&bp) {
            Ok(bytes) => {
                if let Err(e) = parse_bc7_header(&bytes) {
                    problems.push(format!("[VD0AC] 图集 {} 的压缩产物 {bc7}: {e}", path.display()));
                }
            }
            Err(e) => problems.push(format!(
                "[VD0AD] 图集 {} 声明了压缩产物 {bc7} 但读取失败: {e}",
                path.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a solid-color (or fully transparent) image.
    fn solid(w: u32, h: u32, c: [u8; 4]) -> Img {
        Img { width: w, height: h, rgba: c.to_vec().repeat((w * h) as usize) }
    }

    /// Adjacent duplicate frame dedup: 3 identical → keep 1, stay 3.
    #[test]
    fn dedup_collapses_identical_runs() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [200, 0, 0, 255]);
        let imgs = vec![a.clone(), a.clone(), a.clone(), b.clone(), a.clone()];
        let (kept, stays) = dedup_adjacent(&imgs);
        assert_eq!(kept, vec![0, 3, 4], "相同段塌成一张，非相邻的同色不合并");
        assert_eq!(stays, vec![3, 1, 1], "前三帧停留计数 3");
    }

    /// Light jitter within tolerance counts as "nearly identical" and gets deduped.
    #[test]
    fn dedup_tolerates_small_jitter() {
        let mut a = solid(10, 10, [100, 100, 100, 255]);
        let mut b = a.clone();
        // Change one pixel by 1 gray level (within tolerance, ratio far below threshold)
        b.rgba[0] = 101;
        a.rgba[0] = 100;
        let (kept, stays) = dedup_adjacent(&[a, b]);
        assert_eq!(kept.len(), 1, "抖动在容差内 → 去重");
        assert_eq!(stays, vec![2]);
    }

    /// trim crops transparent edges and records offset; cropped content repositioned = original (visual unchanged).
    #[test]
    fn trim_crops_and_records_offset() {
        // 8x8 fully transparent, with a 2x2 opaque block at (2,3)
        let mut img = solid(8, 8, [0, 0, 0, 0]);
        for y in 3..5u32 {
            for x in 2..4u32 {
                let o = ((y * 8 + x) * 4) as usize;
                img.rgba[o..o + 4].copy_from_slice(&[9, 8, 7, 255]);
            }
        }
        let (trimmed, off) = trim_transparent(&img);
        assert_eq!((trimmed.width, trimmed.height), (2, 2), "裁到内容外接框");
        assert_eq!(off, (2, 3), "偏移 = 内容左上角");
        // Every pixel after cropping is that opaque color
        for px in trimmed.rgba.chunks_exact(4) {
            assert_eq!(px, &[9, 8, 7, 255]);
        }
    }

    /// Fully transparent frame cropped to 1x1 transparent, no zero-size blowup.
    #[test]
    fn trim_all_transparent_is_1x1() {
        let (t, off) = trim_transparent(&solid(5, 5, [0, 0, 0, 0]));
        assert_eq!((t.width, t.height, off), (1, 1, (0, 0)));
    }

    /// atlas packing: per-frame rectangles are in bounds and can reconstruct each frame's pixels from the atlas.
    #[test]
    fn atlas_rects_reconstruct_frames() {
        let f0 = solid(3, 2, [1, 2, 3, 255]);
        let f1 = solid(2, 4, [9, 9, 9, 255]);
        let frames = vec![&f0, &f1];
        let (atlas, rects) = pack_atlas(&frames);
        assert_eq!(rects.len(), 2);
        for (f, &(x, y, w, h)) in frames.iter().zip(&rects) {
            assert_eq!((w, h), (f.width, f.height), "矩形尺寸 = 帧尺寸");
            assert!(x + w <= atlas.width && y + h <= atlas.height, "矩形不越界");
            // Per-pixel reconstruction
            for fy in 0..h {
                for fx in 0..w {
                    let a = (((y + fy) * atlas.width + x + fx) * 4) as usize;
                    let s = ((fy * w + fx) * 4) as usize;
                    assert_eq!(&atlas.rgba[a..a + 4], &f.rgba[s..s + 4], "图集应能还原帧像素");
                }
            }
        }
    }

    /// Natural sort: frame2 comes before frame10 (not lexicographic).
    #[test]
    fn natural_sort_orders_numbers() {
        let mut v = vec!["frame10.png", "frame2.png", "frame1.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["frame1.png", "frame2.png", "frame10.png"]);
    }

    /// VBC7 header roundtrip: bytes written by write_bc7 can be read back by parse_bc7_header to original size + block grid.
    #[test]
    fn vbc7_header_roundtrip() {
        let (w, h) = (12u32, 8u32);
        let rgba = vec![100u8; (w * h * 4) as usize];
        let (bx, by, data) = crate::bc7::encode_rgba8(w, h, &rgba).unwrap();
        let dir = std::env::temp_dir().join(format!("vbc7-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.bc7");
        write_bc7(&p, w, h, bx, by, &data).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let (rw, rh, rbx, rby, off) = parse_bc7_header(&bytes).unwrap();
        assert_eq!((rw, rh, rbx, rby, off), (w, h, bx, by, BC7_HEADER_BYTES));
        assert_eq!(&bytes[off..], &data[..], "块数据应在头之后原样");
    }

    /// Bad VBC7 (wrong magic / mismatched length) reports explicit error.
    #[test]
    fn vbc7_header_rejects_garbage() {
        assert!(parse_bc7_header(b"NOPE0000000000000000").is_err(), "魔数错应报错");
        // Header OK but block data length doesn't match
        let mut bad = Vec::new();
        bad.extend_from_slice(b"VBC7");
        bad.extend_from_slice(&4u32.to_le_bytes()); // w
        bad.extend_from_slice(&4u32.to_le_bytes()); // h
        bad.extend_from_slice(&1u32.to_le_bytes()); // bx
        bad.extend_from_slice(&1u32.to_le_bytes()); // by → expects 16 bytes of block data
        bad.extend_from_slice(&[0u8; 8]); // only give 8 bytes
        let err = parse_bc7_header(&bad).unwrap_err();
        assert!(err.contains("VD09C"), "长度不符错误码: {err}");
    }
}
