//! TTF vector font — loading, layout, rasterization and caching for the font pointed to by
//! the manifest `font` field.
//!
//! Constraints (why it looks like this):
//! - **Load-time validation**: a missing or corrupt file is an explicit error in
//!   [`FontStore::load`] that includes the path — there is no "the game starts up and the text
//!   suddenly vanishes".
//! - **Determinism**: rasterization (ab_glyph) is pure Rust floating-point arithmetic — byte-for-byte
//!   identical on the same platform/binary, not guaranteed to the last bit across platforms (the same
//!   determinism boundary as the engine's other trigonometric paths). The cache only saves recomputation;
//!   it does not change any output byte.
//! - **Pixel alignment**: glyphs are rasterized at integer pixel sizes and placed on integer pixels
//!   (no subpixel positioning) — the cache key `(char, px)` is then meaningful, and the CPU/GPU paths
//!   can share the same layout result.
//! - **Cache in a Mutex**: `render_world` takes `&Assets` (rendering is a pure function of the world,
//!   and the signature should not be broken for caching), so the cache uses interior mutability;
//!   `Mutex` rather than `RefCell` keeps Assets Send + Sync (no landmine for future multithreaded use),
//!   and contention-free overhead is negligible single-threaded.
//! - Characters missing from the font render the font's built-in .notdef glyph (the "tofu block") —
//!   a clearly visible placeholder, not a silent skip. To display Chinese, supply a font with CJK
//!   glyphs (see docs/agent-guide.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ab_glyph::{Font, ScaleFont};

/// `Text.reveal` (a 0..=1 ratio) → the number of characters that should be drawn right now (floored).
///
/// The single semantic point of text reveal: `reveal` is a generic text property, and per-character
/// typewriter = a tween pushing it from 0 to 1. Conventions:
/// - `reveal >= 1.0` (including the default 1.0 filled in for a missing field) → fully shown
///   (`total` chars), byte-for-byte identical to before this feature existed (backward compatible);
/// - `reveal <= 0.0` → no characters drawn at all;
/// - in between → `(reveal * total).floor()`; the floor guarantees "the last char only appears once
///   the tween reaches 1".
///
/// Counted by character (not byte); CJK reveals one glyph at a time, on the same basis as
/// [`FontStore::layout`].
pub fn revealed_chars(reveal: f64, total: usize) -> usize {
    if reveal >= 1.0 {
        total
    } else if reveal <= 0.0 {
        0
    } else {
        ((reveal * total as f64).floor() as usize).min(total)
    }
}

/// Rasterization result for one character at a given pixel size (coverage bitmap + pen offset).
pub struct RasterGlyph {
    pub width: u32,
    pub height: u32,
    /// Offset of the bitmap's top-left corner relative to the **baseline pen position**
    /// (pixels, y downward; top is usually negative).
    pub left: i32,
    pub top: i32,
    /// width*height coverage (0..=255), row-major. An empty outline (e.g. space) = empty Vec.
    pub coverage: Vec<u8>,
}

/// Pen position of one glyph within a string (produced by [`FontStore::layout`]).
#[derive(Clone)]
pub struct GlyphPlacement {
    pub ch: char,
    /// Pen x (pixels, relative to the left edge of the whole string; includes proportional advance
    /// and kerning adjustments).
    pub x: f32,
}

/// Layout result for a whole string: per-glyph pen positions + total width (pixels). Shared
/// read-only (the cache returns an `Arc`).
pub type LayoutResult = (Vec<GlyphPlacement>, f32);

/// A loaded TTF font + a glyph cache keyed by `(char, pixel size)`.
pub struct FontStore {
    font: ab_glyph::FontVec,
    path: PathBuf,
    cache: Mutex<BTreeMap<(char, u32), Arc<RasterGlyph>>>,
    /// Whole-string layout cache, keyed `(whole text, pixel size)`. Same rationale as the glyph
    /// cache (`render_world` takes `&Assets`, caching uses interior mutability + Mutex for Send+Sync).
    /// The hard reason it exists is the performance budget: per-character reveal (typewriter) only
    /// changes "how many chars to draw" each tick; the whole-paragraph layout **must not be recomputed
    /// every tick** — it is laid out once and cached, then hits return immediately, and the visible
    /// char count is just a slice over the cached placements. The `layout_calls` counter lets tests
    /// assert "the same text played for N ticks lays out exactly once".
    layout_cache: Mutex<BTreeMap<(String, u32), Arc<LayoutResult>>>,
    /// Number of times the layout algorithm actually ran (incremented only on cache miss). Test-only
    /// observability; does not enter any output.
    layout_calls: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for FontStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontStore").field("path", &self.path).finish_non_exhaustive()
    }
}

impl FontStore {
    /// Load a TTF from disk. Missing or corrupt files explicitly error and name the path.
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

    /// Loaded source path (used to re-read from disk on asset hot reload).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Semantic conversion for Text.size: world-unit size × camera scale → integer pixel size
    /// (lower-bounded at 1). ab_glyph's PxScale means the total ascent-descent height = that many
    /// pixels, which exactly matches "size = glyph height (in world units)".
    pub fn px_size(size: f64, scale: f64) -> u32 {
        (size * scale).round().max(1.0) as u32
    }

    fn scaled(&self, px: u32) -> ab_glyph::PxScaleFont<&ab_glyph::FontVec> {
        self.font.as_scaled(ab_glyph::PxScale::from(px as f32))
    }

    /// Offset of the baseline relative to the vertical center (pixels, positive downward): centers
    /// the [descent, ascent] body band on the anchor — near enough to "vertically centered on the
    /// cap height", and uniform across all sizes.
    pub fn baseline_offset(&self, px: u32) -> f32 {
        let s = self.scaled(px);
        (s.ascent() + s.descent()) / 2.0
    }

    /// Lay out a whole string (single line, no wrapping): each character's pen position + total
    /// width (pixels). Proportional advance (each glyph's own advance) + ab_glyph kerning (kern).
    /// **Uncached** version — runs the layout algorithm directly. Hot paths (text drawn every frame)
    /// should use [`FontStore::layout_cached`], which memoizes results by `(text, pixel size)` so
    /// per-character reveal does not re-lay out the whole string every tick.
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

    /// Cached layout: a hit on `(text, pixel size)` returns directly; a miss runs the layout once.
    /// The landing point of performance-budget rule 3 — when the same text is revealed character by
    /// character over N ticks, the `layout` algorithm runs exactly once (the `layout_runs` counter
    /// locks this invariant down); the changing visible count just slices the returned placements,
    /// never triggering a re-layout. Returns an `Arc` to avoid cloning the whole placement vector
    /// every frame (zero extra allocation on the hot path).
    pub fn layout_cached(&self, text: &str, px: u32) -> Arc<LayoutResult> {
        let key = (text.to_string(), px);
        if let Some(hit) = self.layout_cache.lock().expect("排版缓存锁").get(&key) {
            return hit.clone();
        }
        let laid = Arc::new(self.layout(text, px));
        self.layout_cache.lock().expect("排版缓存锁").insert(key, laid.clone());
        laid
    }

    /// Number of times the `layout` algorithm actually executed (counted only on cache miss).
    /// Test-only — asserts "the layout is computed once"; does not enter any render output.
    pub fn layout_runs(&self) -> u64 {
        self.layout_calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Rasterize one character (a cache hit returns directly). If the font lacks the character,
    /// the font's .notdef glyph is used. The pen is on integer pixels (position = 0,0), so the
    /// result depends only on (char, px).
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
                // No outline (space, etc.): zero size; advance is still layout's responsibility.
                RasterGlyph { width: 0, height: 0, left: 0, top: 0, coverage: Vec::new() }
            }
            Some(outlined) => {
                let b = outlined.px_bounds();
                let (w, h) = (b.width().ceil() as u32, b.height().ceil() as u32);
                let mut coverage = vec![0u8; (w * h) as usize];
                outlined.draw(|x, y, c| {
                    let i = (y * w + x) as usize;
                    // f32 coverage → u8 (rounded). The draw callback scans in a fixed row order; deterministic.
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
        // Fully shown (default 1.0 / above 1): byte-for-byte identical to before reveal existed.
        assert_eq!(revealed_chars(1.0, 5), 5);
        assert_eq!(revealed_chars(2.0, 5), 5);
        // No characters at all.
        assert_eq!(revealed_chars(0.0, 5), 0);
        assert_eq!(revealed_chars(-0.5, 5), 0);
        // Middle, floored: 0.5×5 = 2.5 → 2; 0.99×5 = 4.95 → 4 (the 5th char only appears once the tween reaches 1).
        assert_eq!(revealed_chars(0.5, 5), 2);
        assert_eq!(revealed_chars(0.99, 5), 4);
        assert_eq!(revealed_chars(0.2, 5), 1);
        // Empty string is always 0, no panic.
        assert_eq!(revealed_chars(0.5, 0), 0);
        assert_eq!(revealed_chars(1.0, 0), 0);
    }
}
