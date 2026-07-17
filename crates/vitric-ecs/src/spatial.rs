//! Egocentric spatial relations.
//!
//! Why this lives in the ecs layer: vision/multimodal models are bad at spatial reasoning from **absolute coordinates**
//! ("is the exit to my left or right", "can I reach it" are often wrong). Vitric is a glass box that knows
//! all entity positions, so it should **pre-compute** the relations "relative to some focal point" and feed them
//! to the model, rather than dumping a pile of world coordinates and making the model subtract them.
//!
//! This computation is reused by two "what the AI sees" sites:
//! - vitric-render's `describe_world_with_assets` (control plane `render/describe`);
//! - vitric-playtest's `SceneView` (playtest observation).
//!
//! The lowest common dependency of both crates is vitric-ecs (render does not depend on data/playtest),
//! and this is pure position arithmetic — putting it here lets both call it without creating a cycle.
//!
//! **Pure functions**: only read position/size, do not read World internal state, unit-testable, not hashed, do not affect determinism.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{EntityId, World};

/// An entity's axis-aligned placement in the world (AABB center + half-width/half-height).
/// w/h are its size (e.g. `Sprite.w`/`Sprite.h`) — used for the tolerance in "adjacent" and "same row/col" checks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// World-coordinate center x (y is up).
    pub x: f64,
    pub y: f64,
    /// Placement width/height (world units). Pass 0 when there is no notion of size: adjacency degrades to
    /// "centers coincide counts as touching", same row/col degrades to "centers strictly aligned counts" —
    /// safer than fabricating an arbitrary size.
    pub w: f64,
    pub h: f64,
}

impl Placement {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Placement {
        Placement { x, y, w, h }
    }
}

/// Egocentric spatial relation centered on the focal (focal -> target). All direction/same-row/same-col
/// are computed in **world coordinates** (y up), with no screen flipping mixed in — the semantic
/// observation is about the world itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelativeSpatial {
    /// 8-direction word (world coordinates, y up): which direction the focal sees the target in.
    pub direction: Direction,
    /// Euclidean distance (center to center, world units).
    pub distance: f64,
    /// Approximately same row (vertical offset within tolerance).
    pub same_row: bool,
    /// Approximately same column (horizontal offset within tolerance).
    pub same_col: bool,
    /// Touching/adjacent (the two AABBs' edges are close or overlap) — answers "can I reach it".
    pub adjacent: bool,
    /// Line of sight blocked: the segment from the focal center to the target center crosses the AABB of
    /// a **third-party Solid entity** (answers "can I see it / is there a wall in between"). Pure [`relate`]
    /// (positions only) cannot compute occlusion, so this is always false; for occlusion you need the
    /// world-aware [`relate_in_world`].
    pub blocked: bool,
}

/// 8 directions (plus "coincident"). World coordinates, y up: up = +y, right = +x.
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
    /// Focal and target centers nearly coincide (direction is meaningless).
    Coincident,
}

impl Direction {
    /// English direction word for model/JSON consumption (hyphenated, matching common 8-direction notation).
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

/// Distance rounding precision (two decimal places): world units are typically measured in "cells",
/// two decimals is enough to distinguish adjacent vs. one-apart, and won't leak floating-point
/// trailing-bit noise into observations.
const DISTANCE_DP: f64 = 100.0;

/// Center-coincidence threshold (world units): if the Euclidean distance between the two centers is below it,
/// they are treated as "coincident" and direction is meaningless. Kept very small (one-thousandth of a cell),
/// only catching the truly-same-point degenerate case.
const COINCIDENT_EPS: f64 = 1e-3;

/// Adjacent gap tolerance (world units): if the edge gap between two AABBs on an axis is <= it, they count as "touching".
/// Kept very small to avoid mistaking "half a cell apart" for touching; when edges align bit-for-bit or overlap,
/// the gap is 0 and it hits naturally.
const ADJACENT_GAP_EPS: f64 = 1e-6;

/// Compute the egocentric relation focal -> target (pure function, world coordinates y up).
///
/// - `direction`: based on (dx, dy) falls into one of 8 directions. Uses a 2:1 slope band — `|dy| < dx/2`
///   goes to pure left/right, `|dx| < dy/2` goes to pure up/down, in between goes diagonal.
///   Center coincidence (distance < eps) = `here`.
/// - `distance`: center-to-center Euclidean distance, rounded to two decimals.
/// - `same_row` / `same_col`: whether the vertical/horizontal offset is within "half a sprite's height/width"
///   tolerance. The tolerance is half of the average of the two heights/widths (`(h_f + h_t) / 4`) — giving
///   small and large targets a symmetric, size-adaptive bandwidth.
/// - `adjacent`: whether the two AABBs are edge-close or overlapping. Both axes' gaps must be <= eps
///   (touching along one edge, or partial overlap, both satisfy; a gap between them does not).
pub fn relate(focal: Placement, target: Placement) -> RelativeSpatial {
    let dx = target.x - focal.x;
    let dy = target.y - focal.y;
    let distance = (dx * dx + dy * dy).sqrt();

    let direction = if distance < COINCIDENT_EPS {
        Direction::Coincident
    } else {
        direction_of(dx, dy)
    };

    // Same row/col tolerance: half of the "average sprite height/width". With size 0, tolerance is 0 -> degrades to strict alignment.
    let row_tol = (focal.h + target.h) / 4.0;
    let col_tol = (focal.w + target.w) / 4.0;
    let same_row = dy.abs() <= row_tol;
    let same_col = dx.abs() <= col_tol;

    // Adjacent: edge gap on both axes <= eps. gap = max(0, |center diff| - sum of two half-sides);
    // negative values (overlap) are clamped to 0, overlap naturally counts as adjacent.
    let gap_x = (dx.abs() - (focal.w + target.w) / 2.0).max(0.0);
    let gap_y = (dy.abs() - (focal.h + target.h) / 2.0).max(0.0);
    let adjacent = gap_x <= ADJACENT_GAP_EPS && gap_y <= ADJACENT_GAP_EPS;

    RelativeSpatial {
        direction,
        distance: (distance * DISTANCE_DP).round() / DISTANCE_DP,
        same_row,
        same_col,
        adjacent,
        // Pure positions cannot compute occlusion: without a world there is no way to know if a wall is in between. Always false,
        // overridden by relate_in_world when a World is available.
        blocked: false,
    }
}

/// World-aware relation: on top of [`relate`] (pure positions), also computes whether the focal -> target
/// line of sight is blocked by a **third-party Solid entity** (`blocked`).
///
/// Why a separate world-aware version: pure [`relate`] only takes two `Placement`s and has no way to know
/// whether there are other walls on the field — occlusion is a concept that only exists for "third parties
/// in the world". This function takes the two placements and calls `relate` to get direction/distance/
/// same-row-col/adjacent, then adds a geometric check: does the segment from the focal center to the target
/// center cross the AABB of any Solid entity that is **neither the focal nor the target**.
///
/// - Solid entity = an entity with a `Solid` component (the engine's "body-blocking thing"; with projection
///   enabled it also blocks light, see vitric-render's occluder convention). AABB is taken from `Position`
///   (center) + `Collider` (w/h); Solids missing Position/Collider do not participate (no box -> treated as
///   not blocking).
/// - Segment-AABB intersection uses the standard slab method ([`segment_hits_aabb`], pure geometry, deterministic).
/// - Solid enumeration goes through `World::query` in slot order (BTreeMap, deterministic) — determinism does
///   not depend on iteration order (the result is "any hit", order-independent), but ordered iteration makes
///   behavior reproducible.
/// - The focal or target itself, even if Solid, does not count as blocking itself (the segment endpoints lie
///   on their own box edges, slab would report a hit — explicitly excluded, otherwise "looking out from inside
///   a wall" would always be blocked).
///
/// When the focal or target cannot be read as a Placement (no Position), falling back to pure [`relate`] is
/// not viable — both must have placements for this to be meaningful; the caller must guarantee this (both
/// describe/SceneView only call it when both have Position).
pub fn relate_in_world(world: &World, focal: EntityId, target: EntityId) -> RelativeSpatial {
    let fp = placement_of(world, focal).unwrap_or(Placement::new(0.0, 0.0, 0.0, 0.0));
    let tp = placement_of(world, target).unwrap_or(Placement::new(0.0, 0.0, 0.0, 0.0));
    let mut rel = relate(fp, tp);
    rel.blocked = line_of_sight_blocked(world, focal, target, fp, tp);
    rel
}

/// Whether the focal center -> target center line of sight is blocked by a third-party Solid (the geometric
/// core of [`relate_in_world`]). Excludes the focal and target themselves; a hit on any third-party Solid's
/// AABB returns true.
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
        // The focal/target themselves do not block their own line of sight
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

/// Read a Solid entity's AABB [x0, y0, x1, y1] (world coordinates): center = `Position`,
/// half-width/half-height = half of `Collider.w/h`. Any field missing/non-numeric -> None (no box -> treated as not blocking).
fn solid_aabb(world: &World, id: EntityId) -> Option<(f64, f64, f64, f64)> {
    let pos = world.get_component(id, "Position").ok()?;
    let x = pos.get("x").and_then(Value::as_f64)?;
    let y = pos.get("y").and_then(Value::as_f64)?;
    let col = world.get_component(id, "Collider").ok()?;
    let w = col.get("w").and_then(Value::as_f64)?;
    let h = col.get("h").and_then(Value::as_f64)?;
    Some((x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0))
}

/// Read an entity's world placement: `Position` is required (missing -> None); size taken from `Sprite.w/h`,
/// missing means 0 (consistent with the placement convention in describe/SceneView).
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

/// Whether segment (px,py)->(qx,qy) intersects AABB [x0,y0,x1,y1] (slab method).
/// Same geometry as vitric-render's `segment_hits_aabb` (that one is render-private, used for light blocking;
/// this one is for semantic-layer line-of-sight occlusion): axis-parallel axes (component diff < 1e-12)
/// degrade to "the start point must fall within that axis's slab", with no division (dividing by a near-zero
/// would yield ±inf, and inf is unreliable on the min/max chain).
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

/// Configuration for the ASCII grid map (input to [`ascii_map`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AsciiMapOpts {
    /// Cell edge length (world units). `Some` uses it directly; `None` auto-infers (the mode of Sprite
    /// widths/heights on the field, with a fallback of 1.0) — any game (continuous/grid) gets a reasonably
    /// scaled rough map.
    pub cell: Option<f64>,
    /// Window radius (in cells). Centered on the focal, radius cells in each direction -> side length (2*radius+1).
    /// Default 7 ([`AsciiMapOpts::default`]) -> 15x15, so even a huge world is clipped to this block.
    pub radius: usize,
}

impl Default for AsciiMapOpts {
    fn default() -> AsciiMapOpts {
        AsciiMapOpts { cell: None, radius: 7 }
    }
}

/// Focal-centered ASCII grid map (output of [`ascii_map`]). A rough spatial map for model navigation:
/// more intuitive than a list of coordinates for "who is in which direction, how many cells away, is there a wall in between".
#[derive(Debug, Clone, PartialEq)]
pub struct AsciiMap {
    /// Row-by-row strings, row 0 at the **top** (world y larger is up — consistent with the screen/direction words).
    pub rows: Vec<String>,
    /// Symbol -> entity name/id legend (deterministic order: BTreeMap sorts by symbol character).
    /// `@`=focal, `#`=Solid occluder are not in the legend (their meaning is fixed).
    pub legend: BTreeMap<char, String>,
    /// The cell edge length actually used (world units; auto-inferred or from opts).
    pub cell: f64,
    /// The focal's [row, col] in the grid (always at the center: [radius, radius]).
    pub focal_rc: [usize; 2],
}

impl AsciiMap {
    /// Serialize into the `ascii_map` block shared by both "what the AI sees" sites:
    /// `{"grid":["...","..."],"legend":{...},"cell_size":n,"focal_at":[r,c]}`.
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

/// Auto-infer the cell edge length: the most frequent size value among the (w, h) of entities on the field
/// **that have a Sprite** (the mode). w and h are tallied together (one square sprite contributes two votes
/// of the same value). No Sprite sizes at all -> fallback 1.0. Using the mode rather than average/max: the
/// most common sprite size is this game's "one cell" unit, robust to occasional big background blocks / small props.
/// Floating-point values are keyed by "the bit string rounded to 6 digits" to avoid NaN/trailing-bit noise splitting votes.
fn infer_cell(world: &World) -> f64 {
    // Key = the quantized size value as i64 (×1e6 rounded), value = (count, original size)
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
    // Take the one with the most occurrences; on a tie, take the smaller size (BTreeMap iterates by quantized key
    // ascending, reduce uses **strict `>`**: when counts are equal it keeps the a encountered first, and the first
    // encountered is the smaller size -> ties leave the smaller size)
    tally
        .values()
        .copied()
        .reduce(|a, b| if b.0 > a.0 { b } else { a })
        .map(|(_, size)| size)
        .unwrap_or(1.0)
}

/// Draw a bounded ASCII grid map centered on the focal (pure function, same world + same focal -> same map).
///
/// Design (see vitric's "best model-readable scene view"):
/// - **Cell edge length cell**: use `opts.cell` if given; otherwise auto-infer = mode of Sprite widths/heights
///   on the field ([`infer_cell`], fallback 1.0).
/// - **Bounded window**: centered on the focal, radius `opts.radius` cells (default 7 -> 15x15). World is
///   quantized to cells: the entity center's offset from the focal, divided by cell and rounded to a cell
///   index; entities outside the window are dropped (any size of world is clipped to this block, no overflow).
///   Row 0 is at the top (world y larger is up).
/// - **Symbols**: `@`=focal; `#`=Solid occluder; other entities are assigned `a`, `b`, ..., `z` in
///   **deterministic order** (named ones first by name order, then by id) (after running out of letters,
///   continue with `A`-`Z`, `0`-`9`; entities beyond that are not drawn — a rough map is enough). The legend
///   records symbol -> name/id.
/// - **Multiple entities in one cell**: the symbol with the earliest assignment order occupies the cell
///   (focal `@` always overrides everything; next `#`; then letters by assignment order). Space = empty.
/// - **Determinism**: window, quantization, symbol assignment are all deterministic, not hashed, do not affect replay.
///
/// The focal must have a Position ([`placement_of`] returning None -> returns an empty map with only `@` at the center,
/// no error: a semantic view should not entirely break just because the focal has no coordinates).
pub fn ascii_map(world: &World, focal: EntityId, opts: &AsciiMapOpts) -> AsciiMap {
    let radius = opts.radius;
    let side = 2 * radius + 1;
    let cell = opts.cell.unwrap_or_else(|| infer_cell(world));
    let focal_rc = [radius, radius];

    // Focal center: if no coordinates, place at origin (just so relative offsets can be computed; in that case
    // other entities mostly won't fall in the window either)
    let (fx, fy) = match placement_of(world, focal) {
        Some(p) => (p.x, p.y),
        None => (0.0, 0.0),
    };

    // Each cell stores the "assignment priority" + character of its current occupant: lower priority should be
    // displayed (focal 0, Solid 1, letters 2+assignment order). None = space.
    let mut grid: Vec<Option<(u32, char)>> = vec![None; side * side];
    let mut legend: BTreeMap<char, String> = BTreeMap::new();

    // Quantize a world coordinate to a cell index (row, col), returning only if it falls within the window.
    // col: dx/cell rounded + center column; row: dy is up-positive, world y larger -> smaller row index, so negate.
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

    // A candidate wants to occupy a cell: only overrides if its priority is lower (should be displayed more).
    let place = |grid: &mut Vec<Option<(u32, char)>>, row: usize, col: usize, prio: u32, ch: char| {
        let slot = &mut grid[row * side + col];
        if slot.map(|(p, _)| prio < p).unwrap_or(true) {
            *slot = Some((prio, ch));
        }
    };

    // Focal: always at the center, always @ (priority 0, overrides everything)
    place(&mut grid, focal_rc[0], focal_rc[1], 0, '@');

    // Solid occluders: priority 1, fixed symbol # (not in the legend, meaning is fixed)
    for id in world.query(&["Solid", "Position", "Collider"]) {
        if id == focal {
            continue; // If the focal itself is Solid, it's already occupied by @; don't draw twice
        }
        if let Some(p) = placement_of(world, id) {
            if let Some((row, col)) = quantize(p.x, p.y) {
                place(&mut grid, row, col, 1, '#');
            }
        }
    }

    // Other entities: assign letters in deterministic order. Order = (has-name first, name, id) — same stance
    // as the primary sort (named gameplay subjects get stable characters first). Focal/Solid already handled, skip.
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
    // Named ones first (true first), then by name lexicographically, finally id as tiebreaker
    others.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    // Symbol table: a-z A-Z 0-9 (62, enough for a rough map; entities beyond are not drawn)
    let symbols: Vec<char> = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
    let mut next = 0usize;
    for (has_name, name, id) in &others {
        if let Some(p) = placement_of(world, *id) {
            if let Some((row, col)) = quantize(p.x, p.y) {
                if next >= symbols.len() {
                    break; // Characters exhausted, the rest are not drawn (a rough map is enough)
                }
                let ch = symbols[next];
                next += 1;
                let label = if *has_name { name.clone() } else { id.to_string() };
                legend.insert(ch, label);
                // Priority 2 + assignment order: a symbol assigned earlier overrides later ones in the same cell
                place(&mut grid, row, col, 2 + (next as u32), ch);
            }
        }
    }

    // Render into rows (row 0 at the top)
    let rows: Vec<String> = (0..side)
        .map(|r| {
            (0..side)
                .map(|c| grid[r * side + c].map(|(_, ch)| ch).unwrap_or(' '))
                .collect::<String>()
        })
        .collect();

    AsciiMap { rows, legend, cell, focal_rc }
}

/// (dx, dy) -> 8 directions (world coordinates, y up). The caller has already excluded the degenerate
/// center-coincident case. Slope band: when one axis dominates absolutely (the other < half of this one)
/// it goes to a cardinal direction, otherwise diagonal.
fn direction_of(dx: f64, dy: f64) -> Direction {
    let ax = dx.abs();
    let ay = dy.abs();
    if ay * 2.0 < ax {
        // Horizontal-dominant: pure left/right
        if dx >= 0.0 { Direction::Right } else { Direction::Left }
    } else if ax * 2.0 < ay {
        // Vertical-dominant: pure up/down
        if dy >= 0.0 { Direction::Up } else { Direction::Down }
    } else {
        // Diagonal
        match (dx >= 0.0, dy >= 0.0) {
            (true, true) => Direction::UpRight,
            (false, true) => Direction::UpLeft,
            (false, false) => Direction::DownLeft,
            (true, false) => Direction::DownRight,
        }
    }
}

impl RelativeSpatial {
    /// Serialize into the `relative_to_focal` block shared by both "what the AI sees" sites. Field names for model consumption:
    /// `{"direction","distance","same_row","same_col","adjacent","blocked"}`.
    /// `blocked` is always false via pure [`relate`] (no world = no way to know if there's a wall); only
    /// [`relate_in_world`] computes the real value based on third-party Solids.
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

    /// Size-less point (w=h=0): only test direction/distance/alignment, no size tolerance mixed in.
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
        // dx dominant (dy less than half of dx) -> pure right, not diagonal
        assert_eq!(relate(pt(0.0, 0.0), pt(10.0, 2.0)).direction, Direction::Right);
        // dy dominant -> pure up
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
        // Round to two decimals
        let d = relate(pt(0.0, 0.0), pt(1.0, 1.0)).distance;
        assert_eq!(d, 1.41, "sqrt(2)≈1.41421 → 1.41");
    }

    #[test]
    fn same_row_and_col_with_tolerance() {
        // Two 1x1 cells, tolerance = (1+1)/4 = 0.5
        let f = Placement::new(0.0, 0.0, 1.0, 1.0);
        let same_row = Placement::new(5.0, 0.4, 1.0, 1.0); // dy=0.4 <= 0.5
        let r = relate(f, same_row);
        assert!(r.same_row, "竖向偏移 0.4 在半格容差内");
        assert!(!r.same_col, "横向差 5 远超容差");

        let off_row = Placement::new(5.0, 0.6, 1.0, 1.0); // dy=0.6 > 0.5
        assert!(!relate(f, off_row).same_row, "竖向偏移 0.6 超容差");

        let same_col = Placement::new(0.3, 9.0, 1.0, 1.0); // dx=0.3 <= 0.5
        let r2 = relate(f, same_col);
        assert!(r2.same_col, "横向偏移 0.3 在半格容差内");
        assert!(!r2.same_row);
    }

    #[test]
    fn adjacent_touching_and_overlapping() {
        // Two 2x2 cells (half-width 1), center distance 2.0 = edges exactly aligned -> adjacent
        let f = Placement::new(0.0, 0.0, 2.0, 2.0);
        assert!(relate(f, Placement::new(2.0, 0.0, 2.0, 2.0)).adjacent, "边缘贴齐算相邻");
        // Overlap also counts as adjacent
        assert!(relate(f, Placement::new(1.0, 0.0, 2.0, 2.0)).adjacent, "重叠算相邻");
        // A gap between them (center distance 2.5, gap 0.5) -> not adjacent
        assert!(!relate(f, Placement::new(2.5, 0.0, 2.0, 2.0)).adjacent, "隔缝不算相邻");
    }

    #[test]
    fn zero_size_adjacency_is_strict() {
        // Size-less points: only center coincidence counts as adjacent
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
        // Pure relate has no world -> no way to know if there's a wall, blocked always false
        assert_eq!(v["blocked"], false);
    }

    // ---- relate_in_world: line-of-sight occlusion (blocked) ----

    use crate::World;
    use serde_json::json;

    /// Spawn an entity with Position (+optional Sprite size), returns id.
    fn spawn_at(w: &mut World, name: Option<&str>, x: f64, y: f64) -> EntityId {
        let id = match name {
            Some(n) => w.spawn_named(n).unwrap(),
            None => w.spawn(),
        };
        w.set_component(id, "Position", json!({ "x": x, "y": y })).unwrap();
        id
    }

    /// Place a w x h Solid wall at (x,y) (Solid+Position+Collider).
    fn spawn_wall(w: &mut World, x: f64, y: f64, cw: f64, ch: f64) -> EntityId {
        let id = w.spawn();
        w.set_component(id, "Position", json!({ "x": x, "y": y })).unwrap();
        w.set_component(id, "Collider", json!({ "w": cw, "h": ch })).unwrap();
        w.set_component(id, "Solid", json!({})).unwrap();
        id
    }

    #[test]
    fn blocked_true_when_solid_between_focal_and_target() {
        // Focal(0,0) target(10,0), a 2x4 wall straight in the middle(5,0) -> line of sight blocked
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 0.0);
        spawn_wall(&mut w, 5.0, 0.0, 2.0, 4.0);
        assert!(relate_in_world(&w, focal, target).blocked, "中间有墙 → blocked");
    }

    #[test]
    fn blocked_false_when_solid_removed() {
        // Same as above but the wall is moved aside (y=20, far from the line of sight) -> not blocked
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 0.0);
        spawn_wall(&mut w, 5.0, 20.0, 2.0, 4.0);
        assert!(!relate_in_world(&w, focal, target).blocked, "墙不在视线上 → 不挡");
    }

    #[test]
    fn blocked_false_with_no_solid() {
        // No Solids at all -> blocked always false (does not break no-occlusion scenes)
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let target = spawn_at(&mut w, Some("coin"), 10.0, 0.0);
        let r = relate_in_world(&w, focal, target);
        assert!(!r.blocked);
        // Other fields as usual (consistent with pure relate)
        assert_eq!(r.direction, Direction::Right);
        assert_eq!(r.distance, 10.0);
    }

    #[test]
    fn focal_and_target_solids_dont_block_themselves() {
        // Focal and target themselves are Solid (their boxes cover the segment endpoints): do not count as blocking themselves
        let mut w = World::new();
        let focal = spawn_wall(&mut w, 0.0, 0.0, 2.0, 2.0);
        let target = spawn_wall(&mut w, 10.0, 0.0, 2.0, 2.0);
        assert!(!relate_in_world(&w, focal, target).blocked, "端点自己的墙不挡自己");
        // But adding a third-party Solid in between blocks
        spawn_wall(&mut w, 5.0, 0.0, 2.0, 4.0);
        assert!(relate_in_world(&w, focal, target).blocked, "第三方墙照样挡");
    }

    #[test]
    fn blocked_diagonal_line_of_sight() {
        // Diagonal line of sight: focal(0,0)->target(10,10), wall at(5,5) -> blocked; wall at(5,0) off the diagonal -> not blocked
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
        // Segment-AABB boundary cases (directly testing the geometric core)
        // 1) Horizontal line grazing the AABB's top edge (line y=1, box y in [1,3]) -> counts as hit (boundary counts as intersection)
        assert!(segment_hits_aabb((0.0, 1.0), (10.0, 1.0), (4.0, 1.0, 6.0, 3.0)));
        // 2) Horizontal line below the box (line y=0, box y in [1,3]) -> no hit
        assert!(!segment_hits_aabb((0.0, 0.0), (10.0, 0.0), (4.0, 1.0, 6.0, 3.0)));
        // 3) Vertical line through the box (axis-parallel degenerate branch: dx approx 0, start x falls inside the slab)
        assert!(segment_hits_aabb((5.0, -10.0), (5.0, 10.0), (4.0, -1.0, 6.0, 1.0)));
        // 4) Vertical line to the left of the box (dx approx 0, start x outside the slab) -> no hit
        assert!(!segment_hits_aabb((0.0, -10.0), (0.0, 10.0), (4.0, -1.0, 6.0, 1.0)));
        // 5) Segment too short, can't reach the box (box is far away, the segment cannot reach within t in [0,1]) -> no hit
        assert!(!segment_hits_aabb((0.0, 0.0), (1.0, 0.0), (4.0, -1.0, 6.0, 1.0)));
    }

    // ---- ascii_map: focal-centered grid map ----

    #[test]
    fn ascii_map_focal_at_center() {
        // Focal at the center, grid size = 2*radius+1
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        assert_eq!(m.rows.len(), 7, "7×7");
        assert_eq!(m.focal_rc, [3, 3]);
        assert_eq!(m.rows[3].chars().nth(3), Some('@'), "@ 在正中");
    }

    #[test]
    fn ascii_map_entity_and_solid_placed() {
        // hero(0,0) focal; coin(2,0) two cells right; wall(0,2) two cells up (world y larger -> smaller row index)
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let _coin = spawn_at(&mut w, Some("coin"), 2.0, 0.0);
        spawn_wall(&mut w, 0.0, 2.0, 1.0, 1.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        // Center [3,3]=@; coin at [3,5] (same row, two cells right); wall at [1,3] (same column, two cells up)
        assert_eq!(m.rows[3].chars().nth(3), Some('@'));
        assert_eq!(m.rows[3].chars().nth(5), Some('a'), "coin 分到 a，右两格");
        assert_eq!(m.rows[1].chars().nth(3), Some('#'), "墙在上两格");
        assert_eq!(m.legend.get(&'a').map(String::as_str), Some("coin"));
        assert!(!m.legend.contains_key(&'#'), "# 含义固定不进图例");
    }

    #[test]
    fn ascii_map_infers_cell_from_sprite_mode() {
        // Most sprites are 16x16, one outlier 64x64: auto-inferred cell should be 16 (the mode)
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
        // No Sprite sizes at all -> fallback 1.0
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts::default());
        assert_eq!(m.cell, 1.0);
    }

    #[test]
    fn ascii_map_window_clips_far_entities() {
        // Far entities (beyond radius cells) are not in the map, nor in the legend
        let mut w = World::new();
        let focal = spawn_at(&mut w, Some("hero"), 0.0, 0.0);
        let _far = spawn_at(&mut w, Some("faraway"), 1000.0, 0.0);
        let m = ascii_map(&w, focal, &AsciiMapOpts { cell: Some(1.0), radius: 3 });
        assert!(!m.legend.values().any(|v| v == "faraway"), "窗口外不画: {:?}", m.legend);
        // The whole map has only @, nothing else
        let non_empty: String = m.rows.join("").chars().filter(|c| *c != ' ').collect();
        assert_eq!(non_empty, "@");
    }

    #[test]
    fn ascii_map_is_deterministic() {
        // Same world + same focal -> same map
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
        // Letters assigned by (has-name first, name order): apple before coin (lexicographic)
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
