#!/usr/bin/env python3
# build_replay.py <raw_snapshot.json> <out.html>
# Raw sim/snapshot state stream -> slim incremental render stream -> self-contained HTML canvas player (with operation log / player highlight / legend).
import json, sys, gzip, re

raw_path, out_path = sys.argv[1], sys.argv[2]
raw = json.load(open(raw_path, encoding='utf-8-sig'))
# New format {"snaps":[...],"acts":[...]}; backward compatible with old plain list
if isinstance(raw, dict):
    snaps = raw.get('snaps', []); acts = raw.get('acts', [])
else:
    snaps = raw; acts = ['' for _ in raw]
# Strip emoji/symbols (some browsers lack emoji fonts and render them as tofu boxes), keep … and other common punctuation
_EMO = re.compile('[\U0001F000-\U0001FAFF\U00002600-\U000027BF\U00002300-\U000023FF\U00002B00-\U00002BFF\U00002190-\U000021FF\U0000FE00-\U0000FE0F]')
acts = [_EMO.sub('', a).strip() for a in acts]

def rnd(o):
    if isinstance(o, float): return round(o, 3)
    if isinstance(o, dict): return {k: rnd(v) for k, v in o.items()}
    if isinstance(o, list): return [rnd(v) for v in o]
    return o

def kind_of(c, name):
    if 'Player' in c: return 'me'
    if 'Companion' in c: return 'comp'
    if 'Drifter' in c: return 'drift'
    if name in ('companion_marker', 'drifter_marker'): return 'mark'
    if 'Structure' in c: return 'struct'
    if 'Node' in c: return 'node'
    if 'Cell' in c: return 'cell'
    return 'o'

def zlayer(c):
    if 'Cell' in c: return 0
    if 'Player' in c or 'Companion' in c or 'Drifter' in c: return 3
    if 'Structure' in c: return 2
    return 1

def compact(e):
    c = e['components']; name = e.get('name') or e['id']; out = {}
    out['k'] = kind_of(c, name)
    if 'Position' in c: out['p'] = [rnd(c['Position']['x']), rnd(c['Position']['y'])]
    if 'Sprite' in c:
        s = c['Sprite']; out['s'] = [s.get('color', '#fff'), rnd(s.get('w', 1)), rnd(s.get('h', 1))]; out['z'] = zlayer(c)
    if 'Text' in c:
        t = c['Text']; out['t'] = [t.get('content', ''), rnd(t.get('size', 1)), t.get('color', '#fff'), 1 if t.get('screen') else 0]
    if 'Camera' in c:
        cm = c['Camera']; out['cam'] = [rnd(cm['x']), rnd(cm['y']), rnd(cm['scale'])]
    if 'Ui' in c:
        u = c['Ui']; out['u'] = [round(u['rx'], 1), round(u['ry'], 1), round(u['rw'], 1), round(u['rh'], 1)]
    if 'Panel' in c: out['pn'] = c['Panel'].get('color', '#fff')
    if 'UiLabel' in c:
        l = c['UiLabel']; out['l'] = [l.get('content', ''), rnd(l.get('size', 1)), l.get('color', '#fff'), l.get('align', 'center')]
    return out

frames = []
for fr in snaps:
    m = {}
    for e in fr['world']['entities']:
        ce = compact(e)
        if ce: m[e.get('name') or e['id']] = ce
    frames.append(m)
out = [frames[0]] if frames else []
for i in range(1, len(frames)):
    prev, cur = frames[i - 1], frames[i]
    delta = {n: ce for n, ce in cur.items() if prev.get(n) != ce}
    fd = {'s': delta}
    rem = [n for n in prev if n not in cur]
    if rem: fd['d'] = rem
    out.append(fd)
stream = {'rw': 1920, 'rh': 1080, 'fps': 6, 'frames': out, 'acts': acts[:len(out)]}
js = json.dumps(stream, ensure_ascii=False, separators=(',', ':'))

HTML = r'''<!doctype html><html lang="zh"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Vitric 回放 · frontier</title>
<style>
 html,body{margin:0;background:#0b0f17;color:#cfe3ff;font-family:"Noto Sans CJK SC","Microsoft YaHei",sans-serif}
 #wrap{max-width:1320px;margin:0 auto;padding:12px}
 h3{text-align:center;font-weight:500;opacity:.85;margin:6px 0}
 #main{display:flex;gap:12px;align-items:flex-start}
 #left{flex:1;min-width:0}
 canvas{background:#0e1420;display:block;width:100%;height:auto;border:1px solid #1d2740;border-radius:6px}
 #now{margin-top:8px;font-size:20px;font-weight:600;color:#ffe08a;min-height:28px;text-align:center}
 .ctrl{display:flex;align-items:center;gap:10px;margin-top:8px}
 .ctrl button{background:#26324d;color:#cfe3ff;border:0;border-radius:5px;padding:7px 16px;cursor:pointer;font-size:15px}
 .ctrl input[type=range]{flex:1}
 #fl{font-variant-numeric:tabular-nums;min-width:70px;text-align:right}
 #side{width:320px;flex:none}
 .pn{background:#121a28;border:1px solid #1d2740;border-radius:6px;padding:10px 12px;margin-bottom:10px}
 .pnh{font-size:13px;opacity:.7;margin-bottom:8px;letter-spacing:1px}
 #loglist{max-height:520px;overflow:auto;font-size:14px;line-height:1.9}
 .li{padding:2px 8px;border-radius:4px;cursor:pointer;color:#9fb6d8;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
 .li:hover{background:#1b2740}
 .li.cur{background:#2b6cb0;color:#fff;font-weight:600}
 .li .fi{opacity:.5;font-variant-numeric:tabular-nums;margin-right:8px;font-size:12px}
 .leg{display:flex;align-items:center;gap:7px;font-size:13px;margin:5px 0;color:#bcd0ee}
 .sw{width:15px;height:15px;border-radius:3px;flex:none;border:1px solid #00000040}
 .cap{text-align:center;font-size:12px;opacity:.5;margin-top:8px}
</style></head><body><div id="wrap">
<h3>Vitric 状态流回放 · 《星火》家园主循环(纯数据·canvas 逐帧重画)</h3>
<div id="main">
 <div id="left">
  <canvas id="cv" width="1280" height="720"></canvas>
  <div id="now"></div>
  <div class="ctrl"><button id="pp">暂停</button><input type="range" id="sb" min="0" value="0"><span id="fl"></span></div>
 </div>
 <div id="side">
  <div class="pn"><div class="pnh">操作日志(点击跳转)</div><div id="loglist"></div></div>
  <div class="pn"><div class="pnh">图例</div>
   <div class="leg"><span class="sw" style="background:#ffd84d"></span>你(玩家·黄圈)</div>
   <div class="leg"><span class="sw" style="background:#7ee07e"></span>伙伴(已入伙)</div>
   <div class="leg"><span class="sw" style="background:#6ad8ff"></span>旅人(可邀请)</div>
   <div class="leg"><span class="sw" style="background:#e8a23a"></span>信标 / 建筑</div>
   <div class="leg"><span class="sw" style="background:#6b8f3a"></span>种植台(可种/可收)</div>
   <div class="leg"><span class="sw" style="background:#8a6a4a"></span>资源点(可采)</div>
   <div class="leg"><span class="sw" style="background:#3a2c22"></span>地块(荒原)</div>
  </div>
 </div>
</div>
<div class="cap">整局逐帧世界状态,增量编码 + 浏览器现画(无图片/视频)· 可拖进度/暂停/点日志跳转</div>
</div><script>
const STREAM=__STREAM__;
const RW=STREAM.rw,RH=STREAM.rh,FPS=STREAM.fps||6,ACTS=STREAM.acts||[];
const states=[];let st={};
STREAM.frames.forEach((f,i)=>{
 if(i===0){st=JSON.parse(JSON.stringify(f));}
 else{if(f.s)for(const k in f.s)st[k]=f.s[k];if(f.d)for(const k of f.d)delete st[k];}
 states.push(JSON.parse(JSON.stringify(st)));
});
const cv=document.getElementById('cv'),g=cv.getContext('2d'),K=cv.width/RW;
const KIND_LBL={me:'你',comp:'伙伴',drift:'旅人'};
function outText(txt,x,y,col,px){g.font='600 '+px+'px "PingFang SC","Microsoft YaHei","Noto Sans CJK SC",sans-serif';g.lineWidth=Math.max(2,px/6);g.strokeStyle='rgba(4,8,14,.92)';g.strokeText(txt,x,y);g.fillStyle=col;g.fillText(txt,x,y);}
function render(s){
 g.fillStyle='#0e1420';g.fillRect(0,0,cv.width,cv.height);
 let cam=[0,0,40];for(const k in s){if(s[k].cam){cam=s[k].cam;break;}}
 const cx=cam[0],cy=cam[1],csc=cam[2];
 const WX=v=>(v-cx)*csc*K+cv.width/2,WY=v=>(v-cy)*csc*K+cv.height/2;
 const sp=[];for(const k in s){const e=s[k];if(e.s&&!e.u&&e.p)sp.push(e);}
 sp.sort((a,b)=>(a.z||0)-(b.z||0));
 for(const e of sp){const col=e.s[0],w=e.s[1],h=e.s[2];const ww=w*csc*K,hh=h*csc*K;g.fillStyle=col;g.fillRect(WX(e.p[0])-ww/2,WY(e.p[1])-hh/2,Math.max(1,ww-1),Math.max(1,hh-1));}
 // 玩家高亮黄圈 + 角色标签(你/伙伴/旅人)——让谁是谁一眼分清
 g.textBaseline='middle';g.textAlign='center';
 for(const k in s){const e=s[k];if(!e.p)continue;
  if(e.k==='me'){const r=Math.max(10,0.75*csc*K);g.lineWidth=3;g.strokeStyle='#ffd84d';g.beginPath();g.arc(WX(e.p[0]),WY(e.p[1]),r,0,7);g.stroke();}
  if(KIND_LBL[e.k]){const col=e.k==='me'?'#ffe9a0':e.k==='comp'?'#aef0ae':'#bfeaff';outText(KIND_LBL[e.k],WX(e.p[0]),WY(e.p[1])-0.62*csc*K,col,Math.max(11,0.34*csc*K));}
 }
 // 世界文字(头顶标记:可种植/按I邀请/G送礼…)
 for(const k in s){const e=s[k];if(e.t&&e.t[3]===0&&e.p&&!e.u&&e.t[0]){outText(e.t[0],WX(e.p[0]),WY(e.p[1]),e.t[2],Math.max(9,e.t[1]*csc*K));}}
 // UI 面板 + 文字
 for(const k in s){const e=s[k];if(e.u&&e.pn){g.fillStyle=e.pn;g.fillRect(e.u[0]*K,e.u[1]*K,e.u[2]*K,e.u[3]*K);}}
 for(const k in s){const e=s[k];if(e.u&&e.l&&e.l[0]){const rx=e.u[0],ry=e.u[1],rw=e.u[2],rh=e.u[3];const txt=e.l[0],al=e.l[3];g.fillStyle=e.l[2];g.font=Math.max(11,Math.min(rh*K*0.55,26))+'px "PingFang SC","Microsoft YaHei","Noto Sans CJK SC",sans-serif';g.textAlign=al==='start'?'left':al==='end'?'right':'center';g.textBaseline='middle';const tx=al==='start'?rx*K+8:al==='end'?(rx+rw)*K-8:(rx+rw/2)*K;g.fillText(txt,tx,(ry+rh/2)*K);}}
}
// 操作日志:把非空 acts 做成时间线
const logEntries=[];ACTS.forEach((a,i)=>{if(a)logEntries.push({i,a});});
const loglist=document.getElementById('loglist');
logEntries.forEach(en=>{const d=document.createElement('div');d.className='li';d.dataset.i=en.i;d.innerHTML='<span class="fi">'+(en.i+1)+'</span>'+en.a;d.onclick=()=>{playing=false;pp.textContent='播放';idx=en.i;show();};loglist.appendChild(d);});
function curActIdx(){let r=-1;for(let j=0;j<logEntries.length;j++){if(logEntries[j].i<=idx)r=j;else break;}return r;}
let idx=0,playing=true,last=0;
const sb=document.getElementById('sb'),fl=document.getElementById('fl'),pp=document.getElementById('pp'),now=document.getElementById('now');
sb.max=states.length-1;
function show(){
 sb.value=idx;render(states[idx]);fl.textContent=(idx+1)+' / '+states.length;
 const ci=curActIdx();
 now.textContent=ci>=0?logEntries[ci].a:'';
 [...loglist.children].forEach((c,j)=>{c.classList.toggle('cur',j===ci);});
 if(ci>=0)loglist.children[ci].scrollIntoView({block:'nearest'});
}
function loop(ts){if(playing&&ts-last>1000/FPS){idx=(idx+1)%states.length;last=ts;show();}requestAnimationFrame(loop);}
pp.onclick=()=>{playing=!playing;pp.textContent=playing?'暂停':'播放';};
sb.oninput=()=>{playing=false;pp.textContent='播放';idx=+sb.value;show();};
show();requestAnimationFrame(loop);
</script></body></html>'''
open(out_path, 'w').write(HTML.replace('__STREAM__', js))
print('compact_bytes', len(js.encode()), 'gzip', len(gzip.compress(js.encode())), 'frames', len(out), 'acts', sum(1 for a in acts if a), '-> wrote', out_path)
