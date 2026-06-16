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

use serde_json::{json, Value};

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
    }
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
    /// `{"direction","distance","same_row","same_col","adjacent"}`。
    pub fn to_json(self) -> Value {
        json!({
            "direction": self.direction.as_str(),
            "distance": self.distance,
            "same_row": self.same_row,
            "same_col": self.same_col,
            "adjacent": self.adjacent,
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
    }
}
