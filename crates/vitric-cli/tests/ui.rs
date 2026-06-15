//! UI 控件（布局 1.1）端到端 + 系统级：布局系统每 tick 推进、脏标记零重算、
//! 快照/回放续播一致、镜头变化 UI 不飘、灰盒 demo 摆位正确、render 真的画出来。
//!
//! 用 examples/ui-gallery 这个纯控件原语拼出来的"主菜单灰盒"证明：引擎只给通用
//! 控件（Panel/Label/Container/锚点），具体界面是项目用积木拼的用法（不可交互，
//! 交互见 1.2）。

use std::path::{Path, PathBuf};

use serde_json::json;

use vitric_cli::runtime::{advance_ui_layout, Runtime, UI_REFERENCE_VIEWPORT};
use vitric_ecs::World;
use vitric_render::{layout_runs, solve_layout, UiRect};

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui-gallery")
}

/// `layout_runs()` 背后是 vitric-render 里一个进程级全局原子计数器 `LAYOUT_RUNS`，
/// 每次真解算布局就 +1（给测试观测用，不是产品逻辑）。集成测试默认在同一个测试
/// binary 里并行跑：A 在数"重算了几次"时，B 只要也触发了一次布局 solve，就会把
/// 这个全局计数器 +1，污染 A 的断言 → 间歇性失败（单线程跑必过，并行偶发挂）。
///
/// 这把进程级串行锁让"所有会触发布局解算或读取 layout_runs 计数的测试"在函数开头
/// 各自拿锁、守卫活到函数结束，从而彼此串行、计数器不再被并发污染。它只串行这些
/// 计数敏感的测试，不碰产品代码（全局计数器保留）。锁中毒（持锁测试 panic）不影响
/// 后续测试——下面统一用 `.unwrap_or_else(|e| e.into_inner())` 兜 poisoned。
static LAYOUT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 拿测试串行锁；锁被前一个 panic 的测试毒化时仍取回内层守卫，不连累后续测试。
fn lock_layout_tests() -> std::sync::MutexGuard<'static, ()> {
    LAYOUT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- 系统级：advance_ui_layout 直接驱动一个 World ----

/// 造一个根 + 居中 Panel（400x200）的最小 UI 世界（无字体依赖）。
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
    // 本测试不读 layout_runs，但 advance_ui_layout 会让全局计数器 +1，会污染并行
    // 跑的计数敏感测试，故也拿这把串行锁，与它们错开。
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    // 居中 400x200 于 1000x600 视口 → x=300, y=200
    assert_eq!(w.get_field(panel, "Ui.rx").unwrap(), &json!(300.0));
    assert_eq!(w.get_field(panel, "Ui.ry").unwrap(), &json!(200.0));
    assert_eq!(w.get_field(panel, "Ui.rw").unwrap(), &json!(400.0));
    assert_eq!(w.get_field(panel, "Ui.rh").unwrap(), &json!(200.0));
}

#[test]
fn static_ui_recomputes_zero_times_across_many_ticks() {
    // 性能硬要求：静止 UI 连播 N tick，布局重算 0 次（脏标记）。
    let _guard = lock_layout_tests();
    let (mut w, _panel) = ui_world();
    // 第一次：脏（layout_hash 空），解算一次
    let before = layout_runs();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let after_first = layout_runs();
    assert!(after_first > before, "首次应解算一次");
    // 之后 50 tick UI 没动：布局算法一次都不该再跑
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
    // 改尺寸 → 脏 → 下一 tick 解算一次，再静止又零重算
    w.set_field(panel, "Ui.w", json!(500.0)).unwrap();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let after_change = layout_runs();
    assert_eq!(after_change, settled + 1, "改尺寸应触发恰好一次重算");
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    assert_eq!(layout_runs(), after_change, "改完再静止应零重算");
    // 新尺寸已写回
    assert_eq!(w.get_field(panel, "Ui.rw").unwrap(), &json!(500.0));
}

#[test]
fn empty_ui_world_is_zero_cost() {
    // 没有 UiRoot：advance_ui_layout 不解算（layout_runs 不增）。
    let _guard = lock_layout_tests();
    let mut w = World::new();
    let before = layout_runs();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    assert_eq!(layout_runs(), before, "空 UI 不该跑布局");
}

#[test]
fn snapshot_restore_preserves_layout_state() {
    // 快照/回放：布局态（rx/ry/rw/rh + layout_hash）随组件进快照，回放后续播一致。
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    advance_ui_layout(&mut w, (1000, 600)).unwrap();
    let snap = w.snapshot();
    let hash_before = w.state_hash();

    // 在另一个世界里恢复
    let mut w2 = World::new();
    w2.restore(&snap).unwrap();
    assert_eq!(w2.state_hash(), hash_before, "回放后状态哈希必须一致");
    let panel2 = w2.entity("panel").unwrap();
    assert_eq!(w2.get_field(panel2, "Ui.rx").unwrap(), &json!(300.0));
    // 回放后续播：UI 没动 → 零重算（layout_hash 已随快照带回）
    let runs = layout_runs();
    advance_ui_layout(&mut w2, (1000, 600)).unwrap();
    assert_eq!(layout_runs(), runs, "回放后静止 UI 续播应零重算");
    // panel 句柄只在原世界有意义；这里仅确认它存在过（回放对的是名字 panel2）
    assert!(w.is_alive(panel));
}

#[test]
fn layout_is_independent_of_camera() {
    // UI 屏幕空间：镜头移动/缩放/抖动 UI 屏幕位置不变（证明不经相机变换）。
    // 在同一个世界里加一个会动的相机，solve_layout 的结果不随相机变。
    // 两次 solve_layout 会让全局计数器 +2，会污染计数敏感测试，故也拿锁串行。
    let _guard = lock_layout_tests();
    let (mut w, panel) = ui_world();
    let cam = w.spawn_named("camera").unwrap();
    w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "scale": 8.0})).unwrap();

    let l0 = solve_layout(&w, 1000, 600).unwrap();
    let r0 = *l0.get(&panel).unwrap();

    // 移动相机 + 改缩放
    w.set_field(cam, "Camera.x", json!(123.0)).unwrap();
    w.set_field(cam, "Camera.y", json!(-77.0)).unwrap();
    w.set_field(cam, "Camera.scale", json!(20.0)).unwrap();
    let l1 = solve_layout(&w, 1000, 600).unwrap();
    let r1 = *l1.get(&panel).unwrap();

    assert_eq!(r0, r1, "镜头移动/缩放后 UI 屏幕矩形必须完全不变（不经相机）");
    assert_eq!(r0, UiRect { x: 300.0, y: 200.0, w: 400.0, h: 200.0 });
}

// ---- 端到端：examples/ui-gallery 灰盒 demo ----

#[test]
fn gallery_demo_boots_and_lays_out_three_buttons() {
    // step + solve_layout 都让全局计数器 +1，拿锁避免污染计数敏感测试。
    let _guard = lock_layout_tests();
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    // 跑一 tick：布局系统把 menu-vbox 的三个按钮排好（参照视口 = UI_REFERENCE_VIEWPORT）
    sim.step(&mut rt).unwrap();

    let w = &sim.world;
    let (vw, vh) = UI_REFERENCE_VIEWPORT;
    // 直接用 solver 验布局（和写回组件同一份纯函数）
    let layout = solve_layout(w, vw, vh).unwrap();
    let rect = |name: &str| *layout.get(&w.entity(name).unwrap()).unwrap();

    // menu-panel 居中 600x420 于 1920x1080 → x=660, y=330
    let mp = rect("menu-panel");
    assert_eq!(mp, UiRect { x: 660.0, y: 330.0, w: 600.0, h: 420.0 });

    // 三个按钮在 VBox 里竖排、cross=center 水平居中、gap=24、固定高 72。
    let b0 = rect("btn-start");
    let b1 = rect("btn-options");
    let b2 = rect("btn-quit");
    // 等宽等高
    assert_eq!(b0.w, 480.0);
    assert_eq!(b0.h, 72.0);
    assert_eq!(b1.w, 480.0);
    assert_eq!(b2.w, 480.0);
    // 竖排：每个比上一个低 72+24=96
    assert_eq!(b1.y - b0.y, 96.0);
    assert_eq!(b2.y - b1.y, 96.0);
    // cross=center：三个按钮 x 相同（水平居中于 VBox 内容区）
    assert_eq!(b0.x, b1.x);
    assert_eq!(b1.x, b2.x);

    // 按钮 label stretch 填满按钮框
    let lbl = rect("btn-start-label");
    assert_eq!(lbl, b0, "stretch 的 label 应与按钮框完全重合");
}

#[test]
fn gallery_demo_renders_without_error_and_ui_overlay_present() {
    // 渲染一帧（CPU 真相源）：UI 灰盒画出来，画面非空背景（菜单面板覆盖中心）。
    // step + render_world 内部都会 solve_layout，让全局计数器增长，拿锁串行。
    let _guard = lock_layout_tests();
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    sim.step(&mut rt).unwrap();
    let (w, h) = (960u32, 540u32);
    let buf =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    // 画面中心落在 VBox 中间那个按钮上（#3a4a6b），证明 Panel 叠加层画出来了；
    // 不是清屏背景灰蓝（#181a21）也不是面板底色——按钮盖在面板之上。
    let center = ((h / 2) * w + w / 2) as usize * 4;
    let px = [buf[center], buf[center + 1], buf[center + 2]];
    assert_eq!(px, [0x3a, 0x4a, 0x6b], "中心应是按钮色，证明 UI Panel 叠加层画出来了");
    // 面板边角（中心面板左上角内一点）应是面板底色 #1b1d26，证明背景框也在
    // menu-panel 居中 600x420 于 960x540 → 左上 (180, 60)；取 (190,70) 在面板内、按钮外
    let corner = (70 * w + 190) as usize * 4;
    assert_eq!(
        [buf[corner], buf[corner + 1], buf[corner + 2]],
        [0x1b, 0x1d, 0x26],
        "面板内非按钮处应是面板底色"
    );
    // 同一帧渲两次逐字节一致（确定性）
    let buf2 =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    assert_eq!(buf, buf2, "同一世界同一 tick 渲两次必须逐字节相同");
}

#[test]
fn gallery_demo_ui_does_not_drift_with_camera_in_render() {
    // 渲染层证明：移动相机后，UI 中心像素颜色不变（UI 不随镜头飘）。
    // step + 两次 render_world 都会 solve_layout，让全局计数器增长，拿锁串行。
    let _guard = lock_layout_tests();
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    sim.step(&mut rt).unwrap();
    let (w, h) = (960u32, 540u32);
    let center = ((h / 2) * w + w / 2) as usize * 4;

    let buf_a =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    let px_a = [buf_a[center], buf_a[center + 1], buf_a[center + 2]];

    // 移动相机（demo 有个 camera 实体）
    let cam = sim.world.entity("camera").unwrap();
    sim.world.set_field(cam, "Camera.x", json!(50.0)).unwrap();
    sim.world.set_field(cam, "Camera.scale", json!(20.0)).unwrap();
    let buf_b =
        vitric_render::render_world(&sim.world, w, h, rt_assets(&demo_dir()), sim.tick).unwrap();
    let px_b = [buf_b[center], buf_b[center + 1], buf_b[center + 2]];

    assert_eq!(px_a, px_b, "镜头移动/缩放后 UI 中心像素必须不变（屏幕空间叠加）");
}

// ---- check：坏 UI 项目逐项报路径错误 ----

/// 写一个最小 UI 项目（schema + 一个场景）。`scene_entities` 注入要测的实体。
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
    // schema：把 anchor/kind 声明成 text（不靠 enum），证明引擎兜底校验 UI 语义
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

/// 加载 demo 素材（含字体）——render_world 需要字体来画矢量 label。
fn rt_assets(dir: &Path) -> &'static vitric_render::Assets {
    // 测试里一次性 leak 成 'static 省得每次重载（只在测试进程里）
    let mut a = vitric_render::Assets::load_dir(&dir.join("assets")).unwrap();
    a.load_font(&dir.join("fonts/DejaVuSans.ttf")).unwrap();
    Box::leak(Box::new(a))
}
