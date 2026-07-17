//! UI interaction (1.2) end-to-end: focus navigation + click activation + press feedback + theme
//! + dual-recording gate.
//!
//! Verified on top of 1.1 layout:
//! - Focus navigation: direction inputs move focus by layout-adjacency (assert the landing point
//!   step by step), confirm activation emits ui-activate, disabled is not focusable and is
//!   skipped;
//! - Click activation: screen-normalized coordinates are converted to the 1920×1080 reference
//!   frame to hit button rects (coordinate-conversion correctness assertions: hit inside the
//!   reference frame / miss outside the boundary / disabled does not respond);
//! - Press feedback: scale/modulate analytic per-value; snapshot/replay focus state consistent;
//! - Menu demo + dual-recording gate: one path via focus navigation, one via mouse click, both
//!   activate "Start" → emit game-started → the load-scene rule switches to the game scene; both
//!   replay bit-identical.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;
use vitric_render::{press_modulate, press_scale, PRESS_TICKS};
use vitric_sim::{GameLogic, Sim};

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui-menu")
}

/// The currently focused button name (UiRoot.focus).
fn focus(sim: &Sim) -> String {
    let root = sim.world.entity("ui").unwrap();
    sim.world.get_field(root, "UiRoot.focus").unwrap().as_str().unwrap_or("").to_string()
}

/// The state of a given button.
fn btn_state(sim: &Sim, name: &str) -> String {
    let id = sim.world.entity(name).unwrap();
    sim.world.get_field(id, "Button.state").unwrap().as_str().unwrap_or("").to_string()
}

/// Advance one tick (injected input takes effect on the next tick).
fn step(sim: &mut Sim, rt: &mut Runtime) {
    sim.step(rt).unwrap();
}

// ---- Focus navigation ----

#[test]
fn focus_navigation_moves_between_buttons_skipping_disabled() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    // tick 0: layout solves + the interaction system drops focus on the first focusable button
    // (btn-start, already focused in the scene)
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-start", "初始焦点在第一个可聚焦按钮");
    assert_eq!(btn_state(&sim, "btn-start"), "focused");
    assert_eq!(btn_state(&sim, "btn-options"), "disabled", "options 是 disabled");

    // Down: btn-options is disabled, skipped → straight to btn-quit
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-quit", "向下跳过 disabled 的 options 落到 quit");
    assert_eq!(btn_state(&sim, "btn-quit"), "focused");
    assert_eq!(btn_state(&sim, "btn-start"), "normal", "旧焦点回 normal");

    // Down again: at the bottom, no movement (no wraparound)
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-quit", "到底不环绕");

    // Up: back to btn-start (skipping disabled)
    sim.inject_input("ui-up", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(focus(&sim), "btn-start", "向上跳过 disabled 回 start");
}

#[test]
fn confirm_activates_focused_button_emitting_ui_activate() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // Focus lands on btn-start
    assert_eq!(focus(&sim), "btn-start");

    // Confirm: activate the current focus → the interaction system emits ui-activate (event
    // payload asserted by the next test); this tick btn-start enters the pressed state
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(btn_state(&sim, "btn-start"), "pressed", "确认后焦点按钮进 pressed");

    // Next tick: the rule receives ui-activate{action:start} → emit game-started + load-scene →
    // switch scene
    step(&mut sim, &mut rt);
    step(&mut sim, &mut rt);
    // After switching to the game scene, the menu entity is gone, the hero is present
    assert!(sim.world.entity("hero").is_ok(), "start 激活后应切到 game 场景（hero 存在）");
    assert!(sim.world.entity("btn-start").is_err(), "menu 场景已被推倒");
}

#[test]
fn ui_activate_event_carries_id_and_action() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    sim.inject_input("ui-confirm", "pressed");
    // The ui-activate emitted by the interaction system goes into observed (visible this tick)
    sim.step(&mut rt).unwrap();
    let observed = rt.drain_observed();
    let act = observed.iter().find(|e| e.name == "ui-activate").expect("应发 ui-activate");
    assert_eq!(act.data["action"], json!("start"));
    assert_eq!(act.data["id"], json!("btn-start"));
}

// ---- Click activation + coordinate-conversion correctness ----

/// Take a button's rect (rx/ry/rw/rh) in the 1920×1080 reference frame and compute its center
/// as normalized coordinates.
fn button_center_normalized(sim: &Sim, name: &str) -> (f64, f64) {
    let id = sim.world.entity(name).unwrap();
    let rx = sim.world.get_field(id, "Ui.rx").unwrap().as_f64().unwrap();
    let ry = sim.world.get_field(id, "Ui.ry").unwrap().as_f64().unwrap();
    let rw = sim.world.get_field(id, "Ui.rw").unwrap().as_f64().unwrap();
    let rh = sim.world.get_field(id, "Ui.rh").unwrap().as_f64().unwrap();
    // Reference-frame coords → normalized (divide by 1920/1080); this is the inverse the
    // injection side does
    ((rx + rw / 2.0) / 1920.0, (ry + rh / 2.0) / 1080.0)
}

#[test]
fn click_in_reference_space_hits_button_and_activates() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // Layout solves first, button rects go into rx/ry/rw/rh

    // Click the center of btn-quit (normalized coords): converted back to the reference frame it
    // should hit the quit rect → activate
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
    // Click the top-left corner (0,0) normalized — outside all button rects (buttons are centered)
    vitric_control::inject_ui_click(&mut sim, 0.001, 0.001, "left").unwrap();
    step(&mut sim, &mut rt);
    // No button activated (none are pressed)
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
    // 1 pixel outside the left edge of the rect (reference frame) → normalized → miss
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
    // Click the center of btn-options (disabled)
    let (nx, ny) = button_center_normalized(&sim, "btn-options");
    vitric_control::inject_ui_click(&mut sim, nx, ny, "left").unwrap();
    step(&mut sim, &mut rt);
    assert_eq!(btn_state(&sim, "btn-options"), "disabled", "disabled 按钮点击不响应（仍 disabled）");
}

// ---- Press feedback analytic ----

#[test]
fn press_feedback_is_analytic_and_recorded_in_component() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    // Activate btn-start: press_t starts from 0, advances tick by tick; scale/modulate is a pure
    // function of press_t
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt); // Activation tick: press_t=0
    let id = sim.world.entity("btn-start").unwrap();
    let pt = |sim: &Sim| sim.world.get_field(sim.world.entity("btn-start").unwrap(), "Button.press_t").unwrap().as_i64().unwrap();
    assert_eq!(pt(&sim), 0, "激活那 tick press_t=0");
    // press_scale(0)=1 (no shrink), deepest at the midpoint, back to 1 at the end
    assert_eq!(press_scale(0, 0.92), 1.0);
    assert!(press_scale(PRESS_TICKS / 2, 0.92) < 1.0);
    assert_eq!(press_scale(PRESS_TICKS, 0.92), 1.0);
    let _ = id;

    // Advance press_t tick by tick (note: activating start will trigger the scene-switch rule —
    // here we only inspect the activation-tick feedback, so switch to a button that does not
    // switch scenes: quit)
    let (mut sim2, mut rt2) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim2, &mut rt2);
    sim2.inject_input("ui-down", "pressed"); // Focus to quit
    step(&mut sim2, &mut rt2);
    sim2.inject_input("ui-confirm", "pressed"); // Activate quit (quit rule only emits app-quit, no scene switch)
    step(&mut sim2, &mut rt2);
    let qpt = |sim: &Sim| sim.world.get_field(sim.world.entity("btn-quit").unwrap(), "Button.press_t").unwrap().as_i64().unwrap();
    assert_eq!(qpt(&sim2), 0, "quit 激活 tick press_t=0");
    for expect in 1..PRESS_TICKS {
        step(&mut sim2, &mut rt2);
        assert_eq!(qpt(&sim2) as u64, expect, "press_t 逐 tick +1（解析式驱动 scale/modulate）");
    }
    // At the end: feedback ends, press_t returns to -1, state goes back to normal (quit no longer
    // has focus — focus is on quit but… after activation focus stays on quit, so it should go back
    // to focused)
    step(&mut sim2, &mut rt2);
    assert_eq!(qpt(&sim2), -1, "PRESS_TICKS 后反馈结束 press_t=-1");
    assert_eq!(btn_state(&sim2, "btn-quit"), "focused", "反馈结束回 focused（焦点仍在它身上）");
    // modulate analytic symmetric
    assert_eq!(press_modulate(0), 0.0);
    assert!((press_modulate(PRESS_TICKS / 2) - 1.0).abs() < 1e-9);
}

#[test]
fn snapshot_restore_preserves_focus_and_press_state() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    // Move focus to quit + activate, to produce a focus state + pressed state
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt);
    let snap = sim.snapshot(&rt);
    let hash_before = sim.world.state_hash();
    assert_eq!(focus(&sim), "btn-quit");
    assert_eq!(btn_state(&sim, "btn-quit"), "pressed");

    // Keep running a few ticks (press_t advances)
    for _ in 0..3 {
        step(&mut sim, &mut rt);
    }
    assert_ne!(sim.world.state_hash(), hash_before, "跑几 tick 后哈希应变");

    // Restore to the snapshot: focus state + press_t fully back to the save moment
    sim.restore(&snap, &mut rt).unwrap();
    assert_eq!(sim.world.state_hash(), hash_before, "restore 后哈希必须逐位回到存档时刻");
    assert_eq!(focus(&sim), "btn-quit");
    assert_eq!(btn_state(&sim, "btn-quit"), "pressed");

    // Continued playback consistent: run the same number of ticks again, matching the first run
    // (deterministic continuation)
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

// ---- Theme application + render press feedback ----

fn rt_assets() -> &'static vitric_render::Assets {
    let mut a = vitric_render::Assets::load_dir(&demo_dir().join("assets")).unwrap();
    a.load_font(&demo_dir().join("fonts/DejaVuSans.ttf")).unwrap();
    Box::leak(Box::new(a))
}

#[test]
fn theme_resolves_button_state_into_panel_color() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // Focus lands on btn-start; the theme writes the focused bg into Panel.color
    let start = sim.world.entity("btn-start").unwrap();
    let color = |sim: &Sim, id| sim.world.get_field(id, "Panel.color").unwrap().as_str().unwrap().to_string();
    // btn-start focused → theme dark's button.focused.bg = #5a7bb5
    assert_eq!(color(&sim, start), "#5a7bb5", "focused 按钮底色来自主题 focused 样式");
    // btn-options disabled → button.disabled.bg = #2a2d36
    let opt = sim.world.entity("btn-options").unwrap();
    assert_eq!(color(&sim, opt), "#2a2d36", "disabled 按钮底色来自主题 disabled 样式");
    // btn-quit normal → button.normal.bg = #3a4a6b
    let quit = sim.world.entity("btn-quit").unwrap();
    assert_eq!(color(&sim, quit), "#3a4a6b", "normal 按钮底色来自主题 normal 样式");

    // Move focus to quit: quit becomes focused bg, start goes back to normal bg
    sim.inject_input("ui-down", "pressed");
    step(&mut sim, &mut rt);
    assert_eq!(color(&sim, quit), "#5a7bb5", "焦点移过来后 quit 是 focused 底色");
    assert_eq!(color(&sim, start), "#3a4a6b", "start 失焦回 normal 底色");
}

#[test]
fn press_feedback_shrinks_and_brightens_rendered_button() {
    // Render-layer proof of press feedback: the button at the press midpoint is both shrunk and
    // brightened in the image (pure-function decoration, reads press_t).
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt);
    sim.inject_input("ui-down", "pressed"); // Focus to quit (quit does not switch scenes)
    step(&mut sim, &mut rt);
    sim.inject_input("ui-confirm", "pressed");
    step(&mut sim, &mut rt); // quit activated, press_t=0

    let (w, h) = (960u32, 540u32);
    // Advance to the press-feedback midpoint (press_t ≈ PRESS_TICKS/2, deepest scale/modulate)
    for _ in 0..(PRESS_TICKS / 2) {
        step(&mut sim, &mut rt);
    }
    let quit = sim.world.entity("btn-quit").unwrap();
    let pt = sim.world.get_field(quit, "Button.press_t").unwrap().as_i64().unwrap();
    assert!(pt > 0 && (pt as u64) < PRESS_TICKS, "应在反馈中途，press_t={pt}");

    // quit's reference-frame rect → screen rect at 960x540 (draw_ui re-solves layout at the real
    // resolution)
    let layout = vitric_render::solve_layout(&sim.world, w, h).unwrap();
    let r = *layout.get(&quit).unwrap();
    // Center pixel of the rect: at the press midpoint the bg is pressed(#9fc0f0) further brightened
    // by modulate — brighter than the un-brightened value
    let cx = (r.x + r.w / 2.0) as u32;
    let cy = (r.y + r.h / 2.0) as u32;
    let buf = vitric_render::render_world(&sim.world, w, h, rt_assets(), sim.tick).unwrap();
    let center = ((cy * w + cx) * 4) as usize;
    let px = [buf[center], buf[center + 1], buf[center + 2]];
    // pressed bg #9fc0f0 = (159,192,240); modulate pushes toward white, all channels should be ≥
    // the original
    assert!(px[0] >= 159 && px[1] >= 192 && px[2] >= 240, "按下中点应被提亮（≥pressed 底色）: {px:?}");

    // Scale proof: a point just inside the rect's edge (after shrinking, the background/panel is
    // exposed there, no longer the button color).
    // Take 1px inside the rect's left edge — without scaling it is the button color, after scaling
    // it is pulled in and the base shows through.
    let edge = (cy * w + (r.x as u32 + 1)) as usize * 4;
    let epx = [buf[edge], buf[edge + 1], buf[edge + 2]];
    // After shrinking, the edge pixel is not the brightened pure button color (panel base #12141c
    // or darker shows through)
    assert!(epx != px, "缩放后左边缘像素应与中心不同（按钮缩进去了）: edge={epx:?} center={px:?}");
}

// ---- Performance / determinism: empty UI zero-cost, static UI no stray writes ----

#[test]
fn idle_ui_without_input_does_not_change_state_hash() {
    // Performance/determinism: a static UI (no input at all) played for N consecutive ticks — the
    // interaction system must not write components arbitrarily. The state hash stays unchanged
    // once focus has settled (when no press feedback is in flight, press_t does not move).
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();
    step(&mut sim, &mut rt); // Focus settles + layout solves
    let settled = sim.world.state_hash();
    for _ in 0..30 {
        step(&mut sim, &mut rt);
        assert_eq!(sim.world.state_hash(), settled, "静止 UI 无输入连播，状态哈希必须不变");
    }
}

/// Empty UI (no UiRoot): the interaction system is zero-cost early-return (directly driving
/// advance_ui_interaction).
#[test]
fn empty_ui_interaction_is_zero_cost() {
    use vitric_cli::runtime::advance_ui_interaction;
    use vitric_ecs::World;
    let mut w = World::new();
    // No UiRoot: returns an empty event list, zero allocations, zero traversal (no error, no
    // world mutation)
    let before = w.state_hash();
    let events = advance_ui_interaction(&mut w, &[], (1920, 1080)).unwrap();
    assert!(events.is_empty(), "空 UI 不发任何 ui-activate");
    assert_eq!(w.state_hash(), before, "空 UI 不改世界");
}

// ---- Recording generation + dual-gate replay ----

/// Record one run: boot demo → inject sequence → run to completion → write recording file.
/// Both recordings activate "Start" → emit game-started → the load-scene rule switches to the
/// game scene.
/// The recording stores inputs (ui-down/up/confirm) and replies (ui-click normalized coords);
/// replay injects them verbatim from the recording — offline replay is bit-identical (the body
/// of the gate dual-recording certification).
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

/// Generator: write two gate recordings to examples/ui-menu/recordings/.
/// Normally `#[ignore]`; after changing interaction logic run
/// `cargo test -- --ignored regen_gate_recordings` once to regenerate (the hash changes with
/// deterministic logic and must be updated alongside the code).
#[test]
#[ignore = "录像生成器：改交互逻辑后手动跑一次重生成 gate 录像"]
fn regen_gate_recordings() {
    let dir = demo_dir();
    // One via focus navigation: tick5 down (skip disabled to quit), tick10 up back to start,
    // tick15 confirm → scene switch
    let nav = record_playthrough(&[(5, "ui-down"), (10, "ui-up"), (15, "ui-confirm")], &[], 40);
    std::fs::write(
        dir.join("recordings/focus-nav.json"),
        serde_json::to_string_pretty(&nav).unwrap(),
    )
    .unwrap();

    // One via mouse click: first solve layout to get btn-start's normalized center, tick5 click it
    // → scene switch
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
    // The two recordings replay independently: every checkpoint is consistent + must_emit
    // (game-started) appears + length is compliant.
    // Files are produced by regen_gate_recordings (committed to the repo); here we only verify
    // the gate is green.
    let dir = demo_dir();
    // Recording files must already exist (otherwise prompt to run the generator first)
    assert!(
        dir.join("recordings/focus-nav.json").exists(),
        "缺 recordings/focus-nav.json，先跑 cargo test -- --ignored regen_gate_recordings"
    );
    let (report, pass) =
        vitric_cli::gate::run(&dir).expect("gate 应通过（双录像逐位重放一致 + game-started 出现）");
    assert!(pass, "gate 必须绿: {report}");
    assert_eq!(report["pass"], json!(true), "{report}");
    let gates = report["gates"].as_array().expect("gates 数组");
    // Two clear recordings (the check gate is also one entry, so ≥2)
    let plays: Vec<_> = gates.iter().filter(|g| g["name"].as_str().unwrap_or("").starts_with("playthrough:")).collect();
    assert_eq!(plays.len(), 2, "两条录像都验过: {report}");
    for g in plays {
        assert_eq!(g["status"], json!("pass"), "每条录像逐位重放一致: {g}");
        assert_eq!(g["detail"]["must_emit"], json!("game-started"), "{g}");
        assert_eq!(g["detail"]["verified"], json!(true), "{g}");
    }
}
