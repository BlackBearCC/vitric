//! vitric-render — 2D 光栅化。
//!
//! v0 是纯 CPU 渲染器：world → RGBA 像素 → PNG。
//! 看似保守，实则是闭环的关键一环：**截图不需要 GPU、不需要窗口、
//! 不需要图形会话**——agent 在任何无头环境都能「亲眼看到」游戏画面，
//! 而且同一世界状态渲出的像素逐字节相同（截图也可以进断言）。
//! GPU（wgpu）走的是同一个组件约定，后续替换呈现层不动游戏数据。
//!
//! 组件约定：
//! - `Sprite`  {"w": 数字, "h": 数字, "color": "#rrggbb", "rot": 度数} — 有它才会被画。
//!   rot 可选：绕 Position（精灵中心）旋转，**世界空间逆时针为正**——屏幕 y 翻转后
//!   画面上看同样是逆时针。缺省/0 走原始快路径，输出字节与没有该字段时逐位相同
//!   （向后兼容由测试锁死）。只转精灵：Text 永远直立；describe 的 overlaps 仍用
//!   未旋转尺寸的 AABB（近似，见 [`describe_world`] 内注释）
//! - `Position` {"x", "y"} — 世界坐标，y 向上
//! - `Camera` {"x", "y", "scale"} — 可选；取第一个，没有则原点、8 像素/单位
//! - `Shake` {"amplitude", "decay"} — 挂在相机实体上的屏幕抖动；amplitude > 0 时
//!   取景叠加确定性伪随机偏移（(tick, amplitude) 的纯函数，见 [`shake_offset`]）。
//!   偏移只作用于画面（render_world / GPU 路径 / 选中描边）——describe / pick /
//!   screen_to_world 读不抖的相机：语义观察和点选对的是世界本体，不是抖动后的画面
//! - `Text` {"content", "size", "color"} — 屏上文字，画在精灵之上、整串横向居中于
//!   Position。两条路径，由清单 `font` 字段二选一：
//!   * 默认（无 font）：内嵌 8x8 点阵（ASCII），每字符 size×size 世界单位、等宽、
//!     无抗锯齿——旧行为，输出字节由测试锁死不变；
//!   * 清单设了 `font`（TTF）：**所有** Text 改走矢量字体（[`font::FontStore`]），
//!     size = 字形总高（ascent-descent）的世界单位数，比例字距 + 字距调整，
//!     字体里有的字形都能画（含 CJK）。矢量文字是引擎里**唯一刻意平滑**的元素：
//!     覆盖率抗锯齿（像素风全程最近邻不变）。竖向把字身带居中到 Position。
//!     同平台同二进制仍逐字节确定（ab_glyph 纯 Rust 栅格化）
//! - `Ambient` {"color": "#rrggbb", "shadows": bool} — 场景环境光，挂任意实体（取第一个）。
//!   **光照的总开关**：场上没有 Ambient 实体 = 完全不跑光照（旧行为、零开销）；
//!   有 = 整帧（精灵/文字/背景一视同仁）按下面公式打光。
//!   `shadows` 可选（缺省 false）：**2D 投影的开关**——false/缺省时输出字节与该
//!   功能出现之前逐位相同（向后兼容由测试锁死）；true 时见下面"投影"一节
//! - `Light` {"radius", "color", "intensity", "kind", "angle", "dir"} — 光源，三种 kind
//!   （缺省 "point"，未知值显式报错）：
//!   * `"point"`（点光源，需要 `Position`）：radius 世界单位（到 radius 处衰减为零），
//!     color 缺省 "#ffffff"，intensity 缺省 1.0。**不写 kind 字段 = 点光源 = 旧行为，
//!     输出字节逐位不变（向后兼容由测试锁死）**
//!   * `"spot"`（聚光灯，需要 `Position`）：点光源的全部字段，外加必填 `angle`
//!     （锥角全宽，度数，1..=360）和必填 `dir`（朝向，度数，世界空间，0 = +x、
//!     逆时针为正——和 Sprite.rot 同一个角度约定）
//!   * `"directional"`（平行光）：必填 `dir`（光**行进**的方向，度数，约定同上）+
//!     color/intensity。不读 Position/radius——太阳在无穷远，处处同亮。没有法线的
//!     像素贡献处处均匀 = color·intensity（字节锁死的旧行为）；有法线的像素按 dir
//!     算方向（见下面"法线贴图"一节）。三种合计上限 64 盏
//! - `Bloom` {"threshold", "strength"} — 全屏泛光后效，挂任意实体（取第一个，同 Ambient）。
//!   **泛光的总开关**：场上没有 Bloom 实体 = 完全不跑泛光（旧行为字节不变、零开销）。
//!   threshold ∈ [0,1]：通道值超过 threshold·255 的部分才进泛光；strength ≥ 0：叠加倍率。
//!   两个字段都必填（缺了/不是数字显式报错，不静默给缺省值）
//! - `Emitter` — 粒子发射器（需要 `Position`）。**粒子是纯渲染层产物，不进模拟状态**：
//!   每个粒子在第 T tick 的位置/颜色/大小是
//!   `f(发射器字段, 粒子序号, T, 实体id派生的种子)` 的纯函数（[`emitter_particles`]）——
//!   无积分器（解析式 pos = origin + v0·t + ½g·t²）、无跨帧状态：不进状态哈希、不进存档，
//!   录像重放/快照回退后粒子画面自动正确。随机数用 SplitMix64 确定性散列
//!   （种子 = 实体id哈希 ⊕ 粒子序号，见 [`emitter_seed`]），与模拟 RNG 流完全无关。
//!   字段（语义源头 [`collect_emitters`]，缺/错显式报错）：
//!   * `kind`：必填，`"stream"`（持续流，按 `rate` 粒子/秒发射，发射时间轴从 tick 0 起算）
//!     或 `"burst"`（单次爆发：`burst` 字段 = 触发 tick 号，规则往里写当前 tick 即触发，
//!     `count` 个粒子同时出生；burst < 0 = 未触发。是否在爆发期由字段值纯函数推出，
//!     不需要记历史）
//!   * `lifetime`：必填，粒子寿命（tick，整数 ≥ 1）；`size`：必填，起始大小（世界单位 > 0）
//!   * `rate`（stream 必填 > 0）/ `count` + `burst`（burst 用，count ≥ 1，burst 缺省 -1）
//!   * `speed_min`/`speed_max`：初速范围（世界单位/秒，缺省 0；speed_max 缺省 = speed_min）
//!   * `dir`：发射朝向（度数，0 = +x、逆时针为正——和 Sprite.rot 同一约定，缺省 0）；
//!     `spread`：扩散角全宽（度数 0..=360，缺省 360 = 全方向，此时 dir 无所谓）
//!   * `gravity`：重力加速度（世界单位/秒²，y 轴，通常负数；缺省 0）
//!   * `color`/`color_end`：起始/结束颜色（"#rrggbb"；color_end 缺省/空串 = 不渐变）；
//!     alpha 随寿命线性淡出（内建，255 → 0）
//!   * `size_end`：结束大小（≥ 0，缺省 = size 不渐变；0 = 缩小到消失）
//!   * `active`：开关（bool，缺省 true）。false = 一个粒子都不画——**纯函数的取舍**：
//!     中途关掉会让在途粒子当帧消失（画面只看当前字段值，不记发射历史）
//!   * 渲染：粒子画成方点（与 GPU 路径的方块顶点几何一致），**自发光**：在光照之后、
//!     泛光之前画——不被环境光压暗、不被灯衰减、不投影也不受影（简化约定）；
//!     亮粒子照常进泛光。发射器实体如果移动，所有在途粒子整体跟着移
//!     （位置相对当前原点——无状态的代价）。场上没有 Emitter = 完全不画
//!     （旧行为字节不变、零开销）。上限 [`MAX_EMITTERS`] 个发射器、
//!     单个发射器同屏 [`MAX_PARTICLES_PER_EMITTER`] 粒子，超了显式报错
//!
//! 光照公式（CPU 与 GPU 路径必须一致，GPU 侧在 vitric-cli gpu.rs 的 WGSL 里）：
//!   lit = min(ambient + Σ 各灯贡献, 1.5)
//!   out = min(scene · lit, 1.0)
//! 各灯贡献：
//!   point:       color·intensity·(1 - d/r)²                       （d < r 才有贡献）
//!   spot:        color·intensity·(1 - d/r)²·t²，
//!                t = clamp(1 - Δθ/(angle/2), 0, 1)                 （角度衰减：锥心 1、锥边 0）
//!   directional: color·intensity                                    （处处均匀）
//! d 是像素到灯的距离（像素空间，取景用抖动后的相机——光跟着画面走）；
//! Δθ 是「灯指向像素的方向」与 dir 的夹角（度数语义，实现里用弧度 acos）。
//! 角度衰减刻意用 t²（不是 smoothstep 内建公式）——CPU/GPU 两侧必须镜像同一条式子。
//! 1.5 的上限允许轻微过曝（廉价的"泛光感"），乘回场景色后再夹回 1。
//!
//! 投影（`Ambient.shadows: true` 时启用，CPU 与 GPU 同一套几何）：
//! - 遮光体 = 带 `Solid` + `Position` + `Collider` 的实体——Solid 在物理里就是
//!   "挡"（挡停身体），开了投影后同一批实体顺便挡光，**零新增授权概念**。
//!   上限 [`MAX_OCCLUDERS`]（256）个，超了显式报错（不静默截断）。
//! - 逐像素逐灯：像素→灯心的线段与任何遮光体 AABB 相交（slab 法，
//!   见 [`segment_hits_aabb`]）就把这盏灯的贡献清零（硬影，无半影）。
//!   **例外：像素自己所在的遮光体不挡它**——不然每个 Solid 都把自己涂黑；
//!   规则是"箱子里的像素只被**别的**箱子遮挡"。
//! - 只有 point/spot 投影；directional 在 v1 不投影（太阳影需要按方向射线
//!   而不是点对点线段，留给后续版本），平行光贡献照旧处处均匀。
//! - 已知约束：灯心埋进某个遮光体时，箱外像素全部被那个箱子挡掉——
//!   灯别放在墙里。线段几何用取景后的像素空间（同灯参数，光跟着画面走）。
//! - 性能（输出字节不变，由等价性测试锁死）：边缘逐位贴齐的相邻遮光体每帧
//!   自动合并成大箱（瓦片地板收成一根长条，见 [`build_shadow_boxes`]）、再按
//!   灯盘逐灯剔除碰不到的箱子（[`cull_shadow_boxes`]）；点/聚光只扫自己灯盘的
//!   外接方框，贡献为零（锥外/背光面）的像素跳过遮挡测试。GPU 路径共用同一套
//!   合并/剔除结果，另有 uniform 预算：单灯剔除后 ≤ 64 箱、全部灯合计 ≤ 256 条
//!   （超了显式报错，见 gpu.rs）。
//!
//! 法线贴图（零配置命名配对，见 [`normal_map_name`]）：
//! - 精灵贴图 `hero.png` 在 assets/ 里有 `hero_n.png` 就自动启用——RGB 编码切线空间法线
//!   `n = rgb/255·2-1`，z 取绝对值（强制朝外）再归一化；零向量退化为平面 (0,0,1)。
//!   法线的 xy 轴对齐**屏幕像素空间**（x 向右、y 向下——图按 1:1 blit 时图轴即屏轴），
//!   `Sprite.rot` 旋转精灵时法线 xy 跟着同一矩阵转。
//! - 有法线的像素各灯贡献额外乘 `max(dot(N, L), 0)`。L 是像素指向灯的方向抬升成 3D：
//!   xy 取像素→灯心的单位方向乘 [`NORMAL_LIGHT_XY`]（0.8），z 固定 [`NORMAL_LIGHT_Z`]
//!   （0.6；0.8²+0.6²=1，天然单位长度）——平面法线 (0,0,1) 在灯正下也有 0.6 的贡献，
//!   不会"开了法线反而全黑"。像素正好在灯心（d=0）方向无定义，约定 L=(0,0,1)。
//!   平行光同构：L = (−行进方向单位向量·0.8, 0.6)——dir 自此参与计算，平行光有了方向感。
//! - **没有法线的像素走原公式，输出字节逐位不变**（向后兼容由测试锁死）。实现：光照开启时
//!   精灵 blit 顺手把法线写进一块每帧法线缓冲（哨兵零向量 = 没有法线；后画的精灵/文字
//!   覆盖像素时同步覆盖/清掉法线——盖住的像素属于上层那张图）。GPU 侧同一公式
//!   （法线贴图与普通图同住一张图集，顶点带第二组 UV，见 gpu.rs）。
//!
//! 泛光公式（CPU 是真相源——截图/断言以这条路径为准；GPU 侧求视觉一致，差异见 gpu.rs）：
//!   bright = max(scene - threshold·255, 0)       （逐通道提亮部）
//!   blurred = 盒式模糊(bright) 水平+垂直可分离，迭代 3 次（近似高斯）
//!   out = min(scene + blurred · strength, 255)    （加法合成）
//! 模糊半径 = [`bloom_radius_px`]：视口高/90、下限 2 像素——半径跟分辨率成比例，
//! 同一场景 4K 和 720p 的光晕占画面比例一致。泛光在光照**之后**跑（先打光再发光）。

mod assets;
mod font;
mod ui;
mod ui_interact;

pub use assets::{is_normal_map_name, normal_map_name, Assets, Image};
pub use font::{revealed_chars, FontStore, GlyphPlacement, RasterGlyph};
pub use ui::{
    has_ui, layout_input_hash, layout_runs, solve_layout, Align, Anchor, ContainerKind, Layout,
    UiRect, ALIGN_NAMES, ANCHOR_NAMES, CONTAINER_KINDS,
};
pub use ui_interact::{
    modulate_rgb, navigate, press_modulate, press_scale, ui_press_feedback, ButtonState, Dir,
    Focusable, BUTTON_STATES, PRESS_TICKS,
};

use serde_json::Value;

use vitric_ecs::World;

/// 点光源数量上限。逐像素（CPU）/逐片元（GPU uniform 数组）都要遍历全部灯，
/// 不设上限会把两条路径同时拖死；超了显式报错，不静默截断。
pub const MAX_LIGHTS: usize = 64;

/// 光照亮度上限：ambient + 各灯贡献之和每通道夹在这里（见模块文档的公式）。
pub const LIGHT_CLAMP: f64 = 1.5;

/// 遮光体数量上限（投影开启时）。逐像素逐灯都要扫全部遮光体——CPU 内循环和
/// GPU uniform 数组（256 × vec4 = 4KB）同时受制于它；超了显式报错，不静默截断。
pub const MAX_OCCLUDERS: usize = 256;

/// 法线光照的光方向 z 抬升（固定值，见模块文档）：L.z = 0.6，xy 占 0.8——
/// 单位长度由构造保证。0.6 是审美选择：平面像素在灯正下仍有六成贡献，浮雕感和
/// "别把画面压黑"之间的折中。CPU/GPU 两侧必须同值（gpu.rs WGSL 里硬编码并注明出处）。
pub const NORMAL_LIGHT_Z: f64 = 0.6;

/// 法线光照的光方向 xy 系数：√(1 − 0.6²) = 0.8（与 [`NORMAL_LIGHT_Z`] 配对成单位向量）。
pub const NORMAL_LIGHT_XY: f64 = 0.8;

/// 粒子发射器数量上限。每帧每个发射器都要展开粒子，CPU 光栅化和 GPU 顶点流
/// 同时受制于它；超了显式报错，不静默截断。
pub const MAX_EMITTERS: usize = 64;

/// 单个发射器的同屏粒子预算（stream 按 rate·lifetime 估算、burst 按 count）。
/// 在 [`collect_emitters`] 里校验——超了显式报错（调低 rate/count 或缩短 lifetime），
/// 不静默丢粒子。
pub const MAX_PARTICLES_PER_EMITTER: usize = 1024;

/// 粒子时间换算用的模拟频率（tick/秒）。**必须与 vitric-sim 的 `TICKS_PER_SECOND`
/// 同值**（render 不依赖 sim，常量各自一份；一致性由 vitric-cli 的跨 crate 测试锁死）。
/// rate（粒子/秒）和初速/重力（世界单位/秒）都按它换算到 tick。
pub const PARTICLE_TICKS_PER_SECOND: f64 = 60.0;

/// 清屏背景色：深灰蓝，区别于纯黑（纯黑常被误判为「没渲出来」）。
/// GPU 路径的清屏/背景方块也用它——两条路径背景同字节。
pub const BACKGROUND: [u8; 4] = [24, 26, 33, 255];

/// 文字可读性的对比度下限（WCAG 式比值 `(L1+0.05)/(L2+0.05)`，L 为相对亮度）。
/// 低于它 describe 给 `low-contrast-text` 警告。WCAG AA 正文要求 4.5、大字 3.0；
/// 这里取 2.5 是刻意放宽——这是给 AI 开发者的"人眼基本读不出来"红线，
/// 不是无障碍合规检查（误报会让 agent 学会忽略警告，比漏报更糟）。
pub const TEXT_CONTRAST_MIN: f64 = 2.5;

/// 渲染一帧：返回 RGBA8 像素（行优先，左上原点）。
/// `tick` 只喂给屏幕抖动（[`camera_of`]）——同一世界同一 tick 渲出的字节逐位相同。
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

/// 内部渲染变体的开关（对外不暴露——公开 API 只有一种"正常渲染"）。
/// 存在的唯一理由是 [`describe_world`] 的文字对比度测量：要拿到"这条文字**不画**时
/// 它脚下的底色"，又不能因为手头没有素材就整帧报错。
#[derive(Default)]
struct RenderOpts {
    /// 跳过这一个 Text 实体不画（测它脚下的底色）。`None` = 正常画全部文字。
    skip_text: Option<vitric_ecs::EntityId>,
    /// 宽容贴图模式：`Sprite.image` 不在素材仓库时退化成 `Sprite.color` 纯色块
    /// （亮度近似），而不是报错。**只给对比度测量用**——正常渲染（false）保持
    /// "图不存在直接报错"的约定，缺图绝不静默画占位。
    lenient_images: bool,
}

/// 渲染主体（相机已定）。[`render_world`] 用缺省 opts 走到这里——
/// 正常渲染路径的算术与重构前逐字节相同（向后兼容由测试锁死）。
/// `tick` 只喂给粒子展开（[`emitter_particles`]，粒子是 tick 的纯函数）。
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

    // 法线缓冲（每帧）：光照开着、且素材仓库里真有法线贴图才分配——否则零分配
    // 零开销（None 与"有缓冲但全哨兵"输出逐位相同，旧行为字节不变）。
    // 哨兵零向量 = 该像素没有法线（走原光照公式，字节锁死）；精灵 blit 时顺手填充，
    // 后画的东西覆盖像素就覆盖/清掉法线（盖住的像素属于上层那张图）。
    let ambient = ambient_of(world)?;
    let mut normals: Option<Vec<[f32; 3]>> = ambient
        .as_ref()
        .filter(|_| assets.has_normal_maps())
        .map(|_| vec![[0.0f32; 3]; (width * height) as usize]);

    // 按实体序绘制（确定性；后画的盖前画的）
    for id in world.query(&["Position", "Sprite"]) {
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        let sw = num(world, id, "Sprite.w")?;
        let sh = num(world, id, "Sprite.h")?;
        let rot = rot_of(world, id)?;

        // 世界 → 屏幕（y 翻转，相机居中）
        let cx = (width as f64) / 2.0 + (px - cam_x) * scale;
        let cy = (height as f64) / 2.0 - (py - cam_y) * scale;
        let half_w = sw * scale / 2.0;
        let half_h = sh * scale / 2.0;

        let mut image_name = world
            .get_field(id, "Sprite.image")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        // 宽容贴图模式（只在对比度测量的内部渲染开）：图不在素材仓库就当纯色块画
        // （Sprite.color，缺省白）——拿不到真像素时用色块近似亮度，总比整帧报错强。
        // 正常渲染不走这行，缺图照旧显式报错。
        if opts.lenient_images && !image_name.is_empty() && assets.image(&image_name).is_none() {
            image_name = String::new();
        }

        if rot == 0.0 {
            // —— 快路径：不旋转（rot 缺省/为 0）。这段逻辑不许动——
            //    输出字节必须与 rot 字段出现之前逐位相同（向后兼容由测试锁死）
            let x0 = (cx - half_w).floor().max(0.0) as i64;
            let x1 = (cx + half_w).ceil().min(width as f64) as i64;
            let y0 = (cy - half_h).floor().max(0.0) as i64;
            let y1 = (cy + half_h).ceil().min(height as f64) as i64;
            if image_name.is_empty() {
                // 纯色块
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
                        // 纯色块没有法线：清掉底下可能残留的法线（哨兵零向量）
                        if let Some(ns) = normals.as_mut() {
                            ns[i / 4] = [0.0; 3];
                        }
                    }
                }
            } else {
                // 贴图：最近邻缩放 + alpha 混合。图不存在直接报错（不画占位符）。
                let img = image_of(assets, id, &image_name)?;
                // 法线贴图按命名配对（hero.png → hero_n.png）；没配对 = 像素清法线
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
                            // 用与贴图同一个 (u,v) 采样法线——不旋转时 sn=0/cs=1
                            ns[i / 4] = match nmap {
                                Some(m) => sample_normal(m, u, v, 0.0, 1.0),
                                None => [0.0; 3],
                            };
                        }
                    }
                }
            }
        } else {
            // —— 旋转路径：扫旋转后四角的轴对齐包围盒，逐像素逆旋回精灵局部空间再采样。
            // 角度约定见 [`rot_of`]；f64 三角函数依赖系统数学库——确定性边界与文档一致：
            // 同平台同二进制逐字节保证，跨平台末位不保证。
            // 世界逆时针 + 屏幕 y 翻转 → 屏幕系正向矩阵 [[c, s], [-s, c]]，逆变换取转置
            let (sn, cs) = rot.to_radians().sin_cos();
            let ext_x = half_w * cs.abs() + half_h * sn.abs();
            let ext_y = half_w * sn.abs() + half_h * cs.abs();
            let x0 = (cx - ext_x).floor().max(0.0) as i64;
            let x1 = (cx + ext_x).ceil().min(width as f64) as i64;
            let y0 = (cy - ext_y).floor().max(0.0) as i64;
            let y1 = (cy + ext_y).ceil().min(height as f64) as i64;
            if image_name.is_empty() {
                // 纯色块（旋转）：像素中心落在精灵内才上色
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
                            continue; // 包围盒里但精灵外
                        }
                        let i = ((y as u32 * width + x as u32) * 4) as usize;
                        buf[i..i + 4].copy_from_slice(&rgba);
                        if let Some(ns) = normals.as_mut() {
                            ns[i / 4] = [0.0; 3];
                        }
                    }
                }
            } else {
                // 贴图（旋转）：局部坐标直接当 UV 用，采样逻辑与快路径一致（最近邻 + alpha 混合）
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
                            // 法线跟着精灵转：传入旋转矩阵的 sin/cos（局部→屏幕）
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

    // 光照按 Ambient 实体的存在与否开关：没有 = 完全跳过（旧行为字节不变、零开销）。
    // 有 = 整帧打光——精灵/文字/背景一视同仁，HUD 想保持可读自己在旁边放盏灯
    if let Some((ambient, _)) = ambient {
        let lights = collect_lights(world)?;
        // 遮光体只在投影开启时收集——关着（缺省）传空列表，逐像素循环里
        // 空列表分支不改任何算术，输出字节与投影功能出现之前逐位相同（测试锁死）
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

    // 粒子在光照之后、泛光之前画——自发光（不被环境光压暗/灯衰减/投影），
    // 亮粒子照常进泛光晕开。场上没有 Emitter = 零成本跳过（旧行为字节不变）
    draw_particles(world, &mut buf, width, height, (cam_x, cam_y, scale), tick)?;

    // 泛光按 Bloom 实体的存在与否开关：没有 = 完全跳过（旧行为字节不变、零开销）。
    // 在光照之后跑——亮部是打完光的亮部，灯照亮的东西才会晕开
    if let Some(bloom) = bloom_of(world)? {
        apply_bloom(&mut buf, width, height, &bloom);
    }

    // UI 屏幕空间叠加层：紧接世界渲染（含光照/粒子/泛光）之后画，**不经相机变换**
    // ——镜头移动/缩放/抖动 UI 不飘（像 HUD）。屏幕空间正交投影 = 直接用 layout
    // 算出的屏幕像素矩形落笔，无离屏缓冲、复用同一块 buf。场上没有 UI（无 UiRoot）
    // = 零成本跳过（旧行为字节不变）。
    draw_ui(world, &mut buf, width, height, assets)?;
    Ok(buf)
}

/// 场景环境光：取第一个带 `Ambient` 组件的实体，返回 (0..1 通道值, 原始色串)。
/// `None` = 场上没有 Ambient = 光照整体关闭（这是约定的总开关，不是缺省白光）。
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

/// 投影开关：第一个 `Ambient` 实体上的可选 `shadows` 字段（bool）。
/// 缺省 false = 不投影 = 旧行为字节不变（向后兼容由测试锁死）；
/// 字段在但不是 bool → 显式报错（写错类型静默当 false 比报错更难排查）。
/// 场上没有 Ambient（光照整体关闭）时恒为 false。
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

/// 一个遮光体（`Solid` + `Position` + `Collider` 实体的解析结果，世界坐标）：
/// 中心 (x, y) + 宽高 (w, h) 的 AABB——和物理挡停用的是同一个碰撞盒，
/// "挡身体的东西就挡光"，不引入第二套遮挡数据。
pub struct Occluder {
    pub id: vitric_ecs::EntityId,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 收集场上全部遮光体（带 Solid+Position+Collider 的实体，槽位序）。
/// 只在投影开启（[`shadows_of`] 为 true）时调用；超过 [`MAX_OCCLUDERS`] 显式报错。
/// 字段校验全在这里做（Collider.w/h、Position.x/y 必须是数字），热路径只剩纯算术。
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

/// 线段 (px,py)→(qx,qy) 与 AABB [x0..x1, y0..y1] 相交判定（slab 法）。
/// 轴平行（分量差 < 1e-12）的轴退化成"起点必须落在该轴 slab 内"，不做除法——
/// 除以接近零的数会算出 ±inf，min/max 链上 inf 的语义在 CPU(f64) 和 GPU(f32)
/// 不保证一致，显式分支两侧才镜像得起来。GPU 侧（gpu.rs WGSL 的 shadowed）逐句同构。
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

/// 一个合并后的遮挡大箱（像素空间）。`aabb` 是成员子箱像素 AABB 的逐分量 min/max
/// （**不是**世界合并框再变换——子箱各自走原变换表达式，min/max 保证大箱边界与
/// 子箱边界逐位贴齐，外测命中才与逐箱命中逐位等价）。
pub struct MergedOccluder {
    /// [x0, y0, x1, y1]，像素空间。
    pub aabb: [f64; 4],
    /// 在 [`ShadowBoxes::subs`] 里的起点。
    pub sub_start: usize,
    /// 成员子箱数。
    pub sub_len: usize,
}

/// 投影遮挡的逐帧加速结构（像素空间）：相邻齐边的遮光体合并成大箱，
/// 大箱外的像素只测大箱；大箱内的像素回落到原始子箱（"自己所在的箱子不挡自己"
/// 按原始实体逐个判——合并不改这条规则的语义）。见 [`build_shadow_boxes`]。
pub struct ShadowBoxes {
    pub merged: Vec<MergedOccluder>,
    /// 原始遮光体的像素空间 AABB [x0, y0, x1, y1]，按 merged 分组重排成连续区间。
    /// 每个箱子的数值与逐箱路径同一条变换表达式——逐位相同。
    pub subs: Vec<[f64; 4]>,
}

/// 把遮光体合并成大箱并变换到像素空间（每帧一次；CPU 逐像素路径和 GPU uniform
/// 打包共用，语义源头在这里）。
///
/// 合并规则：两轮贪心 1D 合并——先沿 x（同 y 区间的瓦片行收成横条），再沿 y
/// （同 x 区间的横条摞成大块）。只在**世界空间边缘 f64 逐位相等**（贴齐无缝）
/// 且两侧都是规整箱（w/h > 0）时合并：并集 == 大箱时阴影几何一寸不变，
/// 带容差的"差不多贴齐"会让并集 ≠ 大箱、阴影字节漂移，所以不做。
/// 排序键全走 `total_cmp` + 原槽位——结果与输入顺序无关，逐帧确定。
///
/// 等价性（合并前后输出字节逐位相同，由测试锁死）：箱外像素对大箱的 slab 命中
/// == 对成员子箱逐个 slab 命中取或——前提是子箱像素边缘逐位共享（贴齐的世界边缘
/// 经同一条变换得到同一个 f64；瓦片坐标是二进制可精确表示的常见情形）。
/// 箱内像素不走大箱，直接按原始子箱逐个判，与未合并路径同一条算式。
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
    // 世界空间边缘（只用于合并判定；像素变换始终按原始 Occluder 走原表达式）
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
    // 贪心 1D 合并：排序后线性扫描，与队尾贴齐就并入（队尾边界随合并延伸，长链一次收完）
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
                // 退化/反写箱（w/h ≤ 0）不参与合并：各自成组 = 原行为原样保留
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
        // 单成员组的 aabb 就是子箱本身（min/max 各取一侧，反写箱也原样保留）
        let mut aabb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for &i in &g.members {
            let o = &occluders[i];
            // 与逐箱路径同一条变换表达式（取景含抖动，光跟画面走）——逐位相同
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

/// 这盏灯的遮挡候选：圆盘（灯心 (lx,ly)、半径 r 像素）碰得到的大箱下标。
/// 剔除是**无损**的：像素只在 d² < r² 时才做遮挡测试，线段两端（像素、灯心）
/// 都在灯盘里，圆盘是凸的 → 线段整条在灯盘里 → 灯盘碰不到的箱子永远不会被命中。
/// CPU 逐像素路径和 GPU uniform 打包共用（语义源头在这里）。
pub fn cull_shadow_boxes(boxes: &ShadowBoxes, lx: f64, ly: f64, r: f64) -> Vec<u32> {
    let r2 = r * r;
    boxes
        .merged
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            // slab 法自带区间归一（t1/t2 取 min/max），反写箱命中行为等同归一后的箱——
            // 最近点也按归一后的边算，剔除判定与命中判定才对得上
            let (x0, x1) = (m.aabb[0].min(m.aabb[2]), m.aabb[0].max(m.aabb[2]));
            let (y0, y1) = (m.aabb[1].min(m.aabb[3]), m.aabb[1].max(m.aabb[3]));
            let dx = lx - lx.clamp(x0, x1);
            let dy = ly - ly.clamp(y0, y1);
            dx * dx + dy * dy <= r2
        })
        .map(|(i, _)| i as u32)
        .collect()
}

/// 光源种类（`Light.kind` 的解析结果）。角度字段全部是**度数**、世界空间、
/// 0 = +x、逆时针为正——和 `Sprite.rot` 同一个约定（语义源头见 [`rot_of`]）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// 点光源（kind 缺省）。
    Point,
    /// 聚光灯：`angle` = 锥角全宽（1..=360），`dir` = 朝向。
    Spot { angle: f64, dir: f64 },
    /// 平行光：`dir` = 光行进的方向。没有法线的像素贡献处处均匀（= color·intensity，
    /// 旧行为字节不变）；有法线的像素按 dir 算 `max(dot(N, L), 0)`（见模块文档）。
    Directional { dir: f64 },
}

impl LightKind {
    /// describe / 错误信息里的字符串名（和 `Light.kind` 的合法取值一致）。
    pub fn name(&self) -> &'static str {
        match self {
            LightKind::Point => "point",
            LightKind::Spot { .. } => "spot",
            LightKind::Directional { .. } => "directional",
        }
    }
}

/// 一盏光源（`Light` 实体的解析结果，世界坐标）。
pub struct LightSource {
    pub id: vitric_ecs::EntityId,
    pub name: Option<String>,
    /// 世界坐标。平行光不读 Position，恒为 0（占位，不参与计算）。
    pub x: f64,
    pub y: f64,
    /// 世界单位；到 radius 处光衰减为零。平行光不读 radius，恒为 0（占位）。
    pub radius: f64,
    pub intensity: f64,
    /// 原始色串（describe 输出用）。
    pub color: String,
    /// color 解析后的 0..1 通道值（未乘 intensity）。
    pub rgb: [f64; 3],
    pub kind: LightKind,
}

/// 收集场上全部光源（带 `Light` 组件的实体，槽位序）。超过 [`MAX_LIGHTS`] 直接报错
/// （三种 kind 合计——逐像素/逐片元都要遍历全部灯，平行光也不豁免）。
/// 校验全在这里做：kind 合法性、point/spot 必须有 Position、spot 的 angle/dir、
/// directional 的 dir——渲染热路径里只剩纯算术。
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
            // kind：可选文本字段，缺省 "point"（旧场景没有这个字段，行为必须不变）
            let kind_str = match world.get_field(id, "Light.kind") {
                Err(_) => "point".to_string(),
                Ok(v) => v.as_str().map(String::from).ok_or_else(|| {
                    format!(
                        "实体 {id} 的 Light.kind 不是文本: {v}。\
                         可选: \"point\"（点光源，缺省）/ \"spot\"（聚光灯）/ \"directional\"（平行光）"
                    )
                })?,
            };
            // spot/directional 的必填角度字段（度数）；缺了给写法提示
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
            // 平行光不读 Position/radius（太阳在无穷远）；point/spot 必须有
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

/// 逐像素打光（CPU 路径）。公式见模块文档；GPU 侧（gpu.rs 的 WGSL）必须保持同一公式、
/// 同一顺序（在 sRGB 字节空间上乘）。kind 分支全部在逐像素循环**外**做掉：
/// - 平行光处处均匀 → 一帧折进基底色一次（`base = ambient + Σ directional`），内循环零成本；
/// - point/spot 分进两个独立列表——纯点光场景的内循环和加 kind 之前逐条指令相同
///   （字节级向后兼容由测试锁死），聚光灯才多付角度衰减的钱。
///
/// 灯参数先变换到像素空间，点光内循环只剩距离平方比较。
///
/// `normals`：每帧法线缓冲（哨兵零向量 = 没有法线）。有法线的像素各灯贡献额外乘
/// `max(dot(N, L), 0)`，平行光也按 dir 算方向（不再折进基底）；哨兵像素走上面那条
/// 老路径，**输出字节逐位不变**。`None` = 整帧没有法线（等价全哨兵，但少一次查表）。
///
/// `occluders`：投影的遮光体（空 = 不投影 = 算术逐位不变）。point/spot 在距离判定
/// 通过、贡献非零后再做线段遮挡测试；遮光体先合并成大箱（[`build_shadow_boxes`]）、
/// 再按灯盘逐灯剔除（[`cull_shadow_boxes`]）——两步都不改输出字节（测试锁死）。
/// directional 不投影（v1，见模块文档）。
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

/// [`apply_lighting`] 的主体，遮挡结构由调用方给（`cull=false` = 不做逐灯剔除，
/// 全量候选——等价性测试的对照路径，正常渲染恒走 `cull=true`）。
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
        /// 已乘 intensity 的通道值。
        rgb: [f64; 3],
    }
    struct PxSpot {
        base: PxLight,
        /// 朝向的**像素空间**单位向量（世界 dir 度数 → (cos, -sin)，y 翻转）。
        dir: [f64; 2],
        /// 半锥角（弧度）。collect_lights 保证 angle ∈ 1..=360 → half > 0，除法安全。
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
    /// 平行光在法线路径的预计算：L = (−行进方向单位向量·0.8, 0.6)（单位长度由构造保证）。
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
            // 平行光：哨兵像素的贡献 = color·intensity 处处相同 → 折进基底，逐像素不再付钱；
            // 法线像素要按 dir 算 max(dot(N,L),0) → 另存一份带 L 的（行进方向像素空间
            // (cos,-sin)，指向灯 = 取反，再按 0.8/0.6 抬升成单位向量）
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

    // 每盏灯的遮挡候选（merged 大箱下标）：剔除灯盘碰不到的箱子（无损，
    // 见 cull_shadow_boxes）。投影关闭时 grid 为空——候选全空，老路径算术一字不动。
    let light_boxes = |l: &PxLight| -> Vec<u32> {
        if cull {
            cull_shadow_boxes(grid, l.x, l.y, l.r)
        } else {
            (0..grid.merged.len() as u32).collect()
        }
    };
    let point_boxes: Vec<Vec<u32>> = points.iter().map(light_boxes).collect();
    let spot_boxes: Vec<Vec<u32>> = spots.iter().map(|s| light_boxes(&s.base)).collect();
    // 像素 (fx,fy) 到灯心 (lx,ly) 的线段是否被某个候选箱挡住。
    // 大箱外的像素只测大箱（并集 == 大箱，命中逐位等价，见 build_shadow_boxes）；
    // 大箱内的像素回落到原始子箱——像素自己所在的箱子跳过（规则：箱子里的像素
    // 只被别的箱子遮挡，不自压成黑块），合并不改这条规则的字节语义。
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

    // —— 逐灯有界扫描。重构前是逐像素扫全部灯（每帧 像素数×灯数 次距离判定），
    //    重构后点/聚光只扫自己灯盘的外接方框——方框外的像素连距离都不用算。
    //    字节等价的根据：每个像素收到的 f64 加法**序列**不变——
    //    初始化（ambient/基底 + 平行光）→ 点光（槽位序）→ 聚光（槽位序）→
    //    最后统一夹紧乘回；方框外像素在旧代码里走 d²≥r² 的 continue（不加），
    //    新代码干脆不访问（同样不加）。锁死的光照/投影/法线测试全部盖住这一步。

    // 像素指向灯的单位方向 → xy·0.8 + z 0.6；d=0 方向无定义，约定 (0,0,1)
    fn lambert(n: [f64; 3], dx: f64, dy: f64, d: f64) -> f64 {
        let l = if d > 0.0 {
            [-dx / d * NORMAL_LIGHT_XY, -dy / d * NORMAL_LIGHT_XY, NORMAL_LIGHT_Z]
        } else {
            [0.0, 0.0, 1.0]
        };
        (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0)
    }
    // 该像素的法线（哨兵零向量 = 没有 → None，走老路径——字节锁死）
    let normal_at = |i: usize| -> Option<[f64; 3]> {
        normals
            .map(|ns| ns[i])
            .filter(|n| n[2] != 0.0)
            .map(|n| [n[0] as f64, n[1] as f64, n[2] as f64])
    };
    // 行级候选过滤：slab 法的 y 区间只依赖 (fy, ly, 箱子 y 边)——整行是常量。
    // y 区间已空的箱子，这一行的任何像素都不可能命中（x 轴只会让区间更紧），
    // 用与 segment_hits_aabb **完全相同的算式**预判，过滤逐位无损；子箱的 y 边
    // 被大箱的 min/max 夹住，大箱区间空 ⇒ 子箱区间更空，箱内回落路径同样无损。
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
    // 行级候选的复用暂存（避免逐行分配）
    let mut row_cand: Vec<u32> = Vec::new();

    // 灯盘外接方框（裁到视口；±r 外 d²≥r² 恒不过判定，1px 余量盖住浮点边缘）
    let light_rect = |lx: f64, ly: f64, r: f64| -> (u32, u32, u32, u32) {
        (
            ((lx - r - 1.5).floor().max(0.0) as u32).min(width),
            ((lx + r + 1.5).ceil().max(0.0) as u32).min(width),
            ((ly - r - 1.5).floor().max(0.0) as u32).min(height),
            ((ly + r + 1.5).ceil().max(0.0) as u32).min(height),
        )
    };

    // —— 灯光累加缓冲只开"灯碰得到"的包围矩形（全部灯盘方框的并集外接框）那么大：
    //    远离所有灯的像素整帧不分配不访问，合成时直接走未触碰路径。
    //    touched = 该像素收过点/聚光贡献（累加值已从起点分化）。
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
    // 帧像素 (x,y) → 累加缓冲下标（只在某盏灯的方框内调用，必在并集框内）
    let local = |x: u32, y: u32| ((y - uy0) * uw + (x - ux0)) as usize;

    // 像素的光照累加**起点**（第一次收到灯光贡献时才算；没收过的合成时现算同一条
    // 式子）：法线像素 = ambient + 平行光按方向逐盏算（L 的构造见模块文档）；
    // 哨兵像素 = 基底（平行光已折进去）
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

    // 点光 pass（槽位序——每像素的累加顺序与逐像素全扫描一致）
    for (l, lb) in points.iter().zip(&point_boxes) {
        let (x0, x1, y0, y1) = light_rect(l.x, l.y, l.r);
        for y in y0..y1 {
            let fy = y as f64 + 0.5; // 像素中心——GPU 片元的 @builtin(position) 也是中心坐标
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
                    // 法线像素：贡献 ×= max(dot(N, L), 0)
                    Some(n) => f * f * lambert(n, dx, dy, d),
                    // 老路径：这条算式不许动（字节锁死）
                    None => f * f,
                };
                // 贡献为零（背光面 / d 经舍入顶到 r）加不加都一样（+0.0 逐位
                // 无操作）——先判零再做遮挡测试，零贡献像素一个箱子都不用碰
                if f == 0.0 {
                    continue;
                }
                // 投影：被遮光体挡住 = 这盏灯对该像素零贡献（硬影）；
                // 候选全空时零成本短路——投影关闭的老路径字节锁死不受影响
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

    // 聚光 pass（在点光之后——每像素仍是先点光后聚光，槽位序）
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
                // 角度衰减（公式见模块文档，GPU 侧逐句镜像）：
                //   Δθ = acos(像素方向 · 朝向)，t = clamp(1 - Δθ/half, 0, 1)，贡献 ×= t²
                // d=0（像素正好在灯心）夹角无定义，约定取锥心（t=1）
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
                // 锥外/背光面贡献为零：跳过遮挡测试（+0.0 逐位无操作）——
                // 聚光锥只盖灯盘的一角，锥外像素一个箱子都不用碰
                if f == 0.0 {
                    continue;
                }
                // 投影：聚光灯同点光源一样被遮（锥角衰减不豁免遮挡）
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

    // 合成 pass：夹紧、乘回 sRGB 字节（与重构前同一条算式，alpha 不动）。
    // 没收过灯光贡献的哨兵像素（累加值恒 = 基底）走 256 项查表——表项用同一条
    // 表达式逐档预算，输出字节与逐像素现算逐位相同；没收过贡献的法线像素现算
    // 起点（与 init_lit 同一条式子）再乘回。
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

/// 泛光参数（`Bloom` 组件的解析结果）。
pub struct BloomParams {
    /// 0..=1：通道值超过 threshold·255 的部分进泛光。
    pub threshold: f64,
    /// ≥ 0：模糊后的亮部按这个倍率加回场景。
    pub strength: f64,
}

/// 泛光设置：取第一个带 `Bloom` 组件的实体（同 Ambient/Camera 的约定）。
/// `None` = 场上没有 Bloom = 泛光整体关闭（总开关，不是缺省参数）。
/// 字段缺失/不是数字/越界都显式报错——后效参数写错了静默跳过比报错更难排查。
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

/// 泛光模糊半径（像素）：视口高/90、下限 2——跟分辨率成比例，光晕占画面比例
/// 与分辨率无关。CPU 全分辨率模糊直接用这个值；GPU 半分辨率 ping-pong 用它的一半
/// （见 gpu.rs，语义源头在这里）。
pub fn bloom_radius_px(viewport_h: u32) -> u32 {
    (viewport_h / 90).max(2)
}

/// 全屏泛光后效（CPU 路径，公式见模块文档）。确定性：纯 f32 算术、固定遍历顺序、
/// 无并行——同一输入逐字节同输出。效率：可分离盒式模糊（每像素每方向只加减 2 次的
/// 滑动窗口）、亮部/暂存两个平面共享一次分配。
fn apply_bloom(buf: &mut [u8], width: u32, height: u32, bloom: &BloomParams) {
    let (w, h) = (width as usize, height as usize);
    let n = w * h;
    let thr = (bloom.threshold * 255.0) as f32;

    // 一次分配：前半 = 亮部平面（RGB f32），后半 = 模糊暂存
    let mut planes = vec![0f32; n * 3 * 2];
    let (a, b) = planes.split_at_mut(n * 3);
    for i in 0..n {
        for c in 0..3 {
            a[i * 3 + c] = (buf[i * 4 + c] as f32 - thr).max(0.0);
        }
    }

    // 3 次迭代的可分离盒式模糊（H 写进暂存、V 写回亮部平面），近似高斯
    let r = bloom_radius_px(height) as usize;
    for _ in 0..3 {
        box_blur_pass(a, b, w, h, r, true);
        box_blur_pass(b, a, w, h, r, false);
    }

    // 加法合成：out = min(scene + blurred·strength, 255)
    let s = bloom.strength as f32;
    for i in 0..n {
        for c in 0..3 {
            let v = buf[i * 4 + c] as f32 + a[i * 3 + c] * s;
            buf[i * 4 + c] = v.min(255.0) as u8;
        }
    }
}

/// 盒式模糊单方向一趟（`horizontal` 选轴）：窗口 2r+1，越界采样取边缘像素
/// （clamp-to-edge，GPU 侧 WGSL 的 clamp 同语义）。滑动窗口：每步加新减旧，
/// f32 累加顺序固定 → 确定性。
fn box_blur_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let norm = 1.0 / (2 * r + 1) as f32;
    // 统一成"沿 len 轴扫，跨 lanes 条线"：水平 = 每行一条线（步长 3），
    // 垂直 = 每列一条线（步长 3w）
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
            // 起始窗口：样本 -r..=r（越界 clamp 到边缘）
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

/// 发射形态（`Emitter.kind` 的解析结果）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmitterKind {
    /// 持续流：每秒 `rate` 个粒子，发射时间轴从 tick 0 起算。
    Stream { rate: f64 },
    /// 单次爆发：`burst` = 触发 tick 号（负数 = 未触发），`count` 个粒子同时出生。
    Burst { count: i64, burst: i64 },
}

impl EmitterKind {
    /// describe / 错误信息里的字符串名（和 `Emitter.kind` 的合法取值一致）。
    pub fn name(&self) -> &'static str {
        match self {
            EmitterKind::Stream { .. } => "stream",
            EmitterKind::Burst { .. } => "burst",
        }
    }
}

/// 一个粒子发射器（`Emitter` 实体的解析结果，世界坐标）。字段语义见模块文档；
/// 校验全在 [`collect_emitters`]，粒子展开（[`emitter_particles`]）只剩纯算术。
#[derive(Debug)]
pub struct EmitterSource {
    pub id: vitric_ecs::EntityId,
    pub name: Option<String>,
    /// 发射原点（当前 Position——发射器移动时在途粒子整体跟着移，见模块文档）。
    pub x: f64,
    pub y: f64,
    pub kind: EmitterKind,
    /// 粒子寿命（tick，≥ 1）。
    pub lifetime: i64,
    /// 初速范围（世界单位/秒，0 ≤ min ≤ max）。
    pub speed_min: f64,
    pub speed_max: f64,
    /// 发射朝向（度数，0 = +x、逆时针为正）+ 扩散角全宽（度数 0..=360）。
    pub dir: f64,
    pub spread: f64,
    /// 重力加速度（世界单位/秒²，y 轴）。
    pub gravity: f64,
    /// 起始/结束颜色（0..=255 通道值；rgb_end 缺省 = rgb 不渐变）+ 原始色串（describe 用）。
    pub rgb: [f64; 3],
    pub rgb_end: [f64; 3],
    pub color: String,
    /// 起始/结束大小（世界单位；size_end 缺省 = size 不渐变）。
    pub size: f64,
    pub size_end: f64,
    pub active: bool,
    /// 实体 id 派生的散列种子（[`emitter_seed`]）。
    pub seed: u64,
}

/// SplitMix64 终混器：64 位打散，无状态纯函数。粒子散列和屏幕抖动
/// （[`shake_offset`]）共用这一个——确定性装饰层的统一随机源，不碰模拟 RNG 流。
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 发射器实体 id → 散列种子。index/generation 拼 64 位再过 SplitMix64——
/// 两个发射器哪怕槽位相邻，粒子轨迹也互不相似。
pub fn emitter_seed(id: vitric_ecs::EntityId) -> u64 {
    mix64(((id.index as u64) << 32) | id.generation as u64)
}

/// 收集场上全部发射器（带 `Emitter` 组件的实体，槽位序）。校验全在这里做
/// （kind 合法性、必填字段、范围、粒子预算），热路径只剩纯算术。
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
            // 数值字段读取：缺省值 or 显式报错（带写法提示）
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
                    // 同屏粒子预算：稳态可见数 ≈ rate · lifetime / 60
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
                    // burst 缺省 -1 = 未触发（负数都算未触发）
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
            // 发射器必须有位置（同 point/spot 光源的约定）
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
            // color_end 缺省/空串 = 不渐变（同 Camera.follow 空串 = 不跟随的约定）
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

/// 一个待画的粒子（世界坐标 + 已算好的大小/颜色）。CPU 方点光栅化和 GPU 方块
/// 顶点流共用——位置/数量/颜色两条路径必然一致。
pub struct ParticleDot {
    pub x: f64,
    pub y: f64,
    /// 当前大小（世界单位，按寿命进度从 size 渐变到 size_end）。
    pub size: f64,
    /// 当前颜色（rgb 按寿命进度渐变，alpha 线性淡出 255 → 0）。
    pub rgba: [u8; 4],
}

/// 第 `tick` tick 该发射器的全部在途粒子——**纯函数**：同 (发射器字段, tick) 永远
/// 给出同一串粒子（顺序：老粒子在前 = 先画在底下；burst 全员同龄按序号排）。
/// 无积分器：位置是解析式 `pos = origin + v0·t + ½g·t²`，t = 粒龄（秒）。
/// 每个粒子的方向/初速由 SplitMix64(种子 ⊕ 序号) 散列出（不碰模拟 RNG 流）。
pub fn emitter_particles(e: &EmitterSource, tick: u64) -> Vec<ParticleDot> {
    let mut out = Vec::new();
    if !e.active {
        return out;
    }
    let t = tick as i64;
    match e.kind {
        EmitterKind::Stream { rate } => {
            // 第 b tick 出生的粒子序号 = [n(b), n(b+1))，n(b) = floor(b·rate/60)。
            // 从最老（age = lifetime-1）往最新画——后生的粒子盖在上面
            let births_before =
                |b: i64| -> i64 { (b as f64 * rate / PARTICLE_TICKS_PER_SECOND).floor() as i64 };
            for age in (0..e.lifetime).rev() {
                let b = t - age;
                if b < 0 {
                    continue; // 世界从 tick 0 开始，没有更早的出生
                }
                for k in births_before(b)..births_before(b + 1) {
                    out.push(particle_at(e, k as u64, age));
                }
            }
        }
        EmitterKind::Burst { count, burst } => {
            if burst < 0 {
                return out; // 未触发
            }
            let age = t - burst;
            if age < 0 || age >= e.lifetime {
                return out; // 还没到触发 tick / 寿命已尽
            }
            for k in 0..count {
                out.push(particle_at(e, k as u64, age));
            }
        }
    }
    out
}

/// 序号 `k`、粒龄 `age`（tick）的粒子——纯算术，无状态。
fn particle_at(e: &EmitterSource, k: u64, age: i64) -> ParticleDot {
    let h = mix64(e.seed ^ k);
    // 高/低 32 位各出一个 [0,1] 均匀数：方向偏移 + 初速
    let u1 = (h >> 32) as u32 as f64 / u32::MAX as f64;
    let u2 = h as u32 as f64 / u32::MAX as f64;
    let dir = e.dir + (u1 - 0.5) * e.spread;
    let speed = e.speed_min + u2 * (e.speed_max - e.speed_min);
    let secs = age as f64 / PARTICLE_TICKS_PER_SECOND;
    let (sn, cs) = dir.to_radians().sin_cos();
    // 寿命进度 0..<1（age ∈ 0..lifetime）：颜色/大小线性渐变，alpha 线性淡出
    let s = age as f64 / e.lifetime as f64;
    let ch = |c: usize| (e.rgb[c] + (e.rgb_end[c] - e.rgb[c]) * s).round() as u8;
    ParticleDot {
        x: e.x + cs * speed * secs,
        y: e.y + sn * speed * secs + 0.5 * e.gravity * secs * secs,
        size: e.size + (e.size_end - e.size) * s,
        rgba: [ch(0), ch(1), ch(2), (255.0 * (1.0 - s)).round() as u8],
    }
}

/// 粒子光栅化（CPU 路径）：方点，中心在世界坐标、边长 = 当前大小，与精灵同一套
/// 世界→屏幕变换；src-alpha 混合（与贴图精灵的 alpha 混合同一条算式）。
/// 在光照之后调用——粒子自发光，不参与法线缓冲。
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
                    // src-alpha 混合，与贴图精灵的逐通道算式一致
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

/// 可选的 `Sprite.rot`（度数）。缺省 = 0 = 不旋转；字段在但不是数字 → 显式报错。
/// 角度约定：**世界空间逆时针为正**——屏幕 y 翻转后，画面上看同样是逆时针。
/// CPU 光栅化、GPU 顶点流、点选三处共用这一个语义源头。
pub fn rot_of(world: &World, id: vitric_ecs::EntityId) -> Result<f64, String> {
    match world.get_field(id, "Sprite.rot") {
        Err(_) => Ok(0.0),
        Ok(v) => v
            .as_f64()
            .ok_or_else(|| format!("实体 {id} 的 Sprite.rot 不是数字（度数）: {v}")),
    }
}

/// 采样并解码法线贴图的一个纹素 → **屏幕空间**单位法线（约定见模块文档）：
/// n = rgb/255·2-1，z 取绝对值（强制朝外），xy 按精灵旋转矩阵（局部→屏幕，
/// 与顶点变换同一矩阵 [[c, s], [-s, c]]）旋转后整体归一化；零向量退化为平面 (0,0,1)。
/// (u, v) 与漫反射贴图同一套（含 clamp 行为），法线贴图尺寸不必与漫反射一致。
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

/// 取贴图素材；图不存在直接报错并列出现有素材（不画占位符）。
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

/// 文字：`Text` {"content","size","color"} + `Position`，画在所有精灵之上。
/// 两条路径（语义见模块文档的 Text 约定）：素材仓库没挂字体走内嵌 8x8 点阵
/// （ASCII，等宽，非 ASCII 画实心方块占位——**这段字节不许变**，向后兼容由测试锁死）；
/// 挂了字体（清单 `font`）所有 Text 走矢量路径（比例字距 + 覆盖率抗锯齿）。
/// 文字**永远直立**——`Sprite.rot` 只转精灵，不转文字（HUD 保持水平）。
/// `normals`：文字像素清掉底下的法线（文字盖在法线精灵上时按平面打光，不继承浮雕）。
/// `skip`：跳过这一个 Text 实体（对比度测量专用，见 [`RenderOpts`]）；`None` = 全画。
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
        // reveal（0..=1 比例，文字显隐的进度）：缺省补 1.0=全显。可见字数 = reveal
        // 的纯函数；缺字段 / ≥1 与未引入本特性时逐字节相同（向后兼容）。
        let reveal = world
            .get_field(id, "Text.reveal")
            .ok()
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let total_chars = content.chars().count();
        let visible = revealed_chars(reveal, total_chars);
        if visible == 0 {
            continue; // 一个字都没显（reveal=0）：和 content 为空一样不画
        }
        let px = num(world, id, "Position.x")?;
        let py = num(world, id, "Position.y")?;
        // screen=true: HUD 锚定——Position 解释为相对屏幕中心的偏移,不随相机走
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

        // 矢量路径：挂了字体所有 Text 都走这里（per-Text 覆盖不在范围内）
        if let Some(font) = assets.font() {
            // 整串排版一次（按内容居中、版面缓存），只画前 visible 个字形——
            // 逐字显示绝不重排，可见字数只是排好后切一刀
            draw_text_vector(
                buf, width, height, font, &content, size, scale, (cx, cy), rgba, normals, visible,
            );
            continue;
        }

        // —— 点阵路径：这段逻辑不许动——没挂字体时输出字节必须与字体功能
        //    出现之前逐位相同（向后兼容由测试锁死）。reveal 只是把要画的字符
        //    截到前 visible 个（全显时 chars == content.chars()，字节不变）
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
                let u = ((x as f64 + 0.5) - (cx - half_w)) / span_x; // 0..1 横跨整串
                let v = ((y as f64 + 0.5) - (cy - half_h)) / span_y; // 0..1 纵跨一字
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

/// 字符 → 8x8 点阵（每字节一行，低位在左）。非 ASCII 用实心方块占位。
fn glyph_of(c: char) -> [u8; 8] {
    let cp = c as usize;
    if cp < 128 {
        font8x8::legacy::BASIC_LEGACY[cp]
    } else {
        [0xff; 8]
    }
}

/// 矢量文字：一整串按比例字距排版后逐字形栅格化（缓存）+ 覆盖率混合（抗锯齿）。
/// 几何约定：字号 = size×scale 像素的字形总高；整串横向居中于 (cx,cy)，竖向把
/// 字身带（ascent..descent）居中；字形落笔在整数像素（不做亚像素，缓存键才成立）。
/// GPU 路径（vitric-cli gpu.rs）用同一套 layout/raster/取整——视觉对齐，
/// 但不承诺与 CPU 逐字节相同（截图/断言以这里为准）。
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
    // 缓存版排版：整串排一次进 memo，逐字显示同段文字播 N tick 排版只跑一次。
    // 居中按整串总宽算（reveal 时文字不左右抖动），只画前 visible 个字形。
    let laid = font.layout_cached(content, px_size);
    let (placements, total_w) = (&laid.0, laid.1);
    let left = cx - total_w as f64 / 2.0;
    let baseline = (cy + font.baseline_offset(px_size) as f64).round() as i64;
    for p in placements.iter().take(visible) {
        let g = font.raster(p.ch, px_size);
        if g.coverage.is_empty() {
            continue; // 空轮廓（空格等）只占 advance
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
                // 覆盖率混合 = 抗锯齿。矢量文字是引擎里唯一刻意平滑的元素，
                // 精灵贴图仍是最近邻硬边（像素风不动）
                let i = ((y as u32 * width + x as u32) * 4) as usize;
                let dst = &mut buf[i..i + 4];
                for c in 0..3 {
                    dst[c] = ((rgba[c] as u32 * cov + dst[c] as u32 * (255 - cov)) / 255) as u8;
                }
                dst[3] = 255;
                // 任何覆盖率 > 0 的文字像素都清法线（半覆盖边缘也按文字算，不做半个法线）
                if let Some(ns) = normals.as_mut() {
                    ns[i / 4] = [0.0; 3];
                }
            }
        }
    }
}

/// UI 屏幕空间叠加渲染（CPU 真相源）。布局由 [`ui::solve_layout`] 给出（纯函数，
/// 不经相机），这里只把解算好的屏幕像素矩形画出来：Panel = 背景框（纯色/精灵），
/// UiLabel = 文字（复用 font.rs 版面缓存 + Text.reveal）。
///
/// 性能：场上没有 UI（无 UiRoot）= 第一行 early-return，零分配零遍历（空 UI 零成本）。
/// 复用现有 buf，无离屏缓冲。画家序按 query 槽位序（确定性，后画盖前画）。
/// `normals` 不传——UI 是叠加层，画在光照/泛光之后，自身不参与打光（HUD 语义）。
fn draw_ui(
    world: &World,
    buf: &mut [u8],
    width: u32,
    height: u32,
    assets: &Assets,
) -> Result<(), String> {
    if !ui::has_ui(world) {
        return Ok(()); // 空 UI 零成本：没有 UiRoot，整条 UI 路径零分配零遍历
    }
    let layout = ui::solve_layout(world, width, height)?;

    // Panel：背景框。按实体序画（后画盖前画）。纯色 = 直接 alpha 混合方块；
    // 精灵 = 最近邻缩放贴图（NinePatch 留 1.2，纯色 + 精灵 1.1 必做）。
    for id in world.query(&["Ui", "Panel"]) {
        let Some(rect) = layout.get(&id) else { continue };
        // 按下反馈（1.2）：挂了 Button 且 press_t≥0 时，按 press_scale/press_modulate 的
        // 解析式做 scale（绕矩形中心缩）+ modulate（提亮）。**纯渲染装饰**：只读组件里的
        // press_t（进哈希进存档），偏移是 press_t 的纯函数，不碰布局矩形/模拟 RNG——
        // 重放/快照回退一致（同 shake/bloom 的装饰纪律）。CPU/GPU 共用 ui_press_feedback。
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
            // 纯色框（含 alpha）——Panel.color 缺省不透明白
            let color = world
                .get_field(id, "Panel.color")
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "#ffffff".to_string());
            let mut rgba = parse_color_a(&color).map_err(|e| format!("实体 {id} 的 Panel.color: {e}"))?;
            modulate_rgb(&mut rgba, modulate);
            let a = rgba[3] as u32;
            if a == 0 {
                continue; // 全透明 = 不画
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
            // 精灵背景：图不存在直接报错（不画占位）——口径对齐 Sprite.image
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

    // UiLabel：文字。整串排版后画在节点框里，按 align 水平对齐、竖向居中于框。
    // 复用 font.rs（挂字体走矢量，否则点阵）+ Text.reveal（逐字显示已落地）。
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
        // UI 字号是**屏幕像素**字号（不经相机 scale）——size 直接当像素高用。
        // 文字竖向居中于节点框，水平按 align 在框内对齐。
        draw_ui_label(buf, width, height, assets, &content, size, &align, *rect, rgba, &mut no_normals, visible);
    }
    Ok(())
}

/// 画一条 UI 文字（屏幕空间，字号 = 像素高，不经相机）。挂字体走矢量、否则点阵——
/// 与世界文字 [`draw_texts`] 同两条路径，但坐标是 UI layout 的屏幕矩形。
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
    let cy = rect.y + rect.h / 2.0; // 竖向居中于框
    if let Some(font) = assets.font() {
        // UI 字号直接是像素（scale=1）：px_size = size 像素的字形总高
        let px_size = FontStore::px_size(size, 1.0);
        let laid = font.layout_cached(content, px_size);
        let (placements, total_w) = (&laid.0, laid.1);
        // 水平对齐：在框宽内放整串
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
        // 点阵路径：等宽 size×size 像素方格
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

/// 把一个栅格化字形的覆盖率位图混进 buf（抗锯齿），落笔在 (gx0,gy0)。
/// 抽出来给 UI 文字复用（与 [`draw_text_vector`] 的内层混合逻辑同口径）。
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

/// 屏幕像素 → 世界坐标（检查器拖拽、点选用）。
/// 用不抖的相机：点选/拖拽对的是世界本体，抖动只是几帧的视觉装饰。
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

/// 点选拾取：返回屏幕坐标 (px,py) 命中的最上层实体（绘制顺序靠后者优先）。
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

/// 点选拾取（世界坐标版）：返回世界点 (wx,wy) 命中的最上层实体。
/// 窗口点击（屏幕坐标先经 screen_to_world）和控制面 `input/click`（直接给
/// 世界坐标）共用这一套判定——人点的和 AI 点的命中规则逐位一致。
/// 判定确定性：query 按槽位序、不碰模拟 RNG，可安全用于录制中的点击解析。
pub fn pick_world(
    world: &World,
    wx: f64,
    wy: f64,
) -> Result<Option<vitric_ecs::EntityId>, String> {
    let ids = world.query(&["Position", "Sprite"]);
    // 倒序：后画的盖在上面，优先命中
    for &id in ids.iter().rev() {
        let x = num(world, id, "Position.x")?;
        let y = num(world, id, "Position.y")?;
        let w = num(world, id, "Sprite.w")?;
        let h = num(world, id, "Sprite.h")?;
        let rot = rot_of(world, id)?;
        // rot != 0 时把点击点逆旋回精灵局部空间（世界系，y 向上）——
        // 命中判定对的是旋转后的真实形状，不是未旋转的 AABB
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

/// 在已渲染的帧上给实体画选中描边（检查器高亮，青色 2px）。
/// `tick` 必须和这帧 `render_world` 用的同一个——描边要跟着抖动的画面走，不然抖屏时错位。
pub fn draw_selection_outline(
    buf: &mut [u8],
    world: &World,
    width: u32,
    height: u32,
    selected: vitric_ecs::EntityId,
    tick: u64,
) -> Result<(), String> {
    if !world.is_alive(selected) || !world.has_component(selected, "Sprite") {
        return Ok(()); // 选中的实体没了/不可见，描边静默跳过（选中态本身由上层管理）
    }
    let (cam_x, cam_y, scale) = camera_of(world, tick, height)?;
    let x = num(world, selected, "Position.x")?;
    let y = num(world, selected, "Position.y")?;
    let w = num(world, selected, "Sprite.w")?;
    let h = num(world, selected, "Sprite.h")?;
    let rot = rot_of(world, selected)?;
    // rot != 0 时描边取**旋转后形状的轴对齐包围盒**——画轴对齐矩形比描旋转轮廓
    // 简单得多，检查器高亮要的只是"看见选中了谁"，不需要贴边精确
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

/// RGBA 像素 → PNG 字节。
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

/// 一步到位：world → PNG。
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

/// 语义观察：把"画面上有什么"翻译成 LLM 能精确读懂的结构化描述。
///
/// 这是 agent 的**主观察通道**——比让模型看像素更精准：
/// 坐标是确切数字、方位是九宫格词、遮挡是明确的实体对、
/// 视野外的东西有方向和距离。截图（screenshot）退居兜底验证。
///
/// 没有素材仓库的便捷入口：等价于 `describe_world_with_assets(.., &Assets::empty())`。
/// 结构化字段与素材无关；区别只在文字对比度测量的保真度——空仓库时带贴图的精灵
/// 退化成 `Sprite.color` 纯色块近似底色亮度（见 [`describe_world_with_assets`]）。
pub fn describe_world(world: &World, width: u32, height: u32) -> Result<serde_json::Value, String> {
    describe_world_with_assets(world, width, height, &Assets::empty())
}

/// [`describe_world`] 的全功能版：带素材仓库，文字对比度测量按真贴图渲染。
///
/// 可读性警告（AI 开发者的眼睛）：屏上每条文字，把世界**少画这一条文字**渲一帧
/// （素材宽容模式，缺图退纯色近似），取文字包围盒内的平均背景相对亮度 L_bg，
/// 与 `Text.color` 的相对亮度 L_fg 算 WCAG 式对比度 `(max+0.05)/(min+0.05)`；
/// 低于 [`TEXT_CONTRAST_MIN`] 就给一条 `warnings[]`（kind=`low-contrast-text`）
/// 并在中文摘要里加一行 ⚠。真实事故原型：米色文字叠在米色卡面上，构建它的 agent
/// "看不见"所以从没发现人眼读不出来。
///
/// 已知近似（这是 lint 不是色彩学）：
/// - 文字色取原始值、底色取打光后的像素——开了光照/泛光时文字实际也会被打光，
///   比值有偏差；阈值放宽到 2.5 已把这类偏差吃进余量；
/// - 包围盒按未渲染的排版几何估（点阵 = 等宽字格，矢量 = layout 总宽 × 字高）；
/// - 只测中心点在屏内的文字（describe 判"视野外"的同一条标准），视野外不渲不测；
/// - 测量渲染用不抖的相机（describe 的语义就是不抖）。
///
/// 成本：每条屏上文字多渲一帧 describe 分辨率的 CPU 帧；场上没文字 = 零额外开销。
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
    // 语义观察用不抖的相机：agent 断言的坐标不该被几帧视觉抖动晃花
    let (cam_x, cam_y, scale) = camera_base(world, height)?;
    let half_w_units = width as f64 / scale / 2.0;
    let half_h_units = height as f64 / scale / 2.0;

    let mut visible = Vec::new();
    let mut offscreen = Vec::new();
    let mut rects: Vec<(String, f64, f64, f64, f64)> = Vec::new(); // (id, x, y, w, h) 世界坐标

    for id in world.query(&["Position", "Sprite"]) {
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
            // 旋转角进语义观察（缺省 0 不输出——和画面行为一样，没有就是没有）
            sprite["rot"] = json!(rot);
        }
        entry.insert("sprite".into(), sprite);

        if on_screen {
            let sx = width as f64 / 2.0 + dx * scale;
            let sy = height as f64 / 2.0 - dy * scale;
            entry.insert("screen_px".into(), json!({"x": sx.round(), "y": sy.round()}));
            entry.insert(
                "region".into(),
                json!(region_word(sx / width as f64, sy / height as f64)),
            );
            rects.push((id.to_string(), px, py, sw, sh));
            visible.push(serde_json::Value::Object(entry));
        } else {
            let direction = direction_word(dx, dy);
            entry.insert("direction".into(), json!(direction));
            entry.insert(
                "distance_units".into(),
                json!((dx.powi(2) + dy.powi(2)).sqrt().round()),
            );
            offscreen.push(serde_json::Value::Object(entry));
        }
    }

    // 屏上文字：内容本身就是语义，agent 不用 OCR 截图
    let mut texts = Vec::new();
    // 对比度测量候选：只收中心点在屏内、有正字号的（视野外/画不出来的不渲不测）
    struct ContrastCandidate {
        id: vitric_ecs::EntityId,
        content: String,
        color: String,
        size: f64,
        /// 屏幕像素坐标（与绘制路径同一公式）。
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
            // size 缺失/非正的文字画不出来（render 会报错/跳过），没有"底色"可言，不进对比度测量
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

    // 文字可读性检查（见函数文档）：屏上有文字才渲帧，否则零额外成本
    let mut warnings: Vec<Value> = Vec::new();
    let mut warning_lines: Vec<String> = Vec::new();
    for c in &candidates {
        // 少画这一条文字渲一帧（其余文字照画——文字叠文字时底色也算数），
        // 相机用 describe 自己的不抖相机，素材宽容模式见 RenderOpts。
        // tick 固定 0：describe 没有时间概念，测量帧里的粒子按 tick 0 展开
        //（stream 在 tick 0 几乎没有粒子）——对比度是 lint 不是色彩学，已知近似
        let frame = render_with(
            world,
            width,
            height,
            assets,
            (cam_x, cam_y, scale),
            0,
            &RenderOpts { skip_text: Some(c.id), lenient_images: true },
        )?;
        // 包围盒按绘制几何估算（与 draw_texts 的两条路径镜像），裁到屏内
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
            continue; // 包围盒裁完没有像素（贴边的极端情形），没东西可测
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

    // 视觉重叠（画面上谁压着谁）。已知近似：一律按**未旋转尺寸**的 AABB 判相交——
    // 旋转精灵的精确相交（SAT）对语义观察不值得：方位/坐标/rot 字段已足够 agent 定位，
    // 边缘误报误漏由像素截图兜底
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

    // 一段给 LLM 直接读的中文摘要（结构化字段的浓缩版）
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
    // 可读性警告紧跟文字行——agent 读摘要时警告就在出问题的文字旁边
    lines.extend(warning_lines);

    // 光照设置：开着就让 agent 在文字层面看见全部灯（位置/半径/颜色），不用看像素猜
    let lighting = match ambient_of(world)? {
        None => None,
        Some((_, ambient_color)) => {
            let lights = collect_lights(world)?;
            lines.push(format!(
                "光照开启：环境色 {ambient_color}，{} 盏光源。",
                lights.len()
            ));
            // 投影状态也文字化：开着就报遮光体数量，agent 不用数像素猜"影子为什么没出来"
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
                    // 平行光没有位置/半径（占位 0 不是真值，不输出免得误导 agent）
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

    // 泛光设置：开着就把参数文字化——agent 看 describe 就知道后效怎么配的，不用猜像素
    let bloom = bloom_of(world)?;
    if let Some(b) = &bloom {
        lines.push(format!(
            "泛光开启：threshold {}，strength {}。",
            b.threshold, b.strength
        ));
    }

    // 粒子发射器：按发射器汇总一行（粒子不逐个列——它们是纯函数展开的画面装饰，
    // 不是可观察的世界状态）。describe 没有时间概念：stream 给稳态可见数估算，
    // burst 给触发字段原值，agent 自己对照当前 tick
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
        // 投影关闭时不出现这两个键——"没有键 = 没开"，和 bloom/warnings 同一约定
        if shadows {
            out["shadows"] = json!(true);
            out["occluders"] = json!(occluder_count);
        }
    }
    if let Some(b) = &bloom {
        out["bloom"] = json!({"threshold": b.threshold, "strength": b.strength});
    }
    // 没发射器就不出现 emitters 键——同 bloom/warnings 的"没有键 = 没有"约定
    if !emitters_json.is_empty() {
        out["emitters"] = json!(emitters_json);
    }
    // 没警告就不出现 warnings 键——"没有这个键 = 没发现问题"，agent 不用扫空数组
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    Ok(out)
}

/// WCAG 相对亮度（输入 sRGB 字节的前 3 通道）：先逆 gamma 线性化再加权
/// `L = 0.2126R + 0.7152G + 0.0722B`。对比度比值 = `(L1+0.05)/(L2+0.05)`（亮比暗）。
fn relative_luminance(rgb: &[u8]) -> f64 {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// 屏幕九宫格方位词（输入为 0..1 的屏幕比例坐标）。
fn region_word(fx: f64, fy: f64) -> &'static str {
    let col = if fx < 1.0 / 3.0 { 0 } else if fx < 2.0 / 3.0 { 1 } else { 2 };
    let row = if fy < 1.0 / 3.0 { 0 } else if fy < 2.0 / 3.0 { 1 } else { 2 };
    match (row, col) {
        (0, 0) => "左上", (0, 1) => "上方", (0, 2) => "右上",
        (1, 0) => "左侧", (1, 1) => "中心", (1, 2) => "右侧",
        (2, 0) => "左下", (2, 1) => "下方", _ => "右下",
    }
}

/// 视野外方向词（世界坐标系，y 向上）。
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

/// 相机本体（不含抖动偏移）：取第一个 Camera 实体，没有则原点、8 像素/单位。
/// 可选 `view_h`（竖向可视世界高度，单位数）> 0 时按视口高度反推像素密度——
/// 内容占屏比例与分辨率无关，4K 和 720p 看到同样大的世界；否则用 scale（像素/单位）。
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

/// 渲染取景相机：本体 + 相机实体上 `Shake` 组件的抖动偏移。
/// CPU 光栅化和 GPU 路径都从这里取相机——两条路径抖得逐位一致。
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

/// 屏幕抖动偏移（世界单位）：(tick, amplitude) 的纯函数，与模拟的 RNG 流完全无关
/// ——抖屏永远不会扰动 gameplay 的确定性轨迹，快照里也没有额外状态要存。
/// 实现：SplitMix64 把 tick 打散成 64 位，高/低各 32 位映射到 [-1, 1] 两轴再乘振幅。
pub fn shake_offset(tick: u64, amplitude: f64) -> (f64, f64) {
    if amplitude <= 0.0 {
        return (0.0, 0.0);
    }
    let z = mix64(tick); // 同一个 SplitMix64（粒子散列共用），运算序列与重构前逐位相同
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

/// 颜色解析（带可选 alpha）：`#rrggbb`（不透明）或 `#rrggbbaa`（带透明度）。
/// UI Panel 背景常要半透明遮罩，所以单独一条支持 8 位 hex；世界精灵/文字仍走
/// [`parse_color`]（只认 6 位，alpha 恒 255，字节锁死的旧行为不动）。
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
        // 相机右移 2 单位 → 精灵在屏幕上左移 2*8=16 像素
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
        // "I" 单字符，4 单位 → 32x32 像素，居中
        w.set_component(e, "Text", json!({"content": "I", "size": 4.0, "color": "#00ff00"}))
            .unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        // "I" 的竖干在字形第 2-3 列（8x8 点阵字形偏左），取样打在竖干上
        assert_eq!(pixel(&buf, 64, 25, 32), [0, 255, 0, 255], "竖干处应是字形像素");
        assert_eq!(pixel(&buf, 64, 2, 2), [24, 26, 33, 255]);
        // 同世界同字节（文字也确定性）
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
        // 2x2 贴图：左半红不透明，右半全透明
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
        // 默认相机 scale=8：精灵占屏幕中央 32x32 像素
        let buf = render_world(&w, 64, 64, &assets, 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32 - 8, 32), [255, 0, 0, 255], "左半是贴图红");
        assert_eq!(pixel(&buf, 64, 32 + 8, 32), [24, 26, 33, 255], "右半透明 → 透出背景");

        // 引用不存在的图：报错并列出现有素材
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
        // 跟玩家重叠的金币
        let c = w.spawn_named("coin").unwrap();
        w.set_component(c, "Position", json!({"x": 0.5, "y": 0.0})).unwrap();
        w.set_component(c, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ffd84d"})).unwrap();
        // 视野外左边远处一个
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
        // 玩家和金币视觉重叠要被点名
        let overlaps = d["overlaps"].as_array().unwrap();
        assert_eq!(overlaps.len(), 1, "{overlaps:?}");
        // 摘要可直接读
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
        // 屏幕中心：两个都覆盖，命中后画的 above
        assert_eq!(pick(&w, 64, 64, 32.0, 32.0).unwrap(), Some(above));
        // 偏一点：只有大的 below 覆盖（above 半宽 1 单位 = 8px）
        assert_eq!(pick(&w, 64, 64, 32.0 + 12.0, 32.0).unwrap(), Some(below));
        // 空地
        assert_eq!(pick(&w, 64, 64, 2.0, 2.0).unwrap(), None);
        // 坐标往返
        let (wx, wy) = screen_to_world(&w, 64, 64, 32.0 + 8.0, 32.0 - 16.0).unwrap();
        assert!((wx - 1.0).abs() < 1e-9 && (wy - 2.0).abs() < 1e-9, "{wx},{wy}");
    }

    #[test]
    fn pick_world_same_verdict_as_screen_pick() {
        let mut w = World::new();
        let e = w.spawn_named("card").unwrap();
        w.set_component(e, "Position", json!({"x": 3.0, "y": 2.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#00ff00"})).unwrap();
        // 世界坐标直接命中/空地
        assert_eq!(pick_world(&w, 3.0, 2.0).unwrap(), Some(e));
        assert_eq!(pick_world(&w, 3.9, 1.1).unwrap(), Some(e), "边内要命中");
        assert_eq!(pick_world(&w, 5.5, 2.0).unwrap(), None, "边外是空地");
        // 与屏幕坐标版同一套判定：屏幕点 → screen_to_world → pick_world 闭环
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
        // 精灵半宽 8px + 2px 外扩 → 描边在 x=32±10
        assert_eq!(pixel(&buf, 64, 32 - 10, 32), [39, 192, 168, 255], "左描边");
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "精灵本体不被盖");
    }

    /// 旋转测试用素材：halves.png（2x1，左红右蓝——不对称图案才能看出转没转对）。
    /// 返回 (素材库, 临时目录)，用完调用方负责删目录。
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

    /// 原点处 4x2 的 halves 贴图精灵，可选 rot。
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
        // 显式写 rot: 0 必须与压根没有 rot 字段逐字节相同（快路径向后兼容锁死）
        let plain = render_world(&world_one_red_sprite(), 64, 64, &Assets::empty(), 0).unwrap();
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#ff0000", "rot": 0.0}))
            .unwrap();
        assert_eq!(plain, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());
        // 贴图精灵同理
        let (assets, dir) = assets_with_halves("rot0");
        let plain = render_world(&world_halves_sprite(None), 64, 64, &assets, 0).unwrap();
        let with_field = render_world(&world_halves_sprite(Some(0.0)), 64, 64, &assets, 0).unwrap();
        assert_eq!(plain, with_field);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rot_90_rotates_pixels_counter_clockwise() {
        // 4x2 左红右蓝，逆时针转 90°：右边的蓝半边转到画面上方，红半边到下方
        let (assets, dir) = assets_with_halves("rot90");
        let buf = render_world(&world_halves_sprite(Some(90.0)), 64, 64, &assets, 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 20), [0, 0, 255, 255], "上方是蓝（原来的右半边）");
        assert_eq!(pixel(&buf, 64, 32, 44), [255, 0, 0, 255], "下方是红（原来的左半边）");
        // 未旋转 AABB 的左右两翼现在是空的——旋转后形状是竖条（占 x 24..40, y 16..48）
        assert_eq!(pixel(&buf, 64, 20, 32), BACKGROUND, "左翼是背景");
        assert_eq!(pixel(&buf, 64, 44, 32), BACKGROUND, "右翼是背景");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rot_180_equals_flipping_both_axes() {
        // 居中精灵转 180° = 整幅画面两轴同时翻转（逐像素对比）
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
        // 任意角度（三角函数路径）同世界同 tick → 字节逐位相同
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
        // 屏幕 (32,18) = 世界 (0, 1.75)：未旋转 AABB 之外（h=2），旋转后竖条之内 → 命中
        assert_eq!(pick(&w, 64, 64, 32.0, 18.0).unwrap(), Some(e), "旋转后的形状内要命中");
        // 屏幕 (46,32) = 世界 (1.75, 0)：未旋转 AABB 之内（w=4），旋转后已是空地 → 不命中
        assert_eq!(pick(&w, 64, 64, 46.0, 32.0).unwrap(), None, "转走了的区域不该命中");
        // 对照组：rot 归零后两个判定正好反过来
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
        // 不旋转的精灵不带 rot 字段
        let d0 = describe_world(&world_one_red_sprite(), 64, 64).unwrap();
        assert!(d0["visible"][0]["sprite"].get("rot").is_none());
    }

    #[test]
    fn selection_outline_uses_rotated_bbox() {
        // 4x2 转 90° → 旋转后包围盒约 2x4（世界单位）：描边贴竖条，不贴原始横条
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Sprite", json!({"w": 4.0, "h": 2.0, "color": "#ff0000", "rot": 90.0}))
            .unwrap();
        let mut buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        draw_selection_outline(&mut buf, &w, 64, 64, e, 0).unwrap();
        // 旋转后半宽 1 单位 = 8px + 2px 外扩 → 左描边在 x≈22（2px 厚，取靠内那列避开浮点边界）
        assert_eq!(pixel(&buf, 64, 22, 32), [39, 192, 168, 255], "左描边贴竖条");
        // 未旋转尺寸的描边位置（32-18=14）应是背景——证明用的是旋转后的包围盒
        assert_eq!(pixel(&buf, 64, 14, 32), BACKGROUND, "老位置不该有描边");
        // 上描边：旋转后半高 2 单位 = 16px + 2px → y=14 起两行
        assert_eq!(pixel(&buf, 64, 32, 15), [39, 192, 168, 255], "上描边随包围盒抬高");
        assert_eq!(pixel(&buf, 64, 32, 32), [255, 0, 0, 255], "精灵本体不被盖");
    }

    #[test]
    fn shake_offset_is_pure_function_of_tick_and_amplitude() {
        // 同 (tick, amplitude) → 同偏移（纯函数，没有隐藏状态）
        assert_eq!(shake_offset(7, 0.5), shake_offset(7, 0.5));
        // 不同 tick → 偏移变（不然抖动是冻住的）
        assert_ne!(shake_offset(7, 0.5), shake_offset(8, 0.5));
        // 偏移每轴不超振幅；amplitude=0 → 零偏移
        let (dx, dy) = shake_offset(123, 0.5);
        assert!(dx.abs() <= 0.5 && dy.abs() <= 0.5, "({dx},{dy})");
        assert_eq!(shake_offset(123, 0.0), (0.0, 0.0));
    }

    #[test]
    fn view_h_makes_zoom_resolution_independent() {
        // view_h=8:不管视口多少像素,竖向永远看到 8 个世界单位——内容占屏比例恒定
        let mut w = world_one_red_sprite();
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0, "view_h": 8.0}))
            .unwrap();
        assert_eq!(camera_of(&w, 0, 80).unwrap().2, 10.0, "80px/8单位=10像素每单位");
        assert_eq!(camera_of(&w, 0, 160).unwrap().2, 20.0, "分辨率翻倍像素密度翻倍");
        // 2x2 的精灵在 view_h=8 下永远占竖向 1/4,与分辨率无关
        for vh in [64u32, 128] {
            let buf = render_world(&w, vh, vh, &Assets::empty(), 0).unwrap();
            let bg = [24, 26, 33, 255];
            let top = (vh as f64 * (0.5 - 1.0 / 8.0)) as u32; // 精灵上缘
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

        // 渲染整帧也确定：同 tick 逐字节相同，抖动 tick 间像素不同
        let f7 = render_world(&w, 64, 64, &Assets::empty(), 7).unwrap();
        assert_eq!(f7, render_world(&w, 64, 64, &Assets::empty(), 7).unwrap());
        assert_ne!(f7, render_world(&w, 64, 64, &Assets::empty(), 8).unwrap());

        // amplitude 归零 → 偏移消失，取景回到相机本体
        w.set_field(cam, "Shake.amplitude", json!(0.0)).unwrap();
        assert_eq!(camera_of(&w, 7, 64).unwrap(), (1.0, 2.0, 8.0));
        // 语义观察/点选永远读不抖的相机
        w.set_field(cam, "Shake.amplitude", json!(0.5)).unwrap();
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["camera"], json!({"x": 1.0, "y": 2.0, "scale": 8.0}));
    }

    #[test]
    fn lighting_brightens_near_light_and_is_deterministic() {
        // 暗环境 + 一盏白灯在原点：近灯像素比远处亮，且逐字节确定
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
        // 灯半径 4 单位 × scale 8 = 32px：角落在半径外 → 环境黑 = 全黑
        assert_eq!(far, [0, 0, 0, 255], "半径外只剩环境光（纯黑）");
        assert!(near[0] > far[0] && near[2] > far[2], "近灯应更亮: {near:?} vs {far:?}");
        // 同世界同 tick → 字节逐位相同
        assert_eq!(buf, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn no_ambient_entity_skips_lighting_entirely() {
        // 有灯没 Ambient = 光照整体关闭：和没灯的世界渲出同样的字节（向后兼容）
        let plain = render_world(&world_one_red_sprite(), 64, 64, &Assets::empty(), 0).unwrap();
        let mut w = world_one_red_sprite();
        let lamp = w.spawn();
        w.set_component(lamp, "Position", json!({"x": 1.0, "y": 1.0})).unwrap();
        w.set_component(lamp, "Light", json!({"radius": 4.0})).unwrap();
        assert_eq!(plain, render_world(&w, 64, 64, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn lighting_formula_clamps_at_1_5_and_white_ambient_is_identity() {
        // 公式锁死：lit = min(ambient + Σ 贡献, 1.5)，out = min(scene·lit, 1)
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        // #646464 = (100,100,100)：过曝上限 1.5 → 100*1.5 = 150，数值可精确断言
        w.set_component(e, "Sprite", json!({"w": 2.0, "h": 2.0, "color": "#646464"})).unwrap();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#ffffff"})).unwrap();
        // 白环境光（lit=1.0）且没灯 = 恒等变换，字节不变
        let lit = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&lit, 64, 32, 32), [100, 100, 100, 255], "白环境光不改像素");
        assert_eq!(pixel(&lit, 64, 2, 2), [24, 26, 33, 255], "背景也不变");
        // 加一盏强灯：白环境 1.0 + 大贡献 → 夹到 1.5 → 100*1.5=150
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
        // 半径非法也要显式报
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
        // 没 Ambient：describe 里不出现光照字段
        let d = describe_world(&World::new(), 64, 64).unwrap();
        assert!(d.get("ambient").is_none() && d.get("lights").is_none());
    }

    /// 黑环境 + 一盏可配 kind 的灯（原点）——聚光/平行光测试的公共脚手架。
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
        // kind:"point" 显式写出 = 不写 kind 的旧点光源，输出逐字节相同（快路径不变）
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
        // 灯在原点（像素 32,32）、半径 4 单位 = 32px、锥角 90°、朝 +x（dir=0）。
        // 像素 (40,32) 和 (32,40) 到灯心距离严格相等（dx/dy 对称），只差方向：
        // (40,32) 在 +x 锥内 → 亮；(32,40) 在 -y 方向（Δθ≈87° > 半角 45°）→ 锥外全黑
        let spot = |dir: f64| {
            json!({"radius": 4.0, "kind": "spot", "angle": 90.0, "dir": dir, "intensity": 1.0})
        };
        let buf = render_world(&world_dark_with_light(spot(0.0)), 64, 64, &Assets::empty(), 0)
            .unwrap();
        let inside = pixel(&buf, 64, 40, 32);
        let outside = pixel(&buf, 64, 32, 40);
        assert_eq!(outside, [0, 0, 0, 255], "锥外同距离像素只剩环境黑");
        assert!(inside[0] > 0 && inside[1] > 0 && inside[2] > 0, "锥内该被照亮: {inside:?}");
        // 锥跟着 dir 转：dir=90（世界 +y = 画面上方）→ 上方亮、原来 +x 的像素掉出锥外
        let buf = render_world(&world_dark_with_light(spot(90.0)), 64, 64, &Assets::empty(), 0)
            .unwrap();
        assert!(pixel(&buf, 64, 32, 24)[0] > 0, "dir=90 后画面上方在锥内");
        assert_eq!(pixel(&buf, 64, 40, 32), [0, 0, 0, 255], "+x 方向掉出锥外（Δθ=90° > 45°）");
        assert_eq!(pixel(&buf, 64, 32, 40), [0, 0, 0, 255], "画面下方仍是锥外");
        // 确定性：同世界同 tick → 字节逐位相同
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
        // 未知 kind：报错列出全部合法取值
        let err = render(json!({"radius": 4.0, "kind": "cone"}));
        assert!(
            err.contains("point") && err.contains("spot") && err.contains("directional"),
            "{err}"
        );
        // kind 不是文本
        let err = render(json!({"radius": 4.0, "kind": 1}));
        assert!(err.contains("Light.kind"), "{err}");
        // 聚光灯缺 angle / 缺 dir：显式报错带写法
        let err = render(json!({"radius": 4.0, "kind": "spot", "dir": 0.0}));
        assert!(err.contains("angle"), "{err}");
        let err = render(json!({"radius": 4.0, "kind": "spot", "angle": 60.0}));
        assert!(err.contains("dir"), "{err}");
        // angle 越界（锥角全宽 1..=360）
        for bad in [0.5, 361.0, -90.0] {
            let err = render(json!({"radius": 4.0, "kind": "spot", "angle": bad, "dir": 0.0}));
            assert!(err.contains("1..=360") && err.contains("Light.angle"), "{err}");
        }
        // point/spot 必须有 Position（平行光才允许没有）
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let lamp = w.spawn();
        w.set_component(lamp, "Light", json!({"radius": 4.0})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Position") && err.contains("directional"), "{err}");
        // 平行光也占 64 盏配额：65 盏平行光显式报错
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
        // 平行光不需要 Position；贡献处处 = color·intensity，与离任何东西的距离无关。
        // 黑环境 + 白色平行光 intensity 0.5 → 每个像素 = 原像素 × 0.5（精确可断言）
        let plain = render_world(&world_one_red_sprite(), 64, 64, &Assets::empty(), 0).unwrap();
        let mut w = world_one_red_sprite();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#000000"})).unwrap();
        let sun = w.spawn(); // 不挂 Position——平行光在无穷远
        w.set_component(sun, "Light", json!({"kind": "directional", "dir": 270.0, "intensity": 0.5}))
            .unwrap();
        let lit = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        for i in 0..plain.len() {
            let expect =
                if i % 4 == 3 { 255 } else { (plain[i] as f64 * 0.5).min(255.0) as u8 };
            assert_eq!(lit[i], expect, "字节 {i}：平行光对全画面是同一个倍率");
        }
        // 对照：只有黑环境（没平行光）= 全黑——亮度差全部来自平行光
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
        // 点光源：kind 总是给出（旧场景没写 kind 也报 "point"），没有 angle/dir
        assert_eq!(lights[0]["kind"], json!("point"));
        assert!(lights[0].get("angle").is_none() && lights[0].get("dir").is_none());
        // 聚光灯：kind + angle + dir + world/radius
        assert_eq!(lights[1]["kind"], json!("spot"));
        assert_eq!(lights[1]["angle"], json!(60.0));
        assert_eq!(lights[1]["dir"], json!(90.0));
        assert_eq!(lights[1]["radius"], json!(8.0));
        // 平行光：kind + dir，没有 world/radius（占位 0 不是真值，不输出）
        assert_eq!(lights[2]["kind"], json!("directional"));
        assert_eq!(lights[2]["dir"], json!(270.0));
        assert!(lights[2].get("world").is_none() && lights[2].get("radius").is_none());
        assert!(d["text"].as_str().unwrap().contains("3 盏"), "{}", d["text"]);
    }

    /// 写一张纯色 RGBA PNG（法线贴图测试素材用）。
    fn write_solid_png(path: &std::path::Path, w: u32, h: u32, rgba: [u8; 4]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let pixels: Vec<u8> = rgba.repeat((w * h) as usize);
        enc.write_header().unwrap().write_image_data(&pixels).unwrap();
    }

    /// 法线测试素材：纯白漫反射 hero.png + 指定法线色的 hero_n.png（整张同一向量）。
    fn assets_with_normal(tag: &str, normal_rgba: [u8; 4]) -> (Assets, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vitric-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_solid_png(&dir.join("hero.png"), 2, 2, [255, 255, 255, 255]);
        write_solid_png(&dir.join("hero_n.png"), 2, 2, normal_rgba);
        (Assets::load_dir(&dir).unwrap(), dir)
    }

    /// 黑环境 + 一盏白点光（世界坐标 lx,ly，半径 20）+ 原点 4x4 贴图精灵（可选 rot）。
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
        // 法线整张朝左（r=0 → nx=-1）：灯在左 = 迎光亮，灯在右 = 背光黑。
        // 两盏灯到精灵中心距离相同——亮度差全部来自 max(dot(N,L),0)
        let (assets, dir) = assets_with_normal("nlit", [0, 128, 255, 255]);
        let lit = render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &assets, 0).unwrap();
        let dark = render_world(&world_normal_scene(8.0, 0.0, None), 64, 64, &assets, 0).unwrap();
        let bright_px = pixel(&lit, 64, 32, 32);
        let shadow_px = pixel(&dark, 64, 32, 32);
        assert!(bright_px[0] > 60, "迎光面应明显被照亮: {bright_px:?}");
        assert_eq!(shadow_px, [0, 0, 0, 255], "背光面 dot<0 夹到 0 = 只剩环境黑");
        // 确定性：同世界同 tick → 字节逐位相同
        assert_eq!(lit, render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &assets, 0).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flat_normal_still_gets_lit_by_z_lift() {
        // 平面法线 (128,128,255)≈(0,0,1)：靠 L.z=0.6 的抬升仍被照亮，但偏离灯心的像素
        // 比"没有法线"的同场景暗（老公式没有 dot 因子）——锁住 z_lift 语义
        let (assets, dir) = assets_with_normal("nflat", [128, 128, 255, 255]);
        let with_n = render_world(&world_normal_scene(-8.0, 0.0, None), 64, 64, &assets, 0).unwrap();
        // 对照组：同一场景但素材没有 _n 配对
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
        // 1) 纯色精灵 + 光照：素材仓库里有没有 _n 文件不改一个字节
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
        // 2) 法线精灵被后画的纯色块完全盖住：盖住的像素按"没有法线"打光（法线被覆盖清掉）
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
        // 法线整张朝上（g=0 → ny=-1，屏幕 y 向下）。逆时针转 90° 后法线朝左：
        // 「90° 精灵 + 左灯」 ≈ 「不转 + 上灯」（两盏灯到中心距离相同，逐像素近似相等）
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
        // 对照：转了 90° 之后顶灯不再正对法线 → 比左灯暗（旋转真的改了方向）
        let rot_wrong =
            render_world(&world_normal_scene(0.0, 8.0, Some(90.0)), 64, 64, &assets, 0).unwrap();
        let wrong = pixel(&rot_wrong, 64, 32, 32);
        assert!(wrong[0] + 20 < b[0], "顶灯照旋转后的左向法线应明显更暗: {wrong:?} vs {b:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 泛光测试场景：中央 2x2 纯白精灵（亮部），可选挂 Bloom。
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
        // 白精灵占屏幕中央 24..40：精灵矩形**之外**的像素要被光晕照亮
        let plain = render_world(&world_bright_sprite(None), 64, 64, &Assets::empty(), 0).unwrap();
        let lit = render_world(&world_bright_sprite(Some((0.5, 1.0))), 64, 64, &Assets::empty(), 0)
            .unwrap();
        // 紧贴精灵右缘外侧（半径 2px、3 次迭代 → 扩散约 6px）：无泛光时是背景
        let halo = pixel(&lit, 64, 42, 32);
        let bg = pixel(&plain, 64, 42, 32);
        assert_eq!(bg, BACKGROUND, "对照组：泛光关时精灵外是背景");
        assert!(halo[0] > bg[0] && halo[1] > bg[1] && halo[2] > bg[2], "光晕该比背景亮: {halo:?}");
        // 远角不受影响（光晕是局部的）
        assert_eq!(pixel(&lit, 64, 2, 2), BACKGROUND, "远处仍是背景");
        // strength 越大光晕越亮
        let stronger =
            render_world(&world_bright_sprite(Some((0.5, 3.0))), 64, 64, &Assets::empty(), 0)
                .unwrap();
        assert!(pixel(&stronger, 64, 42, 32)[0] > halo[0], "strength 大光晕更亮");
        // 确定性：同世界同 tick → 字节逐位相同
        assert_eq!(
            lit,
            render_world(&world_bright_sprite(Some((0.5, 1.0))), 64, 64, &Assets::empty(), 0)
                .unwrap()
        );
    }

    #[test]
    fn bloom_threshold_one_changes_nothing() {
        // threshold=1.0：没有通道能超过 255 → 亮部全零 → 字节与无 Bloom 实体逐位相同
        let plain = render_world(&world_bright_sprite(None), 64, 64, &Assets::empty(), 0).unwrap();
        let capped =
            render_world(&world_bright_sprite(Some((1.0, 2.0))), 64, 64, &Assets::empty(), 0)
                .unwrap();
        assert_eq!(plain, capped);
        // strength=0 同理：加 0 不改字节（u8→f32→u8 往返精确）
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
        // threshold 越界
        let mut w = World::new();
        let b = w.spawn();
        w.set_component(b, "Bloom", json!({"threshold": 1.5, "strength": 1.0})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Bloom.threshold"), "{err}");
        // strength 为负
        w.set_component(b, "Bloom", json!({"threshold": 0.5, "strength": -1.0})).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Bloom.strength"), "{err}");
        // 字段缺失：显式报错并给写法
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
        // 没 Bloom：describe 里不出现泛光字段
        let d = describe_world(&world_bright_sprite(None), 64, 64).unwrap();
        assert!(d.get("bloom").is_none());
        assert!(!d["text"].as_str().unwrap().contains("泛光"));
    }

    /// 测试用 TTF：book 示例 vendored 的 DejaVu Sans（Bitstream Vera 许可，
    /// 见 examples/book/fonts/LICENSE）。
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

    /// 收集所有非背景像素的坐标。
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
        // 素材仓库不挂字体 = 点阵旧行为：与 Assets::empty() 渲出的字节逐位相同
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
        // size=3、scale=8 → 字号 24px：所有文字像素都该落在 Position（屏幕中心）附近
        // 的包围盒里（横向按两字符比例宽放余量，竖向按字号一半放余量）
        for &(x, y) in &hits {
            assert!((24..=72).contains(&x) && (30..=66).contains(&y), "({x},{y}) 跑出文字包围盒");
        }
        // 字形内部应有满覆盖像素 = 精确的 Text.color（抗锯齿只在边缘）
        assert!(
            hits.iter().any(|&(x, y)| pixel(&buf, 96, x, y) == [0, 255, 0, 255]),
            "应存在满覆盖像素"
        );
        // 确定性：同世界同 tick → 字节逐位相同（缓存命中与否不影响输出）
        assert_eq!(buf, render_world(&w, 96, 96, &assets, 0).unwrap());
        // 比例字距："iii" 比 "WWW" 窄（点阵等宽做不到这一点）
        let font = assets.font().unwrap();
        let (_, narrow) = font.layout("iii", 24);
        let (_, wide) = font.layout("WWW", 24);
        assert!(narrow < wide, "比例字距: iii({narrow}) 应窄于 WWW({wide})");
    }

    #[test]
    fn vector_font_renders_cjk_with_nonempty_coverage() {
        // CJK 字符走矢量路径必须画出东西：字体含该字形就是真字，不含（如 DejaVu）
        // 就是该字体的 .notdef 豆腐块——明确可见，不是静默消失
        let assets = assets_with_font();
        let g = assets.font().unwrap().raster('中', 24);
        assert!(!g.coverage.is_empty(), "CJK 字符栅格化覆盖率不应为空");
        assert!(g.coverage.iter().any(|&c| c > 0));
        let w = world_with_text("中文");
        let buf = render_world(&w, 96, 96, &assets, 0).unwrap();
        assert!(!non_background(&buf, 96, 96).is_empty(), "CJK 文字应有可见像素");
    }

    /// reveal 缺省 / ≥1 与未引入本特性时逐字节相同（向后兼容，两条路径都验）。
    #[test]
    fn reveal_full_or_absent_is_byte_identical() {
        // 矢量路径
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
        // reveal=2（>1）也全显，等价
        w_full.set_field(e, "Text.reveal", json!(2.0)).unwrap();
        assert_eq!(
            render_world(&w_plain, 128, 64, &assets, 0).unwrap(),
            render_world(&w_full, 128, 64, &assets, 0).unwrap(),
        );
        // 点阵路径同样逐字节相同
        assert_eq!(
            render_world(&w_plain, 128, 64, &Assets::empty(), 0).unwrap(),
            render_world(&w_full, 128, 64, &Assets::empty(), 0).unwrap(),
        );
    }

    /// reveal 驱动下可见字数 = 纯函数：reveal 越大画的字越多、像素单调不减，
    /// 且 reveal<1 画出的是全显的真子集（逐字显示是"往右长"，不重排）。
    #[test]
    fn reveal_progressively_shows_more_pixels() {
        let assets = assets_with_font();
        let mut w = World::new();
        let e = w.spawn_named("label").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(e, "Text", json!({"content": "ABCDEF", "size": 3.0, "color": "#00ff00", "reveal": 0.0}))
            .unwrap();
        // reveal=0：一个字都不画
        let none = render_world(&w, 160, 64, &assets, 0).unwrap();
        assert!(non_background(&none, 160, 64).is_empty(), "reveal=0 不该画任何字");
        // 逐步放开：像素数单调不减
        let mut prev = 0usize;
        for r in [0.34_f64, 0.67, 1.0] {
            w.set_field(e, "Text.reveal", json!(r)).unwrap();
            let buf = render_world(&w, 160, 64, &assets, 0).unwrap();
            let hits = non_background(&buf, 160, 64).len();
            assert!(hits >= prev, "reveal={r} 像素数应不少于更小的 reveal（{hits} < {prev}）");
            prev = hits;
        }
        // 半显的字形落在全显的左半区（同一份排版切片，不左右抖）：
        // reveal=0.5（3 个字 ABC）的最右像素必须 < 全显最右像素
        w.set_field(e, "Text.reveal", json!(0.5)).unwrap();
        let half = render_world(&w, 160, 64, &assets, 0).unwrap();
        w.set_field(e, "Text.reveal", json!(1.0)).unwrap();
        let full = render_world(&w, 160, 64, &assets, 0).unwrap();
        let max_x = |buf: &[u8]| non_background(buf, 160, 64).iter().map(|&(x, _)| x).max();
        assert!(max_x(&half) < max_x(&full), "半显的字应是全显的左前缀，不越过全显右缘");
    }

    /// 性能预算第 3 条：同一段文字播 N tick，排版（layout 算法）只发生 1 次。
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
        // 打字机：reveal 从 0 渐进到 1，渲 40 帧（每帧改可见字数，绝不重排整段）
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
        // 损坏的字体：能读到字节但解析失败，同样点名路径
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
        // rot 写成字符串：显式报错（不是静默当 0）
        w.set_component(e, "Sprite", json!({"w": 1.0, "h": 1.0, "color": "#ff0000", "rot": "45"}))
            .unwrap();
        let err = render_world(&w, 32, 32, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Sprite.rot"), "{err}");
    }

    // ---- 文字可读性警告（真实事故原型：米色字叠米色卡面，建造者 agent 看不见）----

    /// 白底精灵 + 一条文字（位置/颜色可调）的脚手架。
    /// 缺省相机：8 像素/单位 → 64x64 视口可见世界 ±4 单位。
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
        // 白字叠白底：对比度 ≈ 1，必须给 low-contrast-text 警告 + 摘要 ⚠ 行
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
        // 事故原型：米色字（#f5e8cc）叠米色底也必须被抓住
        let d = describe_world(&world_text_on_sprite("#f0e6c8", "#f5e8cc", 0.0), 64, 64).unwrap();
        assert!(d.get("warnings").is_some(), "米色叠米色必须有警告");
    }

    #[test]
    fn describe_no_warning_on_dark_background() {
        // 同一条白字落在深色底（缺省背景色）上：不警告、不出现 warnings 键
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
        // 同样的白叠白搬到视野外（±4 单位之外）：不渲不测，没有警告
        let w = world_text_on_sprite("#ffffff", "#ffffff", 100.0);
        let d = describe_world(&w, 64, 64).unwrap();
        assert_eq!(d["texts"][0]["region"], json!("视野外"));
        assert!(d.get("warnings").is_none(), "视野外的文字不进对比度测量");
    }

    // ---- 2D 投影（Ambient.shadows + Solid 遮光体）----

    /// 投影测试脚手架：黑环境（shadows 字段可配）+ 原点白点光（radius 6 = 48px，
    /// 盖满 64x64 视口大半）。光照作用于整帧（背景也被打光）——不放精灵，
    /// 亮暗变化全部来自光照/投影，不混入绘制差异。
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

    /// 在 (x,y) 放一面 cw×ch 的遮光墙（Solid+Position+Collider，刻意不挂 Sprite——
    /// 画面上隐形，像素差异只能来自它挡光）。
    fn add_wall(w: &mut World, x: f64, y: f64, cw: f64, ch: f64) -> vitric_ecs::EntityId {
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": x, "y": y})).unwrap();
        w.set_component(e, "Collider", json!({"w": cw, "h": ch})).unwrap();
        w.set_component(e, "Solid", json!({})).unwrap();
        e
    }

    #[test]
    fn shadows_off_is_byte_identical() {
        // 三组对照逐字节锁死"不开 = 没这回事"：
        // 1) shadows 字段缺省 vs 显式 false（schema 默认值会把 false 物化进组件）
        let mut absent = world_shadow_scene(None);
        add_wall(&mut absent, 2.0, 0.0, 1.0, 2.0);
        let mut explicit_off = world_shadow_scene(Some(json!(false)));
        add_wall(&mut explicit_off, 2.0, 0.0, 1.0, 2.0);
        let buf_absent = render_world(&absent, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(buf_absent, render_world(&explicit_off, 64, 64, &Assets::empty(), 0).unwrap());
        // 2) 关着时 Solid 墙对画面零影响（墙没有 Sprite，挡光是它唯一可能的像素效应）
        let no_wall = world_shadow_scene(None);
        assert_eq!(buf_absent, render_world(&no_wall, 64, 64, &Assets::empty(), 0).unwrap());
        // 3) 开了但场上没有遮光体：和关着逐字节相同（空列表不改算术）
        let on_empty = world_shadow_scene(Some(json!(true)));
        assert_eq!(buf_absent, render_world(&on_empty, 64, 64, &Assets::empty(), 0).unwrap());
    }

    #[test]
    fn wall_casts_shadow_and_removing_it_equalizes() {
        // 灯在像素 (32,32)，墙占像素 x∈[44,52] y∈[24,40]。
        // 取两个到灯心**严格等距**的像素中心：(56,32)→fx 56.5（墙后）和
        // (7,32)→fx 7.5（对侧无墙），|dx| 同为 24.5、dy 同为 0.5——亮度差只能来自遮挡
        let mut w = world_shadow_scene(Some(json!(true)));
        let wall = add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        let behind = pixel(&buf, 64, 56, 32);
        let open = pixel(&buf, 64, 7, 32);
        assert_eq!(behind, [0, 0, 0, 255], "墙后像素被挡 = 只剩环境黑");
        assert!(open[0] > 0 && open[1] > 0, "对侧等距像素该被照亮: {open:?}");
        // 拆墙（移除 Solid 即不再是遮光体）：两侧等距像素亮度完全相等
        w.remove_component(wall, "Solid").unwrap();
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 56, 32), pixel(&buf, 64, 7, 32), "无墙时等距像素同亮");
        assert!(pixel(&buf, 64, 56, 32)[0] > 0, "拆墙后原阴影处被照亮");
    }

    #[test]
    fn pixel_on_occluder_is_lit_but_other_boxes_still_shadow_it() {
        // 规则锁死：箱子里的像素不被**自己**遮挡，但照样被**别的**箱子遮挡。
        // 像素 (48,32)（fx 48.5）在墙 x∈[44,52] 内部
        let mut w = world_shadow_scene(Some(json!(true)));
        add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        let on_wall = pixel(&buf, 64, 48, 32);
        assert!(on_wall[0] > 0, "遮光体上的像素不被自己压黑: {on_wall:?}");
        // 在灯和墙之间再立一面墙（像素 x∈[38,42]）：原来"在墙上"的像素现在被它挡住
        add_wall(&mut w, 1.0, 0.0, 0.5, 2.0);
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 48, 32), [0, 0, 0, 255], "别的箱子照样遮它");
    }

    #[test]
    fn spot_light_is_shadowed_too() {
        // 聚光灯朝 +x（dir=0，锥角 90°）：墙后像素 (56,32) 在锥内但被挡 → 黑；
        // 拆墙后同像素亮——锥角衰减不豁免遮挡
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
        // 同样的墙阵关着投影不报错（上限只属于投影路径）
        let mut off = world_shadow_scene(None);
        for i in 0..(MAX_OCCLUDERS + 1) {
            add_wall(&mut off, i as f64, -10.0, 1.0, 1.0);
        }
        render_world(&off, 64, 64, &Assets::empty(), 0).unwrap();
    }

    #[test]
    fn shadow_fields_are_validated_explicitly() {
        // shadows 不是 bool：显式报错（不静默当 false）
        let w = world_shadow_scene(Some(json!("yes")));
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Ambient.shadows"), "{err}");
        // 遮光体的 Collider.w 不是数字：显式报错点名字段
        let mut w = world_shadow_scene(Some(json!(true)));
        let wall = add_wall(&mut w, 2.0, 0.0, 1.0, 2.0);
        w.set_field(wall, "Collider.w", json!("wide")).unwrap();
        let err = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap_err();
        assert!(err.contains("Collider.w"), "{err}");
    }

    #[test]
    fn segment_hits_aabb_covers_axis_parallel_and_misses() {
        let bx = (4.0, 4.0, 6.0, 6.0);
        // 横穿 / 纵穿（轴平行分量 = 0 的退化分支）
        assert!(segment_hits_aabb((0.0, 5.0), (10.0, 5.0), bx), "水平穿过");
        assert!(segment_hits_aabb((5.0, 0.0), (5.0, 10.0), bx), "垂直穿过");
        assert!(!segment_hits_aabb((0.0, 7.0), (10.0, 7.0), bx), "水平线在箱外");
        assert!(!segment_hits_aabb((7.0, 0.0), (7.0, 10.0), bx), "垂直线在箱外");
        // 斜穿 / 斜过但不相交（slab 区间不重叠）
        assert!(segment_hits_aabb((0.0, 0.0), (10.0, 10.0), bx), "对角穿过");
        assert!(!segment_hits_aabb((0.0, 6.5), (10.0, 16.5), bx), "斜线擦过上方");
        // 线段截断：方向对但够不着（t > 1）
        assert!(!segment_hits_aabb((0.0, 5.0), (3.0, 5.0), bx), "线段没到箱子就停");
        // 端点在箱内也算相交（区间 [0,1] 截在箱内）
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
        // 关着（缺省）：键不出现、摘要无投影行
        let mut off = world_shadow_scene(None);
        add_wall(&mut off, 2.0, 0.0, 1.0, 2.0);
        let d = describe_world(&off, 64, 64).unwrap();
        assert!(d.get("shadows").is_none() && d.get("occluders").is_none());
        assert!(!d["text"].as_str().unwrap().contains("投影"));
    }

    /// glow 量级的合成场景：1280x720、12 盏点光、100 个瓦片遮光体（两条 40 瓦地板
    /// 加 20 根散立柱）。基准测试和等价性测试共用——盖住合并（整行地板）、
    /// 不可合并（错位立柱）、灯心落在地板带内（箱内像素路径）这几类形态。
    fn world_glow_like_scene() -> World {
        let mut w = World::new();
        let amb = w.spawn();
        w.set_component(amb, "Ambient", json!({"color": "#101018", "shadows": true})).unwrap();
        // 两条 40 瓦的地板（整行可合并成一根长条）
        for row in 0..2 {
            let y = if row == 0 { -2.0 } else { 6.0 };
            for i in 0..40 {
                add_wall(&mut w, -19.5 + i as f64, y, 1.0, 1.0);
            }
        }
        // 20 根散立柱（间隔 3，留缝不可合并）
        for i in 0..20 {
            add_wall(&mut w, -28.5 + i as f64 * 3.0, 2.0, 1.0, 2.0);
        }
        // 12 盏点光：一半飘在地板上方，一半灯心埋进地板带（箱内像素也要走遮挡）
        for i in 0..12 {
            let lamp = w.spawn();
            let x = -22.0 + i as f64 * 4.0;
            let y = if i % 2 == 0 { 1.0 } else { -2.0 };
            w.set_component(lamp, "Position", json!({"x": x, "y": y})).unwrap();
            w.set_component(lamp, "Light", json!({"radius": 10.0, "intensity": 1.2})).unwrap();
        }
        w
    }

    /// 基准（默认忽略，手动跑：`cargo test --release -p vitric-render -- --ignored shadow_bench --nocapture`）。
    /// 1280x720 · 12 灯 · 100 遮光体——优化前这一帧是 像素×灯×箱子 的全乘积。
    #[test]
    #[ignore]
    fn shadow_bench_glow_like_scene() {
        let w = world_glow_like_scene();
        // 预热一次（占位分配、页错误不进计时）
        render_world(&w, 1280, 720, &Assets::empty(), 0).unwrap();
        let n = 5;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            render_world(&w, 1280, 720, &Assets::empty(), 0).unwrap();
        }
        let per_frame = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("shadow_bench: 1280x720 · 12 灯 · 100 遮光体 = {per_frame:.1} ms/帧");
    }

    /// 等价性基准：每个遮光体各自成组（不合并）——外测路径就是原始逐箱算式。
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

    /// 等价性测试的加强版场景：在 glow 量级场景上再加聚光灯、重叠墙、退化墙（w=0）
    /// 和非默认（但二进制可精确表示的）相机——盖住合并判定的全部分支。
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
        add_wall(&mut w, 0.25, -2.0, 1.0, 1.0); // 与地板行重叠但不贴齐：不许合并
        add_wall(&mut w, 5.0, 0.5, 0.0, 2.0); // 退化墙（w=0）：不参与合并，行为原样
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 0.5, "y": -0.25, "scale": 4.0})).unwrap();
        w
    }

    #[test]
    fn merged_and_culled_shadows_are_byte_identical_to_naive() {
        // 优化路径（render_world：合并 + 逐灯剔除）与对照路径（逐箱全量、不剔除）
        // 输出逐字节相同——合并/剔除都不许改一个字节。
        let (width, height) = (320u32, 180u32);
        let mut w = world_equivalence_scene();
        let optimized = render_world(&w, width, height, &Assets::empty(), 0).unwrap();

        let lights = collect_lights(&w).unwrap();
        let occs = collect_occluders(&w).unwrap();
        let cam = camera_of(&w, 0, height).unwrap();
        let (ambient, _) = ambient_of(&w).unwrap().unwrap();
        // 不点亮的底版：拿掉 Ambient = 光照整体关闭，其余绘制完全相同
        let amb = w.query(&["Ambient"])[0];
        w.remove_component(amb, "Ambient").unwrap();
        let unlit = render_world(&w, width, height, &Assets::empty(), 0).unwrap();
        assert_ne!(optimized, unlit, "场景必须真的被光照改写过，否则测试空转");

        // 对照 1：不合并、不剔除（原始逐箱逐灯全量算式）
        let mut naive = unlit.clone();
        let grid = naive_grid(&occs, width, height, cam);
        apply_lighting_impl(&mut naive, width, height, cam, ambient, &lights, &grid, false, None);
        assert_eq!(optimized, naive, "合并+剔除改了输出字节");

        // 对照 2：合并、不剔除——单独锁死"逐灯剔除无损"
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
        // 10 块 1x1 瓦片贴齐成行（中心 x = 0..9）→ 合并成一根横条
        for i in 0..10 {
            add_wall(&mut w, i as f64, 0.0, 1.0, 1.0);
        }
        add_wall(&mut w, 12.0, 0.0, 1.0, 1.0); // 留缝：不并
        add_wall(&mut w, 0.0, 3.0, 1.0, 1.0); // 同列但 y 不贴齐：不并
        let occs = collect_occluders(&w).unwrap();
        let g = build_shadow_boxes(&occs, 64, 64, (0.0, 0.0, 8.0));
        assert_eq!(g.merged.len(), 3, "一根横条 + 两个孤箱");
        assert_eq!(g.subs.len(), 12, "子箱总数 = 原始遮光体数");
        let slab = g.merged.iter().find(|m| m.sub_len == 10).expect("瓦片行收成一组");
        // 行世界 x ∈ [-0.5, 9.5]、y ∈ [-0.5, 0.5] → 像素 [28, 28, 108, 36]（scale 8）
        assert_eq!(slab.aabb, [28.0, 28.0, 108.0, 36.0]);

        // 2x2 瓦片阵：先沿 x 收成两条、再沿 y 摞成一块（4 个子箱）
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
        add_wall(&mut w, 1.0, 0.0, 1.0, 1.0); // 灯盘内：保留
        add_wall(&mut w, 20.0, 0.0, 1.0, 1.0); // 远在灯盘外：剔除
        let occs = collect_occluders(&w).unwrap();
        let g = build_shadow_boxes(&occs, 64, 64, (0.0, 0.0, 8.0));
        // 灯在像素 (32,32)，半径 6*8=48px。近箱中心 (40,32)、远箱中心 (192,32)
        let kept = cull_shadow_boxes(&g, 32.0, 32.0, 48.0);
        assert_eq!(kept.len(), 1);
        assert_eq!(g.merged[kept[0] as usize].aabb, [36.0, 28.0, 44.0, 36.0]);
        // 灯心埋在箱子里：最近距离 0，必然保留
        let kept = cull_shadow_boxes(&g, 40.0, 32.0, 1.0);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn describe_contrast_check_tolerates_missing_images() {
        // 贴图不在素材仓库：对比度测量退 Sprite.color 纯色块近似，describe 不报错。
        // （正常渲染 render_world 对缺图仍是显式报错——宽容只属于测量这条内部路径）
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

    // ---- 粒子发射器（Emitter，纯渲染层产物）----

    /// 火花流测试场：一个 stream 发射器（每秒 60 粒、寿命 30 tick）。
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
        // 不同 tick 画面演化（粒子在动）
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
        // 稳态可见数 = rate·lifetime/60 = 60·30/60 = 30
        assert_eq!(p1.len(), 30, "稳态在途粒子数");
        // 早期（tick < lifetime）只有已出生的：tick 5 → 出生 tick 0..=5 共 6 批×1
        assert_eq!(emitter_particles(e, 5).len(), 6);
        // 老粒子在前（先画在底下）：首个粒子比末个老 → alpha 更低
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
        // burst 缺省 -1 = 未触发
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
        // spread 0 + 固定初速 + 重力：位置必须严格等于解析式 origin + v0·t + ½g·t²
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
            // dir=90（+y）、spread=0：x 不动（cos90 的浮点尾数 ≈ 0）
            assert!((p.x - 1.0).abs() < 1e-9, "age {age}: x={}", p.x);
            let expect_y = 2.0 + 6.0 * t + 0.5 * (-10.0) * t * t;
            assert!((p.y - expect_y).abs() < 1e-9, "age {age}: y={} 应为 {expect_y}", p.y);
            // alpha 线性淡出
            let expect_a = (255.0 * (1.0 - age as f64 / 60.0)).round() as u8;
            assert_eq!(p.rgba[3], expect_a, "age {age}");
        }
    }

    #[test]
    fn emitter_off_or_absent_keeps_bytes_identical() {
        // 同一世界：没有发射器 vs 挂了 active=false 的发射器 → 输出逐字节相同
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
        // 全黑环境光：精灵被压黑，粒子照亮自己（自发光约定）
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
        // burst@0、age 0、speed 0 → 粒子停在原点 = 屏幕中心，alpha 255 纯绿
        let buf = render_world(&w, 64, 64, &Assets::empty(), 0).unwrap();
        assert_eq!(pixel(&buf, 64, 32, 32), [0, 255, 0, 255], "粒子不被全黑环境光压暗");
        // 粒子边界外的精灵像素确实被压黑了（光照在跑）
        assert_eq!(pixel(&buf, 64, 26, 26), [0, 0, 0, 255], "精灵照常被打光");
    }

    #[test]
    fn emitter_errors_are_explicit_with_hints() {
        let mut w = World::new();
        let e = w.spawn_named("bad").unwrap();
        w.set_component(e, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        // kind 缺失
        w.set_component(e, "Emitter", json!({"lifetime": 30, "size": 0.5})).unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("kind") && err.contains("stream") && err.contains("burst"), "{err}");
        // kind 不认识
        w.set_component(e, "Emitter", json!({"kind": "fountain", "lifetime": 30, "size": 0.5}))
            .unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("fountain") && err.contains("不认识"), "{err}");
        // stream 缺 rate
        w.set_component(e, "Emitter", json!({"kind": "stream", "lifetime": 30, "size": 0.5}))
            .unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("rate") && err.contains("写法"), "{err}");
        // lifetime 非法
        w.set_component(e, "Emitter", json!({"kind": "stream", "rate": 10.0, "lifetime": 0, "size": 0.5}))
            .unwrap();
        assert!(collect_emitters(&w).unwrap_err().contains("lifetime"), "lifetime ≥ 1");
        // size 缺失
        w.set_component(e, "Emitter", json!({"kind": "stream", "rate": 10.0, "lifetime": 30}))
            .unwrap();
        assert!(collect_emitters(&w).unwrap_err().contains("size"));
        // 粒子预算超限
        w.set_component(
            e,
            "Emitter",
            json!({"kind": "stream", "rate": 6000.0, "lifetime": 600, "size": 0.5}),
        )
        .unwrap();
        let err = collect_emitters(&w).unwrap_err();
        assert!(err.contains("预算") && err.contains("1024"), "{err}");
        // 没有 Position
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
        // 没有发射器 = 没有 emitters 键
        let d2 = describe_world(&world_one_red_sprite(), 64, 64).unwrap();
        assert!(d2.get("emitters").is_none());
    }

    #[test]
    fn emitter_seed_decorrelates_neighbor_entities() {
        let a = emitter_seed(vitric_ecs::EntityId { index: 1, generation: 1 });
        let b = emitter_seed(vitric_ecs::EntityId { index: 2, generation: 1 });
        assert_ne!(a, b);
        // 同一 id 永远同种子（确定性）
        assert_eq!(a, emitter_seed(vitric_ecs::EntityId { index: 1, generation: 1 }));
    }
}
