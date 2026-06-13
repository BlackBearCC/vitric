//! UI 控件布局 — 屏幕空间叠加层的**确定性纯计算**地基（CPU/GPU 两路读同一份）。
//!
//! 定位（为什么长这样）：UI 也是实体 + 组件，进世界状态/哈希/存档/录像。和精灵/
//! 粒子相反，UI 锚定**视口（屏幕）**、不经相机变换——镜头移动/缩放/抖动 UI 不飘
//! （像 HUD）。布局是 `(UI 树, 视口尺寸)` 的纯函数：无墙钟、无随机、无跨平台分歧，
//! snapshot/restore 往返一致。
//!
//! 组件约定（引擎认名字，字段由用户 schema 定义，缺字段显式报错——和 Sprite/Light 同款）：
//! - `UiRoot {}` — 标记一棵 UI 树的根。布局从挂了 UiRoot 的实体起，对着视口解算。
//!   场上**没有** UiRoot 实体 = 完全没有 UI = 布局/渲染每 tick 零成本 early-return
//!   （旧行为字节不变、零分配零遍历）。`layout_hash` 字段（可选）缓存上次布局输入的
//!   结构哈希——没变就跳过重算（"静止 UI 连播 N tick，布局重算 0 次"的落点）。
//! - `Ui {anchor, ax, ay, ox, oy, w, h, parent, weight, rx, ry, rw, rh}` — 每个 UI 节点。
//!   * `anchor`：锚点预设名（见 [`Anchor`]），决定 (ax,ay) 取哪个预设——给了预设就用
//!     预设的归一化锚点，`"manual"` 时用显式 ax/ay（0..1，父框内比例）。
//!   * `ox/oy`：像素偏移；`w/h`：尺寸（像素，`stretch` 锚点/容器拉伸时被覆盖）。
//!   * `parent`：父 UI 节点的实体引用（空 = 直接锚到视口/根框）。
//!   * `weight`：容器主轴拉伸权重（0 = 用自身 w/h；>0 = 按权重瓜分剩余空间）。
//!   * `rx/ry/rw/rh`：**布局输出**——解算后的屏幕像素矩形（左上原点）。solver 写、
//!     渲染读，进组件 = 进哈希进存档（快照/录像安全）。
//! - `Container {kind, gap, pad, columns, main, cross}` — 挂了它，子节点（parent 指向本
//!   实体的 Ui 节点）由容器自动排版，子节点不自摆坐标（rx/ry/rw/rh 被容器算出覆盖）。
//!   `kind` ∈ {VBox, HBox, Grid}；Grid 需 `columns ≥ 1`。
//! - `Panel {color | image}` — 背景框（纯色或精灵）。渲染读它，布局不关心。
//! - `UiLabel {content, size, color, reveal, align}` — 文字控件，复用 font.rs 版面缓存 +
//!   Text.reveal（逐字显示已落地）。渲染读它，布局只用它的 Ui 框定位。
//!
//! 范围（1.1）：布局地基 + 静态控件 + 屏幕空间渲染，灰盒不可交互。Button 状态机/
//! 焦点导航/点击激活/主题属于 1.2。

use serde_json::Value;

use vitric_ecs::{EntityId, World};

/// 锚点预设：UI 节点贴父框（或视口）的哪个点。归一化坐标 (0,0)=左上、(1,1)=右下。
/// `Stretch` 是特例——拉伸填满父框（忽略自身 w/h，按 padding/offset 收边）。
/// `Manual` 用节点自己的 ax/ay（0..1 比例）当锚点，覆盖最灵活的 HUD 摆位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    /// 拉伸填满父框（ox/oy 当四边内缩，w/h 被忽略）。
    Stretch,
    /// 用节点自己的 ax/ay（父框内 0..1 比例）。
    Manual,
}

/// 全部合法的锚点预设名（check 校验 + 错误提示用，和 [`Anchor::parse`] 一一对应）。
pub const ANCHOR_NAMES: &[&str] = &[
    "top-left",
    "top-center",
    "top-right",
    "center-left",
    "center",
    "center-right",
    "bottom-left",
    "bottom-center",
    "bottom-right",
    "stretch",
    "manual",
];

impl Anchor {
    /// 预设名 → 锚点。未知名返回 `None`（调用方报错带合法名清单）。
    pub fn parse(s: &str) -> Option<Anchor> {
        Some(match s {
            "top-left" => Anchor::TopLeft,
            "top-center" => Anchor::TopCenter,
            "top-right" => Anchor::TopRight,
            "center-left" => Anchor::CenterLeft,
            "center" => Anchor::Center,
            "center-right" => Anchor::CenterRight,
            "bottom-left" => Anchor::BottomLeft,
            "bottom-center" => Anchor::BottomCenter,
            "bottom-right" => Anchor::BottomRight,
            "stretch" => Anchor::Stretch,
            "manual" => Anchor::Manual,
            _ => return None,
        })
    }

    /// 预设对应的父框内归一化锚点 (0..1, 0..1)。Stretch/Manual 不走这条（特判）。
    fn norm(self) -> (f64, f64) {
        match self {
            Anchor::TopLeft => (0.0, 0.0),
            Anchor::TopCenter => (0.5, 0.0),
            Anchor::TopRight => (1.0, 0.0),
            Anchor::CenterLeft => (0.0, 0.5),
            Anchor::Center => (0.5, 0.5),
            Anchor::CenterRight => (1.0, 0.5),
            Anchor::BottomLeft => (0.0, 1.0),
            Anchor::BottomCenter => (0.5, 1.0),
            Anchor::BottomRight => (1.0, 1.0),
            // 下面两个特判，不该走到（保留全枚举一致性）
            Anchor::Stretch => (0.0, 0.0),
            Anchor::Manual => (0.0, 0.0),
        }
    }
}

/// 容器类型（自动排版子节点）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    /// 竖排：子节点沿 y 顺序排。
    VBox,
    /// 横排：子节点沿 x 顺序排。
    HBox,
    /// 网格：固定列数，行高列宽等分。
    Grid,
}

/// 全部合法的容器类型名（check 校验 + 错误提示用）。
pub const CONTAINER_KINDS: &[&str] = &["VBox", "HBox", "Grid"];

impl ContainerKind {
    pub fn parse(s: &str) -> Option<ContainerKind> {
        Some(match s {
            "VBox" => ContainerKind::VBox,
            "HBox" => ContainerKind::HBox,
            "Grid" => ContainerKind::Grid,
            _ => return None,
        })
    }
}

/// 主轴/交叉轴对齐。容器把子节点排进主轴方向，cross 决定垂直于主轴那边怎么贴。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

impl Align {
    pub fn parse(s: &str) -> Option<Align> {
        Some(match s {
            "start" => Align::Start,
            "center" => Align::Center,
            "end" => Align::End,
            _ => return None,
        })
    }
    /// 在 `free` 像素的剩余空间里，按对齐方式给出起点偏移。
    fn offset(self, free: f64) -> f64 {
        match self {
            Align::Start => 0.0,
            Align::Center => free / 2.0,
            Align::End => free,
        }
    }
}

/// 全部合法对齐名（check 校验用）。
pub const ALIGN_NAMES: &[&str] = &["start", "center", "end"];

/// 一个已解算的屏幕像素矩形（左上原点，和渲染 buf 同坐标系）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 布局算法真正执行过的次数（全局原子计数，测试专用）。
/// 学 font.rs 的 `layout_runs`：断言"静止 UI 连播 N tick，布局只算一次"。
/// 不进任何渲染输出，纯可观测。
static LAYOUT_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 布局算法执行总次数（进程级累计）。测试里先读基线、跑若干 tick 再读差值。
pub fn layout_runs() -> u64 {
    LAYOUT_RUNS.load(std::sync::atomic::Ordering::Relaxed)
}

/// 场上是否有任何 UI（挂 UiRoot 的实体）。没有 = 整条 UI 路径零成本跳过。
pub fn has_ui(world: &World) -> bool {
    !world.query(&["UiRoot"]).is_empty()
}

/// 一个 UI 节点的静态输入（解算前从组件读出来；rx/ry/rw/rh 是输出不读）。
struct Node {
    id: EntityId,
    anchor: Anchor,
    ax: f64,
    ay: f64,
    ox: f64,
    oy: f64,
    w: f64,
    h: f64,
    parent: Option<EntityId>,
    weight: f64,
    container: Option<ContainerSpec>,
}

#[derive(Clone, Copy)]
struct ContainerSpec {
    kind: ContainerKind,
    gap: f64,
    pad: f64,
    columns: usize,
    main: Align,
    cross: Align,
}

/// 解析 UI 节点引用：先按名字、再按句柄（e<i>v<g>）查活实体。和 runtime 的
/// resolve_entity 同口径——场景实例化把 entity 字段的名字换成句柄，运行时读到的是句柄。
fn resolve_ui_ref(world: &World, s: &str) -> Option<EntityId> {
    let name = s.strip_prefix('@').unwrap_or(s);
    if let Ok(id) = world.entity(name) {
        return Some(id);
    }
    if let Ok(h) = name.parse::<EntityId>() {
        if world.is_alive(h) {
            return Some(h);
        }
    }
    None
}

/// 读一个 number 字段（缺/非数字显式报错，带实体 + 路径）。
fn numf(world: &World, id: EntityId, path: &str) -> Result<f64, String> {
    world
        .get_field(id, path)
        .map_err(|e| e.to_string())?
        .as_f64()
        .ok_or_else(|| format!("实体 {id} 的 {path} 不是数字（UI 布局字段）"))
}

/// 读一个可选 text 字段（缺 = None，给了非文本报错）。
fn opt_text(world: &World, id: EntityId, path: &str) -> Result<Option<String>, String> {
    match world.get_field(id, path) {
        Err(_) => Ok(None),
        Ok(v) => {
            if v.is_null() {
                return Ok(None);
            }
            v.as_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| format!("实体 {id} 的 {path} 不是文本: {v}"))
        }
    }
}

/// 把 Ui 节点解析成 [`Node`]（含容器规格）。字段全在这里校验，解算热路径只剩算术。
fn read_node(world: &World, id: EntityId) -> Result<Node, String> {
    let anchor_name = opt_text(world, id, "Ui.anchor")?.unwrap_or_else(|| "manual".to_string());
    let anchor = Anchor::parse(&anchor_name).ok_or_else(|| {
        format!(
            "实体 {id} 的 Ui.anchor {anchor_name:?} 不是合法锚点预设。可选: [{}]",
            ANCHOR_NAMES.join(", ")
        )
    })?;
    let ax = numf(world, id, "Ui.ax").unwrap_or(0.0);
    let ay = numf(world, id, "Ui.ay").unwrap_or(0.0);
    let ox = numf(world, id, "Ui.ox").unwrap_or(0.0);
    let oy = numf(world, id, "Ui.oy").unwrap_or(0.0);
    let w = numf(world, id, "Ui.w").unwrap_or(0.0);
    let h = numf(world, id, "Ui.h").unwrap_or(0.0);
    let weight = numf(world, id, "Ui.weight").unwrap_or(0.0);
    // parent：实体引用。空串/null = 无父（锚到视口/根框）。引用可以是运行时句柄
    // （e<i>v<g>，场景实例化已把名字换成句柄）或直接的实体名——和 resolve_entity 同口径。
    let parent = match opt_text(world, id, "Ui.parent")? {
        None => None,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(resolve_ui_ref(world, &s).ok_or_else(|| {
            format!("实体 {id} 的 Ui.parent {s:?} 解析不到实体（父节点必须是场上存在的 UI 实体）")
        })?),
    };
    let container = if world.has_component(id, "Container") {
        let kind_name = opt_text(world, id, "Container.kind")?
            .ok_or_else(|| format!("实体 {id} 挂了 Container 但没有 kind 字段。可选: [{}]", CONTAINER_KINDS.join(", ")))?;
        let kind = ContainerKind::parse(&kind_name).ok_or_else(|| {
            format!(
                "实体 {id} 的 Container.kind {kind_name:?} 不认识。可选: [{}]",
                CONTAINER_KINDS.join(", ")
            )
        })?;
        let columns = numf(world, id, "Container.columns").unwrap_or(1.0);
        if matches!(kind, ContainerKind::Grid) && columns < 1.0 {
            return Err(format!(
                "实体 {id} 的 Container.columns 必须 ≥ 1（Grid 列数），拿到 {columns}"
            ));
        }
        let main = match opt_text(world, id, "Container.main")? {
            None => Align::Start,
            Some(s) => Align::parse(&s).ok_or_else(|| {
                format!("实体 {id} 的 Container.main {s:?} 不是对齐名。可选: [{}]", ALIGN_NAMES.join(", "))
            })?,
        };
        let cross = match opt_text(world, id, "Container.cross")? {
            None => Align::Start,
            Some(s) => Align::parse(&s).ok_or_else(|| {
                format!("实体 {id} 的 Container.cross {s:?} 不是对齐名。可选: [{}]", ALIGN_NAMES.join(", "))
            })?,
        };
        Some(ContainerSpec {
            kind,
            gap: numf(world, id, "Container.gap").unwrap_or(0.0),
            pad: numf(world, id, "Container.pad").unwrap_or(0.0),
            columns: columns.max(1.0) as usize,
            main,
            cross,
        })
    } else {
        None
    };
    Ok(Node {
        id,
        anchor,
        ax,
        ay,
        ox,
        oy,
        w,
        h,
        parent,
        weight,
        container,
    })
}

/// 把节点的锚点 + 偏移 + 尺寸解算成父框（px,py,pw,ph，屏幕像素）里的矩形。
/// 锚点几何：节点的 (ax,ay) 锚点对齐到父框的同名归一化点，再加像素偏移。
fn solve_anchor(node: &Node, parent: UiRect) -> UiRect {
    match node.anchor {
        Anchor::Stretch => {
            // 拉伸填满父框，ox/oy 当四边内缩（对称），忽略 w/h
            UiRect {
                x: parent.x + node.ox,
                y: parent.y + node.oy,
                w: (parent.w - 2.0 * node.ox).max(0.0),
                h: (parent.h - 2.0 * node.oy).max(0.0),
            }
        }
        other => {
            let (nx, ny) = if matches!(other, Anchor::Manual) {
                (node.ax, node.ay)
            } else {
                other.norm()
            };
            // 父框内锚点（像素）+ 偏移；节点自身锚点也按同一归一化点对齐
            // （贴右上角 = 节点右上角对齐父框右上角），自然覆盖四角/四边/居中
            let anchor_px = parent.x + nx * parent.w + node.ox;
            let anchor_py = parent.y + ny * parent.h + node.oy;
            UiRect {
                x: anchor_px - nx * node.w,
                y: anchor_py - ny * node.h,
                w: node.w,
                h: node.h,
            }
        }
    }
}

/// 容器排版：给定容器自己的框，算出每个子节点的矩形（覆盖 anchor 结果）。
/// 子节点顺序 = `children` 给定的顺序（世界槽位序，确定性）。
fn solve_container(spec: ContainerSpec, frame: UiRect, children: &[&Node]) -> Vec<(EntityId, UiRect)> {
    // 内容区 = 框去掉四边 padding
    let inner = UiRect {
        x: frame.x + spec.pad,
        y: frame.y + spec.pad,
        w: (frame.w - 2.0 * spec.pad).max(0.0),
        h: (frame.h - 2.0 * spec.pad).max(0.0),
    };
    match spec.kind {
        ContainerKind::VBox => solve_box(spec, inner, children, false),
        ContainerKind::HBox => solve_box(spec, inner, children, true),
        ContainerKind::Grid => solve_grid(spec, inner, children),
    }
}

/// VBox/HBox 共用：`horizontal=true` 主轴为 x（横排），否则 y（竖排）。
/// 主轴：按子节点声明尺寸顺序排，gap 间隔；有 weight 的瓜分剩余主轴空间。
/// 交叉轴：按 cross 对齐，weight>0 的子节点交叉轴方向拉满内容区。
fn solve_box(spec: ContainerSpec, inner: UiRect, children: &[&Node], horizontal: bool) -> Vec<(EntityId, UiRect)> {
    let n = children.len();
    if n == 0 {
        return Vec::new();
    }
    let total_gap = spec.gap * (n.saturating_sub(1)) as f64;
    let main_extent = if horizontal { inner.w } else { inner.h };
    // 固定子节点（weight=0）占的主轴尺寸 + 总权重
    let mut fixed_main = 0.0;
    let mut total_weight = 0.0;
    for c in children {
        if c.weight > 0.0 {
            total_weight += c.weight;
        } else {
            fixed_main += if horizontal { c.w } else { c.h };
        }
    }
    let free = (main_extent - total_gap - fixed_main).max(0.0);
    // 没有权重子节点：整组按 main 对齐放进剩余空间
    let group_offset = if total_weight == 0.0 {
        spec.main.offset(free)
    } else {
        0.0
    };
    let mut out = Vec::with_capacity(n);
    let mut cursor = if horizontal { inner.x } else { inner.y } + group_offset;
    for c in children {
        let main_size = if c.weight > 0.0 {
            free * c.weight / total_weight
        } else if horizontal {
            c.w
        } else {
            c.h
        };
        // 交叉轴尺寸：weight>0 拉满，否则用自身尺寸
        let cross_extent = if horizontal { inner.h } else { inner.w };
        let cross_size = if c.weight > 0.0 {
            cross_extent
        } else if horizontal {
            c.h
        } else {
            c.w
        };
        let cross_off = spec.cross.offset((cross_extent - cross_size).max(0.0));
        let rect = if horizontal {
            UiRect { x: cursor, y: inner.y + cross_off, w: main_size, h: cross_size }
        } else {
            UiRect { x: inner.x + cross_off, y: cursor, w: cross_size, h: main_size }
        };
        out.push((c.id, rect));
        cursor += main_size + spec.gap;
    }
    out
}

/// Grid：固定列数，行高列宽等分内容区。子节点按行优先填格，整格分配（忽略自身 w/h）。
fn solve_grid(spec: ContainerSpec, inner: UiRect, children: &[&Node]) -> Vec<(EntityId, UiRect)> {
    let n = children.len();
    if n == 0 {
        return Vec::new();
    }
    let cols = spec.columns.max(1);
    let rows = n.div_ceil(cols);
    let cell_w = (inner.w - spec.gap * (cols.saturating_sub(1)) as f64) / cols as f64;
    let cell_h = (inner.h - spec.gap * (rows.saturating_sub(1)) as f64) / rows as f64;
    let mut out = Vec::with_capacity(n);
    for (i, c) in children.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        out.push((
            c.id,
            UiRect {
                x: inner.x + col as f64 * (cell_w + spec.gap),
                y: inner.y + row as f64 * (cell_h + spec.gap),
                w: cell_w.max(0.0),
                h: cell_h.max(0.0),
            },
        ));
    }
    out
}

/// 解算结果（id → 屏幕像素矩形）。渲染按这个画，check/describe 也读它。
pub type Layout = std::collections::BTreeMap<EntityId, UiRect>;

/// 解算整棵（多棵）UI 树 → 每个 Ui 节点的屏幕像素矩形。**纯函数**：
/// 输入 = (世界里的 UI 组件, 视口 w/h)，无墙钟无随机。深度优先一趟树遍历，
/// O(UI 节点数)。每次真解算给全局 `LAYOUT_RUNS` +1（测试可观测）。
///
/// 父子关系由 `Ui.parent` 实体引用建立；无 parent 的节点锚到视口框 (0,0,w,h)。
/// 容器节点用自己解算出的框给子节点排版（覆盖子节点的 anchor 结果）。
/// 环引用（parent 指回祖先）按"已访问集合"截断——不会死循环，多余引用静默忽略
/// （静态 check 不强制无环，但解算永远终止）。
pub fn solve_layout(world: &World, width: u32, height: u32) -> Result<Layout, String> {
    let ids = world.query(&["Ui"]);
    if ids.is_empty() {
        return Ok(Layout::new());
    }
    LAYOUT_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // 读全部节点（字段校验一次做掉）
    let mut nodes: Vec<Node> = Vec::with_capacity(ids.len());
    for id in &ids {
        nodes.push(read_node(world, *id)?);
    }
    // id → 在 nodes 里的下标（快速取子节点）
    let index: std::collections::BTreeMap<EntityId, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    // 父 → 子下标列表（按 nodes 顺序 = 槽位序，确定性）
    let mut children_of: std::collections::BTreeMap<EntityId, Vec<usize>> = std::collections::BTreeMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        match n.parent.filter(|p| index.contains_key(p)) {
            Some(p) => children_of.entry(p).or_default().push(i),
            None => roots.push(i),
        }
    }

    let viewport = UiRect { x: 0.0, y: 0.0, w: width as f64, h: height as f64 };
    let mut layout = Layout::new();
    let mut visited = std::collections::BTreeSet::new();

    // 显式栈深度优先（避免递归爆栈；顺序：根按槽位序，子按 children_of 顺序）
    // 栈元素 = (节点下标, 父框)。
    let mut stack: Vec<(usize, UiRect)> = roots.iter().rev().map(|&i| (i, viewport)).collect();
    while let Some((i, parent_frame)) = stack.pop() {
        if !visited.insert(nodes[i].id) {
            continue; // 环/重复引用：截断（解算永远终止）
        }
        let node = &nodes[i];
        let rect = solve_anchor(node, parent_frame);
        layout.insert(node.id, rect);

        let kids = children_of.get(&node.id).cloned().unwrap_or_default();
        if kids.is_empty() {
            continue;
        }
        if let Some(spec) = node.container {
            // 容器：子节点位置由容器算出（覆盖各自 anchor），交给渲染前先落进 layout
            let kid_nodes: Vec<&Node> = kids.iter().map(|&k| &nodes[k]).collect();
            let placed = solve_container(spec, rect, &kid_nodes);
            // 容器排版只定位**直接子节点**；子节点若还是容器/有自己的子，继续入栈
            // （它们的子用各自被排好的框当父框）
            let placed_map: std::collections::BTreeMap<EntityId, UiRect> = placed.into_iter().collect();
            for &k in kids.iter().rev() {
                let kid_id = nodes[k].id;
                let kframe = placed_map.get(&kid_id).copied().unwrap_or(rect);
                // 直接落定子节点矩形（容器算的就是最终矩形，不再走 anchor）
                layout.insert(kid_id, kframe);
                visited.insert(kid_id);
                // 子节点的孙子用子节点的框当父框
                let grandkids = children_of.get(&kid_id).cloned().unwrap_or_default();
                for &gk in grandkids.iter().rev() {
                    stack.push((gk, kframe));
                }
            }
        } else {
            // 非容器：子节点各自按 anchor 对本节点框解算
            for &k in kids.iter().rev() {
                stack.push((k, rect));
            }
        }
    }
    Ok(layout)
}

/// 布局输入的结构哈希：所有影响布局的字段（每个 Ui 节点的 anchor/ax/ay/ox/oy/w/h/
/// parent/weight + Container 全字段）+ 视口尺寸的确定性散列。**不含** rx/ry/rw/rh
/// 输出本身——否则写回输出会改哈希、永远"脏"。用于脏标记：和 UiRoot.layout_hash
/// 比对，相等就跳过重算（静止 UI 零重算）。
///
/// 顺序确定（query 槽位序），数值走 f64 位串——同输入同哈希，跨 tick 稳定。
pub fn layout_input_hash(world: &World, width: u32, height: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    width.hash(&mut h);
    height.hash(&mut h);
    for id in world.query(&["Ui"]) {
        id.to_string().hash(&mut h);
        // 影响布局的 Ui 字段（缺省按 read_node 的缺省值散列，保持一致）
        for path in ["Ui.anchor", "Ui.parent"] {
            if let Ok(v) = world.get_field(id, path) {
                if let Some(s) = v.as_str() {
                    s.hash(&mut h);
                }
            }
        }
        for path in ["Ui.ax", "Ui.ay", "Ui.ox", "Ui.oy", "Ui.w", "Ui.h", "Ui.weight"] {
            let n = world.get_field(id, path).ok().and_then(Value::as_f64).unwrap_or(0.0);
            n.to_bits().hash(&mut h);
        }
        if world.has_component(id, "Container") {
            for path in ["Container.kind", "Container.main", "Container.cross"] {
                if let Ok(v) = world.get_field(id, path) {
                    if let Some(s) = v.as_str() {
                        s.hash(&mut h);
                    }
                }
            }
            for path in ["Container.gap", "Container.pad", "Container.columns"] {
                let n = world.get_field(id, path).ok().and_then(Value::as_f64).unwrap_or(0.0);
                n.to_bits().hash(&mut h);
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 造一个挂 UiRoot 的根 + 若干 Ui 节点的世界。返回 World，节点用 spawn_named 命名
    /// 方便 parent 引用。
    fn ui_world() -> World {
        let mut w = World::new();
        let root = w.spawn_named("ui-root").unwrap();
        w.set_component(root, "UiRoot", json!({})).unwrap();
        w
    }

    /// 给实体挂一个 Ui 组件（全字段，缺省给 0 / manual / 空 parent）。
    #[allow(clippy::too_many_arguments)]
    fn set_ui(
        w: &mut World,
        id: EntityId,
        anchor: &str,
        ax: f64,
        ay: f64,
        ox: f64,
        oy: f64,
        cw: f64,
        ch: f64,
        parent: &str,
        weight: f64,
    ) {
        w.set_component(
            id,
            "Ui",
            json!({
                "anchor": anchor, "ax": ax, "ay": ay, "ox": ox, "oy": oy,
                "w": cw, "h": ch, "parent": parent, "weight": weight,
                "rx": 0.0, "ry": 0.0, "rw": 0.0, "rh": 0.0
            }),
        )
        .unwrap();
    }

    fn rect(layout: &Layout, w: &World, name: &str) -> UiRect {
        let id = w.entity(name).unwrap();
        *layout.get(&id).unwrap_or_else(|| panic!("布局里没有 {name}"))
    }

    #[test]
    fn anchor_corners_and_center_resolve_to_exact_pixels() {
        // 视口 200x100。各预设贴角/居中，逐值断言位置尺寸。
        let mut w = ui_world();
        let tl = w.spawn_named("tl").unwrap();
        set_ui(&mut w, tl, "top-left", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        let br = w.spawn_named("br").unwrap();
        set_ui(&mut w, br, "bottom-right", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        let c = w.spawn_named("c").unwrap();
        set_ui(&mut w, c, "center", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        let tc = w.spawn_named("tc").unwrap();
        set_ui(&mut w, tc, "top-center", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);

        let l = solve_layout(&w, 200, 100).unwrap();
        // 左上角：节点左上贴 (0,0)
        assert_eq!(rect(&l, &w, "tl"), UiRect { x: 0.0, y: 0.0, w: 40.0, h: 20.0 });
        // 右下角：节点右下贴 (200,100) → x=160, y=80
        assert_eq!(rect(&l, &w, "br"), UiRect { x: 160.0, y: 80.0, w: 40.0, h: 20.0 });
        // 居中：节点中心贴 (100,50) → x=80, y=40
        assert_eq!(rect(&l, &w, "c"), UiRect { x: 80.0, y: 40.0, w: 40.0, h: 20.0 });
        // 上中：节点中上贴 (100,0) → x=80, y=0
        assert_eq!(rect(&l, &w, "tc"), UiRect { x: 80.0, y: 0.0, w: 40.0, h: 20.0 });
    }

    #[test]
    fn anchor_offset_and_manual_and_stretch() {
        let mut w = ui_world();
        // 贴右上角 + 像素偏移：往左下推 (ox=-10, oy=5)
        let e = w.spawn_named("corner").unwrap();
        set_ui(&mut w, e, "top-right", 0.0, 0.0, -10.0, 5.0, 30.0, 10.0, "", 0.0);
        // manual：ax/ay = 0.25/0.75 父框内比例
        let m = w.spawn_named("man").unwrap();
        set_ui(&mut w, m, "manual", 0.25, 0.75, 0.0, 0.0, 20.0, 20.0, "", 0.0);
        // stretch：填满父框，ox/oy=8 当四边内缩
        let s = w.spawn_named("str").unwrap();
        set_ui(&mut w, s, "stretch", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0, "", 0.0);

        let l = solve_layout(&w, 200, 100).unwrap();
        // 右上角锚点 (200,0) + 偏移 (-10,5) = (190,5)，节点右上对齐 → x=160, y=5
        assert_eq!(rect(&l, &w, "corner"), UiRect { x: 160.0, y: 5.0, w: 30.0, h: 10.0 });
        // manual 锚点 (0.25*200, 0.75*100)=(50,75)，节点同名锚点对齐 → x=50-0.25*20=45, y=75-0.75*20=60
        assert_eq!(rect(&l, &w, "man"), UiRect { x: 45.0, y: 60.0, w: 20.0, h: 20.0 });
        // stretch：(8,8) 起，宽高各减 16 → 184x84
        assert_eq!(rect(&l, &w, "str"), UiRect { x: 8.0, y: 8.0, w: 184.0, h: 84.0 });
    }

    #[test]
    fn vbox_stacks_children_with_gap_and_padding() {
        // 容器框 100 宽 × 200 高，pad=10，gap=5，三个 30 高的子节点竖排，main=start
        let mut w = ui_world();
        let box_id = w.spawn_named("vbox").unwrap();
        set_ui(&mut w, box_id, "top-left", 0.0, 0.0, 0.0, 0.0, 100.0, 200.0, "", 0.0);
        w.set_component(box_id, "Container", json!({
            "kind": "VBox", "gap": 5.0, "pad": 10.0, "columns": 1, "main": "start", "cross": "start"
        })).unwrap();
        let parent_handle = box_id.to_string();
        for i in 0..3 {
            let c = w.spawn_named(&format!("v{i}")).unwrap();
            set_ui(&mut w, c, "top-left", 0.0, 0.0, 0.0, 0.0, 40.0, 30.0, &parent_handle, 0.0);
        }
        let l = solve_layout(&w, 300, 300).unwrap();
        // 内容区从 (10,10) 起。第 0 个 y=10，第 1 个 y=10+30+5=45，第 2 个 y=80。
        // cross=start：x 贴内容区左 = 10。子节点保留自身宽 40。
        assert_eq!(rect(&l, &w, "v0"), UiRect { x: 10.0, y: 10.0, w: 40.0, h: 30.0 });
        assert_eq!(rect(&l, &w, "v1"), UiRect { x: 10.0, y: 45.0, w: 40.0, h: 30.0 });
        assert_eq!(rect(&l, &w, "v2"), UiRect { x: 10.0, y: 80.0, w: 40.0, h: 30.0 });
    }

    #[test]
    fn hbox_main_center_and_cross_center() {
        // 横排，框 200 宽 × 60 高，无 pad，gap=10，两个 40 宽×20 高，main=center,cross=center
        let mut w = ui_world();
        let box_id = w.spawn_named("hbox").unwrap();
        set_ui(&mut w, box_id, "top-left", 0.0, 0.0, 0.0, 0.0, 200.0, 60.0, "", 0.0);
        w.set_component(box_id, "Container", json!({
            "kind": "HBox", "gap": 10.0, "pad": 0.0, "columns": 1, "main": "center", "cross": "center"
        })).unwrap();
        let ph = box_id.to_string();
        for i in 0..2 {
            let c = w.spawn_named(&format!("h{i}")).unwrap();
            set_ui(&mut w, c, "top-left", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, &ph, 0.0);
        }
        let l = solve_layout(&w, 300, 300).unwrap();
        // 主轴占用 = 40+10+40 = 90，剩余 200-90=110，main=center → 起点偏移 55
        // 第 0 个 x=55，第 1 个 x=55+40+10=105。cross=center：y=(60-20)/2=20
        assert_eq!(rect(&l, &w, "h0"), UiRect { x: 55.0, y: 20.0, w: 40.0, h: 20.0 });
        assert_eq!(rect(&l, &w, "h1"), UiRect { x: 105.0, y: 20.0, w: 40.0, h: 20.0 });
    }

    #[test]
    fn vbox_weight_splits_remaining_space() {
        // 框高 100，无 pad/gap。三个子节点：固定 20 高、weight=1、weight=3。
        // 剩余 = 100-20 = 80，按 1:3 分 → 20 和 60。
        let mut w = ui_world();
        let box_id = w.spawn_named("wbox").unwrap();
        set_ui(&mut w, box_id, "top-left", 0.0, 0.0, 0.0, 0.0, 50.0, 100.0, "", 0.0);
        w.set_component(box_id, "Container", json!({
            "kind": "VBox", "gap": 0.0, "pad": 0.0, "columns": 1, "main": "start", "cross": "start"
        })).unwrap();
        let ph = box_id.to_string();
        let fixed = w.spawn_named("fixed").unwrap();
        set_ui(&mut w, fixed, "top-left", 0.0, 0.0, 0.0, 0.0, 50.0, 20.0, &ph, 0.0);
        let w1 = w.spawn_named("w1").unwrap();
        set_ui(&mut w, w1, "top-left", 0.0, 0.0, 0.0, 0.0, 50.0, 0.0, &ph, 1.0);
        let w3 = w.spawn_named("w3").unwrap();
        set_ui(&mut w, w3, "top-left", 0.0, 0.0, 0.0, 0.0, 50.0, 0.0, &ph, 3.0);
        let l = solve_layout(&w, 300, 300).unwrap();
        assert_eq!(rect(&l, &w, "fixed").h, 20.0);
        assert_eq!(rect(&l, &w, "fixed").y, 0.0);
        assert_eq!(rect(&l, &w, "w1").y, 20.0);
        assert_eq!(rect(&l, &w, "w1").h, 20.0); // 80 * 1/4
        assert_eq!(rect(&l, &w, "w3").y, 40.0);
        assert_eq!(rect(&l, &w, "w3").h, 60.0); // 80 * 3/4
    }

    #[test]
    fn grid_fixed_columns_equal_cells() {
        // 5 个格子，3 列 → 2 行（最后一行 2 格）。框 100x100，gap=10。
        // cell_w = (100 - 10*2)/3 = 80/3；cell_h = (100 - 10*1)/2 = 45。
        let mut w = ui_world();
        let g = w.spawn_named("grid").unwrap();
        set_ui(&mut w, g, "top-left", 0.0, 0.0, 0.0, 0.0, 100.0, 100.0, "", 0.0);
        w.set_component(g, "Container", json!({
            "kind": "Grid", "gap": 10.0, "pad": 0.0, "columns": 3, "main": "start", "cross": "start"
        })).unwrap();
        let ph = g.to_string();
        for i in 0..5 {
            let c = w.spawn_named(&format!("g{i}")).unwrap();
            set_ui(&mut w, c, "top-left", 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, &ph, 0.0);
        }
        let l = solve_layout(&w, 300, 300).unwrap();
        let cell_w = 80.0 / 3.0;
        // g0 = (0,0)，g1 = (cell_w+10, 0)，g3 = (0, 55)（第二行第一个：cell_h 45 + gap 10）
        assert_eq!(rect(&l, &w, "g0"), UiRect { x: 0.0, y: 0.0, w: cell_w, h: 45.0 });
        assert!((rect(&l, &w, "g1").x - (cell_w + 10.0)).abs() < 1e-9);
        assert_eq!(rect(&l, &w, "g3").y, 55.0);
        assert_eq!(rect(&l, &w, "g3").x, 0.0);
    }

    #[test]
    fn empty_ui_is_zero_cost() {
        // 没有任何 Ui 节点：has_ui=false、solve 返回空 layout、early-return 不解算。
        // （layout_runs 是进程级全局计数，并行测试会互相增，零成本的"不重算"断言放在
        //  runtime 的串行系统测试里——见 vitric-cli/tests/ui.rs。这里只锁"空场返回空"。）
        let w = World::new();
        assert!(!has_ui(&w), "无 UiRoot = 无 UI");
        assert!(solve_layout(&w, 100, 100).unwrap().is_empty(), "空场布局为空");
    }

    #[test]
    fn solve_increments_run_counter_when_ui_present() {
        // 有 Ui 节点时 solve 真跑一次算法（counter +1）；这条单独验"非空才计数"。
        let mut w = ui_world();
        let e = w.spawn_named("a").unwrap();
        set_ui(&mut w, e, "center", 0.0, 0.0, 0.0, 0.0, 10.0, 10.0, "", 0.0);
        let before = layout_runs();
        solve_layout(&w, 100, 100).unwrap();
        assert!(layout_runs() > before, "有 UI 时 solve 应让 layout_runs 增长");
    }

    #[test]
    fn anchor_unknown_name_errors_with_valid_list() {
        let mut w = ui_world();
        let e = w.spawn_named("bad").unwrap();
        set_ui(&mut w, e, "top-middle", 0.0, 0.0, 0.0, 0.0, 10.0, 10.0, "", 0.0);
        let err = solve_layout(&w, 100, 100).unwrap_err();
        assert!(err.contains("top-middle") && err.contains("center"), "{err}");
    }

    #[test]
    fn container_unknown_kind_and_grid_zero_columns_error() {
        let mut w = ui_world();
        let g = w.spawn_named("g").unwrap();
        set_ui(&mut w, g, "top-left", 0.0, 0.0, 0.0, 0.0, 10.0, 10.0, "", 0.0);
        w.set_component(g, "Container", json!({
            "kind": "Flex", "gap": 0.0, "pad": 0.0, "columns": 1, "main": "start", "cross": "start"
        })).unwrap();
        let err = solve_layout(&w, 100, 100).unwrap_err();
        assert!(err.contains("Flex") && err.contains("VBox"), "{err}");

        // Grid 列数 0
        let mut w2 = ui_world();
        let g2 = w2.spawn_named("g2").unwrap();
        set_ui(&mut w2, g2, "top-left", 0.0, 0.0, 0.0, 0.0, 10.0, 10.0, "", 0.0);
        w2.set_component(g2, "Container", json!({
            "kind": "Grid", "gap": 0.0, "pad": 0.0, "columns": 0, "main": "start", "cross": "start"
        })).unwrap();
        let err2 = solve_layout(&w2, 100, 100).unwrap_err();
        assert!(err2.contains("columns") && err2.contains("≥ 1"), "{err2}");
    }

    #[test]
    fn layout_hash_stable_for_unchanged_tree_changes_on_mutation() {
        let mut w = ui_world();
        let e = w.spawn_named("a").unwrap();
        set_ui(&mut w, e, "center", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        let h1 = layout_input_hash(&w, 200, 100);
        let h2 = layout_input_hash(&w, 200, 100);
        assert_eq!(h1, h2, "同一棵树同视口哈希必须稳定");
        // 改 w 字段 → 哈希变（脏）
        w.set_field(e, "Ui.w", json!(60.0)).unwrap();
        assert_ne!(h1, layout_input_hash(&w, 200, 100), "改尺寸要让哈希变（标脏）");
        // 改视口 → 哈希变
        assert_ne!(h1, layout_input_hash(&w, 300, 100), "改视口要让哈希变");
    }

    #[test]
    fn writing_rect_outputs_does_not_change_input_hash() {
        // rx/ry/rw/rh 是布局输出——写回它们绝不能改输入哈希（否则永远脏）。
        let mut w = ui_world();
        let e = w.spawn_named("a").unwrap();
        set_ui(&mut w, e, "center", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        let h1 = layout_input_hash(&w, 200, 100);
        w.set_field(e, "Ui.rx", json!(80.0)).unwrap();
        w.set_field(e, "Ui.ry", json!(40.0)).unwrap();
        w.set_field(e, "Ui.rw", json!(40.0)).unwrap();
        w.set_field(e, "Ui.rh", json!(20.0)).unwrap();
        assert_eq!(h1, layout_input_hash(&w, 200, 100), "写布局输出不该改输入哈希");
    }

    #[test]
    fn nested_container_inside_anchored_panel() {
        // 根 Panel 居中 100x100，里面一个 VBox 拉伸填满，VBox 里两个子节点。
        // 验证多层：anchor → 容器框 → 子节点排版。
        let mut w = ui_world();
        let panel = w.spawn_named("panel").unwrap();
        set_ui(&mut w, panel, "center", 0.0, 0.0, 0.0, 0.0, 100.0, 100.0, "", 0.0);
        let ph = panel.to_string();
        let vbox = w.spawn_named("vbox").unwrap();
        set_ui(&mut w, vbox, "stretch", 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, &ph, 0.0);
        w.set_component(vbox, "Container", json!({
            "kind": "VBox", "gap": 0.0, "pad": 0.0, "columns": 1, "main": "start", "cross": "start"
        })).unwrap();
        let vh = vbox.to_string();
        for i in 0..2 {
            let c = w.spawn_named(&format!("n{i}")).unwrap();
            set_ui(&mut w, c, "top-left", 0.0, 0.0, 0.0, 0.0, 10.0, 30.0, &vh, 0.0);
        }
        let l = solve_layout(&w, 200, 200).unwrap();
        // panel 居中 100x100 → x=50,y=50。vbox stretch 填满 → 也是 (50,50,100,100)。
        assert_eq!(rect(&l, &w, "panel"), UiRect { x: 50.0, y: 50.0, w: 100.0, h: 100.0 });
        assert_eq!(rect(&l, &w, "vbox"), UiRect { x: 50.0, y: 50.0, w: 100.0, h: 100.0 });
        // 两个子节点从 vbox 内容区 (50,50) 起竖排
        assert_eq!(rect(&l, &w, "n0").y, 50.0);
        assert_eq!(rect(&l, &w, "n1").y, 80.0);
    }
}
