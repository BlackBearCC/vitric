//! vitric assets --frames — AI 出的一堆序列图一键变优化过的动画素材。
//!
//! 现实路径：AI 生成视频/序列图 → 切帧（骨骼绑定是 AI 最不擅长的活），所以把
//! 「帧进口 + 极致内存」做到位比追骨骼动画更对个人开发者胃口。前车之鉴：桌宠
//! 2400 帧不压缩 RGBA8 全驻留 → 8.5G 显存。
//!
//! 流水线（全部确定性，同输入逐字节同产物）：
//!   1. 相邻帧去重：逐像素近乎相同的相邻帧只留一张，记「停留多少帧」（AI 动画常有静止段）。
//!   2. 裁空白边 trim：每帧裁透明边，记偏移（播放按偏移摆回原位，视觉不变）。
//!   3. 打包图集 atlas：所有帧拼一张大图（减 GPU 纹理切换），记每帧 uv 矩形。
//!   4. 统一色板：复用 median_cut，整组一套（assets_cmd::quantize_with）。
//!   5. 写动画配置：
//!      - `animations.json` 标准 clip（去重后的帧名，停留=重复帧名）——喂 advance_animations
//!        确定播放，render 核心一字不改（仍是 Sprite.image=完整帧名）。
//!      - `<clip>-atlas.png` + `<clip>-atlas.json`（uv/trim/anchor/停留/fps/loop）——
//!        极致内存路径的产物，GPU BC7 上传读它（gpu.rs），check 校验它。
//!
//! 视频不内置解码器（依赖最小、不写 fallback）：检测系统有 ffmpeg 才可选调用转帧，
//! 没有就明确提示用户先转，不静默失败。

use std::path::{Path, PathBuf};

use crate::assets_cmd::{load_png, save_png, Img};

/// VBC7 产物头字节数：magic(4) + width(4) + height(4) + blocks_x(4) + blocks_y(4)。
pub const BC7_HEADER_BYTES: usize = 20;

/// 相邻帧去重的逐像素差阈值：两帧对应像素**每通道**差都 ≤ 此值，且差异像素占比
/// ≤ [`DEDUP_PIXEL_RATIO`] 才算「近乎相同」。0 = 仅去重逐字节完全相同的帧。
const DEDUP_CHANNEL_TOL: u8 = 2;

/// 允许有差异的像素占比上界（千分比）。AI 帧常有零星抖动噪点，全等太严。
const DEDUP_PIXEL_RATIO_PERMILLE: u64 = 2;

/// 一帧在动画配置里的记录（uv + 停留 + trim 偏移 + 锚点）。
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRec {
    /// 去重后这张图在 assets/ 里的相对名（advance_animations 的 Sprite.image）。
    pub image: String,
    /// 在图集里的像素矩形（x, y, w, h），左上原点。
    pub atlas_rect: (u32, u32, u32, u32),
    /// trim 偏移：裁掉的左/上空白像素数（播放摆回原位用）。
    pub trim_offset: (u32, u32),
    /// 这张去重帧代表的原始帧数（停留多少帧）。
    pub stay: u32,
}

/// `--frames` 的产物清单（写盘后供报告/测试断言）。
#[derive(Debug)]
pub struct FramesReport {
    /// 片段名（取自输入目录名）。
    pub clip: String,
    /// 输入序列图张数。
    pub input_frames: usize,
    /// 去重后剩余的独立帧数。
    pub kept_frames: usize,
    /// 每帧记录（去重 + trim + atlas 之后）。
    pub records: Vec<FrameRec>,
    /// 图集尺寸（像素）。
    pub atlas_size: (u32, u32),
    /// 图集 RGBA8 raw 字节数（= w*h*4，未压缩显存基线）。
    pub atlas_raw_bytes: u64,
    /// BC7 压缩字节数（None = 未压缩/编码受阻）。
    pub bc7_bytes: Option<u64>,
    /// 写出的产物文件相对路径（项目根起算）。
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

/// `--frames` 选项。
pub struct FramesOptions {
    /// 色板颜色数（复用 assets median_cut）。0 = 不做统一色板（保留原色）。
    pub colors: usize,
    /// 是否离线压 BC7（产物多一个 `<clip>-atlas.bc7`）。
    pub compress: bool,
}

impl Default for FramesOptions {
    fn default() -> FramesOptions {
        FramesOptions { colors: 32, compress: true }
    }
}

/// CLI 入口：`vitric assets <项目目录> --frames <序列图目录> [--colors N] [--no-compress]`。
///
/// `<序列图目录>` 可以是项目内或项目外的 png 序列目录；产物写进项目的 assets/ 和
/// 清单旁。片段名取序列图目录的目录名。
pub fn run(project_dir: &Path, frames_dir: &Path, opts: &FramesOptions) -> Result<FramesReport, String> {
    let assets_dir = project_dir.join("assets");
    if !assets_dir.is_dir() {
        return Err(format!(
            "[VD090] {} 不存在。提示：--frames 把产物写进项目 assets/，先建好项目再跑",
            assets_dir.display()
        ));
    }
    // 片段名 = 序列图目录名（确定性、好引用）
    let clip = frames_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("[VD091] 无法从 {} 取片段名（目录名为空？）", frames_dir.display()))?
        .to_string();

    // 1. 收序列图（自然排序），检测视频文件给明确提示（不内置解码器）
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

    // 2. 相邻帧去重：留下独立帧 + 每张的停留数
    let (kept, stays) = dedup_adjacent(&imgs);

    // 3. 统一色板（整组一套，复用 median_cut）。colors=0 跳过。
    let kept_imgs: Vec<Img> = if opts.colors >= 2 {
        let palette = crate::assets_cmd::extract_palette(kept.iter().map(|&i| &imgs[i]), opts.colors)?;
        kept.iter().map(|&i| crate::assets_cmd::quantize_with(&imgs[i], &palette)).collect()
    } else {
        kept.iter().map(|&i| imgs[i].clone()).collect()
    };

    // 4. trim：裁每张的透明边，记偏移
    let trimmed: Vec<(Img, (u32, u32))> = kept_imgs.iter().map(trim_transparent).collect();

    // 5. 打包图集：货架装箱（确定，输入序），记每帧 uv 矩形
    let (atlas, rects) = pack_atlas(&trimmed.iter().map(|(im, _)| im).collect::<Vec<_>>());

    // 6. 写产物
    let mut products = Vec::new();
    let mut records = Vec::new();
    // 6a. 去重帧写进 assets/<clip>/frameNNN.png（advance_animations 引用的真名）
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
    // 6b. animations.json 标准 clip：停留 = 在 frames 列表里重复帧名（advance_animations
    //     按 fps 逐帧推进，重复 N 次 = 停留 N 帧，render 核心零改、确定播放）
    let mut expanded: Vec<&String> = Vec::new();
    for (name, stay) in frame_names.iter().zip(&stays) {
        for _ in 0..(*stay).max(1) {
            expanded.push(name);
        }
    }
    let anim_path = project_dir.join("animations.json");
    merge_clip_into_animations(&anim_path, &clip, &expanded)?;
    products.push("animations.json".to_string());

    // 6c. 图集 png + sidecar json（极致内存路径产物，gpu.rs/check 读它）
    let atlas_png = format!("{clip}-atlas.png");
    save_png(&assets_dir.join(&atlas_png), &atlas)
        .map_err(|e| format!("[VD097] 写图集 {atlas_png}: {e}"))?;
    products.push(format!("assets/{atlas_png}"));
    let atlas_raw_bytes = atlas.width as u64 * atlas.height as u64 * 4;

    // 6d. BC7 离线压缩（可选）。容器无 GPU：只编码 + 落盘 + 字节对比，不上传。
    let bc7_bytes = if opts.compress {
        let (bx, by, data) = crate::bc7::encode_rgba8(atlas.width, atlas.height, &atlas.rgba)?;
        let bc7_rel = format!("{clip}-atlas.bc7");
        write_bc7(&assets_dir.join(&bc7_rel), atlas.width, atlas.height, bx, by, &data)?;
        products.push(format!("assets/{bc7_rel}"));
        Some(BC7_HEADER_BYTES as u64 + data.len() as u64) // 文件真实大小 = 头 + 块数据
    } else {
        None
    };

    // 6e. atlas sidecar json（帧表：uv + 停留 + trim 偏移 + 锚点 + fps/loop）
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

/// 收集序列图目录里的 PNG，按文件名自然排序（frame2 < frame10）。
/// 顺带检测常见视频扩展名，命中就明确提示先转（不内置解码器、不静默失败）。
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

/// 自然排序比较：把连续数字段当整数比（frame2 < frame10），其余按字节。
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.bytes().peekable(), b.bytes().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                // 读两侧的整段数字（跳过前导 0 的影响：先比有效长度再比字节）
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

/// 相邻帧去重：返回（保留帧在原序列里的下标, 每个保留帧的停留数）。
/// 停留数 = 这张帧 + 紧随其后被判定为「近乎相同」的帧数。
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

/// 两帧是否「近乎相同」：尺寸相同 + 差异像素占比 ≤ 阈值 + 每个差异像素每通道差 ≤ 容差。
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

/// 裁掉一帧的透明边（alpha=0 的外圈），返回（裁后图, (左偏移, 上偏移)）。
/// 全透明帧裁成 1×1 透明（避免 0 尺寸），偏移 (0,0)。
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

/// 货架装箱打包图集：所有帧拼一张大图，返回（图集图, 每帧 (x,y,w,h) 矩形）。
/// 顺序 = 输入序（确定）；行宽上限 [`ATLAS_MAX_W`]，超了换行。
pub(crate) fn pack_atlas(frames: &[&Img]) -> (Img, Vec<(u32, u32, u32, u32)>) {
    const ATLAS_MAX_W: u32 = 2048;
    let max_frame_w = frames.iter().map(|f| f.width).max().unwrap_or(1);
    let row_w = ATLAS_MAX_W.max(max_frame_w);
    // 装箱：左到右、撞墙换行，记每帧左上角
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

/// atlas sidecar 的 JSON（帧表 = uv 归一化矩形 + 像素矩形 + trim 偏移 + 停留 + 锚点）。
/// 锚点约定：裁后帧的中心 + trim 偏移 = 原帧中心，播放摆回原位视觉不变。
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

/// BC7 产物文件格式（自描述头 + 块数据），gpu.rs 上传时读它：
///   magic "VBC7"(4) | width u32 | height u32 | blocks_x u32 | blocks_y u32 | 块数据
/// 全小端。头让 GPU 端知道纹理真实尺寸（块网格可能向上取整过）。
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

/// 解析 BC7 产物头（gpu.rs 上传 / check 校验共用）。
/// 返回 (width, height, blocks_x, blocks_y, 块数据起点)。
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

/// 把一个 clip 合并进 animations.json（保留已有 clip，同名覆盖）。文件不存在则新建。
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
    // fps 固定 60（每 tick 一帧）：停留已经在帧序列里展开成重复帧，所以播放速率
    // 取每 tick 推一帧最直观（advance_animations: t*fps/60 → fps=60 时 idx=t）。
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

/// 系统是否有 ffmpeg（PATH 上找得到）。`--frames` 检测到视频时用它判断能否自动转。
/// 目前只检测、不自动调用——保持「不内置、提示用户」的最小依赖姿态。
pub fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 校验一个 clip 的 atlas 产物是否齐全合法（vitric check 调用）：
/// 图集 png 存在、sidecar 帧表合法、uv 在 [0,1] 且回指图集内、引用的帧图都在。
/// 错误带路径 + VDxxx 码，一次报全。
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
    // 图集 png 存在
    let atlas_name = doc.get("atlas").and_then(|v| v.as_str());
    match atlas_name {
        Some(name) if assets_dir.join(name).exists() => {}
        Some(name) => problems.push(format!(
            "[VD0A3] 图集 {} 的 atlas 字段指向不存在的图集 {name:?}",
            path.display()
        )),
        None => problems.push(format!("[VD0A4] 图集 {} 缺 atlas 字段", path.display())),
    }
    // 图集尺寸
    let size = doc.get("atlas_size").and_then(|v| v.as_array());
    let (aw, ah) = match size.and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?))) {
        Some((w, h)) if w > 0 && h > 0 => (w, h),
        _ => {
            problems.push(format!("[VD0A5] 图集 {} 的 atlas_size 非法（要 [w>0, h>0]）", path.display()));
            return;
        }
    };
    // 帧表合法 + uv 不越界 + 帧图引用都在
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
    // 压缩产物（声明了就必须在）
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

    /// 造一张纯色（或纯透明）图。
    fn solid(w: u32, h: u32, c: [u8; 4]) -> Img {
        Img { width: w, height: h, rgba: c.to_vec().repeat((w * h) as usize) }
    }

    /// 相邻重复帧去重：3 张相同 → 留 1 张、停留 3。
    #[test]
    fn dedup_collapses_identical_runs() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [200, 0, 0, 255]);
        let imgs = vec![a.clone(), a.clone(), a.clone(), b.clone(), a.clone()];
        let (kept, stays) = dedup_adjacent(&imgs);
        assert_eq!(kept, vec![0, 3, 4], "相同段塌成一张，非相邻的同色不合并");
        assert_eq!(stays, vec![3, 1, 1], "前三帧停留计数 3");
    }

    /// 容差内的轻微抖动算「近乎相同」被去重。
    #[test]
    fn dedup_tolerates_small_jitter() {
        let mut a = solid(10, 10, [100, 100, 100, 255]);
        let mut b = a.clone();
        // 改一个像素 1 个灰阶（在容差内、占比远低于阈值）
        b.rgba[0] = 101;
        a.rgba[0] = 100;
        let (kept, stays) = dedup_adjacent(&[a, b]);
        assert_eq!(kept.len(), 1, "抖动在容差内 → 去重");
        assert_eq!(stays, vec![2]);
    }

    /// trim 裁掉透明边并记偏移；裁后内容回摆 = 原位（视觉不变）。
    #[test]
    fn trim_crops_and_records_offset() {
        // 8x8 全透明，中间 (2,3) 放一个 2x2 不透明块
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
        // 裁后每个像素都是那个不透明色
        for px in trimmed.rgba.chunks_exact(4) {
            assert_eq!(px, &[9, 8, 7, 255]);
        }
    }

    /// 全透明帧裁成 1x1 透明，不炸 0 尺寸。
    #[test]
    fn trim_all_transparent_is_1x1() {
        let (t, off) = trim_transparent(&solid(5, 5, [0, 0, 0, 0]));
        assert_eq!((t.width, t.height, off), (1, 1, (0, 0)));
    }

    /// atlas 打包：每帧矩形不越界、能从图集还原每帧像素。
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
            // 逐像素还原
            for fy in 0..h {
                for fx in 0..w {
                    let a = (((y + fy) * atlas.width + x + fx) * 4) as usize;
                    let s = ((fy * w + fx) * 4) as usize;
                    assert_eq!(&atlas.rgba[a..a + 4], &f.rgba[s..s + 4], "图集应能还原帧像素");
                }
            }
        }
    }

    /// 自然排序：frame2 排在 frame10 前面（不是字典序）。
    #[test]
    fn natural_sort_orders_numbers() {
        let mut v = vec!["frame10.png", "frame2.png", "frame1.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["frame1.png", "frame2.png", "frame10.png"]);
    }

    /// VBC7 头往返：write_bc7 写出的字节用 parse_bc7_header 能读回原尺寸 + 块网格。
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

    /// 坏 VBC7（魔数错 / 长度不符）显式报错。
    #[test]
    fn vbc7_header_rejects_garbage() {
        assert!(parse_bc7_header(b"NOPE0000000000000000").is_err(), "魔数错应报错");
        // 头对但块数据长度对不上
        let mut bad = Vec::new();
        bad.extend_from_slice(b"VBC7");
        bad.extend_from_slice(&4u32.to_le_bytes()); // w
        bad.extend_from_slice(&4u32.to_le_bytes()); // h
        bad.extend_from_slice(&1u32.to_le_bytes()); // bx
        bad.extend_from_slice(&1u32.to_le_bytes()); // by → 期望 16 字节块数据
        bad.extend_from_slice(&[0u8; 8]); // 只给 8 字节
        let err = parse_bc7_header(&bad).unwrap_err();
        assert!(err.contains("VD09C"), "长度不符错误码: {err}");
    }
}
