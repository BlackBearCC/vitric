//! vitric-render — 2D 光栅化。
//!
//! v0 是纯 CPU 渲染器：world → RGBA 像素 → PNG。
//! 看似保守，实则是闭环的关键一环：**截图不需要 GPU、不需要窗口、
//! 不需要图形会话**——agent 在任何无头环境都能「亲眼看到」游戏画面，
//! 而且同一世界状态渲出的像素逐字节相同（截图也可以进断言）。
//! GPU（wgpu）走的是同一个组件约定，后续替换呈现层不动游戏数据。
//!
//! 组件约定：
//! - `Sprite`  {"w": 数字, "h": 数字, "color": "#rrggbb"} — 有它才会被画
//! - `Position` {"x", "y"} — 世界坐标，y 向上
//! - `Camera` {"x", "y", "scale"} — 可选；取第一个，没有则原点、8 像素/单位
//! - `Shake` {"amplitude", "decay"} — 挂在相机实体上的屏幕抖动；amplitude > 0 时
//!   取景叠加确定性伪随机偏移（(tick, amplitude) 的纯函数，见 [`shake_offset`]）。
//!   偏移只作用于画面（render_world / GPU 路径 / 选中描边）——describe / pick /
//!   screen_to_world 读不抖的相机：语义观察和点选对的是世界本体，不是抖动后的画面
//! - `Text` {"content", "size", "color"} — 屏上文字（内嵌 8x8 点阵，ASCII），
//!   每字符 size×size 世界单位、整串居中于 Position，画在精灵之上

mod assets;

pub use assets::{Assets, Image};

use serde_json::Value;

use vitric_ecs::World;

/// 渲染一帧：返回 RGBA8 像素（行优先，左上原点）。
/// `tick` 只喂给屏幕抖动（[`camera_of`]）——同一世界同一 tick 渲出的字节逐位相同。
pub fn render_world(
    world: &World,
    width: u32,
    height: u32,
    assets: &Assets,
    tick: u64,
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(format!("分辨率 {width}x{height} 不合法（1..=4096）"));
    }
    let (cam_x, cam_y, scale) = camera_of(world, tick)?;

    let mut buf = vec![0u8; (width * height * 4) as usize];
    // 背景：深灰蓝，区别于纯黑（纯黑常被误判为「没渲出来」）
    fill(&mut buf, [24, 26, 33, 255]);

    // 按实体序绘制（确定性；后画的盖前画的）
    for id in world.query(&["Position", "Sprite"]) {
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;

        // 世界 → 屏幕（y 翻转，相机居中）
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;
        let x0 = (cx - half_w).floor().max(0.0) as i64;
        let x1 = (cx + half_w).ceil().min(width as f64) as i64;
        let y0 = (cy - half_h).floor().max(0.0) as i64;
        let y1 = (cy + half_h).ceil().min(height as f64) as i64;

        let image_name = world
            .get_field(id, "Sprite.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        if image_name.is_empty() {
            // 纯色块
            let color = world
                .get_field(id, "Sprite.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Sprite.color: {e}"))?;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&rgba);
                }
            }
        } else {
            // 贴图：最近邻缩放 + alpha 混合。图不存在直接报错（不画占位符）。
            let img = assets.image(&image_name).ok_or_else(|| {
                format!(
                    "实体 {id} 的 Sprite.image {image_name:?} 不在素材仓库里。\
                     现有素材: [{}]。提示：图放进项目 assets/ 目录，路径相对 assets/ 写",
                    assets.names().join(", ")
                )
            })?;
            let span_x = 2.0 * half_w;
            let span_y = 2.0 * half_h;
            for y in y0..y1 {
                for x in x0..x1 {
                    let u = ((x as f64 + 0.5) - (cx - half_w)) / span_x;
                    let v = ((y as f64 + 0.5) - (cy - half_h)) / span_y;
                    let sx = ((u * img.width as f64) as i64).clamp(0, img.width as i64 - 1) as usize;
                    let sy = ((v * img.height as f64) as i64).clamp(0, img.height as i64 - 1) as usize;
                    let s = (sy * img.width as usize + sx) * 4;
                    let src = &img.rgba[s..s + 4];
                    let a = src[3] as u32;
                    if a == 0 {
                        continue;
                    }
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    let dst = &mut buf[i..i + 4];
                    for c in 0..3 {
                        dst[c] = ((src[c] as u32 * a + dst[c] as u32 * (255 - a)) / 255) as u8;
                    }
                    dst[3] = 255;
                }
            }
        }
    }

    draw_texts(world, &mut buf, width, height, (cam_x, cam_y, scale))?;
    Ok(buf)
}

/// 文字：`Text` {"content","size","color"} + `Position`，内嵌 8x8 点阵字体。
/// 每个字符占 size×size 世界单位，整串以 Position 为中心，画在所有精灵之上。
/// 字体只覆盖 ASCII（分数/提示/调试足够），其他字符画实心方块占位。
fn draw_texts(
    world: &World,
    buf: &mut [u8],
    width: u32,
    height: u32,
    (cam_x, cam_y, scale): (f64, f64, f64),
) -> Result<(), String> {
    for id in world.query(&["Position", "Text"]) {
        let content = world
            .get_field(id, "Text.content")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let size = num(world, id, "Text.size")?;
        if size <= 0.0 {
            continue;
        }
        let color = world
            .get_field(id, "Text.color")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "#ffffff".to_string());
        let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Text.color: {e}"))?;
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        // screen=true: HUD 锚定——Position 解释为相对屏幕中心的偏移,不随相机走
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let chars: Vec<char> = content.chars().collect();
        let n = chars.len();
        let (cx, cy) = if screen_anchored {
            ((width as f64) / 2.0 + px * scale, (height as f64) / 2.0 - py * scale)
        } else {
            ((width as f64) / 2.0 + (px - cam_x) * scale, (height as f64) / 2.0 - (py - cam_y) * scale)
        };
        let half_w = n as f64 * size * scale / 2.0;
        let half_h = size * scale / 2.0;
        let x0 = (cx - half_w).floor().max(0.0) as i64;
        let x1 = (cx + half_w).ceil().min(width as f64) as i64;
        let y0 = (cy - half_h).floor().max(0.0) as i64;
        let y1 = (cy + half_h).ceil().min(height as f64) as i64;
        let span_x = 2.0 * half_w;
        let span_y = 2.0 * half_h;
        for y in y0..y1 {
            for x in x0..x1 {
                let u = ((x as f64 + 0.5) - (cx - half_w)) / span_x; // 0..1 横跨整串
                let v = ((y as f64 + 0.5) - (cy - half_h)) / span_y; // 0..1 纵跨一字
                let idx = ((u * n as f64) as usize).min(n - 1);
                let col = (((u * n as f64 - idx as f64) * 8.0) as usize).min(7);
                let row = ((v * 8.0) as usize).min(7);
                if glyph_of(chars[idx])[row] & (1 << col) != 0 {
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&rgba);
                }
            }
        }
    }
    Ok(())
}

/// 字符 → 8x8 点阵（每字节一行，低位在左）。非 ASCII 用实心方块占位。
fn glyph_of(c: char) -> [u8; 8] {
    let cp = c as usize;
    if cp < 128 {
        font8x8::legacy::BASIC_LEGACY[cp]
    } else {
        [0xff; 8]
    }
}

/// 屏幕像素 → 世界坐标（检查器拖拽、点选用）。
/// 用不抖的相机：点选/拖拽对的是世界本体，抖动只是几帧的视觉装饰。
pub fn screen_to_world(
    world: &World,
    width: u32,
    height: u32,
    px: f64,
    py: f64,
) -> Result<(f64, f64), String> {
    let (cam_x, cam_y, scale) = camera_base(world)?;
    Ok((
        cam_x + (px - width as f64 / 2.0) / scale,
        cam_y - (py - height as f64 / 2.0) / scale,
    ))
}

/// 点选拾取：返回屏幕坐标 (px,py) 命中的最上层实体（绘制顺序靠后者优先）。
pub fn pick(
    world: &World,
    width: u32,
    height: u32,
    px: f64,
    py: f64,
) -> Result<Option<vitric_ecs::EntityId>, String> {
    let (wx, wy) = screen_to_world(world, width, height, px, py)?;
    let ids = world.query(&["Position", "Sprite"]);
    // 倒序：后画的盖在上面，优先命中
    for &id in ids.iter().rev() {
        let x = num(world, id, "Position.x")?;
        let y = num(world, id, "Position.y")?;
        let w = num(world, id, "Sprite.w")?;
        let h = num(world, id, "Sprite.h")?;
        if (wx - x).abs() * 2.0 <= w && (wy - y).abs() * 2.0 <= h {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// 在已渲染的帧上给实体画选中描边（检查器高亮，青色 2px）。
/// `tick` 必须和这帧 `render_world` 用的同一个——描边要跟着抖动的画面走，不然抖屏时错位。
pub fn draw_selection_outline(
    buf: &mut [u8],
    world: &World,
    width: u32,
    height: u32,
    selected: vitric_ecs::EntityId,
    tick: u64,
) -> Result<(), String> {
    if !world.is_alive(selected) || !world.has_component(selected, "Sprite") {
        return Ok(()); // 选中的实体没了/不可见，描边静默跳过（选中态本身由上层管理）
    }
    let (cam_x, cam_y, scale) = camera_of(world, tick)?;
    let x = num(world, selected, "Position.x")?;
    let y = num(world, selected, "Position.y")?;
    let w = num(world, selected, "Sprite.w")?;
    let h = num(world, selected, "Sprite.h")?;
    let cx = (width as f64) / 2.0 + (x - cam_x) * scale;
    let cy = (height as f64) / 2.0 - (y - cam_y) * scale;
    let half_w = w * scale / 2.0 + 2.0;
    let half_h = h * scale / 2.0 + 2.0;
    let x0 = (cx - half_w).floor().max(0.0) as i64;
    let x1 = (cx + half_w).ceil().min(width as f64) as i64 - 1;
    let y0 = (cy - half_h).floor().max(0.0) as i64;
    let y1 = (cy + half_h).ceil().min(height as f64) as i64 - 1;
    const TEAL: [u8; 4] = [39, 192, 168, 255];
    let mut put = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
            let i = ((y as u32 * width + x as u32) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&TEAL);
        }
    };
    for t in 0..2i64 {
        for x in x0..=x1 {
            put(x, y0 + t);
            put(x, y1 - t);
        }
        for y in y0..=y1 {
            put(x0 + t, y);
            put(x1 - t, y);
        }
    }
    Ok(())
}

/// RGBA 像素 → PNG 字节。
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| format!("PNG 编码失败: {e}"))?;
        writer.write_image_data(rgba).map_err(|e| format!("PNG 编码失败: {e}"))?;
    }
    Ok(out)
}

/// 一步到位：world → PNG。
pub fn screenshot_png(
    world: &World,
    width: u32,
    height: u32,
    assets: &Assets,
    tick: u64,
) -> Result<Vec<u8>, String> {
    let rgba = render_world(world, width, height, assets, tick)?;
    encode_png(&rgba, width, height)
}

/// 语义观察：把"画面上有什么"翻译成 LLM 能精确读懂的结构化描述。
///
/// 这是 agent 的**主观察通道**——比让模型看像素更精准：
/// 坐标是确切数字、方位是九宫格词、遮挡是明确的实体对、
/// 视野外的东西有方向和距离。截图（screenshot）退居兜底验证。
pub fn describe_world(world: &World, width: u32, height: u32) -> Result<serde_json::Value, String> {
    use serde_json::json;

    if width == 0 || height == 0 {
        return Err(format!("分辨率 {width}x{height} 不合法"));
    }
    // 语义观察用不抖的相机：agent 断言的坐标不该被几帧视觉抖动晃花
    let (cam_x, cam_y, scale) = camera_base(world)?;
    let half_w_units = width as f64 / scale / 2.0;
    let half_h_units = height as f64 / scale / 2.0;

    let mut visible = Vec::new();
    let mut offscreen = Vec::new();
    let mut rects: Vec<(String, f64, f64, f64, f64)> = Vec::new(); // (id, x, y, w, h) 世界坐标

    for id in world.query(&["Position", "Sprite"]) {
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        let color = world
            .get_field(id, "Sprite.color")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "#ffffff".to_string());
        let image = world
            .get_field(id, "Sprite.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let name = world.name_of(id).map(String::from);

        let dx = px - cam_x;
        let dy = py - cam_y;
        let on_screen = dx.abs() - sw / 2.0 < half_w_units && dy.abs() - sh / 2.0 < half_h_units;

        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), json!(id.to_string()));
        if let Some(n) = &name {
            entry.insert("name".into(), json!(n));
        }
        entry.insert("world".into(), json!({"x": px, "y": py}));
        let mut sprite = json!({"w": sw, "h": sh, "color": color});
        if !image.is_empty() {
            sprite["image"] = json!(image);
        }
        entry.insert("sprite".into(), sprite);

        if on_screen {
            let sx = width as f64 / 2.0 + dx * scale;
            let sy = height as f64 / 2.0 - dy * scale;
            entry.insert("screen_px".into(), json!({"x": sx.round(), "y": sy.round()}));
            entry.insert(
                "region".into(),
                json!(region_word(sx / width as f64, sy / height as f64)),
            );
            rects.push((id.to_string(), px, py, sw, sh));
            visible.push(serde_json::Value::Object(entry));
        } else {
            let direction = direction_word(dx, dy);
            entry.insert("direction".into(), json!(direction));
            entry.insert(
                "distance_units".into(),
                json!((dx.powi(2) + dy.powi(2)).sqrt().round()),
            );
            offscreen.push(serde_json::Value::Object(entry));
        }
    }

    // 屏上文字：内容本身就是语义，agent 不用 OCR 截图
    let mut texts = Vec::new();
    for id in world.query(&["Position", "Text"]) {
        let content = world
            .get_field(id, "Text.content")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (dx, dy) = if screen_anchored { (px, py) } else { (px - cam_x, py - cam_y) };
        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), json!(id.to_string()));
        if let Some(n) = world.name_of(id) {
            entry.insert("name".into(), json!(n));
        }
        entry.insert("content".into(), json!(content));
        entry.insert("world".into(), json!({"x": px, "y": py}));
        if dx.abs() < half_w_units && dy.abs() < half_h_units {
            let sx = width as f64 / 2.0 + dx * scale;
            let sy = height as f64 / 2.0 - dy * scale;
            entry.insert("region".into(), json!(region_word(sx / width as f64, sy / height as f64)));
        } else {
            entry.insert("region".into(), json!("视野外"));
        }
        texts.push(serde_json::Value::Object(entry));
    }

    // 视觉重叠（画面上谁压着谁）
    let mut overlaps = Vec::new();
    for i in 0..rects.len() {
        for j in (i + 1)..rects.len() {
            let (ref a, ax, ay, aw, ah) = rects[i];
            let (ref b, bx, by, bw, bh) = rects[j];
            if (ax - bx).abs() * 2.0 < aw + bw && (ay - by).abs() * 2.0 < ah + bh {
                overlaps.push(json!([a, b]));
            }
        }
    }

    // 一段给 LLM 直接读的中文摘要（结构化字段的浓缩版）
    let mut lines = vec![format!(
        "相机({cam_x},{cam_y}) 缩放{scale}，可见世界范围 x∈[{:.0},{:.0}] y∈[{:.0},{:.0}]。可见 {} 个、视野外 {} 个带图形的实体。",
        cam_x - half_w_units, cam_x + half_w_units,
        cam_y - half_h_units, cam_y + half_h_units,
        visible.len(), offscreen.len(),
    )];
    for v in &visible {
        lines.push(format!(
            "- {} {} 在{}（世界 {},{}）",
            v.get("name").and_then(|n| n.as_str()).unwrap_or_else(|| v["id"].as_str().expect("id")),
            v["sprite"]["color"].as_str().expect("color"),
            v["region"].as_str().expect("region"),
            v["world"]["x"], v["world"]["y"],
        ));
    }
    for o in &offscreen {
        lines.push(format!(
            "- {} 在视野外{}方向 {} 单位",
            o.get("name").and_then(|n| n.as_str()).unwrap_or_else(|| o["id"].as_str().expect("id")),
            o["direction"].as_str().expect("direction"),
            o["distance_units"],
        ));
    }
    for t in &texts {
        lines.push(format!(
            "- 文字 {:?} 在{}（世界 {},{}）",
            t["content"].as_str().expect("content"),
            t["region"].as_str().expect("region"),
            t["world"]["x"], t["world"]["y"],
        ));
    }

    Ok(json!({
        "camera": {"x": cam_x, "y": cam_y, "scale": scale},
        "viewport": {"width": width, "height": height},
        "visible": visible,
        "offscreen": offscreen,
        "texts": texts,
        "overlaps": overlaps,
        "text": lines.join("\n"),
    }))
}

/// 屏幕九宫格方位词（输入为 0..1 的屏幕比例坐标）。
fn region_word(fx: f64, fy: f64) -> &'static str {
    let col = if fx < 1.0 / 3.0 { 0 } else if fx < 2.0 / 3.0 { 1 } else { 2 };
    let row = if fy < 1.0 / 3.0 { 0 } else if fy < 2.0 / 3.0 { 1 } else { 2 };
    match (row, col) {
        (0, 0) => "左上", (0, 1) => "上方", (0, 2) => "右上",
        (1, 0) => "左侧", (1, 1) => "中心", (1, 2) => "右侧",
        (2, 0) => "左下", (2, 1) => "下方", _ => "右下",
    }
}

/// 视野外方向词（世界坐标系，y 向上）。
fn direction_word(dx: f64, dy: f64) -> &'static str {
    let horiz = if dx < -0.5 { -1 } else if dx > 0.5 { 1 } else { 0 };
    let vert = if dy < -0.5 { -1 } else if dy > 0.5 { 1 } else { 0 };
    match (horiz, vert) {
        (-1, 1) => "左上", (0, 1) => "上", (1, 1) => "右上",
        (-1, 0) => "左", (1, 0) => "右",
        (-1, -1) => "左下", (0, -1) => "下", (1, -1) => "右下",
        _ => "原地",
    }
}

/// 相机本体（不含抖动偏移）：取第一个 Camera 实体，没有则原点、8 像素/单位。
fn camera_base(world: &World) -> Result<(f64, f64, f64), String> {
    let cams = world.query(&["Camera"]);
    match cams.first() {
        None => Ok((0.0, 0.0, 8.0)),
        Some(&id) => {
            let x = num(world, id, "Camera.x")?;
            let y = num(world, id, "Camera.y")?;
            let scale = num(world, id, "Camera.scale")?;
            if scale <= 0.0 {
                return Err(format!("实体 {id} 的 Camera.scale 必须 > 0，拿到 {scale}"));
            }
            Ok((x, y, scale))
        }
    }
}

/// 渲染取景相机：本体 + 相机实体上 `Shake` 组件的抖动偏移。
/// CPU 光栅化和 GPU 路径都从这里取相机——两条路径抖得逐位一致。
pub fn camera_of(world: &World, tick: u64) -> Result<(f64, f64, f64), String> {
    let (mut x, mut y, scale) = camera_base(world)?;
    if let Some(&id) = world.query(&["Camera"]).first() {
        if world.has_component(id, "Shake") {
            let amplitude = num(world, id, "Shake.amplitude")?;
            let (dx, dy) = shake_offset(tick, amplitude);
            x += dx;
            y += dy;
        }
    }
    Ok((x, y, scale))
}

/// 屏幕抖动偏移（世界单位）：(tick, amplitude) 的纯函数，与模拟的 RNG 流完全无关
/// ——抖屏永远不会扰动 gameplay 的确定性轨迹，快照里也没有额外状态要存。
/// 实现：SplitMix64 把 tick 打散成 64 位，高/低各 32 位映射到 [-1, 1] 两轴再乘振幅。
pub fn shake_offset(tick: u64, amplitude: f64) -> (f64, f64) {
    if amplitude <= 0.0 {
        return (0.0, 0.0);
    }
    let mut z = tick.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let nx = ((z >> 32) as u32 as f64) / (u32::MAX as f64) * 2.0 - 1.0;
    let ny = (z as u32 as f64) / (u32::MAX as f64) * 2.0 - 1.0;
    (nx * amplitude, ny * amplitude)
}

fn num(world: &World, id: vitric_ecs::EntityId, path: &str) -> Result<f64, String> {
    let v: &Value = world.get_field(id, path).map_err(|e| e.to_string())?;
    v.as_f64().ok_or_else(|| format!("实体 {id} 的 {path} 不是数字: {v}"))
}

fn parse_color(s: &str) -> Result<[u8; 4], String> {
    let hex = s.strip_prefix('#').ok_or_else(|| {
        format!("颜色 {s:?} 格式不对。写法: \"#rrggbb\"，如红色 \"#ff0000\"")
    })?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 {s:?} 必须是 6 位十六进制 \"#rrggbb\""));
    }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("已校验十六进制");
    Ok([p(0), p(2), p(4), 255])
}

fn fill(buf: &mut [u8], rgba: [u8; 4]) {
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&rgba);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn world_one_red_sprite() -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        w
    }

    fn pixel(buf: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn sprite_renders_at_screen_center() {
        let w = world_one_red_sprite();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "中心是红色精灵");
        assert_eq!(pixel(&buf, 64, 2, 2), [24, 26, 33, 255], "角落是背景");
    }

    #[test]
    fn camera_moves_the_view() {
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        // 相机右移 2 单位 → 精灵在屏幕上左移 2*8=16 像素
        w.set_component(cam, "Camera", json!({"x": 2.0, "y": 0.0, "scale": 8.0})).unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32 - 16, 32), [255, 0, 0, 255]);
        assert_eq!(pixel(&buf, 64, 32 + 8, 32), [24, 26, 33, 255]);
    }

    #[test]
    fn text_renders_glyph_pixels_and_describe_reads_content() {
        let mut w = World::new();
        let e = w.spawn_named("score").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        // "I" 单字符，4 单位 → 32x32 像素，居中
        w.set_component(e, "Text", json!({"content": "I", "size": 4.0, "color": "#00ff00"}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        // "I" 的竖干在字形第 2-3 列（8x8 点阵字形偏左），取样打在竖干上
        assert_eq!(pixel(&buf, 64, 25, 32), [0, 255, 0, 255], "竖干处应是字形像素");
        assert_eq!(pixel(&buf, 64, 2, 2), [24, 26, 33, 255]);
        // 同世界同字节（文字也确定性）
        assert_eq!(buf, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());

        let d = describe_world(&w, 64, 64).unwrap();
        let texts = d["texts"].as_array().unwrap();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0]["content"], json!("I"));
        assert_eq!(texts[0]["region"], json!("中心"));
        assert!(d["text"].as_str().unwrap().contains("文字 \"I\""), "{}", d["text"]);
    }

    #[test]
    fn empty_text_is_skipped() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Text", json!({"content": "", "size": 4.0, "color": "#00ff00"}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [24, 26, 33, 255], "空文本不画");
        assert_eq!(describe_world(&w, 64, 64).unwrap()["texts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn same_world_same_bytes() {
        let w = world_one_red_sprite();
        assert_eq!(render_world(&w, 128, 96, &Assets::empty(), 0).unwrap(), render_world(&w, 128, 96, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn png_has_magic_and_decodes_back() {
        let w = world_one_red_sprite();
        let data = screenshot_png(&w, 32, 32, &Assets::empty(), 0).unwrap();
        assert_eq!(&data[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], "PNG 魔数");
        let decoder = png::Decoder::new(std::io::Cursor::new(&data[..]));
        let mut reader = decoder.read_info().unwrap();
        let mut out = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut out).unwrap();
        assert_eq!((info.width, info.height), (32, 32));
    }

    #[test]
    fn image_sprite_blits_with_alpha() {
        // 2x2 贴图：左半红不透明，右半全透明
        let dir = std::env::temp_dir().join(format!("vitric-blit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, /**/ 0, 0, 0, 0,
            255, 0, 0, 255, /**/ 0, 0, 0, 0,
        ];
        {
            let file = std::fs::File::create(dir.join("half.png")).unwrap();
            let mut enc = png::Encoder::new(file, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&pixels).unwrap();
        }
        let assets = Assets::load_dir(&dir).unwrap();

        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            e,
            "Sprite",
            json!({"w": 4.0, "h": 4.0, "color": "#ffffff", "image": "half.png"}),
        )
        .unwrap();
        // 默认相机 scale=8：精灵占屏幕中央 32x32 像素
        let buf = render_world(&w, 64, 64, &assets, 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32 - 8, 32), [255, 0, 0, 255], "左半是贴图红");
        assert_eq!(pixel(&buf, 64, 32 + 8, 32), [24, 26, 33, 255], "右半透明 → 透出背景");

        // 引用不存在的图：报错并列出现有素材
        w.set_field(e, "Sprite.image", json!("ghost.png")).unwrap();
        let err = render_world(&w, 64, 64, &assets, 0).unwrap_err();
        assert!(err.contains("half.png"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn describe_gives_semantic_view() {
        let mut w = World::new();
        let p = w.spawn_named("player").unwrap();
        w.set_component(p, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(p, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        // 跟玩家重叠的金币
        let c = w.spawn_named("coin").unwrap();
        w.set_component(c, "Position", json!({"x": 0.5, "y": 0.0})).unwrap();
        w.set_component(c, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ffd84d"})).unwrap();
        // 视野外左边远处一个
        let far = w.spawn_named("far-away").unwrap();
        w.set_component(far, "Position", json!({"x": -100.0, "y": 0.0})).unwrap();
        w.set_component(far, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#00ff00"})).unwrap();

        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["visible"].as_array().unwrap().len(), 2);
        assert_eq!(d["offscreen"].as_array().unwrap().len(), 1);
        assert_eq!(d["visible"][0]["name"], json!("player"));
        assert_eq!(d["visible"][0]["region"], json!("中心"));
        assert_eq!(d["offscreen"][0]["direction"], json!("左"));
        assert_eq!(d["offscreen"][0]["distance_units"], json!(100.0));
        // 玩家和金币视觉重叠要被点名
        let overlaps = d["overlaps"].as_array().unwrap();
        assert_eq!(overlaps.len(), 1, "{overlaps:?}");
        // 摘要可直接读
        let text = d["text"].as_str().unwrap();
        assert!(text.contains("player") && text.contains("中心") && text.contains("视野外"), "{text}");
    }

    #[test]
    fn pick_topmost_and_miss() {
        let mut w = World::new();
        let below = w.spawn_named("below").unwrap();
        w.set_component(below, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(below, "Sprite", json!({"w": 4.0, "h": 4.0, "color": "#ff0000"})).unwrap();
        let above = w.spawn_named("above").unwrap();
        w.set_component(above, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(above, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#00ff00"})).unwrap();
        // 屏幕中心：两个都覆盖，命中后画的 above
        assert_eq!(pick(&w, 64, 64, 32.0, 32.0).unwrap(), Some(above));
        // 偏一点：只有大的 below 覆盖（above 半宽 1 单位 = 8px）
        assert_eq!(pick(&w, 64, 64, 32.0 + 12.0, 32.0).unwrap(), Some(below));
        // 空地
        assert_eq!(pick(&w, 64, 64, 2.0, 2.0).unwrap(), None);
        // 坐标往返
        let (wx, wy) = screen_to_world(&w, 64, 64, 32.0 + 8.0, 32.0 - 16.0).unwrap();
        assert!((wx - 1.0).abs() < 1e-9 && (wy - 2.0).abs() < 1e-9, "{wx},{wy}");
    }

    #[test]
    fn selection_outline_draws_border() {
        let w_ = {
            let mut w = World::new();
            let e = w.spawn();
            w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
            w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
            (w, e)
        };
        let (w, e) = w_;
        let mut buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        draw_selection_outline(&mut buf, &w, 64, 64, e, 0).unwrap();
        // 精灵半宽 8px + 2px 外扩 → 描边在 x=32±10
        assert_eq!(pixel(&buf, 64, 32 - 10, 32), [39, 192, 168, 255], "左描边");
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "精灵本体不被盖");
    }

    #[test]
    fn shake_offset_is_pure_function_of_tick_and_amplitude() {
        // 同 (tick, amplitude) → 同偏移（纯函数，没有隐藏状态）
        assert_eq!(shake_offset(7, 0.5), shake_offset(7, 0.5));
        // 不同 tick → 偏移变（不然抖动是冻住的）
        assert_ne!(shake_offset(7, 0.5), shake_offset(8, 0.5));
        // 偏移每轴不超振幅；amplitude=0 → 零偏移
        let (dx, dy) = shake_offset(123, 0.5);
        assert!(dx.abs() <= 0.5 && dy.abs() <= 0.5, "({dx},{dy})");
        assert_eq!(shake_offset(123, 0.0), (0.0, 0.0));
    }

    #[test]
    fn camera_of_applies_shake_offset_deterministically() {
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 1.0, "y": 2.0, "scale": 8.0})).unwrap();
        w.set_component(cam, "Shake", json!({"amplitude": 0.5, "decay": 0.9})).unwrap();

        let shaken = camera_of(&w, 7).unwrap();
        assert_eq!(shaken, camera_of(&w, 7).unwrap(), "同世界同 tick 必须同取景");
        assert_ne!(shaken, camera_of(&w, 8).unwrap(), "tick 变了偏移要变");
        let (dx, dy) = shake_offset(7, 0.5);
        assert_eq!(shaken, (1.0 + dx, 2.0 + dy, 8.0), "取景 = 相机本体 + shake_offset");

        // 渲染整帧也确定：同 tick 逐字节相同，抖动 tick 间像素不同
        let f7 = render_world(&w, 64, 64, &Assets::empty(), 7).unwrap();
        assert_eq!(f7, render_world(&w, 64, 64, &Assets::empty(), 7).unwrap());
        assert_ne!(f7, render_world(&w, 64, 64, &Assets::empty(), 8).unwrap());

        // amplitude 归零 → 偏移消失，取景回到相机本体
        w.set_field(cam, "Shake.amplitude", json!(0.0)).unwrap();
        assert_eq!(camera_of(&w, 7).unwrap(), (1.0, 2.0, 8.0));
        // 语义观察/点选永远读不抖的相机
        w.set_field(cam, "Shake.amplitude", json!(0.5)).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["camera"], json!({"x": 1.0, "y": 2.0, "scale": 8.0}));
    }

    #[test]
    fn errors_are_helpful() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "red"})).unwrap();
        let err = render_world(&w, 32, 32, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("#rrggbb"), "{err}");
        assert!(render_world(&w, 0, 32, &Assets::empty(), 0).is_err());
    }
}
