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

/// `Text.reveal`（0..=1 比例）→ 当前应画出的字符数（向下取整）。
///
/// 文字显隐的唯一语义点：`reveal` 是通用文本属性，逐字打字机 = 补间把它从 0
/// 推到 1。约定：
/// - `reveal >= 1.0`（含字段缺省补的 1.0）→ 全显（`total` 个字），与未引入本特性
///   时逐字节相同（向后兼容）；
/// - `reveal <= 0.0` → 一个字都不画；
/// - 中间 → `(reveal * total).floor()`，下取整保证"补间到 1 才显最后一个字"。
///
/// 按字符（非字节）计数，CJK 一次显一个字形，和 [`FontStore::layout`] 同口径。
pub fn revealed_chars(reveal: f64, total: usize) -> usize {
    if reveal >= 1.0 {
        total
    } else if reveal <= 0.0 {
        0
    } else {
        ((reveal * total as f64).floor() as usize).min(total)
    }
}

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
#[derive(Clone)]
pub struct GlyphPlacement {
    pub ch: char,
    /// 笔位 x（像素，相对整串左缘，已含比例字距与字距调整）。
    pub x: f32,
}

/// 一整串的排版结果：逐字形落笔位置 + 总宽（像素）。共享只读（缓存返回 `Arc`）。
pub type LayoutResult = (Vec<GlyphPlacement>, f32);

/// 已加载的 TTF 字体 + 按 `(char, 像素字号)` 键的字形缓存。
pub struct FontStore {
    font: ab_glyph::FontVec,
    path: PathBuf,
    cache: Mutex<BTreeMap<(char, u32), Arc<RasterGlyph>>>,
    /// 整串排版结果缓存，键 `(整串文字, 像素字号)`。和字形缓存同样的理由
    /// （`render_world` 拿 `&Assets`，缓存走内部可变性 + Mutex 保 Send+Sync）。
    /// 存在的硬理由是性能预算：逐字显示（打字机）每 tick 只改"画到第几个字"，
    /// 整段版面**绝不能每 tick 重排**——首次排一次进缓存，之后命中即返回，
    /// 可见字数只是在排好的 placements 上切一刀。`layout_calls` 计数器供测试
    /// 断言"同一段文字播 N tick，排版只发生 1 次"。
    layout_cache: Mutex<BTreeMap<(String, u32), Arc<LayoutResult>>>,
    /// 真正跑过排版算法的次数（缓存未命中才 +1）。只为测试可观测，不进任何输出。
    layout_calls: std::sync::atomic::AtomicU64,
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
        Ok(FontStore {
            font,
            path: path.to_path_buf(),
            cache: Mutex::new(BTreeMap::new()),
            layout_cache: Mutex::new(BTreeMap::new()),
            layout_calls: std::sync::atomic::AtomicU64::new(0),
        })
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
    /// **未缓存**版本——直接跑排版算法。热路径（每帧画文字）请走 [`FontStore::layout_cached`]，
    /// 它把结果按 `(文字, 像素字号)` memo 住，逐字显示不会每 tick 重排整段。
    pub fn layout(&self, text: &str, px: u32) -> (Vec<GlyphPlacement>, f32) {
        self.layout_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    /// 缓存版排版：命中 `(文字, 像素字号)` 直接给，未命中才真排一次。
    /// 性能预算第 3 条的落点——逐字显示同一段文字播 N tick，`layout` 算法只跑 1 次
    /// （`layout_runs` 计数器锁住这个不变量）；可见字数变化只在返回的 placements 上切片，
    /// 不触发重排。返回 `Arc` 避免每帧克隆整串 placements（热路径零额外分配）。
    pub fn layout_cached(&self, text: &str, px: u32) -> Arc<LayoutResult> {
        let key = (text.to_string(), px);
        if let Some(hit) = self.layout_cache.lock().expect("排版缓存锁").get(&key) {
            return hit.clone();
        }
        let laid = Arc::new(self.layout(text, px));
        self.layout_cache.lock().expect("排版缓存锁").insert(key, laid.clone());
        laid
    }

    /// `layout` 算法真正执行过的次数（缓存未命中才计数）。测试专用——
    /// 断言"版面只算一次"，不进任何渲染输出。
    pub fn layout_runs(&self) -> u64 {
        self.layout_calls.load(std::sync::atomic::Ordering::Relaxed)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revealed_chars_endpoints_and_floor() {
        // 全显（缺省的 1.0 / 超过 1）：与未引入 reveal 时逐字相同
        assert_eq!(revealed_chars(1.0, 5), 5);
        assert_eq!(revealed_chars(2.0, 5), 5);
        // 一个字都没有
        assert_eq!(revealed_chars(0.0, 5), 0);
        assert_eq!(revealed_chars(-0.5, 5), 0);
        // 中间下取整：0.5×5 = 2.5 → 2；0.99×5 = 4.95 → 4（补间到 1 才显第 5 个）
        assert_eq!(revealed_chars(0.5, 5), 2);
        assert_eq!(revealed_chars(0.99, 5), 4);
        assert_eq!(revealed_chars(0.2, 5), 1);
        // 空串恒 0，不 panic
        assert_eq!(revealed_chars(0.5, 0), 0);
        assert_eq!(revealed_chars(1.0, 0), 0);
    }
}
