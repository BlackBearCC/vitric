//! UI control layout — the **deterministic pure-computation** foundation of the screen-space
//! overlay (the CPU and GPU paths read the same one).
//!
//! Positioning (why it looks like this): UI is also entities + components, entering world state /
//! hash / saves / recordings. Unlike sprites/particles, UI is anchored to the **viewport (screen)**
//! and bypasses the camera transform — when the camera moves/scales/shakes, the UI does not drift
//! (like a HUD). Layout is a pure function of `(UI tree, viewport size)`: no wall clock, no RNG,
//! no cross-platform divergence; snapshot/restore round-trips are consistent.
//!
//! Component conventions (the engine recognizes names; fields are user-schema-defined; missing
//! fields are explicit errors — same as Sprite/Light):
//! - `UiRoot {}` — marks the root of a UI tree. Layout starts at the entity with UiRoot and solves
//!   against the viewport. **No** UiRoot entity in the scene = no UI at all = layout/render early-
//!   returns at zero cost every tick (byte-identical to the old behavior; zero allocation, zero
//!   traversal). The `layout_hash` field (optional) caches the structural hash of the previous
//!   layout input — skip recomputation when unchanged (the landing point of "static UI played for
//!   N ticks recomputes layout 0 times").
//! - `Ui {anchor, ax, ay, ox, oy, w, h, parent, weight, rx, ry, rw, rh}` — each UI node.
//!   * `anchor`: anchor-preset name (see [`Anchor`]), decides which preset (ax,ay) uses — a preset
//!     is used as the normalized anchor, while `"manual"` uses explicit ax/ay (0..1, ratio within
//!     the parent frame).
//!   * `ox/oy`: pixel offset; `w/h`: size (pixels; overridden by `stretch` anchor / container stretching).
//!   * `parent`: entity reference to the parent UI node (empty = anchor directly to viewport/root frame).
//!   * `weight`: container main-axis stretch weight (0 = use own w/h; >0 = split remaining space by weight).
//!   * `rx/ry/rw/rh`: **layout output** — the solved screen-pixel rect (top-left origin). The solver
//!     writes it, rendering reads it; going into the component = into the hash and saves (snapshot /
//!     recording safe).
//! - `Container {kind, gap, pad, columns, main, cross}` — when attached, child nodes (Ui nodes whose
//!   parent points to this entity) are auto-laid-out by the container; children do not place their own
//!   coordinates (rx/ry/rw/rh are overwritten by the container's computation). `kind` ∈ {VBox, HBox,
//!   Grid}; Grid requires `columns ≥ 1`.
//! - `Panel {color | image}` — a background frame (solid color or sprite). Rendering reads it; layout
//!   does not care.
//! - `UiLabel {content, size, color, reveal, align}` — a text control, reusing the font.rs layout
//!   cache + Text.reveal (per-character reveal is already implemented). Rendering reads it; layout
//!   only uses its Ui frame for positioning.
//!
//! Scope (1.1): the layout foundation + static controls + screen-space rendering; a gray box that
//! is not interactive. Button state machine / focus navigation / click activation / theming belong
//! to 1.2.

use serde_json::Value;

use vitric_ecs::{EntityId, World};

/// Anchor preset: which point of the parent frame (or viewport) the UI node attaches to. Normalized
/// coordinates (0,0)=top-left, (1,1)=bottom-right. `Stretch` is a special case — it stretches to
/// fill the parent frame (ignoring its own w/h, inset by padding/offset). `Manual` uses the node's
/// own ax/ay (0..1 ratio) as the anchor, covering the most flexible HUD placement.
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
    /// Stretch to fill the parent frame (ox/oy are inset on all four sides; w/h ignored).
    Stretch,
    /// Use the node's own ax/ay (0..1 ratio within the parent frame).
    Manual,
}

/// All legal anchor-preset names (for check validation + error messages; one-to-one with [`Anchor::parse`]).
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
    /// Preset name → anchor. Unknown names return `None` (the caller errors with the legal-name list).
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

    /// Normalized anchor (0..1, 0..1) within the parent frame for this preset. Stretch/Manual do
    /// not go through this (special-cased).
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
            // The next two are special-cased and should not be reached here (kept for full-enum consistency).
            Anchor::Stretch => (0.0, 0.0),
            Anchor::Manual => (0.0, 0.0),
        }
    }
}

/// Container kind (auto-lays-out child nodes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    /// Vertical: children are arranged along y in order.
    VBox,
    /// Horizontal: children are arranged along x in order.
    HBox,
    /// Grid: fixed column count, with row height and column width evenly split.
    Grid,
}

/// All legal container-kind names (for check validation + error messages).
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

/// Main-axis / cross-axis alignment. The container arranges children along the main axis; `cross`
/// decides how they attach on the axis perpendicular to the main one.
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
    /// Returns the start offset within `free` pixels of remaining space, according to the alignment.
    fn offset(self, free: f64) -> f64 {
        match self {
            Align::Start => 0.0,
            Align::Center => free / 2.0,
            Align::End => free,
        }
    }
}

/// All legal alignment names (for check validation).
pub const ALIGN_NAMES: &[&str] = &["start", "center", "end"];

/// A solved screen-pixel rect (top-left origin, same coordinate system as the render buf).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Number of times the layout algorithm actually ran (a global atomic counter; test-only). Modelled
/// on font.rs's `layout_runs`: asserts "static UI played for N ticks lays out exactly once". Does
/// not enter any render output; pure observability.
static LAYOUT_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total number of layout-algorithm executions (process-wide cumulative). In tests, read a baseline
/// first, run some ticks, then read the delta.
pub fn layout_runs() -> u64 {
    LAYOUT_RUNS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the scene has any UI at all (an entity with UiRoot). If not, the entire UI path is skipped
/// at zero cost.
pub fn has_ui(world: &World) -> bool {
    !world.query(&["UiRoot"]).is_empty()
}

/// Whether the screen point (px,py) (same coordinate system as render -- reference
/// resolution pixels) is over any "blocking" UI element (a visible node with a Panel or
/// Button). The window uses this to block **world clicks** and **build placement preview**
/// when the cursor is over a menu/panel, to avoid clicking through to the map below
/// (previously clicks on a menu would pass through and show green tiles under the menu).
/// Elements hidden by being moved off-screen (ox=-6000) have an off-screen rect and
/// naturally don't hit.
pub fn point_over_ui(world: &World, width: u32, height: u32, px: f64, py: f64) -> bool {
    if !has_ui(world) {
        return false;
    }
    let layout = match solve_layout(world, width, height) {
        Ok(l) => l,
        Err(_) => return false,
    };
    for id in world.query(&["Ui"]) {
        if !world.has_component(id, "Panel") && !world.has_component(id, "Button") {
            continue;
        }
        if let Some(r) = layout.get(&id) {
            if px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h {
                return true;
            }
        }
    }
    false
}

/// Static input for one UI node (read from components before solving; rx/ry/rw/rh are outputs and
/// not read).
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

/// Parse a UI node reference: first by name, then by handle (e<i>v<g>) for a live entity. Same
/// approach as the runtime's resolve_entity — scene instantiation swaps the name in an entity field
/// for a handle, so the runtime reads a handle.
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

/// Read one number field (missing / non-numeric explicitly errors, with the entity + path).
fn numf(world: &World, id: EntityId, path: &str) -> Result<f64, String> {
    world
        .get_field(id, path)
        .map_err(|e| e.to_string())?
        .as_f64()
        .ok_or_else(|| format!("实体 {id} 的 {path} 不是数字（UI 布局字段）"))
}

/// Read one optional text field (missing = None; non-text when present errors).
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

/// Parse a Ui node into a [`Node`] (including its container spec). All field validation happens
/// here; the solving hot path is then pure arithmetic.
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
    // parent: an entity reference. Empty string / null = no parent (anchor to viewport/root frame).
    // The reference may be a runtime handle (e<i>v<g>; scene instantiation has already swapped the
    // name for a handle) or a direct entity name — same approach as resolve_entity.
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

/// Solve the node's anchor + offset + size into a rect within the parent frame (px,py,pw,ph in
/// screen pixels). Anchor geometry: the node's (ax,ay) anchor aligns to the same-named normalized
/// point of the parent frame, then the pixel offset is added.
fn solve_anchor(node: &Node, parent: UiRect) -> UiRect {
    match node.anchor {
        Anchor::Stretch => {
            // Stretch to fill the parent frame; ox/oy are inset on all four sides (symmetric); w/h ignored.
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
            // Anchor point inside the parent frame (pixels) + offset; the node's own anchor aligns to
            // the same normalized point (top-right = node's top-right aligns to parent frame's top-right),
            // naturally covering all four corners / four edges / center.
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

/// Container layout: given the container's own frame, compute each child's rect (overriding the
/// anchor result). Child order = the order given by `children` (the world slot order; deterministic).
fn solve_container(spec: ContainerSpec, frame: UiRect, children: &[&Node]) -> Vec<(EntityId, UiRect)> {
    // Content area = the frame minus four-sided padding.
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

/// Shared VBox/HBox: `horizontal=true` makes the main axis x (horizontal); otherwise y (vertical).
/// Main axis: arrange children in order by their declared sizes, with `gap` between; children with
/// weight split the remaining main-axis space.
/// Cross axis: align by `cross`; children with weight>0 fill the content area on the cross axis.
fn solve_box(spec: ContainerSpec, inner: UiRect, children: &[&Node], horizontal: bool) -> Vec<(EntityId, UiRect)> {
    let n = children.len();
    if n == 0 {
        return Vec::new();
    }
    let total_gap = spec.gap * (n.saturating_sub(1)) as f64;
    let main_extent = if horizontal { inner.w } else { inner.h };
    // Main-axis size taken by fixed children (weight=0) + total weight.
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
    // No weighted children: align the whole group into the remaining space by `main`.
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
        // Cross-axis size: weight>0 fills; otherwise the node's own size.
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

/// Grid: fixed column count, with row height and column width evenly split across the content area.
/// Children fill cells in row-major order; each cell is allocated as a whole (own w/h ignored).
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

/// Solve result (id → screen-pixel rect). Rendering draws by this; check/describe also read it.
pub type Layout = std::collections::BTreeMap<EntityId, UiRect>;

/// Solve the whole (one or more) UI tree(s) → the screen-pixel rect of each Ui node. **Pure
/// function**: input = (UI components in the world, viewport w/h), no wall clock, no RNG. A single
/// depth-first tree traversal, O(number of UI nodes). Each real solve increments the global
/// `LAYOUT_RUNS` by 1 (test-observable).
///
/// Parent-child relationships are established by the `Ui.parent` entity reference; nodes without a
/// parent anchor to the viewport frame (0,0,w,h). Container nodes use their own solved frame to
/// lay out their children (overriding the children's anchor results). Cyclic references (parent
/// pointing back to an ancestor) are cut off by a "visited set" — no infinite loop, and extra
/// references are silently ignored (static check does not enforce acyclicity, but solving always
/// terminates).
pub fn solve_layout(world: &World, width: u32, height: u32) -> Result<Layout, String> {
    let ids = world.query(&["Ui"]);
    if ids.is_empty() {
        return Ok(Layout::new());
    }
    LAYOUT_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Read all nodes (field validation is done once here).
    let mut nodes: Vec<Node> = Vec::with_capacity(ids.len());
    for id in &ids {
        nodes.push(read_node(world, *id)?);
    }
    // id → index in nodes (for quick child lookup).
    let index: std::collections::BTreeMap<EntityId, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    // parent → list of child indices (in nodes order = slot order; deterministic).
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

    // Explicit-stack depth-first (avoids recursion blowups; order: roots in slot order, children in
    // children_of order). Stack element = (node index, parent frame).
    let mut stack: Vec<(usize, UiRect)> = roots.iter().rev().map(|&i| (i, viewport)).collect();
    while let Some((i, parent_frame)) = stack.pop() {
        if !visited.insert(nodes[i].id) {
            continue; // Cycle / duplicate reference: cut off (solving always terminates).
        }
        let node = &nodes[i];
        let rect = solve_anchor(node, parent_frame);
        layout.insert(node.id, rect);

        let kids = children_of.get(&node.id).cloned().unwrap_or_default();
        if kids.is_empty() {
            continue;
        }
        if let Some(spec) = node.container {
            // Container: child positions are computed by the container (overriding each anchor);
            // commit them to layout before handing off to rendering.
            let kid_nodes: Vec<&Node> = kids.iter().map(|&k| &nodes[k]).collect();
            let placed = solve_container(spec, rect, &kid_nodes);
            // Container layout only positions **direct children**; if a child is itself a container /
            // has its own children, keep pushing onto the stack (their children use the child's laid-
            // out frame as the parent frame).
            let placed_map: std::collections::BTreeMap<EntityId, UiRect> = placed.into_iter().collect();
            for &k in kids.iter().rev() {
                let kid_id = nodes[k].id;
                let kframe = placed_map.get(&kid_id).copied().unwrap_or(rect);
                // Commit the child rect directly (what the container computed is the final rect; anchor is not re-run).
                layout.insert(kid_id, kframe);
                visited.insert(kid_id);
                // Grandchildren use the child's frame as their parent frame.
                let grandkids = children_of.get(&kid_id).cloned().unwrap_or_default();
                for &gk in grandkids.iter().rev() {
                    stack.push((gk, kframe));
                }
            }
        } else {
            // Non-container: each child is solved by its own anchor against this node's frame.
            for &k in kids.iter().rev() {
                stack.push((k, rect));
            }
        }
    }
    Ok(layout)
}

/// Structural hash of the layout input: a deterministic hash of all layout-affecting fields (each
/// Ui node's anchor/ax/ay/ox/oy/w/h/parent/weight + all Container fields) + the viewport size. Does
/// **not** include the rx/ry/rw/rh output itself — otherwise writing the output back would change
/// the hash and stay "dirty" forever. Used as a dirty flag: compared against UiRoot.layout_hash; if
/// equal, recomputation is skipped (static UI = zero recomputation).
///
/// Deterministic order (query slot order); numbers go through f64 bit patterns — same input → same
/// hash, stable across ticks.
pub fn layout_input_hash(world: &World, width: u32, height: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    width.hash(&mut h);
    height.hash(&mut h);
    for id in world.query(&["Ui"]) {
        id.to_string().hash(&mut h);
        // Ui fields that affect layout (missing values hash as the read_node defaults, for consistency).
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

    /// Build a world with a UiRoot root + several Ui nodes. Returns World, nodes named via spawn_named
    /// for easy parent references.
    fn ui_world() -> World {
        let mut w = World::new();
        let root = w.spawn_named("ui-root").unwrap();
        w.set_component(root, "UiRoot", json!({})).unwrap();
        w
    }

    /// Attach a Ui component to an entity (all fields, defaults to 0 / manual / empty parent).
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
        // Viewport 200x100. Each preset hugs corner/center, assert position and size per case.
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
        // top-left: node's top-left hugs (0,0)
        assert_eq!(rect(&l, &w, "tl"), UiRect { x: 0.0, y: 0.0, w: 40.0, h: 20.0 });
        // bottom-right: node's bottom-right hugs (200,100) → x=160, y=80
        assert_eq!(rect(&l, &w, "br"), UiRect { x: 160.0, y: 80.0, w: 40.0, h: 20.0 });
        // center: node's center hugs (100,50) → x=80, y=40
        assert_eq!(rect(&l, &w, "c"), UiRect { x: 80.0, y: 40.0, w: 40.0, h: 20.0 });
        // top-center: node's top-middle hugs (100,0) → x=80, y=0
        assert_eq!(rect(&l, &w, "tc"), UiRect { x: 80.0, y: 0.0, w: 40.0, h: 20.0 });
    }

    #[test]
    fn anchor_offset_and_manual_and_stretch() {
        let mut w = ui_world();
        // Hug top-right corner + pixel offset: push toward bottom-left (ox=-10, oy=5)
        let e = w.spawn_named("corner").unwrap();
        set_ui(&mut w, e, "top-right", 0.0, 0.0, -10.0, 5.0, 30.0, 10.0, "", 0.0);
        // manual: ax/ay = 0.25/0.75 proportion within parent frame
        let m = w.spawn_named("man").unwrap();
        set_ui(&mut w, m, "manual", 0.25, 0.75, 0.0, 0.0, 20.0, 20.0, "", 0.0);
        // stretch: fill parent frame, ox/oy=8 acts as four-side inset
        let s = w.spawn_named("str").unwrap();
        set_ui(&mut w, s, "stretch", 0.0, 0.0, 8.0, 8.0, 0.0, 0.0, "", 0.0);

        let l = solve_layout(&w, 200, 100).unwrap();
        // top-right anchor (200,0) + offset (-10,5) = (190,5), node's top-right aligned → x=160, y=5
        assert_eq!(rect(&l, &w, "corner"), UiRect { x: 160.0, y: 5.0, w: 30.0, h: 10.0 });
        // manual anchor (0.25*200, 0.75*100)=(50,75), node's same-named anchor aligned → x=50-0.25*20=45, y=75-0.75*20=60
        assert_eq!(rect(&l, &w, "man"), UiRect { x: 45.0, y: 60.0, w: 20.0, h: 20.0 });
        // stretch: starts at (8,8), width and height each minus 16 → 184x84
        assert_eq!(rect(&l, &w, "str"), UiRect { x: 8.0, y: 8.0, w: 184.0, h: 84.0 });
    }

    #[test]
    fn point_over_ui_hits_panels_and_buttons_not_bare_nodes() {
        // Viewport 200x100. Panel at top-left (0,0)-(40,20), button at top-right (160,0)-(200,20),
        // a bare Ui-only node centered at (80,40)-(120,60).
        let mut w = ui_world();
        let p = w.spawn_named("panel").unwrap();
        set_ui(&mut w, p, "top-left", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        w.set_component(p, "Panel", json!({ "color": "#ffffff" })).unwrap();
        let b = w.spawn_named("btn").unwrap();
        set_ui(&mut w, b, "top-right", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);
        w.set_component(b, "Button", json!({ "action": "x", "state": "normal" })).unwrap();
        let bare = w.spawn_named("bare").unwrap();
        set_ui(&mut w, bare, "center", 0.0, 0.0, 0.0, 0.0, 40.0, 20.0, "", 0.0);

        // Inside panel / inside button -> hit (blocks world click)
        assert!(point_over_ui(&w, 200, 100, 10.0, 10.0), "面板内应命中");
        assert!(point_over_ui(&w, 200, 100, 180.0, 10.0), "按钮内应命中");
        // Bare Ui node (no Panel/Button) doesn't block -- its covered area still lets world clicks through
        assert!(!point_over_ui(&w, 200, 100, 100.0, 50.0), "裸 Ui 节点不该挡手");
        // Blank map area / outside panel and button -> no hit
        assert!(!point_over_ui(&w, 200, 100, 50.0, 10.0), "面板外不该命中");
    }

    #[test]
    fn vbox_stacks_children_with_gap_and_padding() {
        // Container frame 100 wide × 200 tall, pad=10, gap=5, three 30-tall children stacked vertically, main=start
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
        // Content area starts at (10,10). 0th y=10, 1st y=10+30+5=45, 2nd y=80.
        // cross=start: x hugs content area's left = 10. Child nodes keep their own width 40.
        assert_eq!(rect(&l, &w, "v0"), UiRect { x: 10.0, y: 10.0, w: 40.0, h: 30.0 });
        assert_eq!(rect(&l, &w, "v1"), UiRect { x: 10.0, y: 45.0, w: 40.0, h: 30.0 });
        assert_eq!(rect(&l, &w, "v2"), UiRect { x: 10.0, y: 80.0, w: 40.0, h: 30.0 });
    }

    #[test]
    fn hbox_main_center_and_cross_center() {
        // Horizontal, frame 200 wide × 60 tall, no pad, gap=10, two 40 wide×20 tall, main=center,cross=center
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
        // Main axis usage = 40+10+40 = 90, remaining 200-90=110, main=center → start offset 55
        // 0th x=55, 1st x=55+40+10=105. cross=center: y=(60-20)/2=20
        assert_eq!(rect(&l, &w, "h0"), UiRect { x: 55.0, y: 20.0, w: 40.0, h: 20.0 });
        assert_eq!(rect(&l, &w, "h1"), UiRect { x: 105.0, y: 20.0, w: 40.0, h: 20.0 });
    }

    #[test]
    fn vbox_weight_splits_remaining_space() {
        // Frame height 100, no pad/gap. Three children: fixed 20 tall, weight=1, weight=3.
        // Remaining = 100-20 = 80, split 1:3 → 20 and 60.
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
        // 5 cells, 3 columns → 2 rows (last row 2 cells). Frame 100x100, gap=10.
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
        // g0 = (0,0), g1 = (cell_w+10, 0), g3 = (0, 55) (first in second row: cell_h 45 + gap 10)
        assert_eq!(rect(&l, &w, "g0"), UiRect { x: 0.0, y: 0.0, w: cell_w, h: 45.0 });
        assert!((rect(&l, &w, "g1").x - (cell_w + 10.0)).abs() < 1e-9);
        assert_eq!(rect(&l, &w, "g3").y, 55.0);
        assert_eq!(rect(&l, &w, "g3").x, 0.0);
    }

    #[test]
    fn empty_ui_is_zero_cost() {
        // No Ui nodes at all: has_ui=false, solve returns empty layout, early-return without solving.
        // (layout_runs is a process-global counter; parallel tests would add to each other, so the
        //  zero-cost "no recompute" assertion lives in runtime's serial system tests — see vitric-cli/tests/ui.rs. Here we only lock "empty scene returns empty".)
        let w = World::new();
        assert!(!has_ui(&w), "无 UiRoot = 无 UI");
        assert!(solve_layout(&w, 100, 100).unwrap().is_empty(), "空场布局为空");
    }

    #[test]
    fn solve_increments_run_counter_when_ui_present() {
        // With Ui nodes present solve actually runs the algorithm once (counter +1); this case alone verifies "non-empty increments".
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

        // Grid columns 0
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
        // Change w field → hash changes (dirty)
        w.set_field(e, "Ui.w", json!(60.0)).unwrap();
        assert_ne!(h1, layout_input_hash(&w, 200, 100), "改尺寸要让哈希变（标脏）");
        // Change viewport → hash changes
        assert_ne!(h1, layout_input_hash(&w, 300, 100), "改视口要让哈希变");
    }

    #[test]
    fn writing_rect_outputs_does_not_change_input_hash() {
        // rx/ry/rw/rh are layout outputs — writing them back must not change the input hash (otherwise always dirty).
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
        // Root Panel centered 100x100, a VBox inside stretched to fill, two child nodes inside the VBox.
        // Verify multiple layers: anchor → container frame → child layout.
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
        // panel centered 100x100 → x=50,y=50. vbox stretch fills → also (50,50,100,100).
        assert_eq!(rect(&l, &w, "panel"), UiRect { x: 50.0, y: 50.0, w: 100.0, h: 100.0 });
        assert_eq!(rect(&l, &w, "vbox"), UiRect { x: 50.0, y: 50.0, w: 100.0, h: 100.0 });
        // Two children stack vertically starting from vbox content area (50,50)
        assert_eq!(rect(&l, &w, "n0").y, 50.0);
        assert_eq!(rect(&l, &w, "n1").y, 80.0);
    }
}
