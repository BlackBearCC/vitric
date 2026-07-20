//! GPU presentation — wgpu screen path.
//!
//! Only cares about "getting the picture onto the window", **not** the rendering source of truth: HEADLESS screenshots (render/screenshot)
//! always go through vitric-render's CPU rasterization, byte-exact. This reads the same set of component conventions
//! (Position/Sprite/Text/Camera), visually aligned with the CPU path — the same world,
//! what a human sees in the GPU window and what an agent sees in a screenshot are the same picture.
//!
//! Structure: at startup packs all assets + 8x8 bitmap font + 1x1 white tile into **one atlas**,
//! each frame the CPU side assembles a vertex stream in entity order (pixel coordinates + atlas UV + tint),
//! one pipeline, one bind group, one draw finishes. Asset hot reload triggers atlas rebuild via a generation number.
//!
//! Vector fonts (manifest `font`, see vitric-render::FontStore) go through a **second dynamic glyph atlas**:
//! 1024x1024 RGBA (white background + coverage as alpha — shader unchanged, tint multiplication is the anti-alias blend),
//! shelf packed, lazy rasterize + queue.write_texture incremental upload on new (character, pixel size) appearance.
//! Same pipeline, second bind group: first draw the sprite stream (main atlas), then the glyph stream (glyph atlas).
//! Layout/rasterization/rounding all reuse vitric-render's FontStore — visually aligned with the CPU path,
//! but not guaranteed byte-identical (coverage blending happens in the GPU blend stage); screenshots/assertions always use CPU as truth.
//! Atlas full = explicit error (advising fewer distinct sizes), never silently drops glyphs.
//!
//! Bloom (when Bloom entity is present) splits the single pass into multiple passes:
//!   1. Scene pass: same vertex stream rendered into an offscreen texture (Rgba8Unorm, lighting runs here as usual)
//!   2. Downsample + threshold pass: scene → half resolution (bright-part extraction)
//!   3. Box blur ×6: half-resolution ping-pong, horizontal/vertical alternating 3 rounds
//!   4. Composite pass: scene + bloom·strength → surface (sRGB inverse only in this last step)
//!
//! When no Bloom entity is present, fully goes through the old single-pass direct-to-surface render — zero extra cost, bytes unchanged.
//!
//! Known differences from the CPU source of truth (vitric-render::apply_bloom) (visual consistency first, not byte-exact):
//! - GPU blur runs at **half resolution** (radius is half of CPU's, lower bound 1) — saves 4x bandwidth, halo
//!   spatial scale consistent; composite bilinearly upscales back to full resolution (CPU runs full resolution throughout)
//! - Downsampling takes nearest-neighbor single point (CPU has no downsample step)
//! - Intermediate results stored in 8-bit Unorm (CPU is f32 throughout) — quantized once per pass
//!
//! Screenshots/assertions always use the CPU path as truth; here we only ensure "the window looks the same".

use std::sync::Arc;

use winit::window::Window;

use vitric_ecs::World;
use vitric_render::Assets;

/// Vertex: pixel coordinates (shader divides by viewport size to convert to NDC) + atlas UV + tint (multiplied into sampled color)
/// + normal-map UV + sprite rotation.
///
/// Normal maps live in the same atlas as regular images (they are just PNGs in assets/), so only one extra UV set:
/// `nuv` follows corners (expanded with the same corner order as uv), x < 0 = sentinel = this primitive has no normal (solid block/
/// text / unpaired texture) — fragment goes through the original lighting formula, same semantics as the CPU path's sentinel zero vector.
/// `rotcs` = (cos, sin) of Sprite.rot, in the fragment the sampled normal is rotated into screen space
/// (local→screen matrix [[c, s], [-s, c]], same matrix as CPU's sample_normal).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    nuv: [f32; 2],
    rotcs: [f32; 2],
}

/// Normal UV sentinel rect: all four corners -1 (still < 0 after interpolation, fragment skips the normal path based on this).
const NO_NORMAL: UvRect = [-1.0; 4];

/// Emissive primitive sentinel rect: all four corners -2 (< -1.5, fragment **skips lighting entirely** — particle-only,
/// mirrors the CPU path's "particles drawn after lighting" semantics). -1 (regular no-normal) is still lit as usual.
const UNLIT: UvRect = [-2.0; 4];

/// uniform: viewport = [width, height, "surface is sRGB" flag, "lighting on" flag] (when z>0.5 the fragment does
/// sRGB→linear, ensuring the final bytes match the CPU path's sRGB bytes; when w>0.5 the fragment runs the lighting formula).
/// ambient = [ambient color rgb, light count]; each light has three vec4s (packed layout, locked by unit tests):
/// - lights_pos[i]   = [light center x_px, y_px, radius px, kind], kind: 0=point / 1=spot / 2=directional
///   (directional light does not read position/radius, the first three are always 0 placeholders)
/// - lights_color[i] = [r·intensity, g·intensity, b·intensity, 0] (w reserved)
/// - lights_dir[i]   = [toward pixel-space unit vector x, y, half-cone angle radians, 0]; spot uses xy+z,
///   directional uses xy (normal pixels compute direction from this, sentinel pixels do not read), point is always 0
///
/// Shadows (Ambient.shadows) add four more blocks (also locked by unit tests; occluders first merge adjacent tile-aligned boxes,
/// then cull per-light by light disc — semantics source vitric_render::build_shadow_boxes / cull_shadow_boxes,
/// the CPU path uses the same set; neither step changes visible output):
/// - shadow_ranges[i] = [this light's start in occluders, count, 0, 0] (shadow off / directional light = all zeros,
///   shader loop runs zero times)
/// - occluders[k] = merged large-box pixel-space AABB [x0, y0, x1, y1], arranged by light as a contiguous range
///   (packed per-light after per-light culling, repeated per light; budget [`SHADOW_FLAT_BUDGET`] entries, explicit error if exceeded)
/// - occluder_sub_ranges[k] = [large-box sub-box start, sub-box count, 0, 0] (parallel to occluders;
///   when a pixel falls inside a large box it falls back to sub-boxes — "the box you are in does not occlude yourself" judged by original entity)
/// - occluder_subs[s] = original occluder pixel-space AABB (one global copy, not repeated per light,
///   upper bound vitric_render::MAX_OCCLUDERS = collect_occluders' hard upper bound)
///
/// World→pixel transform is done on the CPU side (including y flip: world dir degrees → (cos, -sin)), shader only computes
/// distance and dot product. vec4 arrays in WGSL uniform (std140 style) naturally have a 16-byte stride, no padding pitfalls.
/// The entire uniform ≈ 16.4KB — within default Limits (64KB) (not supported on WebGL2 downlevel's 16KB,
/// desktop native targets have no such constraint).
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

/// Total occluder list uniform budget across all lights (count after merging + per-light culling, counted per light).
const SHADOW_FLAT_BUDGET: usize = 256;

/// Upper bound on the culled occluder list per single light. Exceeded means too many independent (un-mergeable) occluders covered by the light radius —
/// explicit error, advising fewer lights / fewer boxes / tile merging, never silently truncated.
const SHADOW_PER_LIGHT: usize = 64;

/// UV rect [u0, v0, u1, v1] of a region in the atlas.
type UvRect = [f32; 4];

/// Atlas: asset name → UV region, plus a white tile (for solid colors) and 128 font glyphs.
struct Atlas {
    images: std::collections::BTreeMap<String, UvRect>,
    /// White tile center UV (all four corners same UV → flat sampling, never bleeds).
    white: [f32; 2],
    glyphs: [UvRect; 128],
}

/// Dynamic glyph atlas edge length (pixels). 1024²·RGBA = 4MB VRAM, fits hundreds of common size glyphs;
/// when full, an explicit error is raised (see [`GlyphAtlas::alloc_rect`]), never silently drops glyphs.
const GLYPH_ATLAS_SIZE: u32 = 1024;

/// One pending glyph pixel upload ([`GlyphAtlas`] accumulates, present consumes via write_texture).
/// Pixels are RGBA: white background + coverage as alpha — the main shader's `sampled color × tint` directly yields
/// the same coverage blend formula as the CPU path (anti-aliasing), no second pipeline needed.
struct GlyphUpload {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    pixels: Vec<u8>,
}

/// Dynamic glyph atlas (pure bookkeeping, does not touch GPU — unit-testable): shelf packing + (character, pixel size) → UV cache
/// + pending upload queue. Glyphs appear on demand: rasterized once, uploaded once, then forever cache hits.
struct GlyphAtlas {
    size: u32,
    /// Shelf cursor (current row start x / row top y / row height).
    x: u32,
    y: u32,
    row_h: u32,
    map: std::collections::BTreeMap<(char, u32), UvRect>,
    /// White tile center UV (solid color source for selection outline in font mode — outline and glyphs share one bind group).
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

    /// Shelf-pack a w×h block (1px gap on the right/bottom). Full / single-block-too-large both raise explicit errors.
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

    /// UV for one (character, pixel size): cache hit returns directly; first appearance rasterizes + queues upload.
    /// Empty-outline glyphs (spaces etc.) should not reach here (caller checks coverage first to skip).
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

/// GPU side of the glyph atlas: resident texture + bind group (**same layout, same pipeline** as the main atlas,
/// just swapping one texture) + the pure-bookkeeping [`GlyphAtlas`].
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
            format: wgpu::TextureFormat::Rgba8Unorm, // Non-sRGB: same byte space as the main atlas
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

    /// Consume new glyphs appearing this frame (incremental upload, existing content untouched).
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
    viewport: vec4<f32>,                  // xy viewport size / z sRGB flag / w lighting on
    ambient: vec4<f32>,                   // rgb ambient color / w light count
    lights_pos: array<vec4<f32>, 64>,     // xy light center (pixels) / z radius (pixels) / w kind (0 point / 1 spot / 2 directional)
    lights_color: array<vec4<f32>, 64>,   // rgb already multiplied by intensity
    lights_dir: array<vec4<f32>, 64>,     // xy toward unit vector (pixel space) / z half-cone angle (radians), for spot
    shadow_ranges: array<vec4<f32>, 64>,  // per-light occluder range [start, count, 0, 0] (off = 0)
    occluders: array<vec4<f32>, 256>,     // merged large-box AABB [x0, y0, x1, y1], contiguous per light
    occluder_sub_ranges: array<vec4<f32>, 256>, // large-box sub-box range [start, count, 0, 0]
    occluder_subs: array<vec4<f32>, 256>, // original occluder AABB (one global copy)
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) nuv: vec2<f32>,    // normal-map UV; x<0 = sentinel (no normal)
    @location(3) rotcs: vec2<f32>,  // (cos, sin) of Sprite.rot
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
    // Pixel coordinates (top-left origin) → NDC
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

// Segment p → p+d vs AABB b intersection test (slab method, structurally identical line-by-line to the CPU path vitric-render's
// segment_hits_aabb — axis-aligned components do not divide, explicit branches, so inf semantics do not drift on either side).
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

// Shadow: is the segment from pixel p to light center l blocked by the occluder candidates of the li-th light (candidates = merged +
// per-light-culled large-box ranges, packed on the CPU side, isomorphic to vitric-render's blocked).
// Pixels outside a large box only test the large box (tile-aligned merge guarantees the union == large box); pixels inside a large box fall back to sub-boxes —
// the box the pixel itself is in is skipped: a pixel inside a box is only occluded by other boxes.
// When shadow_ranges[li].y = 0 (shadow off / candidates culled empty) the loop runs zero times, zero cost.
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
    // Normal-map sampling must be in uniform control flow (hard constraint of textureSample) —
    // unconditional sampling: sentinel nuv(-1,-1) is sampled by ClampToEdge to the atlas (0,0) corner, the result is discarded entirely in the branch
    let nraw = textureSample(atlas_tex, atlas_samp, in.nuv).rgb * 2.0 - 1.0;
    // Lighting — same formula as the CPU path (vitric-render module docs):
    //   lit = min(ambient + Σ each light contribution, 1.5); out = min(c·lit, 1)
    //   point: color·I·(1-d/r)²; spot: multiplied by angular attenuation t², t = clamp(1 - Δθ/half-cone, 0, 1)
    //   (deliberately uses t², not the built-in smoothstep — the CPU side uses the same formula); directional: color·I uniform everywhere.
    //   Normal pixels (nuv.x ≥ 0) each light contribution additionally ×= max(dot(N, L), 0); L's xy takes the unit direction
    //   from pixel to light ×0.8, z fixed 0.6 (NORMAL_LIGHT_XY/Z, semantics source in vitric-render).
    // Must happen **before** sRGB inverse: CPU multiplies directly on sRGB bytes, here we light in the same numerical space,
    // so the two paths look the same. in.pos is the framebuffer pixel coordinate (center +0.5), same as CPU's pixel center.
    // nuv.x < -1.5 = emissive sentinel (particle): skip lighting entirely — CPU path draws particles after lighting,
    // same semantics here; nuv.x = -1 (regular no-normal) is still lit as usual, old content behavior unchanged.
    if (globals.viewport.w > 0.5 && in.nuv.x > -1.5) {
        let has_n = in.nuv.x >= 0.0;
        var nrm = vec3<f32>(0.0, 0.0, 1.0);
        if (has_n) {
            // Decode (identical to CPU sample_normal): z takes absolute value, xy rotated by sprite rotation
            // (local→screen [[c, s], [-s, c]]), zero vector degenerates to plane normal
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
                // directional: sentinel pixel independent of distance/direction (old behavior); normal pixel computes by dir
                //   L = (-travel direction unit vector·0.8, 0.6) — same construction as CPU
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
                    // spot: Δθ = acos(pixel direction · toward); d=0 angle undefined, convention is cone center (t=1)
                    let ld = globals.lights_dir[i];
                    var cosd = 1.0;
                    if (d > 0.0) {
                        cosd = clamp(dot((in.pos.xy - lp.xy) / d, ld.xy), -1.0, 1.0);
                    }
                    let t = clamp(1.0 - acos(cosd) / ld.z, 0.0, 1.0);
                    f = f * t * t;
                }
                if (has_n) {
                    // L = unit direction from pixel to light center ×0.8, z fixed 0.6; d=0 convention (0,0,1)
                    var l = vec3<f32>(0.0, 0.0, 1.0);
                    if (d > 0.0) {
                        l = vec3<f32>((lp.xy - in.pos.xy) / d * 0.8, 0.6);
                    }
                    f = f * max(dot(nrm, l), 0.0);
                }
                // Fragments with zero contribution (outside cone / back-lit face) skip occlusion test (same order as CPU path).
                // Shadow: blocked by occluder = this light contributes zero (hard shadow; only point/spot, directional
                // already continued in the branch above, never reaches here)
                if (f > 0.0 && !shadowed(in.pos.xy, lp.xy, i)) {
                    lit = lit + globals.lights_color[i].rgb * f;
                }
            }
        }
        lit = min(lit, vec3<f32>(1.5));
        c = vec4<f32>(min(c.rgb * lit, vec3<f32>(1.0)), c.a);
    }
    // When the surface is sRGB format, the written value is interpreted as linear; here we inverse once so the final bytes match the CPU path
    if (globals.viewport.z > 0.5) {
        c = vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
"#;

/// Bloom downsample/threshold/box-blur share one shader: radius=0 + threshold≥0 + scale 2 = downsample bright-part extraction;
/// radius>0 + threshold<0 + scale 1 = single-direction box blur. Full-screen triangle vertex stream, textureLoad integer
/// sampling + manual clamp — out-of-bounds reads edge pixels, same semantics as the CPU path's clamp-to-edge.
const WGSL_BLOOM: &str = r#"
struct Post {
    params: vec4<f32>,   // x threshold (0..1, <0 = no threshold) / y blur radius (pixels) / zw blur direction
    params2: vec4<f32>,  // x sampling coordinate scale (downsample pass is 2, others 1)
};
@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var src: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // Full-screen triangle (3 vertices cover the clip space, no vertex buffer needed)
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
            // Bright-part extraction: max(scene - threshold, 0). Texture values 0..1, threshold directly uses 0..1
            // (CPU does the same subtraction in the 0..255 byte domain, numerically equivalent)
            c = max(c - vec3<f32>(post.params.x), vec3<f32>(0.0));
        }
        sum = sum + c;
    }
    return vec4<f32>(sum / f32(2 * r + 1), 1.0);
}
"#;

/// Bloom composite: scene + blurred bright-part·strength, clamped back to 1. sRGB surface inverse only in this last step
/// (the scene pass writes to an offscreen texture in raw byte space). The bloom texture is half-resolution, bilinearly sampled
/// to upscale — smoother than nearest-neighbor, no mosaic in the halo (visually aligned with CPU's full-resolution blur).
const WGSL_COMPOSITE: &str = r#"
struct Comp {
    params: vec4<f32>,   // x strength / y "surface is sRGB" flag
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
    // out = min(scene + bloom·strength, 1) — same additive composite formula as the CPU path
    var c = min(scene + bloom * comp.params.x, vec3<f32>(1.0));
    if (comp.params.y > 0.5) {
        c = srgb_to_linear(c);
    }
    return vec4<f32>(c, 1.0);
}
"#;

/// Uniform for bloom post-processing passes (downsample/blur/composite three pass types share this layout, field meanings see WGSL comments).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct PostParams {
    params: [f32; 4],
    params2: [f32; 4],
}

pub struct GpuPresenter {
    // surface holds the window handle reference, window goes first just by convenience; 'static because what's passed is Arc<Window>
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    /// Atlas texture changed → bind group must be rebuilt, so the two are stored together.
    bind_group: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    atlas: Atlas,
    /// Vector-font dynamic glyph atlas (texture + bind group + bookkeeping). None = project has no font.
    glyphs: Option<GlyphTexture>,
    /// Asset generation seen so far — compared with Dispatcher::assets_generation, rebuild atlas on change.
    seen_generation: u64,
    /// Whether the local GPU supports BC compressed textures (fixed at startup) — hot-reload atlas rebuild uses the same setting to compress BC7.
    bc_supported: bool,
    /// Vertex buffer grows on demand (capacity in bytes).
    vertex_buf: wgpu::Buffer,
    vertex_cap: u64,
    /// Background clear color (linear/raw value converted for the surface format).
    clear: wgpu::Color,
    /// Main shader module kept — reused when lazily building the bloom offscreen scene pipeline, not recompiled.
    scene_shader: wgpu::ShaderModule,
    /// Bloom pipeline set: lazily built the first time a Bloom entity appears on stage (never built if bloom is never on, zero cost).
    bloom_pipes: Option<BloomPipelines>,
    /// Bloom offscreen texture set: rebuilt when window size changes (pipeline set unchanged).
    bloom_targets: Option<BloomTargets>,
    window: Arc<Window>,
}

impl GpuPresenter {
    /// Initialize the wgpu family + first version of the atlas. Any step failing returns an error with context;
    /// the caller decides how to exit (project philosophy: failures must be explicit, no silent fallback).
    pub fn new(window: Arc<Window>, assets: &Assets, generation: u64) -> Result<Self, String> {
        // No display handle: Vulkan/DX12/Metal don't need it (only GL does),
        // from_env keeps WGPU_BACKEND and other env-var debug knobs
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
        // BC7 (compressed textures): if the adapter **supports** it, request TEXTURE_COMPRESSION_BC; build_atlas then
        // compresses the **entire runtime atlas** to BC7 for upload (hundreds of frames of characters = tens of MB VRAM vs RGBA8 fully resident 8.5G).
        // If the adapter does not support it, do not request; build_atlas falls back to the normal RGBA8 path, but at startup **explicitly states** uncompressed
        // (does not hard-fail the whole session — white tile / glyphs / UI are also in this atlas; does not silently bloat either, letting the user know).
        let bc_supported = adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vitric"),
            required_features: if bc_supported {
                wgpu::Features::TEXTURE_COMPRESSION_BC
            } else {
                wgpu::Features::empty()
            },
            ..Default::default()
        }))
        .map_err(|e| format!("创建 GPU 设备失败: {e}"))?;

        // Surface format: prefer non-sRGB (written bytes are what you see, same byte space as the CPU path);
        // only when sRGB is the only option, go through shader inverse (see WGSL comments)
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
            present_mode: wgpu::PresentMode::Fifo, // VSync, available on all backends
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Background color same bytes as CPU path (vitric_render::BACKGROUND); sRGB surface clear color is interpreted as linear, must inverse first
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
                    // Nearest-neighbor sampling without filtering — pixel-art textures stay crisp when upscaled, consistent with CPU nearest-neighbor
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
            build_atlas(&device, &queue, &bind_layout, &globals_buf, &sampler, assets, bc_supported)?;
        // Only build the glyph atlas if the project has a vector font (none = zero cost, bitmap glyphs already in the main atlas)
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
            bc_supported,
            vertex_buf,
            vertex_cap: INITIAL_VB,
            clear,
            scene_shader: shader,
            bloom_pipes: None,
            bloom_targets: None,
            window,
        })
    }

    /// Present one frame. If asset generation changed, rebuild the atlas first (takes effect on the first frame after hot-reloading assets).
    /// `tick` feeds screen-shake camera framing (vitric_render::camera_of) — shakes consistently with the CPU path.
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
                self.bc_supported,
            )?;
            self.atlas = atlas;
            self.bind_group = bind_group;
            // Font may also have been hot-reloaded / added / removed: rebuild the glyph atlas entirely (re-rasterize with the new font)
            self.glyphs = assets.font().is_some().then(|| {
                GlyphTexture::new(&self.device, &self.bind_layout, &self.globals_buf, &self.sampler)
            });
            self.seen_generation = generation;
        }

        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(()); // Minimized etc. zero-size state, skip frame
        }
        if size.width != self.config.width || size.height != self.config.height {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
        let (w, h) = (size.width, size.height);

        // Vertex stream: no font = old single-stream single draw; with font = sprite stream (main atlas) +
        // glyph stream (glyph atlas, including selection outline — it uses the glyph atlas's white tile), split into two segments by glyph_from
        // UI screen-space overlay: layout solved on the real window resolution (same pure function as the CPU path,
        // without going through the camera), UI nodes anchor to the viewport and scale naturally with the window. Empty UI = empty layout, zero cost.
        let ui_layout = if vitric_render::has_ui(world) {
            vitric_render::solve_layout(world, w, h)?
        } else {
            vitric_render::Layout::new()
        };
        let (verts, glyph_from) = if let Some(font) = assets.font() {
            let gt = self.glyphs.as_mut().ok_or(
                "内部不一致：素材仓库挂了字体但字形图集未建（素材代次没推进？）",
            )?;
            let mut verts = build_scene_vertices(world, w, h, &self.atlas, tick)?;
            // UI Panel (main atlas) before the split — drawn above the world, below text (same pipeline, main atlas binding)
            push_ui_panels(&mut verts, world, &ui_layout, &self.atlas)?;
            let split = verts.len();
            let cam = vitric_render::camera_of(world, tick, h)?;
            push_ttf_texts(&mut verts, world, w, h, font, &mut gt.atlas, cam)?;
            // Particles after text, before outline (same as build_vertices); vertices after the split bind
            // the glyph atlas, so the white tile uses the glyph atlas copy (same layout same pipeline, just swapping texture)
            push_emitter_particles(&mut verts, world, w, h, gt.atlas.white, cam, tick)?;
            // UI vector label (glyph atlas, after split) — drawn on the topmost layer (HUD)
            push_ui_ttf_labels(&mut verts, world, &ui_layout, font, &mut gt.atlas)?;
            if let Some(id) = selection {
                push_selection_outline(&mut verts, world, w, h, gt.atlas.white, id, cam);
            }
            (verts, split)
        } else {
            let mut verts = build_vertices(world, w, h, &self.atlas, selection, tick)?;
            // No font: UI Panel + UI bitmap label all in the main atlas (single stream), drawn after the world
            push_ui_panels(&mut verts, world, &ui_layout, &self.atlas)?;
            push_ui_bitmap_labels(&mut verts, world, &ui_layout, &self.atlas)?;
            let split = verts.len();
            (verts, split)
        };
        // Incremental upload of new glyphs this frame (write_texture queued before submit, guaranteed ready at draw time)
        if let Some(gt) = self.glyphs.as_mut() {
            gt.flush_uploads(&self.queue);
        }

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Timeout | Cst::Occluded => return Ok(()), // Transient / occluded, skip a frame
            Cst::Outdated | Cst::Lost => {
                // Window just changed / surface lost: reconfigure and try once more, real failure if still bad
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
        // Bloom switch = whether a Bloom entity is on stage (semantics source in vitric-render, parameter validation also there).
        // When bloom is on, the scene pass renders into a non-sRGB offscreen texture, and sRGB inverse moves to the composite pass
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
                // Old path preserved as-is: single pass direct-to-surface render, no extra texture / pass cost
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
                // Multi-pass structure see module docs. Pipeline set lazily built once; texture set rebuilt with window size
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

                // Write small uniforms per frame (parameters may be changed by rules/scripts, texture bindings unchanged)
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

                // Scene pass → offscreen (clear color uses raw byte values: offscreen texture is always non-sRGB format)
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
                // Downsample+threshold + blur ×6 (full-screen triangle, targets already arranged in post_views)
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
                // Composite pass → surface (sRGB inverse happens here, full-screen triangle covers everything, clear color does not matter)
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

/// Whether the scene pass should do sRGB inverse in the shader: not done when bloom is on (scene writes to non-sRGB offscreen texture,
/// inverse moves to the composite pass); when off, keeps the old behavior (direct-to-surface, only inverse if surface is sRGB).
/// Split into a pure function: pass-selection logic is unit-testable without a GPU.
fn scene_pass_srgb(surface_srgb: bool, bloom_active: bool) -> bool {
    surface_srgb && !bloom_active
}

/// Parameters for bloom's 7 post-processing passes (pure function without GPU, locked by unit tests):
/// [0] downsample+threshold (radius 0, sampling scale 2 — half-resolution pixel maps back to full-resolution scene),
/// [1..=6] box blur H/V alternating 3 rounds, radius = half of CPU's radius (lower bound 1, because it runs at half resolution,
/// spatial scale consistent with CPU's full-resolution blur), threshold set to -1 means no more thresholding.
fn bloom_pass_params(viewport_h: u32, threshold: f64) -> [PostParams; 7] {
    let r_half = (vitric_render::bloom_radius_px(viewport_h) / 2).max(1) as f32;
    let mut out = [PostParams { params: [0.0; 4], params2: [0.0; 4] }; 7];
    out[0] = PostParams {
        params: [threshold as f32, 0.0, 0.0, 0.0],
        params2: [2.0, 0.0, 0.0, 0.0],
    };
    for i in 0..6 {
        // Even index horizontal, odd vertical (counting from 0): H→V is one round of complete separable blur
        let dir = if i % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
        out[i + 1] = PostParams {
            params: [-1.0, r_half, dir[0], dir[1]],
            params2: [1.0, 0.0, 0.0, 0.0],
        };
    }
    out
}

/// Half-resolution size of bloom intermediate textures (lower bound 1, tiny windows do not go to zero).
fn half_dims(w: u32, h: u32) -> (u32, u32) {
    ((w / 2).max(1), (h / 2).max(1))
}

/// Bloom pipeline set: the parts independent of window size, lazily built once and resident.
struct BloomPipelines {
    /// Variant of the main shader rendering into offscreen Rgba8Unorm (the direct-to-surface pipeline is still in GpuPresenter.pipeline).
    scene_pipeline: wgpu::RenderPipeline,
    post_layout: wgpu::BindGroupLayout,
    post_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    /// Bilinear sampler: smoothly upscales the half-resolution bloom texture at composite time (see WGSL_COMPOSITE comments).
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

/// Bloom offscreen texture set: rebuilt with window size (triggered by size comparison in present).
struct BloomTargets {
    size: (u32, u32),
    scene_view: wgpu::TextureView,
    /// Uniforms / bind groups / target views for the 7 post-processing passes (index aligned with bloom_pass_params).
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

        // Pass chain: downsample (scene→ping), then H/V alternating (ping→pong→ping…), 3 rounds land on ping.
        // The source/target view arrangement must match the direction order of bloom_pass_params
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
                    // Final result of the 3-round H/V blur chain is in ping (see pass chain comment above)
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

/// Format of all bloom offscreen textures: non-sRGB — all intermediate calculations are in raw byte space,
/// same numerical domain as the CPU path (sRGB inverse only happens once in the final composite pass).
const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Main 2D pipeline (vertex stream + atlas + lighting shader). Direct-to-surface and bloom's offscreen scene pass
/// use the same shader, same vertex layout, only the target format differs — factored out and built twice.
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
                // Standard src-alpha blending, aligned with the CPU path's per-pixel alpha blending
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None, // Painter's algorithm: later draws over earlier by entity order, no depth needed
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Post-processing pipeline (shared skeleton for bloom blur/composite): full-screen triangle, no vertex buffer, no blending (direct overwrite).
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
// Atlas building: white tile + 128 glyphs + all assets, packed into one texture at once
// ---------------------------------------------------------------------------

/// Atlas fixed width: single-image upper bound 2048 (enforced at Assets import) + 1px copy border on both sides.
const ATLAS_W: u32 = 2050;

/// **Real size** of the atlas texture: BC7 encodes in 4×4 blocks, width/height each rounded up to a multiple of 4 (block grid),
/// RGBA8 as-is. The UV normalization denominator must use this size — otherwise BC7's rounding padding makes the whole sampling
/// shift overall (content is still in the (0..logical_width, 0..logical_height) sub-region, denominator uses the rounded size so UVs point precisely into content).
/// Pure function, the container can unit-test lock the "denominator = real texture size" invariant without a GPU.
fn atlas_tex_dims(bc7: bool, w: u32, h: u32) -> (u32, u32) {
    if bc7 {
        (w.div_ceil(4) * 4, h.div_ceil(4) * 4)
    } else {
        (w, h)
    }
}

/// `vitric gpu-probe [rgba8|bc7]`: **headless** real-machine diagnostic (no window, SSH/Session 0 can run) — verifies "compressing the entire atlas to BC7 really saves VRAM" on the local
/// real GPU, something the container cannot test.
///
/// **Single format, clean baseline**: only allocates large textures of one format at a time, measuring from this process's baseline to the post-allocation nvidia-smi
/// delta. The two formats run in two separate processes — each with a fresh baseline, avoiding the illusion that "the first one occupies a big chunk, the latter falls into the already-reserved heap" causing
/// the delta to be eaten by the driver heap reservation. Defaults to bc7.
pub fn gpu_probe(args: &[String]) -> Result<(), String> {
    // Atlas size that yields a clear signal: 2048×8192, RGBA8=64MB / BC7=16MB
    const W: u32 = 2048;
    const H: u32 = 8192;
    let want_bc7 = !matches!(args.first().map(|s| s.as_str()), Some("rgba8"));
    let count: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(6);

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    // Headless: no surface (compatible_surface=None), so no desktop window needed
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| format!("找不到 GPU 适配器: {e}"))?;
    let info = adapter.get_info();
    let bc_supported = adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    println!("适配器: {} ({:?}, {:?})", info.name, info.device_type, info.backend);
    println!("TEXTURE_COMPRESSION_BC 支持: {}", if bc_supported { "是" } else { "否" });
    if want_bc7 && !bc_supported {
        println!("→ 本适配器不支持 BC，引擎会走 best-effort RGBA8（显存未压缩）。换独显再测。");
        return Ok(());
    }

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("vitric-probe"),
        required_features: if bc_supported {
            wgpu::Features::TEXTURE_COMPRESSION_BC
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    }))
    .map_err(|e| format!("创建 GPU 设备失败: {e}"))?;

    // wgpu creating a device comes with a large reserved VRAM pool, small allocations fall into the pool and nvidia-smi sees no delta.
    // Allocating count textures of the same format at once, the total far exceeds the reserved pool, the real usage difference (4×) shows up in nvidia-smi.
    let rgba = vec![128u8; (W as usize) * (H as usize) * 4];
    let base = vram_used_mb();
    let mut keep: Vec<wgpu::Texture> = Vec::with_capacity(count);
    let (label, one_mb) = if want_bc7 {
        let (bx, by, blocks) = vitric_cli::bc7::encode_rgba8(W, H, &rgba)?;
        let one = (bx as u64 * by as u64 * vitric_cli::bc7::BLOCK_BYTES as u64) / 1_048_576;
        for _ in 0..count {
            keep.push(upload_probe_texture(
                &device, &queue, wgpu::TextureFormat::Bc7RgbaUnorm, bx * 4, by * 4, &blocks, Some(bx),
            ));
        }
        ("BC7", one)
    } else {
        let one = (W as u64 * H as u64 * 4) / 1_048_576;
        for _ in 0..count {
            keep.push(upload_probe_texture(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, W, H, &rgba, None));
        }
        ("RGBA8", one)
    };
    let after = vram_used_mb();
    let total_mb = one_mb * count as u64;

    println!("--- 图集 {W}x{H}，格式 {label} ×{count} ---");
    println!("单张字节 {one_mb}MB × {count} = {total_mb}MB（字节理论值）");
    match (base, after) {
        (Some(b), Some(a)) => println!(
            "nvidia-smi 全卡已用: 基线 {b}MB → 分配 {count} 张 {label} 后 {a}MB（**实测增 {}MB**）",
            a.saturating_sub(b)
        ),
        _ => println!("（nvidia-smi 不可用，跳过显存实测；字节理论值见上方）"),
    }
    drop(keep);
    Ok(())
}

/// Probe texture upload: create texture + write data + submit + wait for GPU completion (ensure VRAM is really allocated before measuring).
/// `bc7_blocks_per_row`=Some(blocks per row) uses BC7 block layout, None uses RGBA8 row layout.
fn upload_probe_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    data: &[u8],
    bc7_blocks_per_row: Option<u32>,
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vitric-probe-tex"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes_per_row = match bc7_blocks_per_row {
        Some(bx) => bx * vitric_cli::bc7::BLOCK_BYTES as u32, // BC7: blocks per row × 16
        None => w * 4,                                        // RGBA8: width × 4
    };
    let rows = match bc7_blocks_per_row {
        Some(_) => h / 4, // BC7 row count = pixel height / 4 (block grid)
        None => h,
    };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(rows),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely()); // Wait for the write to land, only then is VRAM really occupied
    std::thread::sleep(std::time::Duration::from_millis(800)); // Give the driver / nvidia-smi time to settle
    texture
}

/// Read nvidia-smi's **whole-card** used VRAM (MB). The probe is the only large allocator, the delta is the texture footprint.
/// If nvidia-smi is not on PATH (non-N card / not installed) returns None, the probe degrades to reporting only the byte ratio.
fn vram_used_mb() -> Option<u64> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse::<u64>().ok()
}

fn build_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bind_layout: &wgpu::BindGroupLayout,
    globals_buf: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    assets: &Assets,
    bc_supported: bool,
) -> Result<(Atlas, wgpu::BindGroup), String> {
    // Items to pack: (key, width, height, RGBA pixels). Fixed order: white tile, glyphs, assets (BTreeMap already sorted)
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

    // Shelf packing: each item leaves a 1px replicated border on all sides (nearest-neighbor sampling's floating-point jitter at region edges won't bleed into neighboring images)
    let mut placements: Vec<(u32, u32)> = Vec::with_capacity(items.len()); // top-left of content region
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

    // Lay pixels: content region direct copy, replicated border takes nearest content pixel (equivalent to clamp)
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

    // Format selection: if the device supports BC, the **entire runtime atlas** is compressed to BC7 (1/4 VRAM, directly addressing the root cause of 8.5G
    // RGBA8 fully-resident cases). White tile / bitmap glyphs / any solid-color region are **lossless** under this encoder's min/max endpoints —
    // white = max channel value, transparent black = min, both land exactly on endpoints; only sprite art's smooth gradient blocks are lossy, which is exactly
    // what BC7 was designed to handle and is the accepted trade-off when the contract chooses BC7. Devices without BC support don't hard-fail the whole run (white tile /
    // glyphs / UI also live in this atlas), keep RGBA8 — but the startup phase **explicitly says so**, preventing VRAM from silently ballooning back to 4x.
    let mb = |w: u32, h: u32| w as u64 * h as u64 * 4 / 1_048_576;
    let bc7 = bc_supported.then(|| vitric_cli::bc7::encode_rgba8(ATLAS_W, atlas_h, &pixels)).transpose()?;
    let (tex_w, tex_h) = atlas_tex_dims(bc7.is_some(), ATLAS_W, atlas_h);
    match &bc7 {
        Some((bx, by, _)) => eprintln!(
            "[vitric] 素材图集 {ATLAS_W}x{atlas_h} → BC7 压缩纹理（显存 {}MB → {}MB，1/4）",
            mb(ATLAS_W, atlas_h),
            (*bx as u64 * *by as u64 * vitric_cli::bc7::BLOCK_BYTES as u64) / 1_048_576
        ),
        None => eprintln!(
            "[vitric] 素材图集 {ATLAS_W}x{atlas_h} 保持 RGBA8（本机 GPU 不支持 BC 压缩，\
             显存未压缩 {}MB；换支持 BC 的桌面 GPU 可砍 4 倍）",
            mb(ATLAS_W, atlas_h)
        ),
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vitric-atlas"),
        size: wgpu::Extent3d { width: tex_w, height: tex_h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Both paths are non-sRGB: sampling returns to raw byte space, tint multiplication happens in byte space (same convention as CPU path)
        format: match &bc7 {
            Some(_) => wgpu::TextureFormat::Bc7RgbaUnorm,
            None => wgpu::TextureFormat::Rgba8Unorm,
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    match &bc7 {
        // BC7: one block is 4×4=16 bytes, bytes_per_row = blocks per row × 16, copy range uses the block-grid rounded dimensions
        Some((bx, by, blocks)) => queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            blocks,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bx * vitric_cli::bc7::BLOCK_BYTES as u32),
                rows_per_image: Some(*by),
            },
            wgpu::Extent3d { width: tex_w, height: tex_h, depth_or_array_layers: 1 },
        ),
        None => queue.write_texture(
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
        ),
    }

    // UV index table: normalized by **texture real dimensions** tex_w/tex_h (see atlas_tex_dims — after BC7 rounding the
    // denominator is larger, the content sub-region's UV precisely points into the content, padding rows/columns are never sampled). RGBA8 path
    // tex_w/tex_h == ATLAS_W/atlas_h, UV is bit-for-bit identical to before the change (zero regression).
    let uv = |ox: u32, oy: u32, w: u32, h: u32| -> UvRect {
        [
            ox as f32 / tex_w as f32,
            oy as f32 / tex_h as f32,
            (ox + w) as f32 / tex_w as f32,
            (oy + h) as f32 / tex_h as f32,
        ]
    };
    let mut images = std::collections::BTreeMap::new();
    let mut white = [0.0f32; 2];
    let mut glyphs = [[0.0f32; 4]; 128];
    for ((key, w, h, _), (ox, oy)) in items.iter().zip(&placements) {
        if key == "\u{0}white" {
            // White tile takes the center point (center of the 2x2), all four corners sample the same UV; denominator uses texture real dimensions same as the uv closure
            white = [(*ox as f32 + 1.0) / tex_w as f32, (*oy as f32 + 1.0) / tex_h as f32];
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
// Per-frame vertex stream: mirrors vitric-render's visual semantics (same component conventions, same coordinate transforms)
// ---------------------------------------------------------------------------

/// Accumulate one frame's uniform (pure function, no GPU access; lighting pack layout is locked by unit tests).
/// Light parameters are transformed from world coordinates to pixel space here (the view includes Shake jitter — light follows the frame,
/// same transform set as the CPU path's apply_lighting); the shader's inner loop only computes distance.
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
    // Lighting master switch = whether an Ambient entity exists in the world (semantic source is in vitric-render)
    if let Some((ambient, _)) = vitric_render::ambient_of(world)? {
        let lights = vitric_render::collect_lights(world)?;
        let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick, height)?;
        g.viewport[3] = 1.0;
        g.ambient = [ambient[0] as f32, ambient[1] as f32, ambient[2] as f32, lights.len() as f32];
        for (i, l) in lights.iter().enumerate() {
            // Pack layout see Globals docs: pos.w = kind, dir = [facing pixel-space unit vector, half-cone angle in radians, 0]
            let (kind, dir_deg, half_rad) = match l.kind {
                vitric_render::LightKind::Point => (0.0, None, 0.0),
                vitric_render::LightKind::Spot { angle, dir } => {
                    (1.0, Some(dir), (angle / 2.0).to_radians())
                }
                vitric_render::LightKind::Directional { dir } => (2.0, Some(dir), 0.0),
            };
            // Directional lights skip the world→pixel transform (feeding placeholder 0 into the transform yields screen center, polluting slot semantics) —
            // position/radius are packed as 0 directly, the shader never touches them in the kind branch
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
                // World angle (degrees, 0=+x counterclockwise positive) → pixel-space unit vector: y flip → (cos, -sin),
                // bit-for-bit identical to the CPU path's apply_lighting precomputation
                let rad = dir.to_radians();
                g.lights_dir[i] = [rad.cos() as f32, (-rad.sin()) as f32, half_rad as f32, 0.0];
            }
        }
        // Projection: occluders are merged into large boxes (pixel space, same view transform as lights, including Shake jitter),
        // then culled per-light by light disc and packed into contiguous ranges. The semantic source for switch/collect/limit/merge/cull all lives in
        // vitric-render; off = shadow_ranges all zero, the shader's occlusion loop executes zero times
        if vitric_render::shadows_of(world)? {
            let occs = vitric_render::collect_occluders(world)?;
            let grid = vitric_render::build_shadow_boxes(&occs, width, height, (cam_x, cam_y, scale));
            for (s, b) in grid.subs.iter().enumerate() {
                g.occluder_subs[s] = [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32];
            }
            let mut cursor = 0usize;
            for (i, l) in lights.iter().enumerate() {
                // Directional lights don't project shadows (v1): range left all zero, shader loop zero iterations
                if matches!(l.kind, vitric_render::LightKind::Directional { .. }) {
                    continue;
                }
                // Cull using the same f64 pixel-space light parameters as the CPU path (f32 packing happens only at the end)
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

/// Split one vertex stream into two draws at glyph_from: the first segment binds the main atlas, the second binds the glyph atlas
/// (when no font is attached, glyph_from == len, the second segment is empty, behavior is fully identical to a single draw).
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

/// Legacy single-stream path (no font attached): background/sprites/bitmap text/outline all in one stream of the main atlas.
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
    // Particles come after text (CPU path draws particles after lighting = on top of sprites/text, same painter's-order semantics)
    push_emitter_particles(&mut verts, world, width, height, atlas.white, (cam_x, cam_y, scale), tick)?;
    // Selection outline: teal 2px, drawn on top (geometry aligns with vitric_render::draw_selection_outline).
    // Known minor discrepancy: when lighting is on, the outline in the GPU window gets lit too (same pipeline), the CPU window path
    // draws the outline after rendering so it isn't lit — this is inspector debug decoration, doesn't enter screenshots/asserts, not worth opening a second pipeline for it
    if let Some(id) = selection {
        push_selection_outline(&mut verts, world, width, height, atlas.white, id, (cam_x, cam_y, scale));
    }
    Ok(verts)
}

/// Background (when lighting is on) + sprite stream (main atlas). Text/outline are stitched by the caller per path.
fn build_scene_vertices(
    world: &World,
    width: u32,
    height: u32,
    atlas: &Atlas,
    tick: u64,
) -> Result<Vec<Vertex>, String> {
    // The view (including Shake jitter offset) uses vitric-render's implementation directly — both paths jitter bit-for-bit identically
    let (cam_x, cam_y, scale) = vitric_render::camera_of(world, tick, height)?;
    let mut verts: Vec<Vertex> = Vec::new();

    // When lighting is on the background must also be lit (CPU path runs the whole buf through the formula): clear color can't vary per-pixel,
    // so first lay a full-screen background quad, letting the shader light background and entities together
    if vitric_render::ambient_of(world)?.is_some() {
        push_solid(&mut verts, atlas.white, 0.0, 0.0, width as f32, height as f32, vitric_render::BACKGROUND);
    }

    // Sprites: in entity order (painter's algorithm, later draws cover earlier ones)
    // NOTE: View-frustum culling is intentionally NOT mirrored here. The CPU rasterizer
    // (vitric_render::render_with) culls off-screen sprites — that is the source of truth for
    // screenshots/assertions/gate (Task 4 / E4). The GPU path is only used for the live
    // interactive window display; off-screen sprites cost a tiny bit of vertex-stream overhead
    // but render the same visible pixels. Culling here would be a pure perf optimization and
    // risks diverging from the CPU's culling math (causing visible differences between window
    // and screenshot). The CPU path's AABB check uses (cam_x ± view_w_world/2) with the rotated
    // bounding-box extent; if GPU culling is needed later, port that exact check here.
    for id in world.query(&["Position", "Sprite"]) {
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        let rot = vitric_render::rot_of(world, id)?;
        // World → screen pixel (y flip, camera centered) — same formula as the CPU path
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;
        // Four corners (fixed order: top-left/top-right/bottom-right/bottom-left when unrotated, UV follows the corner).
        // When rot != 0 rotate around center — same angle convention as the CPU path (vitric_render::rot_of):
        // degrees, world counterclockwise positive; screen y flip → screen-system positive matrix [[c, s], [-s, c]]
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
            // Missing image is a hard error (no placeholder drawn) — error message aligns with the CPU path
            let uv = atlas.images.get(&image_name).ok_or_else(|| {
                format!(
                    "实体 {id} 的 Sprite.image {image_name:?} 不在素材仓库里。\
                     现有素材: [{}]。提示：图放进项目 assets/ 目录，路径相对 assets/ 写",
                    atlas.images.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            // Normal maps are paired by name (semantics sourced from vitric_render::normal_map_name) — it's
            // just a regular PNG in assets/, already baked into the same atlas; no pair = sentinel = legacy lighting path
            let nuv = vitric_render::normal_map_name(&image_name)
                .and_then(|n| atlas.images.get(&n))
                .copied()
                .unwrap_or(NO_NORMAL);
            // (cos, sin) of rot: the fragment uses it to rotate normals into screen space (rot=0 → (1,0) identity)
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

/// Bitmap text stream (legacy path with no font attached): one main-atlas glyph quad per character, the whole string centered on Position,
/// drawn above the sprite. Always upright — Sprite.rot only rotates the sprite, not the text (same semantics as the CPU path).
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
        // reveal: same convention as the CPU bitmap path — truncate to the first visible characters, center on the visible count
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
        // screen=true: HUD anchoring — same semantics as the CPU path, coordinates are relative to screen center, independent of camera
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
                // Non-ASCII: solid square placeholder, same as the CPU path
                push_solid(verts, atlas.white, x0, y0, x1, y1, rgba);
            }
        }
    }
    Ok(())
}

/// Vector text stream (font attached): layout/rasterization/rounding all go through vitric_render::FontStore —
/// same geometry as the CPU path (draw_text_vector), one glyph-atlas quad per glyph.
/// New (character, pixel size) pairs are lazily allocated here and queued for upload; atlas-full is an explicit error.
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
        // reveal (same convention as the CPU path): visible char count = pure function; default 1.0 = show all
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
        // Cached layout + draw only the first visible glyphs: same data and same visible count as the CPU path
        let laid = font.layout_cached(&content, px_size);
        let (placements, total_w) = (&laid.0, laid.1);
        let left = cx - total_w as f64 / 2.0;
        let baseline = (cy + font.baseline_offset(px_size) as f64).round();
        for p in placements.iter().take(visible) {
            let g = font.raster(p.ch, px_size);
            if g.coverage.is_empty() {
                continue; // Empty outline (space, etc.) only takes advance, never enters the atlas
            }
            let uv = glyph_atlas.glyph_uv(font, p.ch, px_size)?;
            let x0 = ((left + p.x as f64).round() + g.left as f64) as f32;
            let y0 = (baseline + g.top as f64) as f32;
            push_quad(verts, x0, y0, x0 + g.width as f32, y0 + g.height as f32, uv, tint(rgba));
        }
    }
    Ok(())
}

/// UI Panel stream (screen space, main atlas): background frame, solid color or sprite texture. Layout comes from
/// vitric_render::solve_layout (the same pure function as the CPU path, no camera involved).
/// All use the [`UNLIT`] sentinel — UI is an overlay layer, the CPU path draws it after lighting/bloom so it isn't lit, same semantics.
/// Screen space = place vertices directly from the solved screen-pixel rectangle, no off-screen target, reusing the same vertex stream.
fn push_ui_panels(
    verts: &mut Vec<Vertex>,
    world: &World,
    layout: &vitric_render::Layout,
    atlas: &Atlas,
) -> Result<(), String> {
    for id in world.query(&["Ui", "Panel"]) {
        let Some(base) = layout.get(&id) else { continue };
        // Press feedback: CPU/GPU share ui_press_feedback (scale around center + brighten), the formulas are structurally identical line by line.
        let (r, modulate) = vitric_render::ui_press_feedback(world, id, *base);
        let (x0, y0) = (r.x as f32, r.y as f32);
        let (x1, y1) = ((r.x + r.w) as f32, (r.y + r.h) as f32);
        let image_name = world
            .get_field(id, "Panel.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        if image_name.is_empty() {
            let color = world
                .get_field(id, "Panel.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let mut rgba = parse_color_a(&color).map_err(|e| format!("实体 {id} 的 Panel.color: {e}"))?;
            vitric_render::modulate_rgb(&mut rgba, modulate);
            let [u, v] = atlas.white;
            push_quad_corners_n(verts, corners, [u, v, u, v], UNLIT, [1.0, 0.0], tint(rgba));
        } else {
            let uv = atlas.images.get(&image_name).ok_or_else(|| {
                format!(
                    "实体 {id} 的 Panel.image {image_name:?} 不在素材仓库里。现有素材: [{}]",
                    atlas.images.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            push_quad_corners_n(verts, corners, *uv, UNLIT, [1.0, 0.0], [1.0; 4]);
        }
    }
    Ok(())
}

/// UI Label bitmap text stream (no font attached, main atlas): one glyph quad per character, aligned horizontally by align within the node frame
/// and vertically centered. Same geometry as the CPU draw_ui_label bitmap path. All [`UNLIT`].
fn push_ui_bitmap_labels(
    verts: &mut Vec<Vertex>,
    world: &World,
    layout: &vitric_render::Layout,
    atlas: &Atlas,
) -> Result<(), String> {
    for id in world.query(&["Ui", "UiLabel"]) {
        let Some(r) = layout.get(&id) else { continue };
        let Some((chars, size, rgba, _align)) = read_ui_label(world, id)? else { continue };
        let n = chars.len() as f64;
        let total_w = n * size;
        let left = ui_label_left(world, id, r, total_w)?;
        let top = r.y + r.h / 2.0 - size / 2.0;
        let (y0, y1) = (top as f32, (top + size) as f32);
        for (i, &c) in chars.iter().enumerate() {
            let x0 = (left + i as f64 * size) as f32;
            let x1 = (left + (i + 1) as f64 * size) as f32;
            let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            let cp = c as usize;
            if cp < 128 {
                push_quad_corners_n(verts, corners, atlas.glyphs[cp], UNLIT, [1.0, 0.0], tint(rgba));
            } else {
                let [u, v] = atlas.white;
                push_quad_corners_n(verts, corners, [u, v, u, v], UNLIT, [1.0, 0.0], tint(rgba));
            }
        }
    }
    Ok(())
}

/// UI Label vector text stream (font attached, glyph atlas): same
/// layout/rasterization/rounding as the CPU draw_ui_label vector path (vitric_render::FontStore), font size = screen pixels (scale=1). All [`UNLIT`].
fn push_ui_ttf_labels(
    verts: &mut Vec<Vertex>,
    world: &World,
    layout: &vitric_render::Layout,
    font: &vitric_render::FontStore,
    glyph_atlas: &mut GlyphAtlas,
) -> Result<(), String> {
    use vitric_render::FontStore;
    for id in world.query(&["Ui", "UiLabel"]) {
        let Some(r) = layout.get(&id) else { continue };
        let Some((chars, size, rgba, _align)) = read_ui_label(world, id)? else { continue };
        let content: String = chars.iter().collect();
        let px_size = FontStore::px_size(size, 1.0);
        let laid = font.layout_cached(&content, px_size);
        let (placements, total_w) = (&laid.0, laid.1);
        let left = ui_label_left(world, id, r, total_w as f64)?;
        let cy = r.y + r.h / 2.0;
        let baseline = (cy + font.baseline_offset(px_size) as f64).round();
        for p in placements.iter() {
            let g = font.raster(p.ch, px_size);
            if g.coverage.is_empty() {
                continue;
            }
            let uv = glyph_atlas.glyph_uv(font, p.ch, px_size)?;
            let x0 = ((left + p.x as f64).round() + g.left as f64) as f32;
            let y0 = (baseline + g.top as f64) as f32;
            push_quad_corners_n(
                verts,
                [[x0, y0], [x0 + g.width as f32, y0], [x0 + g.width as f32, y0 + g.height as f32], [x0, y0 + g.height as f32]],
                uv,
                UNLIT,
                [1.0, 0.0],
                tint(rgba),
            );
        }
    }
    Ok(())
}

/// Read UiLabel's visible characters (already truncated by reveal) + size + color + align.
/// Empty / fully hidden / zero size = None (caller continues). Shared by both GPU text paths.
#[allow(clippy::type_complexity)]
fn read_ui_label(
    world: &World,
    id: vitric_ecs::EntityId,
) -> Result<Option<(Vec<char>, f64, [u8; 4], String)>, String> {
    let content = world
        .get_field(id, "UiLabel.content")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    if content.is_empty() {
        return Ok(None);
    }
    let size = num(world, id, "UiLabel.size").unwrap_or(1.0);
    if size <= 0.0 {
        return Ok(None);
    }
    let color = world
        .get_field(id, "UiLabel.color")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "#ffffff".to_string());
    let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 UiLabel.color: {e}"))?;
    let reveal = world
        .get_field(id, "UiLabel.reveal")
        .ok()
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let visible = vitric_render::revealed_chars(reveal, content.chars().count());
    if visible == 0 {
        return Ok(None);
    }
    let align = world
        .get_field(id, "UiLabel.align")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "center".to_string());
    Ok(Some((content.chars().take(visible).collect(), size, rgba, align)))
}

/// Horizontal start of the text within the node frame (by align). Same convention as CPU draw_ui_label.
fn ui_label_left(
    world: &World,
    id: vitric_ecs::EntityId,
    r: &vitric_render::UiRect,
    total_w: f64,
) -> Result<f64, String> {
    let align = world
        .get_field(id, "UiLabel.align")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "center".to_string());
    Ok(match align.as_str() {
        "start" => r.x,
        "end" => r.x + r.w - total_w,
        _ => r.x + (r.w - total_w) / 2.0,
    })
}

/// Particle stream: emitters expand via a pure function (semantics sourced from vitric_render::emitter_particles —
/// position/count/color are the same data as the CPU path), one white-quad per particle + tint.
/// nuv uses the [`UNLIT`] sentinel: the fragment skips lighting (self-emissive, the CPU path draws particles after lighting, same semantics).
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
            // Same world→pixel transform as the CPU rasterizer (dot center + half extent)
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
    // White-quad center UV: the bitmap path uses the main atlas, the font path uses the glyph atlas (the outline shares the glyph's bind group)
    white: [f32; 2],
    id: vitric_ecs::EntityId,
    (cam_x, cam_y, scale): (f64, f64, f64),
) {
    if !world.is_alive(id) || !world.has_component(id, "Sprite") {
        return; // Selected entity is gone/invisible, skip silently (same as the CPU path)
    }
    let field = |path: &str| num(world, id, path).ok();
    let (Some(x), Some(y), Some(w), Some(h)) = (
        field("Position.x"),
        field("Position.y"),
        field("Sprite.w"),
        field("Sprite.h"),
    ) else {
        return; // Broken field doesn't block presentation (the CPU path's caller also ignores it via let _ =)
    };
    // When rot != 0 take the axis-aligned bounding box of the rotated shape — same choice as the CPU path (highlights don't need edge-exact fit)
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
    // Same as the CPU path: clamp to the screen first, then draw the frame (off-screen edges hug the screen border)
    let x0 = (cx - half_w).floor().max(0.0) as f32;
    let x1 = ((cx + half_w).ceil().min(width as f64) - 1.0) as f32;
    let y0 = (cy - half_h).floor().max(0.0) as f32;
    let y1 = ((cy + half_h).ceil().min(height as f64) - 1.0) as f32;
    if x1 < x0 || y1 < y0 {
        return;
    }
    const TEAL: [u8; 4] = [39, 192, 168, 255];
    // Four 2px edges (coverage = union of the CPU's double-ring puts)
    push_solid(verts, white, x0, y0, x1 + 1.0, y0 + 2.0, TEAL); // top
    push_solid(verts, white, x0, y1 - 1.0, x1 + 1.0, y1 + 1.0, TEAL); // bottom
    push_solid(verts, white, x0, y0, x0 + 2.0, y1 + 1.0, TEAL); // left
    push_solid(verts, white, x1 - 1.0, y0, x1 + 1.0, y1 + 1.0, TEAL); // right
}

/// Solid rectangle: samples the white-quad center (all four corners share the same UV), color comes entirely from the tint.
fn push_solid(verts: &mut Vec<Vertex>, white: [f32; 2], x0: f32, y0: f32, x1: f32, y1: f32, rgba: [u8; 4]) {
    let [u, v] = white;
    push_quad(verts, x0, y0, x1, y1, [u, v, u, v], tint(rgba));
}

/// Two triangles form one rectangle (no index buffer; the vertex stream is small enough not to be worth it).
fn push_quad(verts: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, uv: UvRect, color: [f32; 4]) {
    push_quad_corners(verts, [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], uv, color);
}

/// Quad with arbitrary corners (used for sprite rotation). Corner order = unrotated top-left/top-right/bottom-right/bottom-left,
/// the UV rectangle expands in the same corner order and follows the corners — the texture rotates with the corners, no misalignment.
/// Primitives without a normal map (solid/text/glyph) all go through this: nuv sentinel, rotcs identity placeholder.
fn push_quad_corners(verts: &mut Vec<Vertex>, p: [[f32; 2]; 4], uv: UvRect, color: [f32; 4]) {
    push_quad_corners_n(verts, p, uv, NO_NORMAL, [1.0, 0.0], color);
}

/// Quad with a normal-map region: nuv expands in the same corner order as uv (the normal rotates with the texture).
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

// ---- The two small helpers below mirror vitric-render's private implementation (the semantic source for component conventions lives there) ----

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

/// Color parsing (with optional alpha): `#rrggbb` or `#rrggbbaa`. UI Panel backgrounds often need a semi-transparent mask.
/// Same convention as the CPU path's vitric_render parse_color_a (visual alignment).
fn parse_color_a(s: &str) -> Result<[u8; 4], String> {
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| format!("颜色 {s:?} 格式不对。写法: \"#rrggbb\" 或带透明度 \"#rrggbbaa\""))?;
    if (hex.len() != 6 && hex.len() != 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 {s:?} 必须是 6 位 \"#rrggbb\" 或 8 位 \"#rrggbbaa\" 十六进制"));
    }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("已校验十六进制");
    let a = if hex.len() == 8 { p(6) } else { 255 };
    Ok([p(0), p(2), p(4), a])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RGBA8 path: the texture's real size == logical size (UV denominator unchanged, bit-for-bit identical before and after).
    #[test]
    fn atlas_tex_dims_rgba8_unchanged() {
        assert_eq!(atlas_tex_dims(false, ATLAS_W, 100), (ATLAS_W, 100));
        assert_eq!(atlas_tex_dims(false, ATLAS_W, 101), (ATLAS_W, 101));
    }

    /// BC7 path: width and height are each rounded up to a multiple of 4 (block grid), the denominator must use the rounded size.
    #[test]
    fn atlas_tex_dims_bc7_rounds_up_to_block_grid() {
        // 2050 is not a multiple of 4 → 2052; height 100 is already a multiple of 4 → unchanged
        assert_eq!(atlas_tex_dims(true, ATLAS_W, 100), (2052, 100));
        // Height 101 → 104
        assert_eq!(atlas_tex_dims(true, ATLAS_W, 101), (2052, 104));
        // After rounding it must be a multiple of 4 (a hard requirement of GPU block copies)
        for h in [1u32, 3, 4, 7, 99, 2048] {
            let (tw, th) = atlas_tex_dims(true, ATLAS_W, h);
            assert_eq!((tw % 4, th % 4), (0, 0), "BC7 纹理尺寸必须 4 对齐");
        }
    }

    /// Key invariant: after BC7 rounding, the UV at the bottom-right corner of the content sub-region is still < 1.0 (it points precisely into the real content,
    /// never bleeding into padding rows/columns) — this is the core of "denominator uses the texture's real size" preventing overall sampling drift.
    #[test]
    fn bc7_content_uv_stays_inside_real_content() {
        let (logical_w, logical_h) = (ATLAS_W, 101u32);
        let (tex_w, tex_h) = atlas_tex_dims(true, logical_w, logical_h);
        // Right/bottom boundary UV of the bottom-right content pixel (same formula as the uv closure)
        let u1 = logical_w as f32 / tex_w as f32;
        let v1 = logical_h as f32 / tex_h as f32;
        assert!(u1 < 1.0 && v1 < 1.0, "内容边界 UV 必须 < 1（padding 在外侧不被采样）");
        // Reverse to pixels: UV × texture size falls back onto the logical content size (no drift)
        assert_eq!((u1 * tex_w as f32).round() as u32, logical_w);
        assert_eq!((v1 * tex_h as f32).round() as u32, logical_h);
    }

    /// GPU-free fake atlas: white dot + test images (hero has a normal pair, gem doesn't) + all-empty glyphs.
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
        // Default camera scale=8: a 2x2 sprite → 16x16 pixels at screen center (24..40)
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 24.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 40.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 24.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 40.0);
        // Solid: white-dot UV + red tint
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
        // World y=+2, scale 8 → screen y moves up 16 pixels (y up → smaller pixel row)
        let y_min = verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert_eq!(y_min, 24.0 - 16.0);
    }

    #[test]
    fn ui_panel_quad_is_screen_space_unlit_matching_layout() {
        // GPU mirror: UI Panel vertices land on the screen-pixel rectangle solved by solve_layout (no camera involved),
        // and use the UNLIT sentinel (the UI overlay isn't lit, aligning with the CPU path's "drawn after lighting").
        let mut w = World::new();
        let root = w.spawn_named("ui").unwrap();
        w.set_component(root, "UiRoot", json!({})).unwrap();
        let panel = w.spawn_named("p").unwrap();
        w.set_component(
            panel,
            "Ui",
            json!({"anchor": "center", "w": 40.0, "h": 20.0, "parent": "ui",
                   "rx": 0.0, "ry": 0.0, "rw": 0.0, "rh": 0.0}),
        )
        .unwrap();
        w.set_component(panel, "Panel", json!({"color": "#ff0000", "image": ""})).unwrap();

        // Solve on a 200x100 viewport: centered 40x20 → x∈[80,120], y∈[40,60]
        let layout = vitric_render::solve_layout(&w, 200, 100).unwrap();
        let mut verts = Vec::new();
        push_ui_panels(&mut verts, &w, &layout, &fake_atlas()).unwrap();
        assert_eq!(verts.len(), 6, "一个 Panel = 两个三角形");
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 80.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 120.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 40.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 60.0);
        // UNLIT sentinel: nuv = [-2,-2,-2,-2] (the fragment skips lighting based on this)
        assert_eq!(verts[0].nuv, [-2.0, -2.0], "UI 用 UNLIT 哨兵不被打光");
        assert_eq!(verts[0].color, [1.0, 0.0, 0.0, 1.0], "纯色 Panel 染红");
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
        // Sprite 6 + two chars 12, and the text vertices come after the sprite (painter's algorithm: later draws on top)
        assert_eq!(verts.len(), 6 + 12);
        assert_eq!(verts[6].color, [0.0, 1.0, 0.0, 1.0]);
        // Two chars size=2, scale=8 → the whole string is 32px wide centered: x from 16 to 48
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
        // Unrotated corners (16,24)(48,24)(48,40)(16,40) rotate 90° counter-clockwise around center (32,32):
        // horizontal bar becomes vertical bar. World counter-clockwise = screen counter-clockwise — the original top-right corner goes to top-left
        assert_eq!(verts[0].pos, [24.0, 48.0], "左上角 → 左下");
        assert_eq!(verts[1].pos, [24.0, 16.0], "右上角 → 左上");
        assert_eq!(verts[2].pos, [40.0, 16.0], "右下角 → 右上");
        assert_eq!(verts[3].pos, [24.0, 48.0], "第二个三角形从角 0 重新起");
        assert_eq!(verts[4].pos, [40.0, 16.0]);
        assert_eq!(verts[5].pos, [40.0, 48.0], "左下角 → 右下");
        assert_eq!(verts[0].color, [1.0, 0.0, 0.0, 1.0], "染色不受旋转影响");
        // Explicit rot=0 gives the same geometry as the missing field (fast path)
        w.set_field(e, "Sprite.rot", json!(0.0)).unwrap();
        let v0 = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(v0[0].pos, [16.0, 24.0]);
        assert_eq!(v0[2].pos, [48.0, 40.0]);
    }

    #[test]
    fn rotated_texture_uv_follows_corners() {
        // The texture rotates with the corners: UV corner order is unchanged (the top-left corner's UV is always the atlas region's origin),
        // position changes but UV doesn't = the texture content follows the sprite's rotation
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
        // hero.png is paired with hero_n.png in the atlas: vertices carry the normal-region UV + rot's (cos, sin)
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "image": "hero.png"})).unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert_eq!(verts[0].nuv, [0.5, 0.5], "角 0 = 法线区域左上");
        assert_eq!(verts[2].nuv, [0.75, 0.75], "角 2 = 法线区域右下");
        assert_eq!(verts[0].rotcs, [1.0, 0.0], "rot=0 → 恒等旋转");
        // rot=90: rotcs = (cos, sin), nuv corner order unchanged (the normal map follows the corners)
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "image": "hero.png", "rot": 90.0}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), None, 0).unwrap();
        assert!(verts[0].rotcs[0].abs() < 1e-6 && (verts[0].rotcs[1] - 1.0).abs() < 1e-6);
        assert_eq!(verts[0].nuv, [0.5, 0.5]);
    }

    #[test]
    fn unpaired_quads_carry_normal_sentinel() {
        // Unpaired textures / solid blocks / text: nuv is all sentinel (x<0), the fragment takes the original lighting formula
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
        // 4x2 rotated 90° → bounding box ~2x4: the outline geometry takes the rotated bounding box (same choice as the CPU path)
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "color": "#ff0000", "rot": 90.0}))
            .unwrap();
        let verts = build_vertices(&w, 64, 64, &fake_atlas(), Some(e), 0).unwrap();
        assert_eq!(verts.len(), 6 + 24);
        // After rotation the half-height is 2 units = 16px + 2px outset → top edge y=14 (this axis has no floating-point boundary issue, can assert exactly)
        let y_min = verts[6..].iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert_eq!(y_min, 14.0, "描边上缘随旋转后包围盒抬高");
        // The horizontal axis only asserts a range: the floating-point tail of cos(90°) makes floor waver between 21/22, don't pin a specific value
        let x_min = verts[6..].iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!((21.0..=22.0).contains(&x_min), "描边左缘应收窄到竖条附近: {x_min}");
    }

    #[test]
    fn wgsl_parses_and_validates_offline() {
        // Lock the shader in GPU-less environments: parsing + validation pass, uniform-layout errors / syntax errors blow up in CI
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
        // [0] downsample + threshold: radius 0, sample scale 2, threshold unchanged
        assert_eq!(p[0].params, [0.6, 0.0, 0.0, 0.0]);
        assert_eq!(p[0].params2[0], 2.0);
        // Blur pass: threshold off (-1), radius = half of the CPU radius (720/90=8) = 4, scale 1, direction alternates H/V
        for (i, pp) in p[1..].iter().enumerate() {
            assert_eq!(pp.params[0], -1.0, "pass {i} 不再做阈值");
            assert_eq!(pp.params[1], 4.0, "半分辨率半径减半");
            assert_eq!(pp.params2[0], 1.0);
            let expect_dir = if i % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
            assert_eq!([pp.params[2], pp.params[3]], expect_dir, "pass {i} 方向交替");
        }
        // Small viewport: the CPU radius hits the floor of 2 → GPU half-resolution radius 1 (the floor)
        assert_eq!(bloom_pass_params(64, 0.5)[1].params[1], 1.0);
    }

    #[test]
    fn bloom_half_dims_and_scene_pass_srgb_selection() {
        assert_eq!(half_dims(1280, 720), (640, 360));
        assert_eq!(half_dims(1, 1), (1, 1), "极小窗口不归零");
        // When bloom is on the scene pass never does sRGB inverse-conversion (the inverse-conversion moves to the composite pass)
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
        // ambient.rgb = #202838 / 255, .w = light count
        assert_eq!(g.ambient[3], 1.0);
        assert!((g.ambient[0] - 0x20 as f32 / 255.0).abs() < 1e-6);
        assert!((g.ambient[2] - 0x38 as f32 / 255.0).abs() < 1e-6);
        // Default camera scale=8: world (2,1) → pixels (32+16, 32-8), radius 4*8=32px
        assert_eq!(g.lights_pos[0], [48.0, 24.0, 32.0, 0.0]);
        // Color is already multiplied by intensity=2
        assert_eq!(g.lights_color[0][0], 2.0);
        assert!((g.lights_color[0][1] - 2.0 * 0x80 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(g.lights_color[0][2], 0.0);
        // The sRGB flag is independent of lighting
        assert_eq!(build_globals(&w, 64, 64, true, 0).unwrap().viewport[2], 1.0);
        // Point light (kind not written): kind slot = 0, the entire direction array is 0
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
        let sun = w.spawn(); // Directional lights don't need Position
        w.set_component(sun, "Light", json!({"kind": "directional", "dir": 180.0, "intensity": 0.5}))
            .unwrap();
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.ambient[3], 2.0, "平行光也计入灯数");
        // Spot light: position/radius same as point light (scale=8 → pixels (48,24), radius 32px), kind slot = 1
        assert_eq!(g.lights_pos[0], [48.0, 24.0, 32.0, 1.0]);
        assert_eq!(g.lights_color[0], [2.0, 2.0, 2.0, 0.0]);
        // Direction dir=90° (world +y) → pixel space (cos90, -sin90) = (0,-1); half-cone angle 30° in radians
        assert!(g.lights_dir[0][0].abs() < 1e-6, "{:?}", g.lights_dir[0]);
        assert_eq!(g.lights_dir[0][1], -1.0);
        assert!((g.lights_dir[0][2] - (30f32).to_radians()).abs() < 1e-6);
        // Directional light: position/radius are placeholder 0, kind slot = 2, color already multiplied by intensity, direction also packed
        // (the fragment of a normal-bearing pixel uses it to compute max(dot(N,L),0); sentinel pixels don't read it)
        assert_eq!(g.lights_pos[1], [0.0, 0.0, 0.0, 2.0]);
        assert_eq!(g.lights_color[1], [0.5, 0.5, 0.5, 0.0]);
        assert_eq!(g.lights_dir[1][0], -1.0, "dir=180° → 像素空间 (-1, 0)");
        assert!(g.lights_dir[1][1].abs() < 1e-6);
        assert_eq!(g.lights_dir[1][2], 0.0, "半锥角只属于 spot");
    }

    /// Place a cw×ch occluder wall at (x,y) (Solid+Position+Collider).
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
        // Light 0's occluder range: start 0, count 1
        assert_eq!(g.shadow_ranges[0], [0.0, 1.0, 0.0, 0.0], "[起点, 条数]");
        // Default camera scale=8: center (2,1) → pixels (48,24), half-width 4px / half-height 8px
        assert_eq!(g.occluders[0], [44.0, 16.0, 52.0, 32.0], "[x0, y0, x1, y1] 像素空间");
        assert_eq!(g.occluders[1], [0.0; 4], "没占用的槽位保持零");
        // Single box forms a group: sub-box range [0,1], sub-box = the same AABB
        assert_eq!(g.occluder_sub_ranges[0], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(g.occluder_subs[0], [44.0, 16.0, 52.0, 32.0]);
        // shadows off (field defaulted): the wall is still there, but the ranges are all zero → the shader loop runs zero times
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.shadow_ranges[0], [0.0; 4]);
        assert_eq!(g.occluders[0], [0.0; 4]);
    }

    #[test]
    fn globals_merge_flush_tiles_and_cull_per_light() {
        // Three flush tiles merge into one strip (1 big box, 3 sub-boxes); the lone box outside the light disc is culled
        let (mut w, _) = world_shadow_lamp(3.0);
        for i in 0..3 {
            add_wall(&mut w, i as f64, 1.0, 1.0, 1.0);
        }
        add_wall(&mut w, 40.0, 0.0, 1.0, 1.0); // far outside the light disc (3*8=24px)
        let g = build_globals(&w, 64, 64, false, 0).unwrap();
        assert_eq!(g.shadow_ranges[0], [0.0, 1.0, 0.0, 0.0], "合并后只剩 1 条、孤箱被剔除");
        // Row: world x ∈ [-0.5, 2.5], y ∈ [0.5, 1.5] → pixels [28, 20, 52, 28]
        assert_eq!(g.occluders[0], [28.0, 20.0, 52.0, 28.0]);
        assert_eq!(g.occluder_sub_ranges[0], [0.0, 3.0, 0.0, 0.0], "3 个子箱");
        assert_eq!(g.occluder_subs[0], [28.0, 20.0, 36.0, 28.0], "子箱按原始瓦片");
        assert_eq!(g.occluder_subs[2], [44.0, 20.0, 52.0, 28.0]);

        // Each of the two lights culls its own: the far light only sees the box next to it, the ranges are laid out front-to-back in the flat array
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
        // Per-light cap: 65 non-flush (gapped) boxes all fall inside one large light's disc
        let (mut w, _) = world_shadow_lamp(80.0);
        for i in 0..(SHADOW_PER_LIGHT + 1) {
            add_wall(&mut w, i as f64 * 2.0 - 64.0, 0.0, 1.0, 1.0);
        }
        let err = build_globals(&w, 64, 64, false, 0).err().expect("超单灯上限必须报错");
        assert!(err.contains("65") && err.contains("64") && err.contains("提示"), "{err}");

        // Total budget: 5 lights × 60 boxes = 300 > 256
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
        // When lighting is on the first quad is the fullscreen background (the clear color can't be lit by the shader)
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
        // No Ambient: no background quad is laid down (legacy behavior)
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
        // 16x16 small atlas: white block (2x2 + 1px gap) at (0,0), row_h=3
        let mut a = GlyphAtlas::new(16);
        assert_eq!(a.pending.len(), 1, "白块在待上传队列里");
        assert_eq!(a.alloc_rect(8, 8).unwrap(), (3, 0), "同行白块右侧");
        // Can't fit a second 8x8 (x=12+9>16) → wrap to y=9, but 9+9>16 → explicit atlas-full error
        let err = a.alloc_rect(8, 8).unwrap_err();
        assert!(err.contains("已满") && err.contains("字号"), "{err}");
        // Single block exceeds the edge: another explicit error (not wrapped up as "full")
        let err = a.alloc_rect(20, 4).unwrap_err();
        assert!(err.contains("超过"), "{err}");
        // A wrap really did happen: the next smaller block lands on the second row
        assert_eq!(a.alloc_rect(2, 2).unwrap(), (0, 9));
    }

    #[test]
    fn glyph_uv_is_uploaded_once_then_cached() {
        let font = test_font();
        let mut a = GlyphAtlas::new(1024);
        let base = a.pending.len(); // white block
        let uv1 = a.glyph_uv(&font, 'A', 24).unwrap();
        assert_eq!(a.pending.len(), base + 1, "首次出现 → 一次上传");
        let up = a.pending.last().unwrap();
        assert_eq!(up.pixels.len(), (up.w * up.h * 4) as usize, "RGBA 像素量对得上");
        assert!(up.pixels.chunks_exact(4).all(|p| p[..3] == [255, 255, 255]), "白底+覆盖率 alpha");
        // Same key again: cache hit, no further upload
        let uv2 = a.glyph_uv(&font, 'A', 24).unwrap();
        assert_eq!(uv1, uv2);
        assert_eq!(a.pending.len(), base + 1, "缓存命中不产生新上传");
        // Same character with a different size is a different glyph
        let uv3 = a.glyph_uv(&font, 'A', 32).unwrap();
        assert_ne!(uv1, uv3);
        assert_eq!(a.pending.len(), base + 2);
        // CJK: glyphs missing from DejaVu fall back to the .notdef tofu block — they still need a visible placeholder, not silently dropped
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
        // Same layout as the CPU path: total width comes from layout(px=16), the whole string is centered horizontally on screen center 32
        let (_, total_w) = font.layout("Hi", 16);
        let left = 32.0 - total_w / 2.0;
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let x_min = xs.iter().cloned().fold(f32::MAX, f32::min);
        let x_max = xs.iter().cloned().fold(f32::MIN, f32::max);
        assert!(x_min >= left - 1.5 && x_max <= left + total_w + 1.5, "字形落在排版包络内: {x_min}..{x_max} vs {left}+{total_w}");
        // A space only takes advance, it doesn't emit a quad
        w.set_field(t, "Text.content", json!(" ")).unwrap();
        let mut sp = Vec::new();
        push_ttf_texts(&mut sp, &w, 64, 64, &font, &mut a, (0.0, 0.0, 8.0)).unwrap();
        assert!(sp.is_empty());
    }

    #[test]
    fn particle_quads_match_cpu_dots_and_are_unlit() {
        // GPU particle quads must match the CPU source of truth (emitter_particles) on position/count/color
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
            // Same transform as the CPU: center = screen center + world offset · scale (default camera scale=8)
            let cx = 32.0 + p.x * 8.0;
            let cy = 32.0 - p.y * 8.0;
            let half = p.size * 8.0 / 2.0;
            let x_min = quad.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
            let y_min = quad.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
            assert!((x_min as f64 - (cx - half)).abs() < 1e-4, "粒子 {i} 位置对齐 CPU");
            assert!((y_min as f64 - (cy - half)).abs() < 1e-4);
            assert_eq!(quad[0].color, tint(p.rgba), "颜色（含淡出 alpha）一致");
            // Self-emissive sentinel: nuv < -1.5, the fragment skips lighting
            assert!(quad.iter().all(|v| v.nuv[0] < -1.5 && v.nuv[1] < -1.5), "UNLIT 哨兵");
        }
        // An untriggered burst (burst < 0) emits zero vertices
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
        // The particle time-conversion constant must equal the simulation frequency (render doesn't depend on sim; it's locked here across crates)
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
        // Sprite 6 + outline four edges 24
        assert_eq!(verts.len(), 6 + 24);
        let teal = [39.0 / 255.0, 192.0 / 255.0, 168.0 / 255.0, 1.0];
        assert_eq!(verts[6].color, teal);
        // Half-width 8px + 2px outset → outline left edge at 32-10=22 (same geometry as the CPU path)
        let x_min = verts[6..].iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert_eq!(x_min, 22.0);
        // Selected entity is gone → skip silently, no error
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
