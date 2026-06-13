//! vitric check 红灯（UI 交互 1.2）：坏 Theme 引用 / 焦点引用不存在实体 / 非法状态样式
//! / 空按钮 action。错误带路径 + VDxxx 码 + 修复提示，一次报全。

use std::path::PathBuf;

/// 写一个最小 1.2 UI 项目（schema 含 Button/UiRoot.focus + 可选主题文件 + 场景）。
/// `theme_files`: (文件名, 内容) 列表，会写进 themes/ 并挂进清单。
fn make_project(tag: &str, scene_entities: &str, theme_files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-ui12-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::create_dir_all(dir.join("themes")).unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();

    let theme_list: Vec<String> = theme_files.iter().map(|(n, _)| format!("\"themes/{n}\"")).collect();
    std::fs::write(
        dir.join("vitric.json"),
        format!(
            r#"{{"name":"ui12","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"themes":[{}],"seed":1}}"#,
            theme_list.join(",")
        ),
    )
    .unwrap();
    // schema：state 故意声明成 text（不靠 enum），证明引擎兜底校验按钮状态语义
    std::fs::write(
        dir.join("schema.json"),
        r##"{"components":{
            "UiRoot":{"fields":{"focus":{"type":"text","default":""}}},
            "Ui":{"fields":{
                "anchor":{"type":"text","default":"manual"},
                "parent":{"type":"entity"},
                "w":{"type":"number","default":0},"h":{"type":"number","default":0}
            }},
            "Panel":{"fields":{"color":{"type":"text","default":"#ffffff"},"image":{"type":"text","default":""}}},
            "Button":{"fields":{
                "action":{"type":"text","default":""},
                "theme":{"type":"text","default":""},
                "state":{"type":"text","default":"normal"}
            }}
        }}"##,
    )
    .unwrap();
    for (name, content) in theme_files {
        std::fs::write(dir.join("themes").join(name), content).unwrap();
    }
    std::fs::write(dir.join("scenes/main.json"), format!(r#"{{"entities":[{scene_entities}]}}"#)).unwrap();
    dir
}

#[test]
fn check_reports_unknown_theme_reference() {
    // Button 引用了清单里没有的主题 → 红灯，点名缺的主题 + 已定义列表
    let dir = make_project(
        "badtheme",
        r#"{"name":"ui","components":{"UiRoot":{}}},
           {"name":"b","components":{"Ui":{"anchor":"center","parent":"ui","w":100,"h":40},
                                       "Button":{"action":"go","theme":"neon"}}}"#,
        &[("dark.json", r##"{"colors":{"bg":"#111111"}}"##)],
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("坏主题引用必须红灯");
    assert!(err.contains("neon") && err.contains("Button.theme"), "点名缺主题: {err}");
    assert!(err.contains("dark"), "列出已定义主题: {err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_passes_valid_theme_reference() {
    let dir = make_project(
        "goodtheme",
        r#"{"name":"ui","components":{"UiRoot":{}}},
           {"name":"b","components":{"Ui":{"anchor":"center","parent":"ui","w":100,"h":40},
                                       "Button":{"action":"go","theme":"dark"}}}"#,
        &[("dark.json", r##"{"colors":{"bg":"#111111"},"button":{"normal":{"bg":"#222222"}}}"##)],
    );
    vitric_cli::runtime::check(&dir).expect("合法主题引用应通过");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_reports_illegal_button_state() {
    // state 声明成 text、给了非法值 "hover" → 引擎兜底报 VD074
    let dir = make_project(
        "badstate",
        r#"{"name":"ui","components":{"UiRoot":{}}},
           {"name":"b","components":{"Ui":{"anchor":"center","parent":"ui","w":100,"h":40},
                                       "Button":{"action":"go","state":"hover"}}}"#,
        &[],
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("非法按钮状态必须红灯");
    assert!(err.contains("VD074") && err.contains("Button/state"), "{err}");
    assert!(err.contains("hover"), "点名非法状态: {err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_reports_empty_button_action() {
    let dir = make_project(
        "emptyaction",
        r#"{"name":"ui","components":{"UiRoot":{}}},
           {"name":"b","components":{"Ui":{"anchor":"center","parent":"ui","w":100,"h":40},
                                       "Button":{"action":"","state":"normal"}}}"#,
        &[],
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("空 action 必须红灯");
    assert!(err.contains("VD075") && err.contains("Button/action"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_reports_focus_referencing_nonexistent_entity() {
    // UiRoot.focus 是 entity 引用类型（schema 声明），指向不存在的实体 → 红灯（VD033）。
    // 这里把 focus 声明成 entity 类型，证明焦点引用走和别的实体引用同一道存在性校验。
    let dir = std::env::temp_dir().join(format!("vitric-ui12-focusref-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(
        dir.join("vitric.json"),
        r#"{"name":"ui12","schema":"schema.json","entry":"scenes/main.json","scenes":["scenes/main.json"],"seed":1}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("schema.json"),
        r##"{"components":{
            "UiRoot":{"fields":{"focus":{"type":"entity"}}}
        }}"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[{"name":"ui","components":{"UiRoot":{"focus":"ghost-button"}}}]}"#,
    )
    .unwrap();
    let err = vitric_cli::runtime::check(&dir).expect_err("焦点引用不存在实体必须红灯");
    assert!(err.contains("ghost-button"), "点名不存在的焦点实体: {err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_reports_bad_theme_color_with_path() {
    // 主题文件里的颜色非法 → check 期红灯（Theme::parse 在 Project::load 跑）
    let dir = make_project(
        "badcolor",
        r#"{"name":"ui","components":{"UiRoot":{}}}"#,
        &[("dark.json", r##"{"colors":{"bg":"not-a-color"}}"##)],
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("坏主题颜色必须红灯");
    assert!(err.contains("VD081") && err.contains("colors/bg"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}
