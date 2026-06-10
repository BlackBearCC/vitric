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

/// 顶点：像素坐标（shader 里除视口尺寸转 NDC）+ 图集 UV + 染色（乘进采样色）。
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// uniform：viewport = [宽, 高, "表面是 sRGB"标志, "光照开启"标志]（z>0.5 时片元做
/// sRGB→线性，保证最终字节和 CPU 路径的 sRGB 字节一致；w>0.5 时片元跑光照公式）。
/// ambient = [环境色 rgb, 灯数]；每盏灯两条 vec4：位置数组 [灯心 x_px, y_px, 半径 px, 0]、
/// 颜色数组 [r·intensity, g·intensity, b·intensity, 0]——世界→像素的变换在 CPU 端做完，
/// shader 只算距离。vec4 数组在 WGSL uniform（std140 风格）下天然 16 字节步长，无 padding 坑。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 4],
    ambient: [f32; 4],
    lights_pos: [[f32; 4]; vitric_render::MAX_LIGHTS],
    lights_color: [[f32; 4]; vitric_render::MAX_LIGHTS],
}

/// 图集里一块区域的 UV 矩形 [u0, v0, u1, v1]。
type UvRect = [f32; 4];

/// 图集：素材名 → UV 区域，外加白块（纯色用）和 128 个字体字形。
struct Atlas {
    images: std::collections::BTreeMap<String, UvRect>,
    /// 白块中心点 UV（四角同 UV → 平采样，永不渗色）。
    white: [f32; 2],
    glyphs: [UvRect; 128],
}

const WGSL: &str = r#"
struct Globals {
    viewport: vec4<f32>,                  // xy 视口尺寸 / z sRGB 标志 / w 光照开关
    ambient: vec4<f32>,                   // rgb 环境色 / w 灯数
    lights_pos: array<vec4<f32>, 64>,     // xy 灯心(像素) / z 半径(像素)
    lights_color: array<vec4<f32>, 64>,   // rgb 已乘 intensity
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
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
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(atlas_tex, atlas_samp, in.uv) * in.color;
    // 光照——与 CPU 路径同一公式（vitric-render 模块文档）：
    //   lit = min(ambient + Σ color·intensity·(1 - d/r)², 1.5)；out = min(c·lit, 1)
    // 必须在 sRGB 反算**之前**：CPU 是直接在 sRGB 字节上乘的，这里要在同一数值空间打光，
    // 两条路径才长一个样。in.pos 是帧缓冲像素坐标（中心 +0.5），和 CPU 的像素中心一致。
    if (globals.viewport.w > 0.5) {
        var lit = globals.ambient.rgb;
        let n = u32(globals.ambient.w);
        for (var i = 0u; i < n; i = i + 1u) {
            let lp = globals.lights_pos[i];
            let d = distance(in.pos.xy, lp.xy);
            if (d < lp.z) {
                let f = 1.0 - d / lp.z;
                lit = lit + globals.lights_color[i].rgb * f * f;
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

        let verts = build_vertices(world, w, h, &self.atlas, selection, tick)?;

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
                    pass.set_bind_group(0, &self.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.vertex_buf.slice(..bytes.len() as u64));
                    pass.draw(0..verts.len() as u32, 0..1);
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
                        pass.set_bind_group(0, &self.bind_group, &[]);
                        pass.set_vertex_buffer(0, self.vertex_buf.slice(..bytes.len() as u64));
                        pass.draw(0..verts.len() as u32, 0..1);
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
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
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
            g.lights_pos[i] = [
                ((width as f64) / 2.0 + (l.x - cam_x) * scale) as f32,
                ((height as f64) / 2.0 - (l.y - cam_y) * scale) as f32,
                (l.radius * scale) as f32,
                0.0,
            ];
            g.lights_color[i] = [
                (l.rgb[0] * l.intensity) as f32,
                (l.rgb[1] * l.intensity) as f32,
                (l.rgb[2] * l.intensity) as f32,
                0.0,
            ];
        }
    }
    Ok(g)
}

fn build_vertices(
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
    selection: Option<vitric_ecs::EntityId>,
    tick: u64,
) -> Result<Vec<Vertex>, String> {
    // 取景（含 Shake 抖动偏移）直接用 vitric-render 的实现——两条路径抖得逐位一致
    let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick, height)?;
    let mut verts: Vec<Vertex> = Vec::new();

    // 光照开启时背景也要被照（CPU 路径是整张 buf 过公式）：清屏色没法逐像素变，
    // 所以先铺一个全屏背景方块，让 shader 把背景和实体一起打光
    if vitric_render::ambient_of(world)?.is_some() {
        push_solid(&mut verts, atlas, 0.0, 0.0, width as f32, height as f32, vitric_render::BACKGROUND);
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
            push_quad_corners(&mut verts, corners, *uv, [1.0; 4]);
        }
    }

    // 文字：每字符一个字形方块，整串居中于 Position，画在精灵之上。
    // 永远直立——Sprite.rot 只转精灵，不转文字（与 CPU 路径同语义）
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
        // screen=true: HUD 锚定——与 CPU 路径同语义,坐标相对屏幕中心,不随相机走
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let chars: Vec<char> = content.chars().collect();
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
                push_quad(&mut verts, x0, y0, x1, y1, atlas.glyphs[cp], tint(rgba));
            } else {
                // 非 ASCII：实心方块占位，同 CPU 路径
                push_solid(&mut verts, atlas, x0, y0, x1, y1, rgba);
            }
        }
    }

    // 选中描边：青色 2px，画在最上层（几何对齐 vitric_render::draw_selection_outline）。
    // 已知小偏差：光照开启时描边在 GPU 窗口会被一起打光（同一条管线），CPU 窗口路径
    // 是渲完再描所以不被打光——检查器调试装饰，不进截图/断言，不值得为它开第二条管线
    if let Some(id) = selection {
        push_selection_outline(&mut verts, world, width, height, atlas, id, (cam_x, cam_y, scale));
    }
    Ok(verts)
}

fn push_selection_outline(
    verts: &mut Vec<Vertex>,
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
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
    push_solid(verts, atlas, x0, y0, x1 + 1.0, y0 + 2.0, TEAL); // 上
    push_solid(verts, atlas, x0, y1 - 1.0, x1 + 1.0, y1 + 1.0, TEAL); // 下
    push_solid(verts, atlas, x0, y0, x0 + 2.0, y1 + 1.0, TEAL); // 左
    push_solid(verts, atlas, x1 - 1.0, y0, x1 + 1.0, y1 + 1.0, TEAL); // 右
}

/// 纯色矩形：采样白块中心（四角同 UV），颜色全靠染色。
fn push_solid(verts: &mut Vec<Vertex>, atlas: &Atlas, x0: f32, y0: f32, x1: f32, y1: f32, rgba: [u8; 4]) {
    let [u, v] = atlas.white;
    push_quad(verts, x0, y0, x1, y1, [u, v, u, v], tint(rgba));
}

/// 两个三角形拼一个矩形（不用索引缓冲，顶点流够小不值得）。
fn push_quad(verts: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, uv: UvRect, color: [f32; 4]) {
    push_quad_corners(verts, [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], uv, color);
}

/// 任意四角的四边形（精灵旋转用）。角序 = 未旋转时的 左上/右上/右下/左下，
/// UV 矩形按同样的角序展开跟角走——贴图随四角一起转，不会错位。
fn push_quad_corners(verts: &mut Vec<Vertex>, p: [[f32; 2]; 4], uv: UvRect, color: [f32; 4]) {
    let [u0, v0, u1, v1] = uv;
    let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    let vx = |i: usize| Vertex { pos: p[i], uv: uvs[i], color };
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

    /// 不碰 GPU 的假图集：白点 + 一张测试图 + 全空字形。
    fn fake_atlas() -> Atlas {
        let mut images = std::collections::BTreeMap::new();
        images.insert("hero.png".to_string(), [0.25, 0.25, 0.5, 0.5]);
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
