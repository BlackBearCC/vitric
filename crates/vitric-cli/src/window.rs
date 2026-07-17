//! Window presentation — the human's cockpit.
//!
//! Two presentation paths, selected via `--renderer`:
//! - cpu (default): the same pixel buffer rasterized by vitric-render goes to screen
//!   via softbuffer; what you see = what the agent screenshot sees, byte for byte
//! - gpu: wgpu drives the GPU, reads the same component conventions, visually
//!   semantically aligned with the CPU path (see gpu.rs); on init failure it errors
//!   out and exits, never silently falls back — when the user should switch to
//!   --renderer cpu that must be said explicitly
//!
//! Keyboard events are mapped to input events injected into the simulation, going
//! through the same pipe as the control plane `input/inject`; mouse left/right
//! buttons map to `mouse` / `mouse-alt` events (world coordinates + pick result,
//! recorded and replayable via the reply channel), going through the same pipe as
//! the control plane `input/click` — human and AI are peer players. The left
//! button also still drives inspector select/drag (one click, two meanings; the
//! inspector only exists in window mode, the game can ignore it if not wanted).
//! Mouse pick/drag does not depend on the presentation path, both sides behave
//! identically.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use vitric_control::{ControlServer, Dispatcher};
use vitric_sim::{Sim, DT};

use vitric_cli::llm::Llm;
use vitric_cli::runtime::Runtime;

use crate::audio::Audio;
use crate::gpu::GpuPresenter;
use crate::step_once;

/// Unified render/input reference resolution. UI layout and click picking are
/// based on 1920x1080, so any window size first renders to this resolution then
/// scales up; the cursor also maps into this space - guarantees UI doesn't shrink
/// to a corner on 4K displays and world clicks stay aligned.
const REF_W: u32 = 1920;
const REF_H: u32 = 1080;

/// Presentation path selection (from `--renderer`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    Cpu,
    Gpu,
}

/// Ready presenter (can only be built after the window is created). GpuPresenter is large, boxed to flatten variant size differences.
enum Presenter {
    Cpu(softbuffer::Surface<Arc<Window>, Arc<Window>>),
    Gpu(Box<GpuPresenter>),
}

pub struct WindowedGame {
    pub sim: Sim,
    pub rt: Runtime,
    pub dispatcher: Dispatcher,
    pub server: ControlServer,
    audio: Option<Audio>,
    llm: Llm,
    renderer: Renderer,
    /// Window title (project name — Vitric): the game needs its own name in the taskbar/window switcher.
    title: String,
    window: Option<Arc<Window>>,
    presenter: Option<Presenter>,
    last: Instant,
    acc: f64,
    /// Current mouse position (physical pixels).
    cursor: (f64, f64),
    /// While dragging: the dragged entity + the world offset of the grab point relative to the entity center.
    drag: Option<(vitric_ecs::EntityId, f64, f64)>,
    pub error: Option<String>,
}

impl WindowedGame {
    #[allow(clippy::too_many_arguments)] // assemble function, only one call site: cmd_run
    pub fn new(
        sim: Sim,
        rt: Runtime,
        dispatcher: Dispatcher,
        server: ControlServer,
        audio: Option<Audio>,
        llm: Llm,
        renderer: Renderer,
        title: String,
    ) -> Self {
        WindowedGame {
            sim,
            rt,
            dispatcher,
            server,
            audio,
            llm,
            renderer,
            title,
            window: None,
            presenter: None,
            last: Instant::now(),
            acc: 0.0,
            cursor: (0.0, 0.0),
            drag: None,
            error: None,
        }
    }

    /// Open the window and run until exit (window close / sim/quit / logic error).
    pub fn run(mut self) -> Result<(Sim, Option<String>), String> {
        let event_loop = EventLoop::new().map_err(|e| format!("窗口事件循环创建失败: {e}"))?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop
            .run_app(&mut self)
            .map_err(|e| format!("窗口事件循环异常退出: {e}"))?;
        Ok((self.sim, self.error))
    }

    fn draw(&mut self) {
        let (Some(window), Some(presenter)) = (&self.window, &mut self.presenter) else {
            return;
        };
        let size = window.inner_size();
        match presenter {
            Presenter::Cpu(surface) => {
                let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return; // minimized, zero-size state
                };
                if surface.resize(w, h).is_err() {
                    return;
                }
                // Always render at the 1920x1080 reference resolution, then stretch to fill
                // the window. UI layout / click picking both use 1920x1080 as the reference
                // frame; the render must lock to the same reference frame to stay aligned --
                // otherwise at 4K and similar resolutions fixed-pixel UI would shrink into a
                // tiny corner and world clicks would misalign. self.cursor is already mapped
                // to the reference space in CursorMoved, and viewport() returns reference
                // dimensions, so the whole chain stays consistent.
                let mut rgba = match vitric_render::render_world(
                    &self.sim.world,
                    REF_W,
                    REF_H,
                    self.dispatcher.assets(),
                    self.sim.tick,
                ) {
                    Ok(buf) => buf,
                    Err(e) => {
                        self.error = Some(e);
                        return;
                    }
                };
                // Inspector highlight: draw a cyan outline on the selected entity (set by human click or AI inspect/select)
                if let Some(selected) = self.dispatcher.selection() {
                    let _ = vitric_render::draw_selection_outline(
                        &mut rgba,
                        &self.sim.world,
                        REF_W,
                        REF_H,
                        selected,
                        self.sim.tick,
                    );
                }
                // Build placement preview: in build mode with a selected kind, draw a translucent green ghost on the tile under the cursor.
                if let Ok(uistate) = self.sim.world.entity("uistate") {
                    let mode = self
                        .sim
                        .world
                        .get_field(uistate, "Mode.value")
                        .ok()
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let kind = self
                        .sim
                        .world
                        .get_field(uistate, "Build.kind")
                        .ok()
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if mode == "build" && !kind.is_empty() {
                        let (px, py) = self.cursor;
                        // Skip placement preview when the cursor is over a menu/panel (only show over the map).
                        if !vitric_render::point_over_ui(&self.sim.world, REF_W, REF_H, px, py) {
                            if let Ok((wx, wy)) = vitric_render::screen_to_world(
                                &self.sim.world,
                                REF_W,
                                REF_H,
                                px,
                                py,
                            ) {
                                let _ = vitric_render::draw_build_preview(
                                    &mut rgba,
                                    &self.sim.world,
                                    REF_W,
                                    REF_H,
                                    wx,
                                    wy,
                                    self.sim.tick,
                                );
                            }
                        }
                    }
                }
                let Ok(mut frame) = surface.buffer_mut() else {
                    return;
                };
                // Nearest-neighbor stretch the 1920x1080 frame to the window size -> softbuffer 0RGB u32.
                // When aspect ratios differ, scale each axis independently (slight stretch) to
                // prioritize filling the screen + keep render/click consistent.
                let (sw, sh) = (size.width, size.height);
                for y in 0..sh {
                    let sy = (y as u64 * REF_H as u64 / sh as u64) as u32;
                    let srow = (sy * REF_W) as usize * 4;
                    let drow = (y * sw) as usize;
                    for x in 0..sw {
                        let sx = (x as u64 * REF_W as u64 / sw as u64) as u32;
                        let si = srow + sx as usize * 4;
                        frame[drow + x as usize] = ((rgba[si] as u32) << 16)
                            | ((rgba[si + 1] as u32) << 8)
                            | rgba[si + 2] as u32;
                    }
                }
                let _ = frame.present();
            }
            Presenter::Gpu(gpu) => {
                // Sizing / hot-reload are handled inside present (resize reconfigures the surface; generation change rebuilds the atlas)
                if let Err(e) = gpu.present(
                    &self.sim.world,
                    self.dispatcher.assets(),
                    self.dispatcher.assets_generation(),
                    self.dispatcher.selection(),
                    self.sim.tick,
                ) {
                    self.error = Some(e);
                }
            }
        }
    }

    fn viewport(&self) -> Option<(u32, u32)> {
        // The viewport used for input is always the reference resolution (render locks to the same reference frame) -- the cursor is already mapped to this space.
        let size = self.window.as_ref()?.inner_size();
        (size.width > 0 && size.height > 0).then_some((REF_W, REF_H))
    }

    /// Left button down: one click, two meanings —
    /// 1. Game input: inject a `mouse` event (world coordinates + pick result, recorded
    ///    and replayable via the reply channel);
    /// 2. Inspector: pick an entity (hit starts a drag, empty space clears the selection).
    ///    Games that don't want inspector behavior can ignore the selection state — the
    ///    inspector only exists in window mode.
    fn mouse_down(&mut self) {
        let Some((w, h)) = self.viewport() else { return };
        let (px, py) = self.cursor;
        self.inject_mouse(w, h, "left");
        // Skip world picking/drag when the cursor is over UI (otherwise it would grab world entities behind the menu).
        if vitric_render::point_over_ui(&self.sim.world, w, h, px, py) {
            return;
        }
        match vitric_render::pick(&self.sim.world, w, h, px, py) {
            Ok(Some(id)) => {
                self.dispatcher.set_selection(Some(id));
                // Record grab offset so the entity doesn't jump to the cursor during drag
                if let (Ok((wx, wy)), Ok(ex), Ok(ey)) = (
                    vitric_render::screen_to_world(&self.sim.world, w, h, px, py),
                    self.sim.world.get_field(id, "Position.x").map(|v| v.as_f64().unwrap_or(0.0)),
                    self.sim.world.get_field(id, "Position.y").map(|v| v.as_f64().unwrap_or(0.0)),
                ) {
                    self.drag = Some((id, ex - wx, ey - wy));
                }
            }
            Ok(None) => self.dispatcher.set_selection(None),
            Err(_) => {}
        }
    }

    /// Right button down: only inject a `mouse-alt` event (same payload as `mouse`), does not touch the inspector.
    fn mouse_alt_down(&mut self) {
        let Some((w, h)) = self.viewport() else { return };
        self.inject_mouse(w, h, "right");
    }

    /// Translate a click at the cursor into world coordinates + pick result, and inject
    /// it into the simulation via the reply channel (same path as the control plane
    /// `input/click` — human and AI are peer players).
    /// Coordinates use the non-jittering camera (screen_to_world): clicks target the
    /// world itself, jitter is only visual decoration.
    fn inject_mouse(&mut self, w: u32, h: u32, button: &str) {
        let (px, py) = self.cursor;
        // When the cursor is over UI (menu/panel), this click belongs to the UI -- don't
        // inject it into the world, to avoid clicking through to the map below (previously
        // clicking the build menu would also trigger a world click on the tile beneath it).
        let over_ui = vitric_render::point_over_ui(&self.sim.world, w, h, px, py);
        if !over_ui {
            match vitric_render::screen_to_world(&self.sim.world, w, h, px, py) {
                Ok((wx, wy)) => {
                    if let Err(e) = vitric_control::inject_click(&mut self.sim, wx, wy, button) {
                        eprintln!("[vitric] 鼠标点击注入失败: {e}");
                    }
                }
                Err(e) => eprintln!("[vitric] 鼠标点击坐标换算失败: {e}"),
            }
        }
        // When there is UI on screen, the same click additionally injects a UI click
        // (screen-normalized coordinates; picking is deferred to inside the tick where
        // it is converted to the 1920×1080 reference frame and tested against UI rects).
        // World clicks and UI clicks use two coexisting coordinate systems: world
        // clicks pick Sprites (through the camera), UI clicks pick screen-space
        // overlays (no camera).
        if w > 0 && h > 0 && vitric_render::has_ui(&self.sim.world) {
            let (nx, ny) = (px / w as f64, py / h as f64);
            if let Err(e) = vitric_control::inject_ui_click(&mut self.sim, nx, ny, button) {
                eprintln!("[vitric] UI 点击注入失败: {e}");
            }
        }
    }

    /// Drag: write the selected entity's Position back to the data layer — human tweaks are immediately visible to the AI.
    fn mouse_drag(&mut self) {
        // Recording only captures the input stream; dragging writes Position which would not be recorded, breaking replay — disabled while recording
        if self.sim.is_recording() {
            if self.drag.take().is_some() {
                eprintln!("[vitric] 正在录像，检查器拖拽已禁用（拖动不进录像，会让录像不可重放）");
            }
            return;
        }
        let Some((id, off_x, off_y)) = self.drag else { return };
        if !self.sim.world.is_alive(id) {
            self.drag = None;
            return;
        }
        let Some((w, h)) = self.viewport() else { return };
        let (px, py) = self.cursor;
        if let Ok((wx, wy)) = vitric_render::screen_to_world(&self.sim.world, w, h, px, py) {
            let _ = self.sim.world.set_field(id, "Position.x", serde_json::json!(wx + off_x));
            let _ = self.sim.world.set_field(id, "Position.y", serde_json::json!(wy + off_y));
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if event.repeat {
            return; // auto-repeat does not enter the simulation; hold semantics rely on pressed/released pairs
        }
        // F11 = window command (toggle borderless fullscreen), does not enter the simulation input stream
        if event.physical_key == PhysicalKey::Code(KeyCode::F11) {
            if event.state == ElementState::Pressed {
                if let Some(w) = &self.window {
                    let fs = if w.fullscreen().is_some() {
                        None
                    } else {
                        Some(winit::window::Fullscreen::Borderless(None))
                    };
                    w.set_fullscreen(fs);
                }
            }
            return;
        }
        let Some(action) = key_action(&event) else {
            return;
        };
        let phase = match event.state {
            ElementState::Pressed => "pressed",
            ElementState::Released => "released",
        };
        self.sim.inject_input(&action, phase);
        // When there is UI on screen (UiRoot), arrow keys / Enter additionally inject
        // standard UI navigation actions (ui-up/down/left/right/confirm) — the
        // interaction system only honors the ui-* prefix, the game's own left/jump is
        // unaffected. Both go into the input stream and into the recording, replay is
        // consistent.
        if vitric_render::has_ui(&self.sim.world) {
            let ui_action = match action.as_str() {
                "up" => Some("ui-up"),
                "down" => Some("ui-down"),
                "left" => Some("ui-left"),
                "right" => Some("ui-right"),
                "enter" => Some("ui-confirm"),
                _ => None,
            };
            if let Some(ua) = ui_action {
                self.sim.inject_input(ua, phase);
            }
        }
    }
}

/// Key → action name. Arrow keys / space etc. use semantic names; letters/digits use the character itself (lowercase).
fn key_action(event: &KeyEvent) -> Option<String> {
    if let PhysicalKey::Code(code) = event.physical_key {
        let named = match code {
            KeyCode::ArrowLeft => Some("left"),
            KeyCode::ArrowRight => Some("right"),
            KeyCode::ArrowUp => Some("up"),
            KeyCode::ArrowDown => Some("down"),
            KeyCode::Space => Some("space"),
            KeyCode::Enter => Some("enter"),
            KeyCode::Escape => Some("escape"),
            KeyCode::ShiftLeft | KeyCode::ShiftRight => Some("shift"),
            _ => None,
        };
        if let Some(name) = named {
            return Some(name.to_string());
        }
    }
    match &event.logical_key {
        Key::Character(c) => Some(c.to_lowercase()),
        Key::Named(NamedKey::Tab) => Some("tab".to_string()),
        _ => None,
    }
}

impl ApplicationHandler for WindowedGame {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Default borderless fullscreen: for real-machine play, UI needs to fill the
        // screen at the 1920x1080 reference viewport to be readable.
        // F11 still toggles windowed (fallback windowed size is 1920x1080, not the old 960x540).
        // (Remote desktop / recording doesn't use this window path - they use render/screenshot
        // offscreen rendering, unaffected.)
        let attrs = Window::default_attributes()
            .with_title(self.title.as_str())
            .with_active(true)
            .with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)))
            .with_inner_size(LogicalSize::new(1920.0, 1080.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("窗口创建失败: {e}"));
                event_loop.exit();
                return;
            }
        };
        // Grab keyboard focus on startup: when launched from cmd/bat, focus often stays
        // on that console window, and the fullscreen game on top can't receive key presses
        // -- here we actively focus once, so the player doesn't have to click first.
        window.focus_window();
        match self.renderer {
            Renderer::Cpu => {
                let context = match softbuffer::Context::new(window.clone()) {
                    Ok(c) => c,
                    Err(e) => {
                        self.error = Some(format!("softbuffer 上下文创建失败: {e}"));
                        event_loop.exit();
                        return;
                    }
                };
                match softbuffer::Surface::new(&context, window.clone()) {
                    Ok(s) => self.presenter = Some(Presenter::Cpu(s)),
                    Err(e) => {
                        self.error = Some(format!("softbuffer 表面创建失败: {e}"));
                        event_loop.exit();
                        return;
                    }
                }
            }
            Renderer::Gpu => {
                // Failure = exit and state the way out explicitly; never silently switch to the CPU path (behavior would change without the user knowing)
                match GpuPresenter::new(
                    window.clone(),
                    self.dispatcher.assets(),
                    self.dispatcher.assets_generation(),
                ) {
                    Ok(p) => self.presenter = Some(Presenter::Gpu(Box::new(p))),
                    Err(e) => {
                        self.error = Some(format!(
                            "GPU 渲染初始化失败: {e}。\
                             本机没有可用 GPU/驱动时请改用 --renderer cpu"
                        ));
                        event_loop.exit();
                        return;
                    }
                }
            }
        }
        self.window = Some(window);
        self.last = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                // Esc: if something is selected, deselect first; otherwise quit the game
                // (real-machine play needs one-key exit).
                if matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape))
                    && event.state == ElementState::Pressed
                {
                    if self.dispatcher.selection().is_some() {
                        self.dispatcher.set_selection(None);
                        self.drag = None;
                    } else {
                        event_loop.exit();
                    }
                    return;
                }
                self.handle_key(event)
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Store the cursor in 1920x1080 reference space (consistent with render/pick): scale by the actual window size.
                self.cursor = match self.window.as_ref().map(|w| w.inner_size()) {
                    Some(s) if s.width > 0 && s.height > 0 => (
                        position.x * REF_W as f64 / s.width as f64,
                        position.y * REF_H as f64 / s.height as f64,
                    ),
                    _ => (position.x, position.y),
                };
                self.mouse_drag();
            }
            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, winit::event::MouseButton::Left) => self.mouse_down(),
                (ElementState::Released, winit::event::MouseButton::Left) => self.drag = None,
                (ElementState::Pressed, winit::event::MouseButton::Right) => self.mouse_alt_down(),
                _ => {}
            },
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Frame boundary: handle control plane requests (in window mode the AI still has full authority)
        for req in self.server.drain() {
            let resp = self.dispatcher.handle(&req.request, &mut self.sim, &mut self.rt);
            req.respond(resp);
        }
        if self.dispatcher.ctl.quit {
            event_loop.exit();
            return;
        }

        if self.dispatcher.ctl.paused {
            self.last = Instant::now();
            self.acc = 0.0;
        } else {
            let now = Instant::now();
            self.acc += now.duration_since(self.last).as_secs_f64() * self.dispatcher.ctl.speed;
            self.last = now;
            let mut budget = 8;
            while self.acc >= DT && budget > 0 {
                if let Err(e) = step_once(
                    &mut self.sim,
                    &mut self.rt,
                    &mut self.dispatcher,
                    &mut self.audio,
                    &mut self.llm,
                ) {
                    self.error = Some(e);
                    event_loop.exit();
                    return;
                }
                self.acc -= DT;
                budget -= 1;
            }
            if budget == 0 {
                self.acc = 0.0;
            }
        }

        if let Some(window) = &self.window {
            window.request_redraw();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
