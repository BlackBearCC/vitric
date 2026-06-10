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

/// uniform：视口宽高 + "表面是不是 sRGB 格式"标志（z>0.5 时片元做 sRGB→线性，
/// 保证最终字节和 CPU 路径的 sRGB 字节一致）。vec4 凑对齐。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 4],
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
struct Globals { viewport: vec4<f32> };
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
    // 表面是 sRGB 格式时写入值按线性解释，这里反算一次让最终字节贴齐 CPU 路径
    if (globals.viewport.z > 0.5) {
        c = vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
"#;

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

        // 背景色与 CPU 路径同字节 [24,26,33]；sRGB 表面的清屏色按线性解释，要先反算
        let bg = [24.0 / 255.0, 26.0 / 255.0, 33.0 / 255.0];
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vitric-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vitric-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
        });

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
        let srgb_flag = if self.config.format.is_srgb() { 1.0 } else { 0.0 };
        let globals = Globals { viewport: [w as f32, h as f32, srgb_flag, 0.0] };
        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
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
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }
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

fn build_vertices(
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
    selection: Option<vitric_ecs::EntityId>,
    tick: u64,
) -> Result<Vec<Vertex>, String> {
    // 取景（含 Shake 抖动偏移）直接用 vitric-render 的实现——两条路径抖得逐位一致
    let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick)?;
    let mut verts: Vec<Vertex> = Vec::new();

    // 精灵：按实体序（画家算法，后画盖前画）
    for id in world.query(&["Position", "Sprite"]) {
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        // 世界 → 屏幕像素（y 翻转，相机居中）——与 CPU 路径同一公式
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;
        let (x0, y0) = ((cx - half_w) as f32, (cy - half_h) as f32);
        let (x1, y1) = ((cx + half_w) as f32, (cy + half_h) as f32);

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
            push_solid(&mut verts, atlas, x0, y0, x1, y1, rgba);
        } else {
            // 图不存在直接报错（不画占位符）——错误文案对齐 CPU 路径
            let uv = atlas.images.get(&image_name).ok_or_else(|| {
                format!(
                    "实体 {id} 的 Sprite.image {image_name:?} 不在素材仓库里。\
                     现有素材: [{}]。提示：图放进项目 assets/ 目录，路径相对 assets/ 写",
                    atlas.images.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            push_quad(&mut verts, x0, y0, x1, y1, *uv, [1.0; 4]);
        }
    }

    // 文字：每字符一个字形方块，整串居中于 Position，画在精灵之上
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

    // 选中描边：青色 2px，画在最上层（几何对齐 vitric_render::draw_selection_outline）
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
    let cx = (width as f64) / 2.0 + (x - cam_x) * scale;
    let cy = (height as f64) / 2.0 - (y - cam_y) * scale;
    let half_w = w * scale / 2.0 + 2.0;
    let half_h = h * scale / 2.0 + 2.0;
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
    let [u0, v0, u1, v1] = uv;
    let a = Vertex { pos: [x0, y0], uv: [u0, v0], color };
    let b = Vertex { pos: [x1, y0], uv: [u1, v0], color };
    let c = Vertex { pos: [x1, y1], uv: [u1, v1], color };
    let d = Vertex { pos: [x0, y1], uv: [u0, v1], color };
    verts.extend_from_slice(&[a, b, c, a, c, d]);
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
