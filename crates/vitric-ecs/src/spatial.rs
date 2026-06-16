//! 以自我为中心的空间关系（egocentric spatial relations）。
//!
//! 为什么放在 ecs 这一层：视觉/多模态模型不擅长从**绝对坐标**做空间推理
//! （「出口在我左边还是右边」「够不够得到」经常算错）。Vitric 是玻璃盒、知道
//! 所有实体位置，应该把「相对于某个焦点」的关系**预先算好**喂给模型，而不是
//! 丢一堆世界坐标让模型自己减。
//!
//! 这份计算同时被两处「AI 所见」复用：
//! - vitric-render 的 `describe_world_with_assets`（控制面 `render/describe`）；
//! - vitric-playtest 的 `SceneView`（试玩观测）。
//!
//! 两个 crate 的最低公共依赖就是 vitric-ecs（render 不依赖 data/playtest），
//! 而这只是纯位置算术——放这里两边都 call 得到、不会造成循环依赖。
//!
//! **纯函数**：只读位置/尺寸，不读 World 内部状态，可单测、不进哈希、不影响确定性。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{EntityId, World};

/// 一个实体在世界里的轴对齐占位（AABB 中心 + 半宽半高）。
/// w/h 是它的尺寸（如 `Sprite.w`/`Sprite.h`）——用来判「相邻」和「同行/同列」的容差。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// 世界坐标中心 x（y 向上）。
    pub x: f64,
    pub y: f64,
    /// 占位宽高（世界单位）。没有尺寸概念时给 0：相邻判定退化为「中心重合才算挨着」，
    /// 同行/同列退化为「中心严格对齐才算」——比凭空假设一个尺寸安全。
    pub w: f64,
    pub h: f64,
}

impl Placement {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Placement {
        Placement { x, y, w, h }
    }
}

/// 以焦点为中心的空间关系（focal → target）。所有方向/同行同列都按**世界坐标**算
/// （y 向上），不掺屏幕翻转——语义观察对的是世界本体。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelativeSpatial {
    /// 8 方位词（世界坐标，y 向上）：focal 看 target 在哪个方向。
    pub direction: Direction,
    /// 欧氏距离（中心到中心，世界单位）。
    pub distance: f64,
    /// 近似同一行（竖向偏移在容差内）。
    pub same_row: bool,
    /// 近似同一列（横向偏移在容差内）。
    pub same_col: bool,
    /// 紧挨/接触（两个 AABB 边缘接近或重叠）——回答「够不够得到」。
    pub adjacent: bool,
    /// 视线被挡：焦点中心→目标中心的连线穿过了**第三方 Solid 实体**的 AABB
    /// （回答「我看得见它吗 / 中间有墙吗」）。纯 [`relate`]（只有位置）算不出遮挡，
    /// 恒为 false；要带遮挡得走世界感知的 [`relate_in_world`]。
    pub blocked: bool,
}

/// 8 方位（外加「重合」）。世界坐标、y 向上：up = +y、right = +x。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Right,
    UpRight,
    Up,
    UpLeft,
    Left,
    DownLeft,
    Down,
    DownRight,
    /// 焦点与目标中心几乎重合（方向无意义）。
    Coincident,
}

impl Direction {
    /// 给模型/JSON 读的英文方位词（连字符式，和常见 8 方位写法一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::UpRight => "up-right",
            Direction::Up => "up",
            Direction::UpLeft => "up-left",
            Direction::Left => "left",
            Direction::DownLeft => "down-left",
            Direction::Down => "down",
            Direction::DownRight => "down-right",
            Direction::Coincident => "here",
        }
    }
}

/// 距离四舍五入的精度（小数点后两位）：世界单位通常以「格」为量纲，两位足够区分
/// 邻格/隔格，又不会把浮点末位噪声泄进观测。
const DISTANCE_DP: f64 = 100.0;

/// 中心重合判定阈值（世界单位）：两中心欧氏距离小于它就当「重合」，方向无意义。
/// 取得很小（千分之一格），只挡真正同点的退化情形。
const COINCIDENT_EPS: f64 = 1e-3;

/// 相邻判定的间隙容差（世界单位）：两 AABB 在某轴的边缘间隙 ≤ 它就算「贴着」。
/// 取得很小，避免把「隔着半格」误判成挨着；边缘逐位贴齐/重叠时间隙为 0，天然命中。
const ADJACENT_GAP_EPS: f64 = 1e-6;

/// 算焦点 → 目标的以自我为中心关系（纯函数，世界坐标 y 向上）。
///
/// - `direction`：按 (dx, dy) 落到 8 方位之一。判定用 2:1 的斜率带——`|dy| < dx/2`
///   归正左右、`|dx| < dy/2` 归正上下，介于其间归对角。中心重合（距离 < eps）= `here`。
/// - `distance`：中心到中心的欧氏距离，四舍五入到两位小数。
/// - `same_row` / `same_col`：竖/横向偏移是否在「半个 sprite 高/宽」容差内。容差取两者
///   高/宽的平均的一半（`(h_f + h_t) / 4`）——给小目标和大目标一个对称、随尺寸自适应的带宽。
/// - `adjacent`：两 AABB 是否边缘接近或重叠。两轴间隙都 ≤ eps 才算（沿一条边贴着、
///   或部分重叠都满足；隔开一道缝就不算）。
pub fn relate(focal: Placement, target: Placement) -> RelativeSpatial {
    let dx = target.x - focal.x;
    let dy = target.y - focal.y;
    let distance = (dx * dx + dy * dy).sqrt();

    let direction = if distance < COINCIDENT_EPS {
        Direction::Coincident
    } else {
        direction_of(dx, dy)
    };

    // 同行/同列容差：半个「平均 sprite 高/宽」。尺寸为 0 时容差为 0 → 退化成严格对齐。
    let row_tol = (focal.h + target.h) / 4.0;
    let col_tol = (focal.w + target.w) / 4.0;
    let same_row = dy.abs() <= row_tol;
    let same_col = dx.abs() <= col_tol;

    // 相邻：两轴的边缘间隙都 ≤ eps。gap = max(0, |中心差| - 两半边之和)；
    // 负值（重叠）夹到 0，重叠自然算相邻。
    let gap_x = (dx.abs() - (focal.w + target.w) / 2.0).max(0.0);
    let gap_y = (dy.abs() - (focal.h + target.h) / 2.0).max(0.0);
    let adjacent = gap_x <= ADJACENT_GAP_EPS && gap_y <= ADJACENT_GAP_EPS;

    RelativeSpatial {
        direction,
        distance: (distance * DISTANCE_DP).round() / DISTANCE_DP,
        same_row,
        same_col,
        adjacent,
        // 纯位置算不出遮挡：没有世界就不知道中间有没有墙。恒 false，
        // 由 relate_in_world 在有 World 时覆盖。
        blocked: false,
    }
}

/// 世界感知的关系：在 [`relate`]（纯位置）之上，再算焦点→目标的视线有没有被
/// **第三方 Solid 实体**挡住（`blocked`）。
///
/// 为什么单列一个世界感知版本：纯 [`relate`] 只吃两个 `Placement`，无从知道场上还有
/// 没有别的墙——遮挡是「世界里的第三方」才有的概念。这个函数取两者占位调 `relate` 拿
/// 方向/距离/同行同列/相邻，再补一个几何判定：焦点中心→目标中心的线段，是否穿过任何
/// **既不是焦点也不是目标**的 Solid 实体的 AABB。
///
/// - Solid 实体 = 带 `Solid` 组件的实体（引擎里「挡身体的东西」，开了投影也挡光，
///   见 vitric-render 的遮光体约定）。AABB 取 `Position`(中心) + `Collider`(w/h)；缺
///   Position/Collider 的 Solid 不参与（拿不到盒子就当它不挡）。
/// - 线段-AABB 求交用标准 slab 法（[`segment_hits_aabb`]，纯几何、确定）。
/// - Solid 枚举走 `World::query` 的槽位序（BTreeMap，确定）——确定性不依赖遍历顺序
///   （命中是「任一」，顺序无关），但有序遍历让行为可复现。
/// - 焦点或目标本身即使是 Solid 也不算遮挡自己（线段两端贴着自己的盒子边，slab 会判
///   命中——明确排除掉，否则「站在墙里看东西」永远 blocked）。
///
/// 焦点或目标读不到 Placement（无 Position）时退回纯 [`relate`] 不可行——这里要求两者
/// 都有占位才有意义；调用方负责保证（描述/SceneView 都只在两者有 Position 时才调）。
pub fn relate_in_world(world: &World, focal: EntityId, target: EntityId) -> RelativeSpatial {
    let fp = placement_of(world, focal).unwrap_or(Placement::new(0.0, 0.0, 0.0, 0.0));
    let tp = placement_of(world, target).unwrap_or(Placement::new(0.0, 0.0, 0.0, 0.0));
    let mut rel = relate(fp, tp);
    rel.blocked = line_of_sight_blocked(world, focal, target, fp, tp);
    rel
}

/// 焦点中心→目标中心的视线是否被第三方 Solid 挡住（[`relate_in_world`] 的几何内核）。
/// 排除焦点和目标自身；命中任一第三方 Solid 的 AABB 即 true。
fn line_of_sight_blocked(
    world: &World,
    focal: EntityId,
    target: EntityId,
    fp: Placement,
    tp: Placement,
) -> bool {
    let seg_p = (fp.x, fp.y);
    let seg_q = (tp.x, tp.y);
    for id in world.query(&["Solid", "Position", "Collider"]) {
        // 焦点/目标自己不挡自己的视线
        if id == focal || id == target {
            continue;
        }
        let Some(b) = solid_aabb(world, id) else { continue };
        if segment_hits_aabb(seg_p, seg_q, b) {
            return true;
        }
    }
    false
}

/// 读一个 Solid 实体的 AABB [x0, y0, x1, y1]（世界坐标）：中心 = `Position`，
/// 半宽半高 = `Collider.w/h` 的一半。任一字段缺失/非数 → None（拿不到盒子当它不挡）。
fn solid_aabb(world: &World, id: EntityId) -> Option<(f64, f64, f64, f64)> {
    let pos = world.get_component(id, "Position").ok()?;
    let x = pos.get("x").and_then(Value::as_f64)?;
    let y = pos.get("y").and_then(Value::as_f64)?;
    let col = world.get_component(id, "Collider").ok()?;
    let w = col.get("w").and_then(Value::as_f64)?;
    let h = col.get("h").and_then(Value::as_f64)?;
    Some((x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0))
}

/// 读一个实体的世界占位：`Position` 必须有（缺了 → None）；尺寸取 `Sprite.w/h`，
/// 缺了当 0（和 describe/SceneView 的占位口径一致）。
fn placement_of(world: &World, id: EntityId) -> Option<Placement> {
    let pos = world.get_component(id, "Position").ok()?;
    let x = pos.get("x").and_then(Value::as_f64)?;
    let y = pos.get("y").and_then(Value::as_f64)?;
    let (w, h) = match world.get_component(id, "Sprite") {
        Ok(s) => (
            s.get("w").and_then(Value::as_f64).unwrap_or(0.0),
            s.get("h").and_then(Value::as_f64).unwrap_or(0.0),
        ),
        Err(_) => (0.0, 0.0),
    };
    Some(Placement::new(x, y, w, h))
}

/// 线段 (px,py)→(qx,qy) 与 AABB [x0,y0,x1,y1] 是否相交（slab 法）。
/// 与 vitric-render 的 `segment_hits_aabb` 同一套几何（那边是渲染私有、用于挡光；
/// 这里给语义层的视线遮挡用）：轴平行的轴（分量差 < 1e-12）退化成「起点必须落在
/// 该轴 slab 内」，不做除法（除以近零数会算出 ±inf，min/max 链上 inf 不可靠）。
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

/// ASCII 格子图的配置（[`ascii_map`] 的入参）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AsciiMapOpts {
    /// 格子边长（世界单位）。`Some` 直接用；`None` 自动推（场上 Sprite 宽高的众数，
    /// 退化兜底 1.0）——任何游戏（连续/格子）都能出一张尺度合理的粗略图。
    pub cell: Option<f64>,
    /// 窗口半径（格子数）。以焦点为中心、各方向 radius 格 → 边长 (2·radius+1)。
    /// 默认 7（[`AsciiMapOpts::default`]）→ 15×15，再大的世界也裁成这块不爆。
    pub radius: usize,
}

impl Default for AsciiMapOpts {
    fn default() -> AsciiMapOpts {
        AsciiMapOpts { cell: None, radius: 7 }
    }
}

/// 以焦点为中心的 ASCII 格子图（[`ascii_map`] 的产出）。粗略空间图，给模型导航用：
/// 比一串坐标更直观「谁在我哪个方向、隔了几格、中间有没有墙」。
#[derive(Debug, Clone, PartialEq)]
pub struct AsciiMap {
    /// 逐行字符串，行 0 在**上**（世界 y 大的在上——和画面/方位词一致）。
    pub rows: Vec<String>,
    /// 符号 → 实体名/id 的图例（确定序：BTreeMap 按符号字符排）。
    /// `@`=焦点、`#`=Solid 遮挡物不进图例（含义固定）。
    pub legend: BTreeMap<char, String>,
    /// 实际用的格子边长（世界单位；自动推或来自 opts）。
    pub cell: f64,
    /// 焦点在网格里的 [行, 列]（恒在正中：[radius, radius]）。
    pub focal_rc: [usize; 2],
}

impl AsciiMap {
    /// 序列化成两处「AI 所见」共用的 `ascii_map` 块：
    /// `{"grid":["...","..."],"legend":{...},"cell_size":n,"focal_at":[r,c]}`。
    pub fn to_json(&self) -> Value {
        json!({
            "grid": self.rows,
            "legend": self.legend.iter().map(|(k, v)| (k.to_string(), Value::String(v.clone())))
                .collect::<serde_json::Map<String, Value>>(),
            "cell_size": self.cell,
            "focal_at": [self.focal_rc[0], self.focal_rc[1]],
        })
    }
}

/// 自动推格子边长：场上**有 Sprite 的实体**的 (w, h) 里出现最多的那个尺寸值（众数）。
/// w 和 h 一起统计（一个正方形精灵贡献两票同值）。没有任何 Sprite 尺寸 → 兜底 1.0。
/// 用众数而非平均/最大：最常见的精灵尺寸就是这个游戏的「一格」量纲，对偶发的大背景
/// 块/小道具都鲁棒。浮点值用「四舍五入到 6 位的位串」做键避免 NaN/末位噪声分票。
fn infer_cell(world: &World) -> f64 {
    // 键 = 尺寸值量化后的 i64（×1e6 四舍五入），值 = (出现次数, 原始尺寸)
    let mut tally: BTreeMap<i64, (usize, f64)> = BTreeMap::new();
    for id in world.query(&["Sprite"]) {
        if let Ok(s) = world.get_component(id, "Sprite") {
            for key in ["w", "h"] {
                if let Some(v) = s.get(key).and_then(Value::as_f64) {
                    if v > 0.0 {
                        let q = (v * 1e6).round() as i64;
                        let e = tally.entry(q).or_insert((0, v));
                        e.0 += 1;
                    }
                }
            }
        }
    }
    // 取出现次数最多的；平手取尺寸值小的（BTreeMap 按量化键升序遍历，reduce 用**严格 `>`**：
    // count 相等时保留先遇到的 a，而先遇到的是较小尺寸 → 平手留下小尺寸）
    tally
        .values()
        .copied()
        .reduce(|a, b| if b.0 > a.0 { b } else { a })
        .map(|(_, size)| size)
        .unwrap_or(1.0)
}

/// 以焦点为中心画一张有界 ASCII 格子图（纯函数，同世界同焦点出同图）。
///
/// 设计（见 vitric 的「最佳模型可读场景视图」）：
/// - **格子边长 cell**：`opts.cell` 给了就用；否则自动推 = 场上 Sprite 宽高众数
///   （[`infer_cell`]，兜底 1.0）。
/// - **有界窗口**：以焦点为中心、半径 `opts.radius` 格（默认 7 → 15×15）。世界量化到格：
///   实体中心相对焦点的偏移除以 cell 四舍五入到格下标；窗口外的实体丢弃（任何大小的
///   世界都裁成这块，不爆）。行 0 在上（世界 y 大的在上）。
/// - **符号**：`@`=焦点；`#`=Solid 遮挡物；其余实体按**确定序**（有名的优先按名字序，
///   再按 id）依次分配 `a`、`b`、…、`z`（用完字母后继续用 `A`-`Z`、`0`-`9`，仍不够
///   的实体不画——一张粗略图够用）。legend 记符号 → 名字/id。
/// - **一格多实体**：取分配序最靠前的符号占格（焦点 `@` 永远压一切；其次 `#`；再次
///   字母按分配序）。空格 = 空。
/// - **确定性**：窗口、量化、符号分配全确定，不进哈希、不影响回放。
///
/// 焦点必须有 Position（[`placement_of`] 取不到 → 返回一张只有 `@` 在正中的空图，
/// 不报错：语义视图不该因为焦点没坐标整个挂掉）。
pub fn ascii_map(world: &World, focal: EntityId, opts: &AsciiMapOpts) -> AsciiMap {
    let radius = opts.radius;
    let side = 2 * radius + 1;
    let cell = opts.cell.unwrap_or_else(|| infer_cell(world));
    let focal_rc = [radius, radius];

    // 焦点中心：取不到坐标就摆原点（只为了能算相对偏移；此时别的实体多半也落不进窗口）
    let (fx, fy) = match placement_of(world, focal) {
        Some(p) => (p.x, p.y),
        None => (0.0, 0.0),
    };

    // 每格存当前占用符号的「分配优先级」+ 字符：优先级越小越该显示（焦点 0、Solid 1、
    // 字母 2+分配序）。None = 空格。
    let mut grid: Vec<Option<(u32, char)>> = vec![None; side * side];
    let mut legend: BTreeMap<char, String> = BTreeMap::new();

    // 把一个世界坐标量化到格下标 (row, col)，落在窗口内才返回。
    // col：dx/cell 四舍五入 + 中心列；row：dy 向上为正，世界 y 大 → 行号小，故取负。
    let quantize = |x: f64, y: f64| -> Option<(usize, usize)> {
        let cidx = (x - fx) / cell;
        let ridx = (y - fy) / cell;
        let col = radius as i64 + cidx.round() as i64;
        let row = radius as i64 - ridx.round() as i64;
        if (0..side as i64).contains(&col) && (0..side as i64).contains(&row) {
            Some((row as usize, col as usize))
        } else {
            None
        }
    };

    // 一个候选要占某格：优先级更小（更该显示）才覆盖。
    let place = |grid: &mut Vec<Option<(u32, char)>>, row: usize, col: usize, prio: u32, ch: char| {
        let slot = &mut grid[row * side + col];
        if slot.map(|(p, _)| prio < p).unwrap_or(true) {
            *slot = Some((prio, ch));
        }
    };

    // 焦点：永远在正中、永远是 @（优先级 0，压一切）
    place(&mut grid, focal_rc[0], focal_rc[1], 0, '@');

    // Solid 遮挡物：优先级 1，符号固定 #（不进图例，含义固定）
    for id in world.query(&["Solid", "Position", "Collider"]) {
        if id == focal {
            continue; // 焦点自己若是 Solid，已被 @ 占；不重复画
        }
        if let Some(p) = placement_of(world, id) {
            if let Some((row, col)) = quantize(p.x, p.y) {
                place(&mut grid, row, col, 1, '#');
            }
        }
    }

    // 其余实体：确定序分配字母。序 = (有名字优先, 名字, id)——和主次排序同一立场
    // （有名的玩法主体先拿稳定字符）。焦点/Solid 已处理，跳过。
    let solids: std::collections::BTreeSet<EntityId> =
        world.query(&["Solid", "Position", "Collider"]).into_iter().collect();
    let mut others: Vec<(bool, String, EntityId)> = Vec::new();
    for id in world.query(&["Position"]) {
        if id == focal || solids.contains(&id) {
            continue;
        }
        let name = world.name_of(id).map(String::from);
        others.push((name.is_some(), name.unwrap_or_default(), id));
    }
    // 有名字的排前（true 在前），再按名字字典序，最后 id 兜底
    others.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    // 符号表：a-z A-Z 0-9（62 个，够一张粗略图；超了的实体不画）
    let symbols: Vec<char> = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
    let mut next = 0usize;
    for (has_name, name, id) in &others {
        if let Some(p) = placement_of(world, *id) {
            if let Some((row, col)) = quantize(p.x, p.y) {
                if next >= symbols.len() {
                    break; // 字符用尽，剩下的不画（粗略图够用）
                }
                let ch = symbols[next];
                next += 1;
                let label = if *has_name { name.clone() } else { id.to_string() };
                legend.insert(ch, label);
                // 优先级 2 + 分配序：分配序靠前的符号在同格里压后面的
                place(&mut grid, row, col, 2 + (next as u32), ch);
            }
        }
    }

    // 渲染成行（行 0 在上）
    let rows: Vec<String> = (0..side)
        .map(|r| {
            (0..side)
                .map(|c| grid[r * side + c].map(|(_, ch)| ch).unwrap_or(' '))
                .collect::<String>()
        })
        .collect();

    AsciiMap { rows, legend, cell, focal_rc }
}

/// (dx, dy) → 8 方位（世界坐标，y 向上）。调用方已排除中心重合的退化情形。
/// 斜率带：某轴占绝对优势（另一轴 < 本轴的一半）走正方向，否则归对角。
fn direction_of(dx: f64, dy: f64) -> Direction {
    let ax = dx.abs();
    let ay = dy.abs();
    if ay * 2.0 < ax {
        // 横向主导：正左/正右
        if dx >= 0.0 { Direction::Right } else { Direction::Left }
    } else if ax * 2.0 < ay {
        // 纵向主导：正上/正下
        if dy >= 0.0 { Direction::Up } else { Direction::Down }
    } else {
        // 对角
        match (dx >= 0.0, dy >= 0.0) {
            (true, true) => Direction::UpRight,
            (false, true) => Direction::UpLeft,
            (false, false) => Direction::DownLeft,
            (true, false) => Direction::DownRight,
        }
    }
}

impl RelativeSpatial {
    /// 序列化成两处「AI 所见」共用的 `relative_to_focal` 块。字段名给模型读：
    /// `{"direction","distance","same_row","same_col","adjacent","blocked"}`。
    /// `blocked` 走纯 [`relate`] 时恒 false（无世界 = 不知道有没有墙）；走
    /// [`relate_in_world`] 才会按第三方 Solid 算出真值。
    pub fn to_json(self) -> Value {
        json!({
            "direction": self.direction.as_str(),
            "distance": self.distance,
            "same_row": self.same_row,
            "same_col": self.same_col,
            "adjacent": self.adjacent,
            "blocked": self.blocked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没尺寸的点（w=h=0）：只测方向/距离/对齐，不掺尺寸容差。
    fn pt(x: f64, y: f64) -> Placement {
        Placement::new(x, y, 0.0, 0.0)
    }

    #[test]
    fn direction_cardinals() {
        let f = pt(0.0, 0.0);
        assert_eq!(relate(f, pt(5.0, 0.0)).direction, Direction::Right);
        assert_eq!(relate(f, pt(-5.0, 0.0)).direction, Direction::Left);
        assert_eq!(relate(f, pt(0.0, 5.0)).direction, Direction::Up, "y 向上");
        assert_eq!(relate(f, pt(0.0, -5.0)).direction, Direction::Down);
    }

    #[test]
    fn direction_diagonals() {
        let f = pt(0.0, 0.0);
        assert_eq!(relate(f, pt(5.0, 5.0)).direction, Direction::UpRight);
        assert_eq!(relate(f, pt(-5.0, 5.0)).direction, Direction::UpLeft);
        assert_eq!(relate(f, pt(-5.0, -5.0)).direction, Direction::DownLeft);
        assert_eq!(relate(f, pt(5.0, -5.0)).direction, Direction::DownRight);
    }

    #[test]
    fn direction_slope_band_prefers_cardinal_when_dominant() {
        // dx 主导（dy 不到 dx 的一半）→ 正右，不归对角
        assert_eq!(relate(pt(0.0, 0.0), pt(10.0, 2.0)).direction, Direction::Right);
        // dy 主导 → 正上
        assert_eq!(relate(pt(0.0, 0.0), pt(2.0, 10.0)).direction, Direction::Up);
    }

    #[test]
    fn coincident_center_is_here() {
        let r = relate(pt(3.0, 3.0), pt(3.0, 3.0));
        assert_eq!(r.direction, Direction::Coincident);
        assert_eq!(r.direction.as_str(), "here");
    }

    #[test]
    fn distance_euclidean_rounded() {
        // 3-4-5
        assert_eq!(relate(pt(0.0, 0.0), pt(3.0, 4.0)).distance, 5.0);
        // 四舍五入到两位
        let d = relate(pt(0.0, 0.0), pt(1.0, 1.0)).distance;
        assert_eq!(d, 1.41, "sqrt(2)≈1.41421 → 1.41");
    }

    #[test]
    fn same_row_and_col_with_tolerance() {
        // 两个 1×1 的格子，容差 = (1+1)/4 = 0.5
        let f = Placement::new(0.0, 0.0, 1.0, 1.0);
        let same_row = Placement::new(5.0, 0.4, 1.0, 1.0); // dy=0.4 ≤ 0.5
        let r = relate(f, same_row);
        assert!(r.same_row, "竖向偏移 0.4 在半格容差内");
        assert!(!r.same_col, "横向差 5 远超容差");

        let off_row = Placement::new(5.0, 0.6, 1.0, 1.0); // dy=0.6 > 0.5
        assert!(!relate(f, off_row).same_row, "竖向偏移 0.6 超容差");

        let same_col = Placement::new(0.3, 9.0, 1.0, 1.0); // dx=0.3 ≤ 0.5
        let r2 = relate(f, same_col);
        assert!(r2.same_col, "横向偏移 0.3 在半格容差内");
        assert!(!r2.same_row);
    }

    #[test]
    fn adjacent_touching_and_overlapping() {
        // 两个 2×2 格子（半宽 1），中心相距 2.0 = 边缘正好贴齐 → 相邻
        let f = Placement::new(0.0, 0.0, 2.0, 2.0);
        assert!(relate(f, Placement::new(2.0, 0.0, 2.0, 2.0)).adjacent, "边缘贴齐算相邻");
        // 重叠也算相邻
        assert!(relate(f, Placement::new(1.0, 0.0, 2.0, 2.0)).adjacent, "重叠算相邻");
        // 隔开一道缝（中心相距 2.5，间隙 0.5）→ 不相邻
        assert!(!relate(f, Placement::new(2.5, 0.0, 2.0, 2.0)).adjacent, "隔缝不算相邻");
    }

    #[test]
    fn zero_size_adjacency_is_strict() {
        // 无尺寸的点：只有中心重合才算相邻
        let f = pt(0.0, 0.0);
        assert!(relate(f, pt(0.0, 0.0)).adjacent);
        assert!(!relate(f, pt(0.5, 0.0)).adjacent);
    }

    #[test]
    fn to_json_shape() {
        let v = relate(pt(0.0, 0.0), pt(3.0, 4.0)).to_json();
        assert_eq!(v["direction"], "up-right");
        assert_eq!(v["distance"], 5.0);
        assert_eq!(v["same_row"], false);
        assert_eq!(v["same_col"], false);
        assert_eq!(v["adjacent"], false);
        // 纯 relate 无世界 → 不知有没有墙，blocked 恒 false
        assert_eq!(v["blocked"], false);
    }

    // ---- relate_in_world：视线遮挡（blocked） ----

    use crate::World;
    use serde_json::json;

    /// 放一个带 Position(+可选 Sprite 尺寸) 的实体，返回 id。
    fn spawn_at(w: &mut World, name: Option<&str>, x: f64, y: f64) -> EntityId {
        let id = match name {
            Some(n) => w.spawn_named(n).unwrap(),
            None => w.spawn(),
        };
        w.set_component(id, "Position", json!({ "x": x, "y": y })).unwrap();
        id
    }

    /// 在 (x,y) 放一面 w×h 的 Solid 墙（Solid+Position+Collider）。
    fn spawn_wall(w: &mut World, x: f64, y: f64, cw: f64, ch: f64) -> EntityId {
        let id = w.spawn();
        w.set_component(id, "Position", json!({ "x": x, "y": y })).unwrap();
        w.set_component(id, "Collider", json!({ "w": cw, "h": ch })).unwrap();
        w.set_component(id, "Solid", json!({})).unwrap();
        id
    }

    #[test]
    fn blocked_true_when_solid_between_focal_and_target() {
        // 焦点(0,0) 目标(10,0)，正中(5,0)立一面 2×4 的墙 → 视线被挡
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 0.0);
        spawn_wall(&mut w, 5.0, 0.0, 2.0, 4.0);
        assert!(relate_in_world(&w, focal, target).blocked, "中间有墙 → blocked");
    }

    #[test]
    fn blocked_false_when_solid_removed() {
        // 同上但墙挪到一边（y=20，离视线很远）→ 不挡
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 0.0);
        spawn_wall(&mut w, 5.0, 20.0, 2.0, 4.0);
        assert!(!relate_in_world(&w, focal, target).blocked, "墙不在视线上 → 不挡");
    }

    #[test]
    fn blocked_false_with_no_solid() {
        // 没有任何 Solid → blocked 恒 false（不破坏无遮挡场景）
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 0.0);
        let r = relate_in_world(&w, focal, target);
        assert!(!r.blocked);
        // 其余字段照常（和纯 relate 一致）
        assert_eq!(r.direction, Direction::Right);
        assert_eq!(r.distance, 10.0);
    }

    #[test]
    fn focal_and_target_solids_dont_block_themselves() {
        // 焦点和目标本身都是 Solid（它们的盒子盖着线段端点）：不算遮挡自己
        let mut w = World::new();
        let focal = spawn_wall(&mut w, 0.0, 0.0, 2.0, 2.0);
        let target = spawn_wall(&mut w, 10.0, 0.0, 2.0, 2.0);
        assert!(!relate_in_world(&w, focal, target).blocked, "端点自己的墙不挡自己");
        // 但中间再加第三方 Solid 就挡
        spawn_wall(&mut w, 5.0, 0.0, 2.0, 4.0);
        assert!(relate_in_world(&w, focal, target).blocked, "第三方墙照样挡");
    }

    #[test]
    fn blocked_diagonal_line_of_sight() {
        // 斜线视线：焦点(0,0)→目标(10,10)，墙在(5,5) → 挡；墙在(5,0) 偏离斜线 → 不挡
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 10.0);
        let wall = spawn_wall(&mut w, 5.0, 5.0, 2.0, 2.0);
        assert!(relate_in_world(&w, focal, target).blocked, "斜线穿墙");
        w.set_component(wall, "Position", json!({ "x": 5.0, "y": 0.0 })).unwrap();
        assert!(!relate_in_world(&w, focal, target).blocked, "墙偏离斜线不挡");
    }

    #[test]
    fn segment_aabb_boundary_cases() {
        // 线段-AABB 边界用例（直接测几何内核）
        // 1) 水平线擦着 AABB 顶边（线 y=1，盒 y∈[1,3]）→ 算命中（边界算交）
        assert!(segment_hits_aabb((0.0, 1.0), (10.0, 1.0), (4.0, 1.0, 6.0, 3.0)));
        // 2) 水平线在盒下方（线 y=0，盒 y∈[1,3]）→ 不命中
        assert!(!segment_hits_aabb((0.0, 0.0), (10.0, 0.0), (4.0, 1.0, 6.0, 3.0)));
        // 3) 竖直线穿盒（轴平行退化分支：dx≈0，起点 x 落在 slab 内）
        assert!(segment_hits_aabb((5.0, -10.0), (5.0, 10.0), (4.0, -1.0, 6.0, 1.0)));
        // 4) 竖直线在盒左侧（dx≈0，起点 x 在 slab 外）→ 不命中
        assert!(!segment_hits_aabb((0.0, -10.0), (0.0, 10.0), (4.0, -1.0, 6.0, 1.0)));
        // 5) 线段太短，够不到盒（盒在远处，线段在 t∈[0,1] 内到不了）→ 不命中
        assert!(!segment_hits_aabb((0.0, 0.0), (1.0, 0.0), (4.0, -1.0, 6.0, 1.0)));
    }

    // ---- ascii_map：以焦点为中心的格子图 ----

    #[test]
    fn ascii_map_focal_at_center() {
        // 焦点在正中、网格尺寸 = 2·radius+1
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        assert_eq!(m.rows.len(), 7, "7×7");
        assert_eq!(m.focal_rc, [3, 3]);
        assert_eq!(m.rows[3].chars().nth(3), Some('@'), "@ 在正中");
    }

    #[test]
    fn ascii_map_entity_and_solid_placed() {
        // hero(0,0) 焦点；coin(2,0) 右两格；墙(0,2) 上两格（世界 y 大 → 行号小）
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let _coin = spawn_at(&mut w, Some("coin"), 2.0, 0.0);
        spawn_wall(&mut w, 0.0, 2.0, 1.0, 1.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        // 中心 [3,3]=@；coin 在 [3,5]（同行右两格）；墙在 [1,3]（同列上两格）
        assert_eq!(m.rows[3].chars().nth(3), Some('@'));
        assert_eq!(m.rows[3].chars().nth(5), Some('a'), "coin 分到 a，右两格");
        assert_eq!(m.rows[1].chars().nth(3), Some('#'), "墙在上两格");
        assert_eq!(m.legend.get(&'a').map(String::as_str), Some("coin"));
        assert!(!m.legend.contains_key(&'#'), "# 含义固定不进图例");
    }

    #[test]
    fn ascii_map_infers_cell_from_sprite_mode() {
        // 多数精灵是 16×16，一个偏离的 64×64：自动推 cell 应为 16（众数）
        let mut w = World::new();
        let focal = w.spawn_named("hero").unwrap();
        w.set_component(focal, "Position", json!({ "x": 0.0, "y": 0.0 })).unwrap();
        w.set_component(focal, "Sprite", json!({ "w": 16.0, "h": 16.0 })).unwrap();
        for i in 0..3 {
            let e = w.spawn();
            w.set_component(e, "Position", json!({ "x": (i * 100) as f64, "y": 0.0 })).unwrap();
            w.set_component(e, "Sprite", json!({ "w": 16.0, "h": 16.0 })).unwrap();
        }
        let big = w.spawn();
        w.set_component(big, "Position", json!({ "x": 0.0, "y": 200.0 })).unwrap();
        w.set_component(big, "Sprite", json!({ "w": 64.0, "h": 64.0 })).unwrap();
        let m = ascii_map(&w, focal, &AsciiMapOpts::default());
        assert_eq!(m.cell, 16.0, "众数 16");
    }

    #[test]
    fn ascii_map_infers_cell_fallback_one() {
        // 没有任何 Sprite 尺寸 → 兜底 1.0
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts::default());
        assert_eq!(m.cell, 1.0);
    }

    #[test]
    fn ascii_map_window_clips_far_entities() {
        // 远处实体（超出 radius 格）不进图，也不进图例
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let _far = spawn_at(&mut w, Some("faraway"), 1000.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        assert!(!m.legend.values().any(|v| v == "faraway"), "窗口外不画: {:?}", m.legend);
        // 整张图只有 @，别无他物
        let non_empty: String = m.rows.join("").chars().filter(|c| *c != ' ').collect();
        assert_eq!(non_empty, "@");
    }

    #[test]
    fn ascii_map_is_deterministic() {
        // 同世界同焦点出同图
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        spawn_at(&mut w, Some("coin"), 2.0, 0.0);
        spawn_at(&mut w, Some("apple"), -1.0, 1.0);
        spawn_wall(&mut w, 3.0, 3.0, 1.0, 1.0);
        let opts = AsciiMapOpts { cell: Some(1.0), radius: 4 };
        assert_eq!(ascii_map(&w, focal, &opts), ascii_map(&w, focal, &opts));
    }

    #[test]
    fn ascii_map_named_entities_get_letters_in_order() {
        // 字母按 (有名优先, 名字序) 分配：apple 先于 coin（字典序）
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        spawn_at(&mut w, Some("coin"), 1.0, 0.0);
        spawn_at(&mut w, Some("apple"), -1.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        assert_eq!(m.legend.get(&'a').map(String::as_str), Some("apple"), "apple 字典序在前拿 a");
        assert_eq!(m.legend.get(&'b').map(String::as_str), Some("coin"));
    }

    #[test]
    fn ascii_map_to_json_shape() {
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        spawn_at(&mut w, Some("coin"), 2.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        let v = m.to_json();
        assert!(v["grid"].is_array());
        assert_eq!(v["grid"].as_array().unwrap().len(), 7);
        assert_eq!(v["cell_size"], 1.0);
        assert_eq!(v["focal_at"], json!([3, 3]));
        assert_eq!(v["legend"]["a"], json!("coin"));
    }
}
