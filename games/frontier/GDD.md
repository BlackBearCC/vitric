# 《星火 frontier》 — GDD 与全队合同（导演产出，改此文件=改合同）

## 一句话
俯视角科幻拓荒经营:在荒星上种田、采集、制作家具把废墟修成家;走出家园去野外探索、撞见漂泊的旅人,聊几句把他请回来住下;跟着一条主线任务(修信标→种出第一茬→请到第一个伙伴→聚落兴旺)把小聚落带活。鼠标点 UI + 方向键走,达成"兴旺"即通关(发 `game-won`)。

**这一版是"完整可通关的纵切"**:每一层都真做(UI/物品/种田/采集/制作/两片地图/请伙伴/任务/求生底盘),内容收紧到能跑通、能过 `vitric gate`。内容量后续再扩,但骨不缺。

## 直接用的引擎能力(别重造;这是上一版"像骨架"的根——没用上)
- **UI 外壳**:引擎 `UiRoot/Ui/Container(VBox|HBox|Grid)/Panel/UiLabel/Button`。背包、建造/制作菜单、任务栏、角色面板全用它搭;点击经引擎发 `ui-activate{id,action}`,玩法只接事件。**不再用一行屏幕文字当 UI。**
- **地图/探索**:`load-scene{scene}` 事件切场景 + `Persist` 组件让玩家(带背包)跨区域存活,确定性、可回放。家园↔野外靠它。
- **物品/背包**:`Inventory{items:[{id,n}], ...}` 组件存数据,增减堆叠由玩法脚本系统管。
- **对话**:`Dialogue{text}` 组件 + 话泡;伙伴脑沿用现有 `ctx.ask`(录制回放)。
- **交互**:`Collider` + `collision{a,b}` 事件做"走到资源点/传送门/伙伴身边"的触发。
- **任务/胜负**:引擎无内置任务,游戏侧用 `Quest/QuestLog` 组件 + 规则 + 事件做;交付证书=`gates.playthroughs.must_emit:"game-won"`。

> 沉引擎计划(本版不做,先在游戏里跑通形状):任务系统、伙伴邀请/作息框架里通用的那部分,等这版证明了形状,再开 `vitric-engine-dev` 一轮把通用核提进 `crates/`。本版严守"引擎不与游戏内容并行改"。

## 机制(玩法 agent 能直接翻成 rules/scripts 的具体度)
**移动/相机**:方向键设速度,相机 follow 玩家 lerp 0.12。(沿用现状)

**UI 外壳(引擎 Ui,常驻 overlay 实体,见尺寸表)**
- 顶部资源条:阶段 + 氧/电/食 + 人手(UiLabel,规则每帧刷)。
- 左下**背包**:Grid 容器,每格一件物品(图标 + 数量),读 `@player.Inventory.items`。
- 左侧**模式按钮组**:建造 / 制作 / 互动 三个 Button → 点击发 `ui-activate{action:"mode-build|mode-craft|mode-interact"}`,玩法切 `@ui.Mode.value`。
- 建造模式下弹**建造菜单**(VBox 按钮:plot/conduit/extractor/quarters/wall/椅子/灯),点选发 `ui-activate{action:"pick-<kind>"}` → 写 `@ui.Build.kind`;再左键点地块放(消耗材料,发 `built`)。
- 制作模式下弹**配方菜单**:点配方发 `ui-activate{action:"craft-<id>"}` → 玩法查料够不够、扣料、产物进背包(发 `crafted`)。
- 右上**任务栏**:UiLabel 显当前主线目标(规则按 `@quest.QuestLog` 刷)。
- 点伙伴弹**角色面板**:显其人设名/心情/舒适/作息(读伙伴组件)。

**物品/背包**:`item-gained{id,n}`/`item-spent{id,n}` 由玩法脚本统一加减(一个 `inventory` 系统,堆叠按 id 合并)。物品定义见数据表。

**种田**:建造模式放 `plot`(空地)。互动模式在 plot 上用种子→该 plot 挂 `Crop{kind,stage,timer}`,发 `planted`;`crop` 系统按 timer 推 stage(苗→长→熟),熟了发 `crop-ready{plot}` 改贴图;互动收割→产出进背包(`harvested`),plot 复空。

**采集(野外)**:野外区有资源点(矿脉 ore-node / 林木 wood-node / 纤维丛 fiber-node),挂 `Node{kind,left,cooldown}`。互动模式走近采→进背包(`gathered`),`left` 减,耗尽进 cooldown 再恢复。

**制作**:配方表(料→产物)。制作菜单点配方→玩法 `craft` 系统验料、扣料、产物进背包。家具(椅子/灯)是制作产物,建造模式摆下→挂 `Comfort{bonus}` 提升聚落/伙伴舒适。

**地图/探索**:家园区边缘有 `Portal{to}` 实体;走上去(collision)发 `load-scene{scene}`,玩家挂 `Persist`(背包跟着走)。两片:`home`(家园:可建造/种田/住人)、`wild`(野外:采集/撞旅人)。野外也有回家 Portal。

**请伙伴(agent 控制层,沿用 ctx.ask)**:野外有 1 个漂泊旅人(`Drifter`,LLM 现生成人设)。走近(collision)→ 互动"搭话"→ `ctx.ask` 拼人设提示词→话泡回复(录制)。角色面板出现**"邀请他回家"**按钮→点→发 `companion-invited{name,persona}`,写一份待入住名单(`@home_state` 组件)。回到家园:`scene-loaded(home)` 时玩法据名单 spawn 该伙伴进家园,给他 `Wander`(白天逛/晚上回 quarters 的作息)+ `Need`(舒适,没住所就掉、可挽回)。这是动森的心:出门遇见→请回来→住下过日子。

**任务(游戏侧)**:`@quest` 挂 `QuestLog{step}`。主线 4 环,任务栏显当前:
1. **修复信标**:建造模式造 `beacon` 结构 → `built{kind:"beacon"}` → step→2,发 `quest-done{id:"beacon"}`。
2. **种出第一茬**:`harvested{id:"wheat"}` 首次 → step→3。
3. **请到一个伙伴住下**:`companion-moved-in` → step→4。
4. **聚落兴旺**:结构数≥6 且 人手≥2 → 发 `settlement-thrived` → `game-won`。

**求生底盘(轻)**:沿用现有殖民地 氧/电/食(消耗随人口涨,产出结构续命,夹0不死)。是背景压力不是主线,数值给宽,别盖过经营/任务节奏。耀斑事件本版**砍掉**(先把经营/探索/请人/任务这条主线打磨顺,事件后面再回来)。

## 数据表(实现者只翻译不发明)
**物品**(id | 名 | 来源 | 用途)
- `ore` 矿石 | 野外矿脉采 | 制作材料
- `wood` 木料 | 野外林木采 | 制作木板
- `fiber` 纤维 | 野外纤维丛采 | 制作椅子
- `seed` 麦种 | 初始给5 | 种 plot
- `wheat` 麦子 | plot 收割 | 任务2/食物
- `plank` 木板 | 制作(wood×2) | 制作家具
- `chair` 椅子 | 制作(plank×1+fiber×1) | 家具,舒适+
- `lamp` 灯 | 制作(plank×1+ore×1) | 家具,舒适+

**配方**(产物 ← 料):`plank ← wood×2` | `chair ← plank×1 + fiber×1` | `lamp ← plank×1 + ore×1`

**作物**:`wheat ← seed`,生长 3 段 ×各 6 秒(共 18 秒),熟后收 wheat×2

**建造**(kind | 料):plot(免费) | wall(wood×1) | conduit(ore×1) | extractor(ore×1) | quarters(plank×2) | beacon(ore×2+plank×2) | chair/lamp(制作产物,免费摆)

**任务**:见机制4环。

## 事件表(全队接口,只有导演能加改)
`ui-activate{id,action}`(引擎发) / `item-gained{id,n}` / `item-spent{id,n}` / `built{kind,x,y}` / `planted{plot}` / `crop-ready{plot}` / `harvested{id,n}` / `gathered{node,id,n}` / `crafted{id,n}` / `load-scene{scene}`(引擎认) / `scene-loaded{scene}`(引擎发) / `drifter-talk{}` / `drifter-said{say,mood}` / `companion-invited{name,persona}` / `companion-moved-in{name}` / `companion-left{name}` / `quest-done{id}` / `settlement-thrived{}` / `game-won{}` / `llm-reply{id,text}`(引擎认,转 `__onReply`)

## 美术方向(本版灰盒优先,先白模)
- 风格词:**俯视角科幻拓荒,扁平像素白模**,先用纯色/简形占位;后续走"关卡截图→图生图"换贴图,**尺寸锁死见下表,换图不动尺寸**。
- 分辨率档:像素小图 + `vitric assets --height H`。
- 字体:中文 TTF(清单 `font` 字段,`tools/gen_font.py` 生成,不入库)。
- palette.json:美术第一批出,锁全队视觉基调(地表/结构/作物/UI 面板/文字 配色挂语义)。

## 实体尺寸表(灰盒合同,美术贴图必须匹配,改尺寸找导演)
世界物件(格 1×1):
- `player`: Sprite 0.9×0.9, Collider 0.8×0.8
- 地块 tile: 1×1 | 结构(plot/wall/conduit/extractor/quarters/beacon)、家具(chair/lamp): Sprite 1×1
- 资源点(ore/wood/fiber-node): Sprite 1×1 | drifter/companion: Sprite 0.9×0.9, Collider 0.9×0.9
- portal: Collider 1×1
- 相机:view_h 18, follow player, lerp 0.12。地图各 16×12。

UI(像素,参考视口 1920×1080):
- 顶部资源条 anchor top-center, w1600 h64 | 背包 Grid anchor bottom-left, w560 h220, 5列 | 模式按钮组 anchor center-left VBox 每个 w160 h64
- 建造/制作菜单 anchor center-left(模式组右侧)VBox 每项 w220 h56 | 任务栏 anchor top-right w560 h72 | 角色面板 anchor center w640 h420(默认隐藏,Ui.w/h=0 或移出屏)

## 地盘(文件即地盘,越界即违规)
- **美术** = `assets/` + `palette.json` + `animations.json`
- **关卡** = `scenes/`(`home.json` `wild.json`,含两区的 tile 布局 + 资源点 + portal + 各区常驻 **UI overlay 实体**结构;UiLabel 的初始文字占位,正式文案由文案 agent 行级协商)
- **玩法** = `rules/` + `scripts/`(移动/建造/制作/种田/采集/背包/求生/伙伴脑+作息+需求/任务机/区域切换/接 `ui-activate`)
- **文案** = scenes 里 UiLabel/Text/Dialogue 内容(与关卡行级协商)+ `tools/fake_llm.py` 旅人人设 + 任务文案
- **音频** = `sounds/`(本版可缓,后排)
- **QA** = `qa/`(smoke + 断言集 + 通关录像 `qa/clear.json`)
- **导演** = `schema.json` + `vitric.json` + 本 `GDD.md` + `gates` 声明
