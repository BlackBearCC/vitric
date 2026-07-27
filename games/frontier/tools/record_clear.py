#!/usr/bin/env python3
"""Drive frontier from start to settlement-founded (兴旺 stage) playthrough recording.

96-day sandbox playthrough at 90s/day. Covers: research (4 T1 + 4 T2), combat,
trade (nomads → allied), region exploration, faction negotiation.

Uses sim/turbo for full-speed fast-forward: turbo mode runs the engine at full CPU
speed while keeping the RPC server alive, so the script can inject input mid-run.
Operations (build/plant/harvest/click) use sim/pause + sim/step for precise control.
This replaces the old big_step(N, chunk=7200) pattern — no more per-chunk RPC overhead.

Settlement-Founded Path (7 quest gates → step 8 emits settlement-founded):
  1→2  built beacon
  2→3  harvested wheat
  3→4  wish-fulfilled + companion affinity>=60
  4→5  Colony.stage == 立足 (day>=12 + survival_t1 + struct>=5)
  5→6  day>=12 + Inventory.wheat>=5
  6→7  day>=24 + pop>=3 + companion_wish_count>=2
  7→8  Colony.stage == 兴旺 (day>=96 + all 4 T2 + monument + allied) → settlement-founded

Recording constraint: world/set is rejected during recording (only input stream is captured),
so resources/state must be achieved through real gameplay. world/get and world/entities are
NOT recorded — safe for debugging. Only input/click, input/inject, input/ui-click-by-name
and their replies are recorded.
"""
import json, os, subprocess, sys, time, urllib.request

PORT = 6174
QA = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "qa", "clear.json.tmp"))
ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

def rpc(method, params=None, timeout=900):
    data = json.dumps({"method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=data,
                                 headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=timeout).read())

# ---- Turbo fast-forward helpers ----
# wait_until: turbo-run until cond() returns True, then pause.
# Turbo mode ignores wall-clock pacing and runs at full CPU speed while still
# draining RPC input. After cond (or timeout), engine is paused for precise ops.

def turbo_on():
    rpc("sim/turbo", {"on": True})

def turbo_off():
    # Pause first to avoid a real-time gap, then turn off turbo.
    rpc("sim/pause")
    rpc("sim/turbo", {"on": False})

def wait_until(cond, max_s=120):
    """Turbo-run until cond() returns True, then pause. cond is a zero-arg callable."""
    turbo_on()
    deadline = time.time() + max_s
    while time.time() < deadline:
        try:
            if cond():
                break
        except Exception:
            pass
        time.sleep(0.01)
    turbo_off()

def step(n=1):
    rpc("sim/step", {"ticks": n})

def click(x, y):
    return rpc("input/click", {"x": x, "y": y})

def inp(action, phase="pressed"):
    rpc("input/inject", {"action": action, "phase": phase})

def wget(entity):
    return rpc("world/get", {"entity": entity})

def goto_companion(max_iter=20):
    """Walk player to within 2.5 tiles of the nearest companion (gift/talk needs dist<4)."""
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
        d = ("right" if dx > 0 else "left") if abs(dx) >= abs(dy) else ("up" if dy > 0 else "down")
        inp(d); step(20); inp(d, "released"); step(2)

def goto_xy(tx, ty, near=2.0, max_iter=40):
    """Walk player to within `near` tiles of (tx,ty). x-axis dominant, up=+y."""
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
    print(f"[DUMP {tag}] happy={c.get('companion_happy_count')} wish={c.get('companion_wish_count')} pop={c.get('pop')} day={c.get('day')} stage={c.get('stage')} player={pps}")
    for e in ents:
        comp = e.get("components", {})
        n = comp.get("Need", {}); p = comp.get("Position", {})
        print(f"    {e.get('id')} aff={n.get('affinity')} comfort={n.get('comfort')} pos=({p.get('x')},{p.get('y')})")

def find_companion_by_role(role):
    """Find a companion by Persona.role (e.g. 'scholar'). Returns entity dict or None."""
    try:
        ents = rpc("world/entities", {"components": ["Companion", "Persona", "Need", "Position"]})["result"]
    except Exception:
        return None
    for e in ents:
        p = e.get("components", {}).get("Persona", {})
        if p.get("role") == role:
            return e
    return None

def goto_companion_by_role(role):
    """Walk player to within 2.0 tiles of the companion with the given role.
    Returns True if the companion was found and reached."""
    target = find_companion_by_role(role)
    if not target:
        return False
    pos = target.get("components", {}).get("Position")
    if not pos:
        return False
    goto_xy(pos["x"], pos["y"], near=2.0)
    step(5)  # let target_companion system update Colony.target_companion*
    return True

def companion_affinity(role):
    """Return the current affinity of the companion with the given role, or None."""
    target = find_companion_by_role(role)
    if not target:
        return None
    return target.get("components", {}).get("Need", {}).get("affinity")

def gift_talk_to_role(role, gifts=2, talks=3):
    """Walk to the companion with the given role, verify target_companion points to it,
    then gift + talk. Returns the post-interaction affinity, or None if the companion
    could not be reached/targeted.

    The target_companion verification is critical: after advancing days, companions move
    and target_companion may point to a different companion — gifting the wrong one
    silently wastes the action (affinity doesn't change on the intended target).
    """
    target = find_companion_by_role(role)
    if not target:
        return None
    pos = target.get("components", {}).get("Position")
    if not pos:
        return None
    target_id = target.get("id")
    goto_xy(pos["x"], pos["y"], near=1.5)
    step(30)  # let target-companion system update
    # Verify target_companion points to our role before gifting
    for attempt in range(3):
        c = wget("@colony")["result"]["components"]["Colony"]
        tc_id = c.get("target_companion", "")
        if tc_id == target_id:
            break
        goto_xy(pos["x"], pos["y"], near=0.5)
        step(30)
    for _ in range(gifts):
        inp("g"); step(20)
    for _ in range(talks):
        inp("t"); step(60)
    step(20)
    return companion_affinity(role)

def raise_affinity(role, target_aff=50, max_days=3):
    """Raise a companion's affinity to >=target_aff by gifting/talking across multiple days.
    Each day: gift + talk (daily cap resets). Returns final affinity or None on failure."""
    aff = companion_affinity(role)
    print(f"    {role} affinity start: {aff}")
    for day_idx in range(max_days):
        if aff is not None and aff >= target_aff:
            return aff
        if day_idx > 0:
            c = wget("@colony")["result"]["components"]["Colony"]
            print(f"    {role} affinity <{target_aff}, advancing to day {c['day']+1} for cap reset...")
            advance_to_day(c["day"] + 1)
        aff = gift_talk_to_role(role)
        print(f"    {role} affinity after day {day_idx+1} gifts/talks: {aff}")
    return aff

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
    return rpc("input/ui-click", {"nx": nx, "ny": ny})

def ui_click_by_name(name):
    return rpc("input/ui-click-by-name", {"name": name})

PLOTS = [(9, 6), (9, 7), (9, 8), (9, 9)]

# ---- New helpers for 96-day flow ----

def research(tech_id):
    """Start research on a tech. Assumes player has enough TP + prereqs."""
    inp("t"); step(3)  # enter research mode (shows tech_menu)
    ui_click_by_name(f"tech_{tech_id}"); step(3)  # click tech button → start_research fn
    inp("r"); step(3)  # back to interact mode

def wait_research_complete(max_s=60):
    """Wait until Research.current becomes empty (research done). Uses turbo."""
    wait_until(
        lambda: not wget("@colony")["result"]["components"].get("Research", {}).get("current"),
        max_s
    )

def trade_nomads(n=1):
    """Trade with nomads n times. Each: 3 wheat → 2 fiber + +2 relation."""
    inp("b"); step(3)  # enter trade mode (shows trade_menu)
    for _ in range(n):
        ui_click_by_name("trade_nomads"); step(3)
    inp("r"); step(3)  # back to interact mode

def negotiate_nomads(n=1):
    """Negotiate with nomads n times, ONE AT A TIME. Each: +3 relation.

    negotiate fn uses ctx.ask('llm', ...) which is async — onNegotiateReply fires
    later. If multiple negotiates are clicked in rapid succession, the callbacks
    race on the read-modify-write of Faction.relations (all read the same value,
    only the last write wins → +3 total instead of +3*n).

    Fix: click negotiate once, then wait for Colony._negotiate_target to be
    cleared (the callback clears it at the end) before clicking again.
    Uses turbo for the wait — much faster than the old big_step(60) polling.
    """
    inp("b"); step(3)  # enter trade mode
    for _ in range(n):
        ui_click_by_name("negotiate_nomads"); step(3)
        # Wait for callback to complete via turbo
        wait_until(
            lambda: not wget("@colony")["result"]["components"].get("Colony", {}).get("_negotiate_target"),
            max_s=10
        )
        step(30)  # extra margin for relation write to settle
    inp("r"); step(3)  # back to interact mode

def advance_to_day(target_day):
    """Turbo-run until Colony.day >= target_day. Farms plots along the way.

    Only harvests/plants when seed > 0 — seed is a finite initial resource (5),
    and once depleted, plant fails silently (emit plant-fail). Harvesting without
    planting is pointless (no crop to harvest next cycle). When seed=0, we skip
    farming entirely and rely on trader companion's passive wheat contribution
    (+1 wheat per 12s when affinity>=50).

    Uses turbo for each ~1-day farming cycle instead of old big_step(PLOT_CYCLE).
    """
    while True:
        c = wget("@colony")["result"]["components"]["Colony"]
        if c["day"] >= target_day:
            return
        inv = wget("@player")["result"]["components"]["Inventory"]
        if (inv.get("seed", 0) | 0) > 0:
            # Turbo ~1 day, then harvest/plant
            start_day = c["day"]
            wait_until(
                lambda: wget("@colony")["result"]["components"]["Colony"]["day"] >= start_day + 1,
                max_s=10
            )
            for (px, py) in PLOTS:
                harvest(px, py)
            for (px, py) in PLOTS:
                plant(px, py)
        else:
            # No seed — skip farming, turbo straight to target day
            wait_until(
                lambda: wget("@colony")["result"]["components"]["Colony"]["day"] >= target_day,
                max_s=300
            )

def wait_for_tp(n, max_s=120):
    """Wait until TechPoint.value >= n (scholar contribution or POI). Uses turbo."""
    wait_until(
        lambda: wget("@player")["result"]["components"].get("TechPoint", {}).get("value", 0) >= n,
        max_s
    )

def invite_any_drifter():
    """Find the nearest drifter on the field, walk to it, invite. Returns True if invited."""
    ents = rpc("world/entities", {"components": ["Drifter", "Position"]})["result"]
    if not ents:
        return False
    target = None
    for e in ents:
        pos = e.get("components", {}).get("Position")
        if pos:
            target = e; break
    if not target:
        return False
    pos = target["components"]["Position"]
    goto_xy(pos["x"], pos["y"], near=2.0)
    step(3)  # let target-drifter system update
    invite()
    return True

def visit_poi(x, y):
    """Walk to a POI, switch to interact mode, click it for +2 TP."""
    goto_xy(x, y, near=2.0)
    inp("r"); step(2)  # ensure interact mode
    click(x, y); step(5)

def gather_node(x, y, times=1):
    """Gather resource from a node by clicking it in interact mode."""
    goto_xy(x, y, near=2.0)
    inp("r"); step(2)
    for _ in range(times):
        click(x, y); step(3)

def craft_plank(n=1):
    """Craft n planks (each costs 2 wood → 1 plank)."""
    ui_click_by_name("mode_craft"); step(3)  # show craft menu
    for _ in range(n):
        ui_click_by_name("craft_plank"); step(3)
    inp("r"); step(3)  # back to interact mode

def craft_lamp(n=1):
    """Craft n lamps (each costs 1 plank + 1 ore → 1 lamp)."""
    ui_click_by_name("mode_craft"); step(3)  # show craft menu
    for _ in range(n):
        ui_click_by_name("craft_lamp"); step(3)
    inp("r"); step(3)  # back to interact mode

def status(tag=""):
    """Print current quest step, stage, day, pop, key resources."""
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    inv = wget("@player")["result"]["components"]["Inventory"]
    tp = wget("@player")["result"]["components"].get("TechPoint", {}).get("value", 0)
    r = wget("@colony")["result"]["components"].get("Faction", {})
    rel = r.get("relations", "{}")
    print(f"[STATUS {tag}] step={s} stage={c.get('stage')} day={c.get('day')} pop={c.get('pop')} "
          f"wish={c.get('companion_wish_count')} ore={inv['ore']} plank={inv['plank']} "
          f"wheat={inv['wheat']} seed={inv['seed']} tp={tp} rel={rel}")

# ======================================================================
# Main recording flow
# ======================================================================

print("=== frontier 96-day sandbox playthrough recording (turbo mode) ===")
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

    # === Phase 1: Day 1-3 (Early Game → step 4) ===
    # Build beacon, first harvest, fulfill Pip's "build 3" wish (→ step 4),
    # invite Lio, gift/talk, upgrade plot (→ wish_count=2 for step 7 gate).
    print("\n--- Phase 1: Day 1-3 (beacon + harvest + wish → step 4) ---")
    build_beacon(9, 5)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==2 (beacon)", s == 2, f"actual={s}")

    # 2nd build + plant + harvest → step 3
    build_plot(9, 6)
    plant(9, 6)
    # Turbo ~1 day for crops to mature
    wait_until(lambda: wget("@colony")["result"]["components"]["Colony"]["day"] >= 2, max_s=10)
    harvest(9, 6)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    check("step==3 (first harvest)", s == 3, f"actual={s}")

    # 3rd build → Pip's "build 3 structures" wish fulfilled (+30 affinity → 60) → step 4
    build_plot(9, 7)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    check("step==4 (wish-fulfilled)", s == 4, f"actual={s} wish={c.get('companion_wish_count')}")

    # Build remaining plots and plant all
    build_plot(9, 8)
    build_plot(9, 9)
    for (px, py) in PLOTS:
        plant(px, py)

    # Walk to Lio (drifter at ~23,7) and invite
    print("    inviting Lio (farmer)...")
    inp("right"); step(250)
    inp("right", "released"); step(5)
    invite()

    # Walk home
    inp("left"); step(250)
    inp("left", "released"); step(5)

    # Gift/talk to raise Lio affinity (keeps companions healthy)
    goto_companion()
    for _ in range(2):
        inp("g"); step(15)
    for _ in range(3):
        inp("t"); step(15)
    step(10)

    # Upgrade plot (9,6) to greenhouse → Pip's "upgrade 1" wish fulfilled → wish_count=2
    print("    upgrading plot → wish_count=2")
    inp("u"); step(3)
    click(9, 6); step(5)
    inp("r"); step(3)
    c = wget("@colony")["result"]["components"]["Colony"]
    check("wish_count>=2 (upgrade wish)", c.get("companion_wish_count", 0) >= 2,
          f"actual={c.get('companion_wish_count')}")
    status("Phase1-done")

    # === Phase 2: Day 3-12 (survival_t1 + structures → 立足, step 5+6) ===
    # Visit POI for +2 TP, research survival_t1, invite drifters to clear spawn slots,
    # advance to day 12 → stage=立足 → step 5. Ensure wheat>=5 → step 6.
    print("\n--- Phase 2: Day 3-12 (survival_t1 + 立足 → step 5,6) ---")

    # Visit poi_camp (18,10) for +2 TechPoints
    print("    visiting poi_camp for +2 TP...")
    visit_poi(18, 10)
    tp = wget("@player")["result"]["components"].get("TechPoint", {}).get("value", 0)
    check("TP>=2 (POI)", tp >= 2, f"actual={tp}")

    # Research survival_t1 (cost 2 TP, 45s)
    print("    researching survival_t1...")
    research("survival_t1")
    wait_research_complete()
    r = wget("@colony")["result"]["components"].get("Research", {})
    check("survival_t1 researched", r.get("has_survival_t1") == 1, f"known={r.get('known')}")

    # Build walls to ensure struct_count >= 5 (beacon + 3 plots + 1 greenhouse = 5, but add walls for margin)
    build_wall(10, 5)
    build_wall(10, 6)
    build_wall(10, 7)

    # Advance to day 3 → first new drifter (Kade, builder) spawns → invite to clear slot
    print("    advancing to day 3, inviting drifter...")
    advance_to_day(3)
    invite_any_drifter()

    # Advance to day 5 → Sori (scholar) spawns → invite for TP contribution
    print("    advancing to day 5, inviting scholar...")
    advance_to_day(5)
    invite_any_drifter()

    # Raise scholar affinity to >=50 for TP contribution (scholar generates +1 TP every 12s).
    # Scholar preferred items: lamp, chair. Player starts with lamp=2.
    # 2 lamp gifts (+12 each = +24) + 3 talks (+3 each = +9) = +33 → affinity 25+33=58.
    print("    raising scholar affinity (gift lamps + talk)...")
    scholar = find_companion_by_role("scholar")
    check("scholar companion found", scholar is not None, "no companion with role=scholar")
    spos = scholar.get("components", {}).get("Position", {}) if scholar else {}
    print(f"    scholar at ({spos.get('x'):.1f},{spos.get('y'):.1f}) id={scholar.get('id') if scholar else '?'}")

    aff = raise_affinity("scholar", target_aff=50, max_days=3)
    check("scholar affinity>=50 (TP contribution)", aff is not None and aff >= 50,
          f"actual={aff}")

    # Advance to day 12 (farms wheat along the way via advance_to_day)
    print("    advancing to day 12...")
    advance_to_day(12)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    check("step>=5 (立足)", s >= 5, f"actual={s} stage={c.get('stage')} day={c.get('day')}")
    inv = wget("@player")["result"]["components"]["Inventory"]
    check("step>=6 (温饱)", s >= 6, f"actual={s} wheat={inv['wheat']}")
    status("Phase2-done")

    # === Phase 3: Day 12-24 (agriculture_t1 + scholar → 成形, step 7) ===
    # Visit another POI for +2 TP, research agriculture_t1, invite more drifters,
    # advance to day 24 → stage=成形 → step 7 (day>=24 + pop>=3 + wish_count>=2).
    print("\n--- Phase 3: Day 12-24 (agriculture_t1 + 成形 → step 7) ---")

    # Visit poi_cave (23,2) for +2 TP
    print("    visiting poi_cave for +2 TP...")
    visit_poi(23, 2)
    tp = wget("@player")["result"]["components"].get("TechPoint", {}).get("value", 0)
    check("TP>=2 (POI)", tp >= 2, f"actual={tp}")

    # Visit poi_wreck (26,5) for +2 more TP (for exploration_t1 in Phase 4)
    print("    visiting poi_wreck for +2 TP...")
    visit_poi(26, 5)
    tp = wget("@player")["result"]["components"].get("TechPoint", {}).get("value", 0)
    check("TP>=4 (2 POIs)", tp >= 4, f"actual={tp}")

    # Research agriculture_t1 (cost 2 TP, 45s)
    print("    researching agriculture_t1...")
    research("agriculture_t1")
    wait_research_complete()
    r = wget("@colony")["result"]["components"].get("Research", {})
    check("agriculture_t1 researched", r.get("has_agriculture_t1") == 1, f"known={r.get('known')}")

    # Invite pending drifters to keep pop growing
    print("    inviting pending drifters...")
    for _ in range(3):
        if not invite_any_drifter():
            # No drifter on field — turbo ~2 days for next spawn
            start_day = wget("@colony")["result"]["components"]["Colony"]["day"]
            wait_until(
                lambda: wget("@colony")["result"]["components"]["Colony"]["day"] >= start_day + 2,
                max_s=10
            )
        else:
            step(5)

    # Advance to day 24
    print("    advancing to day 24...")
    advance_to_day(24)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    check("step>=7 (成群 gate)", s >= 7, f"actual={s} stage={c.get('stage')} pop={c.get('pop')}")
    status("Phase3-done")

    # === Phase 4: Day 24-48 (exploration + industry + trade → 成群) ===
    # Research exploration_t1 + industry_t1 (TP from POIs + scholar),
    # invite to pop>=5, trade with nomads, advance to day 48 → stage=成群.
    print("\n--- Phase 4: Day 24-48 (exploration + industry → 成群) ---")

    # Verify scholar affinity still >=50 for TP contribution in Phase 5
    aff = companion_affinity("scholar")
    check("scholar affinity>=50 (Phase 4 start)", aff is not None and aff >= 50,
          f"actual={aff}")

    # exploration_t1: use remaining 2 TP from Phase 3 POIs (poi_cave + poi_wreck - agriculture_t1)
    tp = wget("@player")["result"]["components"].get("TechPoint", {}).get("value", 0)
    check("TP>=2 (for exploration_t1)", tp >= 2, f"actual={tp}")
    print("    researching exploration_t1...")
    research("exploration_t1")
    wait_research_complete()
    r = wget("@colony")["result"]["components"].get("Research", {})
    check("exploration_t1 researched", r.get("has_exploration_t1") == 1, f"known={r.get('known')}")

    # industry_t1: wait for 2 TP from scholar contribution
    print("    researching industry_t1 (waiting for scholar TP)...")
    wait_for_tp(2)
    research("industry_t1")
    wait_research_complete()
    r = wget("@colony")["result"]["components"].get("Research", {})
    check("industry_t1 researched", r.get("has_industry_t1") == 1, f"known={r.get('known')}")

    # Invite more drifters until pop >= 5
    print("    inviting drifters to pop>=5...")
    for _ in range(5):
        c = wget("@colony")["result"]["components"]["Colony"]
        if c.get("pop", 0) >= 5:
            break
        if not invite_any_drifter():
            start_day = c["day"]
            wait_until(
                lambda: wget("@colony")["result"]["components"]["Colony"]["day"] >= start_day + 2,
                max_s=10
            )
        else:
            step(5)
    c = wget("@colony")["result"]["components"]["Colony"]
    check("pop>=5", c.get("pop", 0) >= 5, f"actual={c.get('pop')}")

    # Keep inviting until a trader companion joins the colony.
    # Trader (DRIFTER_POOL idx 10=Rix, 11=Lira) is ESSENTIAL for Phase 5: once
    # affinity>=50, the trader passively contributes +1 wheat AND +1 nomad relation
    # every 12-16s. This is the only sustainable wheat source after seeds run out.
    # With the cap at 12 (fixed from 8), Rix spawns after ~10 prior invites.
    print("    inviting drifters until trader joins (idx 10/11)...")
    for _ in range(40):  # safety limit
        if find_companion_by_role("trader") is not None:
            break
        c = wget("@colony")["result"]["components"]["Colony"]
        if c.get("day", 0) >= 70:
            print(f"    [WARN] day={c.get('day')}>=70 and no trader yet, proceeding anyway")
            break
        if not invite_any_drifter():
            # No drifter — turbo ~2 days for next spawn (cadence is 2 days)
            start_day = c["day"]
            wait_until(
                lambda: wget("@colony")["result"]["components"]["Colony"]["day"] >= start_day + 2,
                max_s=10
            )
        else:
            step(5)
    trader = find_companion_by_role("trader")
    check("trader companion present", trader is not None, "no trader companion found")
    tpos = trader.get("components", {}).get("Position", {}) if trader else {}
    print(f"    trader at ({tpos.get('x'):.1f},{tpos.get('y'):.1f}) id={trader.get('id') if trader else '?'}")

    # Raise trader affinity to >=50 for contribution trigger.
    print("    raising trader affinity (gift + talk)...")
    aff = raise_affinity("trader", target_aff=50, max_days=3)
    check("trader affinity>=50 (contribution trigger)", aff is not None and aff >= 50,
          f"actual={aff}")

    # 2 trades with nomads (exercises trade system, +4 relation, costs 6 wheat)
    print("    trading with nomads...")
    inv = wget("@player")["result"]["components"]["Inventory"]
    if inv["wheat"] >= 6:
        trade_nomads(2)
    else:
        print(f"    [WARN] wheat={inv['wheat']}, skipping trades")

    # Advance to day 48 → stage=成群 (day>=48 + pop>=5 + any faction neutral+)
    print("    advancing to day 48...")
    advance_to_day(48)
    c = wget("@colony")["result"]["components"]["Colony"]
    check("stage==成群", c.get("stage") == "成群", f"actual={c.get('stage')} day={c.get('day')} pop={c.get('pop')}")
    status("Phase4-done")

    # === Phase 5: Day 48-96 (T2 techs + monument + allied → 兴旺) ===
    # Research all 4 T2 techs (scholar provides TP), build monument (needs industry_t2!),
    # negotiate to allied (relation>=76), advance to day 96 → stage=兴旺 → settlement-founded.
    print("\n--- Phase 5: Day 48-96 (T2 techs + monument + allied → 兴旺) ---")

    # Research 4 T2 techs (each costs 4 TP, 90s). Scholar should have plenty of TP by now.
    for tech in ["survival_t2", "agriculture_t2", "exploration_t2", "industry_t2"]:
        print(f"    researching {tech}...")
        wait_for_tp(4)
        research(tech)
        wait_research_complete()
        r = wget("@colony")["result"]["components"].get("Research", {})
        check(f"{tech} researched", r.get(f"has_{tech}") == 1, f"known={r.get('known')}")

    # Gather ore for monument (4) + lamp crafting (2).
    print("    gathering ore for monument + lamp crafting...")
    inv = wget("@player")["result"]["components"]["Inventory"]
    ore_needed = max(0, 6 - inv["ore"])  # 4 for monument + 2 for crafting lamps
    if ore_needed > 0:
        gather_node(18, 3, ore_needed)
        inv = wget("@player")["result"]["components"]["Inventory"]
        while inv["ore"] < 6:
            try:
                ents = rpc("world/entities", {"components": ["Node", "Position"]})["result"]
            except Exception:
                break
            found = None
            for e in ents:
                n = e.get("components", {}).get("Node", {})
                if n.get("kind") == "ore" and (n.get("left", 0) > 0):
                    p = e.get("components", {}).get("Position", {})
                    found = p
                    break
            if not found:
                print(f"    no ore node available (ore={inv['ore']}), advancing 2 days for regrow...")
                c = wget("@colony")["result"]["components"]["Colony"]
                advance_to_day(c["day"] + 2)
                continue
            print(f"    gathering from ore node at ({found['x']:.0f},{found['y']:.0f})")
            gather_node(found["x"], found["y"], 6 - inv["ore"])
            inv = wget("@player")["result"]["components"]["Inventory"]

    # Craft planks if needed (each: 2 wood → 1 plank)
    inv = wget("@player")["result"]["components"]["Inventory"]
    plank_needed = max(0, 4 - inv["plank"])
    if plank_needed > 0:
        print(f"    crafting {plank_needed} planks...")
        craft_plank(plank_needed)

    # Craft 2 lamps (each: 1 plank + 1 ore → 1 lamp) — gifted original lamps to scholar
    inv = wget("@player")["result"]["components"]["Inventory"]
    lamp_needed = max(0, 2 - inv["lamp"])
    if lamp_needed > 0:
        print(f"    crafting {lamp_needed} lamps...")
        craft_lamp(lamp_needed)

    # Verify monument resources
    inv = wget("@player")["result"]["components"]["Inventory"]
    check("ore>=4 (monument)", inv["ore"] >= 4, f"actual={inv['ore']}")
    check("plank>=4 (monument)", inv["plank"] >= 4, f"actual={inv['plank']}")
    check("lamp>=2 (monument)", inv["lamp"] >= 2, f"actual={inv['lamp']}")
    check("wheat>=4 (monument)", inv["wheat"] >= 4, f"actual={inv['wheat']}")

    # Build monument at (11,5) — requires industry_t2 (just researched)
    print("    building monument...")
    build_monument(11, 5)
    c = wget("@colony")["result"]["components"]["Colony"]
    check("monument_built", c.get("monument_built") == 1, f"actual={c.get('monument_built')}")

    # Advance to day 96 — the trader companion (affinity>=50 since Phase 4)
    # passively contributes +1 wheat AND +1 nomad relation every 12-16s.
    print("    advancing to day 96 (trader contributes wheat + relation passively)...")
    advance_to_day(96)
    c = wget("@colony")["result"]["components"]["Colony"]
    print(f"    day={c.get('day')} stage={c.get('stage')} (expect 成群, not 兴旺 yet)")

    # Check relation — trader should have passively raised it to >=76 (allied).
    r = wget("@colony")["result"]["components"].get("Faction", {})
    rel_str = r.get("relations", "{}")
    try:
        rel = json.loads(rel_str)
    except Exception:
        rel = {}
    cur_rel = rel.get("nomads", 0)
    print(f"    nomad relation after advance: {cur_rel} (need >=76 for allied)")

    # Bounded trade loop — if relation <76, trade with trader-accumulated wheat.
    trade_attempts = 0
    while cur_rel < 76 and trade_attempts < 30:
        inv = wget("@player")["result"]["components"]["Inventory"]
        if inv["wheat"] < 3:
            c = wget("@colony")["result"]["components"]["Colony"]
            print(f"    wheat={inv['wheat']} too low, advancing 2 days for trader contribution...")
            advance_to_day(c["day"] + 2)
            continue
        trade_nomads(1)
        trade_attempts += 1
        r = wget("@colony")["result"]["components"].get("Faction", {})
        rel_str = r.get("relations", "{}")
        try:
            rel = json.loads(rel_str)
        except Exception:
            rel = {}
        cur_rel = rel.get("nomads", 0)
        print(f"    traded 1x (attempt {trade_attempts}), relation={cur_rel}, wheat={inv['wheat']}")

    check("nomads allied", cur_rel >= 76, f"actual={cur_rel} tier={r.get('tier_nomads')}")

    # Step a few ticks → stage system fires → stage=兴旺 → step 8 → settlement-founded
    print("    stepping for stage advancement...")
    step(120)
    s = wget("@quest")["result"]["components"]["QuestLog"]["step"]
    c = wget("@colony")["result"]["components"]["Colony"]
    check("stage==兴旺", c.get("stage") == "兴旺", f"actual={c.get('stage')} day={c.get('day')}")
    check("step==8 (settlement-founded)", s == 8, f"actual={s}")

    # Step a bit more to capture final state after settlement-founded emission
    step(60)
    status("Phase5-done")

    print("\n=== 通关录像完成 (96-day settlement-founded) ===")
finally:
    try: rpc("sim/quit")
    except: pass
    proc.wait(timeout=10)
