//! The **deterministic pure-computation** part of UI interaction (1.2) — focus-navigation
//! adjacency relations and the analytic formula for press feedback.
//!
//! Like [`crate::ui`], these are pure functions: input = (layout rects + current focus + direction),
//! output = next focus / feedback coefficient. No wall clock, no RNG, no HashMap iteration order
//! — snapshot/restore round-trips are consistent. State (current focus, button state, press timer)
//! is written into components by the runtime (enters the hash and saves); this module only provides
//! the algorithms.
//!
//! Why it lives in the render crate: focus geometry reads the layout rects (rx/ry/rw/rh), on the
//! same coordinate system as [`crate::ui::solve_layout`]; the press-feedback scale/modulate is also
//! fed to rendering. The CPU source of truth is computed here; the runtime (vitric-cli) calls it to
//! advance component state.
//!
//! Scope (1.2):
//! - [`ButtonState`]: normal / focused / pressed / disabled; state goes into components.
//! - [`navigate`]: focusable buttons form a focus ring; focus moves by layout adjacency
//!   (direction + rect geometry).
//! - [`press_scale`] / [`press_alpha`]: the scale + modulate feedback for the ticks right after a
//!   press, **analytic** (the value at tick t is computed in one step, no accumulation) — snapshot
//!   rollback + resume is bit-for-bit consistent, the same discipline as Tween.

use serde_json::Value;
use vitric_ecs::{EntityId, World};

use crate::ui::UiRect;

/// Button state machine (v1 has no hover, see contract section 4). State goes into the `Button.state`
/// component field.
/// - `Normal`: the default state.
/// - `Focused`: selected by focus (highlighted). Only one button in a UI tree is focused at a time.
/// - `Pressed`: the feedback state for the ticks right after activation (press scale + tint); when
///   the timer elapses, it returns to focused/normal.
/// - `Disabled`: cannot be focused, does not respond to click/confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    Focused,
    Pressed,
    Disabled,
}

/// All legal button-state names (for check validation + error messages + serialization; one-to-one
/// with [`ButtonState::parse`]).
pub const BUTTON_STATES: &[&str] = &["normal", "focused", "pressed", "disabled"];

impl ButtonState {
    /// State name → state. Unknown names return `None` (the caller errors with the legal-name list).
    pub fn parse(s: &str) -> Option<ButtonState> {
        Some(match s {
            "normal" => ButtonState::Normal,
            "focused" => ButtonState::Focused,
            "pressed" => ButtonState::Pressed,
            "disabled" => ButtonState::Disabled,
            _ => return None,
        })
    }

    /// State → name (for writing back to the component).
    pub fn name(self) -> &'static str {
        match self {
            ButtonState::Normal => "normal",
            ButtonState::Focused => "focused",
            ButtonState::Pressed => "pressed",
            ButtonState::Disabled => "disabled",
        }
    }
}

/// Focus-navigation direction (the standard `ui-up/down/left/right` input injections map here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    /// Parse a standard input action name (up/down/left/right with the `ui-` prefix stripped) into a direction.
    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "up" => Dir::Up,
            "down" => Dir::Down,
            "left" => Dir::Left,
            "right" => Dir::Right,
            _ => return None,
        })
    }
}

/// Geometry of one focusable control (focus navigation only looks at rect centers + edges, not at
/// which entity it is — entity identity is mapped back by the caller via the index). The rect is the
/// layout-solved reference-frame pixel rect (rx/ry/rw/rh).
#[derive(Clone, Copy, Debug)]
pub struct Focusable {
    pub rect: UiRect,
}

impl Focusable {
    fn center(&self) -> (f64, f64) {
        (self.rect.x + self.rect.w / 2.0, self.rect.y + self.rect.h / 2.0)
    }
}

/// Focus navigation: from `current` (an index into the focus ring) find the adjacent focusable
/// control in direction `dir` and return its index.
///
/// Adjacency (deterministic pure geometry, modeled on Godot's directional focus): among candidates
/// in the `dir` half-plane, pick the **nearest on the main axis** (smallest projection distance in
/// the direction of travel); on a main-axis tie pick the **smallest cross-axis offset**; on a
/// further tie pick the smaller index (the query slot order, stable). No candidate in the half-plane
/// = focus does not move (returns `current`; stops at the edge, does not wrap — deterministic and
/// never "focus jumps to the opposite side out of nowhere").
///
/// O(number of focusable controls): one pass over the candidates, no full-table scan (contract
/// section 6, item 5).
pub fn navigate(items: &[Focusable], current: usize, dir: Dir) -> usize {
    if items.is_empty() || current >= items.len() {
        return current;
    }
    let (cx, cy) = items[current].center();
    let mut best: Option<(usize, f64, f64)> = None; // (index, main-axis distance, cross-axis distance)
    for (i, item) in items.iter().enumerate() {
        if i == current {
            continue;
        }
        let (ix, iy) = item.center();
        // Half-plane filter for direction + main/cross axis components (screen y is downward: Up = smaller y).
        let (main, cross) = match dir {
            Dir::Up => (cy - iy, (ix - cx).abs()),
            Dir::Down => (iy - cy, (ix - cx).abs()),
            Dir::Left => (cx - ix, (iy - cy).abs()),
            Dir::Right => (ix - cx, (iy - cy).abs()),
        };
        if main <= 0.0 {
            continue; // Not in the dir half-plane (concentric included; orthogonal items are not adjacent).
        }
        let better = match best {
            None => true,
            Some((_, bm, bc)) => main < bm || (main == bm && cross < bc),
        };
        if better {
            best = Some((i, main, cross));
        }
    }
    best.map(|(i, _, _)| i).unwrap_or(current)
}

/// Press-feedback duration in ticks (how many ticks the pressed state holds before falling back).
/// A brief flash; pure feedback that does not block interaction.
pub const PRESS_TICKS: u64 = 6;

/// Press-feedback scale coefficient: the scale at tick `t` (0..PRESS_TICKS). **Analytic** — a
/// symmetric triangular envelope (press shrinks to `min_scale` then bounces back to 1.0), with `min`
/// at the midpoint. t outside the range = 1.0 (no feedback). Pure function: same t → same value,
/// no dependency on the previous frame (snapshot rollback + resume is consistent; accumulation is
/// forbidden, same discipline as Tween).
///
/// `min_scale`: the scale at the deepest point of the press (e.g. 0.92 = shrunk to 92%). PetClaw
/// experience: press = scale + tint.
pub fn press_scale(t: u64, min_scale: f64) -> f64 {
    if t >= PRESS_TICKS {
        return 1.0;
    }
    // Triangular envelope: 0 → midpoint linearly down to min, midpoint → end linearly back up to 1. Halfway = PRESS_TICKS/2.
    let half = PRESS_TICKS as f64 / 2.0;
    let tf = t as f64;
    let depth = if tf <= half { tf / half } else { (PRESS_TICKS as f64 - tf) / half };
    // depth ∈ [0,1], 0 = not pressed (scale 1), 1 = deepest (scale min).
    1.0 - (1.0 - min_scale) * depth
}

/// Press-feedback tint intensity: the modulate coefficient at tick `t` (0 = original color,
/// 1 = peak tint, brightest/darkest). Same triangular envelope as [`press_scale`] — when the scale
/// is deepest the tint is also strongest. The renderer interpolates between the original color and
/// the highlight color using this. Pure function; accumulation forbidden.
pub fn press_modulate(t: u64) -> f64 {
    if t >= PRESS_TICKS {
        return 0.0;
    }
    let half = PRESS_TICKS as f64 / 2.0;
    let tf = t as f64;
    if tf <= half {
        tf / half
    } else {
        (PRESS_TICKS as f64 - tf) / half
    }
}

/// **Press-feedback draw parameters** for one UI node: shared by the CPU and GPU paths (the formulas
/// are line-by-line isomorphic). When `Button` is attached and `press_t ≥ 0`, computes
/// **the rect after center-scaled shrinking + the brighten coefficient** via
/// [`press_scale`]/[`press_modulate`]; otherwise the original rect + 0 brighten. Pure function
/// (input = `press_t`/`min_scale` from the component + the layout rect), does not touch layout /
/// simulation RNG — rendering-decoration discipline, replay/snapshot consistent.
pub fn ui_press_feedback(world: &World, id: EntityId, rect: UiRect) -> (UiRect, f64) {
    if !world.has_component(id, "Button") {
        return (rect, 0.0);
    }
    let press_t = world.get_field(id, "Button.press_t").ok().and_then(Value::as_i64).unwrap_or(-1);
    if press_t < 0 {
        return (rect, 0.0);
    }
    let t = press_t as u64;
    let min_scale = world
        .get_field(id, "Button.min_scale")
        .ok()
        .and_then(Value::as_f64)
        .unwrap_or(0.92);
    let scale = press_scale(t, min_scale);
    let modulate = press_modulate(t);
    let cx = rect.x + rect.w / 2.0;
    let cy = rect.y + rect.h / 2.0;
    let nw = rect.w * scale;
    let nh = rect.h * scale;
    (UiRect { x: cx - nw / 2.0, y: cy - nh / 2.0, w: nw, h: nh }, modulate)
}

/// Brighten RGB (modulate ∈ [0,1], 0 = original color, 1 = full white). Press-feedback tint:
/// linear interpolation toward white. Alpha is untouched. Shared by the CPU and GPU paths.
pub fn modulate_rgb(rgba: &mut [u8; 4], modulate: f64) {
    if modulate <= 0.0 {
        return;
    }
    let m = modulate.clamp(0.0, 1.0);
    for c in rgba.iter_mut().take(3) {
        let v = *c as f64;
        *c = (v + (255.0 - v) * m).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fo(x: f64, y: f64, w: f64, h: f64) -> Focusable {
        Focusable { rect: UiRect { x, y, w, h } }
    }

    #[test]
    fn vertical_navigation_moves_to_adjacent_button() {
        // Three buttons in a vertical column (VBox-style): y = 0,100,200, same width and x.
        let items = vec![fo(0.0, 0.0, 50.0, 40.0), fo(0.0, 100.0, 50.0, 40.0), fo(0.0, 200.0, 50.0, 40.0)];
        // From 0 down → 1; down again → 2.
        assert_eq!(navigate(&items, 0, Dir::Down), 1);
        assert_eq!(navigate(&items, 1, Dir::Down), 2);
        // At the bottom, down again = no move (no wrapping).
        assert_eq!(navigate(&items, 2, Dir::Down), 2);
        // Up is symmetric.
        assert_eq!(navigate(&items, 2, Dir::Up), 1);
        assert_eq!(navigate(&items, 0, Dir::Up), 0, "到顶不动");
        // Left/right has no candidates in a pure vertical column = no move.
        assert_eq!(navigate(&items, 1, Dir::Left), 1);
        assert_eq!(navigate(&items, 1, Dir::Right), 1);
    }

    #[test]
    fn picks_nearest_in_main_axis_then_least_cross_offset() {
        // Currently at (0,0). Two candidates below: slightly farther straight below (0,200), a bit
        // closer down-right (50,100). Both are in the Down half-plane; main-axis distance 100 < 200,
        // so the (50,100) one (index 1) is picked.
        let items = vec![fo(0.0, 0.0, 10.0, 10.0), fo(50.0, 100.0, 10.0, 10.0), fo(0.0, 200.0, 10.0, 10.0)];
        assert_eq!(navigate(&items, 0, Dir::Down), 1, "主轴更近的优先");
        // On a main-axis tie, the smaller cross-axis offset wins.
        let items2 = vec![fo(0.0, 0.0, 10.0, 10.0), fo(80.0, 100.0, 10.0, 10.0), fo(10.0, 100.0, 10.0, 10.0)];
        assert_eq!(navigate(&items2, 0, Dir::Down), 2, "同主轴距取交叉轴更近(下标2 x=10)");
    }

    #[test]
    fn horizontal_grid_navigation() {
        // 2x2 grid: (0,0)(100,0) / (0,100)(100,100).
        let items = vec![
            fo(0.0, 0.0, 50.0, 50.0),
            fo(100.0, 0.0, 50.0, 50.0),
            fo(0.0, 100.0, 50.0, 50.0),
            fo(100.0, 100.0, 50.0, 50.0),
        ];
        assert_eq!(navigate(&items, 0, Dir::Right), 1);
        assert_eq!(navigate(&items, 0, Dir::Down), 2);
        assert_eq!(navigate(&items, 1, Dir::Down), 3);
        assert_eq!(navigate(&items, 3, Dir::Left), 2);
        assert_eq!(navigate(&items, 3, Dir::Up), 1);
    }

    #[test]
    fn empty_or_oob_current_is_noop() {
        assert_eq!(navigate(&[], 0, Dir::Down), 0);
        let items = vec![fo(0.0, 0.0, 1.0, 1.0)];
        assert_eq!(navigate(&items, 5, Dir::Down), 5, "越界 current 原样返回");
    }

    #[test]
    fn press_scale_is_analytic_envelope_exact_endpoints() {
        // Halfway = 3. t=0 no scale (1.0); t=3 deepest (min); t=6 back to 1.0; t≥6 no feedback.
        let min = 0.9;
        assert_eq!(press_scale(0, min), 1.0);
        assert!((press_scale(3, min) - min).abs() < 1e-12, "中点 = min");
        assert!((press_scale(1, min) - (1.0 - 0.1 * (1.0 / 3.0))).abs() < 1e-12);
        assert!((press_scale(5, min) - (1.0 - 0.1 * (1.0 / 3.0))).abs() < 1e-12, "对称：t=5 与 t=1 相等");
        assert_eq!(press_scale(6, min), 1.0);
        assert_eq!(press_scale(99, min), 1.0, "超区间无反馈");
        // Pure function: same t same value, repeatedly consistent (does not depend on any external state).
        assert_eq!(press_scale(2, min), press_scale(2, min));
    }

    #[test]
    fn press_modulate_envelope_matches_scale_shape() {
        assert_eq!(press_modulate(0), 0.0);
        assert!((press_modulate(3) - 1.0).abs() < 1e-12, "中点染色最浓");
        assert_eq!(press_modulate(6), 0.0);
        assert_eq!(press_modulate(7), 0.0);
        // Symmetric.
        assert!((press_modulate(1) - press_modulate(5)).abs() < 1e-12);
    }

    #[test]
    fn button_state_parse_roundtrip() {
        for &name in BUTTON_STATES {
            assert_eq!(ButtonState::parse(name).unwrap().name(), name);
        }
        assert_eq!(ButtonState::parse("hover"), None, "hover 不是 v1 状态");
    }
}
