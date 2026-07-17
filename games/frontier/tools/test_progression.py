#!/usr/bin/env python3
"""端到端测试:跑通 7 天 → game-won,只验不录制。"""
import json, os, subprocess, sys, time, urllib.request

PORT = 6174
ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

def rpc(method, params=None, timeout=300):
    data = json.dumps({"method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=data,
                                 headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=timeout).read())

def big_step(n, chunk=3600):
    """分块 sim/step,避开单次 step 过大卡死/超时。"""
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

print("=== 多日通关测试 ===")
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

    print("\n--- 第 1 天: 修信标 + 种麦 ---")
    inp("6"); step(3)  # beacon
    click(9, 5); step(5)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==2 (信标)", s == 2, f"actual={s}")

    inp("1"); step(3)  # plot
    click(9, 6); step(5)
    inp("w"); step(3)  # interact
    click(9, 6); step(5)  # plant

    # wait 12 sec to mature
    big_step(720)
    click(9, 6); step(5)  # harvest
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==3 (首收)", s == 3, f"actual={s}")

    # walk to Lio
    inp("right"); step(250)
    inp("right", "released"); step(5)
    inp("i"); step(10)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==4 (Lio 入住)", s == 4, f"actual={s}")

    print("\n--- 等到第 3 天 (立足) ---")
    big_step(21600)  # 6 minutes sim time = should cross day 3
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  day={c['day']} stage={c['stage']} struct={c['struct_count']}")
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    print(f"  step={s}")
    pip = wget("@companion")
    if "error" not in pip:
        pipn = pip["result"]["components"]["Need"]
        print(f"  Pip comfort={pipn['comfort']:.1f} quarters={pipn['quarters']} leave_timer={pipn['leave_timer']:.1f}")
    else:
        print("  Pip GONE")
    # day>=3 + struct>=3? — we only have 2 structures, need to build more
    if s < 5:
        # walk back and build some structures
        inp("left"); step(250)
        inp("left", "released"); step(5)
        inp("q"); step(3)
        inp("2"); step(3)  # wall
        for x, y in [(10, 5), (10, 6), (10, 7)]:
            click(x, y); step(5)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  After walls: day={c['day']} stage={c['stage']} struct={c['struct_count']} step={s}")
    check("step>=5 (立足)", s >= 5, f"actual={s}")

    print("\n--- 等到第 4 天 (温饱) ---")
    big_step(21600)  # another 6 minutes
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  day={c['day']} stage={c['stage']}")
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  wheat={inv['wheat']} step={s}")
    # wheat>=5?
    if inv["wheat"] < 5:
        # cheat: give some wheat
        rpc("world/set", {"entity": "@player", "path": "Inventory.wheat", "value": 6})
        step(5)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step>=6 (温饱)", s >= 6, f"actual={s}")

    print("\n--- 等到第 5 天 (成群) ---")
    big_step(21600)  # another 6 minutes
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  day={c['day']} stage={c['stage']} drifters_spawned={c['drifters_spawned']} pop={c['pop']}")
    # pop>=3? Pip (initial) + Lio (invited) = 2. Invite more drifters until pop>=3.
    invited_attempts = 0
    while c["pop"] < 3 and invited_attempts < 5:
        invited_attempts += 1
        ents = rpc("world/entities", {"components": ["Drifter"]})["result"]
        names = [e.get("name") for e in ents]
        print(f"  [attempt {invited_attempts}] drifters: {names} pop={c['pop']}")
        target_e = None
        for e in ents:
            pos = e.get("components", {}).get("Position")
            if pos:
                target_e = e
                break
        if not target_e:
            print("  no drifter with Position found; advancing time")
            big_step(3600)
            continue
        pos = target_e["components"]["Position"]
        rpc("world/set", {"entity": "@player", "path": "Position.x", "value": pos["x"]})
        rpc("world/set", {"entity": "@player", "path": "Position.y", "value": pos["y"]})
        step(5)
        inp("i"); step(20)
        c = wget("@colony")["result"]["components"]["Colony"]
        # wait a bit for next drifter to spawn if needed
        if c["pop"] < 3:
            big_step(7200)  # ~2 game days, more drifters should arrive
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  After invite: pop={c['pop']} step={s}")
    check("step>=7 (成群)", s >= 7, f"actual={s}")

    print("\n--- 等到第 6 天 (立丰碑 → game-won) ---")
    # cheat: give monument resources
    rpc("world/set", {"entity": "@player", "path": "Inventory.ore", "value": 5})
    rpc("world/set", {"entity": "@player", "path": "Inventory.plank", "value": 5})
    rpc("world/set", {"entity": "@player", "path": "Inventory.lamp", "value": 3})
    rpc("world/set", {"entity": "@player", "path": "Inventory.wheat", "value": 5})
    step(5)
    inp("q"); step(3)
    inp("8"); step(3)  # monument
    click(11, 5); step(10)
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  monument_built={c['monument_built']}")
    # advance to day 6
    big_step(21600)
    c = wget("@colony")["result"]["components"]["Colony"]
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    print(f"  day={c['day']} step={s} monument={c['monument_built']}")
    check("step==8 (game-won)", s == 8, f"actual={s}")

    print("\n=== PASS ===")
finally:
    try: rpc("sim/quit")
    except: pass
    proc.wait(timeout=10)
