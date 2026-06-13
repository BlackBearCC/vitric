//! UI 控件的**值级**校验（在通用 schema 校验之上的语义约束）。
//!
//! 为什么单独一条：schema 能校验字段类型/范围，但 UI 有几条跨字段的语义约束——
//! 锚点预设名必须在合法集合里、容器类型在 {VBox,HBox,Grid}、Grid 列数 ≥ 1、对齐名
//! 合法。这些和序列的动作名校验同性质（[`crate::sequence`]），由引擎兜底，不依赖
//! 作者把字段声明成 enum 才生效——UI 是引擎给的通用控件，约束是引擎的。
//!
//! 跨文件引用（Panel.image / 字体存在性）不在这里：那要素材仓库，和 Sprite.image
//! 一样在 `vitric check`（cli）里查。这里只看场景内的组件值。

use serde_json::Value;

use crate::ValidationReport;

/// 合法锚点预设名（与 vitric-render 的 ANCHOR_NAMES 同步——引擎只此一份语义，
/// 这里复制一份纯字符串常量，避免 vitric-data 依赖 vitric-render（数据层不依赖渲染））。
pub const UI_ANCHORS: &[&str] = &[
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

/// 合法容器类型名。
pub const UI_CONTAINER_KINDS: &[&str] = &["VBox", "HBox", "Grid"];

/// 合法对齐名。
pub const UI_ALIGNS: &[&str] = &["start", "center", "end"];

/// 合法按钮状态名（与 vitric-render 的 ButtonState 同步——同样复制纯字符串常量，
/// 避免 vitric-data 依赖 vitric-render）。
pub const UI_BUTTON_STATES: &[&str] = &["normal", "focused", "pressed", "disabled"];

/// 校验一个实体上的 UI 组件值。`comps` 是该实体归一化后的组件 → 值映射。
/// `epath` 是实体路径前缀（如 `scenes/main.json#/entities/2`）。
/// 把发现的问题推进 `report`（带路径 + VDxxx 码 + 修复提示），一次报全。
pub fn validate_ui_components(
    comps: &serde_json::Map<String, Value>,
    epath: &str,
    report: &mut ValidationReport,
) {
    if let Some(ui) = comps.get("Ui").and_then(|v| v.as_object()) {
        // 锚点预设名（manual 缺省也合法；给了非法名报错）
        if let Some(anchor) = ui.get("anchor").and_then(|v| v.as_str()) {
            if !UI_ANCHORS.contains(&anchor) {
                report.push(
                    "VD070",
                    format!("{epath}/components/Ui/anchor"),
                    format!("锚点预设 {anchor:?} 不合法"),
                    format!("可选: [{}]", UI_ANCHORS.join(", ")),
                );
            }
        }
    }
    if let Some(c) = comps.get("Container").and_then(|v| v.as_object()) {
        // 容器类型
        let kind = c.get("kind").and_then(|v| v.as_str());
        match kind {
            Some(k) if UI_CONTAINER_KINDS.contains(&k) => {
                // Grid 列数 ≥ 1
                if k == "Grid" {
                    let cols = c.get("columns").and_then(Value::as_f64).unwrap_or(1.0);
                    if cols < 1.0 {
                        report.push(
                            "VD072",
                            format!("{epath}/components/Container/columns"),
                            format!("Grid 列数必须 ≥ 1，拿到 {cols}"),
                            "网格至少要有 1 列（VBox/HBox 不用 columns）",
                        );
                    }
                }
            }
            Some(other) => report.push(
                "VD071",
                format!("{epath}/components/Container/kind"),
                format!("容器类型 {other:?} 不认识"),
                format!("可选: [{}]", UI_CONTAINER_KINDS.join(", ")),
            ),
            None => report.push(
                "VD071",
                format!("{epath}/components/Container/kind"),
                "Container 缺少 kind 字段",
                format!("可选: [{}]", UI_CONTAINER_KINDS.join(", ")),
            ),
        }
        // 对齐名（main/cross，给了就要合法）
        for field in ["main", "cross"] {
            if let Some(a) = c.get(field).and_then(|v| v.as_str()) {
                if !UI_ALIGNS.contains(&a) {
                    report.push(
                        "VD073",
                        format!("{epath}/components/Container/{field}"),
                        format!("对齐 {a:?} 不合法"),
                        format!("可选: [{}]", UI_ALIGNS.join(", ")),
                    );
                }
            }
        }
    }
    // Button（1.2 交互）：状态名合法 + 激活 action 非空。
    // theme 名是否存在要项目级主题表，归 vitric check（和 Panel.image 同口径）。
    if let Some(b) = comps.get("Button").and_then(|v| v.as_object()) {
        if let Some(state) = b.get("state").and_then(|v| v.as_str()) {
            if !UI_BUTTON_STATES.contains(&state) {
                report.push(
                    "VD074",
                    format!("{epath}/components/Button/state"),
                    format!("按钮状态 {state:?} 不合法"),
                    format!("可选: [{}]（不做 hover，见合同第四节）", UI_BUTTON_STATES.join(", ")),
                );
            }
        }
        // action 给了就不能是空串（空 action 的 ui-activate 没有规则能接，是死按钮）
        if let Some(action) = b.get("action").and_then(|v| v.as_str()) {
            if action.is_empty() {
                report.push(
                    "VD075",
                    format!("{epath}/components/Button/action"),
                    "按钮 action 是空串——激活发 ui-activate 时 action 为空，没有规则能接",
                    "填一个 action 名（如 \"start\"），规则按 {\"event\":\"ui-activate\",\"filter\":{\"action\":\"start\"}} 接",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(comps: Value) -> Vec<String> {
        let mut report = ValidationReport::default();
        validate_ui_components(comps.as_object().unwrap(), "scenes/main.json#/entities/0", &mut report);
        report.errors.iter().map(|e| format!("{} {}", e.code, e.path)).collect()
    }

    #[test]
    fn bad_anchor_reported() {
        let errs = check(json!({"Ui": {"anchor": "top-middle"}}));
        assert!(errs.iter().any(|e| e.starts_with("VD070") && e.contains("Ui/anchor")), "{errs:?}");
    }

    #[test]
    fn good_anchor_passes() {
        assert!(check(json!({"Ui": {"anchor": "center"}})).is_empty());
        assert!(check(json!({"Ui": {"anchor": "stretch"}})).is_empty());
    }

    #[test]
    fn bad_container_kind_reported() {
        let errs = check(json!({"Container": {"kind": "Flex"}}));
        assert!(errs.iter().any(|e| e.starts_with("VD071") && e.contains("Container/kind")), "{errs:?}");
    }

    #[test]
    fn grid_zero_columns_reported() {
        let errs = check(json!({"Container": {"kind": "Grid", "columns": 0}}));
        assert!(errs.iter().any(|e| e.starts_with("VD072") && e.contains("columns")), "{errs:?}");
    }

    #[test]
    fn grid_one_column_passes() {
        assert!(check(json!({"Container": {"kind": "Grid", "columns": 1}})).is_empty());
    }

    #[test]
    fn bad_align_reported() {
        let errs = check(json!({"Container": {"kind": "VBox", "main": "middle"}}));
        assert!(errs.iter().any(|e| e.starts_with("VD073") && e.contains("main")), "{errs:?}");
    }

    #[test]
    fn bad_button_state_reported() {
        let errs = check(json!({"Button": {"state": "hover", "action": "start"}}));
        assert!(errs.iter().any(|e| e.starts_with("VD074") && e.contains("Button/state")), "{errs:?}");
    }

    #[test]
    fn good_button_state_passes() {
        for s in UI_BUTTON_STATES {
            assert!(check(json!({"Button": {"state": s, "action": "go"}})).is_empty(), "{s} 该合法");
        }
    }

    #[test]
    fn empty_button_action_reported() {
        let errs = check(json!({"Button": {"state": "normal", "action": ""}}));
        assert!(errs.iter().any(|e| e.starts_with("VD075") && e.contains("Button/action")), "{errs:?}");
    }
}
