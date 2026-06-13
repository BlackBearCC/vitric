//! UI 交互（1.2）端到端：焦点导航 + 点击激活 + 按下反馈 + 主题 + 双录像 gate。
//!
//! 在 1.1 布局之上验证交互：
//! - 焦点导航：方向输入按布局相邻关系移焦点（逐步断言落点），confirm 激活发 ui-activate，
//!   disabled 不可聚焦被跳过；
//! - 点击激活：屏幕归一化坐标换算到参照系 1920×1080 命中按钮矩形（坐标换算正确性断言：
//!   参照系内命中 / 边界外不命中 / disabled 不响应）；
//! - 按下反馈：scale/modulate 解析式逐值；快照/回放焦点态一致；
//! - 菜单 demo + 双录像 gate：一条走焦点导航、一条走鼠标点击，都激活"开始"→ emit
//!   game-started → 规则 load-scene 切到 game 场景，两条逐位重放一致。

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_render::{press_modulate, press_scale, PRESS_TICKS};
use vitric_sim::{GameLogic, Sim};

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui-menu")
}

/// 当前焦点按钮名（UiRoot.focus）。
fn focus(sim: &Sim) -> String {
    let root = sim.world.entity("ui").unwrap();
    sim.world.get_field(root, "UiRoot.focus").unwrap().as_str().unwrap_or("").to_string()
}

/// 某按钮的状态。
fn btn_state(sim: &Sim, name: &str) -> String {
    let id = sim.world.entity(name).unwrap();
    sim.world.get_field(id, "Button.state").unwrap().as_str().unwrap_or("").to_string()
}

/// 推一 tick（注入的输入下一 tick 生效）。
fn step(sim: &mut Sim, rt: &mut Runtime) {
    sim.step(rt).unwrap();
}

// ---- 焦点导航 ----

#[test]
fn focus_navigation_moves_between_buttons_skipping_disabled() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    // tick 0：布局解算 + 交互系统把焦点落到第一个可聚焦按钮（btn-start，场景里已是 focused）
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-start", "初始焦点在第一个可聚焦按钮");
    assert_eq!(btn_state(&sim, "btn-start"), "focused");
    assert_eq!(btn_state(&sim, "btn-options"), "disabled", "options 是 disabled");

    // 往下：btn-options 是 disabled，被跳过 → 直接到 btn-quit
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-quit", "向下跳过 disabled 的 options 落到 quit");
    assert_eq!(btn_state(&sim, "btn-quit"), "focused");
    assert_eq!(btn_state(&sim, "btn-start"), "normal", "旧焦点回 normal");

    // 再往下：到底不动（不环绕）
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-quit", "到底不环绕");

    // 往上：回 btn-start（跳过 disabled）
    sim.inject_input("ui-up", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-start", "向上跳过 disabled 回 start");
}

#[test]
fn confirm_activates_focused_button_emitting_ui_activate() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // 焦点落 btn-start
    assert_eq!(focus(&sim), "btn-start");

    // 确认：激活当前焦点 → 交互系统发 ui-activate（事件内容由下一条测试断言），
    // 本 tick btn-start 进 pressed 态
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(btn_state(&sim, "btn-start"), "pressed", "确认后焦点按钮进 pressed");

    // 下一 tick：规则收到 ui-activate{action:start} → emit game-started + load-scene → 切场景
    step(&mut sim, &mut rt);
    step(&mut sim, &mut rt);
    // 切到 game 场景后 menu 实体没了，hero 在
    assert!(sim.world.entity("hero").is_ok(), "start 激活后应切到 game 场景（hero 存在）");
    assert!(sim.world.entity("btn-start").is_err(), "menu 场景已被推倒");
}

#[test]
fn ui_activate_event_carries_id_and_action() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    sim.inject_input("ui-confirm", "pressed");
    // 交互系统发的 ui-activate 进 observed（本 tick 可见）
    sim.step(&mut rt).unwrap();
    let observed = rt.drain_observed();
    let act = observed.iter().find(|e| e.name == "ui-activate").expect("应发 ui-activate");
    assert_eq!(act.data["action"], json!("start"));
    assert_eq!(act.data["id"], json!("btn-start"));
}

// ---- 点击激活 + 坐标换算正确性 ----

/// 取按钮在参照系 1920×1080 里的矩形（rx/ry/rw/rh），算出它中心的归一化坐标。
fn button_center_normalized(sim: &Sim, name: &str) -> (f64, f64) {
    let id = sim.world.entity(name).unwrap();
    let rx = sim.world.get_field(id, "Ui.rx").unwrap().as_f64().unwrap();
    let ry = sim.world.get_field(id, "Ui.ry").unwrap().as_f64().unwrap();
    let rw = sim.world.get_field(id, "Ui.rw").unwrap().as_f64().unwrap();
    let rh = sim.world.get_field(id, "Ui.rh").unwrap().as_f64().unwrap();
    // 参照系坐标 → 归一化（除以 1920/1080），就是注入端会做的逆运算
    ((rx + rw / 2.0) / 1920.0, (ry + rh / 2.0) / 1080.0)
}

#[test]
fn click_in_reference_space_hits_button_and_activates() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // 布局先解算，按钮矩形写进 rx/ry/rw/rh

    // 点击 btn-quit 中心（归一化坐标）：换算回参照系应命中 quit 矩形 → 激活
    let (nx, ny) = button_center_normalized(&sim, "btn-quit");
    vitric_control::inject_ui_click(&mut sim, nx, ny, "left").unwrap();
    step(&mut sim, &mut rt);
    assert_eq!(btn_state(&sim, "btn-quit"), "pressed", "点中 quit 中心应激活它");
    assert_eq!(focus(&sim), "btn-quit", "点击命中也把焦点移过去");
}

#[test]
fn click_outside_any_button_does_not_activate() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    let start_before = btn_state(&sim, "btn-start");
    // 点最左上角 (0,0) 归一化——在所有按钮矩形外（按钮居中）
    vitric_control::inject_ui_click(&mut sim, 0.001, 0.001, "left").unwrap();
    step(&mut sim, &mut rt);
    // 没有按钮被激活（都不是 pressed）
    for b in ["btn-start", "btn-quit"] {
        assert_ne!(btn_state(&sim, b), "pressed", "{b} 不该被边界外点击激活");
    }
    assert_eq!(btn_state(&sim, "btn-start"), start_before, "边界外点击不改任何按钮");
}

#[test]
fn click_boundary_just_outside_rect_misses() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    let id = sim.world.entity("btn-quit").unwrap();
    let rx = sim.world.get_field(id, "Ui.rx").unwrap().as_f64().unwrap();
    let ry = sim.world.get_field(id, "Ui.ry").unwrap().as_f64().unwrap();
    let rh = sim.world.get_field(id, "Ui.rh").unwrap().as_f64().unwrap();
    // 矩形左边界外 1 像素（参照系）→ 归一化 → 不命中
    let nx = (rx - 1.0) / 1920.0;
    let ny = (ry + rh / 2.0) / 1080.0;
    vitric_control::inject_ui_click(&mut sim, nx, ny, "left").unwrap();
    step(&mut sim, &mut rt);
    assert_ne!(btn_state(&sim, "btn-quit"), "pressed", "左边界外 1px 不该命中");
}

#[test]
fn click_on_disabled_button_does_not_respond() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    // 点 btn-options（disabled）中心
    let (nx, ny) = button_center_normalized(&sim, "btn-options");
    vitric_control::inject_ui_click(&mut sim, nx, ny, "left").unwrap();
    step(&mut sim, &mut rt);
    assert_eq!(btn_state(&sim, "btn-options"), "disabled", "disabled 按钮点击不响应（仍 disabled）");
}

// ---- 按下反馈解析式 ----

#[test]
fn press_feedback_is_analytic_and_recorded_in_component() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    // 激活 btn-start：press_t 从 0 起，逐 tick 推进，scale/modulate 是 press_t 的纯函数
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt); // 激活那 tick：press_t=0
    let id = sim.world.entity("btn-start").unwrap();
    let pt = |sim: &Sim| sim.world.get_field(sim.world.entity("btn-start").unwrap(), "Button.press_t").unwrap().as_i64().unwrap();
    assert_eq!(pt(&sim), 0, "激活那 tick press_t=0");
    // press_scale(0)=1（无缩），中点最深，末端回 1
    assert_eq!(press_scale(0, 0.92), 1.0);
    assert!(press_scale(PRESS_TICKS / 2, 0.92) < 1.0);
    assert_eq!(press_scale(PRESS_TICKS, 0.92), 1.0);
    let _ = id;

    // 逐 tick 推进 press_t（注意 start 激活后规则会切场景——这里只看激活 tick 的反馈，
    // 改测一个不切场景的按钮：quit）
    let (mut sim2, mut rt2) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim2, &mut rt2);
    sim2.inject_input("ui-down", "pressed"); // 焦点到 quit
    step(&mut sim2, &mut rt2);
    sim2.inject_input("ui-confirm", "pressed"); // 激活 quit（quit 规则只 emit app-quit，不切场景）
    step(&mut sim2, &mut rt2);
    let qpt = |sim: &Sim| sim.world.get_field(sim.world.entity("btn-quit").unwrap(), "Button.press_t").unwrap().as_i64().unwrap();
    assert_eq!(qpt(&sim2), 0, "quit 激活 tick press_t=0");
    for expect in 1..PRESS_TICKS {
        step(&mut sim2, &mut rt2);
        assert_eq!(qpt(&sim2) as u64, expect, "press_t 逐 tick +1（解析式驱动 scale/modulate）");
    }
    // 到点：反馈结束，press_t 归 -1，状态回 normal（quit 不再有焦点—焦点在 quit 但…
    // 激活后焦点仍在 quit，所以应回 focused）
    step(&mut sim2, &mut rt2);
    assert_eq!(qpt(&sim2), -1, "PRESS_TICKS 后反馈结束 press_t=-1");
    assert_eq!(btn_state(&sim2, "btn-quit"), "focused", "反馈结束回 focused（焦点仍在它身上）");
    // modulate 解析式对称
    assert_eq!(press_modulate(0), 0.0);
    assert!((press_modulate(PRESS_TICKS / 2) - 1.0).abs() < 1e-9);
}

#[test]
fn snapshot_restore_preserves_focus_and_press_state() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    // 移焦点到 quit + 激活，造出焦点态 + pressed 态
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt);
    let snap = sim.snapshot(&rt);
    let hash_before = sim.world.state_hash();
    assert_eq!(focus(&sim), "btn-quit");
    assert_eq!(btn_state(&sim, "btn-quit"), "pressed");

    // 继续跑几 tick（press_t 推进）
    for _ in 0..3 {
        step(&mut sim, &mut rt);
    }
    assert_ne!(sim.world.state_hash(), hash_before, "跑几 tick 后哈希应变");

    // 恢复到快照：焦点态 + press_t 完全回到存档时刻
    sim.restore(&snap, &mut rt).unwrap();
    assert_eq!(sim.world.state_hash(), hash_before, "restore 后哈希必须逐位回到存档时刻");
    assert_eq!(focus(&sim), "btn-quit");
    assert_eq!(btn_state(&sim, "btn-quit"), "pressed");

    // 续播一致：再跑同样 tick 数，和第一次跑的结果一致（确定性续播）
    let mut replay_hashes = Vec::new();
    for _ in 0..3 {
        step(&mut sim, &mut rt);
        replay_hashes.push(sim.world.state_hash());
    }
    sim.restore(&snap, &mut rt).unwrap();
    let mut second = Vec::new();
    for _ in 0..3 {
        step(&mut sim, &mut rt);
        second.push(sim.world.state_hash());
    }
    assert_eq!(replay_hashes, second, "从同一快照续播两次逐 tick 哈希一致");
}

// ---- 主题应用 + 渲染按下反馈 ----

fn rt_assets() -> &'static vitric_render::Assets {
    let mut a = vitric_render::Assets::load_dir(&demo_dir().join("assets")).unwrap();
    a.load_font(&demo_dir().join("fonts/DejaVuSans.ttf")).unwrap();
    Box::leak(Box::new(a))
}

#[test]
fn theme_resolves_button_state_into_panel_color() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // 焦点落 btn-start，主题把 focused 底色写进 Panel.color
    let start = sim.world.entity("btn-start").unwrap();
    let color = |sim: &Sim, id| sim.world.get_field(id, "Panel.color").unwrap().as_str().unwrap().to_string();
    // btn-start focused → 主题 dark 的 button.focused.bg = #5a7bb5
    assert_eq!(color(&sim, start), "#5a7bb5", "focused 按钮底色来自主题 focused 样式");
    // btn-options disabled → button.disabled.bg = #2a2d36
    let opt = sim.world.entity("btn-options").unwrap();
    assert_eq!(color(&sim, opt), "#2a2d36", "disabled 按钮底色来自主题 disabled 样式");
    // btn-quit normal → button.normal.bg = #3a4a6b
    let quit = sim.world.entity("btn-quit").unwrap();
    assert_eq!(color(&sim, quit), "#3a4a6b", "normal 按钮底色来自主题 normal 样式");

    // 移焦点到 quit：quit 变 focused 底色，start 回 normal 底色
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(color(&sim, quit), "#5a7bb5", "焦点移过来后 quit 是 focused 底色");
    assert_eq!(color(&sim, start), "#3a4a6b", "start 失焦回 normal 底色");
}

#[test]
fn press_feedback_shrinks_and_brightens_rendered_button() {
    // 渲染层证明按下反馈：pressed 中点的按钮在画面里既缩小又提亮（纯函数装饰，读 press_t）。
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    sim.inject_input("ui-down", "pressed"); // 焦点到 quit（quit 不切场景）
    step(&mut sim, &mut rt);
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt); // quit 激活，press_t=0

    let (w, h) = (960u32, 540u32);
    // 推到按下反馈中点（press_t ≈ PRESS_TICKS/2，缩放/染色最深）
    for _ in 0..(PRESS_TICKS / 2) {
        step(&mut sim, &mut rt);
    }
    let quit = sim.world.entity("btn-quit").unwrap();
    let pt = sim.world.get_field(quit, "Button.press_t").unwrap().as_i64().unwrap();
    assert!(pt > 0 && (pt as u64) < PRESS_TICKS, "应在反馈中途，press_t={pt}");

    // quit 的参照系矩形 → 960x540 的屏幕矩形（draw_ui 用真实分辨率重解算布局）
    let layout = vitric_render::solve_layout(&sim.world, w, h).unwrap();
    let r = *layout.get(&quit).unwrap();
    // 矩形中心像素：按下中点底色是 pressed(#9fc0f0) 再被 modulate 提亮——比未提亮更亮
    let cx = (r.x + r.w / 2.0) as u32;
    let cy = (r.y + r.h / 2.0) as u32;
    let buf = vitric_render::render_world(&sim.world, w, h, rt_assets(), sim.tick).unwrap();
    let center = ((cy * w + cx) * 4) as usize;
    let px = [buf[center], buf[center + 1], buf[center + 2]];
    // pressed 底色 #9fc0f0 = (159,192,240)；modulate 往白提亮，三通道都应 ≥ 原值
    assert!(px[0] >= 159 && px[1] >= 192 && px[2] >= 240, "按下中点应被提亮（≥pressed 底色）: {px:?}");

    // 缩放证明：矩形边缘外一点点（按下缩小后该处露出背景/面板，不再是按钮色）。
    // 取矩形左边缘内 1px——未缩放时是按钮色，缩放后这里被缩进去露出底。
    let edge = (cy * w + (r.x as u32 + 1)) as usize * 4;
    let epx = [buf[edge], buf[edge + 1], buf[edge + 2]];
    // 缩进后边缘不是提亮后的纯按钮色（露出面板底 #12141c 或更暗）
    assert!(epx != px, "缩放后左边缘像素应与中心不同（按钮缩进去了）: edge={epx:?} center={px:?}");
}

// ---- 性能 / 确定性：空 UI 零成本、静止 UI 无杂写 ----

#[test]
fn idle_ui_without_input_does_not_change_state_hash() {
    // 性能/确定性：静止 UI（无任何输入）连播 N tick，交互系统不该乱写组件——
    // 状态哈希在焦点稳定后保持不变（按下反馈不在进行中时，press_t 不动）。
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // 焦点落定 + 布局解算
    let settled = sim.world.state_hash();
    for _ in 0..30 {
        step(&mut sim, &mut rt);
        assert_eq!(sim.world.state_hash(), settled, "静止 UI 无输入连播，状态哈希必须不变");
    }
}

/// 空 UI（无 UiRoot）：交互系统零成本 early-return（直接驱动 advance_ui_interaction）。
#[test]
fn empty_ui_interaction_is_zero_cost() {
    use vitric_cli::runtime::advance_ui_interaction;
    use vitric_ecs::World;
    let mut w = World::new();
    // 没有 UiRoot：返回空事件，零分配零遍历（不报错、不改世界）
    let before = w.state_hash();
    let events = advance_ui_interaction(&mut w, &[], (1920, 1080)).unwrap();
    assert!(events.is_empty(), "空 UI 不发任何 ui-activate");
    assert_eq!(w.state_hash(), before, "空 UI 不改世界");
}

// ---- 录像生成 + 双 gate 重放 ----

/// 录一局：boot demo → 注入序列 → 跑到结束 → 写录像文件。
/// 两条录像都激活"开始"→ emit game-started → 规则 load-scene 切到 game 场景。
/// 录像里存的是输入（ui-down/up/confirm）和回复（ui-click 归一化坐标），重放从录像
/// 原样注入——离线重放逐位一致（gate 双录像认证的本体）。
fn record_playthrough(
    inputs: &[(u64, &str)],
    clicks: &[(u64, f64, f64)],
    total_ticks: u64,
) -> vitric_sim::Recording {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    sim.start_recording();
    for t in 0..total_ticks {
        for (it, action) in inputs {
            if *it == t {
                sim.inject_input(action, "pressed");
            }
        }
        for (ct, nx, ny) in clicks {
            if *ct == t {
                vitric_control::inject_ui_click(&mut sim, *nx, *ny, "left").unwrap();
            }
        }
        sim.step(&mut rt).unwrap();
    }
    sim.stop_recording().unwrap()
}

/// 生成器：写两条 gate 录像到 examples/ui-menu/recordings/。
/// 平时 `#[ignore]`，改了交互逻辑后跑一次 `cargo test -- --ignored regen_gate_recordings`
/// 重新生成（哈希随确定性逻辑变化，必须随代码一起更新）。
#[test]
#[ignore = "录像生成器：改交互逻辑后手动跑一次重生成 gate 录像"]
fn regen_gate_recordings() {
    let dir = demo_dir();
    // 一条走焦点导航：tick5 下移(跳过 disabled 到 quit)、tick10 上移回 start、tick15 确认 → 切场景
    let nav = record_playthrough(&[(5, "ui-down"), (10, "ui-up"), (15, "ui-confirm")], &[], 40);
    std::fs::write(
        dir.join("recordings/focus-nav.json"),
        serde_json::to_string_pretty(&nav).unwrap(),
    )
    .unwrap();

    // 一条走鼠标点击：先解算布局拿 btn-start 的归一化中心，tick5 点它 → 切场景
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    sim.step(&mut rt).unwrap();
    let (nx, ny) = button_center_normalized(&sim, "btn-start");
    let click = record_playthrough(&[], &[(5, nx, ny)], 40);
    std::fs::write(
        dir.join("recordings/mouse-click.json"),
        serde_json::to_string_pretty(&click).unwrap(),
    )
    .unwrap();
}

#[test]
fn gate_passes_both_focus_and_click_playthroughs() {
    // 两条录像独立重放：逐校验点一致 + must_emit(game-started) 出现 + 长度合规。
    // 文件由 regen_gate_recordings 生成（提交进仓库）；这里只验 gate 绿。
    let dir = demo_dir();
    // 录像文件必须已生成（否则提示先跑生成器）
    assert!(
        dir.join("recordings/focus-nav.json").exists(),
        "缺 recordings/focus-nav.json，先跑 cargo test -- --ignored regen_gate_recordings"
    );
    let (report, pass) =
        vitric_cli::gate::run(&dir).expect("gate 应通过（双录像逐位重放一致 + game-started 出现）");
    assert!(pass, "gate 必须绿: {report}");
    assert_eq!(report["pass"], json!(true), "{report}");
    let gates = report["gates"].as_array().expect("gates 数组");
    // 两条通关录像（check 门也算一项，故 ≥2）
    let plays: Vec<_> = gates.iter().filter(|g| g["name"].as_str().unwrap_or("").starts_with("playthrough:")).collect();
    assert_eq!(plays.len(), 2, "两条录像都验过: {report}");
    for g in plays {
        assert_eq!(g["status"], json!("pass"), "每条录像逐位重放一致: {g}");
        assert_eq!(g["detail"]["must_emit"], json!("game-started"), "{g}");
        assert_eq!(g["detail"]["verified"], json!(true), "{g}");
    }
}
