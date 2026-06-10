//! 窗口呈现 — 人的驾驶舱。
//!
//! 呈现的就是 vitric-render 光栅化出的同一块像素（CPU 渲染，softbuffer 上屏），
//! 所见 = agent 截图所见，逐字节一致。键盘事件映射成 input 事件注入模拟，
//! 和控制面 `input/inject` 走同一条管道——人和 AI 是同级玩家。

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use vitric_control::{ControlServer, Dispatcher};
use vitric_sim::{Sim, DT};

use vitric_cli::runtime::Runtime;

use crate::step_once;

pub struct WindowedGame {
    pub sim: Sim,
    pub rt: Runtime,
    pub dispatcher: Dispatcher,
    pub server: ControlServer,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    last: Instant,
    acc: f64,
    pub error: Option<String>,
}

impl WindowedGame {
    pub fn new(sim: Sim, rt: Runtime, dispatcher: Dispatcher, server: ControlServer) -> Self {
        WindowedGame {
            sim,
            rt,
            dispatcher,
            server,
            window: None,
            surface: None,
            last: Instant::now(),
            acc: 0.0,
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
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return; // 最小化等零尺寸状态
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let rgba = match vitric_render::render_world(&self.sim.world, size.width, size.height) {
            Ok(buf) => buf,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let Ok(mut frame) = surface.buffer_mut() else {
            return;
        };
        // RGBA8 → softbuffer 的 0RGB u32
        for (dst, px) in frame.iter_mut().zip(rgba.chunks_exact(4)) {
            *dst = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
        }
        let _ = frame.present();
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if event.repeat {
            return; // 自动重复不进模拟，按住的语义靠 pressed/released 对
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
        let attrs = Window::default_attributes()
            .with_title("Vitric")
            .with_inner_size(LogicalSize::new(960.0, 540.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                self.error = Some(format!("窗口创建失败: {e}"));
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("softbuffer 上下文创建失败: {e}"));
                event_loop.exit();
                return;
            }
        };
        match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                self.error = Some(format!("softbuffer 表面创建失败: {e}"));
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        self.last = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(event),
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
                if let Err(e) = step_once(&mut self.sim, &mut self.rt, &mut self.dispatcher) {
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
