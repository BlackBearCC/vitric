//! UI 交互的**确定性纯计算**部分（1.2）——焦点导航的相邻关系、按下反馈的解析式。
//!
//! 和 [`crate::ui`] 一样是纯函数：输入 =（布局矩形 + 当前焦点 + 方向），输出 = 下一个
//! 焦点 / 反馈系数。无墙钟、无随机、无 HashMap 迭代序——snapshot/restore 往返一致。
//! 状态（当前焦点、按钮状态、按下计时）由运行时写进组件（进哈希进存档），这里只给算法。
//!
//! 为什么放渲染 crate：焦点几何要读布局矩形（rx/ry/rw/rh），和 [`crate::ui::solve_layout`]
//! 同一份坐标系；按下反馈的 scale/modulate 也要喂给渲染。CPU 真相源在这里算，
//! 运行时（vitric-cli）调它推进组件状态。
//!
//! 范围（1.2）：
//! - [`ButtonState`]：normal / focused / pressed / disabled，状态进组件。
//! - [`navigate`]：可聚焦按钮组成焦点环，按布局相邻关系（方向 + 矩形几何）移动焦点。
//! - [`press_scale`] / [`press_alpha`]：按下那几 tick 的 scale + modulate 反馈，**解析式**
//!   （第 t tick 的值一步算出，不累加）——快照回退续播逐位一致，和 Tween 同一条纪律。

use serde_json::Value;
use vitric_ecs::{EntityId, World};

use crate::ui::UiRect;

/// 按钮状态机（v1 不做 hover，见合同第四节）。状态进 `Button.state` 组件字段。
/// - `Normal`：默认态。
/// - `Focused`：被焦点选中（高亮）。同一棵 UI 树同时只有一个按钮 focused。
/// - `Pressed`：激活那几 tick 的反馈态（按下缩放 + 染色），计时到点回 focused/normal。
/// - `Disabled`：不可聚焦、不响应点击/确认。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    Focused,
    Pressed,
    Disabled,
}

/// 全部合法按钮状态名（check 校验 + 错误提示 + 序列化用，和 [`ButtonState::parse`] 一一对应）。
pub const BUTTON_STATES: &[&str] = &["normal", "focused", "pressed", "disabled"];

impl ButtonState {
    /// 状态名 → 状态。未知名返回 `None`（调用方报错带合法名清单）。
    pub fn parse(s: &str) -> Option<ButtonState> {
        Some(match s {
            "normal" => ButtonState::Normal,
            "focused" => ButtonState::Focused,
            "pressed" => ButtonState::Pressed,
            "disabled" => ButtonState::Disabled,
            _ => return None,
        })
    }

    /// 状态 → 名字（写回组件用）。
    pub fn name(self) -> &'static str {
        match self {
            ButtonState::Normal => "normal",
            ButtonState::Focused => "focused",
            ButtonState::Pressed => "pressed",
            ButtonState::Disabled => "disabled",
        }
    }
}

/// 焦点导航方向（`ui-up/down/left/right` 标准 input 注入映射到这里）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    /// 把标准 input action 名（去掉 `ui-` 前缀的 up/down/left/right）解析成方向。
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

/// 一个可聚焦控件的几何（焦点导航只看矩形中心 + 边，不关心是哪个实体——实体身份
/// 由调用方按下标对回去）。矩形是布局解算出的参照系像素矩形（rx/ry/rw/rh）。
#[derive(Clone, Copy, Debug)]
pub struct Focusable {
    pub rect: UiRect,
}

impl Focusable {
    fn center(&self) -> (f64, f64) {
        (self.rect.x + self.rect.w / 2.0, self.rect.y + self.rect.h / 2.0)
    }
}

/// 焦点导航：从 `current`（焦点环里的下标）按 `dir` 方向找相邻可聚焦控件，返回它的下标。
///
/// 相邻判定（确定性纯几何，对标 Godot 的方向焦点）：在 `dir` 半边的候选里，挑
/// **主轴最近**（同方向投影距离最小）的；主轴并列时取**交叉轴偏移最小**的；再并列
/// 取下标小的（query 槽位序，稳定）。半边里没有候选 = 焦点不动（返回 `current`，到边停住，
/// 不环绕——确定且不会"焦点凭空跳到对面"）。
///
/// O(可聚焦控件数)：只扫一遍候选，不全表扫描（合同第六节第 5 条）。
pub fn navigate(items: &[Focusable], current: usize, dir: Dir) -> usize {
    if items.is_empty() || current >= items.len() {
        return current;
    }
    let (cx, cy) = items[current].center();
    let mut best: Option<(usize, f64, f64)> = None; // (下标, 主轴距, 交叉轴距)
    for (i, item) in items.iter().enumerate() {
        if i == current {
            continue;
        }
        let (ix, iy) = item.center();
        // 方向半边过滤 + 主轴/交叉轴分量（屏幕系 y 向下：Up = y 更小）
        let (main, cross) = match dir {
            Dir::Up => (cy - iy, (ix - cx).abs()),
            Dir::Down => (iy - cy, (ix - cx).abs()),
            Dir::Left => (cx - ix, (iy - cy).abs()),
            Dir::Right => (ix - cx, (iy - cy).abs()),
        };
        if main <= 0.0 {
            continue; // 不在 dir 半边（含同心，正交项不算相邻）
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

/// 按下反馈持续 tick 数（pressed 态保持几 tick 再落回）。短促一闪，纯反馈不挡交互。
pub const PRESS_TICKS: u64 = 6;

/// 按下反馈缩放系数：第 `t` tick（0..PRESS_TICKS）的 scale。**解析式**——一个对称的
/// 三角包络（按下先缩到 `min_scale` 再弹回 1.0），`min` 在中点。t 超出区间 = 1.0（无反馈）。
/// 纯函数：同 t 同值，不依赖上一帧（快照回退续播一致，禁累加，和 Tween 同纪律）。
///
/// `min_scale`：按到最深时的缩放（如 0.92 = 缩到 92%）。PetClaw 经验：按下=缩+染色。
pub fn press_scale(t: u64, min_scale: f64) -> f64 {
    if t >= PRESS_TICKS {
        return 1.0;
    }
    // 三角包络：0→中点 线性降到 min，中点→末 线性升回 1。半程 = PRESS_TICKS/2。
    let half = PRESS_TICKS as f64 / 2.0;
    let tf = t as f64;
    let depth = if tf <= half { tf / half } else { (PRESS_TICKS as f64 - tf) / half };
    // depth ∈ [0,1]，0=未按(scale 1) 1=最深(scale min)
    1.0 - (1.0 - min_scale) * depth
}

/// 按下反馈染色强度：第 `t` tick 的 modulate 系数（0=原色，1=最亮/最暗的染色峰值）。
/// 和 [`press_scale`] 同一个三角包络——缩到最深时染色也最浓。渲染按它在原色和
/// 高亮色之间插值。纯函数，禁累加。
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

/// 一个 UI 节点的**按下反馈绘制参数**：CPU/GPU 两路共用这一份（公式逐句同构）。
/// 挂了 `Button` 且 `press_t ≥ 0` 时，按 [`press_scale`]/[`press_modulate`] 算出
/// **绕中心缩放后的矩形 + 提亮系数**；否则原样矩形 + 0 提亮。纯函数（输入 = 组件里的
/// `press_t`/`min_scale` + 布局矩形），不碰布局/模拟 RNG——渲染装饰纪律，重放/快照一致。
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

/// 提亮 RGB（modulate ∈ [0,1]，0=原色，1=全白）。按下反馈染色：往白色线性插值。
/// alpha 不动。CPU/GPU 两路共用。
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
        // 三个按钮竖排（VBox 同款）：y = 0,100,200，等宽同 x。
        let items = vec![fo(0.0, 0.0, 50.0, 40.0), fo(0.0, 100.0, 50.0, 40.0), fo(0.0, 200.0, 50.0, 40.0)];
        // 从 0 往下 → 1，再往下 → 2
        assert_eq!(navigate(&items, 0, Dir::Down), 1);
        assert_eq!(navigate(&items, 1, Dir::Down), 2);
        // 到底再往下 = 不动（不环绕）
        assert_eq!(navigate(&items, 2, Dir::Down), 2);
        // 往上对称
        assert_eq!(navigate(&items, 2, Dir::Up), 1);
        assert_eq!(navigate(&items, 0, Dir::Up), 0, "到顶不动");
        // 左右在纯竖排里没有候选 = 不动
        assert_eq!(navigate(&items, 1, Dir::Left), 1);
        assert_eq!(navigate(&items, 1, Dir::Right), 1);
    }

    #[test]
    fn picks_nearest_in_main_axis_then_least_cross_offset() {
        // 当前在 (0,0)。下方两个候选：正下方稍远 (0,200)、右下方近一点 (50,100)。
        // Down 半边都满足；主轴距 100 < 200，所以选 (50,100) 那个（下标 1）。
        let items = vec![fo(0.0, 0.0, 10.0, 10.0), fo(50.0, 100.0, 10.0, 10.0), fo(0.0, 200.0, 10.0, 10.0)];
        assert_eq!(navigate(&items, 0, Dir::Down), 1, "主轴更近的优先");
        // 主轴并列时取交叉轴偏移更小的
        let items2 = vec![fo(0.0, 0.0, 10.0, 10.0), fo(80.0, 100.0, 10.0, 10.0), fo(10.0, 100.0, 10.0, 10.0)];
        assert_eq!(navigate(&items2, 0, Dir::Down), 2, "同主轴距取交叉轴更近(下标2 x=10)");
    }

    #[test]
    fn horizontal_grid_navigation() {
        // 2x2 网格：(0,0)(100,0) / (0,100)(100,100)
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
        // 半程 = 3。t=0 无缩(1.0)；t=3 最深(min)；t=6 回 1.0；t≥6 无反馈。
        let min = 0.9;
        assert_eq!(press_scale(0, min), 1.0);
        assert!((press_scale(3, min) - min).abs() < 1e-12, "中点 = min");
        assert!((press_scale(1, min) - (1.0 - 0.1 * (1.0 / 3.0))).abs() < 1e-12);
        assert!((press_scale(5, min) - (1.0 - 0.1 * (1.0 / 3.0))).abs() < 1e-12, "对称：t=5 与 t=1 相等");
        assert_eq!(press_scale(6, min), 1.0);
        assert_eq!(press_scale(99, min), 1.0, "超区间无反馈");
        // 纯函数：同 t 同值，反复调一致（不依赖任何外部状态）
        assert_eq!(press_scale(2, min), press_scale(2, min));
    }

    #[test]
    fn press_modulate_envelope_matches_scale_shape() {
        assert_eq!(press_modulate(0), 0.0);
        assert!((press_modulate(3) - 1.0).abs() < 1e-12, "中点染色最浓");
        assert_eq!(press_modulate(6), 0.0);
        assert_eq!(press_modulate(7), 0.0);
        // 对称
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
