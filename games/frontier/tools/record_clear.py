#!/usr/bin/env python3
"""驱动 frontier 从开局到 game-won(step=8)的通关录像,写入 qa/clear.json。

完整的多日游戏流程:Day 1 修信标+建田+种麦 → Day 3 立足 → Day 4 温饱 →
Day 5 成群 → Day 6 兴旺(game-won)。
录制约束:world/set 在录制中被禁(只录输入流),所以资源/状态只能靠真实玩法达成。
起步料(seed-start 规则)已经给到能一路打到丰碑通关:ore6/plank4/lamp2/wood8/seed10。
"""
import json, os, subprocess, sys, time, urllib.request

PORT = 6173
QA = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "qa", "clear.json"))
ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

def rpc(method, params=None, timeout=300):
    data = json.dumps({"method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=data,
                                 headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=timeout).read())

def big_step(n, chunk=3600):
    while n > 0:
        c = min(chunk, n)
        r = rpc("sim/step", {"ticks": c})
        if not r.get("ok"):
            print(f"[ERROR] step {c} failed: {r}")
            sys.exit(1)
        n -= c

def step(n=1): rpc("sim/step", {"ticks": n})
def click(x, y): return rpc("input/click", {"x": x, "y": y})
def inp(action, phase="pressed"): rpc("input/inject", {"action": action, "phase": phase})
def wget(entity): return rpc("world/get", {"entity": entity})

def check(msg, cond, detail=""):
    if not cond: print(f"[FAIL] {msg} {detail}"); sys.exit(1)
    print(f"[OK] {msg}")

def plant(x, y):
    inp("w"); step(2); click(x, y); step(3)

def harvest(x, y):
    inp("w"); step(2); click(x, y); step(3)

def build_wall(x, y):
    inp("q"); step(2); inp("2"); step(2); click(x, y); step(5)

def build_beacon(x, y):
    inp("q"); step(2); inp("6"); step(2); click(x, y); step(5)

def build_plot(x, y):
    inp("q"); step(2); inp("1"); step(2); click(x, y); step(5)

def build_monument(x, y):
    inp("q"); step(2); inp("8"); step(2); click(x, y); step(10)

def invite():
    inp("i"); step(20)

def wait_quest(stage_at_least, max_cycles=20, advance=21600):
    """等 quest step >= stage_at_least,大块推进 sim time。"""
    for i in range(max_cycles):
        s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
        if s >= stage_at_least: return s
        # 推进一段时间 + 重种重收
        big_step(advance // 2)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)
    return wget("@quest")["result"]["components"]["QuestLog"]["step"]

PLOTS = [(9, 6), (9, 7), (9, 8), (9, 9)]
PLOT_CYCLE = 1500  # 一茬成熟 ~12 sim sec,留余量

print("=== frontier 多日通关录像 ===")
proc = subprocess.Popen(
    [os.path.join(ROOT, "target/release/vitric.exe"),
     "run", "games/frontier", "--port", str(PORT), "--record", QA],
    cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    for _ in range(30):
        try: rpc("ping"); break
        except Exception: time.sleep(1)
    else: raise RuntimeError("server not ready")
    rpc("sim/pause")
    step(3)

    # === Day 1: 修信标 + 建 4 块田 + 种麦 ===
    print("\n--- Day 1: 修信标 + 建田 + 收第一茬 ---")
    build_beacon(9, 5)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==2 (信标)", s == 2, f"actual={s}")

    for (px, py) in PLOTS:
        build_plot(px, py)
    for (px, py) in PLOTS:
        plant(px, py)
    big_step(PLOT_CYCLE)
    for (px, py) in PLOTS:
        harvest(px, py)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  After first harvest: wheat={inv['wheat']} seed={inv['seed']} step={s}")
    check("step==3 (首收)", s == 3, f"actual={s}")

    # === 走到 Lio 邀请 ===
    print("\n--- Day 1 末尾: 邀请 Lio ---")
    inp("right"); step(250)
    inp("right", "released"); step(5)
    invite()
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==4 (Lio 入住)", s == 4, f"actual={s}")

    # 走回家
    inp("left"); step(250)
    inp("left", "released"); step(5)

    # === 等到 Day 3 (立足) ===
    print("\n--- 等到 Day 3 (立足) ---")
    for cycle in range(8):
        s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
        if s >= 5: break
        # 重种重收
        big_step(PLOT_CYCLE)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)
        c = wget("@colony")["result"]["components"]["Colony"]
        # 缺结构就补墙
        if c["struct_count"] < 3:
            for (wx, wy) in [(10, 5), (10, 6), (10, 7)]:
                build_wall(wx, wy)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  day={c['day']} struct={c['struct_count']} wheat={inv['wheat']} step={s}")
    check("step>=5 (立足)", s >= 5, f"actual={s}")

    # === 等到 Day 4 (温饱) ===
    print("\n--- 等到 Day 4 (温饱) ---")
    for cycle in range(8):
        s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
        if s >= 6: break
        big_step(PLOT_CYCLE)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  wheat={inv['wheat']} step={s}")
    check("step>=6 (温饱)", s >= 6, f"actual={s}")

    # === 等到 Day 5 (成群) ===
    print("\n--- 等到 Day 5 (成群) ---")
    invites_done = 0
    for cycle in range(10):
        s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
        if s >= 7: break
        big_step(PLOT_CYCLE)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)
        # 邀请现有旅人
        ents = rpc("world/entities", {"components": ["Drifter"]})["result"]
        target_e = None
        for e in ents:
            if e.get("components", {}).get("Position"):
                target_e = e; break
        if target_e:
            pos = target_e["components"]["Position"]
            # 走过去
            inp("right"); step(180)
            inp("right", "released"); step(3)
            inp("up"); step(60)
            inp("up", "released"); step(3)
            inp("right"); step(60)
            inp("right", "released"); step(3)
            invite()
            invites_done += 1
            inp("left"); step(200)
            inp("left", "released"); step(3)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  pop={c['pop']} step={s}")
    check("step>=7 (成群)", s >= 7, f"actual={s}")

    # === Day 6: 立丰碑 → game-won ===
    print("\n--- Day 6: 立丰碑 → game-won ---")
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  Resources: ore={inv['ore']} plank={inv['plank']} lamp={inv['lamp']} wheat={inv['wheat']}")

    # 等下一茬确保 wheat>=4
    for cycle in range(4):
        inv = wget("@player")["result"]["components"]["Inventory"]
        if inv["wheat"] >= 4 and inv["plank"] >= 4: break
        big_step(PLOT_CYCLE)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)

    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  Ready: ore={inv['ore']} plank={inv['plank']} lamp={inv['lamp']} wheat={inv['wheat']}")

    build_monument(11, 5)
    big_step(21600)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  day={c['day']} step={s} monument={c['monument_built']}")
    check("step==8 (game-won)", s == 8, f"actual={s}")

    print("\n=== 通关录像完成 ===")
finally:
    try: rpc("sim/quit")
    except: pass
    proc.wait(timeout=10)