use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::JoinHandle;

use serde_json::{json, Value};

/// 一条等待主循环处理的控制请求。
pub struct PendingRequest {
    /// {"method": "...", "params": {...}}
    pub request: Value,
    responder: SyncSender<Value>,
}

impl PendingRequest {
    pub fn respond(self, response: Value) {
        // 客户端可能已断开；发送失败不影响主循环
        let _ = self.responder.send(response);
    }
}

/// HTTP 控制服务器。只做传输：请求进通道，主循环在帧边界取走处理。
pub struct ControlServer {
    pub port: u16,
    inbox: Receiver<PendingRequest>,
    _thread: JoinHandle<()>,
}

impl ControlServer {
    /// 绑定 127.0.0.1:port（port=0 自动分配）。只听本机：控制面就是 root 权限，不上公网。
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
                // 等主循环在帧边界处理（暂停状态下也会处理）
                match rrx.recv() {
                    Ok(response) => respond_json(http_req, 200, response),
                    Err(_) => respond_json(http_req, 503, json!({"ok": false, "error": "引擎主循环已退出"})),
                }
            }
        });

        Ok(ControlServer { port: actual_port, inbox: rx, _thread: thread })
    }

    /// 主循环每帧调用：取走当前积压的全部请求。
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
