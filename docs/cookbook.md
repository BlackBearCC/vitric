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

## 配方 6：使用 combat 模块（HP / 攻击 / 伤害 / 死亡 / 治疗）

**目标**：实体有血量，可以互相攻击；血量归零时死亡并发出事件；可以治疗。比配方 2（纯规则版）更进一步：combat 模块把 HP 钳制、死亡事件、伤害链都封装好，你只管 emit `attack` / `damage` / `heal`。

### 1. includes

```json
{ "includes": ["../../modules/combat"] }
```

combat 模块贡献 2 个组件：
- `Health` — `hp`（当前血量）/ `max`（最大血量）
- `Attack` — `power`（每次攻击造成的伤害）

模块监听 3 个事件（你的规则负责 emit）：
- `attack { attacker, target }` — 攻击者打目标，模块读 `attacker.Attack.power` 并 emit `damage`
- `damage { who, amount, killer? }` — 对 `who` 造成 `amount` 伤害（正值扣血，钳到 [0, max]）
- `heal { who, amount }` — 治疗 `who` `amount` 点（钳到 [0, max]）

模块 emit 的事件（你的规则可以监听做反馈）：
- `damaged { who, amount, hp_after }` — 伤害已结算
- `healed { who, amount, hp_after }` — 治疗已结算
- `died { who, killer }` — HP 归零（**模块不 despawn**，由你的规则决定后续）

### 2. 场景里给实体挂 `Health` + `Attack`

```json
{
  "name": "player",
  "components": {
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 40 }
  }
},
{
  "name": "wolf",
  "components": {
    "Enemy": {},
    "Health": { "hp": 60, "max": 60 },
    "Attack": { "power": 20 }
  }
}
```

### 3. 在规则里驱动战斗

模块本身不决定**何时**攻击——那是游戏逻辑。你的规则负责在合适的时机 emit `attack`：

```json
{
  "id": "wolf-attacks-player",
  "comment": "Wolf attacks player on contact. self=Player, other=Enemy.",
  "on": { "event": "collision", "between": ["Player", "Enemy"] },
  "do": [{ "emit": "attack", "data": { "attacker": "other", "target": "self" } }]
},
{
  "id": "player-attacks-wolf",
  "comment": "Press X: player attacks the wolf.",
  "on": { "event": "input", "filter": { "action": "x", "phase": "pressed" } },
  "do": [{ "emit": "attack", "data": { "attacker": "@player", "target": "@wolf" } }]
}
```

### 4. 死亡处理由你决定

模块 emit `died` 但**不 despawn**——因为不同游戏对死亡的处理不同（销毁 / 复活 / 隐藏 / 触发 game-lose）。你的规则监听 `died` 决定：

```json
{
  "id": "player-dies",
  "comment": "Player died → game-lose.",
  "on": { "event": "died" },
  "if": [["event.who", "==", "@player"]],
  "do": [{ "emit": "game-lose" }]
},
{
  "id": "wolf-dies",
  "comment": "Wolf died → stash off-screen (keep entity for HUD/restart).",
  "on": { "event": "died" },
  "if": [["event.who", "==", "@wolf"]],
  "do": [{ "call": "stash_wolf", "with": { "wolf": "@wolf" } }]
}
```

```js
// stash_wolf: move off-screen instead of despawning — keeps @wolf references
// valid for HUD reads and reset_game on restart.
vitric.fn("stash_wolf", (args, ctx) => {
  ctx.setField(args.wolf, "Position.x", -100);
  ctx.setField(args.wolf, "Position.y", -100);
});
```

> **为什么不 despawn？** despawn 后 `@wolf` 实体不再存在，规则里的 `event.who == @wolf` 比较、HUD 读 `@wolf.Health.hp`、`reset_game` 复活狼——全都会报错。stash（移到屏幕外）保留了实体，所有引用仍然有效。配方 1 的 herb 拾取也用同样的模式。

### 5. 伤害链时序

一次攻击跨 3 个 tick 结算（事件级联）：

```
tick N:   collision/input → emit attack (carryover)
tick N+1: combat-on-attack → __combat_attack → emit damage (carryover)
tick N+2: combat-on-damage → __combat_damage → setField HP, emit damaged (+ died if HP<=0)
tick N+3: died → 你的 player-dies/wolf-dies 规则触发
tick N+4: game-lose → phase=lost
```

测试驱动时，每次 emit `attack` 后 step 3-4 tick 让伤害落地；HP 归零后再 step 2-3 tick 让 `died` → `game-lose` → `phase=lost` 传播完。

### 6. 与配方 2（纯规则版）的关系

配方 2 用纯规则实现了 HP/伤害/死亡——适合学习引擎机制。combat 模块是配方 2 的产品化版本：HP 钳制、`died` 事件、`killer` 透传、`heal` 治疗都封装好了。两者选其一即可，**不要同时用**（会重复扣血）。

### 7. 与其他模块组合

combat 模块的事件接口让它与其他模块无缝组合：
- **+ game-flow**：`died`（who=player）→ emit `game-lose`；`quest-turned-in` → emit `game-win`。战斗胜负直接驱动游戏状态机。
- **+ quest**：杀死敌人可以触发 `collect` 目标（敌人掉落物品 → emit `pickup` → inventory → quest-track）。
- **+ dialogue**：NPC 死亡可以跳过对话，或对话选择决定是否开战。

完整可运行示例见 `examples/combat-demo/`，集成测试见 `crates/vitric-cli/tests/combat_module.rs`；五模块组合（含 combat）见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`。

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

## 配方 8：六模块组合出完整 RPG 闭环（inventory + quest + dialogue + game-flow + combat + progression）

**目标**：把六个模块拼成一个完整的 RPG 小品——标题→对话接任务→收集草药→（躲避或击杀狼，击杀获得经验升级）→交付任务→胜利→重开。这是"商业游戏闭环"的最小可运行证明：六个模块无需胶水代码，纯靠规则 + 模块事件组合。

### 1. includes 六个模块

```json
{
  "includes": [
    "../../modules/inventory",
    "../../modules/quest",
    "../../modules/dialogue",
    "../../modules/game-flow",
    "../../modules/combat",
    "../../modules/progression"
  ]
}
```

### 2. 组合接缝

六个模块的接缝是**事件**，不是函数调用：

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
  collision ──→ attack event ──→ combat module ──→ damage ──→ HP
  input X    ──→ attack event ──→ combat module ──→ damage ──→ HP
                    │                                      │
                    │                                      ▼
                    │                        HP <= 0 → died event
                    │                              │
                    │              ┌───────────────┘
                    │              ▼
                    │    player died → game-lose → phase=lost
                    │    wolf died  → stash_wolf (off-screen)
                    │
  quest-turned-in ──→ emit game-win ──→ game-flow module ──→ phase=won

  died(wolf) ──→ gain-xp ──→ progression module ──→ leveled-up ──→ apply bonus
                    │                                                      │
                    │                                              +20 max HP, +10 ATK
                    │                                                      │
                    └──────────── stronger player ─────────────────────────┘
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

### 4. 战斗 + 升级接缝：attack / died / gain-xp / leveled-up 事件

combat 模块和其他模块一样通过事件组合。狼碰撞玩家 → emit `attack` → 模块结算伤害 → HP 归零 emit `died`（携带 `killer`）→ 你的规则 emit `gain-xp` → progression 模块加 XP、升级 → emit `leveled-up` → 你的规则应用升级奖励：

```json
{
  "id": "wolf-attacks-player",
  "on": { "event": "collision", "between": ["Player", "Enemy"] },
  "do": [{ "emit": "attack", "data": { "attacker": "other", "target": "self" } }]
},
{
  "id": "player-attacks-wolf",
  "on": { "event": "input", "filter": { "action": "x", "phase": "pressed" } },
  "do": [{ "emit": "attack", "data": { "attacker": "@player", "target": "@wolf" } }]
},
{
  "id": "player-dies-on-combat",
  "on": { "event": "died" },
  "if": [["event.who", "==", "@player"]],
  "do": [{ "emit": "game-lose" }]
},
{
  "id": "wolf-dies-on-combat",
  "on": { "event": "died" },
  "if": [["event.who", "==", "@wolf"]],
  "do": [
    { "call": "stash_wolf", "with": { "wolf": "@wolf" } },
    { "emit": "gain-xp", "data": { "who": "event.killer", "amount": 100 } }
  ]
},
{
  "id": "level-up-bonus",
  "on": { "event": "leveled-up" },
  "if": [["event.who", "==", "@player"]],
  "do": [{ "call": "apply_level_up_bonus", "with": { "who": "@player" } }]
}
```

`died` → `game-lose` 与 `quest-turned-in` → `game-win` 是两条独立的胜负路径，都汇入 game-flow 模块的 phase 状态机。`died` → `gain-xp` → `leveled-up` → `apply_level_up_bonus` 是成长路径——击杀狼让玩家变强（+HP/+ATK），但不是通关必须。玩家可以选择躲避狼直奔任务，也可以击杀狼清路升级——combat + progression 是可选的玩法深度层，不阻塞主任务链。

### 5. 重启

`reset_game` 脚本重置游戏内容（玩家位置/HP/max HP/攻击力/XP/Level、收集物位置、背包、quest 状态、dialogue 状态、狼 HP/位置），然后 emit `game-restart` → game-flow 模块重置 `GameState`（phase=title, time=0, score=0）。

### 6. deferred 写入时序

六模块组合时，事件链跨 tick 传播：collision（tick N）→ quest-offer carryover（tick N+1 处理）→ quest-accept carryover（tick N+2）→ ...；战斗链同样：collision（tick N）→ attack（N+1）→ damage（N+2）→ HP 写入 + died（N+3）→ game-lose（N+4）→ phase=lost（N+5）；升级链：died（N）→ gain-xp（N+1）→ leveled-up + XP/Level 写入（N+2）→ apply_level_up_bonus + HP/ATK 写入（N+3）→ 写入可见（N+4）。测试驱动时，每个状态转移后要 step 1-2 tick 让 deferred 写入 flush；战斗结算 step 3-4 tick；升级链 step 4-5 tick。详见集成测试 `tests/rpg_mini.rs` 的注释。

完整可运行示例见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`（4 例：check 通过 / 完整胜利循环 / 战斗失败路径 / 击杀狼+升级路径）。

---

## 配方 9：使用 progression 模块（XP / 等级 / 自动升级 / 阈值增长）

**目标**：实体有经验和等级，击杀敌人获得 XP，XP 达到阈值自动升级，升级时游戏决定奖励（+HP/+攻击力等）。这是 RPG 从"收集 → 通关"变成"战斗 → 成长 → 更强战斗"的关键系统——没有成长的 RPG 是 demo，有成长的才是商业游戏。

### 1. includes

```json
{ "includes": ["../../modules/progression"] }
```

progression 模块贡献 2 个组件：
- `XP` — `current`（当前经验）/ `threshold`（升下一级所需经验，每次升级后 ×1.5 增长）
- `Level` — `value`（当前等级，从 1 开始）/ `points`（未分配的属性点）

模块监听 1 个事件（你的规则负责 emit）：
- `gain-xp { who, amount }` — 给 `who` 加 `amount` 经验；达到阈值自动升级（支持一次跨多级）

模块 emit 的事件（你的规则监听做反馈）：
- `xp-gained { who, amount, total }` — 经验已加（total = 加完后的当前经验）
- `leveled-up { who, level, points }` — 等级提升（points = 累计未分配属性点）

> **模块不决定升级奖励**——不同游戏有不同的属性系统（HP？攻击力？技能树？）。模块只管 XP/Level 状态机，你的规则监听 `leveled-up` 决定加什么。这和 combat 模块不在 `died` 时 despawn 是同一个设计原则：模块管机制，游戏管策略。

### 2. 给实体挂 `XP` + `Level`

```json
{
  "name": "player",
  "components": {
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 40 },
    "XP": { "current": 0, "threshold": 100 },
    "Level": { "value": 1, "points": 0 }
  }
}
```

`threshold` 的起始值由你定（这里 100）。之后的增长是 `floor(threshold × 1.5)`：100 → 150 → 225 → 337 → ... 越往后升级越慢，经典 RPG 曲线。

### 3. 与 combat 模块组合：击杀 → XP → 升级

combat 模块的 `died` 事件携带 `killer`——这就是与 progression 的接缝：

```json
{
  "id": "enemy-dies-grants-xp",
  "on": { "event": "died" },
  "if": [["event.who", "==", "@enemy"]],
  "do": [{ "emit": "gain-xp", "data": { "who": "event.killer", "amount": 120 } }]
}
```

击杀敌人 → `died { who: enemy, killer: player }` → 你的规则 emit `gain-xp` → 模块加 XP → 达到阈值 emit `leveled-up` → 你的规则应用奖励。纯事件流，无函数调用，无需胶水代码。

### 4. 升级奖励：游戏决定加什么

```json
{
  "id": "level-up-bonus",
  "on": { "event": "leveled-up" },
  "if": [["event.who", "==", "@player"]],
  "do": [{ "call": "apply_level_up_bonus", "with": { "who": "@player" } }]
}
```

```js
vitric.fn("apply_level_up_bonus", (args, ctx) => {
  const who = args.who;
  // +20 max HP, full heal to new max.
  const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
  const newMax = maxHp + 20;
  ctx.setField(who, "Health.max", newMax);
  ctx.setField(who, "Health.hp", newMax);
  // +10 attack power.
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", power + 10);
});
```

这个函数**同时读写 combat 模块的组件**（Health/Attack）——它就是 progression 和 combat 的桥。模块本身不耦合（progression 不知道 Health/Attack 的存在），桥接发生在**你的游戏脚本**里。这就是 Vitric 的模块组合哲学：模块之间通过事件通信，通过游戏脚本桥接。

### 5. 阈值增长曲线

```
等级  阈值   累计XP
  1    100    0
  2    150    100
  3    225    250
  4    337    475
  5    505    812
```

每次升级 `threshold = floor(threshold × 1.5)`。如果你想用不同的曲线（线性、指数、对数），修改 `scripts/progression.js` 里的增长公式——模块是可fork的，不是黑盒。

### 6. 升级链时序

```
tick N:   died (from combat module, carries killer)
tick N+1: enemy-dies rule → emit gain-xp (carryover)
tick N+2: progression-on-gain-xp → __progression_gain_xp → XP/Level 写入, emit leveled-up (carryover)
tick N+3: level-up-bonus rule → apply_level_up_bonus → Health/Attack 写入 (deferred)
tick N+4: 所有 deferred 写入可见
```

测试驱动时，击杀后 step 4-5 tick 让整个链走完。

### 7. 与其他模块组合

- **+ combat**：`died { killer }` → `gain-xp` → `leveled-up` → `apply_level_up_bonus`（改 Health/Attack）。这是最常见的接法。
- **+ game-flow**：`leveled-up` 可以触发 UI 动画、音效、甚至解锁新区域。
- **+ quest**：`quest-turned-in` 也可以 emit `gain-xp`（交任务也有经验）。两条 XP 来源（战斗 + 任务）自然组合。
- **+ inventory**：`Level.points` 可以驱动"分配点数换物品"或"升级解锁新装备槽"。

完整可运行示例见 `examples/progression-demo/`，集成测试见 `crates/vitric-cli/tests/progression.rs`；六模块组合（含 progression）见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`。

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
