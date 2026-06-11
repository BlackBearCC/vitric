use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 解码后的图片（RGBA8）。
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 单张图片的边长上限。超了不是警告是错误——显存膨胀这类事故要在导入时拦死。
const MAX_DIMENSION: u32 = 2048;

/// 法线贴图命名配对（零配置约定）：`hero.png` 的法线贴图就是 `hero_n.png`。
/// 返回 `name` 对应的法线贴图名；`name` 本身已是 `_n` 贴图时返回 `None`
/// （法线贴图没有自己的法线贴图，配对不递归）。
pub fn normal_map_name(name: &str) -> Option<String> {
    if is_normal_map_name(name) {
        return None;
    }
    let (stem, ext) = name.rsplit_once('.')?;
    Some(format!("{stem}_n.{ext}"))
}

/// `name` 是不是法线贴图（文件名主干以 `_n` 结尾）。
/// 素材和谐化（vitric assets）用它把 `_n` 文件挡在量化之外——法线编码的是向量
/// 不是颜色，吸附到色板上等于毁掉数据。
pub fn is_normal_map_name(name: &str) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);
    match last.rsplit_once('.') {
        Some((stem, _)) => stem.ends_with("_n"),
        None => last.ends_with("_n"),
    }
}

/// 素材仓库：项目 `assets/` 目录下的全部 PNG，键是相对路径（正斜杠）；
/// 外加可选的 TTF 字体（清单 `font` 字段，见 [`crate::FontStore`]）。
///
/// 加载即校验：解码失败、超尺寸预算、字体损坏都在 `vitric check` / 启动时暴露，
/// 不存在"游戏跑起来图突然不见了"。
#[derive(Debug, Default)]
pub struct Assets {
    root: Option<PathBuf>,
    images: BTreeMap<String, Image>,
    /// 矢量字体（None = 用内嵌 8x8 点阵，旧行为字节不变）。
    font: Option<crate::FontStore>,
}

impl Assets {
    /// 空仓库（纯色块项目 / 测试用）。
    pub fn empty() -> Assets {
        Assets::default()
    }

    /// 从项目 assets 目录加载全部 PNG（递归）。目录不存在 = 空仓库（合法：纯色块游戏）。
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
            // 排序保证加载顺序确定（报错顺序也确定）
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

    /// 重新从磁盘加载（热重载素材）。失败保持旧内容。
    /// 字体跟着图一起重读（路径不变）——字体文件被换了也是热重载的一部分。
    pub fn reload(&mut self) -> Result<(), String> {
        let root = self.root.clone().ok_or("素材仓库没有目录，无法重载")?;
        let mut fresh = Assets::load_dir(&root)?;
        if let Some(font_path) = self.font.as_ref().map(|f| f.path().to_path_buf()) {
            fresh.load_font(&font_path)?;
        }
        *self = fresh;
        Ok(())
    }

    /// 挂载 TTF 字体（清单 `font` 字段）。缺失/损坏显式报错并点名路径。
    /// 挂上之后所有 Text 组件改走矢量路径（见 lib.rs 模块文档的 Text 约定）。
    pub fn load_font(&mut self, path: &Path) -> Result<(), String> {
        self.font = Some(crate::FontStore::load(path)?);
        Ok(())
    }

    /// 矢量字体（None = 点阵旧行为）。CPU 光栅化和 GPU 字形图集共用这一份。
    pub fn font(&self) -> Option<&crate::FontStore> {
        self.font.as_ref()
    }

    pub fn image(&self, name: &str) -> Option<&Image> {
        self.images.get(name)
    }

    /// `name` 的法线贴图（命名配对约定见 [`normal_map_name`]）。
    /// `None` = 没配对 = 该精灵不走法线光照（合法常态，不是错误）。
    pub fn normal_of(&self, name: &str) -> Option<&Image> {
        self.images.get(&normal_map_name(name)?)
    }

    pub fn names(&self) -> Vec<&str> {
        self.images.keys().map(|s| s.as_str()).collect()
    }

    pub fn count(&self) -> usize {
        self.images.len()
    }

    /// 全部图片解码后占用的内存（字节）——预算观测用。
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
    // 统一成 RGBA8
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
        // _n 文件自己没有法线贴图（配对不递归）
        assert_eq!(normal_map_name("hero_n.png"), None);
        // 无扩展名：配不出来（素材都是 .png，这里只是不 panic）
        assert_eq!(normal_map_name("noext"), None);
        assert!(is_normal_map_name("hero_n.png"));
        assert!(is_normal_map_name("ui/icon_n.png"));
        assert!(!is_normal_map_name("hero.png"));
        assert!(!is_normal_map_name("ui/icon.png"));
        // 主干本来就含 n 的不误判
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
