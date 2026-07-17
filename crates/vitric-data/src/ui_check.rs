//! **Value-level** validation of UI components (semantic constraints on top of generic schema validation).
//!
//! Why a separate pass: schema can validate field types/ranges, but UI has a few cross-field semantic constraints —
//! the anchor preset name must be in the legal set, container kind is in {VBox,HBox,Grid}, Grid columns >= 1, and the alignment name
//! is legal. These are the same nature as the sequence action-name validation ([`crate::sequence`]); the engine enforces them as a backstop, without relying on
//! the author declaring fields as enums to take effect — UI is a generic control provided by the engine, and the constraints are the engine's.
//!
//! Cross-file references (Panel.image / font existence) are not here: those need the asset repository, and like Sprite.image
//! they are checked in `vitric check` (cli). Here we only look at component values within the scene.

use serde_json::Value;

use crate::ValidationReport;

/// Legal anchor preset names (kept in sync with vitric-render's ANCHOR_NAMES — the engine has only this one set of semantics,
/// duplicated here as pure string constants to avoid vitric-data depending on vitric-render (the data layer does not depend on rendering)).
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

/// Legal container kind names.
pub const UI_CONTAINER_KINDS: &[&str] = &["VBox", "HBox", "Grid"];

/// Legal alignment names.
pub const UI_ALIGNS: &[&str] = &["start", "center", "end"];

/// Legal button state names (kept in sync with vitric-render's ButtonState — again duplicated as pure string constants,
/// to avoid vitric-data depending on vitric-render).
pub const UI_BUTTON_STATES: &[&str] = &["normal", "focused", "pressed", "disabled"];

/// Validate the UI component values on an entity. `comps` is the entity's normalized component -> value map.
/// `epath` is the entity path prefix (e.g. `scenes/main.json#/entities/2`).
/// Pushes any problems found into `report` (with path + VDxxx code + fix hint), reporting all at once.
pub fn validate_ui_components(
    comps: &serde_json::Map<String, Value>,
    epath: &str,
    report: &mut ValidationReport,
) {
    if let Some(ui) = comps.get("Ui").and_then(|v| v.as_object()) {
        // Anchor preset name (manual default is also legal; an illegal name is reported)
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
        // Container kind
        let kind = c.get("kind").and_then(|v| v.as_str());
        match kind {
            Some(k) if UI_CONTAINER_KINDS.contains(&k) => {
                // Grid columns >= 1
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
        // Alignment names (main/cross; if given, must be legal)
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
    // Button (1.2 interaction): state name is legal + active action is non-empty.
    // Whether the theme name exists needs the project-level theme table, handled by vitric check (same scope as Panel.image).
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
        // action, if given, must not be an empty string (an empty action's ui-activate has no rule to receive it; it's a dead button)
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
