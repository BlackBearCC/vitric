//! vitric-render — 2D rasterization.
//!
//! v0 is a pure-CPU renderer: world → RGBA pixels → PNG.
//! It looks conservative, but it is a key link in the closed loop: **screenshots need no GPU,
//! no window, no graphics session** — an agent can "see with its own eyes" the game frame in
//! any headless environment, and the pixels rendered from the same world state are byte-for-byte
//! identical (screenshots can go into assertions too).
//! The GPU (wgpu) path follows the same component conventions; swapping the presentation layer
//! later does not touch game data.
//!
//! Component conventions:
//! - `Sprite`  {"w": number, "h": number, "color": "#rrggbb", "rot": degrees} — drawn only if present.
//!   rot is optional: rotates around Position (sprite center), **world-space counter-clockwise is
//!   positive** — after the screen-y flip it still looks counter-clockwise on screen. Default/0
//!   takes the original fast path; the output bytes are bit-identical to when the field is absent
//!   (backward compatibility is locked by tests). Only the sprite rotates: Text stays upright;
//!   describe's overlaps still uses the AABB of the unrotated size (approximation, see the comment
//!   inside [`describe_world`])
//! - `Position` {"x", "y"} — world coordinates, y up
//! - `Camera` {"x", "y", "scale"} — optional; the first one is taken, otherwise origin, 8 px/unit
//! - `Shake` {"amplitude", "decay"} — screen shake attached to the camera entity; when amplitude > 0
//!   the framing adds a deterministic pseudo-random offset (a pure function of (tick, amplitude),
//!   see [`shake_offset`]). The offset only affects the picture (render_world / GPU path /
//!   selection outline) — describe / pick / screen_to_world read the un-shaken camera: semantic
//!   observation and picking address the world itself, not the shaken picture
//! - `Text` {"content", "size", "color"} — on-screen text, drawn above sprites, the whole string
//!   horizontally centered on Position. Two paths, switched by the manifest `font` field:
//!   * Default (no font): built-in 8x8 bitmap (ASCII), each character is size×size world units,
//!     monospace, no anti-aliasing — old behavior, output bytes locked unchanged by tests;
//!   * Manifest sets `font` (TTF): **all** Text goes through the vector font ([`font::FontStore`]),
//!     size = total glyph height (ascent-descent) in world units, proportional spacing + kerning,
//!     and every glyph present in the font can be drawn (including CJK). Vector text is the
//!     **only deliberately smoothed** element in the engine: coverage anti-aliasing (pixel-art
//!     stays nearest-neighbor throughout). Vertically the glyph body is centered on Position.
//!     Same platform + same binary is still byte-deterministic (ab_glyph is pure-Rust rasterization)
//! - `Ambient` {"color": "#rrggbb", "shadows": bool} — scene ambient light, attached to any entity
//!   (the first one is taken). **The master switch for lighting**: no Ambient entity in the scene =
//!   no lighting at all (old behavior, zero overhead); otherwise the whole frame (sprites/text/
//!   background, all the same) is lit by the formula below.
//!   `shadows` is optional (default false): **the switch for 2D projection** — false/default makes
//!   the output bytes bit-identical to before this feature existed (backward compatibility locked
//!   by tests); true → see the "Projection" section below
//! - `Light` {"radius", "color", "intensity", "kind", "angle", "dir"} — light source, three kinds
//!   (default "point", unknown values explicitly error):
//!   * `"point"` (point light, requires `Position`): radius in world units (attenuates to zero at
//!     radius), color defaults to "#ffffff", intensity defaults to 1.0. **Omitting the kind field =
//!     point light = old behavior, output bytes bit-identical (backward compatibility locked by tests)**
//!   * `"spot"` (spotlight, requires `Position`): all the point-light fields, plus the required
//!     `angle` (cone full-width, degrees, 1..=360) and the required `dir` (facing, degrees, world
//!     space, 0 = +x, counter-clockwise positive — same angle convention as Sprite.rot)
//!   * `"directional"` (directional light): required `dir` (the direction the light **travels**,
//!     degrees, same convention as above) + color/intensity. Does not read Position/radius — the
//!     sun is at infinity, equally bright everywhere. The contribution at a pixel without a normal
//!     is uniform everywhere = color·intensity (byte-locked old behavior); pixels with normals
//!     compute the direction from dir (see the "Normal map" section below). Combined upper limit
//!     across the three kinds is 64 lights
//! - `Bloom` {"threshold", "strength"} — full-screen bloom post-effect, attached to any entity
//!   (the first one, same as Ambient). **The master switch for bloom**: no Bloom entity in the
//!   scene = no bloom at all (old bytes unchanged, zero overhead). threshold ∈ [0,1]: only the part
//!   of a channel value exceeding threshold·255 enters bloom; strength ≥ 0: additive scale.
//!   Both fields are required (missing / non-numeric explicitly errors, no silent default)
//! - `Emitter` — particle emitter (requires `Position`). **Particles are a pure render-layer
//!   product, they do not enter simulation state**: each particle's position/color/size at tick T
//!   is a pure function `f(emitter fields, particle index, T, seed derived from entity id)`
//!   ([`emitter_particles`]) — no integrator (analytic pos = origin + v0·t + ½g·t²), no cross-frame
//!   state: does not enter the state hash, does not enter saves; recording replay / snapshot
//!   rollback makes the particle picture automatically correct. Random numbers use SplitMix64
//!   deterministic hashing (seed = entity-id hash ⊕ particle index, see [`emitter_seed`]), fully
//!   independent of the simulation RNG stream. Fields (semantic source [`collect_emitters`],
//!   missing/wrong explicitly errors):
//!   * `kind`: required, `"stream"` (continuous stream, emits at `rate` particles/second, the
//!     emission timeline starts from tick 0) or `"burst"` (single burst: the `burst` field = the
//!     trigger tick number; writing the current tick into the rule triggers it, `count` particles
//!     are born simultaneously; burst < 0 = not triggered. Whether it is in the burst period is
//!     derived purely from the field value, no history needed)
//!   * `lifetime`: required, particle lifetime (ticks, integer ≥ 1); `size`: required, starting
//!     size (world units > 0)
//!   * `rate` (required for stream, > 0) / `count` + `burst` (for burst, count ≥ 1, burst default -1)
//!   * `speed_min`/`speed_max`: initial velocity range (world units/second, default 0; speed_max
//!     default = speed_min)
//!   * `dir`: emission direction (degrees, 0 = +x, counter-clockwise positive — same convention as
//!     Sprite.rot, default 0); `spread`: spread full-width (degrees 0..=360, default 360 = omnidirectional,
//!     in which case dir does not matter)
//!   * `gravity`: gravitational acceleration (world units/second², y axis, usually negative; default 0)
//!   * `color`/`color_end`: start/end color ("#rrggbb"; color_end default/empty = no gradient);
//!     alpha fades linearly with lifetime (built-in, 255 → 0)
//!   * `size_end`: end size (≥ 0, default = no size gradient; 0 = shrink to nothing)
//!   * `active`: switch (bool, default true). false = no particles are drawn at all — **a pure-function
//!     trade-off**: turning it off mid-flight makes in-flight particles disappear on that frame
//!     (the picture only reads the current field values, no emission history)
//!   * Rendering: particles are drawn as square dots (matching the GPU path's quad vertex geometry),
//!     **self-emissive**: drawn after lighting and before bloom — not darkened by ambient light,
//!     not attenuated by lights, casts no shadow and is not shadowed (simplifying convention);
//!     bright particles still go into bloom. If the emitter entity moves, all in-flight particles
//!     move with it (positions are relative to the current origin — the cost of being stateless).
//!     No Emitter in the scene = nothing drawn (old bytes unchanged, zero overhead). Upper limits:
//!     [`MAX_EMITTERS`] emitters and [`MAX_PARTICLES_PER_EMITTER`] particles on screen per emitter;
//!     exceeding them explicitly errors
//!
//! Lighting formula (CPU and GPU paths must match; the GPU side is in vitric-cli gpu.rs's WGSL):
//!   lit = min(ambient + Σ each light's contribution, 1.5)
//!   out = min(scene · lit, 1.0)
//! Each light's contribution:
//!   point:       color·intensity·(1 - d/r)²                       (contributes only when d < r)
//!   spot:        color·intensity·(1 - d/r)²·t²,
//!                t = clamp(1 - Δθ/(angle/2), 0, 1)                 (angular falloff: cone center 1, edge 0)
//!   directional: color·intensity                                    (uniform everywhere)
//! d is the pixel-to-light distance (pixel space, framing uses the shaken camera — light follows
//! the picture); Δθ is the angle between "the direction from the light to the pixel" and dir
//! (degree semantics, implemented with radian acos). The angular falloff deliberately uses t²
//! (not the smoothstep built-in) — the CPU and GPU sides must mirror the same formula.
//! The 1.5 cap allows slight overexposure (a cheap "bloom feel"), clamped back to 1 after multiplying
//! the scene color.
//!
//! Projection (enabled when `Ambient.shadows: true`; CPU and GPU share the same geometry):
//! - Occluders = entities with `Solid` + `Position` + `Collider` — Solid in physics is exactly
//!   "blocking" (blocks the body); once projection is on, the same set of entities also blocks light,
//!   **zero new authorization concept**. Upper limit [`MAX_OCCLUDERS`] (256); exceeding it explicitly
//!   errors (no silent truncation).
//! - Per pixel per light: if the segment pixel→light-center intersects any occluder AABB (slab
//!   method, see [`segment_hits_aabb`]) that light's contribution is zeroed (hard shadow, no penumbra).
//!   **Exception: the occluder the pixel itself is inside does not block it** — otherwise every Solid
//!   paints itself black; the rule is "a pixel inside a box is only blocked by **other** boxes".
//! - Only point/spot cast shadows; directional does not in v1 (sun shadows need direction-based rays
//!   rather than point-to-point segments, left for a later version), and the directional contribution
//!   stays uniform everywhere.
//! - Known constraint: when the light center is buried inside some occluder, all out-of-box pixels
//!   are blocked by that box — do not place lights inside walls. The segment geometry uses the
//!   post-framing pixel space (same as the light params, light follows the picture).
//! - Performance (output bytes unchanged, locked by equivalence tests): adjacent occluders whose
//!   edges are bit-aligned merge into big boxes every frame (a tile floor collapses into one long
//!   strip, see [`build_shadow_boxes`]); then per light the boxes that cannot be reached are culled
//!   ([`cull_shadow_boxes`]); point/spot lights only scan their own light-disc bounding box, and
//!   pixels with zero contribution (outside the cone / on the back face) skip the occlusion test.
//!   The GPU path shares the same merge/cull results, with an additional uniform budget: ≤ 64 boxes
//!   per light after culling, ≤ 256 total across all lights (exceeding it explicitly errors, see gpu.rs).
//!
//! Normal map (zero-config name pairing, see [`normal_map_name`]):
//! - If the sprite texture `hero.png` has a `hero_n.png` next to it in assets/, it is auto-enabled —
//!   RGB encodes a tangent-space normal `n = rgb/255·2-1`, z is taken absolute (forced outward) then
//!   normalized; a zero vector degrades to the flat normal (0,0,1). The normal's xy axes align with
//!   **screen pixel space** (x right, y down — when the image is blitted 1:1 the image axes ARE the
//!   screen axes), and when `Sprite.rot` rotates the sprite the normal's xy rotates by the same matrix.
//! - Pixels with normals get each light's contribution multiplied by an additional `max(dot(N, L), 0)`.
//!   L is the pixel-to-light direction lifted to 3D: xy is the pixel→light-center unit direction
//!   scaled by [`NORMAL_LIGHT_XY`] (0.8), z is fixed at [`NORMAL_LIGHT_Z`] (0.6; 0.8²+0.6²=1, naturally
//!   unit length) — so a flat normal (0,0,1) directly under a light still gets a 0.6 contribution,
//!   it does not "go black just because normals are on". A pixel exactly at the light center (d=0)
//!   has undefined direction, by convention L=(0,0,1). Directional light is isomorphic:
//!   L = (−unit travel direction·0.8, 0.6) — dir now participates in the computation, giving the
//!   directional light a sense of direction.
//! - **Pixels without normals take the original formula, output bytes bit-identical** (backward
//!   compatibility locked by tests). Implementation: when lighting is on, sprite blit writes the
//!   normal into a per-frame normal buffer (sentinel zero vector = no normal; later sprites/text
//!   overwrite/clear the normal when they overwrite the pixel — the covered pixel belongs to the
//!   upper image). The GPU side uses the same formula (the normal map lives in the same atlas as
//!   regular images, vertices carry a second UV set, see gpu.rs).
//!
//! Bloom formula (CPU is the source of truth — screenshots/assertions go by this path; the GPU side
//! aims for visual parity, differences are in gpu.rs):
//!   bright = max(scene - threshold·255, 0)       (lift the bright part per channel)
//!   blurred = box blur(bright), horizontal + vertical separable, 3 iterations (approximate Gaussian)
//!   out = min(scene + blurred · strength, 255)    (additive composite)
//! The blur radius = [`bloom_radius_px`]: viewport height / 90, lower bound 2 px — the radius scales
//! with resolution so the same scene at 4K and 720p has the same bloom-to-frame ratio. Bloom runs
//! **after** lighting (light first, then glow).

mod assets;
mod font;
mod ui;
mod ui_interact;

pub use assets::{is_normal_map_name, normal_map_name, Assets, Image};
pub use font::{revealed_chars, FontStore, GlyphPlacement, RasterGlyph};
pub use ui::{
    has_ui, layout_input_hash, layout_runs, point_over_ui, solve_layout, Align, Anchor,
    ContainerKind, Layout, UiRect, ALIGN_NAMES, ANCHOR_NAMES, CONTAINER_KINDS,
};
pub use ui_interact::{
    modulate_rgb, navigate, press_modulate, press_scale, ui_press_feedback, ButtonState, Dir,
    Focusable, BUTTON_STATES, PRESS_TICKS,
};

use serde_json::Value;

use vitric_ecs::{ascii_map, relate_in_world, AsciiMapOpts, Placement, World};

/// Upper limit on the number of point lights. Both the per-pixel (CPU) and per-fragment (GPU
/// uniform array) paths iterate over every light; without a cap both paths would be dragged down.
/// Exceeding it explicitly errors, no silent truncation.
pub const MAX_LIGHTS: usize = 64;

/// Lighting brightness cap: ambient + the sum of each light's contribution is clamped per channel
/// here (see the formula in the module docs).
pub const LIGHT_CLAMP: f64 = 1.5;

/// Upper limit on the number of occluders (when projection is on). Per pixel per light every
/// occluder is scanned — both the CPU inner loop and the GPU uniform array (256 × vec4 = 4KB) are
/// bound by it; exceeding it explicitly errors, no silent truncation.
pub const MAX_OCCLUDERS: usize = 256;

/// z lift of the light direction for normal lighting (fixed value, see module docs): L.z = 0.6, xy
/// takes 0.8 — unit length is guaranteed by construction. 0.6 is an aesthetic choice: a flat pixel
/// directly under a light still gets 60% contribution, a trade-off between relief feel and "don't
/// crush the picture to black". The CPU and GPU sides must use the same value (gpu.rs WGSL hard-
/// codes it and notes the source).
pub const NORMAL_LIGHT_Z: f64 = 0.6;

/// xy coefficient of the light direction for normal lighting: √(1 − 0.6²) = 0.8 (paired with
/// [`NORMAL_LIGHT_Z`] to form a unit vector).
pub const NORMAL_LIGHT_XY: f64 = 0.8;

/// Upper limit on the number of particle emitters. Each emitter expands its particles every frame;
/// both CPU rasterization and the GPU vertex stream are bound by it. Exceeding it explicitly errors,
/// no silent truncation.
pub const MAX_EMITTERS: usize = 64;

/// On-screen particle budget for a single emitter (stream is estimated by rate·lifetime, burst by
/// count). Validated in [`collect_emitters`] — exceeding it explicitly errors (lower rate/count or
/// shorten lifetime), no silent particle loss.
pub const MAX_PARTICLES_PER_EMITTER: usize = 1024;

/// Simulation frequency (ticks/second) used for particle time conversion. **Must equal
/// vitric-sim's `TICKS_PER_SECOND`** (render does not depend on sim; each crate keeps its own copy,
/// consistency is locked by vitric-cli's cross-crate tests). rate (particles/second) and initial
/// velocity / gravity (world units/second) are both converted to ticks via this.
pub const PARTICLE_TICKS_PER_SECOND: f64 = 60.0;

/// Clear-screen background color: a dark gray-blue, distinct from pure black (pure black is often
/// misread as "nothing was rendered"). The GPU path's clear / background quad also uses it — the
/// two paths share the same background bytes.
pub const BACKGROUND: [u8; 4] = [24, 26, 33, 255];

/// Lower bound on text-readability contrast (a WCAG-style ratio `(L1+0.05)/(L2+0.05)`, L is relative
/// luminance). Below it describe emits a `low-contrast-text` warning. WCAG AA requires 4.5 for body
/// text and 3.0 for large text; 2.5 here is a deliberate relaxation — this is the "basically
/// unreadable to the human eye" red line for AI developers, not an accessibility compliance check
/// (false positives would teach the agent to ignore warnings, which is worse than missing some).
pub const TEXT_CONTRAST_MIN: f64 = 2.5;

/// Whether an entity should be considered for rendering / describing. Currently just the
/// negation of `world::is_dormant` — kept as a separate helper so the render layer's intent
/// ("don't draw dormant entities") is locally readable and survives future iteration refactors.
/// Cheap: one component lookup + one field read.
fn is_renderable(world: &World, id: vitric_ecs::EntityId) -> bool {
    !world.is_dormant(id)
}

/// Render one frame: returns RGBA8 pixels (row-major, top-left origin).
/// `tick` is only fed to screen shake ([`camera_of`]) — the bytes rendered from the same world at
/// the same tick are bit-identical.
pub fn render_world(
    world: &World,
    width: u32,
    height: u32,
    assets: &Assets,
    tick: u64,
) -> Result<Vec<u8>, String> {
    let cam = camera_of(world, tick, height)?;
    render_with(world, width, height, assets, cam, tick, &RenderOpts::default())
}

/// Switches for internal render variants (not exposed externally — the public API has only one
/// "normal render"). The only reason it exists is [`describe_world`]'s text-contrast measurement:
/// it needs "the background color under this Text **when it is not drawn**", and must not error
/// out the whole frame just because some asset is missing on hand.
#[derive(Default)]
struct RenderOpts {
    /// Skip drawing this one Text entity (to measure the background under it). `None` = draw all
    /// text normally.
    skip_text: Option<vitric_ecs::EntityId>,
    /// Lenient image mode: when `Sprite.image` is not in the asset store, degrade to a
    /// `Sprite.color` solid block (approximate brightness) instead of erroring. **Only for contrast
    /// measurement** — normal rendering (false) keeps the "missing image = direct error" convention;
    /// a missing image never silently draws a placeholder.
    lenient_images: bool,
}

/// Render main body (camera already decided). [`render_world`] reaches here with default opts —
/// the arithmetic on the normal render path is byte-identical to before the refactor (backward
/// compatibility locked by tests). `tick` is only fed to particle expansion ([`emitter_particles`],
/// particles are a pure function of tick).
fn render_with(
    world: &World,
    width: u32,
    height: u32,
    assets: &Assets,
    (cam_x, cam_y, scale): (f64, f64, f64),
    tick: u64,
    opts: &RenderOpts,
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(format!("分辨率 {width}x{height} 不合法（1..=4096）"));
    }
    let mut buf = vec![0u8; (width * height * 4) as usize];
    fill(&mut buf, BACKGROUND);

    // Normal buffer (per frame): allocated only when lighting is on AND the asset store actually
    // has normal maps — otherwise zero allocation, zero overhead (None is bit-identical to "has a
    // buffer but all sentinels", old bytes unchanged). Sentinel zero vector = this pixel has no
    // normal (takes the original lighting formula, bytes locked); sprite blit fills it in passing,
    // and anything drawn later overwrites/clears the normal when it overwrites the pixel (the
    // covered pixel belongs to the upper image).
    let ambient = ambient_of(world)?;
    let mut normals: Option<Vec<[f32; 3]>> = ambient
        .as_ref()
        .filter(|_| assets.has_normal_maps())
        .map(|_| vec![[0.0f32; 3]; (width * height) as usize]);

    // View-frustum culling bounds (world units): same scale the camera derived (see [`camera_of`])
    // and same pixel dimensions — describe_world uses the same boundary to classify visible vs
    // off-screen. Sprites whose rotated AABB is entirely outside this viewport are skipped by the
    // entity loop below (Task 4 / E4). This is a pure render-layer optimization: skipped entities
    // contribute zero pixels anyway (the pixel loop clamps to [0, width]/[0, height]), so the
    // output bytes for entities that ARE on screen are bit-identical to before this cull was added
    // (locked by `culling_preserves_byte_identical_output_for_onscreen_entities` and the existing
    // `frames.rs` / `glow.rs` screenshot tests).
    let view_w_world = width as f64 / scale;
    let view_h_world = height as f64 / scale;
    let view_x0 = cam_x - view_w_world / 2.0;
    let view_x1 = cam_x + view_w_world / 2.0;
    let view_y0 = cam_y - view_h_world / 2.0;
    let view_y1 = cam_y + view_h_world / 2.0;

    // Draw in entity order (deterministic; later draws cover earlier ones)
    for id in world.query(&["Position", "Sprite"]) {
        // Defensive dormant check: world.query already filters dormant entities, but keep the
        // explicit guard here too so the render invariant is locally visible and survives any
        // future iteration refactor.
        if !is_renderable(world, id) { continue; }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        let rot = rot_of(world, id)?;

        // View-frustum cull: skip entities whose rotated AABB is entirely outside the camera
        // viewport. For rot==0 the AABB is the sprite's own box; for rot!=0 the AABB is the
        // bounding box of the rotated shape (same extent the rotation path's pixel loop uses
        // below — so culling and rendering agree bit-exactly on what is "on screen"). No fixed
        // margin is added: the AABB already covers the rendered pixels exactly (and rotation
        // extents cover the rotated bounding box); shadow casters are collected separately
        // (see [`collect_occluders`]) and are not affected by this cull.
        let half_w_world = sw / 2.0;
        let half_h_world = sh / 2.0;
        let (ext_x, ext_y) = if rot == 0.0 {
            (half_w_world, half_h_world)
        } else {
            let (sn, cs) = rot.to_radians().sin_cos();
            (
                half_w_world * cs.abs() + half_h_world * sn.abs(),
                half_w_world * sn.abs() + half_h_world * cs.abs(),
            )
        };
        if px + ext_x < view_x0 || px - ext_x > view_x1
            || py + ext_y < view_y0 || py - ext_y > view_y1
        {
            continue;
        }

        // World → screen (y flipped, camera centered)
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;

        let mut image_name = world
            .get_field(id, "Sprite.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        // Lenient image mode (only on for the internal contrast-measurement render): if the image
        // is not in the asset store, draw it as a solid color block (Sprite.color, default white) —
        // when there are no real pixels to sample, a color-block brightness approximation is better
        // than erroring the whole frame. Normal rendering does not go through this line; a missing
        // image still explicitly errors.
        if opts.lenient_images && !image_name.is_empty() && assets.image(&image_name).is_none() {
            image_name = String::new();
        }

        if rot == 0.0 {
            // —— Fast path: no rotation (rot default/0). Do not touch this logic —
            //    the output bytes must be bit-identical to before the rot field existed
            //    (backward compatibility locked by tests)
            let x0 = (cx - half_w).floor().max(0.0) as i64;
            let x1 = (cx + half_w).ceil().min(width as f64) as i64;
            let y0 = (cy - half_h).floor().max(0.0) as i64;
            let y1 = (cy + half_h).ceil().min(height as f64) as i64;
            if image_name.is_empty() {
                // Solid color block
                let color = world
                    .get_field(id, "Sprite.color")
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "#ffffff".to_string());
                let rgba =
                    parse_color(&color).map_err(|e| format!("实体 {id} 的 Sprite.color: {e}"))?;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = ((y as u32 * width + x as u32) * 4) as usize;
                        buf[i..i + 4].copy_from_slice(&rgba);
                        // Solid color blocks have no normal: clear any normal that may have been
                        // left below (sentinel zero vector)
                        if let Some(ns) = normals.as_mut() {
                            ns[i / 4] = [0.0; 3];
                        }
                    }
                }
            } else {
                // Image: nearest-neighbor scaling + alpha blending. A missing image directly
                // errors (no placeholder drawn).
                let img = image_of(assets, id, &image_name)?;
                // Normal map by name pairing (hero.png → hero_n.png); no pairing = pixel clears
                // the normal
                let nmap = assets.normal_of(&image_name);
                let span_x = 2.0 * half_w;
                let span_y = 2.0 * half_h;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let u = ((x as f64 + 0.5) - (cx - half_w)) / span_x;
                        let v = ((y as f64 + 0.5) - (cy - half_h)) / span_y;
                        let sx = ((u * img.width as f64) as i64).clamp(0, img.width as i64 - 1) as usize;
                        let sy = ((v * img.height as f64) as i64).clamp(0, img.height as i64 - 1) as usize;
                        let s = (sy * img.width as usize + sx) * 4;
                        let src = &img.rgba[s..s + 4];
                        let a = src[3] as u32;
                        if a == 0 {
                            continue;
                        }
                        let i = ((y as u32 * width + x as u32) * 4) as usize;
                        let dst = &mut buf[i..i + 4];
                        for c in 0..3 {
                            dst[c] = ((src[c] as u32 * a + dst[c] as u32 * (255 - a)) / 255) as u8;
                        }
                        dst[3] = 255;
                        if let Some(ns) = normals.as_mut() {
                            // Sample the normal at the same (u,v) as the image — when not rotated
                            // sn=0/cs=1
                            ns[i / 4] = match nmap {
                                Some(m) => sample_normal(m, u, v, 0.0, 1.0),
                                None => [0.0; 3],
                            };
                        }
                    }
                }
            }
        } else {
            // —— Rotation path: scan the axis-aligned bounding box of the four rotated corners,
            // and per pixel inverse-rotate back into the sprite's local space then sample.
            // Angle convention see [`rot_of`]; f64 trig depends on the system math library — the
            // determinism boundary matches the docs: same platform + same binary is byte-for-byte
            // guaranteed, cross-platform last-bit is not guaranteed.
            // World counter-clockwise + screen y flip → screen-space forward matrix [[c, s], [-s, c]],
            // inverse takes the transpose
            let (sn, cs) = rot.to_radians().sin_cos();
            let ext_x = half_w * cs.abs() + half_h * sn.abs();
            let ext_y = half_w * sn.abs() + half_h * cs.abs();
            let x0 = (cx - ext_x).floor().max(0.0) as i64;
            let x1 = (cx + ext_x).ceil().min(width as f64) as i64;
            let y0 = (cy - ext_y).floor().max(0.0) as i64;
            let y1 = (cy + ext_y).ceil().min(height as f64) as i64;
            if image_name.is_empty() {
                // Solid color block (rotated): a pixel is colored only if its center falls inside
                // the sprite
                let color = world
                    .get_field(id, "Sprite.color")
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "#ffffff".to_string());
                let rgba =
                    parse_color(&color).map_err(|e| format!("实体 {id} 的 Sprite.color: {e}"))?;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let dx = (x as f64 + 0.5) - cx;
                        let dy = (y as f64 + 0.5) - cy;
                        let lx = cs * dx - sn * dy;
                        let ly = sn * dx + cs * dy;
                        if lx.abs() > half_w || ly.abs() > half_h {
                            continue; // Inside the bounding box but outside the sprite
                        }
                        let i = ((y as u32 * width + x as u32) * 4) as usize;
                        buf[i..i + 4].copy_from_slice(&rgba);
                        if let Some(ns) = normals.as_mut() {
                            ns[i / 4] = [0.0; 3];
                        }
                    }
                }
            } else {
                // Image (rotated): local coordinates are used directly as UV; the sampling logic
                // matches the fast path (nearest neighbor + alpha blending)
                let img = image_of(assets, id, &image_name)?;
                let nmap = assets.normal_of(&image_name);
                let span_x = 2.0 * half_w;
                let span_y = 2.0 * half_h;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let dx = (x as f64 + 0.5) - cx;
                        let dy = (y as f64 + 0.5) - cy;
                        let lx = cs * dx - sn * dy;
                        let ly = sn * dx + cs * dy;
                        if lx.abs() > half_w || ly.abs() > half_h {
                            continue;
                        }
                        let u = (lx + half_w) / span_x;
                        let v = (ly + half_h) / span_y;
                        let sx = ((u * img.width as f64) as i64).clamp(0, img.width as i64 - 1) as usize;
                        let sy = ((v * img.height as f64) as i64).clamp(0, img.height as i64 - 1) as usize;
                        let s = (sy * img.width as usize + sx) * 4;
                        let src = &img.rgba[s..s + 4];
                        let a = src[3] as u32;
                        if a == 0 {
                            continue;
                        }
                        let i = ((y as u32 * width + x as u32) * 4) as usize;
                        let dst = &mut buf[i..i + 4];
                        for c in 0..3 {
                            dst[c] = ((src[c] as u32 * a + dst[c] as u32 * (255 - a)) / 255) as u8;
                        }
                        dst[3] = 255;
                        if let Some(ns) = normals.as_mut() {
                            // Normal rotates with the sprite: pass in the sin/cos of the rotation
                            // matrix (local → screen)
                            ns[i / 4] = match nmap {
                                Some(m) => sample_normal(m, u, v, sn, cs),
                                None => [0.0; 3],
                            };
                        }
                    }
                }
            }
        }
    }

    draw_texts(
        world,
        &mut buf,
        width,
        height,
        (cam_x, cam_y, scale),
        assets,
        &mut normals,
        opts.skip_text,
    )?;

    // Lighting is toggled by the presence of an Ambient entity: none = skip entirely (old bytes
    // unchanged, zero overhead). Otherwise the whole frame is lit — sprites/text/background all
    // the same; if a HUD wants to stay readable it places a light next to itself
    if let Some((ambient, _)) = ambient {
        let lights = collect_lights(world)?;
        // Occluders are collected only when projection is on — when off (default) an empty list is
        // passed; the empty-list branch in the per-pixel loop changes no arithmetic, the output
        // bytes are bit-identical to before the projection feature existed (locked by tests)
        let occluders = if shadows_of(world)? { collect_occluders(world)? } else { Vec::new() };
        apply_lighting(
            &mut buf,
            width,
            height,
            (cam_x, cam_y, scale),
            ambient,
            &lights,
            &occluders,
            normals.as_deref(),
        );
    }

    // Particles are drawn after lighting and before bloom — self-emissive (not darkened by ambient
    // light / attenuated by lights / projected), bright particles still go into bloom. No Emitter
    // in the scene = zero-cost skip (old bytes unchanged)
    draw_particles(world, &mut buf, width, height, (cam_x, cam_y, scale), tick)?;

    // Bloom is toggled by the presence of a Bloom entity: none = skip entirely (old bytes
    // unchanged, zero overhead). Runs after lighting — the bright part is the lit bright part;
    // only what a light lit will bloom
    if let Some(bloom) = bloom_of(world)? {
        apply_bloom(&mut buf, width, height, &bloom);
    }

    // UI screen-space overlay: drawn right after the world render (lighting/particles/bloom
    // included), **not through the camera transform** — the UI does not drift when the camera
    // moves/scales/shakes (like a HUD). Screen-space orthographic projection = directly write
    // using the screen-pixel rect computed by layout, no off-screen buffer, reusing the same buf.
    // No UI in the scene (no UiRoot) = zero-cost skip (old bytes unchanged).
    draw_ui(world, &mut buf, width, height, assets)?;
    Ok(buf)
}

/// Scene ambient light: takes the first entity with an `Ambient` component, returns
/// (0..1 channel values, original color string). `None` = no Ambient in the scene = lighting is
/// entirely off (this is the agreed master switch, not a default white light).
pub fn ambient_of(world: &World) -> Result<Option<([f64; 3], String)>, String> {
    match world.query(&["Ambient"]).first() {
        None => Ok(None),
        Some(&id) => {
            let color = world
                .get_field(id, "Ambient.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .ok_or_else(|| {
                    format!(
                        "实体 {id} 挂了 Ambient 但没有 color 字段。\
                         写法: {{\"color\": \"#202838\"}}（暗色洞穴）；不想要光照就删掉 Ambient 组件"
                    )
                })?;
            let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Ambient.color: {e}"))?;
            Ok(Some((
                [rgba[0] as f64 / 255.0, rgba[1] as f64 / 255.0, rgba[2] as f64 / 255.0],
                color,
            )))
        }
    }
}

/// Projection switch: the optional `shadows` field (bool) on the first `Ambient` entity.
/// Default false = no projection = old bytes unchanged (backward compatibility locked by tests);
/// field present but not a bool → explicit error (silently treating a wrong type as false is harder
/// to debug than erroring). Always false when there is no Ambient in the scene (lighting entirely off).
pub fn shadows_of(world: &World) -> Result<bool, String> {
    match world.query(&["Ambient"]).first() {
        None => Ok(false),
        Some(&id) => match world.get_field(id, "Ambient.shadows") {
            Err(_) => Ok(false),
            Ok(v) => v.as_bool().ok_or_else(|| {
                format!(
                    "实体 {id} 的 Ambient.shadows 不是 bool: {v}。\
                     写法: {{\"color\": \"#202838\", \"shadows\": true}}；不想要投影就删掉该字段"
                )
            }),
        },
    }
}

/// One occluder (parsed result of a `Solid` + `Position` + `Collider` entity, world coordinates):
/// an AABB of center (x, y) + size (w, h) — the same collision box used by physics blocking,
/// "what blocks the body also blocks light", no second set of occlusion data introduced.
pub struct Occluder {
    pub id: vitric_ecs::EntityId,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Collect every occluder in the scene (entities with Solid+Position+Collider, in slot order).
/// Called only when projection is on ([`shadows_of`] is true); exceeding [`MAX_OCCLUDERS`]
/// explicitly errors. All field validation happens here (Collider.w/h, Position.x/y must be
/// numbers), so the hot path is left with pure arithmetic.
pub fn collect_occluders(world: &World) -> Result<Vec<Occluder>, String> {
    let ids = world.query(&["Solid", "Position", "Collider"]);
    if ids.len() > MAX_OCCLUDERS {
        return Err(format!(
            "场上有 {} 个遮光体（Solid+Position+Collider），超过投影上限 {MAX_OCCLUDERS} 个。\
             提示：合并相邻的 Solid（一面长墙一个实体），或关掉 Ambient.shadows",
            ids.len()
        ));
    }
    ids.into_iter()
        .map(|id| {
            Ok(Occluder {
                id,
                x: num(world, id, "Position.x")?,
                y: num(world, id, "Position.y")?,
                w: num(world, id, "Collider.w")?,
                h: num(world, id, "Collider.h")?,
            })
        })
        .collect()
}

/// Whether the segment (px,py)→(qx,qy) intersects the AABB [x0..x1, y0..y1] (slab method).
/// An axis-parallel component (delta < 1e-12) degenerates that axis into "the start point must
/// fall inside that axis slab", with no division — dividing by a near-zero number would yield ±inf,
/// and the min/max chain semantics of inf are not guaranteed to match between CPU (f64) and GPU
/// (f32), so an explicit branch is the only way the two sides mirror each other. The GPU side
/// (gpu.rs WGSL `shadowed`) is line-by-line isomorphic.
fn segment_hits_aabb(
    (px, py): (f64, f64),
    (qx, qy): (f64, f64),
    (x0, y0, x1, y1): (f64, f64, f64, f64),
) -> bool {
    let dx = qx - px;
    let dy = qy - py;
    let mut tmin = 0.0f64;
    let mut tmax = 1.0f64;
    if dx.abs() < 1e-12 {
        if px < x0 || px > x1 {
            return false;
        }
    } else {
        let t1 = (x0 - px) / dx;
        let t2 = (x1 - px) / dx;
        tmin = tmin.max(t1.min(t2));
        tmax = tmax.min(t1.max(t2));
    }
    if dy.abs() < 1e-12 {
        if py < y0 || py > y1 {
            return false;
        }
    } else {
        let t1 = (y0 - py) / dy;
        let t2 = (y1 - py) / dy;
        tmin = tmin.max(t1.min(t2));
        tmax = tmax.min(t1.max(t2));
    }
    tmax >= tmin
}

/// One merged big occluder box (pixel space). `aabb` is the per-component min/max of the member
/// sub-box pixel AABBs (**not** a world-space merge box re-transformed — each sub-box goes through
/// the original transform expression, min/max guarantees the big-box edges are bit-aligned with the
/// sub-box edges, so an outside hit is bit-equivalent to per-box hits).
pub struct MergedOccluder {
    /// [x0, y0, x1, y1], pixel space.
    pub aabb: [f64; 4],
    /// Start offset within [`ShadowBoxes::subs`].
    pub sub_start: usize,
    /// Number of member sub-boxes.
    pub sub_len: usize,
}

/// Per-frame acceleration structure for projection occlusion (pixel space): adjacent edge-aligned
/// occluders merge into big boxes; pixels outside a big box only test the big box; pixels inside a
/// big box fall back to the original sub-boxes ("the box the pixel is in does not block itself" is
/// still judged per original entity — merging does not change the semantics of this rule). See
/// [`build_shadow_boxes`].
pub struct ShadowBoxes {
    pub merged: Vec<MergedOccluder>,
    /// Pixel-space AABBs [x0, y0, x1, y1] of the original occluders, regrouped into contiguous
    /// ranges by `merged`. Each box's value uses the same transform expression as the per-box path
    /// — bit-identical.
    pub subs: Vec<[f64; 4]>,
}

/// Merge occluders into big boxes and transform to pixel space (once per frame; shared by the CPU
/// per-pixel path and the GPU uniform packing — the semantic source is here).
///
/// Merge rule: two passes of greedy 1D merging — first along x (tile rows in the same y range
/// collapse into horizontal strips), then along y (strips in the same x range stack into big
/// blocks). Merging happens only when **world-space edges are bit-equal f64** (aligned seamlessly)
/// and both sides are well-formed boxes (w/h > 0): union == big-box means the shadow geometry does
/// not change by a single bit, while a tolerant "roughly aligned" would make union != big-box and
/// shadow bytes drift, so it is not done. Sort keys all go through `total_cmp` + the original slot
/// — the result is independent of input order, frame-deterministic.
///
/// Equivalence (output bytes are bit-identical before and after merging, locked by tests): a slab
/// hit of an outside pixel against the big box == the OR of slab hits against each member sub-box —
/// provided the sub-box pixel edges are bit-shared (aligned world edges go through the same transform
/// to get the same f64; tile coordinates are a common case that is exactly representable in binary).
/// In-box pixels skip the big box and judge per original sub-box with the same formula as the
/// unmerged path.
pub fn build_shadow_boxes(
    occluders: &[Occluder],
    width: u32,
    height: u32,
    (cam_x, cam_y, scale): (f64, f64, f64),
) -> ShadowBoxes {
    struct Group {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        members: Vec<usize>,
    }
    // World-space edges (used only for the merge decision; pixel transform always uses the original
    // Occluder through the original expression)
    let mut groups: Vec<Group> = occluders
        .iter()
        .enumerate()
        .map(|(i, o)| Group {
            x0: o.x - o.w / 2.0,
            y0: o.y - o.h / 2.0,
            x1: o.x + o.w / 2.0,
            y1: o.y + o.h / 2.0,
            members: vec![i],
        })
        .collect();
    // Greedy 1D merge: sort then linearly scan; merge into the tail when aligned (the tail edge
    // extends as merging proceeds, so a long chain is collected in one pass)
    let merge_pass = |mut gs: Vec<Group>, along_x: bool| -> Vec<Group> {
        gs.sort_by(|a, b| {
            let key = |g: &Group| {
                if along_x {
                    (g.y0, g.y1, g.x0, g.x1)
                } else {
                    (g.x0, g.x1, g.y0, g.y1)
                }
            };
            let (a0, a1, a2, a3) = key(a);
            let (b0, b1, b2, b3) = key(b);
            a0.total_cmp(&b0)
                .then(a1.total_cmp(&b1))
                .then(a2.total_cmp(&b2))
                .then(a3.total_cmp(&b3))
                .then(a.members[0].cmp(&b.members[0]))
        });
        let mut out: Vec<Group> = Vec::with_capacity(gs.len());
        for g in gs {
            if let Some(last) = out.last_mut() {
                let flush = if along_x {
                    last.y0 == g.y0 && last.y1 == g.y1 && last.x1 == g.x0
                } else {
                    last.x0 == g.x0 && last.x1 == g.x1 && last.y1 == g.y0
                };
                // Degenerate / inverted boxes (w/h ≤ 0) do not participate in merging: each forms
                // its own group = original behavior preserved as-is
                let well_formed = last.x0 < last.x1
                    && last.y0 < last.y1
                    && g.x0 < g.x1
                    && g.y0 < g.y1;
                if flush && well_formed {
                    if along_x {
                        last.x1 = g.x1;
                    } else {
                        last.y1 = g.y1;
                    }
                    last.members.extend(g.members);
                    continue;
                }
            }
            out.push(g);
        }
        out
    };
    groups = merge_pass(groups, true);
    groups = merge_pass(groups, false);

    let mut subs = Vec::with_capacity(occluders.len());
    let mut merged = Vec::with_capacity(groups.len());
    for g in groups {
        let sub_start = subs.len();
        // For a single-member group the aabb is the sub-box itself (min/max take a side each;
        // inverted boxes are also preserved as-is)
        let mut aabb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for &i in &g.members {
            let o = &occluders[i];
            // Same transform expression as the per-box path (framing includes shake, light follows
            // the picture) — bit-identical
            let cx = (width as f64) / 2.0 + (o.x - cam_x) * scale;
            let cy = (height as f64) / 2.0 - (o.y - cam_y) * scale;
            let (hw, hh) = (o.w * scale / 2.0, o.h * scale / 2.0);
            let b = [cx - hw, cy - hh, cx + hw, cy + hh];
            aabb[0] = aabb[0].min(b[0]);
            aabb[1] = aabb[1].min(b[1]);
            aabb[2] = aabb[2].max(b[2]);
            aabb[3] = aabb[3].max(b[3]);
            subs.push(b);
        }
        merged.push(MergedOccluder { aabb, sub_start, sub_len: g.members.len() });
    }
    ShadowBoxes { merged, subs }
}

/// This light's occluder candidates: the big-box indices whose disc (light center (lx,ly), radius r
/// in pixels) is reachable. Culling is **lossless**: a pixel only does an occlusion test when
/// d² < r²; both endpoints of the segment (pixel, light center) are inside the light disc, and the
/// disc is convex → the whole segment is inside the disc → a box the disc cannot reach is never
/// hit. Shared by the CPU per-pixel path and the GPU uniform packing (semantic source is here).
pub fn cull_shadow_boxes(boxes: &ShadowBoxes, lx: f64, ly: f64, r: f64) -> Vec<u32> {
    let r2 = r * r;
    boxes
        .merged
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            // The slab method's interval normalization (min/max of t1/t2) means an inverted box's
            // hit behavior equals the normalized box — the closest point is also computed on the
            // normalized edges, so the cull test and the hit test agree
            let (x0, x1) = (m.aabb[0].min(m.aabb[2]), m.aabb[0].max(m.aabb[2]));
            let (y0, y1) = (m.aabb[1].min(m.aabb[3]), m.aabb[1].max(m.aabb[3]));
            let dx = lx - lx.clamp(x0, x1);
            let dy = ly - ly.clamp(y0, y1);
            dx * dx + dy * dy <= r2
        })
        .map(|(i, _)| i as u32)
        .collect()
}

/// Light source kind (parsed result of `Light.kind`). All angle fields are **degrees**, world
/// space, 0 = +x, counter-clockwise positive — same convention as `Sprite.rot` (semantic source
/// see [`rot_of`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// Point light (kind default).
    Point,
    /// Spotlight: `angle` = cone full-width (1..=360), `dir` = facing.
    Spot { angle: f64, dir: f64 },
    /// Directional light: `dir` = the direction the light travels. The contribution at a pixel
    /// without a normal is uniform everywhere (= color·intensity, old bytes unchanged); pixels
    /// with normals compute `max(dot(N, L), 0)` from dir (see module docs).
    Directional { dir: f64 },
}

impl LightKind {
    /// String name in describe / error messages (matches the legal values of `Light.kind`).
    pub fn name(&self) -> &'static str {
        match self {
            LightKind::Point => "point",
            LightKind::Spot { .. } => "spot",
            LightKind::Directional { .. } => "directional",
        }
    }
}

/// One light source (parsed result of a `Light` entity, world coordinates).
pub struct LightSource {
    pub id: vitric_ecs::EntityId,
    pub name: Option<String>,
    /// World coordinates. Directional does not read Position, always 0 (placeholder, does not
    /// participate in computation).
    pub x: f64,
    pub y: f64,
    /// World units; light attenuates to zero at radius. Directional does not read radius, always 0
    /// (placeholder).
    pub radius: f64,
    pub intensity: f64,
    /// Original color string (for describe output).
    pub color: String,
    /// Parsed 0..1 channel values of color (not multiplied by intensity).
    pub rgb: [f64; 3],
    pub kind: LightKind,
}

/// Collect every light source in the scene (entities with the `Light` component, in slot order).
/// Exceeding [`MAX_LIGHTS`] directly errors (combined across the three kinds — per-pixel /
/// per-fragment both iterate all lights, directional is not exempt). Validation all happens here:
/// kind validity, point/spot must have Position, spot's angle/dir, directional's dir — the render
/// hot path is left with pure arithmetic.
pub fn collect_lights(world: &World) -> Result<Vec<LightSource>, String> {
    let ids = world.query(&["Light"]);
    if ids.len() > MAX_LIGHTS {
        return Err(format!(
            "场上有 {} 个光源（Light 组件），超过上限 {MAX_LIGHTS} 盏（三种 kind 合计）。\
             提示：删减/合并灯，大面积照亮改用调亮 Ambient.color 或一盏平行光",
            ids.len()
        ));
    }
    ids.into_iter()
        .map(|id| {
            // kind: optional text field, default "point" (old scenes have no such field, behavior
            // must be unchanged)
            let kind_str = match world.get_field(id, "Light.kind") {
                Err(_) => "point".to_string(),
                Ok(v) => v.as_str().map(String::from).ok_or_else(|| {
                    format!(
                        "实体 {id} 的 Light.kind 不是文本: {v}。\
                         可选: \"point\"（点光源，缺省）/ \"spot\"（聚光灯）/ \"directional\"（平行光）"
                    )
                })?,
            };
            // Required angle fields (in degrees) for spot/directional; missing ones get a usage hint
            let angle_field = |field: &str, hint: &str| -> Result<f64, String> {
                match world.get_field(id, &format!("Light.{field}")) {
                    Err(_) => Err(format!(
                        "实体 {id} 的 Light(kind=\"{kind_str}\") 缺 {field} 字段（度数）。{hint}"
                    )),
                    Ok(v) => v.as_f64().ok_or_else(|| {
                        format!("实体 {id} 的 Light.{field} 不是数字（度数）: {v}")
                    }),
                }
            };
            let kind = match kind_str.as_str() {
                "point" => LightKind::Point,
                "spot" => {
                    let angle = angle_field(
                        "angle",
                        "聚光灯写法: {\"kind\": \"spot\", \"radius\": 6, \"angle\": 60, \"dir\": 90}\
                         （angle = 锥角全宽，1..=360）",
                    )?;
                    if !(1.0..=360.0).contains(&angle) {
                        return Err(format!(
                            "实体 {id} 的 Light.angle 必须在 1..=360（锥角全宽，度数），拿到 {angle}"
                        ));
                    }
                    let dir = angle_field(
                        "dir",
                        "dir = 朝向，0 = +x 方向、逆时针为正（和 Sprite.rot 同一约定）",
                    )?;
                    LightKind::Spot { angle, dir }
                }
                "directional" => {
                    let dir = angle_field(
                        "dir",
                        "平行光写法: {\"kind\": \"directional\", \"dir\": 270, \"intensity\": 0.5}\
                         （dir = 光行进的方向，270 = 从上往下照）",
                    )?;
                    LightKind::Directional { dir }
                }
                other => {
                    return Err(format!(
                        "实体 {id} 的 Light.kind {other:?} 不认识。\
                         可选: \"point\"（点光源，缺省）/ \"spot\"（聚光灯）/ \"directional\"（平行光）"
                    ));
                }
            };
            // Directional lights do not read Position/radius (the sun is at infinity); point/spot must have them
            let (x, y, radius) = if matches!(kind, LightKind::Directional { .. }) {
                (0.0, 0.0, 0.0)
            } else {
                let axis = |a: &str| -> Result<f64, String> {
                    match world.get_field(id, &format!("Position.{a}")) {
                        Err(_) => Err(format!(
                            "实体 {id} 的 Light(kind=\"{kind_str}\") 需要 Position 组件（灯在哪）。\
                             不想给位置的全场均匀光改用 kind: \"directional\""
                        )),
                        Ok(v) => v
                            .as_f64()
                            .ok_or_else(|| format!("实体 {id} 的 Position.{a} 不是数字: {v}")),
                    }
                };
                let (x, y) = (axis("x")?, axis("y")?);
                let radius = num(world, id, "Light.radius")?;
                if radius <= 0.0 {
                    return Err(format!("实体 {id} 的 Light.radius 必须 > 0，拿到 {radius}"));
                }
                (x, y, radius)
            };
            let intensity = world
                .get_field(id, "Light.intensity")
                .ok()
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let color = world
                .get_field(id, "Light.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Light.color: {e}"))?;
            Ok(LightSource {
                id,
                name: world.name_of(id).map(String::from),
                x,
                y,
                radius,
                intensity,
                color,
                rgb: [rgba[0] as f64 / 255.0, rgba[1] as f64 / 255.0, rgba[2] as f64 / 255.0],
                kind,
            })
        })
        .collect()
}

/// Per-pixel lighting (CPU path). Formula in the module docs; the GPU side (gpu.rs WGSL) must keep
/// the same formula and the same order (multiplied in sRGB byte space). All kind branches are done
/// **outside** the per-pixel loop:
/// - directional is uniform everywhere → folded into the base color once per frame
///   (`base = ambient + Σ directional`), the inner loop is zero-cost;
/// - point/spot split into two independent lists — the inner loop for a pure-point scene is
///   instruction-identical to before kind was added (byte-level backward compatibility locked by
///   tests), only spotlights pay the extra angular-falloff cost.
///
/// Light params are transformed to pixel space first, so the point-light inner loop is left with
/// only a squared-distance comparison.
///
/// `normals`: per-frame normal buffer (sentinel zero vector = no normal). Pixels with normals get
/// each light's contribution multiplied by an additional `max(dot(N, L), 0)`, and directional also
/// computes the direction from dir (no longer folded into the base); sentinel pixels take the old
/// path above, **output bytes bit-identical**. `None` = no normals for the whole frame (equivalent
/// to all-sentinel but saves one table lookup).
///
/// `occluders`: projection occluders (empty = no projection = arithmetic bit-identical). point/spot
/// do the segment occlusion test only after the distance check passes and the contribution is
/// non-zero; occluders are first merged into big boxes ([`build_shadow_boxes`]), then culled per
/// light disc ([`cull_shadow_boxes`]) — neither step changes output bytes (locked by tests).
/// directional does not cast shadows (v1, see module docs).
#[allow(clippy::too_many_arguments)]
fn apply_lighting(
    buf: &mut [u8],
    width: u32,
    height: u32,
    cam: (f64, f64, f64),
    ambient: [f64; 3],
    lights: &[LightSource],
    occluders: &[Occluder],
    normals: Option<&[[f32; 3]]>,
) {
    let grid = build_shadow_boxes(occluders, width, height, cam);
    apply_lighting_impl(buf, width, height, cam, ambient, lights, &grid, true, normals);
}

/// Body of [`apply_lighting`]; the occlusion structure is supplied by the caller (`cull=false` =
/// no per-light culling, full candidates — the reference path for equivalence tests; normal
/// rendering always uses `cull=true`).
#[allow(clippy::too_many_arguments)]
fn apply_lighting_impl(
    buf: &mut [u8],
    width: u32,
    height: u32,
    (cam_x, cam_y, scale): (f64, f64, f64),
    ambient: [f64; 3],
    lights: &[LightSource],
    grid: &ShadowBoxes,
    cull: bool,
    normals: Option<&[[f32; 3]]>,
) {
    struct PxLight {
        x: f64,
        y: f64,
        r: f64,
        r2: f64,
        /// Channel values already multiplied by intensity.
        rgb: [f64; 3],
    }
    struct PxSpot {
        base: PxLight,
        /// Unit vector of the facing in **pixel space** (world dir degrees → (cos, -sin), y flipped).
        dir: [f64; 2],
        /// Half cone angle (radians). collect_lights guarantees angle ∈ 1..=360 → half > 0, so the
        /// division is safe.
        half: f64,
    }
    let to_px = |l: &LightSource| {
        let r = l.radius * scale;
        PxLight {
            x: (width as f64) / 2.0 + (l.x - cam_x) * scale,
            y: (height as f64) / 2.0 - (l.y - cam_y) * scale,
            r,
            r2: r * r,
            rgb: [l.rgb[0] * l.intensity, l.rgb[1] * l.intensity, l.rgb[2] * l.intensity],
        }
    };
    /// Precomputed values for directional lights on the normal path: L = (−unit travel
    /// direction·0.8, 0.6) (unit length guaranteed by construction).
    struct PxDir {
        l: [f64; 3],
        rgb: [f64; 3],
    }
    let mut base = ambient;
    let mut points: Vec<PxLight> = Vec::new();
    let mut spots: Vec<PxSpot> = Vec::new();
    let mut dirs: Vec<PxDir> = Vec::new();
    for l in lights {
        match l.kind {
            LightKind::Point => points.push(to_px(l)),
            LightKind::Spot { angle, dir } => {
                let rad = dir.to_radians();
                spots.push(PxSpot {
                    base: to_px(l),
                    dir: [rad.cos(), -rad.sin()],
                    half: (angle / 2.0).to_radians(),
                });
            }
            // Directional: a sentinel pixel's contribution = color·intensity is the same everywhere
            // → fold into base, no per-pixel cost; normal pixels must compute max(dot(N,L),0) from
            // dir → store a separate copy carrying L (travel direction in pixel space (cos,-sin),
            // pointing at the light = negate, then lift to a unit vector with 0.8/0.6)
            LightKind::Directional { dir } => {
                for (c, b) in base.iter_mut().enumerate() {
                    *b += l.rgb[c] * l.intensity;
                }
                let rad = dir.to_radians();
                dirs.push(PxDir {
                    l: [
                        -rad.cos() * NORMAL_LIGHT_XY,
                        rad.sin() * NORMAL_LIGHT_XY,
                        NORMAL_LIGHT_Z,
                    ],
                    rgb: [l.rgb[0] * l.intensity, l.rgb[1] * l.intensity, l.rgb[2] * l.intensity],
                });
            }
        }
    }

    // Each light's occluder candidates (merged big-box indices): cull the boxes the light disc
    // cannot reach (lossless, see cull_shadow_boxes). When projection is off the grid is empty —
    // the candidates are all empty, and the old-path arithmetic does not move a single bit.
    let light_boxes = |l: &PxLight| -> Vec<u32> {
        if cull {
            cull_shadow_boxes(grid, l.x, l.y, l.r)
        } else {
            (0..grid.merged.len() as u32).collect()
        }
    };
    let point_boxes: Vec<Vec<u32>> = points.iter().map(light_boxes).collect();
    let spot_boxes: Vec<Vec<u32>> = spots.iter().map(|s| light_boxes(&s.base)).collect();
    // Whether the segment from pixel (fx,fy) to light center (lx,ly) is blocked by some candidate
    // box. Pixels outside a big box only test the big box (union == big-box, hits bit-equivalent,
    // see build_shadow_boxes); pixels inside a big box fall back to the original sub-boxes — the
    // box the pixel itself is in is skipped (rule: a pixel inside a box is only blocked by other
    // boxes, it does not crush itself into a black block), and merging does not change the byte
    // semantics of this rule.
    let blocked = |fx: f64, fy: f64, lx: f64, ly: f64, candidates: &[u32]| -> bool {
        candidates.iter().any(|&k| {
            let m = &grid.merged[k as usize];
            let [x0, y0, x1, y1] = m.aabb;
            if fx >= x0 && fx <= x1 && fy >= y0 && fy <= y1 {
                grid.subs[m.sub_start..m.sub_start + m.sub_len].iter().any(
                    |&[bx0, by0, bx1, by1]| {
                        let inside = fx >= bx0 && fx <= bx1 && fy >= by0 && fy <= by1;
                        !inside && segment_hits_aabb((fx, fy), (lx, ly), (bx0, by0, bx1, by1))
                    },
                )
            } else {
                segment_hits_aabb((fx, fy), (lx, ly), (x0, y0, x1, y1))
            }
        })
    };

    // —— Bounded per-light scan. Before the refactor this was per-pixel scan over all lights
    //    (pixel-count × light-count distance checks per frame); after the refactor point/spot only
    //    scan their own light-disc bounding box — pixels outside the box do not even compute the
    //    distance. The byte-equivalence basis: the f64 addition **sequence** each pixel receives
    //    is unchanged — initialization (ambient/base + directional) → point lights (slot order) →
    //    spotlights (slot order) → final clamp and multiply-back; out-of-box pixels took the
    //    d²≥r² continue branch in the old code (no addition), and the new code does not visit them
    //    at all (also no addition). The locked lighting/projection/normal tests all cover this step.

    // Unit direction from pixel to light → xy·0.8 + z 0.6; d=0 direction is undefined, by
    // convention (0,0,1)
    fn lambert(n: [f64; 3], dx: f64, dy: f64, d: f64) -> f64 {
        let l = if d > 0.0 {
            [-dx / d * NORMAL_LIGHT_XY, -dy / d * NORMAL_LIGHT_XY, NORMAL_LIGHT_Z]
        } else {
            [0.0, 0.0, 1.0]
        };
        (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0)
    }
    // This pixel's normal (sentinel zero vector = none → None, takes the old path — bytes locked)
    let normal_at = |i: usize| -> Option<[f64; 3]> {
        normals
            .map(|ns| ns[i])
            .filter(|n| n[2] != 0.0)
            .map(|n| [n[0] as f64, n[1] as f64, n[2] as f64])
    };
    // Row-level candidate filter: the slab method's y interval depends only on (fy, ly, box y
    // edges) — it is constant for the whole row. A box whose y interval is already empty cannot
    // be hit by any pixel on this row (the x axis only tightens the interval); predicted with the
    // **exact same formula** as segment_hits_aabb, the filter is bit-lossless; the sub-box y edges
    // are clamped by the big box's min/max, an empty big-box interval ⇒ an even emptier sub-box
    // interval, so the in-box fallback path is also lossless.
    let row_pass = |fy: f64, ly: f64, y0: f64, y1: f64| -> bool {
        let dy = ly - fy;
        if dy.abs() < 1e-12 {
            fy >= y0 && fy <= y1
        } else {
            let t1 = (y0 - fy) / dy;
            let t2 = (y1 - fy) / dy;
            let tmin = 0.0f64.max(t1.min(t2));
            let tmax = 1.0f64.min(t1.max(t2));
            tmax >= tmin
        }
    };
    // Reuse buffer for row-level candidates (avoid per-row allocation)
    let mut row_cand: Vec<u32> = Vec::new();

    // Light-disc bounding box (clipped to the viewport; outside ±r the d²≥r² check always fails,
    // 1px margin covers the floating-point edge)
    let light_rect = |lx: f64, ly: f64, r: f64| -> (u32, u32, u32, u32) {
        (
            ((lx - r - 1.5).floor().max(0.0) as u32).min(width),
            ((lx + r + 1.5).ceil().max(0.0) as u32).min(width),
            ((ly - r - 1.5).floor().max(0.0) as u32).min(height),
            ((ly + r + 1.5).ceil().max(0.0) as u32).min(height),
        )
    };

    // —— The lighting accumulation buffer is only as large as the bounding rectangle "reachable by
    //    some light" (the union outer box of all light-disc boxes): pixels far from every light are
    //    not allocated or accessed for the whole frame, and the composite step takes the untouched
    //    path directly. touched = this pixel has received a point/spot contribution (the accumulated
    //    value has diverged from the starting point).
    let (mut ux0, mut ux1, mut uy0, mut uy1) = (width, 0u32, height, 0u32);
    {
        let mut add_rect = |(x0, x1, y0, y1): (u32, u32, u32, u32)| {
            if x0 < x1 && y0 < y1 {
                ux0 = ux0.min(x0);
                ux1 = ux1.max(x1);
                uy0 = uy0.min(y0);
                uy1 = uy1.max(y1);
            }
        };
        for l in &points {
            add_rect(light_rect(l.x, l.y, l.r));
        }
        for l in &spots {
            add_rect(light_rect(l.base.x, l.base.y, l.base.r));
        }
    }
    let uw = ux1.saturating_sub(ux0);
    let un = (uw as usize) * (uy1.saturating_sub(uy0)) as usize;
    let mut lit_buf: Vec<[f64; 3]> = vec![[0.0; 3]; un];
    let mut touched: Vec<bool> = vec![false; un];
    // Frame pixel (x,y) → accumulation-buffer index (only called inside some light's box, must be
    // inside the union box)
    let local = |x: u32, y: u32| ((y - uy0) * uw + (x - ux0)) as usize;

    // The lighting accumulation **starting point** for a pixel (computed only when first receiving
    // a light contribution; untouched pixels are computed on the fly in the composite step with the
    // same formula): normal pixels = ambient + each directional computed by direction (L
    // construction in the module docs); sentinel pixels = base (directional already folded in)
    let init_lit = |i: usize| -> [f64; 3] {
        match normal_at(i) {
            Some(n) => {
                let mut acc = ambient;
                for dl in &dirs {
                    let f = (n[0] * dl.l[0] + n[1] * dl.l[1] + n[2] * dl.l[2]).max(0.0);
                    acc[0] += dl.rgb[0] * f;
                    acc[1] += dl.rgb[1] * f;
                    acc[2] += dl.rgb[2] * f;
                }
                acc
            }
            None => base,
        }
    };

    // Point-light pass (slot order — each pixel's accumulation order matches the per-pixel full scan)
    for (l, lb) in points.iter().zip(&point_boxes) {
        let (x0, x1, y0, y1) = light_rect(l.x, l.y, l.r);
        for y in y0..y1 {
            let fy = y as f64 + 0.5; // Pixel center — the GPU fragment's @builtin(position) is also a center coordinate
            row_cand.clear();
            row_cand.extend(lb.iter().copied().filter(|&k| {
                let m = &grid.merged[k as usize];
                row_pass(fy, l.y, m.aabb[1], m.aabb[3])
            }));
            for x in x0..x1 {
                let fx = x as f64 + 0.5;
                let dx = fx - l.x;
                let dy = fy - l.y;
                let d2 = dx * dx + dy * dy;
                if d2 >= l.r2 {
                    continue;
                }
                let i = (y * width + x) as usize;
                let d = d2.sqrt();
                let f = 1.0 - d / l.r;
                let f = match normal_at(i) {
                    // Normal pixel: contribution ×= max(dot(N, L), 0)
                    Some(n) => f * f * lambert(n, dx, dy, d),
                    // Old path: do not touch this formula (bytes locked)
                    None => f * f,
                };
                // Zero contribution (back face / d rounded up to r) — adding or not is the same
                // (+0.0 is a bit-wise no-op) — test for zero before the occlusion test, so
                // zero-contribution pixels do not touch a single box
                if f == 0.0 {
                    continue;
                }
                // Projection: blocked by an occluder = this light contributes zero to this pixel
                // (hard shadow); empty candidates short-circuit at zero cost — the projection-off
                // old path is byte-locked and unaffected
                if !row_cand.is_empty() && blocked(fx, fy, l.x, l.y, &row_cand) {
                    continue;
                }
                let li = local(x, y);
                if !touched[li] {
                    lit_buf[li] = init_lit(i);
                    touched[li] = true;
                }
                let lit = &mut lit_buf[li];
                lit[0] += l.rgb[0] * f;
                lit[1] += l.rgb[1] * f;
                lit[2] += l.rgb[2] * f;
            }
        }
    }

    // Spot pass (after point lights — each pixel is still point-first then spot, slot order)
    for (l, lb) in spots.iter().zip(&spot_boxes) {
        let (x0, x1, y0, y1) = light_rect(l.base.x, l.base.y, l.base.r);
        for y in y0..y1 {
            let fy = y as f64 + 0.5;
            row_cand.clear();
            row_cand.extend(lb.iter().copied().filter(|&k| {
                let m = &grid.merged[k as usize];
                row_pass(fy, l.base.y, m.aabb[1], m.aabb[3])
            }));
            for x in x0..x1 {
                let fx = x as f64 + 0.5;
                let dx = fx - l.base.x;
                let dy = fy - l.base.y;
                let d2 = dx * dx + dy * dy;
                if d2 >= l.base.r2 {
                    continue;
                }
                let i = (y * width + x) as usize;
                let d = d2.sqrt();
                let f = 1.0 - d / l.base.r;
                // Angular falloff (formula in module docs, GPU side mirrors line by line):
                //   Δθ = acos(pixel direction · facing), t = clamp(1 - Δθ/half, 0, 1),
                //   contribution ×= t². d=0 (pixel exactly at the light center) has undefined angle,
                //   by convention take the cone center (t=1)
                let cosd = if d > 0.0 {
                    ((dx * l.dir[0] + dy * l.dir[1]) / d).clamp(-1.0, 1.0)
                } else {
                    1.0
                };
                let t = (1.0 - cosd.acos() / l.half).clamp(0.0, 1.0);
                let f = match normal_at(i) {
                    Some(n) => f * f * t * t * lambert(n, dx, dy, d),
                    None => f * f * t * t,
                };
                // Outside the cone / back face has zero contribution: skip the occlusion test
                // (+0.0 is a bit-wise no-op) — the spot cone only covers part of the light disc,
                // out-of-cone pixels do not touch a single box
                if f == 0.0 {
                    continue;
                }
                // Projection: spotlights are blocked the same way as point lights (angular falloff
                // does not exempt occlusion)
                if !row_cand.is_empty() && blocked(fx, fy, l.base.x, l.base.y, &row_cand) {
                    continue;
                }
                let li = local(x, y);
                if !touched[li] {
                    lit_buf[li] = init_lit(i);
                    touched[li] = true;
                }
                let lit = &mut lit_buf[li];
                lit[0] += l.base.rgb[0] * f;
                lit[1] += l.base.rgb[1] * f;
                lit[2] += l.base.rgb[2] * f;
            }
        }
    }

    // Composite pass: clamp and multiply back to sRGB bytes (same formula as before the refactor,
    // alpha untouched). Sentinel pixels that never received a light contribution (accumulated value
    // always = base) take the 256-entry lookup table — table entries are precomputed per slot with
    // the same expression, output bytes are bit-identical to per-pixel on-the-fly computation;
    // normal pixels that never received a contribution compute the starting point on the fly (same
    // formula as init_lit) then multiply back.
    let mut lut = [[0u8; 256]; 3];
    for (c, table) in lut.iter_mut().enumerate() {
        let m = base[c].min(LIGHT_CLAMP);
        for (v, e) in table.iter_mut().enumerate() {
            *e = (v as f64 * m).min(255.0) as u8;
        }
    }
    let mul = |buf: &mut [u8], i: usize, lit: [f64; 3]| {
        for c in 0..3 {
            buf[i * 4 + c] = (buf[i * 4 + c] as f64 * lit[c].min(LIGHT_CLAMP)).min(255.0) as u8;
        }
    };
    for y in 0..height {
        let in_uy = y >= uy0 && y < uy1;
        for x in 0..width {
            let i = (y * width + x) as usize;
            if in_uy && x >= ux0 && x < ux1 && touched[local(x, y)] {
                mul(buf, i, lit_buf[local(x, y)]);
            } else if normals.is_some() {
                match normal_at(i) {
                    Some(_) => mul(buf, i, init_lit(i)),
                    None => {
                        for (c, table) in lut.iter().enumerate() {
                            buf[i * 4 + c] = table[buf[i * 4 + c] as usize];
                        }
                    }
                }
            } else {
                for (c, table) in lut.iter().enumerate() {
                    buf[i * 4 + c] = table[buf[i * 4 + c] as usize];
                }
            }
        }
    }
}

/// Bloom parameters (parsed result of the `Bloom` component).
pub struct BloomParams {
    /// 0..=1: the part of a channel value exceeding threshold·255 enters bloom.
    pub threshold: f64,
    /// ≥ 0: the bright part after blur is added back to the scene at this scale.
    pub strength: f64,
}

/// Bloom settings: take the first entity with a `Bloom` component (same convention as
/// Ambient/Camera). `None` = no Bloom in the scene = bloom is entirely off (master switch, not a
/// default parameter). Missing field / non-numeric / out-of-range all explicitly error — silently
/// skipping a post-effect parameter with a wrong value is harder to debug than erroring.
pub fn bloom_of(world: &World) -> Result<Option<BloomParams>, String> {
    match world.query(&["Bloom"]).first() {
        None => Ok(None),
        Some(&id) => {
            let field = |name: &str| -> Result<f64, String> {
                world
                    .get_field(id, &format!("Bloom.{name}"))
                    .ok()
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        format!(
                            "实体 {id} 挂了 Bloom 但 {name} 缺失或不是数字。\
                             写法: {{\"threshold\": 0.6, \"strength\": 0.8}}；\
                             不想要泛光就删掉 Bloom 组件"
                        )
                    })
            };
            let threshold = field("threshold")?;
            if !(0.0..=1.0).contains(&threshold) {
                return Err(format!(
                    "实体 {id} 的 Bloom.threshold 必须在 0..=1，拿到 {threshold}。\
                     0 = 全画面发光，1 = 什么都不发光"
                ));
            }
            let strength = field("strength")?;
            if strength < 0.0 {
                return Err(format!(
                    "实体 {id} 的 Bloom.strength 必须 ≥ 0，拿到 {strength}"
                ));
            }
            Ok(Some(BloomParams { threshold, strength }))
        }
    }
}

/// Bloom blur radius (pixels): viewport height / 90, lower bound 2 — scales with resolution, so
/// the bloom-to-frame ratio is resolution-independent. The CPU full-resolution blur uses this
/// value directly; the GPU half-resolution ping-pong uses half of it (see gpu.rs; the semantic
/// source is here).
pub fn bloom_radius_px(viewport_h: u32) -> u32 {
    (viewport_h / 90).max(2)
}

/// Full-screen bloom post-effect (CPU path, formula in module docs). Determinism: pure f32
/// arithmetic, fixed traversal order, no parallelism — same input → same output byte-for-byte.
/// Efficiency: separable box blur (a sliding window that only adds and subtracts twice per pixel
/// per direction); the bright plane and the scratch plane share one allocation.
fn apply_bloom(buf: &mut [u8], width: u32, height: u32, bloom: &BloomParams) {
    let (w, h) = (width as usize, height as usize);
    let n = w * h;
    let thr = (bloom.threshold * 255.0) as f32;

    // One allocation: first half = bright plane (RGB f32), second half = blur scratch
    let mut planes = vec![0f32; n * 3 * 2];
    let (a, b) = planes.split_at_mut(n * 3);
    for i in 0..n {
        for c in 0..3 {
            a[i * 3 + c] = (buf[i * 4 + c] as f32 - thr).max(0.0);
        }
    }

    // 3 iterations of separable box blur (H writes to scratch, V writes back to the bright plane),
    // approximating a Gaussian
    let r = bloom_radius_px(height) as usize;
    for _ in 0..3 {
        box_blur_pass(a, b, w, h, r, true);
        box_blur_pass(b, a, w, h, r, false);
    }

    // Additive composite: out = min(scene + blurred·strength, 255)
    let s = bloom.strength as f32;
    for i in 0..n {
        for c in 0..3 {
            let v = buf[i * 4 + c] as f32 + a[i * 3 + c] * s;
            buf[i * 4 + c] = v.min(255.0) as u8;
        }
    }
}

/// One pass of box blur in a single direction (`horizontal` picks the axis): window 2r+1, out-of-
/// bounds samples take the edge pixel (clamp-to-edge, same semantics as the GPU side's WGSL
/// clamp). Sliding window: each step adds the new sample and subtracts the old one; the f32
/// accumulation order is fixed → deterministic.
fn box_blur_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let norm = 1.0 / (2 * r + 1) as f32;
    // Unified as "scan along the len axis, across `lanes` lines": horizontal = one line per row
    // (stride 3), vertical = one line per column (stride 3w)
    let (lanes, len, lane_stride, step) = if horizontal {
        (h, w, w * 3, 3usize)
    } else {
        (w, h, 3usize, w * 3)
    };
    let ri = r as i64;
    let last = (len - 1) as i64;
    for lane in 0..lanes {
        let base = lane * lane_stride;
        for c in 0..3 {
            // Initial window: samples -r..=r (out-of-bounds clamped to the edge)
            let mut sum = 0f32;
            for k in -ri..=ri {
                sum += src[base + k.clamp(0, last) as usize * step + c];
            }
            dst[base + c] = sum * norm;
            for x in 1..len {
                let add = (x as i64 + ri).min(last) as usize;
                let sub = (x as i64 - 1 - ri).max(0) as usize;
                sum += src[base + add * step + c] - src[base + sub * step + c];
                dst[base + x * step + c] = sum * norm;
            }
        }
    }
}

/// Emission shape (parsed result of `Emitter.kind`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmitterKind {
    /// Continuous stream: `rate` particles per second, the emission timeline starts from tick 0.
    Stream { rate: f64 },
    /// Single burst: `burst` = the trigger tick number (negative = not triggered), `count`
    /// particles are born simultaneously.
    Burst { count: i64, burst: i64 },
}

impl EmitterKind {
    /// String name in describe / error messages (matches the legal values of `Emitter.kind`).
    pub fn name(&self) -> &'static str {
        match self {
            EmitterKind::Stream { .. } => "stream",
            EmitterKind::Burst { .. } => "burst",
        }
    }
}

/// One particle emitter (parsed result of an `Emitter` entity, world coordinates). Field semantics
/// in the module docs; all validation is in [`collect_emitters`], and particle expansion
/// ([`emitter_particles`]) is left with pure arithmetic.
#[derive(Debug)]
pub struct EmitterSource {
    pub id: vitric_ecs::EntityId,
    pub name: Option<String>,
    /// Emission origin (current Position — when the emitter moves, in-flight particles move with
    /// it, see module docs).
    pub x: f64,
    pub y: f64,
    pub kind: EmitterKind,
    /// Particle lifetime (ticks, ≥ 1).
    pub lifetime: i64,
    /// Initial velocity range (world units/second, 0 ≤ min ≤ max).
    pub speed_min: f64,
    pub speed_max: f64,
    /// Emission direction (degrees, 0 = +x, counter-clockwise positive) + spread full-width
    /// (degrees 0..=360).
    pub dir: f64,
    pub spread: f64,
    /// Gravitational acceleration (world units/second², y axis).
    pub gravity: f64,
    /// Start/end color (0..=255 channel values; rgb_end default = no rgb gradient) + original
    /// color string (for describe).
    pub rgb: [f64; 3],
    pub rgb_end: [f64; 3],
    pub color: String,
    /// Start/end size (world units; size_end default = no size gradient).
    pub size: f64,
    pub size_end: f64,
    pub active: bool,
    /// Hash seed derived from the entity id ([`emitter_seed`]).
    pub seed: u64,
}

/// SplitMix64 final mixer: 64-bit dispersion, stateless pure function. Particle hashing and screen
/// shake ([`shake_offset`]) share this one — the unified random source for the deterministic
/// decoration layer, never touching the simulation RNG stream.
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Emitter entity id → hash seed. The index/generation are packed into 64 bits then passed through
/// SplitMix64 — even two emitters in adjacent slots get dissimilar particle trajectories.
pub fn emitter_seed(id: vitric_ecs::EntityId) -> u64 {
    mix64(((id.index as u64) << 32) | id.generation as u64)
}

/// Collect every emitter in the scene (entities with the `Emitter` component, in slot order). All
/// validation happens here (kind validity, required fields, ranges, particle budget), and the hot
/// path is left with pure arithmetic.
pub fn collect_emitters(world: &World) -> Result<Vec<EmitterSource>, String> {
    let ids = world.query(&["Emitter"]);
    if ids.len() > MAX_EMITTERS {
        return Err(format!(
            "场上有 {} 个发射器（Emitter 组件），超过上限 {MAX_EMITTERS} 个。\
             提示：删减/合并发射器",
            ids.len()
        ));
    }
    ids.into_iter()
        .map(|id| {
            // Numeric field read: default value or explicit error (with a writing hint)
            let opt_num = |field: &str, default: f64| -> Result<f64, String> {
                match world.get_field(id, &format!("Emitter.{field}")) {
                    Err(_) => Ok(default),
                    Ok(v) => v.as_f64().ok_or_else(|| {
                        format!("实体 {id} 的 Emitter.{field} 不是数字: {v}")
                    }),
                }
            };
            let req_int = |field: &str, hint: &str| -> Result<i64, String> {
                match world.get_field(id, &format!("Emitter.{field}")) {
                    Err(_) => Err(format!("实体 {id} 的 Emitter 缺 {field} 字段。{hint}")),
                    Ok(v) => v.as_i64().ok_or_else(|| {
                        format!("实体 {id} 的 Emitter.{field} 必须是整数: {v}。{hint}")
                    }),
                }
            };
            let kind_str = match world.get_field(id, "Emitter.kind") {
                Err(_) => {
                    return Err(format!(
                        "实体 {id} 的 Emitter 缺 kind 字段。\
                         可选: \"stream\"（持续流，配 rate）/ \"burst\"（单次爆发，配 count + burst）"
                    ))
                }
                Ok(v) => v
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("实体 {id} 的 Emitter.kind 不是文本: {v}"))?,
            };
            let lifetime = req_int("lifetime", "粒子寿命（tick，整数 ≥ 1），如 40")?;
            if lifetime < 1 {
                return Err(format!(
                    "实体 {id} 的 Emitter.lifetime 必须 ≥ 1（tick），拿到 {lifetime}"
                ));
            }
            let kind = match kind_str.as_str() {
                "stream" => {
                    let rate = match world.get_field(id, "Emitter.rate") {
                        Err(_) => Err(format!(
                            "实体 {id} 的 Emitter(kind=\"stream\") 缺 rate 字段（粒子/秒）。\
                             写法: {{\"kind\": \"stream\", \"rate\": 20, \"lifetime\": 40, \"size\": 0.3}}"
                        )),
                        Ok(v) => v
                            .as_f64()
                            .ok_or_else(|| format!("实体 {id} 的 Emitter.rate 不是数字: {v}")),
                    }?;
                    if !(rate > 0.0 && rate.is_finite()) {
                        return Err(format!(
                            "实体 {id} 的 Emitter.rate 必须 > 0（粒子/秒），拿到 {rate}"
                        ));
                    }
                    // On-screen particle budget: steady-state visible count ≈ rate · lifetime / 60
                    let steady = (rate * lifetime as f64 / PARTICLE_TICKS_PER_SECOND).ceil();
                    if steady > MAX_PARTICLES_PER_EMITTER as f64 {
                        return Err(format!(
                            "实体 {id} 的 Emitter 稳态同屏约 {steady} 个粒子\
                             （rate {rate} × lifetime {lifetime} tick），\
                             超过单发射器预算 {MAX_PARTICLES_PER_EMITTER}。\
                             提示：调低 rate 或缩短 lifetime"
                        ));
                    }
                    EmitterKind::Stream { rate }
                }
                "burst" => {
                    let count = req_int(
                        "count",
                        "爆发粒子数（整数 ≥ 1）。写法: {\"kind\": \"burst\", \"count\": 30, \
                         \"lifetime\": 40, \"size\": 0.3}（规则往 burst 写当前 tick 即触发）",
                    )?;
                    if count < 1 {
                        return Err(format!(
                            "实体 {id} 的 Emitter.count 必须 ≥ 1，拿到 {count}"
                        ));
                    }
                    if count > MAX_PARTICLES_PER_EMITTER as i64 {
                        return Err(format!(
                            "实体 {id} 的 Emitter.count {count} 超过单发射器预算 \
                             {MAX_PARTICLES_PER_EMITTER}"
                        ));
                    }
                    // burst default -1 = not triggered (any negative counts as not triggered)
                    let burst = match world.get_field(id, "Emitter.burst") {
                        Err(_) => -1,
                        Ok(v) => v.as_i64().ok_or_else(|| {
                            format!(
                                "实体 {id} 的 Emitter.burst 必须是整数（触发 tick 号，负数 = 未触发）: {v}"
                            )
                        })?,
                    };
                    EmitterKind::Burst { count, burst }
                }
                other => {
                    return Err(format!(
                        "实体 {id} 的 Emitter.kind {other:?} 不认识。\
                         可选: \"stream\"（持续流）/ \"burst\"（单次爆发）"
                    ));
                }
            };
            // Emitters must have a position (same convention as point/spot lights)
            let axis = |a: &str| -> Result<f64, String> {
                match world.get_field(id, &format!("Position.{a}")) {
                    Err(_) => Err(format!(
                        "实体 {id} 的 Emitter 需要 Position 组件（发射原点在哪）"
                    )),
                    Ok(v) => v
                        .as_f64()
                        .ok_or_else(|| format!("实体 {id} 的 Position.{a} 不是数字: {v}")),
                }
            };
            let (x, y) = (axis("x")?, axis("y")?);
            let speed_min = opt_num("speed_min", 0.0)?;
            let speed_max = opt_num("speed_max", speed_min)?;
            if speed_min < 0.0 || speed_max < speed_min {
                return Err(format!(
                    "实体 {id} 的 Emitter 初速范围不合法：需要 0 ≤ speed_min ≤ speed_max，\
                     拿到 [{speed_min}, {speed_max}]"
                ));
            }
            let dir = opt_num("dir", 0.0)?;
            let spread = opt_num("spread", 360.0)?;
            if !(0.0..=360.0).contains(&spread) {
                return Err(format!(
                    "实体 {id} 的 Emitter.spread 必须在 0..=360（扩散角全宽，度数），拿到 {spread}"
                ));
            }
            let gravity = opt_num("gravity", 0.0)?;
            let size = match world.get_field(id, "Emitter.size") {
                Err(_) => Err(format!(
                    "实体 {id} 的 Emitter 缺 size 字段（粒子起始大小，世界单位 > 0），如 0.3"
                )),
                Ok(v) => v
                    .as_f64()
                    .ok_or_else(|| format!("实体 {id} 的 Emitter.size 不是数字: {v}")),
            }?;
            if size <= 0.0 {
                return Err(format!("实体 {id} 的 Emitter.size 必须 > 0，拿到 {size}"));
            }
            let size_end = opt_num("size_end", size)?;
            if size_end < 0.0 {
                return Err(format!(
                    "实体 {id} 的 Emitter.size_end 必须 ≥ 0（0 = 缩小到消失），拿到 {size_end}"
                ));
            }
            let color = world
                .get_field(id, "Emitter.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 Emitter.color: {e}"))?;
            let rgb = [rgba[0] as f64, rgba[1] as f64, rgba[2] as f64];
            // color_end default/empty = no gradient (same convention as Camera.follow empty = no follow)
            let color_end = world
                .get_field(id, "Emitter.color_end")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            let rgb_end = if color_end.is_empty() {
                rgb
            } else {
                let rgba = parse_color(&color_end)
                    .map_err(|e| format!("实体 {id} 的 Emitter.color_end: {e}"))?;
                [rgba[0] as f64, rgba[1] as f64, rgba[2] as f64]
            };
            let active = match world.get_field(id, "Emitter.active") {
                Err(_) => true,
                Ok(v) => v.as_bool().ok_or_else(|| {
                    format!("实体 {id} 的 Emitter.active 不是 bool: {v}")
                })?,
            };
            Ok(EmitterSource {
                id,
                name: world.name_of(id).map(String::from),
                x,
                y,
                kind,
                lifetime,
                speed_min,
                speed_max,
                dir,
                spread,
                gravity,
                rgb,
                rgb_end,
                color,
                size,
                size_end,
                active,
                seed: emitter_seed(id),
            })
        })
        .collect()
}

/// One particle to draw (world coordinates + already-computed size/color). Shared by the CPU
/// square-dot rasterizer and the GPU quad vertex stream — position/count/color are necessarily
/// identical on both paths.
pub struct ParticleDot {
    pub x: f64,
    pub y: f64,
    /// Current size (world units, gradient from size to size_end over the lifetime progress).
    pub size: f64,
    /// Current color (rgb gradient over the lifetime progress, alpha fades linearly 255 → 0).
    pub rgba: [u8; 4],
}

/// All in-flight particles of this emitter at tick `tick` — **pure function**: the same
/// (emitter fields, tick) always yields the same particle sequence (order: older particles first
/// = drawn at the bottom; burst particles are all the same age, ordered by index). No integrator:
/// the position is the analytic `pos = origin + v0·t + ½g·t²`, t = particle age (seconds). Each
/// particle's direction / initial velocity is hashed out by SplitMix64(seed ⊕ index) (never
/// touching the simulation RNG stream).
pub fn emitter_particles(e: &EmitterSource, tick: u64) -> Vec<ParticleDot> {
    let mut out = Vec::new();
    if !e.active {
        return out;
    }
    let t = tick as i64;
    match e.kind {
        EmitterKind::Stream { rate } => {
            // Particle indices born at tick b = [n(b), n(b+1)), n(b) = floor(b·rate/60).
            // Draw from oldest (age = lifetime-1) to newest — later-born particles cover the ones above
            let births_before =
                |b: i64| -> i64 { (b as f64 * rate / PARTICLE_TICKS_PER_SECOND).floor() as i64 };
            for age in (0..e.lifetime).rev() {
                let b = t - age;
                if b < 0 {
                    continue; // The world starts at tick 0, no earlier births
                }
                for k in births_before(b)..births_before(b + 1) {
                    out.push(particle_at(e, k as u64, age));
                }
            }
        }
        EmitterKind::Burst { count, burst } => {
            if burst < 0 {
                return out; // Not triggered
            }
            let age = t - burst;
            if age < 0 || age >= e.lifetime {
                return out; // Not yet at the trigger tick / lifetime already exhausted
            }
            for k in 0..count {
                out.push(particle_at(e, k as u64, age));
            }
        }
    }
    out
}

/// The particle with index `k` and age `age` (ticks) — pure arithmetic, stateless.
fn particle_at(e: &EmitterSource, k: u64, age: i64) -> ParticleDot {
    let h = mix64(e.seed ^ k);
    // High/low 32 bits each yield one [0,1] uniform number: direction offset + initial velocity
    let u1 = (h >> 32) as u32 as f64 / u32::MAX as f64;
    let u2 = h as u32 as f64 / u32::MAX as f64;
    let dir = e.dir + (u1 - 0.5) * e.spread;
    let speed = e.speed_min + u2 * (e.speed_max - e.speed_min);
    let secs = age as f64 / PARTICLE_TICKS_PER_SECOND;
    let (sn, cs) = dir.to_radians().sin_cos();
    // Lifetime progress 0..<1 (age ∈ 0..lifetime): color/size gradient linear, alpha fades linearly
    let s = age as f64 / e.lifetime as f64;
    let ch = |c: usize| (e.rgb[c] + (e.rgb_end[c] - e.rgb[c]) * s).round() as u8;
    ParticleDot {
        x: e.x + cs * speed * secs,
        y: e.y + sn * speed * secs + 0.5 * e.gravity * secs * secs,
        size: e.size + (e.size_end - e.size) * s,
        rgba: [ch(0), ch(1), ch(2), (255.0 * (1.0 - s)).round() as u8],
    }
}

/// Particle rasterization (CPU path): square dots, centered on world coordinates, edge length =
/// current size, with the same world→screen transform as sprites; src-alpha blending (same formula
/// as the alpha blend for image sprites). Called after lighting — particles are self-emissive and
/// do not participate in the normal buffer.
fn draw_particles(
    world: &World,
    buf: &mut [u8],
    width: u32,
    height: u32,
    (cam_x, cam_y, scale): (f64, f64, f64),
    tick: u64,
) -> Result<(), String> {
    for e in collect_emitters(world)? {
        for p in emitter_particles(&e, tick) {
            let a = p.rgba[3] as u32;
            if a == 0 {
                continue;
            }
            let cx = (width as f64) / 2.0 + (p.x - cam_x) * scale;
            let cy = (height as f64) / 2.0 - (p.y - cam_y) * scale;
            let half = p.size * scale / 2.0;
            let x0 = (cx - half).floor().max(0.0) as i64;
            let x1 = (cx + half).ceil().min(width as f64) as i64;
            let y0 = (cy - half).floor().max(0.0) as i64;
            let y1 = (cy + half).ceil().min(height as f64) as i64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    let dst = &mut buf[i..i + 4];
                    // src-alpha blend, same per-channel formula as for image sprites
                    for (d, s) in dst.iter_mut().zip(p.rgba).take(3) {
                        *d = ((s as u32 * a + *d as u32 * (255 - a)) / 255) as u8;
                    }
                    dst[3] = 255;
                }
            }
        }
    }
    Ok(())
}

/// Optional `Sprite.rot` (degrees). Default = 0 = no rotation; field present but non-numeric →
/// explicit error. Angle convention: **world-space counter-clockwise is positive** — after the
/// screen-y flip it still looks counter-clockwise on screen. CPU rasterization, GPU vertex stream,
/// and picking all share this one semantic source.
pub fn rot_of(world: &World, id: vitric_ecs::EntityId) -> Result<f64, String> {
    match world.get_field(id, "Sprite.rot") {
        Err(_) => Ok(0.0),
        Ok(v) => v
            .as_f64()
            .ok_or_else(|| format!("实体 {id} 的 Sprite.rot 不是数字（度数）: {v}")),
    }
}

/// Sample and decode one texel of the normal map → **screen-space** unit normal (convention in
/// module docs): n = rgb/255·2-1, z is taken absolute (forced outward), xy is rotated by the
/// sprite's rotation matrix (local→screen, the same matrix as the vertex transform [[c, s], [-s, c]])
/// and the whole vector is normalized; a zero vector degrades to the flat normal (0,0,1). (u, v)
/// matches the diffuse-texture set (including clamp behavior); the normal map size need not match
/// the diffuse map.
fn sample_normal(img: &Image, u: f64, v: f64, sn: f64, cs: f64) -> [f32; 3] {
    let sx = ((u * img.width as f64) as i64).clamp(0, img.width as i64 - 1) as usize;
    let sy = ((v * img.height as f64) as i64).clamp(0, img.height as i64 - 1) as usize;
    let s = (sy * img.width as usize + sx) * 4;
    let nx = img.rgba[s] as f64 / 255.0 * 2.0 - 1.0;
    let ny = img.rgba[s + 1] as f64 / 255.0 * 2.0 - 1.0;
    let nz = (img.rgba[s + 2] as f64 / 255.0 * 2.0 - 1.0).abs();
    let rx = cs * nx + sn * ny;
    let ry = -sn * nx + cs * ny;
    let len = (rx * rx + ry * ry + nz * nz).sqrt();
    if len < 1e-9 {
        return [0.0, 0.0, 1.0];
    }
    [(rx / len) as f32, (ry / len) as f32, (nz / len) as f32]
}

/// Get an image asset; a missing image directly errors and lists the existing assets (no placeholder
/// drawn).
fn image_of<'a>(
    assets: &'a Assets,
    id: vitric_ecs::EntityId,
    image_name: &str,
) -> Result<&'a Image, String> {
    assets.image(image_name).ok_or_else(|| {
        format!(
            "实体 {id} 的 Sprite.image {image_name:?} 不在素材仓库里。\
             现有素材: [{}]。提示：图放进项目 assets/ 目录，路径相对 assets/ 写",
            assets.names().join(", ")
        )
    })
}

/// Text: `Text` {"content","size","color"} + `Position`, drawn above all sprites. Two paths
/// (semantics in the module docs' Text convention): if no font is mounted in the asset store, the
/// built-in 8x8 bitmap is used (ASCII, monospace, non-ASCII draws a solid block placeholder —
/// **these bytes must not change**, backward compatibility locked by tests); if a font is mounted
/// (manifest `font`), all Text goes through the vector path (proportional spacing + coverage
/// anti-aliasing). Text is **always upright** — `Sprite.rot` only rotates sprites, not text (HUD
/// stays horizontal). `normals`: text pixels clear the normal underneath (when text covers a
/// normal-mapped sprite it is lit as a flat surface, not inheriting the relief). `skip`: skip this
/// one Text entity (contrast-measurement only, see [`RenderOpts`]); `None` = draw all.
#[allow(clippy::too_many_arguments)]
fn draw_texts(
    world: &World,
    buf: &mut [u8],
    width: u32,
    height: u32,
    (cam_x, cam_y, scale): (f64, f64, f64),
    assets: &Assets,
    normals: &mut Option<Vec<[f32; 3]>>,
    skip: Option<vitric_ecs::EntityId>,
) -> Result<(), String> {
    for id in world.query(&["Position", "Text"]) {
        if skip == Some(id) {
            continue;
        }
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
        // reveal (0..=1 ratio, the progress of text reveal): default 1.0 = fully shown. The visible
        // character count is a pure function of reveal; missing field / ≥1 is byte-identical to
        // before this feature was introduced (backward compatible).
        let reveal = world
            .get_field(id, "Text.reveal")
            .ok()
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let total_chars = content.chars().count();
        let visible = revealed_chars(reveal, total_chars);
        if visible == 0 {
            continue; // No characters shown at all (reveal=0): same as empty content, draw nothing
        }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        // screen=true: HUD anchoring — Position is interpreted as an offset relative to the screen
        // center, not following the camera
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let (cx, cy) = if screen_anchored {
            ((width as f64) / 2.0 + px * scale, (height as f64) / 2.0 - py * scale)
        } else {
            ((width as f64) / 2.0 + (px - cam_x) * scale, (height as f64) / 2.0 - (py - cam_y) * scale)
        };

        // Vector path: when a font is mounted all Text goes here (per-Text overrides out of scope)
        if let Some(font) = assets.font() {
            // Lay out the whole string once (centered on content, layout cached), only the first
            // `visible` glyphs are drawn — per-character reveal never re-lays-out; the visible count
            // is just slicing the already-laid-out result
            draw_text_vector(
                buf, width, height, font, &content, size, scale, (cx, cy), rgba, normals, visible,
            );
            continue;
        }

        // —— Bitmap path: this logic must not change — when no font is mounted the output bytes
        //    must be bit-identical to before the font feature existed (backward compatibility
        //    locked by tests). reveal just truncates the drawn characters to the first `visible`
        //    (when fully shown chars == content.chars(), bytes unchanged)
        let chars: Vec<char> = content.chars().take(visible).collect();
        let n = chars.len();
        let half_w = n as f64 * size * scale / 2.0;
        let half_h = size * scale / 2.0;
        let x0 = (cx - half_w).floor().max(0.0) as i64;
        let x1 = (cx + half_w).ceil().min(width as f64) as i64;
        let y0 = (cy - half_h).floor().max(0.0) as i64;
        let y1 = (cy + half_h).ceil().min(height as f64) as i64;
        let span_x = 2.0 * half_w;
        let span_y = 2.0 * half_h;
        for y in y0..y1 {
            for x in x0..x1 {
                let u = ((x as f64 + 0.5) - (cx - half_w)) / span_x; // 0..1 spans the whole string
                let v = ((y as f64 + 0.5) - (cy - half_h)) / span_y; // 0..1 spans one character vertically
                let idx = ((u * n as f64) as usize).min(n - 1);
                let col = (((u * n as f64 - idx as f64) * 8.0) as usize).min(7);
                let row = ((v * 8.0) as usize).min(7);
                if glyph_of(chars[idx])[row] & (1 << col) != 0 {
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&rgba);
                    if let Some(ns) = normals.as_mut() {
                        ns[i / 4] = [0.0; 3];
                    }
                }
            }
        }
    }
    Ok(())
}

/// Character → 8x8 bitmap (one byte per row, low bit on the left). Non-ASCII uses a solid block placeholder.
fn glyph_of(c: char) -> [u8; 8] {
    let cp = c as usize;
    if cp < 128 {
        font8x8::legacy::BASIC_LEGACY[cp]
    } else {
        [0xff; 8]
    }
}

/// Vector text: lay out a whole string with proportional spacing, then rasterize each glyph
/// (cached) + coverage blending (anti-aliasing). Geometric conventions: font size = total glyph
/// height of size×scale pixels; the whole string is horizontally centered on (cx,cy), vertically
/// centering the glyph body (ascent..descent); glyphs land on integer pixels (no sub-pixel
/// placement, otherwise the cache key would not hold). The GPU path (vitric-cli gpu.rs) uses the
/// same layout/raster/rounding — visually aligned, but bit-identical output with the CPU is not
/// promised (screenshots/asserts treat this path as the source of truth).
#[allow(clippy::too_many_arguments)]
fn draw_text_vector(
    buf: &mut [u8],
    width: u32,
    height: u32,
    font: &FontStore,
    content: &str,
    size: f64,
    scale: f64,
    (cx, cy): (f64, f64),
    rgba: [u8; 4],
    normals: &mut Option<Vec<[f32; 3]>>,
    visible: usize,
) {
    let px_size = FontStore::px_size(size, scale);
    // Cached layout: lay the whole string out once into the memo; per-character reveal of the same
    // text played for N ticks runs layout exactly once. Centering uses the whole-string total width
    // (text does not jitter left/right during reveal); only the first `visible` glyphs are drawn.
    let laid = font.layout_cached(content, px_size);
    let (placements, total_w) = (&laid.0, laid.1);
    let left = cx - total_w as f64 / 2.0;
    let baseline = (cy + font.baseline_offset(px_size) as f64).round() as i64;
    for p in placements.iter().take(visible) {
        let g = font.raster(p.ch, px_size);
        if g.coverage.is_empty() {
            continue; // Empty outline (space etc.) only contributes advance
        }
        let gx0 = (left + p.x as f64).round() as i64 + g.left as i64;
        let gy0 = baseline + g.top as i64;
        for row in 0..g.height as i64 {
            let y = gy0 + row;
            if y < 0 || y >= height as i64 {
                continue;
            }
            for col in 0..g.width as i64 {
                let x = gx0 + col;
                if x < 0 || x >= width as i64 {
                    continue;
                }
                let cov = g.coverage[(row * g.width as i64 + col) as usize] as u32;
                if cov == 0 {
                    continue;
                }
                // Coverage blending = anti-aliasing. Vector text is the only element in the engine
                // that is intentionally smoothed; sprite images stay nearest-neighbor hard-edged
                // (the pixel-art look is preserved)
                let i = ((y as u32 * width + x as u32) * 4) as usize;
                let dst = &mut buf[i..i + 4];
                for c in 0..3 {
                    dst[c] = ((rgba[c] as u32 * cov + dst[c] as u32 * (255 - cov)) / 255) as u8;
                }
                dst[3] = 255;
                // Any text pixel with coverage > 0 clears the normal (half-covered edges also count
                // as text; no half-normals)
                if let Some(ns) = normals.as_mut() {
                    ns[i / 4] = [0.0; 3];
                }
            }
        }
    }
}

/// UI screen-space overlay rendering (CPU source of truth). Layout is provided by
/// [`ui::solve_layout`] (a pure function, no camera involved); here we only draw the solved
/// screen-pixel rects: Panel = background frame (solid color / sprite),
/// UiLabel = text (reusing the font.rs layout cache + Text.reveal).
///
/// Performance: no UI in the scene (no UiRoot) = early-return on the first line, zero allocation
/// and zero traversal (empty UI is zero-cost). Reuses the existing buf, no offscreen buffer.
/// Painter order follows the query slot order (deterministic; later draws cover earlier ones).
/// `normals` is not passed — UI is an overlay layer, drawn after lighting/bloom, and does not
/// participate in lighting itself (HUD semantics).
fn draw_ui(
    world: &World,
    buf: &mut [u8],
    width: u32,
    height: u32,
    assets: &Assets,
) -> Result<(), String> {
    if !ui::has_ui(world) {
        return Ok(()); // Empty UI zero-cost: no UiRoot, the entire UI path is zero-allocation zero-traversal
    }
    let layout = ui::solve_layout(world, width, height)?;

    // Panel: background frame. Drawn in entity order (later draws cover earlier ones). Solid color
    // = direct alpha-blended block; sprite = nearest-neighbor scaled image (NinePatch deferred to
    // 1.2; solid color + sprite is required for 1.1).
    for id in world.query(&["Ui", "Panel"]) {
        let Some(rect) = layout.get(&id) else { continue };
        // Press feedback (1.2): when Button is attached and press_t≥0, apply the analytic
        // press_scale/press_modulate formulas for scale (shrink around the rect center) + modulate
        // (brighten). **Pure render decoration**: only reads press_t from the component (enters the
        // hash and saves); the offset is a pure function of press_t, does not touch the layout rect
        // or the simulation RNG — replay/snapshot rollback is consistent (same decoration discipline
        // as shake/bloom). The CPU and GPU share ui_press_feedback.
        let (rect, modulate) = ui_interact::ui_press_feedback(world, id, *rect);
        let x0 = rect.x.floor().max(0.0) as i64;
        let y0 = rect.y.floor().max(0.0) as i64;
        let x1 = (rect.x + rect.w).ceil().min(width as f64) as i64;
        let y1 = (rect.y + rect.h).ceil().min(height as f64) as i64;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let image_name = world
            .get_field(id, "Panel.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if image_name.is_empty() {
            // Solid color block (with alpha) — Panel.color defaults to opaque white
            let color = world
                .get_field(id, "Panel.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let mut rgba = parse_color_a(&color).map_err(|e| format!("实体 {id} 的 Panel.color: {e}"))?;
            modulate_rgb(&mut rgba, modulate);
            let a = rgba[3] as u32;
            if a == 0 {
                continue; // Fully transparent = do not draw
            }
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    if a == 255 {
                        buf[i..i + 4].copy_from_slice(&rgba);
                    } else {
                        let dst = &mut buf[i..i + 4];
                        for c in 0..3 {
                            dst[c] = ((rgba[c] as u32 * a + dst[c] as u32 * (255 - a)) / 255) as u8;
                        }
                        dst[3] = 255;
                    }
                }
            }
        } else {
            // Sprite background: a missing image explicitly errors (no placeholder drawn) — same
            // policy as Sprite.image
            let img = assets.image(&image_name).ok_or_else(|| {
                format!(
                    "实体 {id} 的 Panel.image {image_name:?} 不在素材仓库里。现有素材: [{}]",
                    assets.names().join(", ")
                )
            })?;
            let span_x = rect.w;
            let span_y = rect.h;
            for y in y0..y1 {
                for x in x0..x1 {
                    let u = ((x as f64 + 0.5) - rect.x) / span_x;
                    let v = ((y as f64 + 0.5) - rect.y) / span_y;
                    let sx = ((u * img.width as f64) as i64).clamp(0, img.width as i64 - 1) as usize;
                    let sy = ((v * img.height as f64) as i64).clamp(0, img.height as i64 - 1) as usize;
                    let s = (sy * img.width as usize + sx) * 4;
                    let src = &img.rgba[s..s + 4];
                    let sa = src[3] as u32;
                    if sa == 0 {
                        continue;
                    }
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    let dst = &mut buf[i..i + 4];
                    for c in 0..3 {
                        dst[c] = ((src[c] as u32 * sa + dst[c] as u32 * (255 - sa)) / 255) as u8;
                    }
                    dst[3] = 255;
                }
            }
        }
    }

    // UiLabel: text. The whole string is laid out then drawn in the node frame, horizontally
    // aligned by `align` and vertically centered in the frame. Reuses font.rs (vector path when a
    // font is mounted, otherwise bitmap) + Text.reveal (per-character reveal already implemented).
    let mut no_normals: Option<Vec<[f32; 3]>> = None;
    for id in world.query(&["Ui", "UiLabel"]) {
        let Some(rect) = layout.get(&id) else { continue };
        let content = world
            .get_field(id, "UiLabel.content")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let size = world.get_field(id, "UiLabel.size").ok().and_then(Value::as_f64).unwrap_or(1.0);
        if size <= 0.0 {
            continue;
        }
        let color = world
            .get_field(id, "UiLabel.color")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "#ffffff".to_string());
        let rgba = parse_color(&color).map_err(|e| format!("实体 {id} 的 UiLabel.color: {e}"))?;
        let reveal = world.get_field(id, "UiLabel.reveal").ok().and_then(Value::as_f64).unwrap_or(1.0);
        let total = content.chars().count();
        let visible = revealed_chars(reveal, total);
        if visible == 0 {
            continue;
        }
        let align = world
            .get_field(id, "UiLabel.align")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "center".to_string());
        // UI font size is in **screen pixels** (no camera scale applied) — `size` is used directly
        // as the pixel height. Text is vertically centered in the node frame and horizontally
        // aligned by `align` within the frame.
        draw_ui_label(buf, width, height, assets, &content, size, &align, *rect, rgba, &mut no_normals, visible);
    }
    Ok(())
}

/// Draw one UI text (screen space, font size = pixel height, no camera). Vector path when a font
/// is mounted, otherwise bitmap — same two paths as world-space [`draw_texts`], but the coordinates
/// are the UI layout's screen rect.
#[allow(clippy::too_many_arguments)]
fn draw_ui_label(
    buf: &mut [u8],
    width: u32,
    height: u32,
    assets: &Assets,
    content: &str,
    size: f64,
    align: &str,
    rect: ui::UiRect,
    rgba: [u8; 4],
    normals: &mut Option<Vec<[f32; 3]>>,
    visible: usize,
) {
    let cy = rect.y + rect.h / 2.0; // Vertically centered in the frame
    if let Some(font) = assets.font() {
        // UI font size is directly in pixels (scale=1): px_size = total glyph height of `size` pixels
        let px_size = FontStore::px_size(size, 1.0);
        let laid = font.layout_cached(content, px_size);
        let (placements, total_w) = (&laid.0, laid.1);
        // Horizontal alignment: place the whole string within the frame width
        let left = match align {
            "start" => rect.x,
            "end" => rect.x + rect.w - total_w as f64,
            _ => rect.x + (rect.w - total_w as f64) / 2.0,
        };
        let baseline = (cy + font.baseline_offset(px_size) as f64).round() as i64;
        for p in placements.iter().take(visible) {
            let g = font.raster(p.ch, px_size);
            if g.coverage.is_empty() {
                continue;
            }
            let gx0 = (left + p.x as f64).round() as i64 + g.left as i64;
            let gy0 = baseline + g.top as i64;
            blit_coverage(buf, width, height, &g, gx0, gy0, rgba, normals);
        }
    } else {
        // Bitmap path: monospace size×size pixel cells
        let chars: Vec<char> = content.chars().take(visible).collect();
        let n = chars.len();
        let total_w = n as f64 * size;
        let left = match align {
            "start" => rect.x,
            "end" => rect.x + rect.w - total_w,
            _ => rect.x + (rect.w - total_w) / 2.0,
        };
        let top = cy - size / 2.0;
        let x0 = left.floor().max(0.0) as i64;
        let x1 = (left + total_w).ceil().min(width as f64) as i64;
        let y0 = top.floor().max(0.0) as i64;
        let y1 = (top + size).ceil().min(height as f64) as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let u = ((x as f64 + 0.5) - left) / total_w;
                let v = ((y as f64 + 0.5) - top) / size;
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let idx = ((u * n as f64) as usize).min(n - 1);
                let col = (((u * n as f64 - idx as f64) * 8.0) as usize).min(7);
                let row = ((v * 8.0) as usize).min(7);
                if glyph_of(chars[idx])[row] & (1 << col) != 0 {
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&rgba);
                }
            }
        }
    }
}

/// Blend a rasterized glyph's coverage bitmap into buf (anti-aliasing), pen at (gx0,gy0). Factored
/// out for UI text reuse (same blending policy as the inner loop of [`draw_text_vector`]).
#[allow(clippy::too_many_arguments)]
fn blit_coverage(
    buf: &mut [u8],
    width: u32,
    height: u32,
    g: &RasterGlyph,
    gx0: i64,
    gy0: i64,
    rgba: [u8; 4],
    normals: &mut Option<Vec<[f32; 3]>>,
) {
    for row in 0..g.height as i64 {
        let y = gy0 + row;
        if y < 0 || y >= height as i64 {
            continue;
        }
        for col in 0..g.width as i64 {
            let x = gx0 + col;
            if x < 0 || x >= width as i64 {
                continue;
            }
            let cov = g.coverage[(row * g.width as i64 + col) as usize] as u32;
            if cov == 0 {
                continue;
            }
            let i = ((y as u32 * width + x as u32) * 4) as usize;
            let dst = &mut buf[i..i + 4];
            for c in 0..3 {
                dst[c] = ((rgba[c] as u32 * cov + dst[c] as u32 * (255 - cov)) / 255) as u8;
            }
            dst[3] = 255;
            if let Some(ns) = normals.as_mut() {
                ns[i / 4] = [0.0; 3];
            }
        }
    }
}

/// Screen pixels → world coordinates (for inspector drag and picking). Uses the non-shaken camera:
/// picking/dragging targets the world itself, shake is just a few frames of visual decoration.
pub fn screen_to_world(
    world: &World,
    width: u32,
    height: u32,
    px: f64,
    py: f64,
) -> Result<(f64, f64), String> {
    let (cam_x, cam_y, scale) = camera_base(world, height)?;
    Ok((
        cam_x + (px - width as f64 / 2.0) / scale,
        cam_y - (py - height as f64 / 2.0) / scale,
    ))
}

/// Picking: return the topmost entity hit by screen coordinates (px,py) (later draw order wins).
pub fn pick(
    world: &World,
    width: u32,
    height: u32,
    px: f64,
    py: f64,
) -> Result<Option<vitric_ecs::EntityId>, String> {
    let (wx, wy) = screen_to_world(world, width, height, px, py)?;
    pick_world(world, wx, wy)
}

/// Picking (world-coordinate version): return the topmost entity hit by the world point (wx,wy).
/// Window clicks (screen coordinates first go through screen_to_world) and the control-plane
/// `input/click` (which gives world coordinates directly) share this judgment — what a human clicks
/// and what an AI clicks use bit-identical hit rules. Determinism: query is in slot order, does
/// not touch the simulation RNG, safe for click resolution inside a recording.
pub fn pick_world(
    world: &World,
    wx: f64,
    wy: f64,
) -> Result<Option<vitric_ecs::EntityId>, String> {
    let ids = world.query(&["Position", "Sprite"]);
    // Reverse order: later draws are on top, so they are hit first
    for &id in ids.iter().rev() {
        let x = num(world, id, "Position.x")?;
        let y = num(world, id, "Position.y")?;
        let w = num(world, id, "Sprite.w")?;
        let h = num(world, id, "Sprite.h")?;
        let rot = rot_of(world, id)?;
        // When rot != 0, inverse-rotate the click point back into the sprite's local space (world
        // system, y up) — hit testing targets the rotated real shape, not the un-rotated AABB
        let (dx, dy) = (wx - x, wy - y);
        let (lx, ly) = if rot == 0.0 {
            (dx, dy)
        } else {
            let (sn, cs) = rot.to_radians().sin_cos();
            (dx * cs + dy * sn, dy * cs - dx * sn)
        };
        if lx.abs() * 2.0 <= w && ly.abs() * 2.0 <= h {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Draw a selection outline on an already-rendered frame for an entity (inspector highlight,
/// cyan 2px). `tick` must be the same one used by this frame's `render_world` — the outline must
/// follow the shaken picture, otherwise it would be misaligned during screen shake.
pub fn draw_selection_outline(
    buf: &mut [u8],
    world: &World,
    width: u32,
    height: u32,
    selected: vitric_ecs::EntityId,
    tick: u64,
) -> Result<(), String> {
    if !world.is_alive(selected) || !world.has_component(selected, "Sprite") {
        return Ok(()); // The selected entity is gone/invisible — silently skip the outline (the
                        // selected-state itself is managed by the upper layer)
    }
    let (cam_x, cam_y, scale) = camera_of(world, tick, height)?;
    let x = num(world, selected, "Position.x")?;
    let y = num(world, selected, "Position.y")?;
    let w = num(world, selected, "Sprite.w")?;
    let h = num(world, selected, "Sprite.h")?;
    let rot = rot_of(world, selected)?;
    // When rot != 0 the outline uses the **axis-aligned bounding box of the rotated shape** —
    // drawing an axis-aligned rectangle is much simpler than tracing the rotated outline; the
    // inspector highlight only needs "see who is selected", edge-perfect accuracy is not required
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
    let x0 = (cx - half_w).floor().max(0.0) as i64;
    let x1 = (cx + half_w).ceil().min(width as f64) as i64 - 1;
    let y0 = (cy - half_h).floor().max(0.0) as i64;
    let y1 = (cy + half_h).ceil().min(height as f64) as i64 - 1;
    const TEAL: [u8; 4] = [39, 192, 168, 255];
    let mut put = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
            let i = ((y as u32 * width + x as u32) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&TEAL);
        }
    };
    for t in 0..2i64 {
        for x in x0..=x1 {
            put(x, y0 + t);
            put(x, y1 - t);
        }
        for y in y0..=y1 {
            put(x0 + t, y);
            put(x1 - t, y);
        }
    }
    Ok(())
}

/// Draw a "build placement preview" on the already-rendered frame: a translucent green
/// ghost + bright edge at the cursor's world grid cell, so the player sees where a build will
/// land before clicking. Only called by the window side when in build mode with a type selected.
/// `wx,wy` are the cursor's world coordinates (the window first does screen_to_world);
/// the placement snaps to the nearest whole grid cell.
/// `tick` must be the same one passed to this frame's render_world -- it must follow the screen jitter to avoid misalignment.
pub fn draw_build_preview(
    buf: &mut [u8],
    world: &World,
    width: u32,
    height: u32,
    wx: f64,
    wy: f64,
    tick: u64,
) -> Result<(), String> {
    let (cam_x, cam_y, scale) = camera_of(world, tick, height)?;
    // Snap to nearest whole grid cell (build lands on the clicked tile)
    let gx = wx.round();
    let gy = wy.round();
    let cx = (width as f64) / 2.0 + (gx - cam_x) * scale;
    let cy = (height as f64) / 2.0 - (gy - cam_y) * scale;
    let half = scale / 2.0; // 1x1 world grid cell
    let x0 = (cx - half).floor().max(0.0) as i64;
    let x1 = ((cx + half).ceil().min(width as f64) as i64 - 1).max(x0);
    let y0 = (cy - half).floor().max(0.0) as i64;
    let y1 = ((cy + half).ceil().min(height as f64) as i64 - 1).max(y0);
    const FILL: [u8; 3] = [120, 235, 150];
    const EDGE: [u8; 4] = [190, 255, 205, 255];
    let bw = 2i64; // Border width
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
                continue;
            }
            let i = ((y as u32 * width + x as u32) * 4) as usize;
            let edge = x < x0 + bw || x > x1 - bw || y < y0 + bw || y > y1 - bw;
            if edge {
                buf[i..i + 4].copy_from_slice(&EDGE);
            } else {
                // Blend interior with background at 35% -> translucent ghost
                for c in 0..3 {
                    buf[i + c] = ((buf[i + c] as u32 * 65 + FILL[c] as u32 * 35) / 100) as u8;
                }
            }
        }
    }
    Ok(())
}

/// RGBA pixels -> PNG bytes.
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

/// One-shot: world → PNG.
pub fn screenshot_png(
    world: &World,
    width: u32,
    height: u32,
    assets: &Assets,
    tick: u64,
) -> Result<Vec<u8>, String> {
    let rgba = render_world(world, width, height, assets, tick)?;
    encode_png(&rgba, width, height)
}

/// Semantic observation: translate "what is on the screen" into a structured description an LLM
/// can read precisely.
///
/// This is the agent's **primary observation channel** — more precise than letting the model look
/// at pixels: coordinates are exact numbers, directions are 9-region words, occlusion is explicit
/// entity pairs, and off-screen things carry direction and distance. Screenshots (screenshot)
/// become a fallback verification.
///
/// Convenience entry without an asset store: equivalent to
/// `describe_world_with_assets(.., &Assets::empty())`. The structured fields are asset-independent;
/// the only difference is the fidelity of text-contrast measurement — with an empty store, image
/// sprites degrade to `Sprite.color` solid-color blocks approximating the background luminance (see
/// [`describe_world_with_assets`]).
pub fn describe_world(world: &World, width: u32, height: u32) -> Result<serde_json::Value, String> {
    describe_world_with_assets(world, width, height, &Assets::empty())
}

/// The full-featured version of [`describe_world`]: takes an asset store so text-contrast
/// measurement renders with the real images.
///
/// Readability warning (eyes of AI developers): for each piece of text on screen, render one frame
/// with **this one text omitted** (lenient image mode; missing images degrade to solid-color
/// approximation), take the average background relative luminance L_bg within the text bounding
/// box, compute the WCAG-style contrast `(max+0.05)/(min+0.05)` against the relative luminance L_fg
/// of `Text.color`; if below [`TEXT_CONTRAST_MIN`] emit a `warnings[]` entry
/// (kind=`low-contrast-text`) and append a ⚠ line to the Chinese summary. Real-world incident
/// prototype: beige text on a beige card — the agent that built it "could not see it" and so never
/// noticed that human eyes could not read it.
///
/// Known approximations (this is a lint, not color science):
/// - The text color is the raw value, the background is the lit pixel — when lighting/bloom are on
///   the text is actually lit too, so the ratio has bias; the threshold is relaxed to 2.5 to absorb
///   this kind of bias into the margin;
/// - The bounding box is estimated from un-rendered layout geometry (bitmap = monospace cell,
///   vector = layout total width × glyph height);
/// - Only text whose center is on-screen is measured (same standard describe uses for "off-screen");
///   off-screen text is neither rendered nor measured;
/// - The measurement render uses the non-shaken camera (describe semantics are non-shaken by definition).
///
/// Cost: each on-screen text costs one extra CPU frame at describe resolution; no text in the scene
/// = zero extra overhead.
pub fn describe_world_with_assets(
    world: &World,
    width: u32,
    height: u32,
    assets: &Assets,
) -> Result<serde_json::Value, String> {
    use serde_json::json;

    if width == 0 || height == 0 {
        return Err(format!("分辨率 {width}x{height} 不合法"));
    }
    // Semantic observation uses the non-shaken camera: coordinates the agent asserts should not be
    // shaken by a few frames of visual jitter
    let (cam_x, cam_y, scale) = camera_base(world, height)?;
    let half_w_units = width as f64 / scale / 2.0;
    let half_h_units = height as f64 / scale / 2.0;

    // Focal point (the "self" of egocentric relations): the entity named by Camera.follow. When
    // absent, relative_to_focal is not output and distance sorting is not applied (degrades back to
    // slot order, backward compatible).
    let focal = focal_of(world);
    let focal_id = focal.map(|(id, _)| id);

    // Collect with sort keys: (named-order, distance-to-focal, id). Without a focal point the
    // distance is always 0 and the original order is the fallback. The intent of primary/secondary
    // sorting: named entities (the gameplay subjects) come first, then nearer-to-focal comes first
    // — let the model read the things most relevant to "me" first.
    struct DescribeRow {
        named: bool,
        dist: f64,
        id: vitric_ecs::EntityId,
        value: serde_json::Value,
    }
    let mut visible: Vec<DescribeRow> = Vec::new();
    let mut offscreen: Vec<DescribeRow> = Vec::new();
    let mut rects: Vec<(String, f64, f64, f64, f64)> = Vec::new(); // (id, x, y, w, h) world coordinates

    for id in world.query(&["Position", "Sprite"]) {
        // Defensive dormant check (same as render_with): describe should never surface dormant
        // entities. world.query already filters, but the local guard documents the invariant.
        if !is_renderable(world, id) { continue; }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        let color = world
            .get_field(id, "Sprite.color")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "#ffffff".to_string());
        let image = world
            .get_field(id, "Sprite.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let name = world.name_of(id).map(String::from);
        let rot = rot_of(world, id)?;

        let dx = px - cam_x;
        let dy = py - cam_y;
        let on_screen = dx.abs() - sw / 2.0 < half_w_units && dy.abs() - sh / 2.0 < half_h_units;

        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), json!(id.to_string()));
        if let Some(n) = &name {
            entry.insert("name".into(), json!(n));
        }
        entry.insert("world".into(), json!({"x": px, "y": py}));
        let mut sprite = json!({"w": sw, "h": sh, "color": color});
        if !image.is_empty() {
            sprite["image"] = json!(image);
        }
        if rot != 0.0 {
            // Rotation angle goes into semantic observation (default 0 is not output — same as the
            // picture behavior; absent means absent)
            sprite["rot"] = json!(rot);
        }
        entry.insert("sprite".into(), sprite);

        // Egocentric relations: direction/distance/same-row-same-column/adjacency/occlusion
        // relative to the focal point. The focal point itself does not output this block (no relation
        // to itself); without a focal point the whole block is absent (backward compatible). Reuses
        // the ecs world-perception operator relate_in_world — same source, same value as SceneView,
        // brought in once with `blocked` included (whether the line of sight is blocked by a third
        // Solid). dist_to_focal doubles as the primary/secondary sort key (0 when no focal point).
        let mut dist_to_focal = 0.0;
        if let Some((fid, _fplace)) = focal {
            if fid != id {
                let rel = relate_in_world(world, fid, id);
                dist_to_focal = rel.distance;
                entry.insert("relative_to_focal".into(), rel.to_json());
            }
        }

        if on_screen {
            let sx = width as f64 / 2.0 + dx * scale;
            let sy = height as f64 / 2.0 - dy * scale;
            entry.insert("screen_px".into(), json!({"x": sx.round(), "y": sy.round()}));
            entry.insert(
                "region".into(),
                json!(region_word(sx / width as f64, sy / height as f64)),
            );
            rects.push((id.to_string(), px, py, sw, sh));
            visible.push(DescribeRow {
                named: name.is_some(),
                dist: dist_to_focal,
                id,
                value: serde_json::Value::Object(entry),
            });
        } else {
            let direction = direction_word(dx, dy);
            entry.insert("direction".into(), json!(direction));
            entry.insert(
                "distance_units".into(),
                json!((dx.powi(2) + dy.powi(2)).sqrt().round()),
            );
            offscreen.push(DescribeRow {
                named: name.is_some(),
                dist: dist_to_focal,
                id,
                value: serde_json::Value::Object(entry),
            });
        }
    }

    // Primary/secondary sort (only enabled when there is a focal point — without one the sort key
    // is meaningless, slot order is kept for backward compatibility): named first, then ascending
    // by distance-to-focal, ties broken by id — deterministic key → deterministic result per frame.
    if focal_id.is_some() {
        let sort_rows = |rows: &mut Vec<DescribeRow>| {
            rows.sort_by(|a, b| {
                b.named
                    .cmp(&a.named) // named=true first (true > false, reversed)
                    .then(a.dist.total_cmp(&b.dist))
                    .then(a.id.cmp(&b.id))
            });
        };
        sort_rows(&mut visible);
        sort_rows(&mut offscreen);
    }
    let visible: Vec<serde_json::Value> = visible.into_iter().map(|r| r.value).collect();
    let offscreen: Vec<serde_json::Value> = offscreen.into_iter().map(|r| r.value).collect();

    // On-screen text: the content itself is the semantics, the agent does not OCR the screenshot
    let mut texts = Vec::new();
    // Contrast-measurement candidates: only collect text whose center is on-screen with a positive
    // font size (off-screen / undrawable text is neither rendered nor measured)
    struct ContrastCandidate {
        id: vitric_ecs::EntityId,
        content: String,
        color: String,
        size: f64,
        /// Screen-pixel coordinates (same formula as the draw path).
        cx: f64,
        cy: f64,
    }
    let mut candidates: Vec<ContrastCandidate> = Vec::new();
    for id in world.query(&["Position", "Text"]) {
        let content = world
            .get_field(id, "Text.content")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let screen_anchored = world
            .get_field(id, "Text.screen")
            .ok()
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (dx, dy) = if screen_anchored { (px, py) } else { (px - cam_x, py - cam_y) };
        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), json!(id.to_string()));
        if let Some(n) = world.name_of(id) {
            entry.insert("name".into(), json!(n));
        }
        entry.insert("content".into(), json!(content));
        entry.insert("world".into(), json!({"x": px, "y": py}));
        if dx.abs() < half_w_units && dy.abs() < half_h_units {
            let sx = width as f64 / 2.0 + dx * scale;
            let sy = height as f64 / 2.0 - dy * scale;
            entry.insert("region".into(), json!(region_word(sx / width as f64, sy / height as f64)));
            // Text with missing / non-positive size cannot be drawn (render errors/skips it); it has
            // no "background" to speak of, so it is excluded from contrast measurement
            let size = world.get_field(id, "Text.size").ok().and_then(Value::as_f64);
            if let Some(size) = size.filter(|s| *s > 0.0) {
                candidates.push(ContrastCandidate {
                    id,
                    content: content.clone(),
                    color: world
                        .get_field(id, "Text.color")
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_else(|| "#ffffff".to_string()),
                    size,
                    cx: sx,
                    cy: sy,
                });
            }
        } else {
            entry.insert("region".into(), json!("视野外"));
        }
        texts.push(serde_json::Value::Object(entry));
    }

    // Text readability check (see the function doc): only render a frame when there is text on
    // screen, otherwise zero extra cost
    let mut warnings: Vec<Value> = Vec::new();
    let mut warning_lines: Vec<String> = Vec::new();
    for c in &candidates {
        // Render one frame with this one text omitted (the rest of the text is still drawn — when
        // text overlaps text the lower text also counts as background). The camera is describe's
        // own non-shaken camera; lenient image mode see RenderOpts. tick is fixed at 0: describe has
        // no notion of time, particles in the measurement frame expand at tick 0 (stream has almost
        // no particles at tick 0) — contrast is a lint, not color science; known approximation
        let frame = render_with(
            world,
            width,
            height,
            assets,
            (cam_x, cam_y, scale),
            0,
            &RenderOpts { skip_text: Some(c.id), lenient_images: true },
        )?;
        // Bounding box estimated from draw geometry (mirrors the two paths of draw_texts), clipped
        // to the screen
        let (half_w, half_h) = match assets.font() {
            Some(font) => {
                let px_size = FontStore::px_size(c.size, scale);
                let total_w = font.layout_cached(&c.content, px_size).1;
                (total_w as f64 / 2.0, px_size as f64 / 2.0)
            }
            None => {
                let n = c.content.chars().count() as f64;
                (n * c.size * scale / 2.0, c.size * scale / 2.0)
            }
        };
        let x0 = (c.cx - half_w).floor().max(0.0) as i64;
        let x1 = (c.cx + half_w).ceil().min(width as f64) as i64;
        let y0 = (c.cy - half_h).floor().max(0.0) as i64;
        let y1 = (c.cy + half_h).ceil().min(height as f64) as i64;
        if x0 >= x1 || y0 >= y1 {
            continue; // Bounding box clipped to no pixels (extreme edge-hugging case), nothing to measure
        }
        let mut sum = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y as u32 * width + x as u32) * 4) as usize;
                sum += relative_luminance(&frame[i..i + 3]);
            }
        }
        let l_bg = sum / ((x1 - x0) * (y1 - y0)) as f64;
        let fg = parse_color(&c.color).map_err(|e| format!("实体 {} 的 Text.color: {e}", c.id))?;
        let l_fg = relative_luminance(&fg[..3]);
        let ratio = (l_bg.max(l_fg) + 0.05) / (l_bg.min(l_fg) + 0.05);
        if ratio < TEXT_CONTRAST_MIN {
            warnings.push(json!({
                "kind": "low-contrast-text",
                "entity": c.id.to_string(),
                "content": c.content,
                "ratio": (ratio * 100.0).round() / 100.0,
                "hint": "文字与底色亮度太接近,人眼难读;换文字色或挪到深/浅底上",
            }));
            warning_lines.push(format!(
                "⚠ 文字{:?}与底色对比度过低（{:.2}，下限 {TEXT_CONTRAST_MIN}）：人眼难读，换文字色或挪到深/浅底上",
                c.content, ratio,
            ));
        }
    }

    // Visual overlap (who is covering whom on the picture). Known approximation: intersection is
    // always judged by the **un-rotated-size** AABB — precise intersection of rotated sprites (SAT)
    // is not worth it for semantic observation: direction/coordinates/rot fields are enough for the
    // agent to locate things; edge false-positives/false-negatives are caught by the pixel screenshot
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

    // A Chinese summary for the LLM to read directly (condensed version of the structured fields)
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
    for t in &texts {
        lines.push(format!(
            "- 文字 {:?} 在{}（世界 {},{}）",
            t["content"].as_str().expect("content"),
            t["region"].as_str().expect("region"),
            t["world"]["x"], t["world"]["y"],
        ));
    }
    // Readability warnings immediately follow the text lines — when the agent reads the summary,
    // the warning is right next to the problematic text
    lines.extend(warning_lines);

    // Lighting settings: when on, let the agent see all the lights at the text level
    // (position/radius/color) instead of guessing from pixels
    let lighting = match ambient_of(world)? {
        None => None,
        Some((_, ambient_color)) => {
            let lights = collect_lights(world)?;
            lines.push(format!(
                "光照开启：环境色 {ambient_color}，{} 盏光源。",
                lights.len()
            ));
            // Shadow state is also textualized: when on, report the occluder count; the agent does
            // not have to count pixels to guess "why didn't a shadow come out"
            let shadows = shadows_of(world)?;
            let occluder_count = if shadows { collect_occluders(world)?.len() } else { 0 };
            if shadows {
                lines.push(format!(
                    "投影开启：{occluder_count} 个遮光体（Solid+Position+Collider，平行光不投影）。"
                ));
            }
            let lights_json: Vec<Value> = lights
                .iter()
                .map(|l| {
                    let mut entry = serde_json::Map::new();
                    entry.insert("id".into(), json!(l.id.to_string()));
                    if let Some(n) = &l.name {
                        entry.insert("name".into(), json!(n));
                    }
                    entry.insert("kind".into(), json!(l.kind.name()));
                    // Directional lights have no position/radius (placeholder 0 is not a real value;
                    // omit it so as not to mislead the agent)
                    if !matches!(l.kind, LightKind::Directional { .. }) {
                        entry.insert("world".into(), json!({"x": l.x, "y": l.y}));
                        entry.insert("radius".into(), json!(l.radius));
                    }
                    match l.kind {
                        LightKind::Point => {}
                        LightKind::Spot { angle, dir } => {
                            entry.insert("angle".into(), json!(angle));
                            entry.insert("dir".into(), json!(dir));
                        }
                        LightKind::Directional { dir } => {
                            entry.insert("dir".into(), json!(dir));
                        }
                    }
                    entry.insert("intensity".into(), json!(l.intensity));
                    entry.insert("color".into(), json!(l.color));
                    Value::Object(entry)
                })
                .collect();
            Some((ambient_color, lights_json, shadows, occluder_count))
        }
    };

    // Bloom settings: when on, textualize the parameters — the agent reads describe to see how the
    // post effect is configured, no pixel-guessing needed
    let bloom = bloom_of(world)?;
    if let Some(b) = &bloom {
        lines.push(format!(
            "泛光开启：threshold {}，strength {}。",
            b.threshold, b.strength
        ));
    }

    // Particle emitters: summarize one line per emitter (particles are not listed individually —
    // they are pure-function-expanded picture decoration, not observable world state). describe has
    // no notion of time: stream gives a steady-state visible-count estimate, burst gives the raw
    // trigger field; the agent cross-references the current tick itself
    let emitters = collect_emitters(world)?;
    let mut emitters_json: Vec<Value> = Vec::new();
    for em in &emitters {
        let label = em.name.clone().unwrap_or_else(|| em.id.to_string());
        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), json!(em.id.to_string()));
        if let Some(n) = &em.name {
            entry.insert("name".into(), json!(n));
        }
        entry.insert("kind".into(), json!(em.kind.name()));
        entry.insert("active".into(), json!(em.active));
        entry.insert("world".into(), json!({"x": em.x, "y": em.y}));
        entry.insert("lifetime".into(), json!(em.lifetime));
        entry.insert("color".into(), json!(em.color));
        match em.kind {
            EmitterKind::Stream { rate } => {
                let steady =
                    (rate * em.lifetime as f64 / PARTICLE_TICKS_PER_SECOND).ceil() as i64;
                entry.insert("rate".into(), json!(rate));
                entry.insert("visible_estimate".into(), json!(steady));
                lines.push(if em.active {
                    format!("- 发射器 {label}: stream 活跃，~{steady} 粒子可见（世界 {},{}）", em.x, em.y)
                } else {
                    format!("- 发射器 {label}: stream 关闭（active=false）")
                });
            }
            EmitterKind::Burst { count, burst } => {
                entry.insert("count".into(), json!(count));
                entry.insert("burst".into(), json!(burst));
                let state = if !em.active {
                    "关闭（active=false）".to_string()
                } else if burst < 0 {
                    "未触发".to_string()
                } else {
                    format!("触发@tick {burst}")
                };
                lines.push(format!(
                    "- 发射器 {label}: burst {state}（count {count}，寿命 {} tick）",
                    em.lifetime
                ));
            }
        }
        emitters_json.push(Value::Object(entry));
    }

    let mut out = json!({
        "camera": {"x": cam_x, "y": cam_y, "scale": scale},
        "viewport": {"width": width, "height": height},
        "visible": visible,
        "offscreen": offscreen,
        "texts": texts,
        "overlaps": overlaps,
        "text": lines.join("\n"),
    });
    if let Some((ambient_color, lights_json, shadows, occluder_count)) = lighting {
        out["ambient"] = json!({"color": ambient_color});
        out["lights"] = json!(lights_json);
        // When shadows are off these two keys do not appear — "no key = not enabled", same
        // convention as bloom/warnings
        if shadows {
            out["shadows"] = json!(true);
            out["occluders"] = json!(occluder_count);
        }
    }
    if let Some(b) = &bloom {
        out["bloom"] = json!({"threshold": b.threshold, "strength": b.strength});
    }
    // No emitters = no emitters key — same "no key = none" convention as bloom/warnings
    if !emitters_json.is_empty() {
        out["emitters"] = json!(emitters_json);
    }
    // No warnings = no warnings key — "no key = no problem found", the agent does not scan empty arrays
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    // Focal-centered ASCII grid map: only when there is a focal point is the top-level ascii_map
    // added (no follow = not added = backward compatible). Same source as SceneView (both call
    // ecs::ascii_map, default radius / auto-derived cell) — gives the model a rough navigation map
    // of "who is in which direction from me, how many cells away, and whether there is a wall (#) in between".
    if let Some((fid, _)) = focal {
        out["ascii_map"] = ascii_map(world, fid, &AsciiMapOpts::default()).to_json();
    }
    Ok(out)
}

/// WCAG relative luminance (input is the first 3 channels of an sRGB byte): first inverse-gamma
/// linearize, then weight `L = 0.2126R + 0.7152G + 0.0722B`. Contrast ratio = `(L1+0.05)/(L2+0.05)`
/// (brighter over darker).
fn relative_luminance(rgb: &[u8]) -> f64 {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// Screen 9-region direction word (input is 0..1 screen-ratio coordinates).
fn region_word(fx: f64, fy: f64) -> &'static str {
    let col = if fx < 1.0 / 3.0 { 0 } else if fx < 2.0 / 3.0 { 1 } else { 2 };
    let row = if fy < 1.0 / 3.0 { 0 } else if fy < 2.0 / 3.0 { 1 } else { 2 };
    match (row, col) {
        (0, 0) => "左上", (0, 1) => "上方", (0, 2) => "右上",
        (1, 0) => "左侧", (1, 1) => "中心", (1, 2) => "右侧",
        (2, 0) => "左下", (2, 1) => "下方", _ => "右下",
    }
}

/// Off-screen direction word (world coordinate system, y up).
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

/// Camera body (no shake offset): take the first Camera entity; if none, origin / 8 pixels per unit.
/// When optional `view_h` (vertical visible world height, in units) > 0, pixel density is back-derived
/// from the viewport height — content's on-screen ratio is resolution-independent: 4K and 720p see
/// the same-sized world; otherwise `scale` (pixels per unit) is used.
fn camera_base(world: &World, viewport_h: u32) -> Result<(f64, f64, f64), String> {
    let cams = world.query(&["Camera"]);
    match cams.first() {
        None => Ok((0.0, 0.0, 8.0)),
        Some(&id) => {
            let x = num(world, id, "Camera.x")?;
            let y = num(world, id, "Camera.y")?;
            let view_h = world
                .get_field(id, "Camera.view_h")
                .ok()
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if view_h > 0.0 {
                return Ok((x, y, viewport_h as f64 / view_h));
            }
            let scale = num(world, id, "Camera.scale")?;
            if scale <= 0.0 {
                return Err(format!("实体 {id} 的 Camera.scale 必须 > 0，拿到 {scale}"));
            }
            Ok((x, y, scale))
        }
    }
}

/// Focal entity (the "self" of egocentric relations): the entity named by the first `Camera`
/// entity's `follow` field. The convention matches vitric-sim's camera follow — `follow` is an
/// entity name (text); default/empty string = no follow = no focal point (backward compatible:
/// without a focal point describe does not output relative_to_focal).
///
/// Returns the focal point's (id, world placement). The placement's w/h come from `Sprite.w`/
/// `Sprite.h` (default 0 if absent — adjacency degrades to strict center coincidence, which is
/// safer than fabricating a size).
/// Returns `None` when `follow` points to a non-existent entity (semantic observation should not
/// fail an entire frame due to a configuration typo — render/sim will surface that error; describe
/// just omits this block).
fn focal_of(world: &World) -> Option<(vitric_ecs::EntityId, Placement)> {
    let cam = *world.query(&["Camera"]).first()?;
    let name = world.get_field(cam, "Camera.follow").ok()?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    let id = world.entity(&name).ok()?;
    let x = num(world, id, "Position.x").ok()?;
    let y = num(world, id, "Position.y").ok()?;
    // Size is optional: a focal point without a Sprite (e.g. a pure logic anchor) gets w/h = 0
    let w = world.get_field(id, "Sprite.w").ok().and_then(Value::as_f64).unwrap_or(0.0);
    let h = world.get_field(id, "Sprite.h").ok().and_then(Value::as_f64).unwrap_or(0.0);
    Some((id, Placement::new(x, y, w, h)))
}

/// Render camera: body + the shake offset from the `Shake` component on the camera entity. Both
/// the CPU rasterizer and the GPU path take the camera from here — the two paths shake bit-identically.
pub fn camera_of(world: &World, tick: u64, viewport_h: u32) -> Result<(f64, f64, f64), String> {
    let (mut x, mut y, scale) = camera_base(world, viewport_h)?;
    if let Some(&id) = world.query(&["Camera"]).first() {
        if world.has_component(id, "Shake") {
            let amplitude = num(world, id, "Shake.amplitude")?;
            let (dx, dy) = shake_offset(tick, amplitude);
            x += dx;
            y += dy;
        }
    }
    Ok((x, y, scale))
}

/// Screen shake offset (world units): a pure function of (tick, amplitude), completely independent
/// of the simulation's RNG stream — shake never perturbs the deterministic gameplay trajectory,
/// and the snapshot has no extra state to store.
/// Implementation: SplitMix64 spreads the tick into 64 bits; the high/low 32 bits each map to
/// [-1, 1] on the two axes, then multiplied by the amplitude.
pub fn shake_offset(tick: u64, amplitude: f64) -> (f64, f64) {
    if amplitude <= 0.0 {
        return (0.0, 0.0);
    }
    let z = mix64(tick); // Same SplitMix64 (shared with particle hashing); the operation sequence is bit-identical to before the refactor
    let nx = ((z >> 32) as u32 as f64) / (u32::MAX as f64) * 2.0 - 1.0;
    let ny = (z as u32 as f64) / (u32::MAX as f64) * 2.0 - 1.0;
    (nx * amplitude, ny * amplitude)
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

/// Color parsing (with optional alpha): `#rrggbb` (opaque) or `#rrggbbaa` (with transparency).
/// UI Panel backgrounds often need a semi-transparent mask, so this separate path supports 8-digit
/// hex; world sprites/text still go through [`parse_color`] (only 6 digits, alpha always 255 — the
/// byte-locked old behavior is unchanged).
fn parse_color_a(s: &str) -> Result<[u8; 4], String> {
    let hex = s.strip_prefix('#').ok_or_else(|| {
        format!("颜色 {s:?} 格式不对。写法: \"#rrggbb\" 或带透明度 \"#rrggbbaa\"")
    })?;
    if (hex.len() != 6 && hex.len() != 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 {s:?} 必须是 6 位 \"#rrggbb\" 或 8 位 \"#rrggbbaa\" 十六进制"));
    }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("已校验十六进制");
    let a = if hex.len() == 8 { p(6) } else { 255 };
    Ok([p(0), p(2), p(4), a])
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
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "中心是红色精灵");
        assert_eq!(pixel(&buf, 64, 2, 2), [24, 26, 33, 255], "角落是背景");
    }

    #[test]
    fn camera_moves_the_view() {
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        // Camera moves right 2 units → the sprite moves left 2*8=16 pixels on screen
        w.set_component(cam, "Camera", json!({"x": 2.0, "y": 0.0, "scale": 8.0})).unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32 - 16, 32), [255, 0, 0, 255]);
        assert_eq!(pixel(&buf, 64, 32 + 8, 32), [24, 26, 33, 255]);
    }

    #[test]
    fn text_renders_glyph_pixels_and_describe_reads_content() {
        let mut w = World::new();
        let e = w.spawn_named("score").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        // "I" single character, 4 units → 32x32 pixels, centered
        w.set_component(e, "Text", json!({"content": "I", "size": 4.0, "color": "#00ff00"}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        // The vertical stroke of "I" is in columns 2-3 of the glyph (the 8x8 bitmap glyph is
        // left-biased); the sample lands on the stroke
        assert_eq!(pixel(&buf, 64, 25, 32), [0, 255, 0, 255], "竖干处应是字形像素");
        assert_eq!(pixel(&buf, 64, 2, 2), [24, 26, 33, 255]);
        // Same world → same bytes (text is also deterministic)
        assert_eq!(buf, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());

        let d = describe_world(&w, 64, 64).unwrap();
        let texts = d["texts"].as_array().unwrap();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0]["content"], json!("I"));
        assert_eq!(texts[0]["region"], json!("中心"));
        assert!(d["text"].as_str().unwrap().contains("文字 \"I\""), "{}", d["text"]);
    }

    #[test]
    fn empty_text_is_skipped() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Text", json!({"content": "", "size": 4.0, "color": "#00ff00"}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [24, 26, 33, 255], "空文本不画");
        assert_eq!(describe_world(&w, 64, 64).unwrap()["texts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn same_world_same_bytes() {
        let w = world_one_red_sprite();
        assert_eq!(render_world(&w, 128, 96, &Assets::empty(), 0).unwrap(), render_world(&w, 128, 96, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn png_has_magic_and_decodes_back() {
        let w = world_one_red_sprite();
        let data = screenshot_png(&w, 32, 32, &Assets::empty(), 0).unwrap();
        assert_eq!(&data[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], "PNG 魔数");
        let decoder = png::Decoder::new(std::io::Cursor::new(&data[..]));
        let mut reader = decoder.read_info().unwrap();
        let mut out = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut out).unwrap();
        assert_eq!((info.width, info.height), (32, 32));
    }

    #[test]
    fn image_sprite_blits_with_alpha() {
        // 2x2 image: left half red opaque, right half fully transparent
        let dir = std::env::temp_dir().join(format!("vitric-blit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, /**/ 0, 0, 0, 0,
            255, 0, 0, 255, /**/ 0, 0, 0, 0,
        ];
        {
            let file = std::fs::File::create(dir.join("half.png")).unwrap();
            let mut enc = png::Encoder::new(file, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&pixels).unwrap();
        }
        let assets = Assets::load_dir(&dir).unwrap();

        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            e,
            "Sprite",
            json!({"w": 4.0, "h": 4.0, "color": "#ffffff", "image": "half.png"}),
        )
        .unwrap();
        // Default camera scale=8: the sprite occupies the central 32x32 pixels of the screen
        let buf = render_world(&w, 64, 64, &assets, 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32 - 8, 32), [255, 0, 0, 255], "左半是贴图红");
        assert_eq!(pixel(&buf, 64, 32 + 8, 32), [24, 26, 33, 255], "右半透明 → 透出背景");

        // Referencing a non-existent image: errors and lists the existing assets
        w.set_field(e, "Sprite.image", json!("ghost.png")).unwrap();
        let err = render_world(&w, 64, 64, &assets, 0).unwrap_err();
        assert!(err.contains("half.png"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn describe_gives_semantic_view() {
        let mut w = World::new();
        let p = w.spawn_named("player").unwrap();
        w.set_component(p, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(p, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        // A coin overlapping the player
        let c = w.spawn_named("coin").unwrap();
        w.set_component(c, "Position", json!({"x": 0.5, "y": 0.0})).unwrap();
        w.set_component(c, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ffd84d"})).unwrap();
        // One far off-screen to the left
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
        // Player and coin visually overlap — must be flagged
        let overlaps = d["overlaps"].as_array().unwrap();
        assert_eq!(overlaps.len(), 1, "{overlaps:?}");
        // The summary is directly readable
        let text = d["text"].as_str().unwrap();
        assert!(text.contains("player") && text.contains("中心") && text.contains("视野外"), "{text}");
    }

    #[test]
    fn pick_topmost_and_miss() {
        let mut w = World::new();
        let below = w.spawn_named("below").unwrap();
        w.set_component(below, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(below, "Sprite", json!({"w": 4.0, "h": 4.0, "color": "#ff0000"})).unwrap();
        let above = w.spawn_named("above").unwrap();
        w.set_component(above, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(above, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#00ff00"})).unwrap();
        // Screen center: both cover it; the later-drawn `above` is hit
        assert_eq!(pick(&w, 64, 64, 32.0, 32.0).unwrap(), Some(above));
        // Slightly offset: only the larger `below` covers it (above half-width = 1 unit = 8px)
        assert_eq!(pick(&w, 64, 64, 32.0 + 12.0, 32.0).unwrap(), Some(below));
        // Empty land
        assert_eq!(pick(&w, 64, 64, 2.0, 2.0).unwrap(), None);
        // Coordinate round-trip
        let (wx, wy) = screen_to_world(&w, 64, 64, 32.0 + 8.0, 32.0 - 16.0).unwrap();
        assert!((wx - 1.0).abs() < 1e-9 && (wy - 2.0).abs() < 1e-9, "{wx},{wy}");
    }

    #[test]
    fn pick_world_same_verdict_as_screen_pick() {
        let mut w = World::new();
        let e = w.spawn_named("card").unwrap();
        w.set_component(e, "Position", json!({"x": 3.0, "y": 2.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#00ff00"})).unwrap();
        // Direct hit / empty land in world coordinates
        assert_eq!(pick_world(&w, 3.0, 2.0).unwrap(), Some(e));
        assert_eq!(pick_world(&w, 3.9, 1.1).unwrap(), Some(e), "边内要命中");
        assert_eq!(pick_world(&w, 5.5, 2.0).unwrap(), None, "边外是空地");
        // Same judgment as the screen-coordinate version: screen point → screen_to_world →
        // pick_world closed loop
        let (wx, wy) = screen_to_world(&w, 64, 64, 32.0 + 24.0, 32.0 - 16.0).unwrap();
        assert_eq!(pick(&w, 64, 64, 32.0 + 24.0, 32.0 - 16.0).unwrap(), pick_world(&w, wx, wy).unwrap());
        assert_eq!(pick_world(&w, wx, wy).unwrap(), Some(e));
    }

    #[test]
    fn selection_outline_draws_border() {
        let w_ = {
            let mut w = World::new();
            let e = w.spawn();
            w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
            w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
            (w, e)
        };
        let (w, e) = w_;
        let mut buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        draw_selection_outline(&mut buf, &w, 64, 64, e, 0).unwrap();
        // Sprite half-width 8px + 2px outset → outline at x=32±10
        assert_eq!(pixel(&buf, 64, 32 - 10, 32), [39, 192, 168, 255], "左描边");
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "精灵本体不被盖");
    }

    /// Rotation-test asset: halves.png (2x1, left red right blue — an asymmetric pattern is needed
    /// to see whether rotation is correct). Returns (asset store, temp dir); the caller deletes the
    /// dir when done.
    fn assets_with_halves(tag: &str) -> (Assets, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vitric-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pixels: Vec<u8> = vec![255, 0, 0, 255, /**/ 0, 0, 255, 255];
        {
            let file = std::fs::File::create(dir.join("halves.png")).unwrap();
            let mut enc = png::Encoder::new(file, 2, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&pixels).unwrap();
        }
        (Assets::load_dir(&dir).unwrap(), dir)
    }

    /// A 4x2 halves-image sprite at the origin, with optional rot.
    fn world_halves_sprite(rot: Option<f64>) -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut sprite = json!({"w": 4.0, "h": 2.0, "image": "halves.png"});
        if let Some(r) = rot {
            sprite["rot"] = json!(r);
        }
        w.set_component(e, "Sprite", sprite).unwrap();
        w
    }

    #[test]
    fn rot_zero_takes_fast_path_byte_identical() {
        // Explicit rot: 0 must be byte-identical to having no rot field at all (fast-path backward
        // compatibility locked down)
        let plain = render_world(&world_one_red_sprite(), 64, 64, &Assets::empty(), 0).unwrap();
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000", "rot": 0.0}))
            .unwrap();
        assert_eq!(plain, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());
        // Same for image sprites
        let (assets, dir) = assets_with_halves("rot0");
        let plain = render_world(&world_halves_sprite(None), 64, 64, &assets, 0).unwrap();
        let with_field = render_world(&world_halves_sprite(Some(0.0)), 64, 64, &assets, 0).unwrap();
        assert_eq!(plain, with_field);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rot_90_rotates_pixels_counter_clockwise() {
        // 4x2 left-red right-blue, rotated 90° counter-clockwise: the blue right half rotates to
        // the top of the picture, the red half to the bottom
        let (assets, dir) = assets_with_halves("rot90");
        let buf = render_world(&world_halves_sprite(Some(90.0)), 64, 64, &assets, 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 20), [0, 0, 255, 255], "上方是蓝（原来的右半边）");
        assert_eq!(pixel(&buf, 64, 32, 44), [255, 0, 0, 255], "下方是红（原来的左半边）");
        // The left/right wings of the un-rotated AABB are now empty — the rotated shape is a
        // vertical bar (occupying x 24..40, y 16..48)
        assert_eq!(pixel(&buf, 64, 20, 32), BACKGROUND, "左翼是背景");
        assert_eq!(pixel(&buf, 64, 44, 32), BACKGROUND, "右翼是背景");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rot_180_equals_flipping_both_axes() {
        // A centered sprite rotated 180° = the whole picture flipped on both axes at once
        // (pixel-by-pixel comparison)
        let (assets, dir) = assets_with_halves("rot180");
        let plain = render_world(&world_halves_sprite(None), 64, 64, &assets, 0).unwrap();
        let turned = render_world(&world_halves_sprite(Some(180.0)), 64, 64, &assets, 0).unwrap();
        for y in 0..64u32 {
            for x in 0..64u32 {
                assert_eq!(
                    pixel(&turned, 64, x, y),
                    pixel(&plain, 64, 63 - x, 63 - y),
                    "({x},{y}) 应等于未旋转帧的中心对称点"
                );
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rotated_render_is_deterministic() {
        // Arbitrary angle (trig path) same world same tick → bit-identical bytes
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.5, "y": -0.25})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 3.0, "h": 1.0, "color": "#00ff88", "rot": 37.0}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 5).unwrap();
        assert_eq!(buf, render_world(&w, 64, 64, &Assets::empty(), 5).unwrap());
    }

    #[test]
    fn pick_respects_rotation() {
        let mut w = World::new();
        let e = w.spawn_named("bar").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "color": "#ff0000", "rot": 90.0}))
            .unwrap();
        // Screen (32,18) = world (0, 1.75): outside the un-rotated AABB (h=2), inside the rotated
        // vertical bar → hit
        assert_eq!(pick(&w, 64, 64, 32.0, 18.0).unwrap(), Some(e), "旋转后的形状内要命中");
        // Screen (46,32) = world (1.75, 0): inside the un-rotated AABB (w=4), but after rotation
        // already empty land → no hit
        assert_eq!(pick(&w, 64, 64, 46.0, 32.0).unwrap(), None, "转走了的区域不该命中");
        // Control group: with rot reset to 0 the two verdicts are exactly reversed
        w.set_field(e, "Sprite.rot", json!(0.0)).unwrap();
        assert_eq!(pick(&w, 64, 64, 32.0, 18.0).unwrap(), None);
        assert_eq!(pick(&w, 64, 64, 46.0, 32.0).unwrap(), Some(e));
    }

    #[test]
    fn describe_includes_rot_when_nonzero() {
        let mut w = World::new();
        let e = w.spawn_named("tilted").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000", "rot": 45.0}))
            .unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["visible"][0]["sprite"]["rot"], json!(45.0));
        // Non-rotated sprites do not carry a rot field
        let d0 = describe_world(&world_one_red_sprite(), 64, 64).unwrap();
        assert!(d0["visible"][0]["sprite"].get("rot").is_none());
    }

    #[test]
    fn selection_outline_uses_rotated_bbox() {
        // 4x2 rotated 90° → the rotated bounding box is about 2x4 (world units): the outline hugs
        // the vertical bar, not the original horizontal bar
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "color": "#ff0000", "rot": 90.0}))
            .unwrap();
        let mut buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        draw_selection_outline(&mut buf, &w, 64, 64, e, 0).unwrap();
        // After rotation half-width 1 unit = 8px + 2px outset → left outline at x≈22 (2px thick;
        // take the inner column to avoid floating-point boundaries)
        assert_eq!(pixel(&buf, 64, 22, 32), [39, 192, 168, 255], "左描边贴竖条");
        // The outline position for the un-rotated size (32-18=14) should be background — proves the
        // rotated bounding box is what's used
        assert_eq!(pixel(&buf, 64, 14, 32), BACKGROUND, "老位置不该有描边");
        // Top outline: after rotation half-height 2 units = 16px + 2px → y=14 for two rows
        assert_eq!(pixel(&buf, 64, 32, 15), [39, 192, 168, 255], "上描边随包围盒抬高");
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "精灵本体不被盖");
    }

    #[test]
    fn shake_offset_is_pure_function_of_tick_and_amplitude() {
        // Same (tick, amplitude) → same offset (pure function, no hidden state)
        assert_eq!(shake_offset(7, 0.5), shake_offset(7, 0.5));
        // Different tick → offset changes (otherwise shake is frozen)
        assert_ne!(shake_offset(7, 0.5), shake_offset(8, 0.5));
        // Offset per axis does not exceed amplitude; amplitude=0 → zero offset
        let (dx, dy) = shake_offset(123, 0.5);
        assert!(dx.abs() <= 0.5 && dy.abs() <= 0.5, "({dx},{dy})");
        assert_eq!(shake_offset(123, 0.0), (0.0, 0.0));
    }

    #[test]
    fn view_h_makes_zoom_resolution_independent() {
        // view_h=8: regardless of viewport pixel count, vertically you always see 8 world units —
        // the content's on-screen ratio is constant
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0, "view_h": 8.0}))
            .unwrap();
        assert_eq!(camera_of(&w, 0, 80).unwrap().2, 10.0, "80px/8单位=10像素每单位");
        assert_eq!(camera_of(&w, 0, 160).unwrap().2, 20.0, "分辨率翻倍像素密度翻倍");
        // A 2x2 sprite under view_h=8 always occupies 1/4 of the vertical, regardless of resolution
        for vh in [64u32, 128] {
            let buf = render_world(&w, vh, vh, &Assets::empty(), 0).unwrap();
            let bg = [24, 26, 33, 255];
            let top = (vh as f64 * (0.5 - 1.0 / 8.0)) as u32; // Sprite top edge
            assert_eq!(pixel(&buf, vh, vh / 2, top + 1), [255, 0, 0, 255]);
            assert_eq!(pixel(&buf, vh, vh / 2, top - 1), bg);
        }
    }

    #[test]
    fn camera_of_applies_shake_offset_deterministically() {
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 1.0, "y": 2.0, "scale": 8.0})).unwrap();
        w.set_component(cam, "Shake", json!({"amplitude": 0.5, "decay": 0.9})).unwrap();

        let shaken = camera_of(&w, 7, 64).unwrap();
        assert_eq!(shaken, camera_of(&w, 7, 64).unwrap(), "同世界同 tick 必须同取景");
        assert_ne!(shaken, camera_of(&w, 8, 64).unwrap(), "tick 变了偏移要变");
        let (dx, dy) = shake_offset(7, 0.5);
        assert_eq!(shaken, (1.0 + dx, 2.0 + dy, 8.0), "取景 = 相机本体 + shake_offset");

        // Rendering the whole frame is also deterministic: same tick → byte-identical; pixels
        // differ between shake ticks
        let f7 = render_world(&w, 64, 64, &Assets::empty(), 7).unwrap();
        assert_eq!(f7, render_world(&w, 64, 64, &Assets::empty(), 7).unwrap());
        assert_ne!(f7, render_world(&w, 64, 64, &Assets::empty(), 8).unwrap());

        // amplitude zero → offset vanishes, framing returns to the camera body
        w.set_field(cam, "Shake.amplitude", json!(0.0)).unwrap();
        assert_eq!(camera_of(&w, 7, 64).unwrap(), (1.0, 2.0, 8.0));
        // Semantic observation / picking always reads the non-shaken camera
        w.set_field(cam, "Shake.amplitude", json!(0.5)).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["camera"], json!({"x": 1.0, "y": 2.0, "scale": 8.0}));
    }

    #[test]
    fn lighting_brightens_near_light_and_is_deterministic() {
        // Dark ambient + one white light at the origin: pixels near the light are brighter than
        // far ones, and the output is byte-deterministic
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 4.0, "color": "#ffffff", "intensity": 1.0}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        let near = pixel(&buf, 64, 32, 32);
        let far = pixel(&buf, 64, 2, 2);
        // Light radius 4 units × scale 8 = 32px: the corner is outside the radius → ambient black
        // = all black
        assert_eq!(far, [0, 0, 0, 255], "半径外只剩环境光（纯黑）");
        assert!(near[0] > far[0] && near[2] > far[2], "近灯应更亮: {near:?} vs {far:?}");
        // Same world same tick → bit-identical bytes
        assert_eq!(buf, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn no_ambient_entity_skips_lighting_entirely() {
        // Lights without Ambient = lighting entirely off: renders the same bytes as a world with
        // no lights (backward compatible)
        let plain = render_world(&world_one_red_sprite(), 64, 64, &Assets::empty(), 0).unwrap();
        let mut w = world_one_red_sprite();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 1.0, "y": 1.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 4.0})).unwrap();
        assert_eq!(plain, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn lighting_formula_clamps_at_1_5_and_white_ambient_is_identity() {
        // Formula locked: lit = min(ambient + Σ contributions, 1.5), out = min(scene·lit, 1)
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        // #646464 = (100,100,100): overexposure cap 1.5 → 100*1.5 = 150, the value can be asserted
        // exactly
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#646464"})).unwrap();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#ffffff"})).unwrap();
        // White ambient (lit=1.0) with no lights = identity transform, bytes unchanged
        let lit = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&lit, 64, 32, 32), [100, 100, 100, 255], "白环境光不改像素");
        assert_eq!(pixel(&lit, 64, 2, 2), [24, 26, 33, 255], "背景也不变");
        // Add a strong light: white ambient 1.0 + large contribution → clamped to 1.5 → 100*1.5=150
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 100.0, "intensity": 10.0})).unwrap();
        let lit = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&lit, 64, 32, 32), [150, 150, 150, 255], "过曝夹在 1.5 倍");
    }

    #[test]
    fn light_cap_is_an_explicit_error() {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#202838"})).unwrap();
        for i in 0..65 {
            let l = w.spawn();
            w.set_component(l, "Position", json!({"x": i as f64, "y": 0.0})).unwrap();
            w.set_component(l, "Light", json!({"radius": 2.0})).unwrap();
        }
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("65") && err.contains("64"), "{err}");
        // Invalid radius must also be an explicit error
        let mut w2 = World::new();
        let a2 = w2.spawn();
        w2.set_component(a2, "Ambient", json!({"color": "#202838"})).unwrap();
        let l2 = w2.spawn();
        w2.set_component(l2, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w2.set_component(l2, "Light", json!({"radius": 0.0})).unwrap();
        let err = render_world(&w2, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Light.radius"), "{err}");
    }

    #[test]
    fn describe_includes_lights_and_ambient_when_active() {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#202838"})).unwrap();
        let lamp = w.spawn_named("torch").unwrap();
        w.set_component(lamp, "Position", json!({"x": 3.0, "y": -1.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 5.0, "color": "#ff8800", "intensity": 2.0}))
            .unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["ambient"], json!({"color": "#202838"}));
        let lights = d["lights"].as_array().unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0]["name"], json!("torch"));
        assert_eq!(lights[0]["world"], json!({"x": 3.0, "y": -1.0}));
        assert_eq!(lights[0]["radius"], json!(5.0));
        assert_eq!(lights[0]["intensity"], json!(2.0));
        assert_eq!(lights[0]["color"], json!("#ff8800"));
        let text = d["text"].as_str().unwrap();
        assert!(text.contains("光照开启") && text.contains("#202838") && text.contains("1 盏"), "{text}");
        // No Ambient: lighting fields do not appear in describe
        let d = describe_world(&World::new(), 64, 64).unwrap();
        assert!(d.get("ambient").is_none() && d.get("lights").is_none());
    }

    /// Dark ambient + one configurable-kind light (at the origin) — shared scaffold for spot /
    /// directional light tests.
    fn world_dark_with_light(light: Value) -> World {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", light).unwrap();
        w
    }

    #[test]
    fn point_light_with_explicit_kind_matches_no_kind_byte_for_byte() {
        // Explicit kind:"point" = the old point light without a kind field; output is byte-identical
        // (fast path unchanged)
        let implicit = render_world(
            &world_dark_with_light(json!({"radius": 4.0})),
            64,
            64,
            &Assets::empty(),
            0,
        )
        .unwrap();
        let explicit = render_world(
            &world_dark_with_light(json!({"radius": 4.0, "kind": "point"})),
            64,
            64,
            &Assets::empty(),
            0,
        )
        .unwrap();
        assert_eq!(implicit, explicit);
    }

    #[test]
    fn spot_light_lights_cone_and_rotates_with_dir() {
        // Light at the origin (pixel 32,32), radius 4 units = 32px, cone angle 90°, facing +x
        // (dir=0). Pixels (40,32) and (32,40) are at strictly equal distance from the light center
        // (dx/dy symmetric), differing only in direction: (40,32) is inside the +x cone → lit;
        // (32,40) is in the -y direction (Δθ≈87° > half-angle 45°) → outside the cone, all black
        let spot = |dir: f64| {
            json!({"radius": 4.0, "kind": "spot", "angle": 90.0, "dir": dir, "intensity": 1.0})
        };
        let buf = render_world(&world_dark_with_light(spot(0.0)), 64, 64, &Assets::empty(), 0)
            .unwrap();
        let inside = pixel(&buf, 64, 40, 32);
        let outside = pixel(&buf, 64, 32, 40);
        assert_eq!(outside, [0, 0, 0, 255], "锥外同距离像素只剩环境黑");
        assert!(inside[0] > 0 && inside[1] > 0 && inside[2] > 0, "锥内该被照亮: {inside:?}");
        // The cone rotates with dir: dir=90 (world +y = top of the picture) → the top is lit, and
        // the previously +x pixel falls outside the cone
        let buf = render_world(&world_dark_with_light(spot(90.0)), 64, 64, &Assets::empty(), 0)
            .unwrap();
        assert!(pixel(&buf, 64, 32, 24)[0] > 0, "dir=90 后画面上方在锥内");
        assert_eq!(pixel(&buf, 64, 40, 32), [0, 0, 0, 255], "+x 方向掉出锥外（Δθ=90° > 45°）");
        assert_eq!(pixel(&buf, 64, 32, 40), [0, 0, 0, 255], "画面下方仍是锥外");
        // Determinism: same world, same tick → bit-identical
        assert_eq!(
            buf,
            render_world(&world_dark_with_light(spot(90.0)), 64, 64, &Assets::empty(), 0).unwrap()
        );
    }

    #[test]
    fn light_kind_and_spot_fields_are_validated_explicitly() {
        let render = |light: Value| {
            render_world(&world_dark_with_light(light), 64, 64, &Assets::empty(), 0).unwrap_err()
        };
        // Unknown kind: error lists all legal values
        let err = render(json!({"radius": 4.0, "kind": "cone"}));
        assert!(
            err.contains("point") && err.contains("spot") && err.contains("directional"),
            "{err}"
        );
        // kind is not text
        let err = render(json!({"radius": 4.0, "kind": 1}));
        assert!(err.contains("Light.kind"), "{err}");
        // Spot light missing angle / missing dir: explicit error with usage hint
        let err = render(json!({"radius": 4.0, "kind": "spot", "dir": 0.0}));
        assert!(err.contains("angle"), "{err}");
        let err = render(json!({"radius": 4.0, "kind": "spot", "angle": 60.0}));
        assert!(err.contains("dir"), "{err}");
        // angle out of range (cone full-width 1..=360)
        for bad in [0.5, 361.0, -90.0] {
            let err = render(json!({"radius": 4.0, "kind": "spot", "angle": bad, "dir": 0.0}));
            assert!(err.contains("1..=360") && err.contains("Light.angle"), "{err}");
        }
        // point/spot must have Position (only directional is allowed to omit it)
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Light", json!({"radius": 4.0})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Position") && err.contains("directional"), "{err}");
        // Directional lights also count against the 64-light quota: 65 directional lights is an
        // explicit error
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        for _ in 0..65 {
            let l = w.spawn();
            w.set_component(l, "Light", json!({"kind": "directional", "dir": 270.0})).unwrap();
        }
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("65") && err.contains("64"), "{err}");
    }

    #[test]
    fn directional_light_brightens_uniformly_without_position() {
        // Directional lights do not need Position; contribution is everywhere = color·intensity, independent of distance to anything.
        // Dark ambient + white directional light intensity 0.5 → each pixel = original pixel × 0.5 (exactly assertable)
        let plain = render_world(&world_one_red_sprite(), 64, 64, &Assets::empty(), 0).unwrap();
        let mut w = world_one_red_sprite();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let sun = w.spawn(); // No Position attached — directional light is at infinity
        w.set_component(sun, "Light", json!({"kind": "directional", "dir": 270.0, "intensity": 0.5}))
            .unwrap();
        let lit = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        for i in 0..plain.len() {
            let expect =
                if i % 4 == 3 { 255 } else { (plain[i] as f64 * 0.5).min(255.0) as u8 };
            assert_eq!(lit[i], expect, "字节 {i}：平行光对全画面是同一个倍率");
        }
        // Control: only dark ambient (no directional light) = all black — the brightness difference comes entirely from the directional light
        let mut w2 = world_one_red_sprite();
        let amb2 = w2.spawn();
        w2.set_component(amb2, "Ambient", json!({"color": "#000000"})).unwrap();
        let dark = render_world(&w2, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&dark, 64, 32, 32), [0, 0, 0, 255]);
        assert_eq!(pixel(&dark, 64, 2, 2), [0, 0, 0, 255]);
    }

    #[test]
    fn describe_includes_light_kind_and_angles() {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#202838"})).unwrap();
        let torch = w.spawn_named("torch").unwrap();
        w.set_component(torch, "Position", json!({"x": 1.0, "y": 2.0})).unwrap();
        w.set_component(torch, "Light", json!({"radius": 5.0})).unwrap();
        let beam = w.spawn_named("beam").unwrap();
        w.set_component(beam, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            beam,
            "Light",
            json!({"radius": 8.0, "kind": "spot", "angle": 60.0, "dir": 90.0}),
        )
        .unwrap();
        let sun = w.spawn_named("sun").unwrap();
        w.set_component(sun, "Light", json!({"kind": "directional", "dir": 270.0})).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        let lights = d["lights"].as_array().unwrap();
        assert_eq!(lights.len(), 3);
        // Point light: kind is always present (old scenes without kind also report "point"), no angle/dir
        assert_eq!(lights[0]["kind"], json!("point"));
        assert!(lights[0].get("angle").is_none() && lights[0].get("dir").is_none());
        // Spot light: kind + angle + dir + world/radius
        assert_eq!(lights[1]["kind"], json!("spot"));
        assert_eq!(lights[1]["angle"], json!(60.0));
        assert_eq!(lights[1]["dir"], json!(90.0));
        assert_eq!(lights[1]["radius"], json!(8.0));
        // Directional light: kind + dir, no world/radius (placeholder 0 is not a real value, not emitted)
        assert_eq!(lights[2]["kind"], json!("directional"));
        assert_eq!(lights[2]["dir"], json!(270.0));
        assert!(lights[2].get("world").is_none() && lights[2].get("radius").is_none());
        assert!(d["text"].as_str().unwrap().contains("3 盏"), "{}", d["text"]);
    }

    /// Write a solid-color RGBA PNG (used as normal-map test asset).
    fn write_solid_png(path: &std::path::Path, w: u32, h: u32, rgba: [u8; 4]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let pixels: Vec<u8> = rgba.repeat((w * h) as usize);
        enc.write_header().unwrap().write_image_data(&pixels).unwrap();
    }

    /// Normal-map test asset: pure-white diffuse hero.png + hero_n.png with the specified normal color (whole image, single vector).
    fn assets_with_normal(tag: &str, normal_rgba: [u8; 4]) -> (Assets, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vitric-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_solid_png(&dir.join("hero.png"), 2, 2, [255, 255, 255, 255]);
        write_solid_png(&dir.join("hero_n.png"), 2, 2, normal_rgba);
        (Assets::load_dir(&dir).unwrap(), dir)
    }

    /// Dark ambient + one white point light (world coords lx,ly, radius 20) + a 4x4 textured sprite at the origin (optional rot).
    fn world_normal_scene(lx: f64, ly: f64, rot: Option<f64>) -> World {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": lx, "y": ly})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 20.0, "intensity": 1.0})).unwrap();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        let mut sprite = json!({"w": 4.0, "h": 4.0, "image": "hero.png"});
        if let Some(r) = rot {
            sprite["rot"] = json!(r);
        }
        w.set_component(e, "Sprite", sprite).unwrap();
        w
    }

    #[test]
    fn normal_mapped_sprite_lit_side_brighter_than_shadow_side() {
        // Normal points left across the whole image (r=0 → nx=-1): light on the left = lit side bright, light on the right = back side dark.
        // The two lights are at equal distance from the sprite center — the brightness difference comes entirely from max(dot(N,L),0)
        let (assets, dir) = assets_with_normal("nlit", [0, 128, 255, 255]);
        let lit = render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &assets, 0).unwrap();
        let dark = render_world(&world_normal_scene(8.0, 0.0, None), 64, 64, &assets, 0).unwrap();
        let bright_px = pixel(&lit, 64, 32, 32);
        let shadow_px = pixel(&dark, 64, 32, 32);
        assert!(bright_px[0] > 60, "迎光面应明显被照亮: {bright_px:?}");
        assert_eq!(shadow_px, [0, 0, 0, 255], "背光面 dot<0 夹到 0 = 只剩环境黑");
        // Determinism: same world, same tick → bit-identical
        assert_eq!(lit, render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &assets, 0).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flat_normal_still_gets_lit_by_z_lift() {
        // Flat normal (128,128,255)≈(0,0,1): still lit thanks to the L.z=0.6 lift, but pixels away from the light center
        // are darker than the same scene "without a normal map" (the old formula has no dot factor) — locks the z_lift semantics
        let (assets, dir) = assets_with_normal("nflat", [128, 128, 255, 255]);
        let with_n = render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &assets, 0).unwrap();
        // Control group: same scene but the asset has no _n pairing
        let plain_dir = std::env::temp_dir().join(format!("vitric-nflatp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain_dir);
        write_solid_png(&plain_dir.join("hero.png"), 2, 2, [255, 255, 255, 255]);
        let plain_assets = Assets::load_dir(&plain_dir).unwrap();
        let without_n =
            render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &plain_assets, 0).unwrap();
        let (n_px, p_px) = (pixel(&with_n, 64, 40, 32), pixel(&without_n, 64, 40, 32));
        assert!(n_px[0] > 0, "平面法线在灯侧仍被照亮: {n_px:?}");
        assert!(n_px[0] < p_px[0], "dot 因子 ≤ 1：带法线比不带暗: {n_px:?} vs {p_px:?}");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&plain_dir).unwrap();
    }

    #[test]
    fn pixels_without_normals_stay_byte_identical_under_lighting() {
        // 1) Solid sprite + lighting: whether the asset store has a _n file or not changes not a single byte
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#404040"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 1.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 6.0})).unwrap();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        let (assets, dir) = assets_with_normal("nlock", [0, 128, 255, 255]);
        assert_eq!(
            render_world(&w, 64, 64, &Assets::empty(), 0).unwrap(),
            render_world(&w, 64, 64, &assets, 0).unwrap(),
            "没引用法线精灵的场景：字节与空素材仓库逐位相同"
        );
        // 2) A normal-mapped sprite fully covered by a solid block drawn later: covered pixels are lit as "having no normal map" (the normal is cleared by the cover)
        let mut covered = world_normal_scene(-8.0, 0.0, None);
        let cover = covered.spawn();
        covered.set_component(cover, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        covered.set_component(cover, "Sprite", json!({"w": 6.0, "h": 6.0, "color": "#ffffff"})).unwrap();
        let mut only_cover = World::new();
        let amb = only_cover.spawn();
        only_cover.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let lamp = only_cover.spawn();
        only_cover.set_component(lamp, "Position", json!({"x": -8.0, "y": 0.0})).unwrap();
        only_cover.set_component(lamp, "Light", json!({"radius": 20.0, "intensity": 1.0})).unwrap();
        let c2 = only_cover.spawn();
        only_cover.set_component(c2, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        only_cover.set_component(c2, "Sprite", json!({"w": 6.0, "h": 6.0, "color": "#ffffff"})).unwrap();
        assert_eq!(
            render_world(&covered, 64, 64, &assets, 0).unwrap(),
            render_world(&only_cover, 64, 64, &assets, 0).unwrap(),
            "被盖住的法线像素必须与压根没有法线精灵的画面逐字节相同"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rotation_rotates_normals_with_the_sprite() {
        // Normal points up across the whole image (g=0 → ny=-1, screen y is downward). After a 90° counter-clockwise rotation the normal points left:
        // "90° sprite + left light" ≈ "unrotated + top light" (the two lights are at equal distance from the center, roughly equal pixel by pixel)
        let (assets, dir) = assets_with_normal("nrot", [128, 0, 255, 255]);
        let up_lit = render_world(&world_normal_scene(0.0, 8.0, None), 64, 64, &assets, 0).unwrap();
        let rot_lit =
            render_world(&world_normal_scene(-8.0, 0.0, Some(90.0)), 64, 64, &assets, 0).unwrap();
        let a = pixel(&up_lit, 64, 32, 32);
        let b = pixel(&rot_lit, 64, 32, 32);
        for c in 0..3 {
            assert!(
                (a[c] as i32 - b[c] as i32).abs() <= 2,
                "中心像素应近似相等: {a:?} vs {b:?}"
            );
        }
        assert!(a[0] > 60, "迎光面确实亮着（不是两边都黑的虚假相等）: {a:?}");
        // Control: after the 90° rotation the top light no longer faces the normal head-on → darker than the left light (rotation really changed the direction)
        let rot_wrong =
            render_world(&world_normal_scene(0.0, 8.0, Some(90.0)), 64, 64, &assets, 0).unwrap();
        let wrong = pixel(&rot_wrong, 64, 32, 32);
        assert!(wrong[0] + 20 < b[0], "顶灯照旋转后的左向法线应明显更暗: {wrong:?} vs {b:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Bloom test scene: a 2x2 pure-white sprite at the center (bright area), with an optional Bloom component.
    fn world_bright_sprite(bloom: Option<(f64, f64)>) -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ffffff"})).unwrap();
        if let Some((threshold, strength)) = bloom {
            let b = w.spawn();
            w.set_component(b, "Bloom", json!({"threshold": threshold, "strength": strength}))
                .unwrap();
        }
        w
    }

    #[test]
    fn bloom_halo_brightens_outside_sprite_and_scales_with_strength() {
        // The white sprite occupies the screen center 24..40: pixels **outside** the sprite rect should be lit by the halo
        let plain = render_world(&world_bright_sprite(None), 64, 64, &Assets::empty(), 0).unwrap();
        let lit = render_world(&world_bright_sprite(Some((0.5, 1.0))), 64, 64, &Assets::empty(), 0)
            .unwrap();
        // Just outside the sprite's right edge (radius 2px, 3 iterations → spreads ~6px): without bloom this is background
        let halo = pixel(&lit, 64, 42, 32);
        let bg = pixel(&plain, 64, 42, 32);
        assert_eq!(bg, BACKGROUND, "对照组：泛光关时精灵外是背景");
        assert!(halo[0] > bg[0] && halo[1] > bg[1] && halo[2] > bg[2], "光晕该比背景亮: {halo:?}");
        // Far corners are unaffected (the halo is local)
        assert_eq!(pixel(&lit, 64, 2, 2), BACKGROUND, "远处仍是背景");
        // Larger strength → brighter halo
        let stronger =
            render_world(&world_bright_sprite(Some((0.5, 3.0))), 64, 64, &Assets::empty(), 0)
                .unwrap();
        assert!(pixel(&stronger, 64, 42, 32)[0] > halo[0], "strength 大光晕更亮");
        // Determinism: same world, same tick → bit-identical
        assert_eq!(
            lit,
            render_world(&world_bright_sprite(Some((0.5, 1.0))), 64, 64, &Assets::empty(), 0)
                .unwrap()
        );
    }

    #[test]
    fn bloom_threshold_one_changes_nothing() {
        // threshold=1.0: no channel can exceed 255 → bright area all zero → bytes bit-identical to an entity without Bloom
        let plain = render_world(&world_bright_sprite(None), 64, 64, &Assets::empty(), 0).unwrap();
        let capped =
            render_world(&world_bright_sprite(Some((1.0, 2.0))), 64, 64, &Assets::empty(), 0)
                .unwrap();
        assert_eq!(plain, capped);
        // strength=0 likewise: adding 0 changes no byte (u8→f32→u8 round-trip is exact)
        let zero = render_world(&world_bright_sprite(Some((0.5, 0.0))), 64, 64, &Assets::empty(), 0)
            .unwrap();
        assert_eq!(plain, zero);
    }

    #[test]
    fn bloom_radius_is_resolution_proportional_with_floor() {
        assert_eq!(bloom_radius_px(64), 2, "小视口踩下限 2");
        assert_eq!(bloom_radius_px(180), 2);
        assert_eq!(bloom_radius_px(720), 8, "720/90 = 8");
        assert_eq!(bloom_radius_px(2160), 24, "4K 半径按比例放大");
    }

    #[test]
    fn bloom_params_are_validated_explicitly() {
        // threshold out of range
        let mut w = World::new();
        let b = w.spawn();
        w.set_component(b, "Bloom", json!({"threshold": 1.5, "strength": 1.0})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Bloom.threshold"), "{err}");
        // strength negative
        w.set_component(b, "Bloom", json!({"threshold": 0.5, "strength": -1.0})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Bloom.strength"), "{err}");
        // Missing field: explicit error with the correct usage
        w.set_component(b, "Bloom", json!({"threshold": 0.5})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("strength") && err.contains("threshold"), "{err}");
    }

    #[test]
    fn describe_includes_bloom_when_active() {
        let w = world_bright_sprite(Some((0.6, 0.8)));
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["bloom"], json!({"threshold": 0.6, "strength": 0.8}));
        assert!(d["text"].as_str().unwrap().contains("泛光开启"), "{}", d["text"]);
        // No Bloom: the bloom field does not appear in describe
        let d = describe_world(&world_bright_sprite(None), 64, 64).unwrap();
        assert!(d.get("bloom").is_none());
        assert!(!d["text"].as_str().unwrap().contains("泛光"));
    }

    /// Test TTF: the DejaVu Sans vendored with the book example (Bitstream Vera license,
    /// see examples/book/fonts/LICENSE).
    fn test_font_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/book/fonts/DejaVuSans.ttf")
    }

    fn assets_with_font() -> Assets {
        let mut a = Assets::empty();
        a.load_font(&test_font_path()).unwrap();
        a
    }

    fn world_with_text(content: &str) -> World {
        let mut w = World::new();
        let e = w.spawn_named("label").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Text", json!({"content": content, "size": 3.0, "color": "#00ff00"}))
            .unwrap();
        w
    }

    /// Collect the coordinates of all non-background pixels.
    fn non_background(buf: &[u8], width: u32, height: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for y in 0..height {
            for x in 0..width {
                if pixel(buf, width, x, y) != BACKGROUND {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn no_font_keeps_bitmap_path_byte_identical() {
        // No font attached to the asset store = old bitmap behavior: bit-identical to what Assets::empty() renders
        let w = world_with_text("SCORE 42");
        let dir = std::env::temp_dir().join(format!("vitric-nofont-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let loaded = Assets::load_dir(&dir).unwrap();
        assert!(loaded.font().is_none());
        assert_eq!(
            render_world(&w, 96, 64, &Assets::empty(), 0).unwrap(),
            render_world(&w, 96, 64, &loaded, 0).unwrap(),
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vector_font_renders_near_position_and_is_deterministic() {
        let w = world_with_text("Hi");
        let assets = assets_with_font();
        let buf = render_world(&w, 96, 96, &assets, 0).unwrap();
        let hits = non_background(&buf, 96, 96);
        assert!(!hits.is_empty(), "矢量文字应画出像素");
        // size=3, scale=8 → font size 24px: all text pixels should fall inside the bounding box near Position (screen center)
        // (horizontal margin scaled by the two-character ratio, vertical margin by half the font size)
        for &(x, y) in &hits {
            assert!((24..=72).contains(&x) && (30..=66).contains(&y), "({x},{y}) 跑出文字包围盒");
        }
        // Inside the glyph there must be fully-covered pixels = exact Text.color (anti-aliasing only at edges)
        assert!(
            hits.iter().any(|&(x, y)| pixel(&buf, 96, x, y) == [0, 255, 0, 255]),
            "应存在满覆盖像素"
        );
        // Determinism: same world, same tick → bit-identical (cache hit/miss does not affect output)
        assert_eq!(buf, render_world(&w, 96, 96, &assets, 0).unwrap());
        // Proportional kerning: "iii" is narrower than "WWW" (the fixed-width bitmap path cannot do this)
        let font = assets.font().unwrap();
        let (_, narrow) = font.layout("iii", 24);
        let (_, wide) = font.layout("WWW", 24);
        assert!(narrow < wide, "比例字距: iii({narrow}) 应窄于 WWW({wide})");
    }

    #[test]
    fn vector_font_renders_cjk_with_nonempty_coverage() {
        // CJK characters must draw something via the vector path: if the font has the glyph it is the real character, otherwise (e.g. DejaVu)
        // it is the font's .notdef tofu block — visibly present, not silently dropped
        let assets = assets_with_font();
        let g = assets.font().unwrap().raster('中', 24);
        assert!(!g.coverage.is_empty(), "CJK 字符栅格化覆盖率不应为空");
        assert!(g.coverage.iter().any(|&c| c > 0));
        let w = world_with_text("中文");
        let buf = render_world(&w, 96, 96, &assets, 0).unwrap();
        assert!(!non_background(&buf, 96, 96).is_empty(), "CJK 文字应有可见像素");
    }

    /// reveal default / ≥1 is byte-identical to before the feature was introduced (backward compatibility, both paths verified).
    #[test]
    fn reveal_full_or_absent_is_byte_identical() {
        // Vector path
        let w_plain = world_with_text("REVEAL");
        let mut w_full = World::new();
        let e = w_full.spawn_named("label").unwrap();
        w_full.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w_full
            .set_component(e, "Text", json!({"content": "REVEAL", "size": 3.0, "color": "#00ff00", "reveal": 1.0}))
            .unwrap();
        let assets = assets_with_font();
        assert_eq!(
            render_world(&w_plain, 128, 64, &assets, 0).unwrap(),
            render_world(&w_full, 128, 64, &assets, 0).unwrap(),
            "矢量路径 reveal=1 必须与无 reveal 字段逐字节相同"
        );
        // reveal=2 (>1) also fully shows, equivalent
        w_full.set_field(e, "Text.reveal", json!(2.0)).unwrap();
        assert_eq!(
            render_world(&w_plain, 128, 64, &assets, 0).unwrap(),
            render_world(&w_full, 128, 64, &assets, 0).unwrap(),
        );
        // Bitmap path likewise byte-identical
        assert_eq!(
            render_world(&w_plain, 128, 64, &Assets::empty(), 0).unwrap(),
            render_world(&w_full, 128, 64, &Assets::empty(), 0).unwrap(),
        );
    }

    /// Under reveal, the visible character count = pure function: larger reveal draws more characters, pixel count is monotonically non-decreasing,
    /// and reveal<1 draws a true subset of the fully-shown text (character-by-character reveal "grows to the right", no reflow).
    #[test]
    fn reveal_progressively_shows_more_pixels() {
        let assets = assets_with_font();
        let mut w = World::new();
        let e = w.spawn_named("label").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Text", json!({"content": "ABCDEF", "size": 3.0, "color": "#00ff00", "reveal": 0.0}))
            .unwrap();
        // reveal=0: not a single character is drawn
        let none = render_world(&w, 160, 64, &assets, 0).unwrap();
        assert!(non_background(&none, 160, 64).is_empty(), "reveal=0 不该画任何字");
        // Progressively open up: pixel count is monotonically non-decreasing
        let mut prev = 0usize;
        for r in [0.34_f64, 0.67, 1.0] {
            w.set_field(e, "Text.reveal", json!(r)).unwrap();
            let buf = render_world(&w, 160, 64, &assets, 0).unwrap();
            let hits = non_background(&buf, 160, 64).len();
            assert!(hits >= prev, "reveal={r} 像素数应不少于更小的 reveal（{hits} < {prev}）");
            prev = hits;
        }
        // The half-shown glyph falls in the left half of the full show (the same layout slice, no left-right jitter):
        // reveal=0.5 (3 chars ABC) the rightmost pixel must be < the full-show rightmost pixel
        w.set_field(e, "Text.reveal", json!(0.5)).unwrap();
        let half = render_world(&w, 160, 64, &assets, 0).unwrap();
        w.set_field(e, "Text.reveal", json!(1.0)).unwrap();
        let full = render_world(&w, 160, 64, &assets, 0).unwrap();
        let max_x = |buf: &[u8]| non_background(buf, 160, 64).iter().map(|&(x, _)| x).max();
        assert!(max_x(&half) < max_x(&full), "半显的字应是全显的左前缀，不越过全显右缘");
    }

    /// Performance budget item 3: playing the same text over N ticks, layout (the layout algorithm) runs exactly once.
    #[test]
    fn typewriter_layout_runs_exactly_once_over_many_ticks() {
        let assets = assets_with_font();
        let font = assets.font().unwrap();
        let mut w = World::new();
        let e = w.spawn_named("line").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Text", json!({"content": "TYPEWRITER", "size": 2.0, "color": "#ffffff", "reveal": 0.0}))
            .unwrap();
        let base = font.layout_runs();
        // Typewriter: reveal gradually goes from 0 to 1, render 40 frames (each frame changes the visible char count, never reflows the whole line)
        for i in 0..=40u32 {
            w.set_field(e, "Text.reveal", json!(i as f64 / 40.0)).unwrap();
            let _ = render_world(&w, 192, 96, &assets, 0).unwrap();
        }
        assert_eq!(
            font.layout_runs() - base,
            1,
            "同一段文字（同字号）排版只该算一次，之后命中缓存——逐字显示不许每 tick 重排"
        );
    }

    #[test]
    fn font_missing_or_corrupt_is_an_explicit_error_naming_the_path() {
        let mut a = Assets::empty();
        let err = a.load_font(std::path::Path::new("/nonexistent/ghost.ttf")).unwrap_err();
        assert!(err.contains("/nonexistent/ghost.ttf"), "{err}");
        // Corrupt font: bytes can be read but parsing fails, likewise names the path
        let dir = std::env::temp_dir().join(format!("vitric-badfont-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.ttf");
        std::fs::write(&bad, b"definitely not a font").unwrap();
        let err = a.load_font(&bad).unwrap_err();
        assert!(err.contains("bad.ttf"), "{err}");
        assert!(a.font().is_none(), "加载失败不应留下半个字体");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn assets_reload_keeps_the_font() {
        let dir = std::env::temp_dir().join(format!("vitric-fontreload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Assets::load_dir(&dir).unwrap();
        a.load_font(&test_font_path()).unwrap();
        a.reload().unwrap();
        assert!(a.font().is_some(), "热重载不能把字体弄丢");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn errors_are_helpful() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "red"})).unwrap();
        let err = render_world(&w, 32, 32, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("#rrggbb"), "{err}");
        assert!(render_world(&w, 0, 32, &Assets::empty(), 0).is_err());
        // rot written as a string: explicit error (not silently treated as 0)
        w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ff0000", "rot": "45"}))
            .unwrap();
        let err = render_world(&w, 32, 32, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Sprite.rot"), "{err}");
    }

    // ---- Text readability warnings (real incident prototype: beige text on a beige card face, invisible to the builder agent) ----

    /// Scaffold: white-background sprite + one text line (position/color adjustable).
    /// Default camera: 8 pixels/unit → a 64x64 viewport sees world ±4 units.
    fn world_text_on_sprite(sprite_color: &str, text_color: &str, x: f64) -> World {
        let mut w = World::new();
        let bg = w.spawn();
        w.set_component(bg, "Position", json!({"x": x, "y": 0.0})).unwrap();
        w.set_component(bg, "Sprite", json!({"w": 8.0, "h": 8.0, "color": sprite_color}))
            .unwrap();
        let t = w.spawn_named("hud").unwrap();
        w.set_component(t, "Position", json!({"x": x, "y": 0.0})).unwrap();
        w.set_component(t, "Text", json!({"content": "HP", "size": 2.0, "color": text_color}))
            .unwrap();
        w
    }

    #[test]
    fn describe_warns_on_low_contrast_text() {
        // White text on white background: contrast ≈ 1, must give a low-contrast-text warning + a ⚠ line in the summary
        let w = world_text_on_sprite("#ffffff", "#ffffff", 0.0);
        let d = describe_world(&w, 64, 64).unwrap();
        let warns = d["warnings"].as_array().expect("白字白底必须有 warnings");
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert_eq!(warns[0]["kind"], json!("low-contrast-text"));
        assert_eq!(warns[0]["content"], json!("HP"));
        let ratio = warns[0]["ratio"].as_f64().expect("ratio 是数字");
        assert!(ratio < TEXT_CONTRAST_MIN, "白叠白比值该接近 1，拿到 {ratio}");
        assert!(warns[0]["hint"].as_str().unwrap().contains("人眼难读"));
        let text = d["text"].as_str().unwrap();
        assert!(text.contains('⚠') && text.contains("对比度过低"), "{text}");
        // Incident prototype: beige text (#f5e8cc) on a beige background must also be caught
        let d = describe_world(&world_text_on_sprite("#f0e6c8", "#f5e8cc", 0.0), 64, 64).unwrap();
        assert!(d.get("warnings").is_some(), "米色叠米色必须有警告");
    }

    #[test]
    fn describe_no_warning_on_dark_background() {
        // The same white text on a dark background (the default background color): no warning, no warnings key
        let mut w = World::new();
        let t = w.spawn();
        w.set_component(t, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(t, "Text", json!({"content": "HP", "size": 2.0, "color": "#ffffff"}))
            .unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert!(d.get("warnings").is_none(), "深底白字不该警告: {:?}", d.get("warnings"));
        assert!(!d["text"].as_str().unwrap().contains('⚠'));
    }

    #[test]
    fn describe_skips_contrast_check_for_offscreen_text() {
        // The same white-on-white moved off-screen (beyond ±4 units): neither rendered nor measured, no warning
        let w = world_text_on_sprite("#ffffff", "#ffffff", 100.0);
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["texts"][0]["region"], json!("视野外"));
        assert!(d.get("warnings").is_none(), "视野外的文字不进对比度测量");
    }

    // ---- 2D shadows (Ambient.shadows + Solid occluders) ----

    /// Shadow test scaffold: dark ambient (shadows field configurable) + a white point light at the origin (radius 6 = 48px,
    /// covering most of the 64x64 viewport). Lighting applies to the whole frame (the background is also lit) — no sprite is placed,
    /// all brightness changes come from lighting/shadows, with no drawing differences mixed in.
    fn world_shadow_scene(shadows: Option<Value>) -> World {
        let mut w = World::new();
        let amb = w.spawn();
        let mut a = json!({"color": "#000000"});
        if let Some(s) = shadows {
            a["shadows"] = s;
        }
        w.set_component(amb, "Ambient", a).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 6.0, "intensity": 1.0})).unwrap();
        w
    }

    /// Place a cw×ch occluder wall at (x,y) (Solid+Position+Collider, deliberately without a Sprite —
    /// invisible on screen, pixel differences can only come from it blocking light).
    fn add_wall(w: &mut World, x: f64, y: f64, cw: f64, ch: f64) -> vitric_ecs::EntityId {
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": x, "y": y})).unwrap();
        w.set_component(e, "Collider", json!({"w": cw, "h": ch})).unwrap();
        w.set_component(e, "Solid", json!({})).unwrap();
        e
    }

    #[test]
    fn shadows_off_is_byte_identical() {
        // Three control groups lock down byte-for-byte that "off = as if it didn't exist":
        // 1) shadows field default vs explicit false (the schema default materializes false into the component)
        let mut absent = world_shadow_scene(None);
        add_wall(&mut absent, 2.0, 0.0, 1.0, 2.0);
        let mut explicit_off = world_shadow_scene(Some(json!(false)));
        add_wall(&mut explicit_off, 2.0, 0.0, 1.0, 2.0);
        let buf_absent = render_world(&absent, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(buf_absent, render_world(&explicit_off, 64, 64, &Assets::empty(), 0).unwrap());
        // 2) When off, the Solid wall has zero effect on the frame (the wall has no Sprite, blocking light is its only possible pixel effect)
        let no_wall = world_shadow_scene(None);
        assert_eq!(buf_absent, render_world(&no_wall, 64, 64, &Assets::empty(), 0).unwrap());
        // 3) On but with no occluders in the scene: byte-identical to off (an empty list changes no arithmetic)
        let on_empty = world_shadow_scene(Some(json!(true)));
        assert_eq!(buf_absent, render_world(&on_empty, 64, 64, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn wall_casts_shadow_and_removing_it_equalizes() {
        // The light is at pixel (32,32), the wall occupies pixels x∈[44,52] y∈[24,40].
        // Pick two pixel centers **strictly equidistant** from the light center: (56,32)→fx 56.5 (behind the wall) and
        // (7,32)→fx 7.5 (the opposite side with no wall), |dx| both 24.5, dy both 0.5 — the brightness difference can only come from occlusion
        let mut w = world_shadow_scene(Some(json!(true)));
        let wall = add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        let behind = pixel(&buf, 64, 56, 32);
        let open = pixel(&buf, 64, 7, 32);
        assert_eq!(behind, [0, 0, 0, 255], "墙后像素被挡 = 只剩环境黑");
        assert!(open[0] > 0 && open[1] > 0, "对侧等距像素该被照亮: {open:?}");
        // Tear down the wall (removing Solid means it is no longer an occluder): the two equidistant pixels are now equally bright
        w.remove_component(wall, "Solid").unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 56, 32), pixel(&buf, 64, 7, 32), "无墙时等距像素同亮");
        assert!(pixel(&buf, 64, 56, 32)[0] > 0, "拆墙后原阴影处被照亮");
    }

    #[test]
    fn pixel_on_occluder_is_lit_but_other_boxes_still_shadow_it() {
        // Rule lock: a pixel inside a box is not occluded by **itself**, but is still occluded by **other** boxes.
        // Pixel (48,32) (fx 48.5) is inside the wall x∈[44,52]
        let mut w = world_shadow_scene(Some(json!(true)));
        add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        let on_wall = pixel(&buf, 64, 48, 32);
        assert!(on_wall[0] > 0, "遮光体上的像素不被自己压黑: {on_wall:?}");
        // Stand up another wall between the light and the wall (pixels x∈[38,42]): the pixel that was "on the wall" is now blocked by it
        add_wall(&mut w, 1.0, 0.0, 0.5, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 48, 32), [0, 0, 0, 255], "别的箱子照样遮它");
    }

    #[test]
    fn spot_light_is_shadowed_too() {
        // Spot light facing +x (dir=0, cone angle 90°): the pixel behind the wall (56,32) is inside the cone but blocked → black;
        // after tearing down the wall the same pixel is lit — cone attenuation does not exempt occlusion
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000", "shadows": true})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            lamp,
            "Light",
            json!({"radius": 6.0, "kind": "spot", "angle": 90.0, "dir": 0.0}),
        )
        .unwrap();
        let wall = add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 56, 32), [0, 0, 0, 255], "锥内但被墙挡 = 黑");
        w.remove_component(wall, "Solid").unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert!(pixel(&buf, 64, 56, 32)[0] > 0, "拆墙后锥内像素被照亮");
    }

    #[test]
    fn shadowed_render_is_deterministic() {
        let mut w = world_shadow_scene(Some(json!(true)));
        add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        add_wall(&mut w, -1.5, 1.0, 2.0, 0.5);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 7).unwrap();
        assert_eq!(buf, render_world(&w, 64, 64, &Assets::empty(), 7).unwrap());
    }

    #[test]
    fn occluder_cap_is_an_explicit_error() {
        let mut w = world_shadow_scene(Some(json!(true)));
        for i in 0..(MAX_OCCLUDERS + 1) {
            add_wall(&mut w, i as f64, -10.0, 1.0, 1.0);
        }
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("257") && err.contains("256"), "{err}");
        // The same wall array with shadows off does not error (the cap only belongs to the shadow path)
        let mut off = world_shadow_scene(None);
        for i in 0..(MAX_OCCLUDERS + 1) {
            add_wall(&mut off, i as f64, -10.0, 1.0, 1.0);
        }
        render_world(&off, 64, 64, &Assets::empty(), 0).unwrap();
    }

    #[test]
    fn shadow_fields_are_validated_explicitly() {
        // shadows not a bool: explicit error (not silently treated as false)
        let w = world_shadow_scene(Some(json!("yes")));
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Ambient.shadows"), "{err}");
        // Occluder Collider.w not a number: explicit error naming the field
        let mut w = world_shadow_scene(Some(json!(true)));
        let wall = add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        w.set_field(wall, "Collider.w", json!("wide")).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Collider.w"), "{err}");
    }

    #[test]
    fn segment_hits_aabb_covers_axis_parallel_and_misses() {
        let bx = (4.0, 4.0, 6.0, 6.0);
        // Cross horizontally / vertically (degenerate branch where the axis-parallel component = 0)
        assert!(segment_hits_aabb((0.0, 5.0), (10.0, 5.0), bx), "水平穿过");
        assert!(segment_hits_aabb((5.0, 0.0), (5.0, 10.0), bx), "垂直穿过");
        assert!(!segment_hits_aabb((0.0, 7.0), (10.0, 7.0), bx), "水平线在箱外");
        assert!(!segment_hits_aabb((7.0, 0.0), (7.0, 10.0), bx), "垂直线在箱外");
        // Diagonal cross / diagonal pass-through without intersection (slab intervals do not overlap)
        assert!(segment_hits_aabb((0.0, 0.0), (10.0, 10.0), bx), "对角穿过");
        assert!(!segment_hits_aabb((0.0, 6.5), (10.0, 16.5), bx), "斜线擦过上方");
        // Segment truncation: right direction but out of reach (t > 1)
        assert!(!segment_hits_aabb((0.0, 5.0), (3.0, 5.0), bx), "线段没到箱子就停");
        // An endpoint inside the box also counts as intersection (the interval [0,1] is clipped inside the box)
        assert!(segment_hits_aabb((5.0, 5.0), (10.0, 5.0), bx), "起点在箱内");
    }

    #[test]
    fn describe_includes_shadows_and_occluder_count() {
        let mut w = world_shadow_scene(Some(json!(true)));
        add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        add_wall(&mut w, -3.0, 1.0, 1.0, 1.0);
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["shadows"], json!(true));
        assert_eq!(d["occluders"], json!(2));
        let text = d["text"].as_str().unwrap();
        assert!(text.contains("投影开启") && text.contains("2 个遮光体"), "{text}");
        // Off (default): the key does not appear, the summary has no shadow line
        let mut off = world_shadow_scene(None);
        add_wall(&mut off, 2.0, 0.0, 1.0, 2.0);
        let d = describe_world(&off, 64, 64).unwrap();
        assert!(d.get("shadows").is_none() && d.get("occluders").is_none());
        assert!(!d["text"].as_str().unwrap().contains("投影"));
    }

    /// Glow-scale composite scene: 1280x720, 12 point lights, 100 tile occluders (two 40-tile floors
    /// plus 20 scattered pillars). Shared by the benchmark and equivalence tests — covers mergeable (whole-row floors),
    /// non-mergeable (staggered pillars), and light center inside a floor band (in-box pixel path) shapes.
    fn world_glow_like_scene() -> World {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#101018", "shadows": true})).unwrap();
        // Two 40-tile floors (a whole row can be merged into one long slab)
        for row in 0..2 {
            let y = if row == 0 { -2.0 } else { 6.0 };
            for i in 0..40 {
                add_wall(&mut w, -19.5 + i as f64, y, 1.0, 1.0);
            }
        }
        // 20 scattered pillars (spaced 3 apart, leaving gaps, not mergeable)
        for i in 0..20 {
            add_wall(&mut w, -28.5 + i as f64 * 3.0, 2.0, 1.0, 2.0);
        }
        // 12 point lights: half float above the floor, half have their center buried in a floor band (in-box pixels also go through occlusion)
        for i in 0..12 {
            let lamp = w.spawn();
            let x = -22.0 + i as f64 * 4.0;
            let y = if i % 2 == 0 { 1.0 } else { -2.0 };
            w.set_component(lamp, "Position", json!({"x": x, "y": y})).unwrap();
            w.set_component(lamp, "Light", json!({"radius": 10.0, "intensity": 1.2})).unwrap();
        }
        w
    }

    /// Benchmark (ignored by default, run manually: `cargo test --release -p vitric-render -- --ignored shadow_bench --nocapture`).
    /// 1280x720 · 12 lights · 100 occluders — before optimization this frame was the full product of pixels × lights × boxes.
    #[test]
    #[ignore]
    fn shadow_bench_glow_like_scene() {
        let w = world_glow_like_scene();
        // Warm up once (placeholder allocations and page faults do not enter timing)
        render_world(&w, 1280, 720, &Assets::empty(), 0).unwrap();
        let n = 5;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            render_world(&w, 1280, 720, &Assets::empty(), 0).unwrap();
        }
        let per_frame = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("shadow_bench: 1280x720 · 12 灯 · 100 遮光体 = {per_frame:.1} ms/帧");
    }

    /// Equivalence baseline: each occluder forms its own group (no merging) — the external path is the original per-box formula.
    fn naive_grid(occluders: &[Occluder], width: u32, height: u32, cam: (f64, f64, f64)) -> ShadowBoxes {
        let mut grid = ShadowBoxes { merged: Vec::new(), subs: Vec::new() };
        for i in 0..occluders.len() {
            let g = build_shadow_boxes(&occluders[i..=i], width, height, cam);
            let off = grid.subs.len();
            grid.subs.extend(g.subs);
            for mut m in g.merged {
                m.sub_start += off;
                grid.merged.push(m);
            }
        }
        grid
    }

    /// Strengthened scene for the equivalence test: on top of the glow-scale scene add a spot light, overlapping walls, degenerate walls (w=0)
    /// and a non-default (but binary-exactly-representable) camera — covers every branch of the merge predicate.
    fn world_equivalence_scene() -> World {
        let mut w = world_glow_like_scene();
        let beam = w.spawn();
        w.set_component(beam, "Position", json!({"x": -4.0, "y": 4.0})).unwrap();
        w.set_component(
            beam,
            "Light",
            json!({"radius": 12.0, "kind": "spot", "angle": 80.0, "dir": 270.0}),
        )
        .unwrap();
        add_wall(&mut w, 0.25, -2.0, 1.0, 1.0); // Overlaps the floor row but is not flush: must not merge
        add_wall(&mut w, 5.0, 0.5, 0.0, 2.0); // Degenerate wall (w=0): does not participate in merging, behavior unchanged
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 0.5, "y": -0.25, "scale": 4.0})).unwrap();
        w
    }

    #[test]
    fn merged_and_culled_shadows_are_byte_identical_to_naive() {
        // The optimized path (render_world: merge + per-light culling) and the control path (per-box full pass, no culling)
        // produce byte-identical output — neither merge nor culling may change a single byte.
        let (width, height) = (320u32, 180u32);
        let mut w = world_equivalence_scene();
        let optimized = render_world(&w, width, height, &Assets::empty(), 0).unwrap();

        let lights = collect_lights(&w).unwrap();
        let occs = collect_occluders(&w).unwrap();
        let cam = camera_of(&w, 0, height).unwrap();
        let (ambient, _) = ambient_of(&w).unwrap().unwrap();
        // Unlit baseplate: remove Ambient = lighting entirely off, the rest of the drawing is exactly the same
        let amb = w.query(&["Ambient"])[0];
        w.remove_component(amb, "Ambient").unwrap();
        let unlit = render_world(&w, width, height, &Assets::empty(), 0).unwrap();
        assert_ne!(optimized, unlit, "场景必须真的被光照改写过，否则测试空转");

        // Control 1: no merging, no culling (the original per-box per-light full pass formula)
        let mut naive = unlit.clone();
        let grid = naive_grid(&occs, width, height, cam);
        apply_lighting_impl(&mut naive, width, height, cam, ambient, &lights, &grid, false, None);
        assert_eq!(optimized, naive, "合并+剔除改了输出字节");

        // Control 2: merge, no culling — independently locks down "per-light culling is lossless"
        let mut merged_only = unlit;
        let grid = build_shadow_boxes(&occs, width, height, cam);
        apply_lighting_impl(
            &mut merged_only,
            width,
            height,
            cam,
            ambient,
            &lights,
            &grid,
            false,
            None,
        );
        assert_eq!(optimized, merged_only, "逐灯剔除改了输出字节");
    }

    #[test]
    fn flush_tiles_merge_into_slabs_and_gaps_do_not() {
        let mut w = World::new();
        // 10 1x1 tiles flush in a row (center x = 0..9) → merge into one horizontal slab
        for i in 0..10 {
            add_wall(&mut w, i as f64, 0.0, 1.0, 1.0);
        }
        add_wall(&mut w, 12.0, 0.0, 1.0, 1.0); // Gap: does not merge
        add_wall(&mut w, 0.0, 3.0, 1.0, 1.0); // Same column but y not flush: does not merge
        let occs = collect_occluders(&w).unwrap();
        let g = build_shadow_boxes(&occs, 64, 64, (0.0, 0.0, 8.0));
        assert_eq!(g.merged.len(), 3, "一根横条 + 两个孤箱");
        assert_eq!(g.subs.len(), 12, "子箱总数 = 原始遮光体数");
        let slab = g.merged.iter().find(|m| m.sub_len == 10).expect("瓦片行收成一组");
        // Row world x ∈ [-0.5, 9.5], y ∈ [-0.5, 0.5] → pixels [28, 28, 108, 36] (scale 8)
        assert_eq!(slab.aabb, [28.0, 28.0, 108.0, 36.0]);

        // 2x2 tile array: first collect along x into two strips, then stack along y into one block (4 sub-boxes)
        let mut w = World::new();
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            add_wall(&mut w, x, y, 1.0, 1.0);
        }
        let occs = collect_occluders(&w).unwrap();
        let g = build_shadow_boxes(&occs, 64, 64, (0.0, 0.0, 8.0));
        assert_eq!((g.merged.len(), g.merged[0].sub_len), (1, 4), "两轮 1D 合并 = 一整块");
    }

    #[test]
    fn light_disc_culls_only_unreachable_boxes() {
        let mut w = World::new();
        add_wall(&mut w, 1.0, 0.0, 1.0, 1.0); // Inside the light disc: kept
        add_wall(&mut w, 20.0, 0.0, 1.0, 1.0); // Far outside the light disc: culled
        let occs = collect_occluders(&w).unwrap();
        let g = build_shadow_boxes(&occs, 64, 64, (0.0, 0.0, 8.0));
        // Light at pixel (32,32), radius 6*8=48px. Near box center (40,32), far box center (192,32)
        let kept = cull_shadow_boxes(&g, 32.0, 32.0, 48.0);
        assert_eq!(kept.len(), 1);
        assert_eq!(g.merged[kept[0] as usize].aabb, [36.0, 28.0, 44.0, 36.0]);
        // Light center buried inside a box: nearest distance 0, necessarily kept
        let kept = cull_shadow_boxes(&g, 40.0, 32.0, 1.0);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn describe_contrast_check_tolerates_missing_images() {
        // Image not in the asset store: the contrast measurement falls back to a Sprite.color solid-block approximation, describe does not error.
        // (Normal rendering render_world still errors explicitly on a missing image — the leniency belongs only to this internal measurement path)
        let mut w = world_text_on_sprite("#ffffff", "#ffffff", 0.0);
        let bg = w.query(&["Sprite"])[0];
        w.set_component(
            bg,
            "Sprite",
            json!({"w": 8.0, "h": 8.0, "color": "#ffffff", "image": "ghost.png"}),
        )
        .unwrap();
        assert!(render_world(&w, 64, 64, &Assets::empty(), 0).is_err(), "正常渲染缺图必须报错");
        let d = describe_world(&w, 64, 64).unwrap();
        let warns = d["warnings"].as_array().expect("白色块近似底下仍是白底，警告照给");
        assert_eq!(warns[0]["kind"], json!("low-contrast-text"));
    }

    // ---- Particle emitters (Emitter, a pure render-layer product) ----

    /// Spark-stream test field: one stream emitter (60 particles/sec, lifetime 30 ticks).
    fn world_stream_emitter() -> World {
        let mut w = World::new();
        let e = w.spawn_named("sparks").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "stream", "rate": 60.0, "lifetime": 30, "size": 0.5,
                   "speed_min": 2.0, "speed_max": 5.0, "spread": 360.0,
                   "color": "#ffcc40", "color_end": "#ff3000"}),
        )
        .unwrap();
        w
    }

    #[test]
    fn emitter_same_tick_renders_byte_identical_and_draws_pixels() {
        let w = world_stream_emitter();
        let a = render_world(&w, 64, 64, &Assets::empty(), 100).unwrap();
        let b = render_world(&w, 64, 64, &Assets::empty(), 100).unwrap();
        assert_eq!(a, b, "同一 tick 两次渲染必须逐字节一致");
        assert!(
            a.chunks_exact(4).any(|p| p != BACKGROUND),
            "稳态 tick 100 必须真的画出粒子"
        );
        // Different ticks evolve the frame (particles are moving)
        let c = render_world(&w, 64, 64, &Assets::empty(), 101).unwrap();
        assert_ne!(a, c, "粒子是 tick 的函数，下一 tick 画面应不同");
    }

    #[test]
    fn emitter_particles_is_a_pure_function_of_tick() {
        let w = world_stream_emitter();
        let e = &collect_emitters(&w).unwrap()[0];
        let p1 = emitter_particles(e, 100);
        let p2 = emitter_particles(e, 100);
        assert_eq!(p1.len(), p2.len());
        for (a, b) in p1.iter().zip(&p2) {
            assert_eq!((a.x, a.y, a.size, a.rgba), (b.x, b.y, b.size, b.rgba));
        }
        // Steady-state visible count = rate·lifetime/60 = 60·30/60 = 30
        assert_eq!(p1.len(), 30, "稳态在途粒子数");
        // Early (tick < lifetime) only already-born batches: tick 5 → birth ticks 0..=5, 6 batches × 1
        assert_eq!(emitter_particles(e, 5).len(), 6);
        // Older particles first (drawn underneath): the first particle is older than the last → lower alpha
        assert!(p1.first().unwrap().rgba[3] < p1.last().unwrap().rgba[3]);
    }

    #[test]
    fn burst_appears_at_trigger_tick_and_expires_after_lifetime() {
        let mut w = World::new();
        let e = w.spawn_named("boom").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "burst", "count": 12, "lifetime": 20, "size": 0.5, "burst": 50}),
        )
        .unwrap();
        let em = &collect_emitters(&w).unwrap()[0];
        assert_eq!(emitter_particles(em, 49).len(), 0, "触发前一无所有");
        assert_eq!(emitter_particles(em, 50).len(), 12, "触发 tick 全员出生");
        assert_eq!(emitter_particles(em, 69).len(), 12, "寿命最后一 tick 还在");
        assert_eq!(emitter_particles(em, 70).len(), 0, "寿命到期当帧消失");
        // burst default -1 = not triggered
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "burst", "count": 12, "lifetime": 20, "size": 0.5}),
        )
        .unwrap();
        let em = &collect_emitters(&w).unwrap()[0];
        assert_eq!(emitter_particles(em, 100).len(), 0);
    }

    #[test]
    fn particle_motion_is_analytic_and_fades() {
        // spread 0 + fixed initial velocity + gravity: position must strictly equal the analytic formula origin + v0·t + ½g·t²
        let mut w = World::new();
        let e = w.spawn_named("jet").unwrap();
        w.set_component(e, "Position", json!({"x": 1.0, "y": 2.0})).unwrap();
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "burst", "count": 1, "lifetime": 60, "size": 1.0, "burst": 0,
                   "dir": 90.0, "spread": 0.0, "speed_min": 6.0, "speed_max": 6.0,
                   "gravity": -10.0}),
        )
        .unwrap();
        let em = &collect_emitters(&w).unwrap()[0];
        for age in [0i64, 10, 30, 59] {
            let p = &emitter_particles(em, age as u64)[0];
            let t = age as f64 / PARTICLE_TICKS_PER_SECOND;
            // dir=90 (+y), spread=0: x does not move (the floating-point tail of cos90 ≈ 0)
            assert!((p.x - 1.0).abs() < 1e-9, "age {age}: x={}", p.x);
            let expect_y = 2.0 + 6.0 * t + 0.5 * (-10.0) * t * t;
            assert!((p.y - expect_y).abs() < 1e-9, "age {age}: y={} 应为 {expect_y}", p.y);
            // alpha fades out linearly
            let expect_a = (255.0 * (1.0 - age as f64 / 60.0)).round() as u8;
            assert_eq!(p.rgba[3], expect_a, "age {age}");
        }
    }

    #[test]
    fn emitter_off_or_absent_keeps_bytes_identical() {
        // Same world: no emitter vs an emitter with active=false → output byte-identical
        let base = world_one_red_sprite();
        let frame_none = render_world(&base, 64, 64, &Assets::empty(), 77).unwrap();
        let mut with_off = world_one_red_sprite();
        let e = with_off.spawn_named("muted").unwrap();
        with_off.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        with_off
            .set_component(
                e,
                "Emitter",
                json!({"kind": "stream", "rate": 60.0, "lifetime": 30, "size": 0.5,
                       "active": false}),
            )
            .unwrap();
        let frame_off = render_world(&with_off, 64, 64, &Assets::empty(), 77).unwrap();
        assert_eq!(frame_none, frame_off, "active=false = 一个粒子都不画 = 旧行为字节不变");
    }

    #[test]
    fn particles_are_self_lit_under_darkness() {
        // Fully-black ambient light: the sprite is darkened, particles light themselves (self-emission convention)
        let mut w = world_one_red_sprite();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let e = w.spawn_named("sparks").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "burst", "count": 1, "lifetime": 60, "size": 1.0, "burst": 0,
                   "spread": 0.0, "color": "#00ff00"}),
        )
        .unwrap();
        // burst@0, age 0, speed 0 → particle stays at origin = screen center, alpha 255 pure green
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [0, 255, 0, 255], "粒子不被全黑环境光压暗");
        // Sprite pixels outside the particle's bounds are indeed darkened (lighting is running)
        assert_eq!(pixel(&buf, 64, 26, 26), [0, 0, 0, 255], "精灵照常被打光");
    }

    #[test]
    fn emitter_errors_are_explicit_with_hints() {
        let mut w = World::new();
        let e = w.spawn_named("bad").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        // kind missing
        w.set_component(e, "Emitter", json!({"lifetime": 30, "size": 0.5})).unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("kind") && err.contains("stream") && err.contains("burst"), "{err}");
        // kind unrecognized
        w.set_component(e, "Emitter", json!({"kind": "fountain", "lifetime": 30, "size": 0.5}))
            .unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("fountain") && err.contains("不认识"), "{err}");
        // stream missing rate
        w.set_component(e, "Emitter", json!({"kind": "stream", "lifetime": 30, "size": 0.5}))
            .unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("rate") && err.contains("写法"), "{err}");
        // lifetime invalid
        w.set_component(e, "Emitter", json!({"kind": "stream", "rate": 10.0, "lifetime": 0, "size": 0.5}))
            .unwrap();
        assert!(collect_emitters(&w).unwrap_err().contains("lifetime"), "lifetime ≥ 1");
        // size missing
        w.set_component(e, "Emitter", json!({"kind": "stream", "rate": 10.0, "lifetime": 30}))
            .unwrap();
        assert!(collect_emitters(&w).unwrap_err().contains("size"));
        // particle budget exceeded
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "stream", "rate": 6000.0, "lifetime": 600, "size": 0.5}),
        )
        .unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("预算") && err.contains("1024"), "{err}");
        // no Position
        let mut w2 = World::new();
        let e2 = w2.spawn_named("floating").unwrap();
        w2.set_component(e2, "Emitter", json!({"kind": "burst", "count": 5, "lifetime": 30, "size": 0.5}))
            .unwrap();
        assert!(collect_emitters(&w2).unwrap_err().contains("Position"));
    }

    #[test]
    fn describe_summarizes_emitters_one_line_each() {
        let mut w = world_stream_emitter();
        let b = w.spawn_named("boom").unwrap();
        w.set_component(b, "Position", json!({"x": 3.0, "y": 0.0})).unwrap();
        w.set_component(
            b,
            "Emitter",
            json!({"kind": "burst", "count": 8, "lifetime": 20, "size": 0.4}),
        )
        .unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        let ems = d["emitters"].as_array().expect("有发射器必须给 emitters 键");
        assert_eq!(ems.len(), 2);
        assert_eq!(ems[0]["kind"], json!("stream"));
        assert_eq!(ems[0]["visible_estimate"], json!(30), "rate 60 × lifetime 30 / 60");
        assert_eq!(ems[1]["kind"], json!("burst"));
        assert_eq!(ems[1]["burst"], json!(-1));
        let text = d["text"].as_str().unwrap();
        assert!(text.contains("发射器 sparks") && text.contains("~30 粒子可见"), "{text}");
        assert!(text.contains("发射器 boom") && text.contains("未触发"), "{text}");
        // no emitters = no emitters key
        let d2 = describe_world(&world_one_red_sprite(), 64, 64).unwrap();
        assert!(d2.get("emitters").is_none());
    }

    #[test]
    fn emitter_seed_decorrelates_neighbor_entities() {
        let a = emitter_seed(vitric_ecs::EntityId { index: 1, generation: 1 });
        let b = emitter_seed(vitric_ecs::EntityId { index: 2, generation: 1 });
        assert_ne!(a, b);
        // same id always produces same seed (deterministic)
        assert_eq!(a, emitter_seed(vitric_ecs::EntityId { index: 1, generation: 1 }));
    }

    // ---- egocentric relations + primary/secondary sort (relative_to_focal) ----

    /// A world with a follow camera: hero(0,0) focal point + right neighbor coin(3,0) + offscreen far(-100,0).
    /// The camera is wide enough to bring coin into view (small scale).
    fn focal_world() -> World {
        let mut w = World::new();
        let hero = w.spawn_named("hero").unwrap();
        w.set_component(hero, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(hero, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000"})).unwrap();
        let coin = w.spawn_named("coin").unwrap();
        w.set_component(coin, "Position", json!({"x": 3.0, "y": 0.0})).unwrap();
        w.set_component(coin, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ffd84d"})).unwrap();
        let cam = w.spawn();
        // focal = follow points to hero; scale 1 pixel/unit → 64px viewport can see ±32 units
        w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 1.0, "follow": "hero"}))
            .unwrap();
        w
    }

    #[test]
    fn describe_attaches_relative_to_focal_for_neighbor() {
        let w = focal_world();
        let d = describe_world(&w, 64, 64).unwrap();
        let vis = d["visible"].as_array().unwrap();
        // coin is 3 units to the right of hero
        let coin = vis.iter().find(|e| e["name"] == json!("coin")).unwrap();
        let rel = &coin["relative_to_focal"];
        assert_eq!(rel["direction"], json!("right"), "coin 在 hero 右边");
        assert_eq!(rel["distance"], json!(3.0));
        assert_eq!(rel["same_row"], json!(true), "y 相同 → 同行");
        assert_eq!(rel["same_col"], json!(false));
    }

    #[test]
    fn describe_focal_entity_has_no_relative_block() {
        let w = focal_world();
        let d = describe_world(&w, 64, 64).unwrap();
        let vis = d["visible"].as_array().unwrap();
        let hero = vis.iter().find(|e| e["name"] == json!("hero")).unwrap();
        assert!(hero.get("relative_to_focal").is_none(), "焦点自己不输出 relative_to_focal");
    }

    #[test]
    fn describe_without_follow_has_no_relative_block() {
        // Backward compatibility: without Camera.follow the whole block is absent, output matches before this feature was added
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        // has camera but no follow (not following)
        w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0})).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        for e in d["visible"].as_array().unwrap() {
            assert!(e.get("relative_to_focal").is_none(), "无 follow 不该有 relative_to_focal");
        }
    }

    #[test]
    fn describe_offscreen_neighbor_relative_direction() {
        // Offscreen entities also carry relative_to_focal (relations don't distinguish on/off-screen)
        let mut w = focal_world();
        let up = w.spawn_named("moon").unwrap();
        w.set_component(up, "Position", json!({"x": 0.0, "y": 100.0})).unwrap();
        w.set_component(up, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ffffff"})).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        let moon = d["offscreen"].as_array().unwrap().iter()
            .find(|e| e["name"] == json!("moon")).unwrap();
        assert_eq!(moon["relative_to_focal"]["direction"], json!("up"), "y 向上 → 正上方");
        assert_eq!(moon["relative_to_focal"]["distance"], json!(100.0));
    }

    #[test]
    fn describe_primary_sort_named_then_distance() {
        // Primary/secondary sort: named first, then ascending by distance to focal point
        let mut w = focal_world(); // hero(0,0, focal point) + coin(3,0)
        // An unnamed near neighbor near(1,0): distance 1, but unnamed
        let near = w.spawn();
        w.set_component(near, "Position", json!({"x": 1.0, "y": 0.0})).unwrap();
        w.set_component(near, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#0000ff"})).unwrap();
        // A named far neighbor star(5,0): distance 5, named
        let star = w.spawn_named("star").unwrap();
        w.set_component(star, "Position", json!({"x": 5.0, "y": 0.0})).unwrap();
        w.set_component(star, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#00ff00"})).unwrap();

        let d = describe_world(&w, 64, 64).unwrap();
        let vis = d["visible"].as_array().unwrap();
        let names: Vec<String> = vis.iter()
            .map(|e| e.get("name").and_then(|n| n.as_str()).unwrap_or("<anon>").to_string())
            .collect();
        // Named ones first (hero distance 0, coin distance 3, star distance 5), unnamed near at the end
        assert_eq!(names, vec!["hero", "coin", "star", "<anon>"], "有名字优先、再按距离升序");
    }

    // ---- Line-of-sight occlusion (blocked) + ASCII grid map (ascii_map) ----

    #[test]
    fn describe_relative_carries_blocked_field() {
        // relative_to_focal now carries a blocked field: no wall → false
        let w = focal_world();
        let d = describe_world(&w, 64, 64).unwrap();
        let coin = d["visible"].as_array().unwrap().iter()
            .find(|e| e["name"] == json!("coin")).unwrap();
        assert_eq!(coin["relative_to_focal"]["blocked"], json!(false), "无墙不挡");
    }

    #[test]
    fn describe_blocked_true_when_wall_between() {
        // Stand a Solid wall(1.5,0) between focal hero(0,0) and coin(3,0) → blocked=true
        let mut w = focal_world();
        let wall = w.spawn();
        w.set_component(wall, "Position", json!({"x": 1.5, "y": 0.0})).unwrap();
        w.set_component(wall, "Collider", json!({"w": 0.5, "h": 2.0})).unwrap();
        w.set_component(wall, "Solid", json!({})).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        let coin = d["visible"].as_array().unwrap().iter()
            .find(|e| e["name"] == json!("coin")).unwrap();
        assert_eq!(coin["relative_to_focal"]["blocked"], json!(true), "中间有墙 → 挡");
    }

    #[test]
    fn describe_has_ascii_map_with_focus() {
        // With Camera.follow → top level has ascii_map, @ in the center
        let w = focal_world();
        let d = describe_world(&w, 64, 64).unwrap();
        let map = &d["ascii_map"];
        assert!(map.is_object(), "有焦点 → 有 ascii_map: {map:?}");
        let grid = map["grid"].as_array().unwrap();
        let center = grid.len() / 2;
        let mid_row = grid[center].as_str().unwrap();
        assert_eq!(mid_row.chars().nth(center), Some('@'), "@ 在正中");
        assert_eq!(map["focal_at"], json!([center, center]));
        // coin is in some cell to the right, included in the legend
        assert!(map["legend"].as_object().unwrap().values().any(|v| v == "coin"));
    }

    #[test]
    fn describe_no_ascii_map_without_follow() {
        // Backward compatibility: no follow → no ascii_map key
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0})).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert!(d.get("ascii_map").is_none(), "无 follow 不该有 ascii_map");
    }
}
