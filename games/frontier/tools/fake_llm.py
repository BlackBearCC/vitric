#!/usr/bin/env python3
"""开发用假 LLM 端点:OpenAI chat/completions 格式,返回一句写死的角色回复。
没有真模型时,用它测 ctx.ask → llm-reply → 伙伴反应这条链路(回复走录制通道,可回放)。
用法: python3 games/frontier/tools/fake_llm.py [port]  (默认 6190)
配合: VITRIC_LLM_URL=http://127.0.0.1:6190/v1/chat/completions VITRIC_LLM_KEY=x VITRIC_LLM_MODEL=stub"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

REPLY = {"say": "诶,你可算来了!搭把手呗~", "mood": "兴奋"}

# 求人设的提问(prompt 含"来历")返回轮换的人设 JSON,模拟现生成的不同旅人。
PERSONAS = [
    {"name": "Mara", "archetype": "沉默矿工", "traits": "话少,手稳,认死理", "speech": "惜字如金,常用单字回应"},
    {"name": "Lio", "archetype": "乐天厨子", "traits": "贪吃,爱张罗,记仇又健忘", "speech": "热络,爱用感叹号"},
    {"name": "Sera", "archetype": "落魄学者", "traits": "好奇,絮叨,怕黑", "speech": "文绉绉,爱掉书袋"},
]
_n = [0]


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n).decode("utf-8", "ignore")
        if "来历" in body:  # 求人设
            p = PERSONAS[_n[0] % len(PERSONAS)]
            _n[0] += 1
            content = json.dumps(p, ensure_ascii=False)
        else:  # 打招呼/提愿望
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
