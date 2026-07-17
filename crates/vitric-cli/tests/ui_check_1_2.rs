//! vitric check red (UI interaction 1.2): bad Theme reference / focus referencing a non-existent entity / illegal state style
//! / empty button action. Errors carry path + VDxxx code + fix hint, all reported at once.

use std::path::PathBuf;

/// Write a minimal 1.2 UI project (schema with Button/UiRoot.focus + optional theme files + scene).
/// `theme_files`: list of (filename, content) written into themes/ and attached to the manifest.
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
    // schema: state is deliberately declared as text (not enum) to prove the engine falls back to validating button state semantics
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
    // Button references a theme not in the manifest → red, naming the missing theme + the defined list
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
    // state declared as text, given the illegal value "hover" → the engine falls back to reporting VD074
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
    // UiRoot.focus is an entity-reference type (schema-declared), pointing at a non-existent entity → red (VD033).
    // Here focus is declared as entity type, proving the focus reference goes through the same existence check as any other entity reference.
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
    // Illegal color in a theme file → red during check (Theme::parse runs in Project::load)
    let dir = make_project(
        "badcolor",
        r#"{"name":"ui","components":{"UiRoot":{}}}"#,
        &[("dark.json", r##"{"colors":{"bg":"not-a-color"}}"##)],
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("坏主题颜色必须红灯");
    assert!(err.contains("VD081") && err.contains("colors/bg"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}
