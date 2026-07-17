use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::JoinHandle;

use serde_json::{json, Value};

/// A control request awaiting main-loop processing.
pub struct PendingRequest {
    /// {"method": "...", "params": {...}}
    pub request: Value,
    responder: SyncSender<Value>,
}

impl PendingRequest {
    pub fn respond(self, response: Value) {
        // The client may already be disconnected; a send failure does not affect the main loop.
        let _ = self.responder.send(response);
    }
}

/// HTTP control server. Does transport only: requests go into a channel, the main loop
/// drains them at frame boundaries.
pub struct ControlServer {
    pub port: u16,
    inbox: Receiver<PendingRequest>,
    _thread: JoinHandle<()>,
}

impl ControlServer {
    /// Bind 127.0.0.1:port (port=0 = auto-allocate). Listens on localhost only: the control
    /// plane is root-level authority, never exposed to the public internet.
    pub fn start(port: u16) -> Result<ControlServer, String> {
        let server = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| format!("控制面端口绑定失败: {e}"))?;
        let actual_port = server.server_addr().to_ip().expect("ip 监听").port();
        let (tx, rx) = sync_channel::<PendingRequest>(256);

        let thread = std::thread::spawn(move || {
            for mut http_req in server.incoming_requests() {
                let mut body = String::new();
                if http_req.as_reader().read_to_string(&mut body).is_err() {
                    respond_json(http_req, 400, json!({"ok": false, "error": "请求体读取失败"}));
                    continue;
                }
                let request: Value = match serde_json::from_str(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        respond_json(
                            http_req,
                            400,
                            json!({"ok": false, "error": format!(
                                "请求体不是合法 JSON: {e}。协议: POST /rpc {{\"method\": \"...\", \"params\": {{}}}}"
                            )}),
                        );
                        continue;
                    }
                };
                let (rtx, rrx) = sync_channel::<Value>(1);
                if tx.send(PendingRequest { request, responder: rtx }).is_err() {
                    respond_json(http_req, 503, json!({"ok": false, "error": "引擎主循环已退出"}));
                    continue;
                }
                // Wait for the main loop to process at a frame boundary (also handled when paused).
                match rrx.recv() {
                    Ok(response) => respond_json(http_req, 200, response),
                    Err(_) => respond_json(http_req, 503, json!({"ok": false, "error": "引擎主循环已退出"})),
                }
            }
        });

        Ok(ControlServer { port: actual_port, inbox: rx, _thread: thread })
    }

    /// Called by the main loop each frame: drains all currently pending requests.
    pub fn drain(&self) -> Vec<PendingRequest> {
        let mut out = Vec::new();
        while let Ok(req) = self.inbox.try_recv() {
            out.push(req);
        }
        out
    }
}

fn respond_json(req: tiny_http::Request, status: u16, body: Value) {
    let data = body.to_string();
    let response = tiny_http::Response::from_string(data)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("合法 header"),
        );
    let _ = req.respond(response);
}
