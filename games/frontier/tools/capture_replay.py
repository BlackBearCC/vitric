#!/usr/bin/env python3
"""跑《星火》通关 + 每帧抓 sim/snapshot,给关键帧标注动作(操作日志)。
输出 ROOT/_snapstream.json = {"snaps":[每帧世界状态], "acts":[每帧动作文字或""]}。
再用 scripts/build_replay.py 把它拼成自包含 HTML 回放页(项目的 web 端),浏览器直接打开看。
用法: 先 vitric run games/frontier --port 6181 不需要——本脚本自起 EXE;直接 python games/frontier/tools/capture_replay.py"""
import json,os,subprocess,time,urllib.request
import os as _os
PORT=6181
ROOT=_os.path.normpath(_os.path.join(_os.path.dirname(__file__),"..","..",".."))
EXE=_os.path.join(ROOT,"target","release","vitric.exe")
MAXF=260; SNAPS=[]; ACTS=[]
def rpc(m,p=None,t=300):
    d=json.dumps({"method":m,"params":p or {}}).encode()
    return json.loads(urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc",data=d,headers={"Content-Type":"application/json"}),timeout=t).read())
def frame(act=""):
    if len(SNAPS)>=MAXF: return
    s=rpc("sim/snapshot")
    if isinstance(s,dict) and s.get("result") is not None: SNAPS.append(s["result"]); ACTS.append(act)
def day():
    try: return wget("@colony")["result"]["components"]["Colony"]["day"]
    except: return 1
def big_cap(n,chunk=220,tag="⏳ 时间流逝…"):
    first=True
    while n>0:
        c=min(chunk,n); rpc("sim/step",{"ticks":c}); n-=c
        frame(tag if first else ""); first=False
def step(n=1): rpc("sim/step",{"ticks":n})
def click(x,y): return rpc("input/click",{"x":x,"y":y})
def inp(a,ph="pressed"): rpc("input/inject",{"action":a,"phase":ph})
def wget(e): return rpc("world/get",{"entity":e})
def qstep():
    try: return wget("@quest")["result"]["components"]["QuestLog"]["step"]
    except: return -1
def goto_companion(mx=20):
    for _ in range(mx):
        try:
            ents=rpc("world/entities",{"components":["Companion","Position"]})["result"]
            pp=wget("@player")["result"]["components"]["Position"]
        except: return
        px,py=pp["x"],pp["y"]; best=None; bd=1e9
        for e in ents:
            p=e.get("components",{}).get("Position")
            if not p: continue
            dd=(p["x"]-px)**2+(p["y"]-py)**2
            if dd<bd: bd=dd; best=p
        if not best or bd<=2.5*2.5: return
        dx,dy=best["x"]-px,best["y"]-py
        d=("right" if dx>0 else "left") if abs(dx)>=abs(dy) else ("up" if dy>0 else "down")
        inp(d); step(18); frame("🚶 走向伙伴"); inp(d,"released"); step(2)
def goto_xy(tx,ty,near=2.0,mx=40,tag="🚶 走向旅人"):
    for _ in range(mx):
        try: pp=wget("@player")["result"]["components"]["Position"]
        except: return
        px,py=pp["x"],pp["y"]; dx,dy=tx-px,ty-py
        if dx*dx+dy*dy<=near*near: return
        d=("right" if dx>0 else "left") if abs(dx)>=abs(dy) else ("up" if dy>0 else "down")
        inp(d); step(18); frame(tag); inp(d,"released"); step(2)
def plant(x,y): inp("r"); step(2); click(x,y); step(3); frame("🌱 撒下麦种")
def harvest(x,y): inp("r"); step(2); click(x,y); step(3); frame("🌾 收获麦子")
def build_wall(x,y): inp("q"); step(2); inp("2"); step(2); click(x,y); step(5); frame("🧱 建墙")
def build_beacon(x,y): inp("q"); step(2); inp("6"); step(2); click(x,y); step(5); frame("📡 建造信标")
def build_plot(x,y): inp("q"); step(2); inp("1"); step(2); click(x,y); step(5); frame("🟩 建种植台")
def build_monument(x,y): inp("q"); step(2); inp("8"); step(2); click(x,y); step(10); frame("🏛 立起丰碑")
def invite(): inp("i"); step(20); frame("🤝 邀请旅人入伙")
PLOTS=[(9,6),(9,7),(9,8),(9,9)]; PC=1500
proc=subprocess.Popen([EXE,"run","games/frontier","--port",str(PORT)],cwd=ROOT,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
try:
    for _ in range(40):
        try: rpc("ping"); break
        except: time.sleep(1)
    rpc("sim/pause"); step(4)
    frame("🌑 坠舱·开场"); frame(""); frame("")
    inp("q"); step(2)
    build_beacon(9,5)
    for p in PLOTS: build_plot(*p)
    for p in PLOTS: plant(*p)
    big_cap(PC,tag="⏳ 等麦子长大")
    for p in PLOTS: harvest(*p)
    inp("right"); 
    for _ in range(12): step(20); frame("🚶 出野外找旅人")
    inp("right","released"); step(4)
    invite()
    inp("left")
    for _ in range(12): step(20); frame("🏠 带新伙伴回家")
    inp("left","released"); step(4)
    goto_companion()
    for _ in range(2): inp("g"); step(12); frame("🎁 送礼(加好感)")
    for _ in range(3): inp("t"); step(12); frame("💬 对话(加好感)")
    for cyc in range(12):
        if qstep()>=7: break
        big_cap(PC,tag="⏳ 经营·又过一阵")
        for p in PLOTS: harvest(*p)
        for p in PLOTS: plant(*p)
        c=wget("@colony")["result"]["components"]["Colony"]
        if c["struct_count"]<3:
            for w in [(10,5),(10,6),(10,7)]: build_wall(*w)
        ents=rpc("world/entities",{"components":["Drifter"]})["result"]
        tgt=next((e for e in ents if e.get("components",{}).get("Position")),None)
        if tgt:
            pos=tgt["components"]["Position"]; goto_xy(pos["x"],pos["y"],2.0); invite(); goto_xy(8,7,2.5,tag="🏠 回聚落")
    for cyc in range(5):
        inv=wget("@player")["result"]["components"]["Inventory"]
        if inv["wheat"]>=4 and inv["plank"]>=4: break
        big_cap(PC,tag="⏳ 攒料备丰碑")
        for p in PLOTS: harvest(*p)
        for p in PLOTS: plant(*p)
    build_monument(11,5); big_cap(21600,chunk=3000,tag="✨ 丰碑落成…")
    step(4)
    frame("🎉 聚落兴旺·通关"); frame(""); frame("")
finally:
    try: rpc("sim/quit")
    except: pass
    out=_os.path.join(ROOT,"_snapstream.json")
    try:
        open(out,"w",encoding="utf-8").write(json.dumps({"snaps":SNAPS,"acts":ACTS},separators=(",",":"),ensure_ascii=False))
        print("SNAPS=%d -> %s (%d bytes)"%(len(SNAPS),out,os.path.getsize(out)))
    except Exception as e: print("write fail",e)
    try: proc.wait(timeout=10)
    except: pass
