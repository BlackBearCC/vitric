use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A decoded image (RGBA8).
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Side-length limit for a single image. Exceeding it is an error, not a warning — VRAM bloat
/// like this must be stopped at import time.
const MAX_DIMENSION: u32 = 2048;

/// Normal-map name pairing (a zero-config convention): the normal map for `hero.png` is
/// `hero_n.png`. Returns the normal-map name for `name`; if `name` is itself a `_n` map, returns
/// `None` (normal maps do not have their own normal maps; pairing does not recurse).
pub fn normal_map_name(name: &str) -> Option<String> {
    if is_normal_map_name(name) {
        return None;
    }
    let (stem, ext) = name.rsplit_once('.')?;
    Some(format!("{stem}_n.{ext}"))
}

/// Whether `name` is a normal map (file-name stem ends with `_n`). Asset harmonization
/// (`vitric assets`) uses this to keep `_n` files out of quantization — normals encode vectors,
/// not colors; snapping them to a palette would destroy the data.
pub fn is_normal_map_name(name: &str) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);
    match last.rsplit_once('.') {
        Some((stem, _)) => stem.ends_with("_n"),
        None => last.ends_with("_n"),
    }
}

/// Asset store: all PNGs under the project's `assets/` directory, keyed by relative path
/// (forward slashes); plus an optional TTF font (the manifest `font` field, see [`crate::FontStore`]).
///
/// Load-time validation: decode failures, over-budget sizes, and corrupt fonts are all surfaced
/// during `vitric check` / startup — there is no "the game starts up and an image suddenly vanishes".
#[derive(Debug, Default)]
pub struct Assets {
    root: Option<PathBuf>,
    images: BTreeMap<String, Image>,
    /// Vector font (None = use the built-in 8x8 bitmap, byte-identical to the old behavior).
    font: Option<crate::FontStore>,
}

impl Assets {
    /// An empty store (solid-color projects / tests).
    pub fn empty() -> Assets {
        Assets::default()
    }

    /// Load all PNGs from the project assets directory (recursive). A missing directory = an empty
    /// store (legal: a solid-color game).
    pub fn load_dir(dir: &Path) -> Result<Assets, String> {
        let mut assets =
            Assets { root: Some(dir.to_path_buf()), images: BTreeMap::new(), font: None };
        if !dir.exists() {
            return Ok(assets);
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let entries = std::fs::read_dir(&d)
                .map_err(|e| format!("读素材目录 {} 失败: {e}", d.display()))?;
            // Sort so the load order (and the error order) is deterministic.
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
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
                    let img = load_png(&path)
                        .map_err(|e| format!("素材 {rel}: {e}"))?;
                    assets.images.insert(rel, img);
                }
            }
        }
        Ok(assets)
    }

    /// Reload from disk (hot-reload assets). On failure the old contents are kept.
    /// The font is re-read together with the images (path unchanged) — a swapped font file is
    /// part of hot reload too.
    pub fn reload(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or("素材仓库没有目录，无法重载")?;
        let mut fresh = Assets::load_dir(&root)?;
        if let Some(font_path) = self.font.as_ref().map(|f| f.path().to_path_buf()) {
            fresh.load_font(&font_path)?;
        }
        *self = fresh;
        Ok(())
    }

    /// Mount a TTF font (manifest `font` field). Missing/corrupt explicitly errors and names the path.
    /// Once mounted, all Text components go through the vector path (see the Text convention in the
    /// lib.rs module docs).
    pub fn load_font(&mut self, path: &Path) -> Result<(), String> {
        self.font = Some(crate::FontStore::load(path)?);
        Ok(())
    }

    /// Vector font (None = legacy bitmap). Shared by the CPU rasterizer and the GPU glyph atlas.
    pub fn font(&self) -> Option<&crate::FontStore> {
        self.font.as_ref()
    }

    pub fn image(&self, name: &str) -> Option<&Image> {
        self.images.get(name)
    }

    /// The normal map for `name` (naming-pairing convention, see [`normal_map_name`]).
    /// `None` = no pairing = this sprite skips normal-map lighting (a legal state, not an error).
    pub fn normal_of(&self, name: &str) -> Option<&Image> {
        self.images.get(&normal_map_name(name)?)
    }

    /// Whether the asset store contains any normal maps (`*_n.png` pairing). If not, the renderer
    /// skips the normal buffer for the whole frame (allocation / clear / compositing read all saved)
    /// — the output is bit-identical to "having a buffer but all sentinels" (sentinel semantics are
    /// in the lib.rs module docs; backward compatibility is locked down by tests).
    pub fn has_normal_maps(&self) -> bool {
        self.images.keys().any(|k| is_normal_map_name(k))
    }

    pub fn names(&self) -> Vec<&str> {
        self.images.keys().map(|s| s.as_str()).collect()
    }

    pub fn count(&self) -> usize {
        self.images.len()
    }

    /// Memory used by all decoded images (bytes) — for budget observability.
    pub fn total_bytes(&self) -> usize {
        self.images.values().map(|i| i.rgba.len()).sum()
    }
}

fn load_png(path: &Path) -> Result<Image, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().map_err(|e| format!("PNG 解码失败: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG 尺寸异常")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG 解码失败: {e}"))?;
    if info.width > MAX_DIMENSION || info.height > MAX_DIMENSION {
        return Err(format!(
            "尺寸 {}x{} 超过上限 {MAX_DIMENSION}。提示：精灵图不需要这么大，缩小或切分它",
            info.width, info.height
        ));
    }
    // Unify to RGBA8.
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
    Ok(Image { width: info.width, height: info.height, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(rgba).unwrap();
    }

    #[test]
    fn load_and_query() {
        let dir = std::env::temp_dir().join(format!("vitric-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_png(&dir.join("coin.png"), 2, 2, &[255, 0, 0, 255].repeat(4));
        write_png(&dir.join("ui/icon.png"), 1, 1, &[0, 255, 0, 255]);
        let assets = Assets::load_dir(&dir).unwrap();
        assert_eq!(assets.count(), 2);
        assert_eq!(assets.names(), vec!["coin.png", "ui/icon.png"]);
        assert_eq!(assets.image("coin.png").unwrap().width, 2);
        assert!(assets.image("ghost.png").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn normal_map_naming_pairs_and_excludes_n_files() {
        assert_eq!(normal_map_name("hero.png").as_deref(), Some("hero_n.png"));
        assert_eq!(normal_map_name("ui/icon.png").as_deref(), Some("ui/icon_n.png"));
        // _n files have no normal map of their own (pairing does not recurse).
        assert_eq!(normal_map_name("hero_n.png"), None);
        // No extension: cannot pair (assets are all .png; this just doesn't panic).
        assert_eq!(normal_map_name("noext"), None);
        assert!(is_normal_map_name("hero_n.png"));
        assert!(is_normal_map_name("ui/icon_n.png"));
        assert!(!is_normal_map_name("hero.png"));
        assert!(!is_normal_map_name("ui/icon.png"));
        // A stem that merely contains an n is not misclassified.
        assert!(!is_normal_map_name("lantern.png"));
    }

    #[test]
    fn normal_of_finds_paired_image() {
        let dir = std::env::temp_dir().join(format!("vitric-normalpair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_png(&dir.join("hero.png"), 2, 2, &[255, 0, 0, 255].repeat(4));
        write_png(&dir.join("hero_n.png"), 2, 2, &[128, 128, 255, 255].repeat(4));
        write_png(&dir.join("gem.png"), 1, 1, &[0, 255, 0, 255]);
        let assets = Assets::load_dir(&dir).unwrap();
        assert!(assets.normal_of("hero.png").is_some(), "命名配对生效");
        assert!(assets.normal_of("gem.png").is_none(), "没配对 = 不走法线光照");
        assert!(assets.normal_of("hero_n.png").is_none(), "法线贴图自己没有法线贴图");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_dir_is_empty_not_error() {
        let assets = Assets::load_dir(Path::new("/nonexistent-vitric-assets")).unwrap();
        assert_eq!(assets.count(), 0);
    }

    #[test]
    fn corrupt_png_is_named_in_error() {
        let dir = std::env::temp_dir().join(format!("vitric-assets-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.png"), b"not a png").unwrap();
        let err = Assets::load_dir(&dir).unwrap_err();
        assert!(err.contains("broken.png"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
