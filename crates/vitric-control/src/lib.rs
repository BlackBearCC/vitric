//! vitric-control — AI control plane: a debug port built into the engine process.
//!
//! This is Vitric's signature differentiator: any agent over HTTP JSON-RPC can
//! **see** (inspect any entity/component/event), **act** (inject input, mutate state),
//! **control time** (pause/step/speed), and **test** (register assertions that report on violation).
//!
//! Architecture: the HTTP server thread only does transport (parse request → push into channel → wait for reply);
//! commands are executed by the game main loop at **frame boundaries** — the control plane never breaks determinism.
//!
//! Protocol: `POST /rpc` with a single object `{"method": "...", "params": {...}}`,
//! responding with `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`.

mod dispatcher;
pub mod saves;
mod server;

pub use dispatcher::{inject_click, inject_ui_click, Dispatcher, LoopCtl};
pub use saves::SaveStore;
pub use server::{ControlServer, PendingRequest};
