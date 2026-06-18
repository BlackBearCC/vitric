#!/usr/bin/env python3
"""驱动 frontier 从开局到 game-won 的通关录像,写入 qa/clear.json。"""
import json, os, signal, subprocess, sys, time, urllib.request

PORT = 6173
QA = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "qa", "clear.json"))
ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

def rpc(method, params=None):
    data = json.dumps({"method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=data,
                                 headers={"Content-Type": "application/json"})
    resp = urllib.request.urlopen(req)
    return json.loads(resp.read())

def wait_server(timeout=30):
    for i in range(timeout):
        try:
            rpc("ping")
            return True
        except Exception:
            time.sleep(1)
    raise RuntimeError("服务器未就绪")

def step(n=1):
    rpc("sim/step", {"ticks": n})

def click(x, y):
    return rpc("input/click", {"x": x, "y": y})

def inp(action, phase="pressed"):
    rpc("input/inject", {"action": action, "phase": phase})

def wget(entity):
    return rpc("world/get", {"entity": entity})

def check(msg, cond, detail=""):
    if not cond:
        print(f"[FAIL] {msg} {detail}")
        sys.exit(1)
    print(f"[OK] {msg}")

print("=== frontier 通关录像 ===")

# 1 启动服务端(录制模式)
print("\n--- 启动录制 ---")
proc = subprocess.Popen(
    [os.path.join(ROOT, "target/release/vitric.exe"),
     "run", "games/frontier", "--port", str(PORT), "--record", QA],
    cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    wait_server()
    print("服务端就绪")

    # 2 暂停
    rpc("sim/pause")
    step()
    print("已暂停")

    # === 主线1: 修信标 ===
    print("\n--- 主线1 修信标 ---")
    step(5)
    # 选信标(键 6)
    inp("6")
    step(2)
    # 点地建造(空地 9,5 — 不在陆块/岩石/冰上)
    click(9.0, 5.0)
    step(10)
    # 验证 step==2
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step 应为 2", s == 2, f"实际 {s}")

    # === 主线2: 种麦收获 ===
    print("\n--- 主线2 种麦收获 ---")
    # 起种植台(键1选plot)
    inp("1")
    step(2)
    click(9.0, 6.0)
    step(10)
    # 切互动模式(w)
    inp("w")
    step(3)
    # 点种植台播麦种
    click(9.0, 6.0)
    step(10)
    # 等作物成熟(3段 ×6秒 =18秒 约1080tick, +余量1500)
    step(1500)
    # 收麦
    click(9.0, 6.0)
    step(10)
    # 验证 step==3
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step 应为 3", s == 3, f"实际 {s}")

    # === 主线3: 请伙伴 ===
    print("\n--- 主线3 请伙伴 ---")
    # 走到旅人(23,7) —从(7,7)出发,250步≈走16格
    inp("right")
    step(250)
    inp("right", "released")
    step(10)
    # 邀请旅人(按i)
    inp("i")
    step(20)
    # 验证 step==4
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step 应为 4", s == 4, f"实际 {s}")

    # === 主线4: 聚落兴旺 ===
    print("\n--- 主线4 聚落兴旺 ---")
    # 已有: 信标(1) + 种植台(1) = 2. 再造≥4座墙.
    # 起始木料4,每墙1木,正好4墙.
    inp("q")  # 切回建造
    step(2)
    inp("2")  # 选墙
    step(2)
    for (wx, wy) in [(10,5), (10,6), (10,7), (10,8)]:
        click(wx, wy)
        step(5)
    # 等结算: struct_count≥6 + pop≥2 触发 settlement-thrived → game-won
    step(60)
    # 查 colony struct_count
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  struct_count={c['struct_count']}  pop={c['pop']}")
    check("结构≥6", c["struct_count"] >= 6, f"实际 {c['struct_count']}")
    check("人口≥2", c["pop"] >= 2, f"实际 {c['pop']}")
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step 应为 5(game-won)", s == 5, f"实际 {s}")

    print("\n=== 通关录像完成 ===")

except Exception as e:
    print(f"\n[ERROR] {e}")
    sys.exit(1)
finally:
    # 停服务端
    try:
        rpc("sim/quit")
    except Exception:
        pass
    proc.wait(timeout=10)
