# 错误码目录

每条校验错误带稳定错误码、精确路径、修复提示。这里是全集索引（按码查修法）。

## VD —— 数据层（schema / 场景 / 项目 / 动画）

| 码 | 含义 | 修法 |
|---|---|---|
| VD001 | schema 文件结构错误（缺 components 或组件缺 fields） | 顶层 `{"components": {"组件名": {"fields": {...}}}}` |
| VD002 | 组件值不是对象 | 组件值写成 `{"字段": 值}` 对象 |
| VD003 | 组件里出现 schema 没定义的字段 | 查错误里列出的合法字段；要新字段先改 schema |
| VD004 | 缺少必填字段（required: true） | 显式给值 |
| VD005 | 场景/spawn 引用了未知组件 | 查错误里列出的已定义组件；先在 schema 里加 |
| VD010 | enum 类型缺 variants | `{"type":"enum","variants":["a","b"]}` |
| VD011 | list 类型缺 of | `{"type":"list","of":{"type":"number"}}` |
| VD012 | 未知字段类型 | 可用: number/int/bool/text/vec2/entity/enum/list |
| VD013 | 字段定义缺 type | 每个字段形如 `{"type":"number","default":0}` |
| VD020 | 值类型不符 | 按错误提示的期望类型改；int 不收小数，vec2 必须正好 {x,y} |
| VD021 | 数值超出 min/max | 改值或放宽 schema 的范围 |
| VD022 | 枚举值不在可选项里 | 用错误里列出的可选项之一 |
| VD030 | 场景缺 entities 数组 | 顶层 `{"entities": [...]}` |
| VD031 | 实体名重复 | 实体名场景内唯一 |
| VD032 | 实体缺 components 对象 | 至少给空对象 `{}` |
| VD033 | entity 字段引用了不存在的实体名 | 查错误里列出的命名实体 |
| VD034 | 实例化时实体名已被占用 | World 里已有同名实体（场景重复加载？） |
| VD040 | 文件缺失/读不到 | 清单里列的路径必须存在 |
| VD041 | vitric.json 清单解析失败 | 必填: name/schema/entry；可选: scenes/rules/scripts/animations/font/seed |
| VD042 | entry 场景不在 scenes 列表里 | 把它加进 scenes 数组 |
| VD050 | 动画文件缺 clips 对象 | `{"clips": {"名": {"frames": [...], "fps": 8}}}` |
| VD051 | 动画片段解析失败 | 片段写法 `{"frames": ["a.png"], "fps": 8, "loop": true}` |
| VD052 | 片段 frames 为空 | 至少一帧 |
| VD053 | 片段 fps 为 0 | 常用 4-12 |

## VR —— 规则层

| 码 | 含义 | 修法 |
|---|---|---|
| VR001 | 规则文件缺 rules 数组 | 顶层 `{"rules": [...]}` |
| VR002 | 规则 id 重复 | id 全局唯一 |
| VR003 | 规则缺 id | 起个说明意图的 id |
| VR004 | between 不是两个组件名 | `"between": ["Player", "Coin"]` |
| VR005 | between 配了非 collision 事件 | between 只配 collision；其他事件用 filter |
| VR006 | 缺触发器 on | `"on": "tick"` 或 `"on": {"event": "事件名"}` |
| VR007 | each 不是非空组件名数组 | `"each": ["Position", "Velocity"]` |
| VR008 | 条件不是三元组 | `["self.Health.hp", "<", 10]`；exists/!exists 可两元 |
| VR009 | 未知操作符 | 可用: == != < <= > >= exists !exists |
| VR010 | 未知动作类型 | 可用: set/add/spawn/despawn/emit/call（按对象的键识别） |
| VR011 | do 数组缺失或为空 | 至少一个动作 |

## 运行时错误（无码，消息自含路径与修法）

- ECS：死实体操作 / 未知组件（带现有组件列表）/ 字段路径无效（带现有字段列表）/ 实体名占用
- 规则执行：`规则 "id" 在 do/N 执行失败: 原因`；事件级联超 8 层报触发链
- 脚本：加载失败（带文件名）/ 运行异常 / **越权写入**（writes 没声明的组件）/ spawn 未过 schema
- 模拟：组件数据不是数字（内建运动/碰撞要求）/ 重放跑偏（报精确 tick 和期望/实际哈希）
- 渲染：Sprite.image 不在素材库（带现有素材列表）/ 颜色格式 / 超 2048 尺寸
- 光照：Light.kind 未知（带合法取值 point/spot/directional）/ 超 64 盏（三种 kind 合计）/ point·spot 缺 Position 或 radius ≤ 0 / spot 缺 angle·dir 或 angle 不在 1..=360 / directional 缺 dir——全部显式报错带写法示例
- 动画：Anim.clip 未定义（带已定义片段列表）
