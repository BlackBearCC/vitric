//! vitric assets --normals — batch-generate normal maps for project assets (paired naming
//! `xxx.png` → `xxx_n.png`, auto-enabled on the render side with zero config; see the
//! vitric-render module docs for the convention).
//!
//! Two generation paths, one loop:
//! - **Procedural** (default): height field = luminance × alpha × edge bevel (a gradient
//!   over [`BEVEL_RADIUS`] pixels from transparent pixels, raising the sprite outline), then
//!   Sobel gradient encoded as a normal. Pure arithmetic, fixed traversal order — same input
//!   always produces the same bytes (deterministic tests are locked).
//! - **--normals-ai**: Doubao Ark Seedream image-to-image (sprite nearest-neighbor upscaled
//!   onto a 512x512 #8080FF canvas, sent, then the 2048x2048 result downscaled back to original
//!   size). Vectors returned by the AI are not physically valid (varying lengths, z may point
//!   inward), so they must be sanitized (per-pixel renormalization, z clamped ≥0.1, alpha
//!   backfilled from the diffuse) before lighting. Network generation is inherently
//!   non-deterministic — for reproducibility, commit the generated _n files as assets instead
//!   of rerunning.
//!
//! Shared constraints:
//! - `_n` files are never treated as diffuse input (no `xxx_n_n.png`); existing pairs are
//!   skipped, not regenerated.
//! - Only new files are added, never modifying existing files — so the assets_original/
//!   backup safety gate is not needed.
//! - AI configuration comes only from environment variables (no secrets on disk):
//!   `ARK_API_KEY` (required, explicit error if missing),
//!   `VITRIC_NORMALS_MODEL` (default doubao-seedream-5-0-260128),
//!   `VITRIC_NORMALS_URL` (default Ark production endpoint; test stubs inject from here).

use std::path::Path;

use serde_json::Value;

use crate::assets_cmd::{collect_pngs, load_png, save_png, Img};

/// Strength coefficient mapping Sobel gradient to normal (larger = steeper relief).
/// Deliberately a constant, not a CLI parameter: the whole project must use one strength;
/// parameterizing it only invites the disharmony of "half the project at one steepness".
const SOBEL_STRENGTH: f64 = 2.0;

/// Edge bevel radius (pixels): height rises linearly over 1..=BEVEL_RADIUS steps from
/// transparent pixels. Outside the image counts as transparent — even a fully opaque
/// image gets an outline bevel.
const BEVEL_RADIUS: u32 = 3;

/// Ark image-to-image production endpoint (can be overridden by VITRIC_NORMALS_URL —
/// test stubs inject from here, never touching the real network).
const ARK_URL: &str = "https://ark.cn-beijing.volces.com/api/v3/images/generations";

/// Default model (can be overridden by VITRIC_NORMALS_MODEL).
const DEFAULT_MODEL: &str = "doubao-seedream-5-0-260128";

/// Canvas side length sent to the model / required return side length (return side = canvas ×4,
/// sampling more detail then downscaling back).
const CANVAS: u32 = 512;
const AI_OUT: u32 = 2048;

/// Neutral base color of normal maps #8080FF (decodes to ≈ flat normal (0,0,1)).
const NEUTRAL: [u8; 4] = [0x80, 0x80, 0xff, 255];

/// Image-to-image prompt. Hardcoded in the source: the prompt is part of the generator,
/// just like SOBEL_STRENGTH — no escape hatch; to change it, change the code so the whole
/// project stays consistent.
const PROMPT: &str = "Convert this sprite into a tangent-space normal map. \
    Keep the silhouette exactly identical, do not move or resize anything. \
    Flat areas must be the neutral normal color #8080FF; edges and raised details \
    tilt away from it (red = facing right, green = facing down). \
    Output only the normal map on a solid #8080FF background, no text, no labels.";

/// AI generation configuration (all from environment variables, no secrets on disk).
#[derive(Debug)]
pub struct AiConfig {
    pub url: String,
    pub key: String,
    pub model: String,
}

impl AiConfig {
    /// Read environment variables. Missing ARK_API_KEY is an explicit error — no silent
    /// fallback to the procedural path (if the user asked for AI, silently returning
    /// procedural output would make issues harder to diagnose).
    pub fn from_env() -> Result<AiConfig, String> {
        AiConfig::from_lookup(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    }

    /// Dependency-injected variant (for tests: doesn't touch process env vars,
    /// so tests can run in parallel).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<AiConfig, String> {
        let key = get("ARK_API_KEY").ok_or(
            "--normals-ai 需要环境变量 ARK_API_KEY（豆包 Ark 平台密钥）。\
             不想配密钥就用程序化路径：--normals",
        )?;
        Ok(AiConfig {
            url: get("VITRIC_NORMALS_URL").unwrap_or_else(|| ARK_URL.to_string()),
            key,
            model: get("VITRIC_NORMALS_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }
}

/// Generation report (stdout JSON).
#[derive(Debug)]
pub struct Report {
    pub mode: &'static str,
    /// _n files newly written this run (relative to assets/).
    pub generated: Vec<String>,
    /// Diffuse images skipped because an existing pair was present.
    pub skipped: Vec<String>,
}

impl Report {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "mode": self.mode,
            "generated": self.generated,
            "skipped_existing": self.skipped,
        })
    }
}

/// Main flow: scan assets/ → for each non-_n PNG without an _n pair, generate `xxx_n.png`.
/// `ai = None` takes the procedural path; `Some(cfg)` takes the Ark image-to-image path.
pub fn generate(project_dir: &Path, ai: Option<&AiConfig>) -> Result<Report, String> {
    let assets_dir = project_dir.join("assets");
    if !assets_dir.is_dir() {
        return Err(format!(
            "{} 不存在。提示：vitric assets 处理项目的 assets/ 目录，先把图放进去",
            assets_dir.display()
        ));
    }
    let rels = collect_pngs(&assets_dir)?;
    if rels.is_empty() {
        return Err(format!(
            "{} 里没有 PNG。提示：先把图放进 assets/ 再生成法线",
            assets_dir.display()
        ));
    }
    let existing: std::collections::BTreeSet<&str> = rels.iter().map(|s| s.as_str()).collect();
    let mut generated = Vec::new();
    let mut skipped = Vec::new();
    for rel in &rels {
        // _n files are never treated as diffuse input (normals don't have their own normals)
        let Some(pair) = vitric_render::normal_map_name(rel) else { continue };
        if existing.contains(pair.as_str()) {
            skipped.push(rel.clone());
            continue;
        }
        let img = load_png(&assets_dir.join(rel)).map_err(|e| format!("素材 {rel}: {e}"))?;
        let normal = match ai {
            None => procedural_normal(&img),
            Some(cfg) => ai_normal(cfg, &img).map_err(|e| format!("素材 {rel}: {e}"))?,
        };
        save_png(&assets_dir.join(&pair), &normal).map_err(|e| format!("素材 {pair}: {e}"))?;
        generated.push(pair);
    }
    Ok(Report { mode: if ai.is_some() { "ai" } else { "procedural" }, generated, skipped })
}

// ---------------------------------------------------------------------------
// Procedural path: height field (luminance × alpha × edge bevel) → Sobel → encode
// ---------------------------------------------------------------------------

/// Procedurally generate a normal map (deterministic: pure f64 arithmetic, fixed traversal order).
fn procedural_normal(img: &Img) -> Img {
    let (w, h) = (img.width as usize, img.height as usize);
    let height_map = height_field(img);
    let mut rgba = Vec::with_capacity(w * h * 4);
    // Sobel kernel (x direction; y direction is the transpose). Out-of-bounds sampling
    // clamps to the edge — same semantics as the render-side clamp.
    let sample = |x: i64, y: i64| -> f64 {
        let xc = x.clamp(0, w as i64 - 1) as usize;
        let yc = y.clamp(0, h as i64 - 1) as usize;
        height_map[yc * w + xc]
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let a = img.rgba[(y as usize * w + x as usize) * 4 + 3];
            if a == 0 {
                // Transparent pixels have no surface: write neutral color + alpha 0
                // (renderer never samples them; uniform encoding helps compression).
                rgba.extend_from_slice(&[NEUTRAL[0], NEUTRAL[1], NEUTRAL[2], 0]);
                continue;
            }
            let gx = (sample(x + 1, y - 1) + 2.0 * sample(x + 1, y) + sample(x + 1, y + 1))
                - (sample(x - 1, y - 1) + 2.0 * sample(x - 1, y) + sample(x - 1, y + 1));
            let gy = (sample(x - 1, y + 1) + 2.0 * sample(x, y + 1) + sample(x + 1, y + 1))
                - (sample(x - 1, y - 1) + 2.0 * sample(x, y - 1) + sample(x + 1, y - 1));
            // Height rising toward +x → surface tilts left (nx negative); same for y
            // (image y downward = screen y downward).
            let nx = -gx * SOBEL_STRENGTH;
            let ny = -gy * SOBEL_STRENGTH;
            let len = (nx * nx + ny * ny + 1.0).sqrt();
            rgba.extend_from_slice(&[
                encode(nx / len),
                encode(ny / len),
                encode(1.0 / len),
                a, // alpha follows the diffuse (semi-transparent edges keep consistent shape info)
            ]);
        }
    }
    Img { width: img.width, height: img.height, rgba }
}

/// Height field: luminance (Rec.601) × alpha × edge bevel.
/// Bevel = min(steps to a transparent pixel, BEVEL_RADIUS) / BEVEL_RADIUS — a linear rise
/// around the outline. Distance uses 4-connected BFS (integer step count, no floating-point
/// sqrt — determinism for free); **outside the image counts as transparent**: a fully opaque
/// image still gets a border bevel (if tile blocks need seamless tiling, don't use the
/// procedural path; see art-pipeline).
fn height_field(img: &Img) -> Vec<f64> {
    let (w, h) = (img.width as usize, img.height as usize);
    // BFS distance to transparency: transparent pixels have distance 0; outside the image
    // counts as transparent → border pixels start at 1.
    let mut dist = vec![u32::MAX; w * h];
    let mut queue = std::collections::VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if img.rgba[i * 4 + 3] == 0 {
                dist[i] = 0;
                queue.push_back((x, y));
            } else if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                dist[i] = 1; // adjacent to outside the image (outside = transparent)
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        let d = dist[y * w + x];
        if d >= BEVEL_RADIUS {
            continue; // distances beyond the bevel radius don't affect the result; no need to fully diffuse
        }
        let neighbors =
            [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)];
        for (nx, ny) in neighbors {
            if nx < w && ny < h && dist[ny * w + nx] > d + 1 {
                dist[ny * w + nx] = d + 1;
                queue.push_back((nx, ny));
            }
        }
    }
    let mut out = vec![0.0; w * h];
    for (i, slot) in out.iter_mut().enumerate() {
        let px = &img.rgba[i * 4..i * 4 + 4];
        if px[3] == 0 {
            continue; // transparent = height 0
        }
        let lum = (0.299 * px[0] as f64 + 0.587 * px[1] as f64 + 0.114 * px[2] as f64) / 255.0;
        let alpha = px[3] as f64 / 255.0;
        let bevel = dist[i].min(BEVEL_RADIUS) as f64 / BEVEL_RADIUS as f64;
        *slot = lum * alpha * bevel;
    }
    out
}

/// [-1,1] → 0..255 (rounded; 0 → 128, the neutral component).
fn encode(v: f64) -> u8 {
    ((v * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------------------
// AI path: lay out canvas → Ark image-to-image → downscale to original size → sanitize
// ---------------------------------------------------------------------------

/// AI-generate a normal map (lay out canvas → request → download → downscale → sanitize).
/// Network/format errors all propagate explicitly (with endpoint and echoed response),
/// never silently switching paths.
fn ai_normal(cfg: &AiConfig, img: &Img) -> Result<Img, String> {
    let canvas = paste_on_canvas(img);
    let png = encode_png_bytes(&canvas)?;
    let data_url = format!("data:image/png;base64,{}", base64(&png));
    let result_png = call_ark(cfg, &data_url)?;
    let result = decode_png_bytes(&result_png)?;
    if result.width != AI_OUT || result.height != AI_OUT {
        return Err(format!(
            "Ark 返回的图是 {}x{}，要求的是 {AI_OUT}x{AI_OUT}——尺寸对不上没法映射回精灵，\
             检查模型 {} 是否支持 size 参数",
            result.width, result.height, cfg.model
        ));
    }
    Ok(sanitize(&downscale_from_canvas(&result, img), img))
}

/// Nearest-neighbor scale the sprite onto the center of a CANVAS×CANVAS #8080FF canvas
/// (preserving aspect ratio). The neutral base color means the canvas itself is a valid
/// "flat" normal, so the model doesn't have to guess what the background should be.
fn paste_on_canvas(img: &Img) -> Img {
    let (tw, th, ox, oy) = canvas_layout(img);
    let mut rgba = NEUTRAL.repeat((CANVAS * CANVAS) as usize);
    for y in 0..th {
        let sy = (y as u64 * img.height as u64 / th as u64) as usize;
        for x in 0..tw {
            let sx = (x as u64 * img.width as u64 / tw as u64) as usize;
            let s = (sy * img.width as usize + sx) * 4;
            if img.rgba[s + 3] == 0 {
                continue; // transparent pixels keep the canvas neutral color
            }
            let d = (((oy + y) * CANVAS + ox + x) * 4) as usize;
            rgba[d..d + 3].copy_from_slice(&img.rgba[s..s + 3]);
        }
    }
    Img { width: CANVAS, height: CANVAS, rgba }
}

/// Sprite layout on the canvas: scaled dimensions (aspect-preserving, at least 1px) + centered offset.
fn canvas_layout(img: &Img) -> (u32, u32, u32, u32) {
    let scale = (CANVAS as f64 / img.width as f64).min(CANVAS as f64 / img.height as f64);
    let tw = ((img.width as f64 * scale) as u32).clamp(1, CANVAS);
    let th = ((img.height as f64 * scale) as u32).clamp(1, CANVAS);
    ((tw), (th), (CANVAS - tw) / 2, (CANVAS - th) / 2)
}

/// Nearest-neighbor downscale the sprite region out of the 2048 result back to original size
/// (result image = canvas × (AI_OUT/CANVAS)).
fn downscale_from_canvas(result: &Img, sprite: &Img) -> Img {
    let (tw, th, ox, oy) = canvas_layout(sprite);
    let f = (AI_OUT / CANVAS) as f64; // upscale factor from canvas to result image
    let mut rgba = Vec::with_capacity((sprite.width * sprite.height * 4) as usize);
    for y in 0..sprite.height {
        // Sprite pixel center → canvas coordinate → result image coordinate (nearest-neighbor throughout)
        let cy = oy as f64 + (y as f64 + 0.5) * th as f64 / sprite.height as f64;
        let ry = ((cy * f) as i64).clamp(0, AI_OUT as i64 - 1) as usize;
        for x in 0..sprite.width {
            let cx = ox as f64 + (x as f64 + 0.5) * tw as f64 / sprite.width as f64;
            let rx = ((cx * f) as i64).clamp(0, AI_OUT as i64 - 1) as usize;
            let s = (ry * AI_OUT as usize + rx) * 4;
            rgba.extend_from_slice(&result.rgba[s..s + 4]);
        }
    }
    Img { width: sprite.width, height: sprite.height, rgba }
}

/// Sanitize: AI-produced vectors are not physically valid (varying lengths, z may point
/// inward), so per-pixel renormalize, clamp z ≥ 0.1 (take abs first, then clamp, then
/// renormalize — still guarantees z ≥ 0.1 after renormalization by rescaling xy precisely),
/// and backfill alpha from the diffuse (silhouette must match pixel-for-pixel; the AI's
/// edge feathering is not trustworthy).
fn sanitize(raw: &Img, diffuse: &Img) -> Img {
    let mut rgba = Vec::with_capacity(raw.rgba.len());
    for (npx, dpx) in raw.rgba.chunks_exact(4).zip(diffuse.rgba.chunks_exact(4)) {
        if dpx[3] == 0 {
            rgba.extend_from_slice(&[NEUTRAL[0], NEUTRAL[1], NEUTRAL[2], 0]);
            continue;
        }
        let nx = npx[0] as f64 / 255.0 * 2.0 - 1.0;
        let ny = npx[1] as f64 / 255.0 * 2.0 - 1.0;
        let nz = (npx[2] as f64 / 255.0 * 2.0 - 1.0).abs();
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        let (mut x, mut y, mut z) =
            if len < 1e-9 { (0.0, 0.0, 1.0) } else { (nx / len, ny / len, nz / len) };
        if z < 0.1 {
            // Renormalization may push z back below 0.1: pin z=0.1 and rescale xy back
            // onto the unit sphere.
            z = 0.1;
            let xy = (x * x + y * y).sqrt();
            let s = (1.0 - z * z).sqrt() / xy; // when z<0.1, xy≈1 > 0, division is safe
            x *= s;
            y *= s;
        }
        rgba.extend_from_slice(&[encode(x), encode(y), encode(z), dpx[3]]);
    }
    Img { width: raw.width, height: raw.height, rgba }
}

/// Call Ark image-to-image and return the result image's PNG bytes. Request/parsing pattern
/// mirrors llm.rs (ureq synchronous blocking, errors carry endpoint and truncated echo).
/// response_format=url: first take the URL from the JSON, then download it once more.
fn call_ark(cfg: &AiConfig, data_url: &str) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({
        "model": cfg.model,
        "prompt": PROMPT,
        "image": data_url,
        "size": format!("{AI_OUT}x{AI_OUT}"),
        "response_format": "url",
        "watermark": false,
    });
    let mut resp = ureq::post(&cfg.url)
        .header("Authorization", &format!("Bearer {}", cfg.key))
        .send_json(&body)
        .map_err(|e| format!("Ark 请求 {} 失败: {e}", cfg.url))?;
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Ark 响应读取失败: {e}"))?;
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("Ark 响应不是合法 JSON: {e}"))?;
    let url = v
        .get("data")
        .and_then(|d| d.get(0))
        .and_then(|d| d.get("url"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| {
            let shown: String = text.chars().take(200).collect();
            format!("Ark 响应缺 data[0].url（response_format=url 的约定字段），实际响应: {shown}")
        })?;
    let mut img_resp =
        ureq::get(url).call().map_err(|e| format!("下载 Ark 结果图 {url} 失败: {e}"))?;
    img_resp
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024) // a 2048² PNG may exceed ureq's default 10MB limit
        .read_to_vec()
        .map_err(|e| format!("Ark 结果图读取失败: {e}"))
}

/// PNG bytes → Img (in-memory; downloaded results are not written to temp files).
fn decode_png_bytes(bytes: &[u8]) -> Result<Img, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
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
        other => return Err(format!("Ark 返回的 PNG 颜色类型 {other:?} 不支持（要 RGB/RGBA）")),
    };
    Ok(Img { width: info.width, height: info.height, rgba })
}

/// Img → PNG bytes (in-memory; used for sending requests).
fn encode_png_bytes(img: &Img) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| format!("PNG 编码失败: {e}"))?;
        writer.write_image_data(&img.rgba).map_err(|e| format!("PNG 编码失败: {e}"))?;
    }
    Ok(out)
}

/// Standard base64 (with padding). 20 hand-written lines aren't worth a new dependency for.
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a temp project: several (name, width, height, RGBA) under assets/.
    fn project_with(tag: &str, files: &[(&str, u32, u32, &[u8])]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vitric-norm-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, w, h, rgba) in files {
            let path = dir.join("assets").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = png::Encoder::new(file, *w, *h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(rgba).unwrap();
        }
        dir
    }

    /// 12x12 transparent canvas with an 8x8 gray square in the center (2..10) — the standard
    /// test sprite for the procedural path.
    fn square_sprite() -> Vec<u8> {
        let mut rgba = vec![0u8; 12 * 12 * 4];
        for y in 2..10 {
            for x in 2..10 {
                let i = (y * 12 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[180, 180, 180, 255]);
            }
        }
        rgba
    }

    fn px(img: &Img, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]
    }

    #[test]
    fn procedural_edge_normals_point_outward() {
        let sprite = square_sprite();
        let dir = project_with("edge", &[("hero.png", 12, 12, &sprite)]);
        let report = generate(&dir, None).unwrap();
        assert_eq!(report.mode, "procedural");
        assert_eq!(report.generated, vec!["hero_n.png"]);
        let n = load_png(&dir.join("assets/hero_n.png")).unwrap();
        assert_eq!((n.width, n.height), (12, 12));
        // Square 2..10: edge-pixel normals point outward (screen space: x right, y down)
        let left = px(&n, 2, 6);
        let right = px(&n, 9, 6);
        let top = px(&n, 6, 2);
        let bottom = px(&n, 6, 9);
        assert!(left[0] < 128, "左缘法线朝左（红 < 128）: {left:?}");
        assert!(right[0] > 128, "右缘法线朝右（红 > 128）: {right:?}");
        assert!(top[1] < 128, "上缘法线朝上（绿 < 128，y 向下）: {top:?}");
        assert!(bottom[1] > 128, "下缘法线朝下（绿 > 128）: {bottom:?}");
        // Central flat region: neutral normal, alpha follows the diffuse
        assert_eq!(px(&n, 6, 6), [128, 128, 255, 255], "倒角半径外的平坦区是中性法线");
        // Transparent region: neutral color + alpha 0
        assert_eq!(px(&n, 0, 0), [128, 128, 255, 0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn procedural_is_deterministic_across_runs() {
        let sprite = square_sprite();
        let a = project_with("det-a", &[("hero.png", 12, 12, &sprite)]);
        let b = project_with("det-b", &[("hero.png", 12, 12, &sprite)]);
        generate(&a, None).unwrap();
        generate(&b, None).unwrap();
        assert_eq!(
            std::fs::read(a.join("assets/hero_n.png")).unwrap(),
            std::fs::read(b.join("assets/hero_n.png")).unwrap(),
            "同输入两次生成必须逐字节相同"
        );
        std::fs::remove_dir_all(&a).unwrap();
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn existing_pairs_are_skipped_and_n_files_never_become_inputs() {
        let sprite = square_sprite();
        let marker = [1u8, 2, 3, 255].repeat(4); // 2x2 fake normal (content arbitrary, only checking if it changes)
        let dir = project_with(
            "skip",
            &[("hero.png", 12, 12, &sprite), ("hero_n.png", 2, 2, &marker)],
        );
        let before = std::fs::read(dir.join("assets/hero_n.png")).unwrap();
        let report = generate(&dir, None).unwrap();
        assert!(report.generated.is_empty(), "有配对的不重生成: {report:?}");
        assert_eq!(report.skipped, vec!["hero.png"]);
        assert_eq!(before, std::fs::read(dir.join("assets/hero_n.png")).unwrap(), "既有 _n 一个字节不动");
        // _n files are never treated as diffuse input: no hero_n_n.png exists
        assert!(!dir.join("assets/hero_n_n.png").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_ark_key_is_an_explicit_error() {
        let err = AiConfig::from_lookup(|_| None).unwrap_err();
        assert!(err.contains("ARK_API_KEY"), "{err}");
        // With everything set, default endpoint/model is used
        let cfg = AiConfig::from_lookup(|k| (k == "ARK_API_KEY").then(|| "sk-x".to_string()))
            .unwrap();
        assert_eq!(cfg.url, ARK_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        // Model/endpoint can be overridden by environment variables
        let cfg = AiConfig::from_lookup(|k| Some(format!("v-{k}"))).unwrap();
        assert_eq!(cfg.model, "v-VITRIC_NORMALS_MODEL");
        assert_eq!(cfg.url, "v-VITRIC_NORMALS_URL");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ai_path_sanitizes_against_local_stub() {
        // Stub server: ① validate request shape and return data[0].url; ② serve a 2048²
        // "bad normal" via that url (vectors not normalized and z pointing inward:
        // (10,200,30) → nz<0). After generation, assert all pixels are sanitized.
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr();
        let url = format!("http://{addr}/api/v3/images/generations");
        let handle = std::thread::spawn(move || {
            // First request: image-to-image
            let mut req = server.recv().unwrap();
            assert_eq!(req.method(), &tiny_http::Method::Post);
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body).unwrap();
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["model"], "stub-model");
            assert_eq!(v["size"], "2048x2048");
            assert_eq!(v["response_format"], "url");
            assert_eq!(v["watermark"], false);
            assert!(v["prompt"].as_str().unwrap().contains("normal map"), "{}", v["prompt"]);
            assert!(
                v["image"].as_str().unwrap().starts_with("data:image/png;base64,"),
                "image 必须是 data URL"
            );
            let resp = serde_json::json!({
                "data": [{"url": format!("http://{addr}/result.png")}]
            });
            req.respond(
                tiny_http::Response::from_string(resp.to_string()).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                ),
            )
            .unwrap();
            // Second request: download the result image (solid color "bad normal",
            // PNG row-filter compresses very small)
            let bad = Img {
                width: AI_OUT,
                height: AI_OUT,
                rgba: [10u8, 200, 30, 255].repeat((AI_OUT * AI_OUT) as usize),
            };
            let png_bytes = encode_png_bytes(&bad).unwrap();
            let req = server.recv().unwrap();
            assert_eq!(req.url(), "/result.png");
            req.respond(tiny_http::Response::from_data(png_bytes)).unwrap();
        });

        // Sprite: 4x4, left half opaque right half transparent — alpha backfill must show a difference
        let mut sprite = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..2 {
                let i = (y * 4 + x) * 4;
                sprite[i..i + 4].copy_from_slice(&[200, 100, 50, 255]);
            }
        }
        let dir = project_with("ai", &[("hero.png", 4, 4, &sprite)]);
        let cfg = AiConfig { url, key: "k".to_string(), model: "stub-model".to_string() };
        let report = generate(&dir, Some(&cfg)).unwrap();
        assert_eq!(report.mode, "ai");
        assert_eq!(report.generated, vec!["hero_n.png"]);
        let n = load_png(&dir.join("assets/hero_n.png")).unwrap();
        for y in 0..4 {
            for x in 0..4 {
                let p = px(&n, x, y);
                let dpx_a = sprite[((y * 4 + x) * 4 + 3) as usize];
                assert_eq!(p[3], dpx_a, "alpha 必须回填漫反射的 ({x},{y})");
                if dpx_a == 0 {
                    assert_eq!([p[0], p[1], p[2]], [128, 128, 255], "透明区写中性色");
                    continue;
                }
                // After sanitization: unit length (within encoding quantization error) and z ≥ 0.1
                let nx = p[0] as f64 / 255.0 * 2.0 - 1.0;
                let ny = p[1] as f64 / 255.0 * 2.0 - 1.0;
                let nz = p[2] as f64 / 255.0 * 2.0 - 1.0;
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                assert!((len - 1.0).abs() < 0.02, "({x},{y}) 长度 {len} 不是单位向量: {p:?}");
                assert!(nz >= 0.1 - 0.01, "({x},{y}) z={nz} 低于 0.1: {p:?}");
            }
        }
        handle.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sanitize_math_holds_at_extremes() {
        // Pure xy vector (z=0 encodes 128... i.e. decodes to 0.0039) → clamp to z=0.1,
        // rescale xy back onto the sphere
        let raw = Img { width: 1, height: 1, rgba: vec![255, 128, 128, 255] };
        let diffuse = Img { width: 1, height: 1, rgba: vec![255, 255, 255, 255] };
        let out = sanitize(&raw, &diffuse);
        let nz = out.rgba[2] as f64 / 255.0 * 2.0 - 1.0;
        assert!(nz >= 0.1 - 0.01, "z 夹到 ≥0.1: {nz}");
        // (128,128,128) does not decode to exact zero (±1/255 per channel): normal-path
        // normalization produces a diagonal vector.
        // Locked bytes: (0.0039,0.0039,0.0039) → normalized (0.577,0.577,0.577) → encoded 201
        let raw = Img { width: 1, height: 1, rgba: vec![128, 128, 128, 255] };
        let out = sanitize(&raw, &diffuse);
        assert_eq!(&out.rgba[..3], &[201, 201, 201], "近零向量归一化成对角单位向量");
    }
}
