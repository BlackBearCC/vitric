# 《游戏名 项目目录名》 — GDD 与全队合同（导演产出，改此文件=改合同）

<!-- 整份合同控制在一页内。范例：examples/ember/GDD.md（23 行）、examples/spire/GDD.md。 -->

## 一句话

<!-- 类型 + 核心循环 + 胜负条件 + 操作方式，一两句说死。例：
"黑暗高塔里向上爬,点燃 4 座火盆驱散黑暗,全亮后塔顶之门开启。点灯即玩法,光照即关卡。" -->

## 机制

<!-- 逐条列玩法规则，每条一行，写到"玩法 agent 能直接翻译成规则/脚本"的具体度：
- 触发 → 效果 → 发什么事件（例：碰火盆即点燃(一次性): Light 0.15→1.6, 发 brazier-lit）
- 状态机放哪个组件（全在组件里，快照安全）
- 视觉基调一行带过（Ambient 色 / 是否 Bloom） -->

## 数据表（卡牌 / 关卡 / 物品——按游戏类型选用）

<!-- 内容数值在此定死，实现者只翻译不发明。例（卡牌）：
STRIKE 攻6 费1 ×5 | DEFEND 甲5 费1 ×4 — 牌库 12 张,洗牌用 ctx.random -->

## 事件表（全队接口，只有导演能加改）

<!-- 事件名{字段} 一行列全。这是规则↔脚本↔音频↔文案↔QA 的共同语言。例：
brazier-lit{n} / all-lit{} / hero-died{deaths} / game-won{} -->

## 实体尺寸表（灰盒合同，美术贴图必须匹配，改尺寸找导演）

<!-- 名字 → Collider w×h, Sprite w×h, 位置/排布参数。此刻锁定：
关卡用纯色块按此搭灰盒；美术贴图按此出尺寸；出图后只换 Sprite.image，物理与布局零波动。
顺手定死相机：view_h / follow / lerp。例：
hero: Collider 1.3x2.1, Sprite 1.7x2.2 | tile: 2x2
塔宽 x∈[-8,8],高 y∈[0,40],camera view_h 20 follow hero lerp 0.12 -->

## 地盘（文件即地盘，越界即违规）

<!-- 每个文件/目录恰好归一个角色。例：
美术=assets/+animations.json+palette.json | 关卡=scenes/ | 玩法=rules/+scripts/
音频=sounds/ | 文案=scenes 里的 Text 内容(与关卡协商行级地盘)或单列文案场景 | QA=qa/
schema.json/vitric.json/本文件=导演 -->
