#!/usr/bin/env python3
"""驱动 frontier 从开局到 game-won(step=8)的通关录像,写入 qa/clear.json。

完整的多日游戏流程:Day 1 修信标+建田+种麦 → Day 3 立足 → Day 4 温饱 →
Day 5 成群 → Day 6 兴旺(game-won)。
录制约束:world/set 在录制中被禁(只录输入流),所以资源/状态只能靠真实玩法达成。
起步料(seed-start 规则)已经给到能一路打到丰碑通关:ore6/plank4/lamp2/wood8/seed10。
"""
import json, os, subprocess, sys, time, urllib.request

PORT = 6181
QA = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "qa", "clear.json"))
ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

def rpc(method, params=None, timeout=300):
    data = json.dumps({"method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=data,
                                 headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=timeout).read())

SNAP_EVERY = 60        # 每 60 tick(约1秒)抓一帧
MAX_FRAMES = 2200      # 安全上限
FRAMES = []
def _snap():
    if len(FRAMES) >= MAX_FRAMES: return
    s = rpc("sim/snapshot")
    if isinstance(s, dict) and s.get("result") is not None:
        FRAMES.append(s["result"])

def big_step(n, chunk=SNAP_EVERY):
    while n > 0:
        c = min(chunk, n)
        r = rpc("sim/step", {"ticks": c})
        if not r.get("ok"):
            print(f"[ERROR] step {c} failed: {r}")
            sys.exit(1)
        n -= c
        _snap()

def step(n=1):
    rpc("sim/step", {"ticks": n}); _snap()
def click(x, y): return rpc("input/click", {"x": x, "y": y})
def inp(action, phase="pressed"): rpc("input/inject", {"action": action, "phase": phase})
def wget(entity): return rpc("world/get", {"entity": entity})

def goto_companion(max_iter=20):
    """把玩家挪到离最近伙伴 <2.5 格(gift/talk 需 dist<4)。伙伴会游荡,每步重读位置。"""
    for _ in range(max_iter):
        try:
            ents = rpc("world/entities", {"components": ["Companion", "Position"]})["result"]
            pp = wget("@player")["result"]["components"]["Position"]
        except Exception:
            return
        px, py = pp["x"], pp["y"]
        best = None; bd = 1e9
        for e in ents:
            p = e.get("components", {}).get("Position")
            if not p: continue
            d = (p["x"] - px) ** 2 + (p["y"] - py) ** 2
            if d < bd: bd = d; best = p
        if not best or bd <= 2.5 * 2.5:
            return
        dx, dy = best["x"] - px, best["y"] - py
        # 本作坐标:"up" 键 = +y(实测玩家按 up 后 y 增大)。
        d = ("right" if dx > 0 else "left") if abs(dx) >= abs(dy) else ("up" if dy > 0 else "down")
        inp(d); step(20); inp(d, "released"); step(2)

def goto_xy(tx, ty, near=2.0, max_iter=40):
    """把玩家走到目标点 <near 格。x 主轴优先,"up"=+y。"""
    for _ in range(max_iter):
        try:
            pp = wget("@player")["result"]["components"]["Position"]
        except Exception:
            return
        px, py = pp["x"], pp["y"]
        dx, dy = tx - px, ty - py
        if dx * dx + dy * dy <= near * near:
            return
        d = ("right" if dx > 0 else "left") if abs(dx) >= abs(dy) else ("up" if dy > 0 else "down")
        inp(d); step(20); inp(d, "released"); step(2)

def dump_companions(tag):
    try:
        ents = rpc("world/entities", {"components": ["Companion", "Need", "Position"]})["result"]
    except Exception as e:
        print(f"[DUMP {tag}] err {e}"); return
    c = wget("@colony")["result"]["components"]["Colony"]
    try:
        pp = wget("@player")["result"]["components"]["Position"]; pps = f"({pp.get('x')},{pp.get('y')})"
    except Exception: pps = "?"
    print(f"[DUMP {tag}] happy_count={c.get('companion_happy_count')} pop={c.get('pop')} day={c.get('day')} player={pps}")
    for e in ents:
        comp = e.get("components", {})
        n = comp.get("Need", {}); p = comp.get("Position", {})
        print(f"    {e.get('id')} aff={n.get('affinity')} comfort={n.get('comfort')} quarters={n.get('quarters')} talked={n.get('talked_today')} gifted={n.get('gifted_today')} pos=({p.get('x')},{p.get('y')})")

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
     "run", "games/frontier", "--port", str(PORT)],
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

    # 先走回家:Lio 入住后住在聚落(home 5~9),玩家此刻还在野外(x~23),在野外 gift/talk 打不到他。
    # 回到聚落两人同处家园,互动才落得到。
    inp("left"); step(250)
    inp("left", "released"); step(5)
    # iter2:在家把 Lio 养到 happy(affinity>=50)。送偏好(wheat/seed)+12×2 + 对话 +3×3 = +33(25→58)。
    # 每次互动后 step 拉开:+affinity 走 setField 延迟落地,挨太近会读到旧值互相覆盖、累加不上。
    print("    iter2 关系:在家 gift×2 + talk×3 把 Lio 养到 happy(>=50)")
    goto_companion()
    for _ in range(2):
        inp("g"); step(15)
    for _ in range(3):
        inp("t"); step(15)
    step(10)
    dump_companions("Day1-after-care")

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
            goto_xy(pos["x"], pos["y"], near=2.0)   # 读旅人真实位置导航过去(不再硬走固定路径)
            invite()
            invites_done += 1
            goto_xy(8, 7, near=2.5)                  # 回聚落中心,便于下一轮种收
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  pop={c['pop']} step={s}")
    dump_companions("Day5-成群-check")
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
    try:
        for _ in range(6):
            rpc("sim/step", {"ticks": 10}); _snap()
    except Exception: pass
    out = os.path.join(ROOT, "_snapstream_iter2.json")
    try:
        with open(out, "w", encoding="utf-8") as f:
            f.write("[" + ",".join(json.dumps(fr, separators=(",", ":")) for fr in FRAMES) + "]")
        print(f"SNAP_FRAMES={len(FRAMES)} -> {out} ({os.path.getsize(out)} bytes)")
    except Exception as e:
        print("write snap failed", e)
    try: rpc("sim/quit")
    except: pass
    proc.wait(timeout=10)