//! vitric assets — 把项目里所有 PNG 规整到同一张色板上（素材和谐化）。
//!
//! 解决的问题：AI 出图每张色调/风格都不一样，拼到一个画面里就散。
//! 做法：全项目像素一起提一张共享色板（中位切分量化），再把每张图的
//! 不透明像素吸附到最近的色板色——一个项目一张色板 = 视觉和谐。
//!
//! 约束（都是刻意的）：
//! - 确定性：遍历顺序排序、频次表用 BTreeMap、切分无随机——同样的输入永远出同样的色板和字节。
//! - 安全：动手前先把原件备份到 assets_original/；已有备份就拒绝跑，不静默覆盖上次的原件。
//! - 透明度：alpha=0 的像素保持全透明（RGB 归零）；半透明像素保留 alpha 只量化 RGB。
//! - palette.json 是项目的官方色板：--palette-lock 跳过提取直接用它，后补的素材自动入伙。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 解码后的图片（RGBA8），和 vitric-render 的约定一致。
/// pub(crate)：法线生成（normals.rs）复用同一套解码/编码，不再造一份。
pub(crate) struct Img {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

pub struct Options {
    /// 色板颜色数上限（中位切分的目标箱数）。
    pub colors: usize,
    /// 给了就把高于 H 的图先按最近邻缩到高 H（保持宽高比）——像素风路径，缩放在提色板之前。
    pub height: Option<u32>,
    /// 跳过提取，直接用项目已有的 palette.json——新素材按老色板入伙。
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

/// CLI 入口：`vitric assets <项目目录> [--colors N] [--height H] [--palette-lock]
/// | --normals [--normals-ai]`。色板和谐化与法线生成是两种模式，不混着跑
/// （一次只做一件事，报告才说得清）。
pub fn run(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("assets 缺少项目目录参数")?;
    let mut opts = Options::default();
    let mut normals = false;
    let mut normals_ai = false;
    let mut palette_opts_given = false;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--colors" => {
                opts.colors = args
                    .get(i + 1)
                    .ok_or(need("--colors"))?
                    .parse()
                    .map_err(|e| format!("--colors: {e}"))?;
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
            // --normals-ai 蕴含 --normals（它只是换生成器）
            "--normals-ai" => {
                normals = true;
                normals_ai = true;
                i += 1;
            }
            other => {
                return Err(format!(
                    "未知选项 {other:?}。可用: --colors --height --palette-lock --normals --normals-ai"
                ))
            }
        }
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

/// 主流程：收集 → （可选缩放）→ 提/读色板 → 备份原件 → 量化回写 → 落盘 palette.json。
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
    // 安全门：已有备份就拒绝。覆盖备份 = 永久丢失真正的原件，这种事不静默发生。
    let backup_dir = project_dir.join("assets_original");
    if backup_dir.exists() {
        return Err(format!(
            "{} 已有 assets_original 备份，确认后删掉它再跑——不静默覆盖上次的原件",
            project_dir.display()
        ));
    }

    let mut rels = collect_pngs(&assets_dir)?;
    // 法线贴图（_n 配对，约定见 vitric_render::is_normal_map_name）整个排除在和谐化之外：
    // 不进色板提取、不量化、不缩放、不备份——RGB 编码的是向量不是颜色，吸附到色板
    // 等于毁掉法线数据，且之后每次重跑都会再毁一遍。
    rels.retain(|r| !vitric_render::is_normal_map_name(r));
    if rels.is_empty() {
        return Err(format!(
            "{} 里没有 PNG（法线贴图 _n 文件不算，它们不参与和谐化）。\
             提示：先把 AI 出的图（PNG）放进 assets/ 再跑",
            assets_dir.display()
        ));
    }

    // 解码 +（可选）缩放。缩放必须在提色板之前：缩完的像素才是最终参与配色的像素。
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

    // 色板：锁定模式用项目已有的 palette.json，否则从全项目不透明像素提取（按频次加权）。
    let palette_path = project_dir.join("palette.json");
    let palette: Vec<[u8; 3]> = if opts.palette_lock {
        read_palette(&palette_path)?
    } else {
        let mut freq: BTreeMap<[u8; 3], u64> = BTreeMap::new();
        for (_, img) in &images {
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
        median_cut(&freq, opts.colors)
    };

    // 先备份再动手：备份没写全就报错退出，assets/ 一个字节都没改。
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

    // 量化回写：不透明像素吸附最近色板色，alpha 原样保留。
    let mut bytes_after = 0u64;
    for (rel, img) in &images {
        let q = quantize(img, &palette);
        let path = assets_dir.join(rel);
        save_png(&path, &q).map_err(|e| format!("素材 {rel}: {e}"))?;
        bytes_after += std::fs::metadata(&path)
            .map_err(|e| format!("素材 {rel}: 读元数据失败: {e}"))?
            .len();
    }

    // palette.json = 项目官方色板。锁定模式不重写（它就是输入）。
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

/// 递归收集 assets/ 下全部 PNG 的相对路径（正斜杠），排序保证确定性。
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

/// 中位切分量化：把（颜色 → 频次）切成至多 n 个箱子，每箱取加权平均色。
///
/// 确定性约束：输入是 BTreeMap（迭代有序）；选箱按像素数最多、并列取下标小的；
/// 切分通道并列取 R→G→B 靠前的；箱内排序带全元组兜底——全程没有 HashMap、没有随机。
fn median_cut(freq: &BTreeMap<[u8; 3], u64>, n: usize) -> Vec<[u8; 3]> {
    let entries: Vec<([u8; 3], u64)> = freq.iter().map(|(c, &w)| (*c, w)).collect();
    if entries.len() <= n {
        // 颜色本来就不多：原色直接当色板，一个像素都不动
        return entries.into_iter().map(|(c, _)| c).collect();
    }
    let mut boxes: Vec<Vec<([u8; 3], u64)>> = vec![entries];
    while boxes.len() < n {
        // 选还能切（≥2 种颜色）且像素最多的箱子；并列取下标小的
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
        // 沿跨度最大的通道排序，在加权中位数处切开
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
    // 每箱取加权平均色；BTreeSet 去重 + 排序，色板顺序与内容都确定
    let mut out: BTreeSet<[u8; 3]> = BTreeSet::new();
    for b in &boxes {
        out.insert(box_average(b));
    }
    out.into_iter().collect()
}

/// 箱内跨度最大的通道（0=R 1=G 2=B），并列取靠前的。
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

/// 箱子的加权平均色（四舍五入）。
fn box_average(entries: &[([u8; 3], u64)]) -> [u8; 3] {
    let total: u64 = entries.iter().map(|e| e.1).sum();
    let mut out = [0u8; 3];
    for (ch, slot) in out.iter_mut().enumerate() {
        let sum: u64 = entries.iter().map(|(c, w)| c[ch] as u64 * w).sum();
        *slot = ((sum + total / 2) / total) as u8;
    }
    out
}

/// 把每个不透明像素吸附到最近的色板色（RGB 欧氏距离，平方比较即可）。
/// alpha=0 → 整像素归零（RGB 无意义，归零利于压缩）；0<alpha<255 → 保留 alpha 只换 RGB。
fn quantize(img: &Img, palette: &[[u8; 3]]) -> Img {
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

/// 最近色：严格小于才更新 → 距离并列时取色板里靠前的（色板有序，结果确定）。
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

/// 最近邻缩放到高 h（保持宽高比，宽四舍五入、至少 1）。像素风缩放不插值，边缘保持硬。
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

/// 读项目官方色板 palette.json（格式 `{"colors": ["#rrggbb", ...]}`）。
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

/// 解码 PNG 成 RGBA8。模式对齐 vitric-render/src/assets.rs（错误同样带修法），
/// 但不设 2048 上限——这工具本来就是用来把超大图缩小的（--height）。
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

/// 编码 RGBA8 PNG。固定编码参数 → 同样的像素永远出同样的字节（确定性测试依赖这点）。
pub(crate) fn save_png(path: &Path, img: &Img) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("写入失败: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| format!("PNG 编码失败: {e}"))?;
    writer.write_image_data(&img.rgba).map_err(|e| format!("PNG 编码失败: {e}"))?;
    Ok(())
}
