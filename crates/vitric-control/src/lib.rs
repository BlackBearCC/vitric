//! vitric-control — AI 控制面：引擎进程内建的调试端口。
//!
//! 这是 Vitric 的招牌差异化：任何 agent 通过 HTTP JSON-RPC 就能
//! **看**（查任意实体/组件/事件）、**动**（注入输入、改状态）、
//! **控时间**（暂停/单步/倍速）、**测**（注册断言，违反即上报）。
//!
//! 架构：HTTP 服务线程只做传输（解析请求 → 塞进通道 → 等回应）；
//! 命令由游戏主循环在**帧边界**统一执行——控制面永远不破坏确定性。
//!
//! 协议：`POST /rpc` 单对象 `{"method": "...", "params": {...}}`，
//! 响应 `{"ok": true, "result": ...}` 或 `{"ok": false, "error": "..."}`。

mod dispatcher;
pub mod saves;
mod server;

pub use dispatcher::{Dispatcher, LoopCtl};
pub use saves::SaveStore;
pub use server::{ControlServer, PendingRequest};
