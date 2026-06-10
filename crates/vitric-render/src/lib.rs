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

use serde_json::Value;

use vitric_ecs::World;

/// 渲染一帧：返回 RGBA8 像素（行优先，左上原点）。
pub fn render_world(world: &World, width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(format!("分辨率 {width}x{height} 不合法（1..=4096）"));
    }
    let (cam_x, cam_y, scale) = camera_of(world)?;

    let mut buf = vec![0u8; (width * height * 4) as usize];
    // 背景：深灰蓝，区别于纯黑（纯黑常被误判为「没渲出来」）
    fill(&mut buf, [24, 26, 33, 255]);

    // 按实体序绘制（确定性；后画的盖前画的）
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
        let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Sprite.color: {e}"))?;

        // 世界 → 屏幕（y 翻转，相机居中）
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;
        let x0 = (cx - half_w).floor().max(0.0) as i64;
        let x1 = (cx + half_w).ceil().min(width as f64) as i64;
        let y0 = (cy - half_h).floor().max(0.0) as i64;
        let y1 = (cy + half_h).ceil().min(height as f64) as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y as u32 * width + x as u32) * 4) as usize;
                buf[i..i + 4].copy_from_slice(&rgba);
            }
        }
    }
    Ok(buf)
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
pub fn screenshot_png(world: &World, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let rgba = render_world(world, width, height)?;
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
    let (cam_x, cam_y, scale) = camera_of(world)?;
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
        entry.insert("sprite".into(), json!({"w": sw, "h": sh, "color": color}));

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

    Ok(json!({
        "camera": {"x": cam_x, "y": cam_y, "scale": scale},
        "viewport": {"width": width, "height": height},
        "visible": visible,
        "offscreen": offscreen,
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

fn camera_of(world: &World) -> Result<(f64, f64, f64), String> {
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
        let buf = render_world(&w, 64, 64).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "中心是红色精灵");
        assert_eq!(pixel(&buf, 64, 2, 2), [24, 26, 33, 255], "角落是背景");
    }

    #[test]
    fn camera_moves_the_view() {
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        // 相机右移 2 单位 → 精灵在屏幕上左移 2*8=16 像素
        w.set_component(cam, "Camera", json!({"x": 2.0, "y": 0.0, "scale": 8.0})).unwrap();
        let buf = render_world(&w, 64, 64).unwrap();
        assert_eq!(pixel(&buf, 64, 32 - 16, 32), [255, 0, 0, 255]);
        assert_eq!(pixel(&buf, 64, 32 + 8, 32), [24, 26, 33, 255]);
    }

    #[test]
    fn same_world_same_bytes() {
        let w = world_one_red_sprite();
        assert_eq!(render_world(&w, 128, 96).unwrap(), render_world(&w, 128, 96).unwrap());
    }

    #[test]
    fn png_has_magic_and_decodes_back() {
        let w = world_one_red_sprite();
        let data = screenshot_png(&w, 32, 32).unwrap();
        assert_eq!(&data[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], "PNG 魔数");
        let decoder = png::Decoder::new(std::io::Cursor::new(&data[..]));
        let mut reader = decoder.read_info().unwrap();
        let mut out = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut out).unwrap();
        assert_eq!((info.width, info.height), (32, 32));
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
    fn errors_are_helpful() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "red"})).unwrap();
        let err = render_world(&w, 32, 32).unwrap_err();
        assert!(err.contains("#rrggbb"), "{err}");
        assert!(render_world(&w, 0, 32).is_err());
    }
}
