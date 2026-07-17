//! UI controls (layout 1.1) end-to-end + system-level: the layout system advances each tick,
//! dirty-flag zero recompute, snapshot/replay continuation consistent, UI does not drift with
//! camera, the gray-box demo is positioned correctly, render actually draws it.
//!
//! Uses examples/ui-gallery, a "main menu gray box" assembled from pure control primitives, to
//! prove: the engine only provides generic controls (Panel/Label/Container/anchor); specific
//! interfaces are assembled by the project from building blocks (non-interactive;
//! interaction is covered in 1.2).

use std::path::{Path, PathBuf};

use serde_json::json;

use vitric_cli::runtime::{advance_ui_layout, Runtime, UI_REFERENCE_VIEWPORT};
use vitric_ecs::World;
use vitric_render::{layout_runs, solve_layout, UiRect};

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui-gallery")
}

/// `layout_runs()` is backed by a process-level global atomic counter `LAYOUT_RUNS` in
/// vitric-render, +1 each time layout is actually solved (for test observation, not product
/// logic). Integration tests run in parallel by default inside the same test binary: while A is
/// counting "how many recomputes happened", as long as B triggers one layout solve it bumps this
/// global counter by 1, polluting A's assertion → intermittent failure (passes single-threaded,
/// occasionally fails when parallel).
///
/// This process-level serial lock lets "every test that triggers layout solving or reads the
/// layout_runs counter" acquire the lock at function entry and hold the guard until function end,
/// so they run serially and the counter is no longer polluted by concurrency. It only serializes
/// these counter-sensitive tests and does not touch product code (the global counter is kept).
/// Lock poisoning (the holding test panics) does not affect subsequent tests — we uniformly use
/// `.unwrap_or_else(|e| e.into_inner())` to recover from a poisoned lock below.
static LAYOUT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire the test serial lock; when the lock was poisoned by a previous test's panic, still
/// take back the inner guard so subsequent tests are not affected.
fn lock_layout_tests() -> std::sync::MutexGuard<'static, ()> {
    LAYOUT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- System-level: advance_ui_layout directly drives a World ----

/// Build a minimal UI world (font-free): a root + a centered Panel (400x200).
fn ui_world() -> (World, vitric_ecs::EntityId) {
    let mut w = World::new();
    let root = w.spawn_named("ui").unwrap();
    w.set_component(root, "UiRoot", json!({"layout_hash": ""})).unwrap();
    let panel = w.spawn_named("panel").unwrap();
    w.set_component(
        panel,
        "Ui",
        json!({"anchor": "center", "ax": 0.0, "ay": 0.0, "ox": 0.0, "oy": 0.0,
               "w": 400.0, "h": 200.0, "parent": "ui", "weight": 0.0,
               "rx": 0.0, "ry": 0.0, "rw": 0.0, "rh": 0.0}),
    )
    .unwrap();
    (w, panel)
}

#[test]
fn layout_system_writes_rects_into_components() {
    // This test does not read layout_runs, but advance_ui_layout will bump the global counter
    // by 1, polluting parallel counter-sensitive tests, so it also takes this serial lock to
    // stagger against them.
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    // Centered 400x200 within a 1000x600 viewport → x=300, y=200
    assert_eq!(w.get_field(panel, "Ui.rx").unwrap(), &json!(300.0));
    assert_eq!(w.get_field(panel, "Ui.ry").unwrap(), &json!(200.0));
    assert_eq!(w.get_field(panel, "Ui.rw").unwrap(), &json!(400.0));
    assert_eq!(w.get_field(panel, "Ui.rh").unwrap(), &json!(200.0));
}

#[test]
fn static_ui_recomputes_zero_times_across_many_ticks() {
    // Hard performance requirement: a static UI played for N consecutive ticks → 0 layout
    // recomputes (dirty flag).
    let _guard = lock_layout_tests();
    let (mut w, _panel) = ui_world();
    // First time: dirty (layout_hash empty), solves once
    let before = layout_runs();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let after_first = layout_runs();
    assert!(after_first > before, "首次应解算一次");
    // Next 50 ticks the UI did not move: the layout algorithm must not run again
    for _ in 0..50 {
        advance_ui_layout(&mut w, (1000, 600)).unwrap();
    }
    assert_eq!(layout_runs(), after_first, "静止 UI 连播 50 tick，布局重算必须为 0");
}

#[test]
fn mutating_ui_marks_dirty_and_recomputes_once() {
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let settled = layout_runs();
    // Change size → dirty → next tick solves once, then static again with zero recompute
    w.set_field(panel, "Ui.w", json!(500.0)).unwrap();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let after_change = layout_runs();
    assert_eq!(after_change, settled + 1, "改尺寸应触发恰好一次重算");
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    assert_eq!(layout_runs(), after_change, "改完再静止应零重算");
    // New size written back
    assert_eq!(w.get_field(panel, "Ui.rw").unwrap(), &json!(500.0));
}

#[test]
fn empty_ui_world_is_zero_cost() {
    // No UiRoot: advance_ui_layout does not solve (layout_runs unchanged).
    let _guard = lock_layout_tests();
    let mut w = World::new();
    let before = layout_runs();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    assert_eq!(layout_runs(), before, "空 UI 不该跑布局");
}

#[test]
fn snapshot_restore_preserves_layout_state() {
    // Snapshot/replay: layout state (rx/ry/rw/rh + layout_hash) goes into the snapshot with the
    // components; continued playback after replay is consistent.
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let snap = w.snapshot();
    let hash_before = w.state_hash();

    // Restore in another world
    let mut w2 = World::new();
    w2.restore(&snap).unwrap();
    assert_eq!(w2.state_hash(), hash_before, "回放后状态哈希必须一致");
    let panel2 = w2.entity("panel").unwrap();
    assert_eq!(w2.get_field(panel2, "Ui.rx").unwrap(), &json!(300.0));
    // Continued playback after replay: UI did not move → zero recompute (layout_hash was brought
    // back with the snapshot)
    let runs = layout_runs();
    advance_ui_layout(&mut w2, (1000, 600)).unwrap();
    assert_eq!(layout_runs(), runs, "回放后静止 UI 续播应零重算");
    // The panel handle only makes sense in the original world; here we just confirm it once
    // existed (replay targets the name panel2)
    assert!(w.is_alive(panel));
}

#[test]
fn layout_is_independent_of_camera() {
    // UI screen space: when the camera moves/scales/shakes, the UI's screen position does not
    // change (proves it does not go through the camera transform).
    // Add a moving camera to the same world; solve_layout's result does not change with the
    // camera.
    // Two solve_layout calls bump the global counter by +2, polluting counter-sensitive tests,
    // so this also takes the lock to serialize.
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    let cam = w.spawn_named("camera").unwrap();
    w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0})).unwrap();

    let l0 = solve_layout(&w, 1000, 600).unwrap();
    let r0 = *l0.get(&panel).unwrap();

    // Move camera + change scale
    w.set_field(cam, "Camera.x", json!(123.0)).unwrap();
    w.set_field(cam, "Camera.y", json!(-77.0)).unwrap();
    w.set_field(cam, "Camera.scale", json!(20.0)).unwrap();
    let l1 = solve_layout(&w, 1000, 600).unwrap();
    let r1 = *l1.get(&panel).unwrap();

    assert_eq!(r0, r1, "镜头移动/缩放后 UI 屏幕矩形必须完全不变（不经相机）");
    assert_eq!(r0, UiRect { x: 300.0, y: 200.0, w: 400.0, h: 200.0 });
}

// ---- End-to-end: examples/ui-gallery gray-box demo ----

#[test]
fn gallery_demo_boots_and_lays_out_three_buttons() {
    // Both step and solve_layout bump the global counter by +1; take the lock to avoid polluting
    // counter-sensitive tests.
    let _guard = lock_layout_tests();
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    // Run one tick: the layout system lays out the three buttons of menu-vbox (reference
    // viewport = UI_REFERENCE_VIEWPORT)
    sim.step(&mut rt).unwrap();

    let w = &sim.world;
    let (vw, vh) = UI_REFERENCE_VIEWPORT;
    // Verify layout directly with the solver (same pure function used to write back to components)
    let layout = solve_layout(w, vw, vh).unwrap();
    let rect = |name: &str| *layout.get(&w.entity(name).unwrap()).unwrap();

    // menu-panel centered 600x420 within 1920x1080 → x=660, y=330
    let mp = rect("menu-panel");
    assert_eq!(mp, UiRect { x: 660.0, y: 330.0, w: 600.0, h: 420.0 });

    // Three buttons stacked vertically in VBox, cross=center horizontally centered, gap=24, fixed
    // height 72.
    let b0 = rect("btn-start");
    let b1 = rect("btn-options");
    let b2 = rect("btn-quit");
    // Equal width and height
    assert_eq!(b0.w, 480.0);
    assert_eq!(b0.h, 72.0);
    assert_eq!(b1.w, 480.0);
    assert_eq!(b2.w, 480.0);
    // Vertical: each is 72+24=96 lower than the previous
    assert_eq!(b1.y - b0.y, 96.0);
    assert_eq!(b2.y - b1.y, 96.0);
    // cross=center: the three buttons share the same x (horizontally centered in the VBox
    // content area)
    assert_eq!(b0.x, b1.x);
    assert_eq!(b1.x, b2.x);

    // Button label stretch fills the button frame
    let lbl = rect("btn-start-label");
    assert_eq!(lbl, b0, "stretch 的 label 应与按钮框完全重合");
}

#[test]
fn gallery_demo_renders_without_error_and_ui_overlay_present() {
    // Render one frame (CPU source of truth): the UI gray box is drawn, the image has a non-empty
    // background (the menu panel covers the center).
    // Both step and render_world solve_layout internally, bumping the global counter; take the
    // lock to serialize.
    let _guard = lock_layout_tests();
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    sim.step(&mut rt).unwrap();
    let (w, h) = (960u32, 540u32);
    let buf =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    // The center of the image lands on the middle button of the VBox (#3a4a6b), proving the Panel
    // overlay was drawn;
    // it is not the clear-screen background gray-blue (#181a21) nor the panel base color — the
    // button sits on top of the panel.
    let center = ((h / 2) * w + w / 2) as usize * 4;
    let px = [buf[center], buf[center + 1], buf[center + 2]];
    assert_eq!(px, [0x3a, 0x4a, 0x6b], "中心应是按钮色，证明 UI Panel 叠加层画出来了");
    // A panel corner (a point just inside the top-left of the center panel) should be the panel
    // base color #1b1d26, proving the background frame is also present
    // menu-panel centered 600x420 within 960x540 → top-left (180, 60); take (190,70) inside the
    // panel, outside the buttons
    let corner = (70 * w + 190) as usize * 4;
    assert_eq!(
        [buf[corner], buf[corner + 1], buf[corner + 2]],
        [0x1b, 0x1d, 0x26],
        "面板内非按钮处应是面板底色"
    );
    // Rendering the same frame twice is byte-identical (determinism)
    let buf2 =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    assert_eq!(buf, buf2, "同一世界同一 tick 渲两次必须逐字节相同");
}

#[test]
fn gallery_demo_ui_does_not_drift_with_camera_in_render() {
    // Render-layer proof: after moving the camera, the UI center pixel color does not change (UI
    // does not drift with the camera).
    // Both step and two render_world calls solve_layout, bumping the global counter; take the
    // lock to serialize.
    let _guard = lock_layout_tests();
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    sim.step(&mut rt).unwrap();
    let (w, h) = (960u32, 540u32);
    let center = ((h / 2) * w + w / 2) as usize * 4;

    let buf_a =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    let px_a = [buf_a[center], buf_a[center + 1], buf_a[center + 2]];

    // Move the camera (the demo has a camera entity)
    let cam = sim.world.entity("camera").unwrap();
    sim.world.set_field(cam, "Camera.x", json!(50.0)).unwrap();
    sim.world.set_field(cam, "Camera.scale", json!(20.0)).unwrap();
    let buf_b =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    let px_b = [buf_b[center], buf_b[center + 1], buf_b[center + 2]];

    assert_eq!(px_a, px_b, "镜头移动/缩放后 UI 中心像素必须不变（屏幕空间叠加）");
}

// ---- check: a bad UI project reports path errors item by item ----

/// Write a minimal UI project (schema + one scene). `scene_entities` injects the entities to test.
fn make_ui_project(tag: &str, scene_entities: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-uicheck-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("vitric.json"),
        r#"{"name":"uicheck","schema":"schema.json","entry":"scenes/main.json",
            "scenes":["scenes/main.json"],"seed":1}"#,
    )
    .unwrap();
    // schema: anchor/kind declared as text (not enum) to prove the engine falls back to
    // validating UI semantics
    std::fs::write(
        dir.join("schema.json"),
        r##"{"components":{
            "UiRoot":{"fields":{}},
            "Ui":{"fields":{
                "anchor":{"type":"text","default":"manual"},
                "parent":{"type":"entity"},
                "w":{"type":"number","default":0},"h":{"type":"number","default":0}
            }},
            "Container":{"fields":{
                "kind":{"type":"text","default":"VBox"},
                "columns":{"type":"int","default":1}
            }},
            "Panel":{"fields":{
                "color":{"type":"text","default":"#ffffff"},
                "image":{"type":"text","default":""}
            }}
        }}"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/main.json"),
        format!(r#"{{"entities":[{scene_entities}]}}"#),
    )
    .unwrap();
    dir
}

#[test]
fn check_reports_illegal_anchor_unknown_container_grid_zero() {
    let dir = make_ui_project(
        "semantic",
        r#"{"name":"ui","components":{"UiRoot":{}}},
           {"name":"a","components":{"Ui":{"anchor":"top-middle"}}},
           {"name":"b","components":{"Container":{"kind":"Flex"}}},
           {"name":"g","components":{"Container":{"kind":"Grid","columns":0}}}"#,
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("坏 UI 必须红灯");
    assert!(err.contains("VD070") && err.contains("Ui/anchor"), "非法锚点: {err}");
    assert!(err.contains("VD071") && err.contains("Container/kind"), "未知容器: {err}");
    assert!(err.contains("VD072") && err.contains("columns"), "Grid 列数 0: {err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_reports_missing_panel_image() {
    let dir = make_ui_project(
        "panelimg",
        r#"{"name":"ui","components":{"UiRoot":{}}},
           {"name":"p","components":{"Ui":{"anchor":"center","parent":"ui","w":100,"h":50},
                                       "Panel":{"image":"nope.png"}}}"#,
    );
    let err = vitric_cli::runtime::check(&dir).expect_err("Panel.image 缺图必须红灯");
    assert!(err.contains("nope.png") && err.contains("Panel.image"), "报错点名缺图: {err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Load demo assets (including fonts) — render_world needs fonts to draw vector labels.
fn rt_assets(dir: &Path) -> &'static vitric_render::Assets {
    // In tests, leak once into 'static to avoid reloading each time (test-process only)
    let mut a = vitric_render::Assets::load_dir(&dir.join("assets")).unwrap();
    a.load_font(&dir.join("fonts/DejaVuSans.ttf")).unwrap();
    Box::leak(Box::new(a))
}
