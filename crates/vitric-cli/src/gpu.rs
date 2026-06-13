//! GPU 呈现 — wgpu 上屏路径。
//!
//! 只管"把画面送上窗口"，**不是**渲染真相源：HEADLESS 截图（render/screenshot）
//! 永远走 vitric-render 的 CPU 光栅化，逐字节确定。这里读同一套组件约定
//! （Position/Sprite/Text/Camera），视觉语义对齐 CPU 路径——同一世界，
//! 人在 GPU 窗口看到的和 agent 截图看到的是同一幅画。
//!
//! 结构：启动时把全部素材 + 8x8 点阵字体 + 1x1 白块打进**一张图集**，
//! 每帧 CPU 端按实体序攒一份顶点流（像素坐标 + 图集 UV + 染色），
//! 一条管线、一个绑定组、一次 draw 画完。素材热重载靠代次号触发图集重建。
//!
//! 矢量字体（清单 `font`，见 vitric-render::FontStore）走**第二张动态字形图集**：
//! 1024x1024 RGBA（白底 + 覆盖率当 alpha——shader 不用改，染色乘法即抗锯齿混合），
//! 货架打包，新 (字符, 像素字号) 出现时懒栅格化 + queue.write_texture 增量上传。
//! 同一条管线、第二个绑定组：先画精灵流（主图集），再画字形流（字形图集）。
//! 排版/栅格化/取整全部复用 vitric-render 的 FontStore——与 CPU 路径视觉对齐，
//! 但不承诺逐字节相同（覆盖率混合发生在 GPU 混合阶段）；截图/断言永远以 CPU 为准。
//! 图集满了显式报错（提示减少不同字号），不静默丢字。
//!
//! 泛光（Bloom 实体存在时）把单 pass 拆成多 pass：
//!   1. 场景 pass：同一条顶点流渲进离屏纹理（Rgba8Unorm，光照照常在这里跑）
//!   2. 下采样+阈值 pass：场景 → 半分辨率（亮部提取）
//!   3. 盒式模糊 ×6：半分辨率 ping-pong，水平/垂直交替 3 轮
//!   4. 合成 pass：场景 + 泛光·strength → 表面（sRGB 反算只在这最后一步做）
//!
//! 没有 Bloom 实体时完全走老的单 pass 直渲表面——零额外开销，字节不变。
//!
//! 与 CPU 真相源（vitric-render::apply_bloom）的已知差异（视觉一致优先，不逐字节）：
//! - GPU 模糊在**半分辨率**跑（半径取 CPU 的一半，下限 1）——省 4 倍带宽，光晕
//!   空间尺度一致；合成时双线性放大回全分辨率（CPU 全程全分辨率）
//! - 下采样取最近邻单点（CPU 没有下采样这一步）
//! - 中间结果存 8 位 Unorm（CPU 全程 f32）——每 pass 量化一次
//!
//! 截图/断言永远以 CPU 路径为准，这里只负责"窗口里看起来一样"。

use std::sync::Arc;

use winit::window::Window;

use vitric_ecs::World;
use vitric_render::Assets;

/// 顶点：像素坐标（shader 里除视口尺寸转 NDC）+ 图集 UV + 染色（乘进采样色）
/// + 法线贴图 UV + 精灵旋转。
///
/// 法线贴图与普通图同住一张图集（它们就是 assets/ 里的 PNG），所以只多一组 UV：
/// `nuv` 跟角走（与 uv 同一角序展开），x < 0 = 哨兵 = 该图元没有法线（纯色块/
/// 文字/没配对的贴图）——片元走原光照公式，与 CPU 路径的哨兵零向量同语义。
/// `rotcs` = Sprite.rot 的 (cos, sin)，片元里把采样出的法线旋到屏幕空间
/// （局部→屏幕矩阵 [[c, s], [-s, c]]，与 CPU 的 sample_normal 同一矩阵）。
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    nuv: [f32; 2],
    rotcs: [f32; 2],
}

/// 法线 UV 的哨兵矩形：四角全 -1（插值后仍 < 0，片元据此跳过法线路径）。
const NO_NORMAL: UvRect = [-1.0; 4];

/// 自发光图元的哨兵矩形：四角全 -2（< -1.5，片元据此**整个跳过光照**——粒子专用，
/// 与 CPU 路径"粒子在光照之后画"的语义镜像）。-1（普通无法线）照旧被打光。
const UNLIT: UvRect = [-2.0; 4];

/// uniform：viewport = [宽, 高, "表面是 sRGB"标志, "光照开启"标志]（z>0.5 时片元做
/// sRGB→线性，保证最终字节和 CPU 路径的 sRGB 字节一致；w>0.5 时片元跑光照公式）。
/// ambient = [环境色 rgb, 灯数]；每盏灯三条 vec4（打包布局，被单测锁死）：
/// - lights_pos[i]   = [灯心 x_px, y_px, 半径 px, kind]，kind: 0=point / 1=spot / 2=directional
///   （平行光不读位置/半径，前三位恒 0 占位）
/// - lights_color[i] = [r·intensity, g·intensity, b·intensity, 0]（w 保留）
/// - lights_dir[i]   = [朝向像素空间单位向量 x, y, 半锥角弧度, 0]；spot 用 xy+z，
///   directional 用 xy（法线像素按它算方向，哨兵像素不读），point 恒 0
///
/// 投影（Ambient.shadows）再加四块（同样被单测锁死；遮光体先合并相邻贴齐的箱子、
/// 再按灯盘逐灯剔除——语义源头 vitric_render::build_shadow_boxes / cull_shadow_boxes，
/// CPU 路径同一套，两步都不改可见输出）：
/// - shadow_ranges[i] = [该灯在 occluders 里的起点, 条数, 0, 0]（投影关闭/平行光 = 全 0，
///   shader 循环零次）
/// - occluders[k] = 合并大箱的像素空间 AABB [x0, y0, x1, y1]，按灯排成连续区间
///   （逐灯剔除后逐灯重复打包；预算 [`SHADOW_FLAT_BUDGET`] 条，超了显式报错）
/// - occluder_sub_ranges[k] = [大箱的子箱起点, 子箱数, 0, 0]（与 occluders 平行；
///   像素落在大箱内时回落到子箱——"自己所在的箱子不挡自己"按原始实体判）
/// - occluder_subs[s] = 原始遮光体的像素空间 AABB（全局一份不逐灯重复，
///   上限 vitric_render::MAX_OCCLUDERS = collect_occluders 的硬上限）
///
/// 世界→像素的变换在 CPU 端做完（含 y 翻转：世界 dir 度数 → (cos, -sin)），shader 只算
/// 距离和点积。vec4 数组在 WGSL uniform（std140 风格）下天然 16 字节步长，无 padding 坑。
/// 整个 uniform ≈ 16.4KB——在默认 Limits（64KB）内（不支持 WebGL2 downlevel 的 16KB，
/// 桌面原生目标无此约束）。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 4],
    ambient: [f32; 4],
    lights_pos: [[f32; 4]; vitric_render::MAX_LIGHTS],
    lights_color: [[f32; 4]; vitric_render::MAX_LIGHTS],
    lights_dir: [[f32; 4]; vitric_render::MAX_LIGHTS],
    shadow_ranges: [[f32; 4]; vitric_render::MAX_LIGHTS],
    occluders: [[f32; 4]; SHADOW_FLAT_BUDGET],
    occluder_sub_ranges: [[f32; 4]; SHADOW_FLAT_BUDGET],
    occluder_subs: [[f32; 4]; vitric_render::MAX_OCCLUDERS],
}

/// 全部灯合计的遮挡列表 uniform 预算（合并 + 逐灯剔除之后的条数，逐灯重复计）。
const SHADOW_FLAT_BUDGET: usize = 256;

/// 单盏灯剔除后的遮挡列表上限。超了说明灯radius盖住的独立（合并不了的）遮光体太多——
/// 显式报错，提示减灯/减箱/合并瓦片，不静默截断。
const SHADOW_PER_LIGHT: usize = 64;

/// 图集里一块区域的 UV 矩形 [u0, v0, u1, v1]。
type UvRect = [f32; 4];

/// 图集：素材名 → UV 区域，外加白块（纯色用）和 128 个字体字形。
struct Atlas {
    images: std::collections::BTreeMap<String, UvRect>,
    /// 白块中心点 UV（四角同 UV → 平采样，永不渗色）。
    white: [f32; 2],
    glyphs: [UvRect; 128],
}

/// 动态字形图集的边长（像素）。1024²·RGBA = 4MB 显存，装得下几百个常用字号字形；
/// 满了显式报错（见 [`GlyphAtlas::alloc_rect`]），不静默丢字。
const GLYPH_ATLAS_SIZE: u32 = 1024;

/// 一次待执行的字形像素上传（[`GlyphAtlas`] 攒，present 里 write_texture 消化）。
/// 像素是 RGBA：白底 + 覆盖率当 alpha——主 shader 的 `采样色 × 染色` 直接得到
/// 与 CPU 路径同公式的覆盖率混合（抗锯齿），不需要第二条管线。
struct GlyphUpload {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    pixels: Vec<u8>,
}

/// 动态字形图集（纯簿记，不碰 GPU——可单测）：货架打包 + (字符, 像素字号) → UV 缓存
/// + 待上传队列。字形按需出现：栅格化一次、上传一次、之后永远命中缓存。
struct GlyphAtlas {
    size: u32,
    /// 货架游标（当前行起点 x / 行顶 y / 行高）。
    x: u32,
    y: u32,
    row_h: u32,
    map: std::collections::BTreeMap<(char, u32), UvRect>,
    /// 白块中心点 UV（字体模式下选中描边的纯色来源——描边和字形同一个绑定组）。
    white: [f32; 2],
    pending: Vec<GlyphUpload>,
}

impl GlyphAtlas {
    fn new(size: u32) -> GlyphAtlas {
        let mut a = GlyphAtlas {
            size,
            x: 0,
            y: 0,
            row_h: 0,
            map: std::collections::BTreeMap::new(),
            white: [0.0; 2],
            pending: Vec::new(),
        };
        let (ox, oy) = a.alloc_rect(2, 2).expect("空图集必然放得下 2x2 白块");
        a.white = [(ox as f32 + 1.0) / size as f32, (oy as f32 + 1.0) / size as f32];
        a.pending.push(GlyphUpload { x: ox, y: oy, w: 2, h: 2, pixels: vec![255u8; 2 * 2 * 4] });
        a
    }

    /// 货架打包一块 w×h（右/下各留 1px 空隙）。满了/单块过大都显式报错。
    fn alloc_rect(&mut self, w: u32, h: u32) -> Result<(u32, u32), String> {
        let (bw, bh) = (w + 1, h + 1);
        if bw > self.size || bh > self.size {
            return Err(format!(
                "字形 {w}x{h} 超过字形图集边长 {0}。提示：Text.size×相机 scale 太大了，调小字号",
                self.size
            ));
        }
        if self.x + bw > self.size {
            self.x = 0;
            self.y += self.row_h;
            self.row_h = 0;
        }
        if self.y + bh > self.size {
            return Err(format!(
                "字形图集 {0}x{0} 已满（游戏用了太多不同的 字符×字号 组合）。\
                 提示：减少不同的 Text.size 取值/相机缩放档位，字形按 (字符,像素字号) 缓存",
                self.size
            ));
        }
        let pos = (self.x, self.y);
        self.row_h = self.row_h.max(bh);
        self.x += bw;
        Ok(pos)
    }

    /// 一个 (字符, 像素字号) 的 UV：命中缓存直接给；首次出现栅格化 + 排队上传。
    /// 空轮廓字形（空格等）不该走到这里（调用方先看 coverage 跳过）。
    fn glyph_uv(
        &mut self,
        font: &vitric_render::FontStore,
        ch: char,
        px: u32,
    ) -> Result<UvRect, String> {
        if let Some(uv) = self.map.get(&(ch, px)) {
            return Ok(*uv);
        }
        let g = font.raster(ch, px);
        let (ox, oy) = self.alloc_rect(g.width, g.height)?;
        let mut pixels = Vec::with_capacity((g.width * g.height * 4) as usize);
        for &cov in &g.coverage {
            pixels.extend_from_slice(&[255, 255, 255, cov]);
        }
        self.pending.push(GlyphUpload { x: ox, y: oy, w: g.width, h: g.height, pixels });
        let s = self.size as f32;
        let uv = [
            ox as f32 / s,
            oy as f32 / s,
            (ox + g.width) as f32 / s,
            (oy + g.height) as f32 / s,
        ];
        self.map.insert((ch, px), uv);
        Ok(uv)
    }
}

/// 字形图集的 GPU 侧：常驻纹理 + 绑定组（与主图集**同一布局、同一管线**，
/// 只是换一张纹理）+ 纯簿记的 [`GlyphAtlas`]。
struct GlyphTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    atlas: GlyphAtlas,
}

impl GlyphTexture {
    fn new(
        device: &wgpu::Device,
        bind_layout: &wgpu::BindGroupLayout,
        globals_buf: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
    ) -> GlyphTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vitric-glyphs"),
            size: wgpu::Extent3d {
                width: GLYPH_ATLAS_SIZE,
                height: GLYPH_ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm, // 非 sRGB：与主图集同一字节空间
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vitric-glyphs-bind"),
            layout: bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });
        GlyphTexture { texture, bind_group, atlas: GlyphAtlas::new(GLYPH_ATLAS_SIZE) }
    }

    /// 消化本帧新出现的字形（增量上传，已有内容不动）。
    fn flush_uploads(&mut self, queue: &wgpu::Queue) {
        for up in self.atlas.pending.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: up.x, y: up.y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &up.pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(up.w * 4),
                    rows_per_image: Some(up.h),
                },
                wgpu::Extent3d { width: up.w, height: up.h, depth_or_array_layers: 1 },
            );
        }
    }
}

const WGSL: &str = r#"
struct Globals {
    viewport: vec4<f32>,                  // xy 视口尺寸 / z sRGB 标志 / w 光照开关
    ambient: vec4<f32>,                   // rgb 环境色 / w 灯数
    lights_pos: array<vec4<f32>, 64>,     // xy 灯心(像素) / z 半径(像素) / w kind(0 点/1 聚/2 平行)
    lights_color: array<vec4<f32>, 64>,   // rgb 已乘 intensity
    lights_dir: array<vec4<f32>, 64>,     // xy 朝向单位向量(像素空间) / z 半锥角(弧度)，spot 用
    shadow_ranges: array<vec4<f32>, 64>,  // 每盏灯的遮挡区间 [起点, 条数, 0, 0]（关闭 = 0）
    occluders: array<vec4<f32>, 256>,     // 合并大箱 AABB [x0, y0, x1, y1]，按灯连续
    occluder_sub_ranges: array<vec4<f32>, 256>, // 大箱的子箱区间 [起点, 条数, 0, 0]
    occluder_subs: array<vec4<f32>, 256>, // 原始遮光体 AABB（全局一份）
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) nuv: vec2<f32>,    // 法线贴图 UV；x<0 = 哨兵（没有法线）
    @location(3) rotcs: vec2<f32>,  // Sprite.rot 的 (cos, sin)
};

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) nuv: vec2<f32>,
    @location(4) rotcs: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    // 像素坐标（左上原点）→ NDC
    let ndc = vec2<f32>(
        pos.x / globals.viewport.x * 2.0 - 1.0,
        1.0 - pos.y / globals.viewport.y * 2.0,
    );
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    out.nuv = nuv;
    out.rotcs = rotcs;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// 线段 p → p+d 与 AABB b 的相交判定（slab 法，与 CPU 路径 vitric-render 的
// segment_hits_aabb 逐句同构——轴平行分量不做除法，显式分支，两侧 inf 语义才不会漂）。
fn seg_hits(p: vec2<f32>, d: vec2<f32>, b: vec4<f32>) -> bool {
    var tmin = 0.0;
    var tmax = 1.0;
    if (abs(d.x) < 1e-6) {
        if (p.x < b.x || p.x > b.z) { return false; }
    } else {
        let t1 = (b.x - p.x) / d.x;
        let t2 = (b.z - p.x) / d.x;
        tmin = max(tmin, min(t1, t2));
        tmax = min(tmax, max(t1, t2));
    }
    if (abs(d.y) < 1e-6) {
        if (p.y < b.y || p.y > b.w) { return false; }
    } else {
        let t1 = (b.y - p.y) / d.y;
        let t2 = (b.w - p.y) / d.y;
        tmin = max(tmin, min(t1, t2));
        tmax = min(tmax, max(t1, t2));
    }
    return tmax >= tmin;
}

// 投影：像素 p → 灯心 l 的线段是否被第 li 盏灯的遮挡候选挡住（候选 = 合并 +
// 逐灯剔除后的大箱区间，CPU 端打包，与 vitric-render 的 blocked 同构）。
// 大箱外的像素只测大箱（贴齐合并保证并集 == 大箱）；大箱内的像素回落到子箱——
// 像素自己所在的箱子跳过：箱子里的像素只被别的箱子遮挡。
// shadow_ranges[li].y = 0（投影关闭/候选剔空）时循环零次，零成本。
fn shadowed(p: vec2<f32>, l: vec2<f32>, li: u32) -> bool {
    let range = globals.shadow_ranges[li];
    let off = u32(range.x);
    let cnt = u32(range.y);
    let d = l - p;
    for (var k = off; k < off + cnt; k = k + 1u) {
        let m = globals.occluders[k];
        if (p.x >= m.x && p.x <= m.z && p.y >= m.y && p.y <= m.w) {
            let sr = globals.occluder_sub_ranges[k];
            let soff = u32(sr.x);
            let scnt = u32(sr.y);
            for (var s = soff; s < soff + scnt; s = s + 1u) {
                let b = globals.occluder_subs[s];
                if (p.x >= b.x && p.x <= b.z && p.y >= b.y && p.y <= b.w) {
                    continue;
                }
                if (seg_hits(p, d, b)) {
                    return true;
                }
            }
        } else if (seg_hits(p, d, m)) {
            return true;
        }
    }
    return false;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(atlas_tex, atlas_samp, in.uv) * in.color;
    // 法线贴图采样必须在 uniform control flow 里（textureSample 的硬约束）——
    // 无条件采样：哨兵 nuv(-1,-1) 被 ClampToEdge 采到图集 (0,0) 角，结果在分支里整个丢弃
    let nraw = textureSample(atlas_tex, atlas_samp, in.nuv).rgb * 2.0 - 1.0;
    // 光照——与 CPU 路径同一公式（vitric-render 模块文档）：
    //   lit = min(ambient + Σ 各灯贡献, 1.5)；out = min(c·lit, 1)
    //   point: color·I·(1-d/r)²；spot: 再乘角度衰减 t²，t = clamp(1 - Δθ/半锥角, 0, 1)
    //   （刻意用 t²，不用 smoothstep 内建——CPU 侧是同一条式子）；directional: color·I 处处均匀。
    //   法线像素（nuv.x ≥ 0）各灯贡献额外 ×= max(dot(N, L), 0)；L 的 xy 取像素指向灯的
    //   单位方向 ×0.8、z 固定 0.6（NORMAL_LIGHT_XY/Z，语义源头在 vitric-render）。
    // 必须在 sRGB 反算**之前**：CPU 是直接在 sRGB 字节上乘的，这里要在同一数值空间打光，
    // 两条路径才长一个样。in.pos 是帧缓冲像素坐标（中心 +0.5），和 CPU 的像素中心一致。
    // nuv.x < -1.5 = 自发光哨兵（粒子）：整个跳过光照——CPU 路径粒子画在光照之后，
    // 这里同语义；nuv.x = -1（普通无法线）照旧被打光，旧内容行为不变。
    if (globals.viewport.w > 0.5 && in.nuv.x > -1.5) {
        let has_n = in.nuv.x >= 0.0;
        var nrm = vec3<f32>(0.0, 0.0, 1.0);
        if (has_n) {
            // 解码（与 CPU sample_normal 一字不差）：z 取绝对值，xy 按精灵旋转
            // （局部→屏幕 [[c, s], [-s, c]]），零向量退化为平面法线
            var v = vec3<f32>(nraw.xy, abs(nraw.z));
            v = vec3<f32>(
                in.rotcs.x * v.x + in.rotcs.y * v.y,
                -in.rotcs.y * v.x + in.rotcs.x * v.y,
                v.z,
            );
            let len = length(v);
            if (len > 1e-6) {
                nrm = v / len;
            }
        }
        var lit = globals.ambient.rgb;
        let n = u32(globals.ambient.w);
        for (var i = 0u; i < n; i = i + 1u) {
            let lp = globals.lights_pos[i];
            if (lp.w > 1.5) {
                // directional：哨兵像素与距离/方向无关（老行为）；法线像素按 dir 算
                //   L = (-行进方向单位向量·0.8, 0.6)——与 CPU 同一构造
                var fd = 1.0;
                if (has_n) {
                    fd = max(dot(nrm, vec3<f32>(-globals.lights_dir[i].xy * 0.8, 0.6)), 0.0);
                }
                lit = lit + globals.lights_color[i].rgb * fd;
                continue;
            }
            let d = distance(in.pos.xy, lp.xy);
            if (d < lp.z) {
                var f = (1.0 - d / lp.z) * (1.0 - d / lp.z);
                if (lp.w > 0.5) {
                    // spot：Δθ = acos(像素方向 · 朝向)；d=0 夹角无定义，约定锥心（t=1）
                    let ld = globals.lights_dir[i];
                    var cosd = 1.0;
                    if (d > 0.0) {
                        cosd = clamp(dot((in.pos.xy - lp.xy) / d, ld.xy), -1.0, 1.0);
                    }
                    let t = clamp(1.0 - acos(cosd) / ld.z, 0.0, 1.0);
                    f = f * t * t;
                }
                if (has_n) {
                    // L = 像素指向灯心的单位方向 ×0.8、z 固定 0.6；d=0 约定 (0,0,1)
                    var l = vec3<f32>(0.0, 0.0, 1.0);
                    if (d > 0.0) {
                        l = vec3<f32>((lp.xy - in.pos.xy) / d * 0.8, 0.6);
                    }
                    f = f * max(dot(nrm, l), 0.0);
                }
                // 贡献为零（锥外/背光面）的片元不做遮挡测试（CPU 路径同一顺序）。
                // 投影：被遮光体挡住 = 这盏灯零贡献（硬影；只 point/spot，directional
                // 在上面的分支里已经 continue 掉，永远走不到这里）
                if (f > 0.0 && !shadowed(in.pos.xy, lp.xy, i)) {
                    lit = lit + globals.lights_color[i].rgb * f;
                }
            }
        }
        lit = min(lit, vec3<f32>(1.5));
        c = vec4<f32>(min(c.rgb * lit, vec3<f32>(1.0)), c.a);
    }
    // 表面是 sRGB 格式时写入值按线性解释，这里反算一次让最终字节贴齐 CPU 路径
    if (globals.viewport.z > 0.5) {
        c = vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
"#;

/// 泛光下采样/阈值/盒式模糊共用一个 shader：radius=0 + threshold≥0 + 倍率 2 = 下采样提亮部；
/// radius>0 + threshold<0 + 倍率 1 = 单方向盒式模糊。全屏三角形顶点流，textureLoad 整数
/// 采样 + 手动 clamp——越界取边缘像素，与 CPU 路径的 clamp-to-edge 同语义。
const WGSL_BLOOM: &str = r#"
struct Post {
    params: vec4<f32>,   // x 阈值(0..1，<0 = 不做阈值) / y 模糊半径(像素) / zw 模糊方向
    params2: vec4<f32>,  // x 采样坐标倍率（下采样 pass 为 2，其余 1）
};
@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var src: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // 全屏三角形（3 个顶点盖满裁剪空间，不需要顶点缓冲）
    var out: VsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.uv = uv;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src));
    let r = i32(post.params.y);
    let dir = vec2<i32>(vec2<f32>(post.params.z, post.params.w));
    let scale = i32(post.params2.x);
    let base = vec2<i32>(in.pos.xy) * scale;
    var sum = vec3<f32>(0.0);
    for (var k: i32 = -r; k <= r; k = k + 1) {
        let p = clamp(base + dir * k, vec2<i32>(0), dims - vec2<i32>(1));
        var c = textureLoad(src, p, 0).rgb;
        if (post.params.x >= 0.0) {
            // 亮部提取：max(scene - threshold, 0)。纹理值 0..1，阈值直接用 0..1
            //（CPU 在 0..255 字节域做同一减法，数值等价）
            c = max(c - vec3<f32>(post.params.x), vec3<f32>(0.0));
        }
        sum = sum + c;
    }
    return vec4<f32>(sum / f32(2 * r + 1), 1.0);
}
"#;

/// 泛光合成：场景 + 模糊亮部·strength，夹回 1。sRGB 表面的反算只在这最后一步做
/// （场景 pass 写的是原始字节空间的离屏纹理）。泛光纹理是半分辨率，用双线性采样
/// 放大——比最近邻平滑，光晕不出马赛克（与 CPU 的全分辨率模糊视觉对齐）。
const WGSL_COMPOSITE: &str = r#"
struct Comp {
    params: vec4<f32>,   // x strength / y "表面是 sRGB"标志
};
@group(0) @binding(0) var<uniform> comp: Comp;
@group(0) @binding(1) var scene_tex: texture_2d<f32>;
@group(0) @binding(2) var bloom_tex: texture_2d<f32>;
@group(0) @binding(3) var bloom_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.uv = uv;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let scene = textureLoad(scene_tex, vec2<i32>(in.pos.xy), 0).rgb;
    let bloom = textureSample(bloom_tex, bloom_samp, in.uv).rgb;
    // out = min(scene + bloom·strength, 1) —— 与 CPU 路径同一加法合成公式
    var c = min(scene + bloom * comp.params.x, vec3<f32>(1.0));
    if (comp.params.y > 0.5) {
        c = srgb_to_linear(c);
    }
    return vec4<f32>(c, 1.0);
}
"#;

/// 泛光后效 pass 的 uniform（下采样/模糊/合成三种 pass 共用布局，字段含义见 WGSL 注释）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct PostParams {
    params: [f32; 4],
    params2: [f32; 4],
}

pub struct GpuPresenter {
    // surface 持有窗口句柄引用，window 放前面只是顺手；'static 因为传的是 Arc<Window>
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    /// 图集纹理换了要重建绑定组，所以两者一起存。
    bind_group: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    atlas: Atlas,
    /// 矢量字体的动态字形图集（纹理 + 绑定组 + 簿记）。None = 项目没挂字体。
    glyphs: Option<GlyphTexture>,
    /// 已见过的素材代次——和 Dispatcher::assets_generation 比，变了就重建图集。
    seen_generation: u64,
    /// 顶点缓冲按需扩容（容量字节数）。
    vertex_buf: wgpu::Buffer,
    vertex_cap: u64,
    /// 背景清屏色（按表面格式换算好的线性/原始值）。
    clear: wgpu::Color,
    /// 主 shader 模块留着——泛光的离屏场景管线懒建时复用，不重新编译。
    scene_shader: wgpu::ShaderModule,
    /// 泛光管线组：第一次场上出现 Bloom 实体时懒建（不开泛光永远不建，零开销）。
    bloom_pipes: Option<BloomPipelines>,
    /// 泛光离屏纹理组：窗口尺寸变了重建（管线组不动）。
    bloom_targets: Option<BloomTargets>,
    window: Arc<Window>,
}

impl GpuPresenter {
    /// 初始化 wgpu 全家桶 + 首版图集。任何一步失败都返回带上下文的错误，
    /// 由调用方决定怎么退出（项目哲学：失败要显式，不做静默回退）。
    pub fn new(window: Arc<Window>, assets: &Assets, generation: u64) -> Result<Self, String> {
        // 不带 display handle：Vulkan/DX12/Metal 用不上它（只有 GL 需要），
        // from_env 保留 WGPU_BACKEND 等环境变量调试口
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("创建 GPU 表面失败: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("找不到可用的 GPU 适配器: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vitric"),
            ..Default::default()
        }))
        .map_err(|e| format!("创建 GPU 设备失败: {e}"))?;

        // 表面格式：优先非 sRGB（写入字节即所见，和 CPU 路径同一字节空间）；
        // 只有 sRGB 可选时走 shader 反算（见 WGSL 注释）
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("GPU 表面不支持任何像素格式")?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo, // 垂直同步，全后端可用
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // 背景色与 CPU 路径同字节（vitric_render::BACKGROUND）；sRGB 表面的清屏色按线性解释，要先反算
        let [bg_r, bg_g, bg_b, _] = vitric_render::BACKGROUND;
        let bg = [bg_r as f64 / 255.0, bg_g as f64 / 255.0, bg_b as f64 / 255.0];
        let to_linear = |c: f64| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let clear = if format.is_srgb() {
            wgpu::Color { r: to_linear(bg[0]), g: to_linear(bg[1]), b: to_linear(bg[2]), a: 1.0 }
        } else {
            wgpu::Color { r: bg[0], g: bg[1], b: bg[2], a: 1.0 }
        };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vitric-2d"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vitric-bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    // 最近邻采样不过滤——像素风贴图放大不糊，和 CPU 最近邻一致
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let pipeline = create_scene_pipeline(&device, &shader, &bind_layout, format);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vitric-nearest"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vitric-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (atlas, bind_group) =
            build_atlas(&device, &queue, &bind_layout, &globals_buf, &sampler, assets)?;
        // 项目挂了矢量字体才建字形图集（没挂 = 零开销，点阵字形已在主图集里）
        let glyphs = assets
            .font()
            .is_some()
            .then(|| GlyphTexture::new(&device, &bind_layout, &globals_buf, &sampler));

        const INITIAL_VB: u64 = 64 * 1024;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vitric-vertices"),
            size: INITIAL_VB,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(GpuPresenter {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_layout,
            bind_group,
            globals_buf,
            sampler,
            atlas,
            glyphs,
            seen_generation: generation,
            vertex_buf,
            vertex_cap: INITIAL_VB,
            clear,
            scene_shader: shader,
            bloom_pipes: None,
            bloom_targets: None,
            window,
        })
    }

    /// 呈现一帧。素材代次变了先重建图集（热重载素材后第一帧生效）。
    /// `tick` 喂给屏幕抖动取景（vitric_render::camera_of）——和 CPU 路径抖得一致。
    pub fn present(
        &mut self,
        world: &World,
        assets: &Assets,
        generation: u64,
        selection: Option<vitric_ecs::EntityId>,
        tick: u64,
    ) -> Result<(), String> {
        if generation != self.seen_generation {
            let (atlas, bind_group) = build_atlas(
                &self.device,
                &self.queue,
                &self.bind_layout,
                &self.globals_buf,
                &self.sampler,
                assets,
            )?;
            self.atlas = atlas;
            self.bind_group = bind_group;
            // 字体可能也被热重载换了/挂上/摘掉：字形图集整个重建（按新字体重新栅格化）
            self.glyphs = assets.font().is_some().then(|| {
                GlyphTexture::new(&self.device, &self.bind_layout, &self.globals_buf, &self.sampler)
            });
            self.seen_generation = generation;
        }

        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(()); // 最小化等零尺寸状态，跳帧
        }
        if size.width != self.config.width || size.height != self.config.height {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
        let (w, h) = (size.width, size.height);

        // 顶点流：没挂字体 = 老的单流单 draw；挂了字体 = 精灵流（主图集）+
        // 字形流（字形图集，含选中描边——它用字形图集的白块），按 glyph_from 切两段
        let (verts, glyph_from) = if let Some(font) = assets.font() {
            let gt = self.glyphs.as_mut().ok_or(
                "内部不一致：素材仓库挂了字体但字形图集未建（素材代次没推进？）",
            )?;
            let mut verts = build_scene_vertices(world, w, h, &self.atlas, tick)?;
            let split = verts.len();
            let cam = vitric_render::camera_of(world, tick, h)?;
            push_ttf_texts(&mut verts, world, w, h, font, &mut gt.atlas, cam)?;
            // 粒子在文字之后、描边之前（同 build_vertices）；split 之后的顶点绑
            // 字形图集，所以白块用字形图集那份（同一布局同一管线，只是换纹理）
            push_emitter_particles(&mut verts, world, w, h, gt.atlas.white, cam, tick)?;
            if let Some(id) = selection {
                push_selection_outline(&mut verts, world, w, h, gt.atlas.white, id, cam);
            }
            (verts, split)
        } else {
            let verts = build_vertices(world, w, h, &self.atlas, selection, tick)?;
            let split = verts.len();
            (verts, split)
        };
        // 本帧新字形增量上传（write_texture 在 submit 前排队，draw 时必然就绪）
        if let Some(gt) = self.glyphs.as_mut() {
            gt.flush_uploads(&self.queue);
        }

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Timeout | Cst::Occluded => return Ok(()), // 偶发/被遮挡，跳一帧
            Cst::Outdated | Cst::Lost => {
                // 窗口刚变化/表面失效：重配再试一次，还不行就是真故障
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    Cst::Success(f) | Cst::Suboptimal(f) => f,
                    other => return Err(format!("GPU 表面取帧失败: {other:?}")),
                }
            }
            Cst::Validation => return Err("GPU 表面取帧失败: 校验错误".to_string()),
        };

        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        if bytes.len() as u64 > self.vertex_cap {
            self.vertex_cap = (bytes.len() as u64).next_power_of_two();
            self.vertex_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vitric-vertices"),
                size: self.vertex_cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.vertex_buf, 0, bytes);
        }
        // 泛光开关 = 场上有没有 Bloom 实体（语义源头在 vitric-render，参数校验也在那边）。
        // 开了泛光时场景 pass 渲进非 sRGB 的离屏纹理，sRGB 反算挪到合成 pass 做
        let bloom = vitric_render::bloom_of(world)?;
        let surface_srgb = self.config.format.is_srgb();
        let globals =
            build_globals(world, w, h, scene_pass_srgb(surface_srgb, bloom.is_some()), tick)?;
        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        match &bloom {
            None => {
                // 老路径原样保留：单 pass 直渲表面，没有任何额外纹理/pass 开销
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("vitric-frame"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(self.clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                if !verts.is_empty() {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_vertex_buffer(0, self.vertex_buf.slice(..bytes.len() as u64));
                    draw_split(&mut pass, &self.bind_group, self.glyphs.as_ref(), verts.len(), glyph_from);
                }
            }
            Some(b) => {
                // 多 pass 结构见模块文档。管线组懒建一次；纹理组随窗口尺寸重建
                if self.bloom_pipes.is_none() {
                    self.bloom_pipes = Some(BloomPipelines::new(
                        &self.device,
                        &self.scene_shader,
                        &self.bind_layout,
                        self.config.format,
                    ));
                }
                let pipes = self.bloom_pipes.as_ref().expect("上面刚建过");
                if self.bloom_targets.as_ref().map(|t| t.size) != Some((w, h)) {
                    self.bloom_targets = Some(BloomTargets::new(&self.device, pipes, (w, h)));
                }
                let targets = self.bloom_targets.as_ref().expect("上面刚建过");

                // 每帧写小 uniform（参数可能被规则/脚本改，纹理绑定不变）
                for (buf, p) in
                    targets.post_bufs.iter().zip(bloom_pass_params(h, b.threshold))
                {
                    self.queue.write_buffer(buf, 0, bytemuck::bytes_of(&p));
                }
                let comp = PostParams {
                    params: [b.strength as f32, if surface_srgb { 1.0 } else { 0.0 }, 0.0, 0.0],
                    params2: [0.0; 4],
                };
                self.queue.write_buffer(&targets.composite_buf, 0, bytemuck::bytes_of(&comp));

                // 场景 pass → 离屏（清屏色用原始字节值：离屏纹理永远是非 sRGB 格式）
                {
                    let [r, g, bl, _] = vitric_render::BACKGROUND;
                    let clear_raw = wgpu::Color {
                        r: r as f64 / 255.0,
                        g: g as f64 / 255.0,
                        b: bl as f64 / 255.0,
                        a: 1.0,
                    };
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("vitric-bloom-scene"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &targets.scene_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(clear_raw),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    if !verts.is_empty() {
                        pass.set_pipeline(&pipes.scene_pipeline);
                        pass.set_vertex_buffer(0, self.vertex_buf.slice(..bytes.len() as u64));
                        draw_split(&mut pass, &self.bind_group, self.glyphs.as_ref(), verts.len(), glyph_from);
                    }
                }
                // 下采样+阈值 + 模糊 ×6（全屏三角形，目标在 post_views 里排好了）
                for (i, (bind, dst)) in
                    targets.post_binds.iter().zip(&targets.post_views).enumerate()
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some(if i == 0 { "vitric-bloom-bright" } else { "vitric-bloom-blur" }),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: dst,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&pipes.post_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                // 合成 pass → 表面（sRGB 反算在这里做，全屏三角形盖满，清屏色无所谓）
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("vitric-bloom-composite"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(self.clear),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&pipes.composite_pipeline);
                    pass.set_bind_group(0, &targets.composite_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }
}

/// 场景 pass 要不要在 shader 里做 sRGB 反算：泛光开启时不做（场景写进非 sRGB 离屏纹理，
/// 反算挪到合成 pass）；关闭时维持老行为（直渲表面，表面是 sRGB 才反算）。
/// 拆成纯函数：pass 选择逻辑无 GPU 也可单测。
fn scene_pass_srgb(surface_srgb: bool, bloom_active: bool) -> bool {
    surface_srgb && !bloom_active
}

/// 泛光 7 个后效 pass 的参数（纯函数无 GPU，被单测锁住）：
/// [0] 下采样+阈值（radius 0、采样倍率 2——半分辨率像素映射回全分辨率场景），
/// [1..=6] 盒式模糊 H/V 交替 3 轮，半径 = CPU 半径的一半（下限 1，因为在半分辨率上跑，
/// 空间尺度与 CPU 全分辨率模糊一致），threshold 置 -1 表示不再做阈值。
fn bloom_pass_params(viewport_h: u32, threshold: f64) -> [PostParams; 7] {
    let r_half = (vitric_render::bloom_radius_px(viewport_h) / 2).max(1) as f32;
    let mut out = [PostParams { params: [0.0; 4], params2: [0.0; 4] }; 7];
    out[0] = PostParams {
        params: [threshold as f32, 0.0, 0.0, 0.0],
        params2: [2.0, 0.0, 0.0, 0.0],
    };
    for i in 0..6 {
        // 偶数下标水平、奇数垂直（从 0 数）：H→V 为一轮完整可分离模糊
        let dir = if i % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
        out[i + 1] = PostParams {
            params: [-1.0, r_half, dir[0], dir[1]],
            params2: [1.0, 0.0, 0.0, 0.0],
        };
    }
    out
}

/// 泛光中间纹理的半分辨率尺寸（下限 1，极小窗口不归零）。
fn half_dims(w: u32, h: u32) -> (u32, u32) {
    ((w / 2).max(1), (h / 2).max(1))
}

/// 泛光管线组：与窗口尺寸无关的部分，懒建一次后常驻。
struct BloomPipelines {
    /// 主 shader 渲进离屏 Rgba8Unorm 的变体（直渲表面那条管线照旧在 GpuPresenter.pipeline）。
    scene_pipeline: wgpu::RenderPipeline,
    post_layout: wgpu::BindGroupLayout,
    post_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    /// 双线性采样器：合成时把半分辨率泛光纹理平滑放大（见 WGSL_COMPOSITE 注释）。
    linear_sampler: wgpu::Sampler,
}

impl BloomPipelines {
    fn new(
        device: &wgpu::Device,
        scene_shader: &wgpu::ShaderModule,
        scene_bind_layout: &wgpu::BindGroupLayout,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let scene_pipeline =
            create_scene_pipeline(device, scene_shader, scene_bind_layout, BLOOM_FORMAT);

        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding: u32, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };

        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vitric-bloom-post-bind"),
            entries: &[uniform_entry(0), texture_entry(1, false)],
        });
        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vitric-bloom-post"),
            source: wgpu::ShaderSource::Wgsl(WGSL_BLOOM.into()),
        });
        let post_pipeline =
            create_fullscreen_pipeline(device, &post_shader, &post_layout, BLOOM_FORMAT, "post");

        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vitric-bloom-composite-bind"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, false),
                texture_entry(2, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vitric-bloom-composite"),
            source: wgpu::ShaderSource::Wgsl(WGSL_COMPOSITE.into()),
        });
        let composite_pipeline = create_fullscreen_pipeline(
            device,
            &composite_shader,
            &composite_layout,
            surface_format,
            "composite",
        );

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vitric-bloom-linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        BloomPipelines {
            scene_pipeline,
            post_layout,
            post_pipeline,
            composite_layout,
            composite_pipeline,
            linear_sampler,
        }
    }
}

/// 泛光离屏纹理组：随窗口尺寸重建（present 里比对 size 触发）。
struct BloomTargets {
    size: (u32, u32),
    scene_view: wgpu::TextureView,
    /// 7 个后效 pass 的 uniform / 绑定组 / 目标视图（下标对齐 bloom_pass_params）。
    post_bufs: Vec<wgpu::Buffer>,
    post_binds: Vec<wgpu::BindGroup>,
    post_views: Vec<wgpu::TextureView>,
    composite_buf: wgpu::Buffer,
    composite_bind: wgpu::BindGroup,
}

impl BloomTargets {
    fn new(device: &wgpu::Device, pipes: &BloomPipelines, (w, h): (u32, u32)) -> Self {
        let make_tex = |label: &str, tw: u32, th: u32| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: BLOOM_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        };
        let scene_view = make_tex("vitric-bloom-scene", w, h);
        let (hw, hh) = half_dims(w, h);
        let ping_view = make_tex("vitric-bloom-ping", hw, hh);
        let pong_view = make_tex("vitric-bloom-pong", hw, hh);

        let make_buf = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<PostParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        // pass 链：下采样(scene→ping)，然后 H/V 交替（ping→pong→ping…），3 轮后落在 ping。
        // 源/目标视图的排布必须与 bloom_pass_params 的方向序一致
        let mut post_bufs = Vec::with_capacity(7);
        let mut post_binds = Vec::with_capacity(7);
        let mut post_views = Vec::with_capacity(7);
        for i in 0..7usize {
            let buf = make_buf("vitric-bloom-post-params");
            let src = match i {
                0 => &scene_view,
                odd if odd % 2 == 1 => &ping_view,
                _ => &pong_view,
            };
            post_binds.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vitric-bloom-post-bind"),
                layout: &pipes.post_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                ],
            }));
            post_views.push(if i % 2 == 0 { ping_view.clone() } else { pong_view.clone() });
            post_bufs.push(buf);
        }

        let composite_buf = make_buf("vitric-bloom-composite-params");
        let composite_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vitric-bloom-composite-bind"),
            layout: &pipes.composite_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: composite_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&scene_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    // 模糊链 3 轮 H/V 后的最终结果在 ping（见上面 pass 链注释）
                    resource: wgpu::BindingResource::TextureView(&ping_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&pipes.linear_sampler),
                },
            ],
        });

        BloomTargets {
            size: (w, h),
            scene_view,
            post_bufs,
            post_binds,
            post_views,
            composite_buf,
            composite_bind,
        }
    }
}

/// 泛光全部离屏纹理的格式：非 sRGB——所有中间计算都在原始字节空间，
/// 和 CPU 路径同一数值域（sRGB 反算只在最后的合成 pass 做一次）。
const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// 主 2D 管线（顶点流 + 图集 + 光照 shader）。直渲表面和泛光的离屏场景 pass
/// 用同一个 shader、同一套顶点布局，只有目标格式不同——抽出来建两次。
fn create_scene_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vitric-pipeline-layout"),
        bind_group_layouts: &[Some(bind_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vitric-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // 标准 src-alpha 混合，对齐 CPU 路径的逐像素 alpha 混合
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None, // 画家算法：按实体序后画盖前画，不要深度
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// 后效管线（泛光模糊/合成共用骨架）：全屏三角形、无顶点缓冲、无混合（直接覆盖）。
fn create_fullscreen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bind_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

// ---------------------------------------------------------------------------
// 图集构建：白块 + 128 字形 + 全部素材，一次性打进一张纹理
// ---------------------------------------------------------------------------

/// 图集固定宽度：单图上限 2048（Assets 导入时拦的）+ 两侧 1px 复制边。
const ATLAS_W: u32 = 2050;

fn build_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bind_layout: &wgpu::BindGroupLayout,
    globals_buf: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    assets: &Assets,
) -> Result<(Atlas, wgpu::BindGroup), String> {
    // 待打包项：(键, 宽, 高, RGBA 像素)。顺序固定：白块、字形、素材（BTreeMap 已排序）
    let mut items: Vec<(String, u32, u32, Vec<u8>)> = Vec::new();
    items.push(("\u{0}white".into(), 2, 2, vec![255u8; 2 * 2 * 4]));
    for (i, glyph) in font8x8::legacy::BASIC_LEGACY.iter().enumerate() {
        let mut px = vec![0u8; 8 * 8 * 4];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << col) != 0 {
                    let o = (row * 8 + col) * 4;
                    px[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        items.push((format!("\u{0}glyph{i}"), 8, 8, px));
    }
    for name in assets.names() {
        let img = assets.image(name).expect("names() 给出的名字必然存在");
        items.push((name.to_string(), img.width, img.height, img.rgba.clone()));
    }

    // 货架打包：每项四周留 1px 复制边（最近邻采样在区域边缘的浮点抖动不会渗到邻图）
    let mut placements: Vec<(u32, u32)> = Vec::with_capacity(items.len()); // 内容区左上角
    let (mut x, mut y, mut row_h) = (0u32, 0u32, 0u32);
    for (_, w, h, _) in &items {
        let (bw, bh) = (w + 2, h + 2);
        if x + bw > ATLAS_W {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        placements.push((x + 1, y + 1));
        row_h = row_h.max(bh);
        x += bw;
    }
    let atlas_h = (y + row_h).max(1);
    let max_dim = device.limits().max_texture_dimension_2d;
    if atlas_h > max_dim {
        return Err(format!(
            "素材图集需要 {ATLAS_W}x{atlas_h}，超过本机 GPU 纹理上限 {max_dim}。\
             提示：减少/缩小素材，或改用 --renderer cpu"
        ));
    }

    // 铺像素：内容区直拷，复制边取最近内容像素（等效 clamp）
    let mut pixels = vec![0u8; (ATLAS_W * atlas_h * 4) as usize];
    for ((_, w, h, src), (ox, oy)) in items.iter().zip(&placements) {
        for ty in -1..=(*h as i64) {
            for tx in -1..=(*w as i64) {
                let sx = tx.clamp(0, *w as i64 - 1) as usize;
                let sy = ty.clamp(0, *h as i64 - 1) as usize;
                let s = (sy * *w as usize + sx) * 4;
                let dx = (*ox as i64 + tx) as usize;
                let dy = (*oy as i64 + ty) as usize;
                let d = (dy * ATLAS_W as usize + dx) * 4;
                pixels[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vitric-atlas"),
        size: wgpu::Extent3d { width: ATLAS_W, height: atlas_h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm, // 非 sRGB：采样回原始字节，染色乘法在字节空间
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(ATLAS_W * 4),
            rows_per_image: Some(atlas_h),
        },
        wgpu::Extent3d { width: ATLAS_W, height: atlas_h, depth_or_array_layers: 1 },
    );

    // UV 索引表
    let uv = |ox: u32, oy: u32, w: u32, h: u32| -> UvRect {
        [
            ox as f32 / ATLAS_W as f32,
            oy as f32 / atlas_h as f32,
            (ox + w) as f32 / ATLAS_W as f32,
            (oy + h) as f32 / atlas_h as f32,
        ]
    };
    let mut images = std::collections::BTreeMap::new();
    let mut white = [0.0f32; 2];
    let mut glyphs = [[0.0f32; 4]; 128];
    for ((key, w, h, _), (ox, oy)) in items.iter().zip(&placements) {
        if key == "\u{0}white" {
            // 白块取中心点（2x2 的正中），四角同 UV 平采样
            white = [(*ox as f32 + 1.0) / ATLAS_W as f32, (*oy as f32 + 1.0) / atlas_h as f32];
        } else if let Some(i) = key.strip_prefix('\u{0}').and_then(|k| k.strip_prefix("glyph")) {
            glyphs[i.parse::<usize>().expect("字形键自己拼的")] = uv(*ox, *oy, *w, *h);
        } else {
            images.insert(key.clone(), uv(*ox, *oy, *w, *h));
        }
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vitric-bind"),
        layout: bind_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    });
    Ok((Atlas { images, white, glyphs }, bind_group))
}

// ---------------------------------------------------------------------------
// 每帧顶点流：镜像 vitric-render 的视觉语义（同一套组件约定、同一坐标变换）
// ---------------------------------------------------------------------------

/// 攒一帧的 uniform（纯函数不碰 GPU，光照打包布局被单测锁住）。
/// 灯参数在这里从世界坐标变换到像素空间（取景含 Shake 抖动——光跟着画面走，
/// 和 CPU 路径的 apply_lighting 同一套变换）；shader 内循环只算距离。
fn build_globals(
    world: &World,
    width: u32,
    height: u32,
    srgb: bool,
    tick: u64,
) -> Result<Globals, String> {
    let mut g = Globals {
        viewport: [width as f32, height as f32, if srgb { 1.0 } else { 0.0 }, 0.0],
        ..bytemuck::Zeroable::zeroed()
    };
    // 光照总开关 = 场上有没有 Ambient 实体（语义源头在 vitric-render）
    if let Some((ambient, _)) = vitric_render::ambient_of(world)? {
        let lights = vitric_render::collect_lights(world)?;
        let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick, height)?;
        g.viewport[3] = 1.0;
        g.ambient = [ambient[0] as f32, ambient[1] as f32, ambient[2] as f32, lights.len() as f32];
        for (i, l) in lights.iter().enumerate() {
            // 打包布局见 Globals 文档：pos.w = kind，dir = [朝向像素空间单位向量, 半锥角弧度, 0]
            let (kind, dir_deg, half_rad) = match l.kind {
                vitric_render::LightKind::Point => (0.0, None, 0.0),
                vitric_render::LightKind::Spot { angle, dir } => {
                    (1.0, Some(dir), (angle / 2.0).to_radians())
                }
                vitric_render::LightKind::Directional { dir } => (2.0, Some(dir), 0.0),
            };
            // 平行光不过世界→像素变换（占位 0 喂进变换会得到屏幕中心，污染槽位语义）——
            // 位置/半径直接打 0，shader 在 kind 分支里根本不碰它们
            g.lights_pos[i] = if matches!(l.kind, vitric_render::LightKind::Directional { .. }) {
                [0.0, 0.0, 0.0, kind]
            } else {
                [
                    ((width as f64) / 2.0 + (l.x - cam_x) * scale) as f32,
                    ((height as f64) / 2.0 - (l.y - cam_y) * scale) as f32,
                    (l.radius * scale) as f32,
                    kind,
                ]
            };
            g.lights_color[i] = [
                (l.rgb[0] * l.intensity) as f32,
                (l.rgb[1] * l.intensity) as f32,
                (l.rgb[2] * l.intensity) as f32,
                0.0,
            ];
            if let Some(dir) = dir_deg {
                // 世界角度（度，0=+x 逆时针正）→ 像素空间单位向量：y 翻转 → (cos, -sin)，
                // 和 CPU 路径 apply_lighting 的预计算一字不差
                let rad = dir.to_radians();
                g.lights_dir[i] = [rad.cos() as f32, (-rad.sin()) as f32, half_rad as f32, 0.0];
            }
        }
        // 投影：遮光体合并成大箱（像素空间，与灯同一套取景变换，含 Shake 抖动）、
        // 按灯盘逐灯剔除后打成连续区间。开关/收集/上限/合并/剔除的语义源头全在
        // vitric-render；关闭 = shadow_ranges 全零，shader 的遮挡循环零次执行
        if vitric_render::shadows_of(world)? {
            let occs = vitric_render::collect_occluders(world)?;
            let grid = vitric_render::build_shadow_boxes(&occs, width, height, (cam_x, cam_y, scale));
            for (s, b) in grid.subs.iter().enumerate() {
                g.occluder_subs[s] = [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32];
            }
            let mut cursor = 0usize;
            for (i, l) in lights.iter().enumerate() {
                // 平行光不投影（v1）：区间留全零，shader 循环零次
                if matches!(l.kind, vitric_render::LightKind::Directional { .. }) {
                    continue;
                }
                // 与 CPU 路径同一套 f64 像素空间灯参数做剔除（f32 打包只发生在最后）
                let lx = (width as f64) / 2.0 + (l.x - cam_x) * scale;
                let ly = (height as f64) / 2.0 - (l.y - cam_y) * scale;
                let kept = vitric_render::cull_shadow_boxes(&grid, lx, ly, l.radius * scale);
                if kept.len() > SHADOW_PER_LIGHT {
                    return Err(format!(
                        "光源 {} 的半径内剔除后仍有 {} 个遮挡大箱，超过 GPU 每灯上限 \
                         {SHADOW_PER_LIGHT}。提示：减小 Light.radius、减少灯数，或把相邻的 \
                         Solid 摆贴齐（贴齐的瓦片会自动合并成一个大箱）",
                        l.id,
                        kept.len()
                    ));
                }
                if cursor + kept.len() > SHADOW_FLAT_BUDGET {
                    return Err(format!(
                        "全部灯的遮挡列表合计超过 GPU uniform 预算 {SHADOW_FLAT_BUDGET} 条\
                         （打包到光源 {} 时已有 {cursor} 条，还差 {} 条放不下）。\
                         提示：减少灯数/遮光体，或把相邻的 Solid 摆贴齐让它们合并",
                        l.id,
                        kept.len()
                    ));
                }
                g.shadow_ranges[i] = [cursor as f32, kept.len() as f32, 0.0, 0.0];
                for &k in &kept {
                    let m = &grid.merged[k as usize];
                    g.occluders[cursor] = [
                        m.aabb[0] as f32,
                        m.aabb[1] as f32,
                        m.aabb[2] as f32,
                        m.aabb[3] as f32,
                    ];
                    g.occluder_sub_ranges[cursor] =
                        [m.sub_start as f32, m.sub_len as f32, 0.0, 0.0];
                    cursor += 1;
                }
            }
        }
    }
    Ok(g)
}

/// 一段顶点流按 glyph_from 切两个 draw：前段绑主图集，后段绑字形图集
/// （没挂字体时 glyph_from == len，第二段为空，行为与单 draw 完全一致）。
fn draw_split(
    pass: &mut wgpu::RenderPass<'_>,
    main_bind: &wgpu::BindGroup,
    glyphs: Option<&GlyphTexture>,
    len: usize,
    glyph_from: usize,
) {
    if glyph_from > 0 {
        pass.set_bind_group(0, main_bind, &[]);
        pass.draw(0..glyph_from as u32, 0..1);
    }
    if glyph_from < len {
        let gt = glyphs.expect("有字形顶点必有字形图集（present 里建流时校验过）");
        pass.set_bind_group(0, &gt.bind_group, &[]);
        pass.draw(glyph_from as u32..len as u32, 0..1);
    }
}

/// 老的单流路径（没挂字体）：背景/精灵/点阵文字/描边全在主图集一段流里。
fn build_vertices(
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
    selection: Option<vitric_ecs::EntityId>,
    tick: u64,
) -> Result<Vec<Vertex>, String> {
    let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick, height)?;
    let mut verts = build_scene_vertices(world, width, height, atlas, tick)?;
    push_bitmap_texts(&mut verts, world, width, height, atlas, (cam_x, cam_y, scale))?;
    // 粒子在文字之后（CPU 路径粒子画在光照之后 = 盖在精灵/文字上，画家序同语义）
    push_emitter_particles(&mut verts, world, width, height, atlas.white, (cam_x, cam_y, scale), tick)?;
    // 选中描边：青色 2px，画在最上层（几何对齐 vitric_render::draw_selection_outline）。
    // 已知小偏差：光照开启时描边在 GPU 窗口会被一起打光（同一条管线），CPU 窗口路径
    // 是渲完再描所以不被打光——检查器调试装饰，不进截图/断言，不值得为它开第二条管线
    if let Some(id) = selection {
        push_selection_outline(&mut verts, world, width, height, atlas.white, id, (cam_x, cam_y, scale));
    }
    Ok(verts)
}

/// 背景（光照开启时）+ 精灵流（主图集）。文字/描边由调用方按路径拼接。
fn build_scene_vertices(
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
    tick: u64,
) -> Result<Vec<Vertex>, String> {
    // 取景（含 Shake 抖动偏移）直接用 vitric-render 的实现——两条路径抖得逐位一致
    let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick, height)?;
    let mut verts: Vec<Vertex> = Vec::new();

    // 光照开启时背景也要被照（CPU 路径是整张 buf 过公式）：清屏色没法逐像素变，
    // 所以先铺一个全屏背景方块，让 shader 把背景和实体一起打光
    if vitric_render::ambient_of(world)?.is_some() {
        push_solid(&mut verts, atlas.white, 0.0, 0.0, width as f32, height as f32, vitric_render::BACKGROUND);
    }

    // 精灵：按实体序（画家算法，后画盖前画）
    for id in world.query(&["Position", "Sprite"]) {
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        let rot = vitric_render::rot_of(world, id)?;
        // 世界 → 屏幕像素（y 翻转，相机居中）——与 CPU 路径同一公式
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;
        // 四角（顺序固定：未旋转时的 左上/右上/右下/左下，UV 跟角走）。
        // rot != 0 时绕中心旋转——与 CPU 路径同一角度约定（vitric_render::rot_of）：
        // 度数、世界逆时针为正；屏幕 y 翻转 → 屏幕系正向矩阵 [[c, s], [-s, c]]
        let corners: [[f32; 2]; 4] = if rot == 0.0 {
            let (x0, y0) = ((cx - half_w) as f32, (cy - half_h) as f32);
            let (x1, y1) = ((cx + half_w) as f32, (cy + half_h) as f32);
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        } else {
            let (sn, cs) = rot.to_radians().sin_cos();
            let p = |dx: f64, dy: f64| {
                [(cx + cs * dx + sn * dy) as f32, (cy - sn * dx + cs * dy) as f32]
            };
            [p(-half_w, -half_h), p(half_w, -half_h), p(half_w, half_h), p(-half_w, half_h)]
        };

        let image_name = world
            .get_field(id, "Sprite.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if image_name.is_empty() {
            let color = world
                .get_field(id, "Sprite.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Sprite.color: {e}"))?;
            let [u, v] = atlas.white;
            push_quad_corners(&mut verts, corners, [u, v, u, v], tint(rgba));
        } else {
            // 图不存在直接报错（不画占位符）——错误文案对齐 CPU 路径
            let uv = atlas.images.get(&image_name).ok_or_else(|| {
                format!(
                    "实体 {id} 的 Sprite.image {image_name:?} 不在素材仓库里。\
                     现有素材: [{}]。提示：图放进项目 assets/ 目录，路径相对 assets/ 写",
                    atlas.images.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            // 法线贴图按命名配对（语义源头 vitric_render::normal_map_name）——它就是
            // assets/ 里的普通 PNG，必然已在同一张图集里；没配对 = 哨兵 = 老光照路径
            let nuv = vitric_render::normal_map_name(&image_name)
                .and_then(|n| atlas.images.get(&n))
                .copied()
                .unwrap_or(NO_NORMAL);
            // rot 的 (cos, sin)：片元用它把法线旋到屏幕空间（rot=0 → (1,0) 恒等）
            let rotcs = if rot == 0.0 {
                [1.0, 0.0]
            } else {
                let (sn, cs) = rot.to_radians().sin_cos();
                [cs as f32, sn as f32]
            };
            push_quad_corners_n(&mut verts, corners, *uv, nuv, rotcs, [1.0; 4]);
        }
    }
    Ok(verts)
}

/// 点阵文字流（没挂字体的老路径）：每字符一个主图集字形方块，整串居中于 Position，
/// 画在精灵之上。永远直立——Sprite.rot 只转精灵，不转文字（与 CPU 路径同语义）。
fn push_bitmap_texts(
    verts: &mut Vec<Vertex>,
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
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
        // reveal：与 CPU 点阵路径同一口径——截到前 visible 个字符、按 visible 居中
        let reveal = world
            .get_field(id, "Text.reveal")
            .ok()
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        let visible = vitric_render::revealed_chars(reveal, content.chars().count());
        if visible == 0 {
            continue;
        }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        // screen=true: HUD 锚定——与 CPU 路径同语义,坐标相对屏幕中心,不随相机走
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let chars: Vec<char> = content.chars().take(visible).collect();
        let n = chars.len() as f64;
        let (cx, cy) = if screen_anchored {
            ((width as f64) / 2.0 + px * scale, (height as f64) / 2.0 - py * scale)
        } else {
            ((width as f64) / 2.0 + (px - cam_x) * scale, (height as f64) / 2.0 - (py - cam_y) * scale)
        };
        let char_w = size * scale;
        let left = cx - n * char_w / 2.0;
        let (y0, y1) = ((cy - char_w / 2.0) as f32, (cy + char_w / 2.0) as f32);
        for (i, &c) in chars.iter().enumerate() {
            let x0 = (left + i as f64 * char_w) as f32;
            let x1 = (left + (i + 1) as f64 * char_w) as f32;
            let cp = c as usize;
            if cp < 128 {
                push_quad(verts, x0, y0, x1, y1, atlas.glyphs[cp], tint(rgba));
            } else {
                // 非 ASCII：实心方块占位，同 CPU 路径
                push_solid(verts, atlas.white, x0, y0, x1, y1, rgba);
            }
        }
    }
    Ok(())
}

/// 矢量文字流（挂了字体）：排版/栅格化/取整全走 vitric_render::FontStore——
/// 与 CPU 路径（draw_text_vector）同一套几何，每字形一个字形图集方块。
/// 新 (字符, 像素字号) 在这里懒分配 + 排进上传队列；图集满了显式报错。
fn push_ttf_texts(
    verts: &mut Vec<Vertex>,
    world: &World,
    width: u32,
    height: u32,
    font: &vitric_render::FontStore,
    glyph_atlas: &mut GlyphAtlas,
    (cam_x, cam_y, scale): (f64, f64, f64),
) -> Result<(), String> {
    use vitric_render::FontStore;
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
        // reveal（与 CPU 路径同一口径）：可见字数 = 纯函数；缺省 1.0=全显
        let reveal = world
            .get_field(id, "Text.reveal")
            .ok()
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        let visible = vitric_render::revealed_chars(reveal, content.chars().count());
        if visible == 0 {
            continue;
        }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let (cx, cy) = if screen_anchored {
            ((width as f64) / 2.0 + px * scale, (height as f64) / 2.0 - py * scale)
        } else {
            ((width as f64) / 2.0 + (px - cam_x) * scale, (height as f64) / 2.0 - (py - cam_y) * scale)
        };

        let px_size = FontStore::px_size(size, scale);
        // 缓存版排版 + 只画前 visible 个字形：与 CPU 路径同一份数据、同一个可见字数
        let laid = font.layout_cached(&content, px_size);
        let (placements, total_w) = (&laid.0, laid.1);
        let left = cx - total_w as f64 / 2.0;
        let baseline = (cy + font.baseline_offset(px_size) as f64).round();
        for p in placements.iter().take(visible) {
            let g = font.raster(p.ch, px_size);
            if g.coverage.is_empty() {
                continue; // 空轮廓（空格等）只占 advance，不进图集
            }
            let uv = glyph_atlas.glyph_uv(font, p.ch, px_size)?;
            let x0 = ((left + p.x as f64).round() + g.left as f64) as f32;
            let y0 = (baseline + g.top as f64) as f32;
            push_quad(verts, x0, y0, x0 + g.width as f32, y0 + g.height as f32, uv, tint(rgba));
        }
    }
    Ok(())
}

/// 粒子流：发射器按纯函数展开（语义源头 vitric_render::emitter_particles——
/// 位置/数量/颜色与 CPU 路径同一份数据），每个粒子一个白块方块 + 染色。
/// nuv 用 [`UNLIT`] 哨兵：片元跳过光照（自发光，CPU 路径粒子画在光照之后，同语义）。
fn push_emitter_particles(
    verts: &mut Vec<Vertex>,
    world: &World,
    width: u32,
    height: u32,
    white: [f32; 2],
    (cam_x, cam_y, scale): (f64, f64, f64),
    tick: u64,
) -> Result<(), String> {
    for e in vitric_render::collect_emitters(world)? {
        for p in vitric_render::emitter_particles(&e, tick) {
            if p.rgba[3] == 0 {
                continue;
            }
            // 与 CPU 光栅化同一条世界→像素变换（方点中心 + 半边长）
            let cx = (width as f64) / 2.0 + (p.x - cam_x) * scale;
            let cy = (height as f64) / 2.0 - (p.y - cam_y) * scale;
            let half = p.size * scale / 2.0;
            let (x0, y0) = ((cx - half) as f32, (cy - half) as f32);
            let (x1, y1) = ((cx + half) as f32, (cy + half) as f32);
            let [u, v] = white;
            push_quad_corners_n(
                verts,
                [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                [u, v, u, v],
                UNLIT,
                [1.0, 0.0],
                tint(p.rgba),
            );
        }
    }
    Ok(())
}

fn push_selection_outline(
    verts: &mut Vec<Vertex>,
    world: &World,
    width: u32,
    height: u32,
    // 白块中心点 UV：点阵路径给主图集的，字体路径给字形图集的（描边跟字形同一绑定组）
    white: [f32; 2],
    id: vitric_ecs::EntityId,
    (cam_x, cam_y, scale): (f64, f64, f64),
) {
    if !world.is_alive(id) || !world.has_component(id, "Sprite") {
        return; // 选中的实体没了/不可见，静默跳过（同 CPU 路径）
    }
    let field = |path: &str| num(world, id, path).ok();
    let (Some(x), Some(y), Some(w), Some(h)) = (
        field("Position.x"),
        field("Position.y"),
        field("Sprite.w"),
        field("Sprite.h"),
    ) else {
        return; // 字段坏了不阻塞呈现（CPU 路径里调用方也是 let _ = 忽略）
    };
    // rot != 0 时取旋转后形状的轴对齐包围盒——与 CPU 路径同一选择（高亮不需要贴边精确）
    let rot = vitric_render::rot_of(world, id).unwrap_or(0.0);
    let (ew, eh) = if rot == 0.0 {
        (w, h)
    } else {
        let (sn, cs) = rot.to_radians().sin_cos();
        (w * cs.abs() + h * sn.abs(), w * sn.abs() + h * cs.abs())
    };
    let cx = (width as f64) / 2.0 + (x - cam_x) * scale;
    let cy = (height as f64) / 2.0 - (y - cam_y) * scale;
    let half_w = ew * scale / 2.0 + 2.0;
    let half_h = eh * scale / 2.0 + 2.0;
    // 与 CPU 路径同样先夹到屏幕内再画框（出屏的边贴着屏幕缘显示）
    let x0 = (cx - half_w).floor().max(0.0) as f32;
    let x1 = ((cx + half_w).ceil().min(width as f64) - 1.0) as f32;
    let y0 = (cy - half_h).floor().max(0.0) as f32;
    let y1 = ((cy + half_h).ceil().min(height as f64) - 1.0) as f32;
    if x1 < x0 || y1 < y0 {
        return;
    }
    const TEAL: [u8; 4] = [39, 192, 168, 255];
    // 四条 2px 边（覆盖范围 = CPU 双圈 put 的并集）
    push_solid(verts, white, x0, y0, x1 + 1.0, y0 + 2.0, TEAL); // 上
    push_solid(verts, white, x0, y1 - 1.0, x1 + 1.0, y1 + 1.0, TEAL); // 下
    push_solid(verts, white, x0, y0, x0 + 2.0, y1 + 1.0, TEAL); // 左
    push_solid(verts, white, x1 - 1.0, y0, x1 + 1.0, y1 + 1.0, TEAL); // 右
}

/// 纯色矩形：采样白块中心点（四角同 UV），颜色全靠染色。
fn push_solid(verts: &mut Vec<Vertex>, white: [f32; 2], x0: f32, y0: f32, x1: f32, y1: f32, rgba: [u8; 4]) {
    let [u, v] = white;
    push_quad(verts, x0, y0, x1, y1, [u, v, u, v], tint(rgba));
}

/// 两个三角形拼一个矩形（不用索引缓冲，顶点流够小不值得）。
fn push_quad(verts: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, uv: UvRect, color: [f32; 4]) {
    push_quad_corners(verts, [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], uv, color);
}

/// 任意四角的四边形（精灵旋转用）。角序 = 未旋转时的 左上/右上/右下/左下，
/// UV 矩形按同样的角序展开跟角走——贴图随四角一起转，不会错位。
/// 没有法线贴图的图元（纯色/文字/字形）一律走这个：nuv 哨兵、rotcs 恒等占位。
fn push_quad_corners(verts: &mut Vec<Vertex>, p: [[f32; 2]; 4], uv: UvRect, color: [f32; 4]) {
    push_quad_corners_n(verts, p, uv, NO_NORMAL, [1.0, 0.0], color);
}

/// 带法线贴图区域的四边形：nuv 与 uv 同一角序展开（法线随贴图一起转）。
fn push_quad_corners_n(
    verts: &mut Vec<Vertex>,
    p: [[f32; 2]; 4],
    uv: UvRect,
    nuv: UvRect,
    rotcs: [f32; 2],
    color: [f32; 4],
) {
    let [u0, v0, u1, v1] = uv;
    let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    let [n0, m0, n1, m1] = nuv;
    let nuvs = [[n0, m0], [n1, m0], [n1, m1], [n0, m1]];
    let vx = |i: usize| Vertex { pos: p[i], uv: uvs[i], color, nuv: nuvs[i], rotcs };
    verts.extend_from_slice(&[vx(0), vx(1), vx(2), vx(0), vx(2), vx(3)]);
}

fn tint(rgba: [u8; 4]) -> [f32; 4] {
    [
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    ]
}

// ---- 以下两个小工具镜像 vitric-render 的私有实现（组件约定的语义源头在那边）----

fn num(world: &World, id: vitric_ecs::EntityId, path: &str) -> Result<f64, String> {
    let v = world.get_field(id, path).map_err(|e| e.to_string())?;
    v.as_f64().ok_or_else(|| format!("实体 {id} 的 {path} 不是数字: {v}"))
}

fn parse_color(s: &str) -> Result<[u8; 4], String> {
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| format!("颜色 {s:?} 格式不对。写法: \"#rrggbb\"，如红色 \"#ff0000\""))?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 {s:?} 必须是 6 位十六进制 \"#rrggbb\""));
    }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("已校验十六进制");
    Ok([p(0), p(2), p(4), 255])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// 不碰 GPU 的假图集：白点 + 测试图（hero 带法线配对，gem 没有）+ 全空字形。
    fn fake_atlas() -> Atlas {
        let mut images = std::collections::BTreeMap::new();
        images.insert("hero.png".to_string(), [0.25, 0.25, 0.5, 0.5]);
        images.insert("hero_n.png".to_string(), [0.5, 0.5, 0.75, 0.75]);
        images.insert("gem.png".to_string(), [0.75, 0.75, 0.9, 0.9]);
        Atlas { images, white: [0.1, 0.1], glyphs: [[0.0; 4]; 128] }
    }

    #[test]
    fn solid_sprite_quad_matches_cpu_transform() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts.len(), 6, "一个矩形 = 两个三角形");
        // 默认相机 scale=8：2x2 精灵 → 屏幕中心 16x16 像素（24..40）
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 24.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 40.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 24.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 40.0);
        // 纯色：白点 UV + 红染色
        assert_eq!(verts[0].uv, [0.1, 0.1]);
        assert_eq!(verts[0].color, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn camera_moves_quads_and_y_is_up() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 2.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ffffff"})).unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        // 世界 y=+2、scale 8 → 屏幕 y 上移 16 像素（y 向上 → 像素行更小）
        let y_min = verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert_eq!(y_min, 24.0 - 16.0);
    }

    #[test]
    fn textured_sprite_uses_atlas_region_and_missing_image_is_explicit() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "image": "hero.png"})).unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts[0].uv, [0.25, 0.25], "左上角取图集区域起点");
        assert_eq!(verts[0].color, [1.0; 4], "贴图不染色");

        w.set_field(e, "Sprite.image", json!("ghost.png")).unwrap();
        let err = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap_err();
        assert!(err.contains("ghost.png") && err.contains("hero.png"), "{err}");
    }

    #[test]
    fn text_emits_one_quad_per_char_above_sprites() {
        let mut w = World::new();
        let s = w.spawn();
        w.set_component(s, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(s, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ffffff"})).unwrap();
        let t = w.spawn();
        w.set_component(t, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(t, "Text", json!({"content": "HI", "size": 2.0, "color": "#00ff00"}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        // 精灵 6 + 两字符 12，且文字顶点在精灵之后（画家算法：后画在上）
        assert_eq!(verts.len(), 6 + 12);
        assert_eq!(verts[6].color, [0.0, 1.0, 0.0, 1.0]);
        // 两字符 size=2、scale=8 → 整串宽 32px 居中：x 从 16 到 48
        let xs: Vec<f32> = verts[6..].iter().map(|v| v.pos[0]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 16.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 48.0);
    }

    #[test]
    fn rotated_quad_corners_exact_for_rot_90() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "color": "#ff0000", "rot": 90.0}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts.len(), 6, "旋转不增加顶点：仍是两个三角形");
        // 未旋转四角 (16,24)(48,24)(48,40)(16,40) 绕中心 (32,32) 逆时针转 90°：
        // 横条变竖条。世界逆时针 = 画面逆时针——原右上角转到左上
        assert_eq!(verts[0].pos, [24.0, 48.0], "左上角 → 左下");
        assert_eq!(verts[1].pos, [24.0, 16.0], "右上角 → 左上");
        assert_eq!(verts[2].pos, [40.0, 16.0], "右下角 → 右上");
        assert_eq!(verts[3].pos, [24.0, 48.0], "第二个三角形从角 0 重新起");
        assert_eq!(verts[4].pos, [40.0, 16.0]);
        assert_eq!(verts[5].pos, [40.0, 48.0], "左下角 → 右下");
        assert_eq!(verts[0].color, [1.0, 0.0, 0.0, 1.0], "染色不受旋转影响");
        // 显式 rot=0 与无字段同一几何（快路径）
        w.set_field(e, "Sprite.rot", json!(0.0)).unwrap();
        let v0 = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(v0[0].pos, [16.0, 24.0]);
        assert_eq!(v0[2].pos, [48.0, 40.0]);
    }

    #[test]
    fn rotated_texture_uv_follows_corners() {
        // 贴图随四角一起转：UV 角序不变（左上角的 UV 永远是图集区域起点），
        // 位置变了 UV 不变 = 贴图内容跟着精灵转
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "image": "hero.png", "rot": 90.0}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts[0].uv, [0.25, 0.25], "角 0 仍是图集区域左上");
        assert_eq!(verts[2].uv, [0.5, 0.5], "角 2 仍是图集区域右下");
        assert_eq!(verts[0].pos, [24.0, 48.0], "但位置已旋转");
    }

    #[test]
    fn normal_paired_sprite_carries_normal_uv_and_rotation() {
        // hero.png 在图集里有 hero_n.png 配对：顶点带法线区域 UV + rot 的 (cos, sin)
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "image": "hero.png"})).unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts[0].nuv, [0.5, 0.5], "角 0 = 法线区域左上");
        assert_eq!(verts[2].nuv, [0.75, 0.75], "角 2 = 法线区域右下");
        assert_eq!(verts[0].rotcs, [1.0, 0.0], "rot=0 → 恒等旋转");
        // rot=90：rotcs = (cos, sin)，nuv 角序不变（法线贴图跟着四角转）
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "image": "hero.png", "rot": 90.0}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert!(verts[0].rotcs[0].abs() < 1e-6 && (verts[0].rotcs[1] - 1.0).abs() < 1e-6);
        assert_eq!(verts[0].nuv, [0.5, 0.5]);
    }

    #[test]
    fn unpaired_quads_carry_normal_sentinel() {
        // 没配对的贴图 / 纯色块 / 文字：nuv 全是哨兵（x<0），片元走原光照公式
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "image": "gem.png"})).unwrap();
        let c = w.spawn();
        w.set_component(c, "Position", json!({"x": 3.0, "y": 0.0})).unwrap();
        w.set_component(c, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ff0000"})).unwrap();
        let t = w.spawn();
        w.set_component(t, "Position", json!({"x": 0.0, "y": 2.0})).unwrap();
        w.set_component(t, "Text", json!({"content": "A", "size": 1.0, "color": "#ffffff"}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert!(!verts.is_empty());
        for (i, v) in verts.iter().enumerate() {
            assert!(v.nuv[0] < 0.0 && v.nuv[1] < 0.0, "顶点 {i} 该是哨兵: {:?}", v.nuv);
            assert_eq!(v.rotcs, [1.0, 0.0], "哨兵图元的 rotcs 是恒等占位");
        }
    }

    #[test]
    fn selection_outline_uses_rotated_bbox() {
        // 4x2 转 90° → 包围盒约 2x4：描边几何取旋转后的包围盒（与 CPU 路径同一选择）
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "color": "#ff0000", "rot": 90.0}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), Some(e), 0).unwrap();
        assert_eq!(verts.len(), 6 + 24);
        // 旋转后半高 2 单位 = 16px + 2px 外扩 → 上缘 y=14（此轴无浮点边界问题，可精确断言）
        let y_min = verts[6..].iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert_eq!(y_min, 14.0, "描边上缘随旋转后包围盒抬高");
        // 横轴只断言范围：cos(90°) 的浮点尾数会让 floor 在 21/22 之间摆，不锁具体值
        let x_min = verts[6..].iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!((21.0..=22.0).contains(&x_min), "描边左缘应收窄到竖条附近: {x_min}");
    }

    #[test]
    fn wgsl_parses_and_validates_offline() {
        // 无 GPU 环境锁住 shader：解析 + 验证都过，uniform 布局错/语法错在 CI 就炸
        for (name, src) in [("scene", WGSL), ("bloom", WGSL_BLOOM), ("composite", WGSL_COMPOSITE)] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("WGSL({name}) 解析失败: {e}"));
            naga::valid::Validator::new(Default::default(), naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("WGSL({name}) 验证失败: {e:?}"));
        }
    }

    #[test]
    fn bloom_pass_params_pack_downsample_then_alternating_blur() {
        let p = bloom_pass_params(720, 0.6);
        assert_eq!(p.len(), 7, "1 下采样 + 3 轮 H/V");
        // [0] 下采样+阈值：radius 0、采样倍率 2、阈值原样
        assert_eq!(p[0].params, [0.6, 0.0, 0.0, 0.0]);
        assert_eq!(p[0].params2[0], 2.0);
        // 模糊 pass：阈值关（-1）、半径 = CPU 半径(720/90=8) 的一半 = 4、倍率 1、方向 H/V 交替
        for (i, pp) in p[1..].iter().enumerate() {
            assert_eq!(pp.params[0], -1.0, "pass {i} 不再做阈值");
            assert_eq!(pp.params[1], 4.0, "半分辨率半径减半");
            assert_eq!(pp.params2[0], 1.0);
            let expect_dir = if i % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
            assert_eq!([pp.params[2], pp.params[3]], expect_dir, "pass {i} 方向交替");
        }
        // 小视口：CPU 半径踩下限 2 → GPU 半分辨率半径 1（下限）
        assert_eq!(bloom_pass_params(64, 0.5)[1].params[1], 1.0);
    }

    #[test]
    fn bloom_half_dims_and_scene_pass_srgb_selection() {
        assert_eq!(half_dims(1280, 720), (640, 360));
        assert_eq!(half_dims(1, 1), (1, 1), "极小窗口不归零");
        // 泛光开启时场景 pass 永不做 sRGB 反算（反算挪到合成 pass）
        assert!(scene_pass_srgb(true, false), "无泛光 + sRGB 表面 = 老行为");
        assert!(!scene_pass_srgb(true, true));
        assert!(!scene_pass_srgb(false, false));
        assert!(!scene_pass_srgb(false, true));
    }

    #[test]
    fn globals_pack_lights_in_pixel_space() {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#202838"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 2.0, "y": 1.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 4.0, "color": "#ff8000", "intensity": 2.0}))
            .unwrap();
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.viewport, [64.0, 64.0, 0.0, 1.0], "w 位是光照开关");
        // ambient.rgb = #202838 / 255，.w = 灯数
        assert_eq!(g.ambient[3], 1.0);
        assert!((g.ambient[0] - 0x20 as f32 / 255.0).abs() < 1e-6);
        assert!((g.ambient[2] - 0x38 as f32 / 255.0).abs() < 1e-6);
        // 默认相机 scale=8：世界 (2,1) → 像素 (32+16, 32-8)，半径 4*8=32px
        assert_eq!(g.lights_pos[0], [48.0, 24.0, 32.0, 0.0]);
        // 颜色已乘 intensity=2
        assert_eq!(g.lights_color[0][0], 2.0);
        assert!((g.lights_color[0][1] - 2.0 * 0x80 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(g.lights_color[0][2], 0.0);
        // sRGB 标志独立于光照
        assert_eq!(build_globals(&w, 64, 64, true, 0).unwrap().viewport[2], 1.0);
        // 点光源（不写 kind）：kind 槽位 = 0，朝向数组整条为 0
        assert_eq!(g.lights_dir[0], [0.0; 4]);
    }

    #[test]
    fn globals_pack_spot_and_directional_kinds() {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let beam = w.spawn();
        w.set_component(beam, "Position", json!({"x": 2.0, "y": 1.0})).unwrap();
        w.set_component(
            beam,
            "Light",
            json!({"radius": 4.0, "kind": "spot", "angle": 60.0, "dir": 90.0, "intensity": 2.0}),
        )
        .unwrap();
        let sun = w.spawn(); // 平行光不需要 Position
        w.set_component(sun, "Light", json!({"kind": "directional", "dir": 180.0, "intensity": 0.5}))
            .unwrap();
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.ambient[3], 2.0, "平行光也计入灯数");
        // 聚光灯：位置/半径同点光源（scale=8 → 像素 (48,24)、半径 32px），kind 槽位 = 1
        assert_eq!(g.lights_pos[0], [48.0, 24.0, 32.0, 1.0]);
        assert_eq!(g.lights_color[0], [2.0, 2.0, 2.0, 0.0]);
        // 朝向 dir=90°（世界 +y）→ 像素空间 (cos90, -sin90) = (0,-1)；半锥角 30° 弧度
        assert!(g.lights_dir[0][0].abs() < 1e-6, "{:?}", g.lights_dir[0]);
        assert_eq!(g.lights_dir[0][1], -1.0);
        assert!((g.lights_dir[0][2] - (30f32).to_radians()).abs() < 1e-6);
        // 平行光：位置/半径占位 0，kind 槽位 = 2，颜色已乘 intensity，朝向也打包
        // （法线像素的片元按它算 max(dot(N,L),0)，哨兵像素不读）
        assert_eq!(g.lights_pos[1], [0.0, 0.0, 0.0, 2.0]);
        assert_eq!(g.lights_color[1], [0.5, 0.5, 0.5, 0.0]);
        assert_eq!(g.lights_dir[1][0], -1.0, "dir=180° → 像素空间 (-1, 0)");
        assert!(g.lights_dir[1][1].abs() < 1e-6);
        assert_eq!(g.lights_dir[1][2], 0.0, "半锥角只属于 spot");
    }

    /// 在 (x,y) 放一面 cw×ch 的遮光墙（Solid+Position+Collider）。
    fn add_wall(w: &mut World, x: f64, y: f64, cw: f64, ch: f64) {
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": x, "y": y})).unwrap();
        w.set_component(e, "Collider", json!({"w": cw, "h": ch})).unwrap();
        w.set_component(e, "Solid", json!({})).unwrap();
    }

    fn world_shadow_lamp(radius: f64) -> (World, vitric_ecs::EntityId) {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000", "shadows": true})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": radius})).unwrap();
        (w, amb)
    }

    #[test]
    fn globals_pack_occluders_in_pixel_space() {
        let (mut w, amb) = world_shadow_lamp(6.0);
        add_wall(&mut w, 2.0, 1.0, 1.0, 2.0);
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        // 灯 0 的遮挡区间：起点 0、1 条
        assert_eq!(g.shadow_ranges[0], [0.0, 1.0, 0.0, 0.0], "[起点, 条数]");
        // 默认相机 scale=8：中心 (2,1) → 像素 (48,24)，半宽 4px / 半高 8px
        assert_eq!(g.occluders[0], [44.0, 16.0, 52.0, 32.0], "[x0, y0, x1, y1] 像素空间");
        assert_eq!(g.occluders[1], [0.0; 4], "没占用的槽位保持零");
        // 单箱成组：子箱区间 [0,1]，子箱 = 同一个 AABB
        assert_eq!(g.occluder_sub_ranges[0], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(g.occluder_subs[0], [44.0, 16.0, 52.0, 32.0]);
        // shadows 关（字段缺省）：墙还在，但区间全零 → shader 循环零次
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.shadow_ranges[0], [0.0; 4]);
        assert_eq!(g.occluders[0], [0.0; 4]);
    }

    #[test]
    fn globals_merge_flush_tiles_and_cull_per_light() {
        // 三块贴齐的瓦片合并成一条（1 个大箱、3 个子箱）；灯盘外的孤箱被剔除
        let (mut w, _) = world_shadow_lamp(3.0);
        for i in 0..3 {
            add_wall(&mut w, i as f64, 1.0, 1.0, 1.0);
        }
        add_wall(&mut w, 40.0, 0.0, 1.0, 1.0); // 远在灯盘（3*8=24px）外
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.shadow_ranges[0], [0.0, 1.0, 0.0, 0.0], "合并后只剩 1 条、孤箱被剔除");
        // 行：世界 x ∈ [-0.5, 2.5]、y ∈ [0.5, 1.5] → 像素 [28, 20, 52, 28]
        assert_eq!(g.occluders[0], [28.0, 20.0, 52.0, 28.0]);
        assert_eq!(g.occluder_sub_ranges[0], [0.0, 3.0, 0.0, 0.0], "3 个子箱");
        assert_eq!(g.occluder_subs[0], [28.0, 20.0, 36.0, 28.0], "子箱按原始瓦片");
        assert_eq!(g.occluder_subs[2], [44.0, 20.0, 52.0, 28.0]);

        // 两盏灯各剔各的：远灯只看得到自己旁边的箱子，区间在 flat 数组里前后排
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000", "shadows": true})).unwrap();
        for x in [-3.0, 3.0] {
            let lamp = w.spawn();
            w.set_component(lamp, "Position", json!({"x": x, "y": 0.0})).unwrap();
            w.set_component(lamp, "Light", json!({"radius": 1.5})).unwrap();
        }
        add_wall(&mut w, -3.0, 1.0, 1.0, 1.0);
        add_wall(&mut w, 3.0, 1.0, 1.0, 1.0);
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.shadow_ranges[0], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(g.shadow_ranges[1], [1.0, 1.0, 0.0, 0.0], "第二盏灯的区间接在后面");
        assert_eq!(g.occluders[0], [4.0, 20.0, 12.0, 28.0], "灯 0 只带自己旁边的墙");
        assert_eq!(g.occluders[1], [52.0, 20.0, 60.0, 28.0], "灯 1 同理");
    }

    #[test]
    fn shadow_uniform_budgets_are_explicit_errors() {
        // 单灯上限：65 个互不贴齐（留缝）的箱子全部落进一盏大灯的灯盘
        let (mut w, _) = world_shadow_lamp(80.0);
        for i in 0..(SHADOW_PER_LIGHT + 1) {
            add_wall(&mut w, i as f64 * 2.0 - 64.0, 0.0, 1.0, 1.0);
        }
        let err = build_globals(&w, 64, 64, false, 0).err().expect("超单灯上限必须报错");
        assert!(err.contains("65") && err.contains("64") && err.contains("提示"), "{err}");

        // 合计预算：5 盏灯 × 60 箱 = 300 > 256
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000", "shadows": true})).unwrap();
        for i in 0..5 {
            let lamp = w.spawn();
            w.set_component(lamp, "Position", json!({"x": i as f64, "y": 0.0})).unwrap();
            w.set_component(lamp, "Light", json!({"radius": 80.0})).unwrap();
        }
        for i in 0..60 {
            add_wall(&mut w, i as f64 * 2.0 - 60.0, 0.0, 1.0, 1.0);
        }
        let err = build_globals(&w, 64, 64, false, 0).err().expect("超合计预算必须报错");
        assert!(err.contains("256") && err.contains("提示"), "{err}");
    }

    #[test]
    fn globals_without_ambient_keep_lighting_off() {
        let mut w = World::new();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 4.0})).unwrap();
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.viewport[3], 0.0, "没 Ambient = 光照关，灯不打包");
        assert_eq!(g.ambient, [0.0; 4]);
        assert_eq!(g.lights_pos[0], [0.0; 4]);
    }

    #[test]
    fn lighting_adds_fullscreen_background_quad() {
        // 光照开启时第一个方块是全屏背景（清屏色没法被 shader 打光）
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#202838"})).unwrap();
        let verts = build_vertices(&w, 64, 48, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts.len(), 6);
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 0.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 64.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 48.0);
        assert_eq!(verts[0].color, tint(vitric_render::BACKGROUND));
        // 没 Ambient：不铺背景方块（旧行为）
        assert!(build_vertices(&World::new(), 64, 48, &fake_atlas(), None, 0).unwrap().is_empty());
    }

    fn test_font() -> vitric_render::FontStore {
        vitric_render::FontStore::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/book/fonts/DejaVuSans.ttf"),
        )
        .unwrap()
    }

    #[test]
    fn glyph_atlas_shelf_packs_rows_and_reports_full_explicitly() {
        // 16x16 小图集：白块(2x2+1px 空隙)在 (0,0)，row_h=3
        let mut a = GlyphAtlas::new(16);
        assert_eq!(a.pending.len(), 1, "白块在待上传队列里");
        assert_eq!(a.alloc_rect(8, 8).unwrap(), (3, 0), "同行白块右侧");
        // 放不下第二个 8x8（x=12+9>16）→ 换行到 y=9，但 9+9>16 → 显式报满
        let err = a.alloc_rect(8, 8).unwrap_err();
        assert!(err.contains("已满") && err.contains("字号"), "{err}");
        // 单块超边长：另一种显式错误（不是包装成"满"）
        let err = a.alloc_rect(20, 4).unwrap_err();
        assert!(err.contains("超过"), "{err}");
        // 换行确实发生过：下一块小的落在第二行
        assert_eq!(a.alloc_rect(2, 2).unwrap(), (0, 9));
    }

    #[test]
    fn glyph_uv_is_uploaded_once_then_cached() {
        let font = test_font();
        let mut a = GlyphAtlas::new(1024);
        let base = a.pending.len(); // 白块
        let uv1 = a.glyph_uv(&font, 'A', 24).unwrap();
        assert_eq!(a.pending.len(), base + 1, "首次出现 → 一次上传");
        let up = a.pending.last().unwrap();
        assert_eq!(up.pixels.len(), (up.w * up.h * 4) as usize, "RGBA 像素量对得上");
        assert!(up.pixels.chunks_exact(4).all(|p| p[..3] == [255, 255, 255]), "白底+覆盖率 alpha");
        // 同键再来：命中缓存，不再上传
        let uv2 = a.glyph_uv(&font, 'A', 24).unwrap();
        assert_eq!(uv1, uv2);
        assert_eq!(a.pending.len(), base + 1, "缓存命中不产生新上传");
        // 同字符不同字号是另一个字形
        let uv3 = a.glyph_uv(&font, 'A', 32).unwrap();
        assert_ne!(uv1, uv3);
        assert_eq!(a.pending.len(), base + 2);
        // CJK：DejaVu 没有的字形落到 .notdef 豆腐块——也得占位可见，不静默丢
        a.glyph_uv(&font, '中', 24).unwrap();
        assert_eq!(a.pending.len(), base + 3);
    }

    #[test]
    fn ttf_text_quads_follow_proportional_layout() {
        let font = test_font();
        let mut a = GlyphAtlas::new(1024);
        let mut w = World::new();
        let t = w.spawn();
        w.set_component(t, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(t, "Text", json!({"content": "Hi", "size": 2.0, "color": "#00ff00"}))
            .unwrap();
        let mut verts = Vec::new();
        push_ttf_texts(&mut verts, &w, 64, 64, &font, &mut a, (0.0, 0.0, 8.0)).unwrap();
        assert_eq!(verts.len(), 12, "两个非空字形 = 两个方块");
        assert_eq!(verts[0].color, [0.0, 1.0, 0.0, 1.0]);
        // 与 CPU 路径同一套排版：总宽来自 layout(px=16)，整串横向居中于屏幕中心 32
        let (_, total_w) = font.layout("Hi", 16);
        let left = 32.0 - total_w / 2.0;
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let x_min = xs.iter().cloned().fold(f32::MAX, f32::min);
        let x_max = xs.iter().cloned().fold(f32::MIN, f32::max);
        assert!(x_min >= left - 1.5 && x_max <= left + total_w + 1.5, "字形落在排版包络内: {x_min}..{x_max} vs {left}+{total_w}");
        // 空格只占 advance 不出方块
        w.set_field(t, "Text.content", json!(" ")).unwrap();
        let mut sp = Vec::new();
        push_ttf_texts(&mut sp, &w, 64, 64, &font, &mut a, (0.0, 0.0, 8.0)).unwrap();
        assert!(sp.is_empty());
    }

    #[test]
    fn particle_quads_match_cpu_dots_and_are_unlit() {
        // GPU 粒子方块必须与 CPU 真相源（emitter_particles）的位置/数量/颜色一致
        let mut w = World::new();
        let e = w.spawn_named("sparks").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "burst", "count": 3, "lifetime": 40, "size": 1.0, "burst": 0,
                   "speed_min": 2.0, "speed_max": 4.0, "color": "#ffcc40"}),
        )
        .unwrap();
        let tick = 10u64;
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, tick).unwrap();
        let src = &vitric_render::collect_emitters(&w).unwrap()[0];
        let dots = vitric_render::emitter_particles(src, tick);
        assert_eq!(dots.len(), 3);
        assert_eq!(verts.len(), dots.len() * 6, "每个粒子一个方块（两个三角形）");
        for (i, p) in dots.iter().enumerate() {
            let quad = &verts[i * 6..i * 6 + 6];
            // 与 CPU 同一条变换：中心 = 屏幕中心 + 世界偏移·scale（默认相机 scale=8）
            let cx = 32.0 + p.x * 8.0;
            let cy = 32.0 - p.y * 8.0;
            let half = p.size * 8.0 / 2.0;
            let x_min = quad.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
            let y_min = quad.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
            assert!((x_min as f64 - (cx - half)).abs() < 1e-4, "粒子 {i} 位置对齐 CPU");
            assert!((y_min as f64 - (cy - half)).abs() < 1e-4);
            assert_eq!(quad[0].color, tint(p.rgba), "颜色（含淡出 alpha）一致");
            // 自发光哨兵：nuv < -1.5，片元跳过光照
            assert!(quad.iter().all(|v| v.nuv[0] < -1.5 && v.nuv[1] < -1.5), "UNLIT 哨兵");
        }
        // 未触发的 burst（burst < 0）一个顶点都不出
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "burst", "count": 3, "lifetime": 40, "size": 1.0}),
        )
        .unwrap();
        assert!(build_vertices(&w, 64, 64, &fake_atlas(), None, tick).unwrap().is_empty());
    }

    #[test]
    fn particle_tick_rate_constant_matches_sim() {
        // 粒子时间换算常量必须与模拟频率同值（render 不依赖 sim，跨 crate 在这里锁死）
        assert_eq!(
            vitric_render::PARTICLE_TICKS_PER_SECOND,
            vitric_sim::TICKS_PER_SECOND as f64
        );
    }

    #[test]
    fn selection_outline_is_teal_and_on_top() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), Some(e), 0).unwrap();
        // 精灵 6 + 描边四条边 24
        assert_eq!(verts.len(), 6 + 24);
        let teal = [39.0 / 255.0, 192.0 / 255.0, 168.0 / 255.0, 1.0];
        assert_eq!(verts[6].color, teal);
        // 半宽 8px + 2px 外扩 → 描边左缘在 32-10=22（与 CPU 路径同一几何）
        let x_min = verts[6..].iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert_eq!(x_min, 22.0);
        // 选中实体没了 → 静默跳过，不报错
        let dead = {
            let mut w2 = World::new();
            let d = w2.spawn();
            w2.despawn(d).unwrap();
            (w2, d)
        };
        let verts = build_vertices(&dead.0, 64, 64, &fake_atlas(), Some(dead.1), 0).unwrap();
        assert!(verts.is_empty());
    }
}
