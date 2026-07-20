# Frontier 深化设计：自由运转的四循环

**日期**: 2026-07-17
**状态**: 设计已确认，待写实现计划
**前置**: [2026-06-17-frontier-sim-design.md](./2026-06-17-frontier-sim-design.md)（原白模设计）

## 一句话

把 frontier 从"8 步线性任务 → 通关结局"深化为"四个自由运转循环 → 无限玩"，前 9 天有引导曲线但无硬结局。demo 录前 9 天证明机制运转良好。

## 核心理念

**旧**：压缩纵切，每步门是"时间墙 + 资源堆"，5 分钟通关，什么都跳过。
**新**：自由运转机制，玩家想玩多久玩多久，四循环永远转，前 9 天有引导但 step 8 不发 game-won。

## 核心诊断

现有 8 步任务骨架（信标→种麦→请伙伴→立足→温饱→成群→丰碑→兴旺）本身是对的，但每步的"玩法深度"是压缩纵切：

- 信标 = 建造一下就过（无燃料/防御/调试）
- 种麦 = 等 18 秒就过（无季节/水利/轮作）
- 请伙伴 = 走过去按 I 就过（无对话深化/任务/个性）
- 立足/温饱/成群 = 等 day-floor 就过（纯时间墙）
- 丰碑 = 攒料建造就过（无仪式/意义）

根源：**任务门是时间墙 + 资源堆，不是玩法深度**。玩家在"等"而不是"玩"。

## 解决方案

不加新任务步、不堆内容量、不加硬结局。只在现有骨架上叠加 **4 个自由运转循环 + 删 step 8 结局**。

## 四个自由运转循环

### 循环 1：求生经济（永远转）

**现状**：氧/电/食/水消耗恒定，耀斑砍掉了。求生是"背景压力"不是循环。

**深化**：
- 恢复**耀斑事件**（GDD v1 砍了，但它是节奏核心）：每 1-2 游戏日一次，砸电氧，玩家必须预留缓冲
- **夜晚危险**：晚上野外刷怪（轻量——只掉心情不掉血），伙伴主动回 quarters，玩家必须天黑前回家
- **求生曲线**：早期宽（教学）、中期紧（扩张压力）、后期松（已成型）——不是恒定压力
- **消耗随人口涨**：每个伙伴有胃口，招人就得同步扩产能

**自由运转**：人口涨 → 消耗涨 → 必须扩产能 → 要资源 → 要探索 → 带回资源 → 扩产能。永不停止。

**数据**（在现有 Colony/Need 组件上扩字段，不加新组件）：
- `Colony.flare_timer`: 下次耀斑倒计时（秒）
- `Colony.flare_warning`: 耀斑预警（0/1，提前 30 秒亮 UI 警告）
- `Colony.is_night`: 是否夜晚（clock.js 已有 compTodOf，写回 Colony）
- `Colony.wild_threat`: 夜晚野外威胁等级（0=白天 / 1-3=夜晚强度）

### 循环 2：伙伴关系（永远转）

**现状**：Pip 会游荡、会说话、会因没住所离开。但"说话"是随机的，"留下"只看 quarters。

**深化**：
- 每个伙伴有 **3 条心愿**（基于 Persona.archetype 现生成）：
  - archetype=" explorer" → 心愿：去野外某 POI / 找到稀有矿 / 看日出
  - archetype="builder" → 心愿：建 N 个结构 / 建灯 / 建 quarters
  - archetype="farmer" → 心愿：种 N 茬作物 / 收获 N 次 / 吃饱饭
- 玩家做对应行为时 → 心愿点亮 → 伙伴好感 +30 → 解锁**记忆对话**（LLM 生成，关于他的过去）
- 8 步任务里 step 3（请伙伴）/ step 6（成群）的"好感≥50"门，改为"点亮 ≥2 心愿"——玩家必须真互动
- 伙伴会因低好感/低舒适离开；野外会刷新旅人（每 1-2 天一个，上限 4 个在场）

**自由运转**：伙伴来来去去，每个有独特 persona + 心愿 + 记忆。想留住喜欢的就要持续照顾。

**数据**（扩 Persona / 加 Wish 组件）：
- 新组件 `Wish { items: [{desc, done, kind, target}, ×3] }` — 3 条心愿
- `Persona.affinity`: 好感值（0-100，初始 30）
- `Persona.memory_unlocked`: 已解锁记忆条数

**心愿触发**（在现有事件上加规则）：
- `built{kind}` → 检查所有伙伴 Wish，kind 匹配 + target 达标 → done=true
- `harvested{id, n}` → 同上
- `gathered{node, id, n}` → 同上
- `entered-poi{id}` → 同上（见循环 3）
- 心愿全 done → 发 `wish-fulfilled{companion, wish_desc}` → LLM 记忆对话

### 循环 3：野外探索（永远转）

**现状**：野外区只有 6 个资源点，采完没理由再出去。"立足/温饱"门纯靠 day-floor 等。

**深化**：
- 野外区加 **3 个 POI**（兴趣点）：
  - `abandoned-camp` — 废弃营地：给科技碎片（未来扩用）/ 食物 / 偶尔伙伴
  - `cave-entrance` — 洞穴入口：给稀有矿（建丰碑需要）/ 风险高
  - `shipwreck` — 沉船：给食物 / 偶尔家具
- 每个 POI 是一个**轻量事件**：走到 → 弹选项（探索 / 无视 / 拿走）→ 有 reward + 有风险（受伤 / 伙伴掉心情 / 触发耀斑）
- POI 有**冷却 + 再生**（每游戏日刷新一次），每天有理由出去
- 资源点（ore/wood/fiber-node）也加冷却再生（现在耗尽就没了）

**自由运转**：每天有理由出去——采集 + POI + 遇旅人。野外是"经济入口"，不探索就断粮。

**数据**：
- 新组件 `Poi { kind, state: "fresh"|"looted"|"depleted", cooldown, reward_table }`
- 走到 POI 触发 `poi-reached{id}` → 规则发 `ui-activate{action:"poi-prompt"}` → 弹选项 UI
- 玩家选 → 发 `poi-choice{id, choice}` → 脚本按 reward_table 结算

### 循环 4：建设经营（永远转）

**现状**：建造结构 → 提升产能/舒适。丰碑是结局。

**深化**：
- 丰碑**不是结局**，改为"里程碑建筑"（类似动森公共设施），建了有成就感但游戏继续
- 加 **3 类装饰建筑**（纯舒适/美观，无产能）：lamp（已有）/ flower-bed / signpost
- 加 **产能升级路径**：plot→greenhouse（产量×2）/ conduit→solar-array（免耀斑影响）/ quarters→cabin（舒适×2）
- 升级 = 在原结构上"加料升级"（消耗资源 + 满足前置）

**自由运转**：永远有下一个要建的东西——更多 quarters / 更好的防御 / 装饰 / 产能升级。

**数据**：
- 扩 `Structure { kind, level }` — 加 level 字段
- 新 kind：`greenhouse` / `solar-array` / `cabin` / `flower-bed` / `signpost`
- 升级规则：`upgrade-structure` → 检查前置 + 扣料 → level+1 或 kind 变更

## 任务系统改造

### 删 step 8 结局

- step 1-7 保留（信标→种麦→请伙伴→立足→温饱→成群→丰碑），作为**前期引导曲线**
- **step 8 "game-won" 删掉**，改为"定居点建立"里程碑（建完丰碑自动发 `settlement-founded`，不是结局）
- 第 9 天后任务栏显示"自由探索中…"，玩家完全自由——四循环自驱

### step 3 / step 6 门改心愿

- step 3（请伙伴）：旧门 `companion-moved-in` → 新门 `companion-moved-in && 该伙伴点亮≥1 心愿`
- step 6（成群）：旧门 `companion_happy_count≥1` → 新门 `3 个伙伴各点亮≥2 心愿`

### gate 改造

- 旧 gate：`must_emit: game-won`
- 新 gate：`must_emit: settlement-founded`（建丰碑 = 定居点建立，不是结局）
- 录像录到建完丰碑 + 第 9 天结束，证明前 9 天曲线跑通

## 前 9 天引导曲线（demo 录这段）

| 阶段 | 游戏日 | 引导目标 | 自由度 |
|---|---|---|---|
| 起 | Day 1-2 | 学建造/种田/求生底盘（step 1-2） | 低，强引导 |
| 承 | Day 3-4 | 出野外/遇旅人/请伙伴（step 3-4） | 中，有目标但可选怎么达成 |
| 转 | Day 5-7 | 扩产能/多伙伴/耀斑压力/心愿（step 5-6） | 高，多目标并行 |
| 合 | Day 8-9 | 建丰碑里程碑（step 7）→ 自由（无 step 8） | 全自由 |
| 之后 | Day 10+ | 无任务，四循环自驱 | 完全自由 |

## 引擎能力展示（demo 隐性目标）

这套深化正好展示引擎独特价值：
- 自由运转机制 = 确定性模拟的真正用途（不是脚本序列，是系统涌现）
- 伙伴记忆对话 = LLM 集成 + 确定性录制
- 野外事件 = 规则引擎 + ctx.ask
- 9 天可重放 = 通关证书（settlement-founded）

## 不做的事（YAGNI）

- 不加新任务步（7 步引导够）
- 不加新地图（单场景双区够）
- 不加科技树（循环 3 的 POI reward 够）
- 不加贸易/多资源（现有 8 物品 + 升级路径够）
- 不加硬结局
- 不设计固定时长内容
- 不动引擎（所有深化都在 rules/scripts 层）

## 实现范围

### 新组件（schema.json 加）

- `Wish { items: [{desc, done, kind, target}, ×3] }`
- `Poi { kind, state, cooldown, reward_table }`
- 扩 `Structure { level }`
- 扩 `Persona { affinity, memory_unlocked }`
- 扩 `Colony { flare_timer, flare_warning, is_night, wild_threat }`

### 新脚本（scripts/ 加）

- `wish.js` — 心愿系统（注册/触发/解锁记忆）
- `poi.js` — POI 系统（刷新/触发/结算）
- `flare.js` — 耀斑 + 夜晚威胁系统
- 扩 `companion.js` — 心愿触发钩子 + 旅人刷新
- 扩 `economy.js` — 资源点冷却再生 + 升级建造

### 新规则（rules/ 加）

- `wish.json` — 心愿触发规则
- `poi.json` — POI 事件规则
- `flare.json` — 耀斑 + 夜晚规则
- 改 `quest.json` — step 3/6 门改心愿、删 step 8、step 7 后发 settlement-founded
- 改 `economy.json` — 升级建造规则

### 新场景内容（scenes/main.json 改）

- 野外区加 3 个 POI 实体
- gen_scene.py 同步更新

### 新 UI

- 心愿面板（点伙伴弹角色面板时显示 3 心愿）
- POI 选项弹窗（走到 POI 触发）
- 耀斑预警（屏顶 HUD 警告条）
- 升级菜单（建造模式右键结构弹升级选项）

### gate 改

- `vitric.json` gates.must_emit: `game-won` → `settlement-founded`
- 重录 `qa/clear.json`（9 天完整流程）

## 验收

- `vitric check games/frontier` 绿
- `vitric gate games/frontier` PASS（settlement-founded 发出 + 录像重放一致）
- 手玩 9 天：起承转合节奏正常，不是"什么都跳过"
- 手玩到 Day 11+：四循环自驱，玩家有理由继续
- LLM 伙伴对话：心愿解锁记忆对话，录制可重放
