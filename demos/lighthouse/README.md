# 灯塔守夜人（lighthouse）

一个 agent 从一句话点子、只靠引擎的机器验证链造出来的小游戏。这份 README 诚实记录整条链：
机器每一步给了什么反馈、据此改了什么——证明它是"机器逼着改"出来的，不是一次拍脑袋成的。

## 一句话点子

> 灯塔守夜人：一个单屏游戏——守夜人要爬到顶端的灯，摸到灯就赢。

## 游戏最终长啥样

- **实体**：守夜人 `keeper`（开局在塔底）、塔身 `tower`、塔基 `base`、顶端的灯 `lamp`（赢的目标）、跟随相机、提示字 `msg`。
- **规则**：按一下「上」键，守夜人往上爬 `0.225` 个世界单位（`rules/game.json` 的 `climb` 规则）；守夜人碰到灯 → 发 `game-won` 事件、灯变亮、提示字变「THE LIGHT IS LIT」。
- **胜负**：摸到灯 = 赢（`game-won`）。没有死亡，只有"天亮前（tick 预算内）爬没爬到"。
- **难度旋钮**：每按一下爬多高（`climb` 规则的 `by` 值）。爬得越少 → 同样时间内越难到顶 → 通关率越低。这是一个干净单调的旋钮，`vitric balance` 直接能调。

为什么是"按键爬"而不是"跳缺口"：见下面【踩的大坑】——机器验证链把跳跃版本毙了。

## 机器链每一步（真实输出）

### 1. `vitric check demos/lighthouse`

一遍过，没报 VD 错误：`entities 6, rules [climb, reach-lamp], initial_hash 0xa83d3f8cf1169bec`，退出 0。
（写之前先读了 `examples/jump` 学格式、读了 `crates/vitric-playtest/src/config.rs` 确认 `playtest.json` 的
确切字段名，所以 schema/scene/rules/playtest.json 一次写对。）

### 2. `vitric playtest demos/lighthouse --sessions 16 --strategy lookahead --horizon 12`

前瞻 16 局**全部通关**，机器报告（关键字段）：

```
outcome: {win: 16, lose: 0, timeout: 0, win_rate: 1.0}
reachability: {reached_events: [collision, game-won, input, start], unbeatable_by_swarm: false}
ending_coverage: {declared: [game-won], reached: [game-won], unreachable: []}
stuck_clusters: 0     inert_actions: []     numeric_breakage: 全空
summary: "...通关率 100%。结局覆盖：声明 1 个结局，全部被触达。
          ⚠ 通关几乎全靠动作「up」(占注入 100%)，疑似一招鲜，其他选择没意义。"
```

可通关 ✓、零软锁 ✓、灯（唯一结局）可达 ✓。机器还主动点了一句"一招鲜"——这游戏确实只有「上」一个有用动作，
对这个极简小游戏是预期内的诚实观察。`playtest.json` 里声明的 `distance`（守夜人→灯的曼哈顿距离）+ `goal:min`
给了前瞻方向：爬得越高距离越小，所以前瞻一路往上按。

### 3. `vitric balance --knob 'rules/game.json#/rules/0/do/0/by' --target-clear-rate 0.4:0.7 --range 0.1:0.6`

旋钮 = `climb` 规则的 `by`（每按一下爬多高），初始值 `0.5`（≈94% 通关，太简单、在目标带外）。
balance 二分搜索 4 轮（3.2 秒）：

```
range 两端定方向: by=0.1 → 通关率 0% ；  by=0.6 → 100%   （确认单调）
二分:             by=0.35 → 81.25% ；   by=0.225 → 53.1%  ← 落进 0.4~0.7 带
found_value=0.225  found_clear_rate=0.531  in_target=true  note="二分命中目标带"
```

把旋钮从 **0.5（≈94%，太简单）调到 0.225（53.1%，进带）**。balance 不动源文件（全在临时副本上跑），
拿到推荐值后我手动写回 `rules/game.json`：`by: 0.225`。

### 4. 通关录像 + `vitric replay`

录像不是手搓的——直接拿 **playtest 前瞻通关那局的录像**当 clear 录像（在 `by=0.225` 下重跑一局
`--strategy lookahead --sessions 1 --out`，outcome=Win，33 个「上」键、33 tick）。拷成 `recordings/clear.json`。

```
vitric replay demos/lighthouse recordings/clear.json
→ {"final_hash":"0xb9ff6a2804c58645","replayed_ticks":33,"verified":true}   退出 0
```

冷启动逐位重放一致（verified）✓。

### 5. `vitric gate demos/lighthouse`

清单 `vitric.json` 声明了四道门：check + 通关录像（必须 emit `game-won`）+ 断言集 + playtest 门
（前瞻 8 局，要求"能通关 / 零软锁 / 结局全可达 / 无数值崩"）。全过：

```
check                              pass   (entities 6)
playthrough:recordings/clear.json  pass   (must_emit game-won, verified true, ticks 33)
assertions                         pass   (1 条断言每 tick 求值，零违反)
playtest                           pass   (win_rate 1.0, soft_locks 0, unreachable_endings 0,
                                           inert_actions 0, numeric_breakage 全 0, sessions 8)
pass: true                         退出 0
```

**`vitric gate` 退出 0、pass=true、机器出证书。** 这是这根棒的硬验收。

### 6. `vitric bundle demos/lighthouse --out /tmp/lighthouse-linux`

gate 先行（不 PASS 不出包）→ 出自包含单文件二进制：

```
{"bundled":true,"bytes":22244990,"engine":".../release/vitric","files":7,
 "out":"/tmp/lighthouse-linux","project":"lighthouse"}
```

22MB 的 ELF 可执行，7 个项目文件内嵌。`/tmp/lighthouse-linux run-embedded --ticks 5` 自跑起来
（`vitric: running, project: lighthouse`，退出 0）。产物写 /tmp、没留仓库。

## 踩的大坑：机器把"跳缺口"版本毙了

最初**老老实实做了一句话点子的字面版**：守夜人带重力、按空格起跳、沿途是有水平缺口的台阶，
要精准跳上去（`Body{gravity}` + `Solid` 平台 + 碰撞，照着 `examples/jump` 的物理）。
这版做出来、用手写的逐帧物理模拟器算出一条 14 个输入的通关序列、`vitric replay` 也 verified、
`vitric gate` 的通关录像门也 pass 了——**单看 gate 像是成了**。

但 `vitric playtest` 把它否了，而且是反复否：

- **前瞻（lookahead）通不了**：horizon 12/30 都超时不通关，连"守夜人就在灯旁边、跳一下就赢"的
  退化关也通不了。查录像看轨迹：前瞻把守夜人跳到灯附近，但**到不了灯**——它在灯下面来回蹭
  （139 次左、118 次右），从不"跳起来的同一刻横向也对齐"。
- **根因（机器逼出来的真问题）**：前瞻按"曼哈顿距离 min"做束搜索规划，但**重力跳跃的"赢"是一瞬间
  (x,y) 的刀尖巧合**——守夜人没法悬停在距离最小点，只能在跳跃中途短暂经过。距离梯度把它导到
  "灯正下方"就卡住，因为再往上要起跳、起跳后下落距离又变大，规划器在 horizon 窗口里看不到净收益。
  这不是关卡调参能救的：连一段**纯平地、只有一个 0.8 宽小缺口、灯做成 3×3 巨大**的关，
  random/greedy/coverage 跑 24 局也**一局通不了**——策略档根本凑不出"持续朝一个方向 + 在缺口边起跳"
  这种多 tick 连续机动。
- **连带把 balance 也废了**：balance 靠通关率找难度带，但 swarm 在任何旋钮值下通关率都是 0，
  balance 没有可优化的信号。

机器给的结论很清楚：**vitric 的自动试玩策略（前瞻/贪心/随机/种子扰动）擅长"每个动作有清晰即时收益"
的游戏（卡牌、菜单、经营数值），不擅长需要精准连续操作的平台跳跃**。设计稿也写了平台跳跃的主力是
`greedy(空间)/seed-perturb`、且警告精准跳是它的弱项。

**据此改了什么**：把玩法从"带重力跳缺口"换成"按键爬"——守夜人按一下「上」就上一截（无重力、无精准时序），
摸到顶端灯就赢。这样：①前瞻/swarm 真能玩通（每按一下都让距离单调变小）；②难度旋钮（每按爬多高）
让通关率平滑单调，balance 有信号可调；③零软锁、零数值崩。灯塔"爬到顶端的光"这个核儿没变，
只是把机器验证链通不过的精准跳跃，换成了机器能验证的爬升。**这就是"机器驱动"的全部意义：
不是我觉得跳跃版好就硬上，是机器逐步把它否了、逼出一个它能闭环认证的设计。**

## 文件

```
demos/lighthouse/
  vitric.json         清单 + 四道 gate（含 playtest 门）
  schema.json         组件定义（Keeper/Lamp/Position/Collider/Sprite/Camera/Text）
  scenes/main.json    塔/灯/守夜人/相机/提示字
  rules/game.json     climb（上键爬，by=难度旋钮）+ reach-lamp（碰灯发 game-won）
  playtest.json       distance 派生量（守夜人→灯）+ goal:min，给前瞻方向
  qa/asserts.json     不变式：守夜人只升不沉（Position.y ≥ 1）
  recordings/clear.json  playtest 前瞻通关那局的录像（33 tick，verified）
  README.md           本文件
```
