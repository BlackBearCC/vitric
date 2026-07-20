#!/usr/bin/env python3
"""Drive frontier from start to settlement-founded (step=8) playthrough recording, writes qa/clear.json.

Multi-day flow: Day 1 fix beacon + build plots + harvest → Day 3 foothold → Day 4 food →
Day 5 crowd → Day 6 monument (settlement-founded).
Recording constraint: world/set is rejected during recording (only input stream is captured),
so resources/state must be achieved through real gameplay.
The seed-start inventory (ore6/plank6/lamp2/wood8/seed10) covers the full run including
a structure upgrade (plot→greenhouse) to fulfill Pip's upgrade wish; ore+plank spent on the
upgrade are recovered by gathering ore from a node and crafting planks from wood before the monument.
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
        # In this game's coordinate system: the "up" key = +y (measured: after pressing up, player y increases).
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
    print(f"[DUMP {tag}] happy_count={c.get('companion_happy_count')} wish_count={c.get('companion_wish_count')} pop={c.get('pop')} day={c.get('day')} player={pps}")
    for e in ents:
        comp = e.get("components", {})
        n = comp.get("Need", {}); p = comp.get("Position", {})
        print(f"    {e.get('id')} aff={n.get('affinity')} comfort={n.get('comfort')} quarters={n.get('quarters')} talked={n.get('talked_today')} gifted={n.get('gifted_today')} pos=({p.get('x')},{p.get('y')})")

def check(msg, cond, detail=""):
    if not cond: print(f"[FAIL] {msg} {detail}"); sys.exit(1)
    print(f"[OK] {msg}")

def plant(x, y):
    inp("r"); step(2); click(x, y); step(3)

def harvest(x, y):
    inp("r"); step(2); click(x, y); step(3)

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

def ui_click(nx, ny):
    # Screen-normalized coords (0..1); picking deferred to in-tick UI system (1920×1080 ref frame).
    return rpc("input/ui-click", {"nx": nx, "ny": ny})

def wait_quest(stage_at_least, max_cycles=20, advance=21600):
    """等 quest step >= stage_at_least,大块推进 sim time。"""
    for i in range(max_cycles):
        s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
        if s >= stage_at_least: return s
        # Advance time a bit + replant and reharvest
        big_step(advance // 2)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)
    return wget("@quest")["result"]["components"]["QuestLog"]["step"]

PLOTS = [(9, 6), (9, 7), (9, 8), (9, 9)]
PLOT_CYCLE = 1500  # one crop matures in ~12 sim sec, leave margin

print("=== frontier 多日通关录像 ===")
proc = subprocess.Popen(
    [os.path.join(ROOT, "target/release/vitric"),
     "run", "games/frontier", "--port", str(PORT), "--record", QA],
    cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    for _ in range(30):
        try: rpc("ping"); break
        except Exception: time.sleep(1)
    else: raise RuntimeError("server not ready")
    rpc("sim/pause")
    step(3)

    # === Day 1: fix beacon + 1 plot + plant/harvest → step 3, then 3rd build → step 4 ===
    # Build order matters: the 3rd build must fire AFTER step reaches 3, so the wish-fulfilled
    # event triggers the step 3→4 gate (wish-fulfilled + affinity>=60). Building 3 structures
    # before harvest would fulfill Pip's "build 3" wish while step==2 → gate fails.
    print("\n--- Day 1: 修信标 + 1田 + 收第一茬 → step 3 → 第3建(wish) → step 4 ---")
    build_beacon(9, 5)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==2 (信标)", s == 2, f"actual={s}")

    # 2nd build (beacon was 1st): wish progress 2/3
    build_plot(9, 6)
    plant(9, 6)
    big_step(PLOT_CYCLE)
    harvest(9, 6)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  After first harvest: wheat={inv['wheat']} seed={inv['seed']} step={s}")
    check("step==3 (首收)", s == 3, f"actual={s}")

    # 3rd build → Pip's "build 3 structures" wish fulfilled (+30 affinity → 60) → wish_count=1
    # → step 3→4 gate passes (wish-fulfilled + affinity>=60)
    build_plot(9, 7)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  After 3rd build: step={s} wish_count={c.get('companion_wish_count')}")
    check("step==4 (wish-fulfilled)", s == 4, f"actual={s}")

    # Build remaining plots and plant on all
    build_plot(9, 8)
    build_plot(9, 9)
    for (px, py) in PLOTS:
        plant(px, py)

    # === Walk to Lio and invite (for pop>=3 later; step 4 already reached via wish) ===
    print("\n--- Day 1 末尾: 邀请 Lio (为后续 pop>=3) ---")
    inp("right"); step(250)
    inp("right", "released"); step(5)
    invite()
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    print(f"  After invite: step={s} (already 4 from wish-fulfilled)")

    # Walk home first: after Lio joins he lives in the colony (home 5~9), but the player is still out in the wild (x~23); gift/talk can't reach him in the wild.
    inp("left"); step(250)
    inp("left", "released"); step(5)
    # iter2: raise Lio affinity (gift×2 + talk×3). Not strictly required by new wish-based gates,
    # but kept for relationship health (prevents companions from leaving due to low affinity).
    print("    iter2 关系:在家 gift×2 + talk×3")
    goto_companion()
    for _ in range(2):
        inp("g"); step(15)
    for _ in range(3):
        inp("t"); step(15)
    step(10)
    dump_companions("Day1-after-care")

    # === Upgrade a plot to greenhouse (Pip's "upgrade" wish → wish_count=2) ===
    # Must happen before day 5 (step 6→7 gate: day>=5 + pop>=3 + wish_count>=2).
    # Upgrade cost: plot→greenhouse {ore:2, plank:2}. After beacon(ore2+plank2) we have ore4/plank4;
    # after upgrade we have ore2/plank2 — recovered below before the monument.
    print("\n--- 升级种植台 → Pip 升级心愿达成 (wish_count=2) ---")
    inp("u"); step(3)       # enter upgrade mode
    click(9, 6); step(5)    # click plot at (9,6) → upgrade to greenhouse (cost ore2+plank2)
    inp("r"); step(3)       # back to interact mode
    c = wget("@colony")["result"]["components"]["Colony"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  After upgrade: wish_count={c.get('companion_wish_count')} ore={inv['ore']} plank={inv['plank']}")
    check("wish_count>=2 (upgrade wish)", c.get("companion_wish_count", 0) >= 2, f"actual={c.get('companion_wish_count')}")
    dump_companions("after-upgrade")

    # === Recover upgrade cost: gather 4 ore + craft 2 planks ===
    # After upgrade + 2 gifts (gifts consume the first ITEM_KINDS item = ore): ore0/plank2.
    # Monument needs ore4/plank4. Gather 4 ore from node at (18,3), craft 2 planks from 4 wood
    # (CRAFT.plank = {wood:2}→1 plank). End state: ore4/plank4/wood4.
    print("\n--- 采集矿石 + 制作木板 (补回升级消耗 + 礼物消耗) ---")
    # Gather 4 ore from the ore node at (18,3) — interact mode, click the node tile.
    click(18, 3); step(3)   # gather +1 ore
    click(18, 3); step(3)   # gather +1 ore
    click(18, 3); step(3)   # gather +1 ore
    click(18, 3); step(3)   # gather +1 ore
    # Craft 2 planks: click the mode_craft button (shows craft_menu), then click craft_plank button.
    # NOTE: pressing "e" (kb-mode-craft) only sets Mode.value — it does NOT show the craft_menu.
    # Only the mode-craft ui-activate rule (triggered by clicking the mode_craft button) sets
    # craft_menu.Ui.ox=208. Without this, the craft_plank button stays off-screen and UI clicks miss.
    # mode_craft button (mode_row HBox at ox=24/oy=100, gap=6, pad=9; 2nd child w=92/h=48)
    #   center ≈ (177, 132) in 1920×1080 ref frame → nx≈0.092, ny≈0.122.
    # craft_plank button (craft_menu VBox at ox=208/oy=176 when visible, pad=12, gap=8; 1st child w=222/h=42)
    #   center ≈ (331, 209) in 1920×1080 ref frame → nx≈0.173, ny≈0.194.
    ui_click(0.092, 0.122); step(3)   # click mode_craft → craft menu visible (Mode=craft, ox=208)
    ui_click(0.173, 0.194); step(3)   # craft plank #1 (cost 2 wood → 1 plank)
    ui_click(0.173, 0.194); step(3)   # craft plank #2 (cost 2 wood → 1 plank)
    inp("r"); step(3)       # back to interact mode (menu stays visible but doesn't affect world clicks)
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  After recover: ore={inv['ore']} plank={inv['plank']} wood={inv['wood']}")
    check("ore>=4 (monument budget)", inv["ore"] >= 4, f"actual={inv['ore']}")
    check("plank>=4 (monument budget)", inv["plank"] >= 4, f"actual={inv['plank']}")

    # === Wait until Day 3 (foothold) ===
    print("\n--- 等到 Day 3 (立足) ---")
    for cycle in range(8):
        s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
        if s >= 5: break
        # replant and reharvest
        big_step(PLOT_CYCLE)
        for (px, py) in PLOTS:
            harvest(px, py)
        for (px, py) in PLOTS:
            plant(px, py)
        c = wget("@colony")["result"]["components"]["Colony"]
        # if short on structures, build walls
        if c["struct_count"] < 3:
            for (wx, wy) in [(10, 5), (10, 6), (10, 7)]:
                build_wall(wx, wy)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  day={c['day']} struct={c['struct_count']} wheat={inv['wheat']} step={s}")
    check("step>=5 (立足)", s >= 5, f"actual={s}")

    # === Wait until Day 4 (food & shelter) ===
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

    # === Wait until Day 5 (a crowd) ===
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
        # invite existing drifters
        ents = rpc("world/entities", {"components": ["Drifter"]})["result"]
        target_e = None
        for e in ents:
            if e.get("components", {}).get("Position"):
                target_e = e; break
        if target_e:
            pos = target_e["components"]["Position"]
            goto_xy(pos["x"], pos["y"], near=2.0)   # read drifter's real position and navigate there (no longer hard-coding a fixed path)
            invite()
            invites_done += 1
            goto_xy(8, 7, near=2.5)                  # return to colony center for the next plant/harvest round
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"  pop={c['pop']} step={s}")
    dump_companions("Day5-成群-check")
    check("step>=7 (成群)", s >= 7, f"actual={s}")

    # === Day 6: raise monument → settlement-founded ===
    print("\n--- Day 6: 立丰碑 → settlement-founded ---")
    inv = wget("@player")["result"]["components"]["Inventory"]
    print(f"  Resources: ore={inv['ore']} plank={inv['plank']} lamp={inv['lamp']} wheat={inv['wheat']}")

    # wait one more crop to ensure wheat>=4
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
    check("step==8 (settlement-founded)", s == 8, f"actual={s}")

    print("\n=== 通关录像完成 ===")
finally:
    try: rpc("sim/quit")
    except: pass
    proc.wait(timeout=10)