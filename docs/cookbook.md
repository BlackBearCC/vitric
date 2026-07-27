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

## 配方 5：使用 dialogue 模块（分支对话树 / 选项推进 / 与 quest 组合）

**目标**：NPC 有可分支的对话树，玩家选选项推进，对话结束时自动增量 `Talked.count`——quest 模块的 `talk` 目标读它判定完成。这是 RPG 三件套（inventory + quest + dialogue）的最后一块。

### 1. includes

```json
{ "includes": ["../../modules/inventory", "../../modules/quest", "../../modules/dialogue"] }
```

dialogue 模块贡献 2 个组件：
- `Dialogue`（挂在 NPC 上，静态）— 并行列表编码对话树：
  - `node_text` — 每个节点的 NPC 台词
  - `node_choices` — 每个节点的玩家选项，`;`-分隔（如 `"Yes;No"`）
  - `node_next` — 每个选项跳转的节点索引，`;`-分隔（如 `"1;2"`），`-1` = 结束对话
  - `entry` — 起始节点索引
- `DialogueRunner`（挂在玩家上，运行时）— `active_npc`（当前对话的 NPC）/ `current`（当前节点索引，`-1` = 不在对话中）

### 2. 在 NPC 上写对话树

```json
{
  "name": "elder",
  "components": {
    "Npc": {},
    "Talked": { "count": 0 },
    "Dialogue": {
      "node_text": [
        "Hello! The village needs herbs.",
        "Bring me 3 herbs, will you?",
        "Thank you! Come back when you have them."
      ],
      "node_choices": ["I can help.;Not now.", "I'll do it.;Maybe later.", "Goodbye."],
      "node_next": ["1;-1", "2;-1", "-1"],
      "entry": 0
    }
  }
}
```

玩家也挂 `DialogueRunner: {active_npc:"", current:-1}`。

### 3. 在规则里驱动

dialogue 模块监听两个事件：`talk`（开始对话）/ `dialogue-choose`（选选项）。你的游戏规则负责 emit 它们：

```json
{
  "id": "talk-on-collision",
  "on": { "event": "collision", "between": ["Player", "Npc"] },
  "if": [["self.DialogueRunner.current", "<", 0]],
  "do": [{ "emit": "talk", "data": { "npc": "other", "who": "self" } }]
},
{
  "id": "dialogue-choose-1",
  "on": { "event": "input", "filter": { "action": "1", "phase": "pressed" } },
  "if": [["@player.DialogueRunner.current", ">=", 0]],
  "do": [{ "emit": "dialogue-choose", "data": { "who": "@player", "choice_index": 0 } }]
}
```

碰 NPC → `talk`（仅当不在对话中）。按 1 → `dialogue-choose`（仅当在对话中）。

### 4. 推进与结束是自动的

模块的 `__dialogue_choose` 读 `node_next[current][choice_index]`：是 `-1` 或缺失 → 结束对话（`current=-1`，emit `dialogue-ended`）；否则 `current=next`，emit `dialogue-advanced`。你不用写任何推进逻辑。

### 5. 与 quest 的组合接缝：Talked.count

`__dialogue_end` 结束对话时会读 NPC 的 `Talked.count`——**如果有就 +1**（软依赖：NPC 没 Talked 组件则跳过，不报错）。而 quest 模块的 `talk` 目标读 `Talked.count > 0` 判定完成。所以三模块自动组合：

```
玩家碰NPC → talk事件 → dialogue开始 → 选选项推进 → dialogue结束
  → Talked.count++ → quest的quest-track系统读Talked → talk目标完成
```

要启用这条链，NPC 同时挂 `Dialogue` + `Talked` + `Npc`，quest 的 `QuestObjective` 设 `{"kind":"talk","arg":"elder","target":1}`。

### 6. HUD 渲染当前节点

```js
vitric.fn("render_dialogue_hud", (args, ctx) => {
  const current = ctx.getField(args.who, "DialogueRunner.current");
  if (current < 0) { /* 显示提示 */ return; }
  const texts = ctx.getField(args.npc, "Dialogue.node_text") || [];
  const choices = (ctx.getField(args.npc, "Dialogue.node_choices") || [])[current] || "";
  ctx.setField(args.hud, "Text.content", "Elder: " + texts[current] + "  [" + choices + "]");
});
```

> **注意 deferred 写入时序**：同一 tick 内，`__dialogue_choose` 的 `setField(current=-1)` 对同 tick 的 HUD 渲染不可见（deferred）。HUD 要到下一 tick 才反映结束状态。测试断言 HUD 时多 step 一 tick。

完整可运行示例见 `examples/dialogue-demo/`，集成测试见 `crates/vitric-cli/tests/dialogue.rs`。

---

## 配方 7：使用 game-flow 模块（游戏状态机 / 闭环）

**目标**：给游戏一个统一的"开始-玩-胜/负-重开"结构骨架。这是"完整游戏"与"沙盒 demo"的结构性差别——game-flow 模块让每款 Vitric 游戏都有相同的开始/结束形状。

### 1. includes

```json
{ "includes": ["../../modules/game-flow"] }
```

game-flow 模块贡献 1 个组件：
- `GameState`（挂在 `@game` 实体上）— `phase`(`title`|`playing`|`won`|`lost`|`paused`) / `time`(ticks, playing 时自动 +1) / `score`(int, 你的游戏通过 `__game_add_score` 累加)

模块监听 4 个事件：`game-start` / `game-win` / `game-lose` / `game-restart`，自动转移 phase 并 emit `game-started` / `game-won` / `game-lost` / `game-restarted`。

### 2. 场景里放一个 `@game` 实体

```json
{
  "name": "game",
  "components": {
    "GameState": { "phase": "title", "time": 0, "score": 0 }
  }
}
```

### 3. 在规则里驱动状态机

```json
{
  "id": "start-on-space",
  "on": { "event": "input", "filter": { "action": "space", "phase": "pressed" } },
  "if": [["@game.GameState.phase", "==", "title"]],
  "do": [{ "emit": "game-start" }]
},
{
  "id": "restart-on-r",
  "on": { "event": "input", "filter": { "action": "r", "phase": "pressed" } },
  "if": [["@game.GameState.phase", "==", "won"]],
  "do": [{ "call": "reset_game" }]
}
```

你的游戏规则负责：emit `game-start`（开始）/ `game-win`（胜利条件达成）/ `game-lose`（失败条件）。`game-restart` 由你的 `reset_game` 脚本 emit（重置场景后），模块把 phase 切回 `title`。

### 4. time 自动累加

模块注册了 `game-tick-time` tick 系统：`phase==playing` 时每 tick `time += 1`。你不用写计时逻辑。title/won/lost 屏 time 不动。

### 5. 重启的完整模式

重启需要重置**你的游戏状态**（玩家位置、收集物、分数等）**再** emit `game-restart`。典型 `reset_game` 脚本：

```js
vitric.fn("reset_game", (_args, ctx) => {
  ctx.setField("@player", "Position.x", 0);
  ctx.setField("@player", "Position.y", 0);
  ctx.setField("@player", "Velocity.x", 0);
  ctx.setField("@player", "Velocity.y", 0);
  // ... 重置收集物位置、清空背包等 ...
  ctx.emit("game-restart", {});  // 模块 catch → phase=title, time=0, score=0
});
```

> **注意**：`game-restart` 事件是模块重置 `GameState` 的触发器。你的 `reset_game` 脚本负责游戏内容重置，模块负责 `GameState` 重置——分工清晰。

完整可运行示例见 `examples/game-flow-demo/`，集成测试见 `crates/vitric-cli/tests/game_flow.rs`。

---

## 配方 8：四模块组合出完整 RPG 闭环（inventory + quest + dialogue + game-flow）

**目标**：把四个模块拼成一个完整的 RPG 小品——标题→对话接任务→收集草药→交付任务→胜利→重开。这是"商业游戏闭环"的最小可运行证明：四个模块无需胶水代码，纯靠规则 + 模块事件组合。

### 1. includes 四个模块

```json
{
  "includes": [
    "../../modules/inventory",
    "../../modules/quest",
    "../../modules/dialogue",
    "../../modules/game-flow"
  ]
}
```

### 2. 组合接缝

四个模块的接缝是**事件**，不是函数调用：

```
                    ┌─── game-flow ────┐
                    │ title→playing    │
                    │   →won/lost      │
                    │   →restart       │
                    └──────────────────┘
                           ▲ ▼
  collision ──→ quest-offer/accept/turn-in ──→ quest module
                    │                              │
                    │                              ▼
                    │                        quest-track (tick)
                    │                              │
                    │              ┌───────────────┘
                    │              ▼
  collision ──→ pickup event ──→ inventory module ──→ Inventory.items
                    │                                      │
                    │                                      ▼
                    │                        quest-track reads Inventory
                    │                        → progress → completed
                    │
  collision ──→ talk event ──→ dialogue module ──→ Talked.count++
                    │                              │
                    │                              ▼
                    │                        quest-track reads Talked
                    │                        → talk objective done
                    │
  quest-turned-in ──→ emit game-win ──→ game-flow module ──→ phase=won
```

### 3. 关键规则模式

**NPC 碰撞驱动 quest 状态机 + dialogue 同时启动**：

```json
{
  "id": "elder-offer-quest",
  "on": { "event": "collision", "between": ["Player", "Npc"] },
  "if": [["@herb-quest.QuestState.state", "==", "inactive"]],
  "do": [{ "emit": "quest-offer", "data": { "quest": "herb-quest" } }]
},
{
  "id": "elder-accept-quest",
  "on": { "event": "collision", "between": ["Player", "Npc"] },
  "if": [["@herb-quest.QuestState.state", "==", "offered"]],
  "do": [{ "emit": "quest-accept", "data": { "quest": "herb-quest", "who": "self" } }]
},
{
  "id": "elder-turn-in-quest",
  "on": { "event": "collision", "between": ["Player", "Npc"] },
  "if": [["@herb-quest.QuestState.state", "==", "completed"]],
  "do": [{ "emit": "quest-turn-in", "data": { "quest": "herb-quest", "who": "self" } }]
},
{
  "id": "elder-start-dialogue",
  "on": { "event": "collision", "between": ["Player", "Npc"] },
  "if": [["self.DialogueRunner.current", "<", 0]],
  "do": [{ "emit": "talk", "data": { "npc": "other", "who": "self" } }]
}
```

碰 NPC → quest 状态机跨 tick 推进（inactive→offered→active→completed→turned-in），同时 dialogue 启动。两条链独立，不互相阻塞。

**拾取物 → inventory → quest 自动追踪**：

```json
{
  "id": "herb-pickup",
  "on": { "event": "collision", "between": ["Player", "Pickup"] },
  "do": [
    { "emit": "pickup", "data": { "who": "self", "item": "other.Pickup.item", "count": "other.Pickup.count" } },
    { "call": "stash_herb", "with": { "herb": "other" } }
  ]
}
```

emit `pickup` → inventory 模块加进背包 → quest 模块的 `quest-track` tick 系统每 tick 读 `Inventory.items` 自动更新 `progress` → `progress >= target` 时自动切 `completed`。你不用写追踪逻辑。

**quest 交付 → 胜利**：

```json
{
  "id": "win-on-quest-turned-in",
  "on": { "event": "quest-turned-in" },
  "do": [{ "emit": "game-win" }]
}
```

quest 模块 emit `quest-turned-in`（含奖励 `pickup` 事件，inventory 模块接收）→ 你的规则 emit `game-win` → game-flow 模块切 `phase=won`。

### 4. 重启

`reset_game` 脚本重置游戏内容（玩家位置、收集物位置、背包、quest 状态、dialogue 状态），然后 emit `game-restart` → game-flow 模块重置 `GameState`（phase=title, time=0, score=0）。

### 5. deferred 写入时序

四模块组合时，事件链跨 tick 传播：collision（tick N）→ quest-offer carryover（tick N+1 处理）→ quest-accept carryover（tick N+2）→ ... 测试驱动时，每个状态转移后要 step 1-2 tick 让 deferred 写入 flush。详见集成测试 `tests/rpg_mini.rs` 的注释。

完整可运行示例见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`（3 例：check 通过 / 完整胜利循环 / 失败路径）。

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
