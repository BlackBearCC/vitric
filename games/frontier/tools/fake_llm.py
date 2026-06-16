#!/usr/bin/env python3
"""开发用假 LLM 端点:OpenAI chat/completions 格式,返回一句写死的角色回复。
没有真模型时,用它测 ctx.ask → llm-reply → 伙伴反应这条链路(回复走录制通道,可回放)。
用法: python3 games/frontier/tools/fake_llm.py [port]  (默认 6190)
配合: VITRIC_LLM_URL=http://127.0.0.1:6190/v1/chat/completions VITRIC_LLM_KEY=x VITRIC_LLM_MODEL=stub"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

REPLY = {"say": "诶,新来的!我是 Pip——这破地儿正缺个会拧螺丝的,你可算来了~", "mood": "兴奋"}


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        self.rfile.read(n)
        content = json.dumps(REPLY, ensure_ascii=False)
        body = json.dumps({"choices": [{"message": {"content": content}}]}).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 6190
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
