//! Theme (1.2) — the style roll of UI controls: defined once, referenced globally, overridable per control.
//!
//! Positioning (same principle as schema/sequence): the engine provides generic control primitives, and Theme gathers scattered parameters like "colors / font size / margins / per-state button
//! styles" into a swappable declarative definition. Skinning = swapping a Theme reference, without touching scene structure.
//! `themes/<name>.json`, declared in the manifest's `themes` list, controls reference by name at runtime (check validates name existence).
//!
//! Theme is **purely static data**: it does not enter world state / hash / recordings (it's an assembly-time constant, same level as schema, animation clips
//! — defined in files, read-only at runtime). Controls fetch colors/sizes from Theme by state; the fetched values
//! go into rendering, while the control's own state (focused/pressed) goes into components.
//!
//! Format:
//! ```json
//! {
//!   "colors": {"bg": "#1b1d26", "text": "#f0f0f0", "focus": "#5a7bb5", "disabled": "#555555"},
//!   "font_size": 30, "padding": 12, "margin": 8,
//!   "button": {
//!     "normal":   {"bg": "#3a4a6b", "text": "#ffffff"},
//!     "focused":  {"bg": "#5a7bb5", "text": "#ffffff"},
//!     "pressed":  {"bg": "#8fb0e8", "text": "#ffffff"},
//!     "disabled": {"bg": "#2a2d36", "text": "#777777"}
//!   }
//! }
//! ```
//! `colors` is the global color roll (fallback / generic reference), `button.<state>` is the background/text color for the four button states.
//! Default: undeclared button states are derived from `colors` (focused -> focus color, disabled -> disabled color,
//! normal/pressed -> bg), so that a minimal Theme (only writing colors) still works.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::ValidationReport;

/// Style of one button state (background + text color, both `#rrggbb`/`#rrggbbaa` literals).
#[derive(Debug, Clone, PartialEq)]
pub struct ButtonStyle {
    pub bg: String,
    pub text: String,
}

/// A theme. An assembly-time constant; does not enter world state.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Theme name (taken from the file name, stripping the `themes/` prefix and `.json` suffix).
    pub name: String,
    /// Global color roll: bg / text / focus / disabled (a set of neutral values is given as default).
    pub colors: BTreeMap<String, String>,
    /// Default font size (pixels).
    pub font_size: f64,
    /// Default inner padding of controls (pixels).
    pub padding: f64,
    /// Default outer margin of controls (pixels, also used for container spacing).
    pub margin: f64,
    /// Styles for the four button states (normal/focused/pressed/disabled all filled in; already completed from colors at parse time).
    pub button: BTreeMap<String, ButtonStyle>,
}

/// Button state names (keys of Theme.button, kept in sync with vitric-render's ButtonState).
const STATES: &[&str] = &["normal", "focused", "pressed", "disabled"];

impl Theme {
    /// Fetch the style of a button state (state must be one of STATES; already completed at parse time, fetched directly).
    pub fn button_style(&self, state: &str) -> Option<&ButtonStyle> {
        self.button.get(state)
    }

    /// Parse a Theme document. Structural / color format problems are pushed into report (with path + VDxxx + hint),
    /// reporting all at once. `name` is the theme name, `tpath` is the file path prefix (used in error paths).
    pub fn parse(doc: &Value, name: &str, tpath: &str, report: &mut ValidationReport) -> Theme {
        let obj = match doc.as_object() {
            Some(o) => o,
            None => {
                report.push(
                    "VD080",
                    tpath.to_string(),
                    "主题文件顶层必须是 JSON 对象",
                    "写法: {\"colors\": {...}, \"font_size\": 30, \"button\": {...}}",
                );
                return Theme::neutral(name);
            }
        };

        // Color roll: each value is validated as a legal #rrggbb(aa)
        let mut colors = BTreeMap::new();
        if let Some(cobj) = obj.get("colors").and_then(|v| v.as_object()) {
            for (key, v) in cobj {
                match v.as_str() {
                    Some(s) if is_hex_color(s) => {
                        colors.insert(key.clone(), s.to_string());
                    }
                    Some(s) => report.push(
                        "VD081",
                        format!("{tpath}#/colors/{key}"),
                        format!("颜色 {s:?} 不是合法十六进制颜色"),
                        "写法: \"#rrggbb\" 或带透明度 \"#rrggbbaa\"",
                    ),
                    None => report.push(
                        "VD081",
                        format!("{tpath}#/colors/{key}"),
                        "颜色值必须是字符串",
                        "写法: \"#rrggbb\"",
                    ),
                }
            }
        }
        // Default colors (so a Theme that only writes part of them can still get bg/text/focus/disabled)
        let default_color = |k: &str| match k {
            "bg" => "#1b1d26",
            "text" => "#f0f0f0",
            "focus" => "#5a7bb5",
            "disabled" => "#555555",
            _ => "#ffffff",
        };
        for k in ["bg", "text", "focus", "disabled"] {
            colors.entry(k.to_string()).or_insert_with(|| default_color(k).to_string());
        }

        let font_size = read_size(obj, "font_size", 28.0, tpath, report);
        let padding = read_size(obj, "padding", 8.0, tpath, report);
        let margin = read_size(obj, "margin", 8.0, tpath, report);

        // Per-state button styles: if declared, validate; if not, derive from colors (minimal Theme also works)
        let mut button = BTreeMap::new();
        let button_obj = obj.get("button").and_then(|v| v.as_object());
        if let Some(bobj) = button_obj {
            // Report unknown state keys (silently swallowing a misspelled state name = a backdoor)
            for key in bobj.keys() {
                if !STATES.contains(&key.as_str()) {
                    report.push(
                        "VD083",
                        format!("{tpath}#/button/{key}"),
                        format!("按钮状态 {key:?} 不认识"),
                        format!("可选: [{}]", STATES.join(", ")),
                    );
                }
            }
        }
        for &state in STATES {
            let declared = button_obj.and_then(|b| b.get(state)).and_then(|v| v.as_object());
            // Default background color for this state: focused -> focus, disabled -> disabled, others -> bg
            let fallback_bg = match state {
                "focused" => colors["focus"].clone(),
                "disabled" => colors["disabled"].clone(),
                _ => colors["bg"].clone(),
            };
            let fallback_text = colors["text"].clone();
            let bg = read_button_color(declared, "bg", fallback_bg, state, tpath, report);
            let text = read_button_color(declared, "text", fallback_text, state, tpath, report);
            button.insert(state.to_string(), ButtonStyle { bg, text });
        }

        Theme { name: name.to_string(), colors, font_size, padding, margin, button }
    }

    /// A neutral fallback theme (returned when parsing hard-fails, to avoid subsequent color-fetch panics; errors already in report).
    fn neutral(name: &str) -> Theme {
        let mut colors = BTreeMap::new();
        for (k, v) in [("bg", "#1b1d26"), ("text", "#f0f0f0"), ("focus", "#5a7bb5"), ("disabled", "#555555")] {
            colors.insert(k.to_string(), v.to_string());
        }
        let mut button = BTreeMap::new();
        for &state in STATES {
            let bg = match state {
                "focused" => colors["focus"].clone(),
                "disabled" => colors["disabled"].clone(),
                _ => colors["bg"].clone(),
            };
            button.insert(state.to_string(), ButtonStyle { bg, text: colors["text"].clone() });
        }
        Theme { name: name.to_string(), colors, font_size: 28.0, padding: 8.0, margin: 8.0, button }
    }
}

/// Read a non-negative numeric field (default given if absent; negative / non-number reports VD082).
fn read_size(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    default: f64,
    tpath: &str,
    report: &mut ValidationReport,
) -> f64 {
    match obj.get(key) {
        None => default,
        Some(v) => match v.as_f64() {
            Some(n) if n >= 0.0 => n,
            Some(n) => {
                report.push(
                    "VD082",
                    format!("{tpath}#/{key}"),
                    format!("{key} 必须 ≥ 0，拿到 {n}"),
                    "字号/边距是非负像素值",
                );
                default
            }
            None => {
                report.push(
                    "VD082",
                    format!("{tpath}#/{key}"),
                    format!("{key} 必须是数字"),
                    "字号/边距是非负像素值",
                );
                default
            }
        },
    }
}

/// Read a color field of a button state (missing / undeclared = fallback; a declared illegal color reports VD081).
fn read_button_color(
    declared: Option<&serde_json::Map<String, Value>>,
    field: &str,
    fallback: String,
    state: &str,
    tpath: &str,
    report: &mut ValidationReport,
) -> String {
    match declared.and_then(|d| d.get(field)).and_then(|v| v.as_str()) {
        Some(s) if is_hex_color(s) => s.to_string(),
        Some(s) => {
            report.push(
                "VD081",
                format!("{tpath}#/button/{state}/{field}"),
                format!("颜色 {s:?} 不是合法十六进制颜色"),
                "写法: \"#rrggbb\" 或 \"#rrggbbaa\"",
            );
            fallback
        }
        None => fallback,
    }
}

/// Validate `#rrggbb` or `#rrggbbaa` (same scope as the render layer's parse_color_a).
fn is_hex_color(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else { return false };
    (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(doc: Value) -> (Theme, Vec<String>) {
        let mut report = ValidationReport::default();
        let theme = Theme::parse(&doc, "dark", "themes/dark.json", &mut report);
        let errs = report.errors.iter().map(|e| format!("{} {}", e.code, e.path)).collect();
        (theme, errs)
    }

    #[test]
    fn full_theme_parses_all_fields() {
        let (t, errs) = parse(json!({
            "colors": {"bg": "#1b1d26", "text": "#f0f0f0", "focus": "#5a7bb5", "disabled": "#555555"},
            "font_size": 30, "padding": 12, "margin": 8,
            "button": {
                "normal": {"bg": "#3a4a6b", "text": "#ffffff"},
                "focused": {"bg": "#5a7bb5", "text": "#ffffff"},
                "pressed": {"bg": "#8fb0e8", "text": "#ffffff"},
                "disabled": {"bg": "#2a2d36", "text": "#777777"}
            }
        }));
        assert!(errs.is_empty(), "完整合法主题不该报错: {errs:?}");
        assert_eq!(t.font_size, 30.0);
        assert_eq!(t.padding, 12.0);
        assert_eq!(t.button_style("focused").unwrap().bg, "#5a7bb5");
        assert_eq!(t.button_style("pressed").unwrap().bg, "#8fb0e8");
        assert_eq!(t.button_style("disabled").unwrap().text, "#777777");
    }

    #[test]
    fn minimal_theme_fills_button_states_from_colors() {
        // Only colors given; button's four states are derived from colors
        let (t, errs) = parse(json!({"colors": {"bg": "#111111", "focus": "#2222ff"}}));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(t.button_style("normal").unwrap().bg, "#111111", "normal 用 bg");
        assert_eq!(t.button_style("focused").unwrap().bg, "#2222ff", "focused 用 focus 色");
        assert_eq!(t.button_style("disabled").unwrap().bg, "#555555", "disabled 用缺省 disabled 色");
        // All four states present
        for s in STATES {
            assert!(t.button_style(s).is_some(), "{s} 必须补全");
        }
    }

    #[test]
    fn bad_color_reported_with_path() {
        let (_, errs) = parse(json!({"colors": {"bg": "red"}}));
        assert!(errs.iter().any(|e| e.starts_with("VD081") && e.contains("colors/bg")), "{errs:?}");
    }

    #[test]
    fn bad_button_state_color_reported() {
        let (_, errs) = parse(json!({"button": {"normal": {"bg": "not-a-color"}}}));
        assert!(errs.iter().any(|e| e.starts_with("VD081") && e.contains("button/normal/bg")), "{errs:?}");
    }

    #[test]
    fn unknown_button_state_reported() {
        let (_, errs) = parse(json!({"button": {"hover": {"bg": "#ffffff"}}}));
        assert!(errs.iter().any(|e| e.starts_with("VD083") && e.contains("button/hover")), "{errs:?}");
    }

    #[test]
    fn negative_size_reported() {
        let (_, errs) = parse(json!({"font_size": -5}));
        assert!(errs.iter().any(|e| e.starts_with("VD082") && e.contains("font_size")), "{errs:?}");
    }

    #[test]
    fn non_object_top_level_reported() {
        let (_, errs) = parse(json!([1, 2, 3]));
        assert!(errs.iter().any(|e| e.starts_with("VD080")), "{errs:?}");
    }
}
