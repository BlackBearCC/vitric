# Frontier 剩余增量实施计划

## 总览

> **状态(2026-07-17):全部完成。** 增量 3(请伙伴)+ 增量 4(任务通关)+ 收尾 已交付,`vitric gate` PASS。其后又叠了一轮"深化增量"(Tasks 1-9,见文末"深化增量"节)。本文件保留为历史实施计划,不再驱动后续工作。

完成 GDD 中增量 3（请伙伴）+ 增量 4（任务通关）+ 收尾，使 `vitric gate` PASS。

### 增量完成状态

| 增量 | 状态 |
|------|------|
| 3a 野外区地图扩展 | ✅ 已完成 |
| 3b 野外资源采集 | ✅ 已完成 |
| 3c 伙伴游荡(Wander) | ✅ 已完成 |
| 3d 伙伴舒适度/需求/离开 | ✅ 已完成 |
| 3e 对话+邀请 | ✅ 已完成 |
| 3f 多伙伴支持 | ✅ 已完成 |
| 4a Quest Step 3 伙伴入住 | ✅ 已完成(后被里程碑制改写,见文末) |
| 4b Quest Step 4 聚落兴旺 | ✅ 已完成(后被里程碑制改写,见文末) |
| 4c settlement-thrived → game-won | ✅ 已完成(后被 `settlement-founded` 替换,见文末) |
| 4d Gate 配置 | ✅ 已完成(`must_emit` 现为 `settlement-founded`) |
| 4e 通关录像录制 | ✅ 已完成(`qa/clear.json`,9 天 37247 tick) |
| 收尾(Polish) | ✅ 已完成 |

### 当前状态（历史快照，实施前）
- 家园 16×12 地图 + 完整 UI/建造/种田/制作/求生底盘
- 伙伴 Pip 已在家园(6,7)，有 Persona/Wander/Need/Companion 组件但**无驱动逻辑**
- 主线 step 1-2（信标→种麦）已完成
- `vitric.json` 无 `gates` 配置
- 野外区未展开（地图只有 x 0-15）

---

## 增量 3：请伙伴（Companion System）

### 3a. 野外区地图扩展

**目标**：把地图从 16×12 扩展到 28×12，右半 (x 16-27) 为野外区。

**实现**：
- 修改 `tools/gen_scene.py`：W=28，H=12（或 H=14 若需要）
- 右侧 12 列（x 16-27）全是 regolith，点缀几块岩石/r矿石
- 野外区放 6 个资源点：ore-node×2（(18,3),(24,8)）、wood-node×2（(20,6),(26,4)）、fiber-node×2（(17,8),(25,2)），挂 Node{kind, left:5, max:5, cooldown:0}
- 野外放一个 Drifter（(23,7)）：Drifter{}, Persona{name:"Lio",...}, Sprite, Collider, Position
- 重跑 gen_scene.py → 重新生成 main.json

**Node 组件字段**（schema 已有）：
```
Node{kind:"ore"|"wood"|"fiber", left:5, max:5, cooldown:0}
```

### 3b. 野外资源采集

**目标**：互动模式下点击资源点 → 采集进背包。

**实现**：
- 修改 `scripts/economy.js` 的 `interact` fn：加一条判断，点中年实体有 Node 组件且 left>0
- left-1，对应物品+1，emit inv-set（同现有扣料回写模式）
- 如果 left==0：不产=no-op（可选：cooldown 计时恢复——暂不做，GDD 纵切够了）
- 规则不动（`interact-click` 已把 event.entity/event.comp/inventory 传进 `interact`）

### 3c. 伙伴游荡（Wander System）

**目标**：Companion 实体每天在家园附近随机走动，给人"活着的邻居"感。

**实现**：
- 新建 `scripts/companion.js`
- 系统 `companion-wander`：query [Companion, Wander, Position, Velocity]
  - timer 递减；timer 到 0 时随机选 tx/ty（home_x±2 范围内整数），设 Velocity 朝 tx/ty 走
  - 走一步位置就已经+Velocity*dt（引擎内置Position+Velocity积分）
  - 到达 tx/ty（距离<0.5）时停 Velocity、重置 timer=2.0~5.0（ctx.random 走确定性）
  - 注意：引擎的 Velocity 积分是引擎内置行为（引擎认 Position+Velocity），不需要自己算位置

- 注册新脚本：加到 `vitric.json` scripts 列表

### 3d. 伙伴舒适度 / 需求 / 离开

**目标**：伙伴有 Need {comfort, quarters, leave_timer}，没住所会掉舒适，掉光就离开。

**实现**：
- `scripts/companion.js` 加系统 `companion-need`：query [Companion, Need, Census, Position]
  - 每帧：comfort -= 0.05 * dt（基准衰减）
  - 查场上有没有 Structure{kind:"quarters"} 在实体 Position ±1.5 范围内 → 有则 comfort += 0.1 * dt，quarters=1
  - 无 quarters 时：comfort 更快掉（-0.2 * dt）
  - comfort ≤ 0 时：leave_timer += dt；≥ 30 时 despawn 自己 + Census.count-1 + emit companion-left
  - comfort > 50：leave_timer 归零
  - comfort_i = Math.round(comfort) 供 HUD
  - **确定性约束**：所有计算用 ctx.dt、ctx.random()、组件字段；禁全局变量/闭包状态

### 3e. 对话 + 邀请

**目标**：走近伙伴按 t 键 → LLM 对话 → 邀请按钮 → 伙伴搬入（companion-moved-in）。

**实现**：
- 新规则 `rules/companion.json`：
  - `talk-to-drifter`：按 t 时，检测 @player 附近~3格内的@drifter（用 filter 或 if 条件检查距离）
    - 发 llm-ask 给当前伙伴或 drifter 生成回复（prompt 里带上 Persona 信息）
  - `drifter-reply`：on llm-reply → 把回复设到伙伴实体的 Text.content（用 ctx.setField）
  - 注意：llm-reply 需要路由到 __onReply；可以复用 prelude 模式

实际上 ctx.ask 需要更精细的集成：
1. 游戏侧注册 `vitric.fn("onTalkReply", (reply, ctx) => {...})`
2. 规则里 `{ "on": {"event": "llm-reply"}, "do": [{ "call": "__onReply", "with": { "id": "event.id", "text": "event.text" } }] }`

对话流程简化版（先不做完整 ctx.ask，用规则事件链够用）：
- 按 t 键 → emit `llm-ask{id:"drifter-talk", prompt:"你是{persona}，对走近的人说句话"}`
- on llm-reply filter id:"drifter-talk" → set @drifter.Text.content = event.text

简化为直接发 llm-ask，因为 ctx.ask 需要注册 fn 回调，规则级就够了。

**邀请**（简化）：
- 按 i 键（或同伴对话后自动）→ 发 companion-invited
- rule: on companion-invited → 若 quest step==3 → step=4，emit companion-moved-in
- 若@drifter存在 且 not yet moved in: drifter 变成 companion（加 Companion 组件/去 Drifter 组件,或把 drifter despawn 掉在家园重生带 Companion）

因为 Pip 已经在家园里有 Companion 组件了，简化做法：
- 在 scene 里配一个 `@drifter`（野外）+ 现有的 `@companion`（Pip，家园）
- 新规则 `drifter-talk`：按 t 键时，找到最近的 drifter/companion 实体，发 llm-ask
- 新规则 `companion-reply`：on llm-reply → 设到对应伙伴的 Text
- `invite-drifter`：按 i 键 → 如果附近有 drifter → companion-moved-in → quest step→4

但关键是 gate 录制需要确定性——LLM 回复由假 LLM 端点提供。在录制时，输入序列 + LLM 回复都会进录像，重放一致。

Gate 录制策略：
- 用 fake_llm.py 提供确定性回复
- 录制时按 t 触发对话，等待 llm-reply
- 按特定键触发邀请（i 键）
- 然后 quest 推进到 step 4

### 3f. 多伙伴支持

**目标**：对话路由能定位到具体伙伴实体。

**实现**：
- 使用 ctx.setField 在交互 fn 里直接写：`ctx.setField(drifterHandle, "Text.content", reply)`
- Drifter 对话时，规则把 event.entity（命中实体句柄）传给脚本，脚本用它 setField
- 这不涉及规则层面的复杂路由——简化为"点中谁就和谁说话"

---

## 增量 4：任务通关 + Gate

### 4a. Quest Step 3: 伙伴入住

**实现**：
- 更新 `rules/quest.json`：
  - step=3 时 banner 文案改为："目标:请到一个伙伴住下 — 走到伙伴身边按 T 搭话,按 I 邀请"
  - 新规则 `quest-step3-done`：on companion-moved-in && step==3 → step=4
  - 清理旧占位文案（step>=3 写死的文案）

### 4b. Quest Step 4: 聚落兴旺

**实现**：
- 更新 `rules/quest.json`：
  - step=4 时 banner 文案："目标:让聚落兴旺起来 — 建造≥6结构 + 人手≥2"
  - 新规则 `quest-step4-done`：每 tick，if step==4 && struct_count>=6 && pop>=2 → emit settlement-thrived

### 4c. settlement-thrived → game-won

**实现**：
- 新规则 `game-won`：on settlement-thrived → emit game-won（引擎内置事件）
- game-won 会在控制面日志里标出

### 4d. Gate 配置

**实现**：
- `vitric.json` 加 `gates` 配置：
```json
"gates": {
  "playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}],
  "check": true,
  "max_ticks": 100000
}
```

### 4e. 通关录像录制

用控制面 RPC 模拟完整一局：
1. 开局：seed start（已有）
2. 选 beacon → 点地建 beacon
3. 选 plot → 点地建 plot
4. 切互动 → 点 plot 下种
5. 等作物长 18秒（或 sim/speed 加速）
6. 收麦子
7. 触发 companion-moved-in（直接 world/set 改 quest step 或 world/spawn 触发）
8. 建足够结构（quarters, conduit, extractor, wall 等）
9. 等 conditions satisfied → game-won
10. 停录：vitric run --record qa/clear.json

---

## 收尾（Polish）

### HUD 资源条"食"字标签
- hud.json 里 `食` 字可能显示为 `食 {food_i}` → 检查格式串，确保 vs 其他项对齐
- 如果 "食" 字显示异常（可能因为字体未生成），确认 fonts/cjk.otf 生成

### 数值微调
- 求生经济数值验证：build > 跑 > 看资源曲线
- 伙伴舒适度衰减/恢复速度调舒适
- 初始资源数量确认够通关

### 烟雾测试补全
- 更新 qa/smoke.sh 测完整通关流程

---

## 执行顺序

> 全部完成。

1. ~~**写 PLAN.md**~~ ✅
2. ~~**扩展场景**~~ ✅ gen_scene.py → 28×12 + 野外节点 + drifter
3. ~~**采集脚本**~~ ✅ economy.js 加 Node 采集
4. ~~**伙伴行为**~~ ✅ companion.js(wander/need/leave) + companion.json(rules)
5. ~~**对话+邀请**~~ ✅ companion.json(llm-ask/reply) + quest.json step 3
6. ~~**通关**~~ ✅ quest.json step 4 + game-won + gates(后被里程碑制改写,见文末)
7. ~~**录制通关录像**~~ ✅ qa/clear.json
8. ~~**跑 gate**~~ ✅ vitric gate games/frontier PASS
9. ~~**收尾修复**~~ ✅ HUD 标签 / 数值 / smoke 补全
10. ~~**创建 _GAME_DONE.txt**~~ ✅

每一步的产出：check 绿 → cargo build --release (如果引擎源码改了) → vitric run + 控制面自测 → git commit → git push

---

## 深化增量 (2026-07-17,Tasks 1-9)

在增量 3/4/收尾 之上又叠了一轮深化,把"完整可通关的纵切"扩成"有四个自驱循环的无限游玩"。GDD 的"深化系统"节是这一轮的合同。`vitric gate` 仍 PASS(`must_emit` 改为 `settlement-founded`)。

| Task | 增量 | 状态 |
|------|------|------|
| 1 | 耀斑/夜循环:`Colony.flare_timer`/`flare_warning`/`is_night`/`wild_threat` 字段;flare 系统倒计时,夜落,野外威胁上升;`flare-bar` 系统 + `hud-flare-bar` 规则 + `flare_lbl` 实体 | ✅ |
| 2 | POI 系统:`Poi{kind,state,cooldown,reward_table}` 组件;`interact_poi` fn;`poi_tick` 系统;互动模式点击 POI 拾取 | ✅ |
| 3 | 伙伴需求/心愿脚手架;`apply_mood_drop` fn;`toast-show` 通用监听 | ✅ |
| 4 | 心愿系统:`Wish{items,fulfilled}` 组件;`advance_wish` fn;12 条 `wish.json` 规则;`wish_food_check` 系统;9 种心愿类别 | ✅ |
| 5 | LLM 记忆对话:心愿达成 → `triggerWishMemory` fn → `ctx.ask("llm",...)`;`onWishMemoryReply` 回调;`MEMORY_FALLBACKS` 兜底 | ✅ |
| 6 | 任务转里程碑:step 3→4 门改为 `wish-fulfilled + affinity>=60`;step 6→7 门改为 `companion_wish_count>=2`;`game-won` 规则改名 `settlement-founded`(只发 `settlement-founded`);quest-banner-8 → "自由探索中";`vitric.json` gates.must_emit → `settlement-founded` | ✅ |
| 7 | 资源点再生 + 结构升级:`Node.cooldown` + `node_regrow` 系统;`upgrade_structure` fn(plot→greenhouse / conduit→solar-array / quarters→cabin);`upgrade-button-click` 规则 | ✅ |
| 8 | UI 钩子:`flare-bar` 系统 + `hud-flare-bar` 规则 + `flare_lbl`;`kb-mode-upgrade`(u 键 → 升级模式);`upgrade-click` 规则;`Mode.value` 加 `"upgrade"` 变体 | ✅ |
| 9 | 重录 9 天通关录像:`tools/record_clear.py` 重录 `qa/clear.json`;gate PASS(37247 tick,发出 `settlement-founded`) | ✅ |

### 设计转向

这一轮把"硬结局"换成了"里程碑 + 自由探索":发完 `settlement-founded`(step 8)后游戏不结束,四个循环(资源再生 / 伙伴需求 / 心愿达成 / 耀斑夜威胁)继续自驱,玩家继续自己的故事。demo 录的是前 9 天到定居点建立为止。
