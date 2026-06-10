//! TTF 矢量字体 — 清单 `font` 字段指向的字体的加载、排版、栅格化与缓存。
//!
//! 约束（为什么长这样）：
//! - **加载即校验**：文件缺失/损坏在 [`FontStore::load`] 显式报错并带上路径，
//!   不存在"游戏跑起来字突然没了"。
//! - **确定性**：栅格化（ab_glyph）是纯 Rust 浮点运算——同平台同二进制逐字节相同，
//!   跨平台末位不保证（与引擎其它三角函数路径同一条确定性边界）。缓存只省重算，
//!   不改变任何输出字节。
//! - **像素对齐**：字形按整数像素字号栅格化、整数像素落笔（不做亚像素定位）——
//!   缓存键 `(char, px)` 才有意义，CPU/GPU 两条路径也才能用同一份排版结果。
//! - **缓存放 Mutex 里**：`render_world` 拿的是 `&Assets`（渲染是世界的纯函数，
//!   签名不想为缓存破坏），所以缓存用内部可变性；`Mutex` 而非 `RefCell` 是为了
//!   让 Assets 保持 Send + Sync（不给未来的多线程使用埋雷），单线程下无竞争开销可忽略。
//! - 字体里没有的字符画该字体自带的 .notdef 字形（"豆腐块"）——明确可见的占位，
//!   不是静默跳过。要显示中文就给一个含 CJK 字形的字体（见 docs/agent-guide.md）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ab_glyph::{Font, ScaleFont};

/// 一个字符在某个像素字号下的栅格化结果（覆盖率位图 + 落笔偏移）。
pub struct RasterGlyph {
    pub width: u32,
    pub height: u32,
    /// 位图左上角相对**基线落笔点**的偏移（像素，y 向下；top 通常为负）。
    pub left: i32,
    pub top: i32,
    /// width*height 的覆盖率（0..=255），行优先。空轮廓（如空格）= 空 Vec。
    pub coverage: Vec<u8>,
}

/// 串内一个字形的落笔位置（由 [`FontStore::layout`] 给出）。
pub struct GlyphPlacement {
    pub ch: char,
    /// 笔位 x（像素，相对整串左缘，已含比例字距与字距调整）。
    pub x: f32,
}

/// 已加载的 TTF 字体 + 按 `(char, 像素字号)` 键的字形缓存。
pub struct FontStore {
    font: ab_glyph::FontVec,
    path: PathBuf,
    cache: Mutex<BTreeMap<(char, u32), Arc<RasterGlyph>>>,
}

impl std::fmt::Debug for FontStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontStore").field("path", &self.path).finish_non_exhaustive()
    }
}

impl FontStore {
    /// 从磁盘加载 TTF。缺失/损坏都显式报错并点名路径。
    pub fn load(path: &Path) -> Result<FontStore, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("读取字体 {} 失败: {e}。清单 font 字段的路径相对项目根目录", path.display()))?;
        let font = ab_glyph::FontVec::try_from_vec(bytes).map_err(|e| {
            format!("字体 {} 不是合法的 TTF/OTF: {e}。提示：换一个能正常打开的字体文件", path.display())
        })?;
        Ok(FontStore { font, path: path.to_path_buf(), cache: Mutex::new(BTreeMap::new()) })
    }

    /// 加载来源路径（素材热重载时按它重读磁盘）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Text.size 的语义换算：世界单位 size × 相机 scale → 整数像素字号（下限 1）。
    /// ab_glyph 的 PxScale 含义是 ascent-descent 的总高 = 这么多像素，
    /// 正好对应"size = 字形高度（世界单位）"。
    pub fn px_size(size: f64, scale: f64) -> u32 {
        (size * scale).round().max(1.0) as u32
    }

    fn scaled(&self, px: u32) -> ab_glyph::PxScaleFont<&ab_glyph::FontVec> {
        self.font.as_scaled(ab_glyph::PxScale::from(px as f32))
    }

    /// 基线相对竖向中心的偏移（像素，向下为正）：把 [descent, ascent] 的字身带
    /// 居中到锚点上——near enough to "竖向居中于大写字高"，且对所有字号一致。
    pub fn baseline_offset(&self, px: u32) -> f32 {
        let s = self.scaled(px);
        (s.ascent() + s.descent()) / 2.0
    }

    /// 排版一整串（单行，无换行）：每个字符的笔位 + 总宽（像素）。
    /// 比例字距（每字形自己的 advance）+ ab_glyph 字距调整（kern）。
    pub fn layout(&self, text: &str, px: u32) -> (Vec<GlyphPlacement>, f32) {
        let s = self.scaled(px);
        let mut placements = Vec::new();
        let mut pen = 0.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for ch in text.chars() {
            let id = s.glyph_id(ch);
            if let Some(p) = prev {
                pen += s.kern(p, id);
            }
            placements.push(GlyphPlacement { ch, x: pen });
            pen += s.h_advance(id);
            prev = Some(id);
        }
        (placements, pen)
    }

    /// 栅格化一个字符（命中缓存直接给）。字体没有该字符 → 该字体的 .notdef 字形。
    /// 落笔在整数像素上（position = 0,0），所以结果只取决于 (char, px)。
    pub fn raster(&self, ch: char, px: u32) -> Arc<RasterGlyph> {
        if let Some(hit) = self.cache.lock().expect("字形缓存锁").get(&(ch, px)) {
            return hit.clone();
        }
        let s = self.scaled(px);
        let glyph = s.glyph_id(ch).with_scale_and_position(
            ab_glyph::PxScale::from(px as f32),
            ab_glyph::point(0.0, 0.0),
        );
        let raster = match self.font.outline_glyph(glyph) {
            None => {
                // 无轮廓（空格等）：零尺寸，advance 仍由 layout 负责
                RasterGlyph { width: 0, height: 0, left: 0, top: 0, coverage: Vec::new() }
            }
            Some(outlined) => {
                let b = outlined.px_bounds();
                let (w, h) = (b.width().ceil() as u32, b.height().ceil() as u32);
                let mut coverage = vec![0u8; (w * h) as usize];
                outlined.draw(|x, y, c| {
                    let i = (y * w + x) as usize;
                    // f32 覆盖率 → u8（四舍五入）。draw 回调按固定行序扫，确定性
                    coverage[i] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                });
                RasterGlyph {
                    width: w,
                    height: h,
                    left: b.min.x.floor() as i32,
                    top: b.min.y.floor() as i32,
                    coverage,
                }
            }
        };
        let raster = Arc::new(raster);
        self.cache.lock().expect("字形缓存锁").insert((ch, px), raster.clone());
        raster
    }
}
