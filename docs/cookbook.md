# Vitric Cookbook — 玩法系统配方

配方（recipe）= 可复制粘贴的完整步骤，拿来即用。每个配方自成一体，不依赖本文档其他配方。

---

## 配方 1：使用 inventory 模块（拾取 / 堆叠 / 溢出 / 转移）

**目标**：玩家碰到拾取物，物品进背包；同种物品堆叠；背包满时触发 `inventory-full` 事件。

### 1. 在项目 `vitric.json` 加 includes

```json
{
  "name": "my-game",
  "schema": "schema.json",
  "entry": "scenes/main.json",
  "scenes": ["scenes/main.json"],
  "rules": ["rules/game.json"],
  "scripts": ["scripts/game.js"],
  "includes": ["../../modules/inventory"],
  "seed": 42
}
```

`includes` 路径相对项目根目录。模块自动贡献：
- **schema 片段**：`Inventory` 组件（`items` / `counts` / `capacity`），按字段级合并进项目 schema。
- **规则**：`inv-pickup` / `inv-drop` / `inv-transfer`，监听 `pickup` / `drop` / `transfer` 事件并调用脚本。
- **脚本**：`__inv_pickup` / `__inv_drop` / `__inv_transfer`，执行列表操作（规则引擎不支持列表操作，所以走脚本）。

### 2. 在场景里给实体挂 `Inventory` 组件

```json
{
  "name": "player",
  "components": {
    "Position": { "x": 0, "y": 0 },
    "Collider": { "w": 1, "h": 2 },
    "Player": {},
    "Inventory": { "items": [], "counts": [], "capacity": 8 }
  }
}
```

### 3. 在自己的规则里 emit `pickup` 事件

```json
{
  "id": "collect-pickup",
  "on": { "event": "collision", "between": ["Player", "Pickup"] },
  "do": [
    {
      "emit": "pickup",
      "data": {
        "who": "self",
        "item": "other.Pickup.item",
        "count": "other.Pickup.count"
      }
    },
    { "despawn": "other" }
  ]
}
```

### 4. 监听模块发出的事件做反馈

模块会 emit：`item-picked-up` / `item-dropped` / `item-transferred` / `inventory-full` / `inventory-missing`。

```json
{
  "id": "update-hud",
  "on": { "event": "item-picked-up" },
  "do": [
    { "call": "render_inventory", "with": { "who": "event.who", "hud": "@hud" } }
  ]
}
```

```js
// scripts/game.js
vitric.fn("render_inventory", (args, ctx) => {
  const items = ctx.getField(args.who, "Inventory.items") || [];
  const counts = ctx.getField(args.who, "Inventory.counts") || [];
  const text = items.length === 0
    ? "Inventory: empty"
    : "Inventory: " + items.map((it, i) => it + "x" + counts[i]).join(", ");
  ctx.setField(args.hud, "Text.content", text);
});
```

### 5. 验证

```bash
vitric check my-game
```

完整可运行示例见 `examples/inventory-demo/`，集成测试见 `crates/vitric-cli/tests/inventory.rs`。

---

## 配方 2：HP / 伤害 / 死亡

**目标**：实体有血量，受伤扣血，血量归零时死亡（despawn）并发出事件。

### schema

```json
{
  "Health": {
    "fields": {
      "hp": { "type": "int", "default": 100, "min": 0 },
      "max": { "type": "int", "default": 100, "min": 1 }
    }
  }
}
```

### 规则

```json
{
  "rules": [
    {
      "id": "take-damage",
      "on": { "event": "damage" },
      "do": [
        { "add": "event.who.Health.hp", "by": "event.amount" }
      ]
    },
    {
      "id": "death-check",
      "on": { "event": "damage" },
      "if": [["event.who.Health.hp", "<=", 0]],
      "do": [
        { "emit": "died", "data": { "who": "event.who" } },
        { "despawn": "event.who" }
      ]
    }
  ]
}
```

### 用法

```json
{ "emit": "damage", "data": { "who": "@player", "amount": -10 } }
```

> `add` 动作用于负数即扣血。`death-check` 在同一 `damage` 事件上追加条件——两条规则都会触发，顺序是文件内的声明顺序。

---

## 配方 3：存档点（checkpoint）

**目标**：玩家碰到存档点时，记录当前位置；死亡后从最后存档点复活。

### schema

```json
{
  "Checkpoint": { "fields": {} },
  "Respawn": {
    "fields": {
      "x": { "type": "number", "default": 0 },
      "y": { "type": "number", "default": 0 }
    }
  }
}
```

### 规则

```json
{
  "rules": [
    {
      "id": "hit-checkpoint",
      "on": { "event": "collision", "between": ["Player", "Checkpoint"] },
      "do": [
        { "set": "@player.Respawn.x", "to": "other.Position.x" },
        { "set": "@player.Respawn.y", "to": "other.Position.y" },
        { "emit": "checkpoint-saved", "data": { "who": "self" } }
      ]
    },
    {
      "id": "respawn-on-death",
      "on": { "event": "died" },
      "if": [["event.who.Respawn", "exists"]],
      "do": [
        { "set": "event.who.Position.x", "to": "event.who.Respawn.x" },
        { "set": "event.who.Position.y", "to": "event.who.Respawn.y" },
        { "set": "event.who.Health.hp", "to": "event.who.Health.max" }
      ]
    }
  ]
}
```

> `respawn-on-death` 监听配方 2 的 `died` 事件。但 `died` 规则会 despawn 实体——
> 如果你想要"复活"而不是"销毁"，把配方 2 的 `despawn` 动作去掉，改用 `emit` 通知，
> 或者让 `respawn-on-death` 在 despawn 之前通过事件级联抢先把位置 / 血量复原。
> 事件级联在同一 tick 内按 FIFO 处理，声明顺序决定先后。

---

## 配方 4：使用 quest 模块（任务系统 / 状态机 / 目标追踪 / 奖励）

**目标**：NPC 给玩家派任务，玩家完成目标后回来交付，获得奖励。支持任务前置依赖（prereq）、collect / talk 两类目标、与 inventory 模块组合发奖。

### 1. includes 两个模块

```json
{
  "includes": ["../../modules/inventory", "../../modules/quest"]
}
```

quest 模块贡献 5 个组件：
- `QuestDef` — 静态：`id` / `title` / `desc` / `prereq`(前置任务 id 列表) / `reward_item` / `reward_count`
- `QuestObjective` — 静态：`kind`(`collect`|`talk`) / `arg`(物品 id 或 NPC 名) / `target`
- `QuestState` — 可变：`state`(`inactive`→`offered`→`active`→`completed`→`turned-in`) / `progress` / `assignee`
- `QuestLog` — 挂在玩家身上：`active`(已接任务 id 列表) / `completed`(已交付任务 id 列表)
- `Talked` — 挂在 NPC 身上：`count`（被对话次数，talk 目标读它）

### 2. 场景里定义任务实体 + 给玩家挂 QuestLog

任务是一个**逻辑实体**（不需要 Position/Collider），同时挂三个组件：

```json
{
  "name": "herb-quest",
  "components": {
    "QuestDef": {
      "id": "herb-quest",
      "title": "采 3 株草药",
      "desc": "去田野采 3 株草药，带回给长老。",
      "prereq": [],
      "reward_item": "coin",
      "reward_count": 5
    },
    "QuestObjective": { "kind": "collect", "arg": "herb", "target": 3 },
    "QuestState": { "state": "inactive", "progress": 0, "assignee": "" }
  }
}
```

```json
{
  "name": "player",
  "components": {
    "Player": {},
    "Position": { "x": 0, "y": 0 },
    "Inventory": { "items": [], "counts": [], "capacity": 8 },
    "QuestLog": { "active": [], "completed": [] }
  }
}
```

### 3. 在自己的规则里驱动状态机

quest 模块监听三个事件：`quest-offer` / `quest-accept` / `quest-turn-in`。你的游戏规则负责在合适的时机 emit 它们。典型模式是 NPC 碰撞驱动（碰撞每 tick 持续触发，状态机跨 tick 推进）：

```json
{
  "rules": [
    {
      "id": "elder-offer",
      "on": { "event": "collision", "between": ["Player", "Npc"] },
      "if": [["@herb-quest.QuestState.state", "==", "inactive"]],
      "do": [{ "emit": "quest-offer", "data": { "quest": "herb-quest" } }]
    },
    {
      "id": "elder-accept",
      "on": { "event": "collision", "between": ["Player", "Npc"] },
      "if": [["@herb-quest.QuestState.state", "==", "offered"]],
      "do": [{ "emit": "quest-accept", "data": { "quest": "herb-quest", "who": "self" } }]
    },
    {
      "id": "elder-turnin",
      "on": { "event": "collision", "between": ["Player", "Npc"] },
      "if": [["@herb-quest.QuestState.state", "==", "completed"]],
      "do": [{ "emit": "quest-turn-in", "data": { "quest": "herb-quest", "who": "self" } }]
    }
  ]
}
```

玩家碰到 NPC：tick 1 offer（inactive→offered），tick 2 accept（offered→active，因为状态写入是 deferred，下一 tick 可见），完成后再次碰撞 turn-in。

### 4. 目标追踪是自动的

quest 模块注册了 `quest-track` tick 系统，每 tick 扫描所有 `active` 任务：
- **collect** 目标：读 `assignee` 的 `Inventory.items/counts`，把当前持有量写进 `progress`。**这会和 inventory 模块自动组合**——你只需用配方 1 的方式 emit `pickup` 事件，背包变了，任务进度自动跟上。
- **talk** 目标：读 NPC（名为 `QuestObjective.arg`）的 `Talked.count`，>0 即完成。

`progress >= target` 时系统自动把 state 切到 `completed` 并 emit `quest-completed`。你不用写任何追踪逻辑。

### 5. 奖励也是自动的

`quest-turn-in` 触发 `__quest_turn_in`，它会：
- 把任务 id 从玩家的 `QuestLog.active` 移到 `QuestLog.completed`
- 如果 `reward_item` 非空，emit `pickup` 事件——**inventory 模块接收并加进背包**。这就是模块组合的接缝：quest 发奖 = emit pickup。

### 6. 监听模块发出的事件做反馈

模块会 emit：`quest-offered` / `quest-locked`(前置未满足) / `quest-accepted` / `quest-completed` / `quest-turned-in`。

```json
{
  "id": "update-hud",
  "on": "tick",
  "do": [{ "call": "render_hud", "with": { "who": "@player", "quest": "@herb-quest", "hud": "@hud" } }]
}
```

### 7. 任务前置依赖

`QuestDef.prereq` 列出必须先完成的任务 id。`__quest_offer` 会检查 `@player.QuestLog.completed` 是否包含全部 prereq；不满足则 emit `quest-locked`（任务保持 inactive），满足才切到 offered。用这个串起任务链：q2.prereq=["q1"]，玩家必须先交付 q1 才能接 q2。

完整可运行示例见 `examples/quest-demo/`，集成测试见 `crates/vitric-cli/tests/quest.rs`。

---

## 编写自己的模块

模块就是一个含 `module.json` 的目录：

```
modules/my-module/
  module.json       # 清单
  schema.json       # schema 片段（可选）
  rules/*.json      # 规则文件（可选）
  scripts/*.js      # 脚本文件（可选）
```

### module.json

```json
{
  "name": "my-module",
  "schema": "schema.json",
  "rules": ["rules/my-module.json"],
  "scripts": ["scripts/my-module.js"],
  "includes": []
}
```

### 合并语义

- **schema**：按组件 → 按字段合并。同名字段类型必须一致（否则 VD092）。相同声明是幂等的（不报错）。
- **rules**：追加到项目的 rules 列表。规则 id 在**文件内**必须唯一（VR002）；跨文件 / 跨模块的 id 重复当前不报错（两规则都会触发）——建议用模块名前缀（如 `inv-pickup`）避免碰撞。
- **scripts**：追加到项目的 scripts 列表。函数名（`vitric.fn` 注册）和系统名（`vitric.system` 注册）全局唯一，重复注册会抛错。
- **嵌套 includes**：模块可以 include 其他模块。路径相对模块目录。循环引用报 VD093。

### 错误码

| 码 | 含义 |
|---|---|
| VD090 | includes 指向的目录缺 module.json |
| VD091 | module.json 解析失败 |
| VD092 | 字段类型冲突（同一字段在两处类型不一致） |
| VD093 | includes 循环引用 |
