//! 主题 Theme（1.2）——UI 控件的样式卷：一处定义、全局引用、单控件可覆盖。
//!
//! 定位（和 schema/序列同原则）：引擎给通用控件原语，Theme 是把"颜色/字号/边距/按钮各
//! 状态样式"这些散落参数收成一份可换肤的声明。换肤 = 换一份 Theme 引用，不动场景结构。
//! `themes/<名>.json`，清单 `themes` 列表声明，运行时控件按名字引用（check 校验名存在）。
//!
//! Theme 是**纯静态数据**：不进世界状态/哈希/录像（它是装配期常量，和 schema、动画片段
//! 同级——定义在文件里，运行时只读不写）。控件按状态从 Theme 取颜色/尺寸，取出来的值
//! 进渲染，控件自身的状态（focused/pressed）才进组件。
//!
//! 格式：
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
//! `colors` 是全局颜色卷（兜底/通用引用），`button.<state>` 是按钮四态的背景/文字色。
//! 缺省：未声明的 button 状态从 `colors` 推（focused→focus 色、disabled→disabled 色，
//! normal/pressed→bg），让最小 Theme（只写 colors）也能用。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::ValidationReport;

/// 按钮一个状态的样式（背景 + 文字色，都是 `#rrggbb`/`#rrggbbaa` 字面）。
#[derive(Debug, Clone, PartialEq)]
pub struct ButtonStyle {
    pub bg: String,
    pub text: String,
}

/// 一份主题。装配期常量，不进世界状态。
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// 主题名（取文件名，去掉 `themes/` 前缀和 `.json` 后缀）。
    pub name: String,
    /// 全局颜色卷：bg / text / focus / disabled（缺省给一组中性值）。
    pub colors: BTreeMap<String, String>,
    /// 默认字号（像素）。
    pub font_size: f64,
    /// 控件默认内边距（像素）。
    pub padding: f64,
    /// 控件默认外边距（像素，容器间距用）。
    pub margin: f64,
    /// 按钮四态样式（normal/focused/pressed/disabled 全填，解析时已按 colors 补全）。
    pub button: BTreeMap<String, ButtonStyle>,
}

/// 按钮状态名（Theme.button 的键，和 vitric-render 的 ButtonState 同步）。
const STATES: &[&str] = &["normal", "focused", "pressed", "disabled"];

impl Theme {
    /// 取某个按钮状态的样式（state 必是 STATES 之一，解析时已补全，直接取）。
    pub fn button_style(&self, state: &str) -> Option<&ButtonStyle> {
        self.button.get(state)
    }

    /// 解析一份 Theme 文档。结构/颜色格式问题推进 report（带路径 + VDxxx + 提示），
    /// 一次报全。`name` 是主题名，`tpath` 是文件路径前缀（错误路径用）。
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

        // 颜色卷：每个值校验成合法 #rrggbb(aa)
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
        // 缺省颜色（让只写一部分的 Theme 也能取到 bg/text/focus/disabled）
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

        // 按钮各状态样式：声明了就校验，没声明从 colors 推（最小 Theme 也可用）
        let mut button = BTreeMap::new();
        let button_obj = obj.get("button").and_then(|v| v.as_object());
        if let Some(bobj) = button_obj {
            // 报告未知状态键（拼错 state 名静默吞掉 = 后门）
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
            // 该状态的缺省底色：focused→focus、disabled→disabled、其余→bg
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

    /// 一份中性兜底主题（解析硬失败时返回，避免后续取色 panic；错误已进 report）。
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

/// 读一个非负数值字段（缺省给 default，负数/非数字报 VD082）。
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

/// 读按钮某状态的某色字段（缺/没声明 = fallback；声明了非法颜色报 VD081）。
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

/// `#rrggbb` 或 `#rrggbbaa` 校验（和渲染层 parse_color_a 同口径）。
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
        // 只给 colors，button 四态从 colors 推全
        let (t, errs) = parse(json!({"colors": {"bg": "#111111", "focus": "#2222ff"}}));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(t.button_style("normal").unwrap().bg, "#111111", "normal 用 bg");
        assert_eq!(t.button_style("focused").unwrap().bg, "#2222ff", "focused 用 focus 色");
        assert_eq!(t.button_style("disabled").unwrap().bg, "#555555", "disabled 用缺省 disabled 色");
        // 四态全在
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
