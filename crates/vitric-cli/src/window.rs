//! 窗口呈现 — 人的驾驶舱。
//!
//! 两条上屏路径，由 `--renderer` 选：
//! - cpu（默认）：vitric-render 光栅化的同一块像素经 softbuffer 上屏，
//!   所见 = agent 截图所见，逐字节一致
//! - gpu：wgpu 走显卡，读同一套组件约定，视觉语义对齐 CPU 路径（见 gpu.rs）；
//!   初始化失败直接报错退出，不静默回退——用户该换 --renderer cpu 时要明说
//!
//! 键盘事件映射成 input 事件注入模拟，和控制面 `input/inject` 走同一条管道；
//! 鼠标左/右键映射成 `mouse` / `mouse-alt` 事件（世界坐标 + 拾取结果，经回复
//! 通道被录像、可重放），和控制面 `input/click` 走同一条管道——人和 AI 是
//! 同级玩家。左键同时照旧驱动检查器点选/拖拽（同一击两个含义，检查器只在
//! 窗口模式存在，游戏不想要可忽略）。鼠标点选/拖拽不依赖呈现路径，两边行为一致。

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

/// 上屏路径选择（来自 `--renderer`）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    Cpu,
    Gpu,
}

/// 已就绪的呈现器（窗口创建后才能建）。GpuPresenter 体积大，装箱压平变体差异。
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
    /// 窗口标题（项目名 — Vitric）：任务栏/切窗里游戏要有自己的名字。
    title: String,
    window: Option<Arc<Window>>,
    presenter: Option<Presenter>,
    last: Instant,
    acc: f64,
    /// 鼠标当前位置（物理像素）。
    cursor: (f64, f64),
    /// 拖拽中：被拖实体 + 抓取点相对实体中心的世界偏移。
    drag: Option<(vitric_ecs::EntityId, f64, f64)>,
    pub error: Option<String>,
}

impl WindowedGame {
    #[allow(clippy::too_many_arguments)] // 装配函数，调用点只有 cmd_run 一处
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

    /// 开窗口跑到退出（关窗 / sim/quit / 逻辑出错）。
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
                    return; // 最小化等零尺寸状态
                };
                if surface.resize(w, h).is_err() {
                    return;
                }
                let rgba = match vitric_render::render_world(
                    &self.sim.world,
                    size.width,
                    size.height,
                    self.dispatcher.assets(),
                    self.sim.tick,
                ) {
                    Ok(buf) => buf,
                    Err(e) => {
                        self.error = Some(e);
                        return;
                    }
                };
                let mut rgba = rgba;
                // 检查器高亮：选中实体画青色描边（人点的或 AI inspect/select 设的）
                if let Some(selected) = self.dispatcher.selection() {
                    let _ = vitric_render::draw_selection_outline(
                        &mut rgba,
                        &self.sim.world,
                        size.width,
                        size.height,
                        selected,
                        self.sim.tick,
                    );
                }
                let Ok(mut frame) = surface.buffer_mut() else {
                    return;
                };
                // RGBA8 → softbuffer 的 0RGB u32
                for (dst, px) in frame.iter_mut().zip(rgba.chunks_exact(4)) {
                    *dst = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
                }
                let _ = frame.present();
            }
            Presenter::Gpu(gpu) => {
                // 尺寸/热重载都在 present 内部处理（resize 重配表面、代次变了重建图集）
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
        let size = self.window.as_ref()?.inner_size();
        (size.width > 0 && size.height > 0).then_some((size.width, size.height))
    }

    /// 左键按下：同一次点击两个含义——
    /// 1. 游戏输入：注入 `mouse` 事件（世界坐标 + 拾取结果，经回复通道被录像、可重放）；
    /// 2. 检查器：点选实体（命中开始拖拽，空地取消选中）。
    ///    游戏不想要检查器行为可忽略选中态——检查器只在窗口模式存在。
    fn mouse_down(&mut self) {
        let Some((w, h)) = self.viewport() else { return };
        let (px, py) = self.cursor;
        self.inject_mouse(w, h, "left");
        match vitric_render::pick(&self.sim.world, w, h, px, py) {
            Ok(Some(id)) => {
                self.dispatcher.set_selection(Some(id));
                // 记抓取偏移，拖拽时实体不跳心
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

    /// 右键按下：只注入 `mouse-alt` 事件（payload 同 `mouse`），不动检查器。
    fn mouse_alt_down(&mut self) {
        let Some((w, h)) = self.viewport() else { return };
        self.inject_mouse(w, h, "right");
    }

    /// 把光标处的点击翻译成世界坐标 + 拾取结果，经回复通道注入模拟
    /// （和控制面 `input/click` 同一条路径——人和 AI 是同级玩家）。
    /// 坐标用不抖的相机（screen_to_world）：点击对的是世界本体，抖动只是视觉装饰。
    fn inject_mouse(&mut self, w: u32, h: u32, button: &str) {
        let (px, py) = self.cursor;
        match vitric_render::screen_to_world(&self.sim.world, w, h, px, py) {
            Ok((wx, wy)) => {
                if let Err(e) = vitric_control::inject_click(&mut self.sim, wx, wy, button) {
                    eprintln!("[vitric] 鼠标点击注入失败: {e}");
                }
            }
            Err(e) => eprintln!("[vitric] 鼠标点击坐标换算失败: {e}"),
        }
    }

    /// 拖拽：把选中实体的 Position 写回数据层——人的微调 AI 立刻可见。
    fn mouse_drag(&mut self) {
        // 录像只记输入流，拖拽写 Position 不会进录像，会让录像不可重放——录制中禁用
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
            return; // 自动重复不进模拟，按住的语义靠 pressed/released 对
        }
        // F11 = 窗口命令(切无边框全屏),不进模拟输入流
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
    }
}

/// 按键 → 动作名。方向键/空格等用语义名，字母数字用字符本身（小写）。
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
        // 默认 960x540 窗口:远程桌面/录屏场景全屏暗色渐变是编码最差情况,
        // 像素越多越卡;要大屏自己最大化或 F11 无边框全屏
        let attrs = Window::default_attributes()
            .with_title(self.title.as_str())
            .with_inner_size(LogicalSize::new(960.0, 540.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("窗口创建失败: {e}"));
                event_loop.exit();
                return;
            }
        };
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
                // 失败 = 退出并明说出路，绝不静默换 CPU 路径跑（行为变了用户却不知道）
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
                // Esc 优先作检查器的取消选中；没有选中时才进游戏输入
                if matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape))
                    && event.state == ElementState::Pressed
                    && self.dispatcher.selection().is_some()
                {
                    self.dispatcher.set_selection(None);
                    self.drag = None;
                    return;
                }
                self.handle_key(event)
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
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
        // 帧边界：处理控制面请求（窗口模式下 AI 照样全权操作）
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
