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

## 配方 8：七模块组合出完整 RPG 闭环（inventory + quest + dialogue + game-flow + combat + progression + loot）

**目标**：把七个模块拼成一个完整的 RPG 小品——标题→对话接任务→收集草药→（躲避或击杀狼，击杀掉落金币+获得经验升级）→交付任务→胜利→重开。这是"商业游戏闭环"的最小可运行证明：七个模块无需胶水代码，纯靠规则 + 模块事件组合。

### 1. includes 七个模块

```json
{
  "includes": [
    "../../modules/inventory",
    "../../modules/quest",
    "../../modules/dialogue",
    "../../modules/game-flow",
    "../../modules/combat",
    "../../modules/progression",
    "../../modules/loot"
  ]
}
```

### 2. 组合接缝

七个模块的接缝是**事件**，不是函数调用：

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

  died(wolf) ──→ loot module ──→ pickup(coin) ──→ inventory module ──→ Inventory += coin
                    │
                    └──→ gain-xp ──→ progression module ──→ leveled-up ──→ apply bonus
                                                                      │
                                                              +20 max HP, +10 ATK
                                                                      │
                                                                      └── stronger player
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

### 4. 战斗 + 掉落 + 升级接缝：attack / died / loot / gain-xp / leveled-up 事件

combat 模块和其他模块一样通过事件组合。狼碰撞玩家 → emit `attack` → 模块结算伤害 → HP 归零 emit `died`（携带 `killer`）→ **loot 模块自动滚 wolf 的 LootTable，emit `pickup` 给 killer** → inventory 模块接收 → 同时你的规则 emit `gain-xp` → progression 模块加 XP、升级 → emit `leveled-up` → 你的规则应用升级奖励：

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
  "comment": "wolf 死亡：stash + grant XP。loot 模块的 loot-on-died 规则同时触发，自动滚 LootTable 掉落金币——无需在这里写掉落逻辑。",
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

wolf 需要挂 `LootTable` 组件（见配方 10）。`died` 触发后，loot 模块的 `loot-on-died` 规则和你的 `wolf-dies-on-combat` 规则**同一 tick 内并行触发**——loot 模块 emit `pickup`（inventory 自动接收），你的规则 emit `gain-xp`（progression 自动接收）。两条链独立。

`died` → `game-lose` 与 `quest-turned-in` → `game-win` 是两条独立的胜负路径，都汇入 game-flow 模块的 phase 状态机。`died` → `loot` + `gain-xp` → `leveled-up` → `apply_level_up_bonus` 是成长路径——击杀狼让玩家变强（+HP/+ATK）并获得金币，但不是通关必须。玩家可以选择躲避狼直奔任务，也可以击杀狼清路升级攒钱——combat + loot + progression 是可选的玩法深度层，不阻塞主任务链。

### 5. 重启

`reset_game` 脚本重置游戏内容（玩家位置/HP/max HP/攻击力/XP/Level、收集物位置、背包、quest 状态、dialogue 状态、狼 HP/位置），然后 emit `game-restart` → game-flow 模块重置 `GameState`（phase=title, time=0, score=0）。

### 6. deferred 写入时序

七模块组合时，事件链跨 tick 传播：collision（tick N）→ quest-offer carryover（tick N+1 处理）→ quest-accept carryover（tick N+2）→ ...；战斗链同样：collision（tick N）→ attack（N+1）→ damage（N+2）→ HP 写入 + died（N+3）→ game-lose（N+4）→ phase=lost（N+5）；掉落链：died（N）→ loot roll + pickup（N+1）→ inventory 写入（N+2）→ 写入可见（N+3）；升级链：died（N）→ gain-xp（N+1）→ leveled-up + XP/Level 写入（N+2）→ apply_level_up_bonus + HP/ATK 写入（N+3）→ 写入可见（N+4）。掉落链和升级链**并行**，都在 died 后的同一 tick 启动。测试驱动时，每个状态转移后要 step 1-2 tick 让 deferred 写入 flush；战斗结算 step 3-4 tick；升级+掉落链 step 4-5 tick。详见集成测试 `tests/rpg_mini.rs` 的注释。

完整可运行示例见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`（4 例：check 通过 / 完整胜利循环 / 战斗失败路径 / 击杀狼+掉落+升级路径）。

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

完整可运行示例见 `examples/progression-demo/`，集成测试见 `crates/vitric-cli/tests/progression.rs`；七模块组合（含 progression）见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`。

---

## 配方 10：使用 loot 模块（死亡掉落 / 战利品表 / 确定性 RNG / 自动拾取）

**目标**：敌人死亡时按战利品表掉落物品，自动拾取到击杀者的背包。这是 RPG 经济循环的核心环节——combat → loot → inventory 形成闭环，玩家击杀怪物获得物品，物品可以是货币（coin）、消耗品（herb）或装备。没有掉落的战斗只是"清障"，有掉落才是"刷装备/攒钱"的商业 RPG 玩法。

### 1. includes

```json
{ "includes": ["../../modules/combat", "../../modules/inventory", "../../modules/loot"] }
```

loot 模块依赖 combat 的 `died` 事件，并 emit `pickup` 给 inventory 模块接收。所以这三个模块通常一起 includes。loot 模块贡献 1 个组件：

- `LootTable`（挂在可掉落的实体上，静态）— 并行列表编码掉落条目（和 dialogue 模块的 node_* 同模式）：
  - `items` — 物品 id 列表（如 `["coin", "herb", "gem"]`）
  - `count_mins` — 每条最小数量（含，如 `[2, 1, 1]`）
  - `count_maxs` — 每条最大数量（含，如 `[5, 2, 1]`）；缺省或 < min 时 = min（固定数量）
  - `chances` — 每条掉落概率 0.0-1.0（如 `[1.0, 0.75, 0.25]`）；缺省时 = 1.0（必定掉落）

模块监听 1 个事件（combat 模块 emit 的）：
- `died { who, killer }` — 滚 `who` 的 LootTable，每条成功 emit `pickup` 给 killer

模块 emit 2 个事件：
- `pickup { who, item, count }` — 自动拾取到 killer 的背包（inventory 模块接收并加进 Inventory）
- `loot-dropped { who, killer, item, count }` — 每条掉落条目（游戏可监听做飘字/音效/HUD 更新）

### 2. 场景里给敌人挂 LootTable

```json
{
  "name": "wolf",
  "components": {
    "Enemy": {},
    "Health": { "hp": 60, "max": 60 },
    "Attack": { "power": 20 },
    "LootTable": {
      "items": ["coin", "fang"],
      "count_mins": [2, 1],
      "count_maxs": [5, 1],
      "chances": [1.0, 0.3]
    }
  }
}
```

这条 LootTable 的含义：
- **coin**：100% 掉落 2-5 个（随机数量）
- **fang**：30% 掉落 1 个（固定数量，因为 min=max=1）

### 3. 不用写掉落规则——loot 模块自动触发

loot 模块的 `loot-on-died` 规则监听 `died` 事件，自动滚 LootTable。你的游戏规则只需要处理 `died` 的其他后果（stash/respawn/game-lose 等），掉落完全自动：

```json
{
  "id": "wolf-dies",
  "comment": "wolf 死亡：stash + grant XP。loot 模块的 loot-on-died 规则自动并行触发，无需在这里写掉落逻辑。",
  "on": { "event": "died" },
  "if": [["event.who", "==", "@wolf"]],
  "do": [
    { "call": "stash_wolf", "with": { "wolf": "@wolf" } },
    { "emit": "gain-xp", "data": { "who": "event.killer", "amount": 100 } }
  ]
}
```

`died` 触发后，loot 模块的 `loot-on-died` 规则和你的 `wolf-dies` 规则**同一 tick 内并行触发**。loot 模块 emit `pickup` → inventory 模块接收 → 物品进背包。你的规则 emit `gain-xp` → progression 模块接收 → 升级。两条链独立，不互相阻塞。

### 4. 监听 loot-dropped 做反馈

```json
{
  "id": "loot-feedback",
  "on": { "event": "loot-dropped" },
  "do": [{ "call": "show_loot_text", "with": { "item": "event.item", "count": "event.count", "hud": "@hud" } }]
}
```

```js
vitric.fn("show_loot_text", (args, ctx) => {
  const text = "+" + args.count + " " + args.item;
  ctx.setField(args.hud, "Text.content", text);
});
```

`pickup` 事件被 inventory 模块消费（加进背包），`loot-dropped` 是给你做反馈的——飘字、音效、HUD 闪烁。

### 5. 确定性 RNG

loot 模块用 `ctx.random_stream("loot")`——一个**命名子流**，种子由 `(world_seed, "loot")` 决定，独立于主 RNG 流。这意味着：

- **掉落结果可复现**：同 seed + 同输入 = 同掉落。回放/录像/测试都确定。
- **不干扰其他系统**：loot 滚动不消耗主 RNG 流，不会让其他系统的随机数发生偏移。
- **多次击杀有序**：同 tick 内多个敌人死亡时，按事件 FIFO 顺序从子流取数，确定且可复现。

测试时可以断言"两次运行产生完全相同的 Inventory.items/counts"（见 `tests/loot.rs` 的 `loot_roll_is_deterministic_across_runs`）。

### 6. 掉落链时序

```
tick N:   died (from combat module, carries killer)
tick N+1: loot-on-died rule → __loot_roll → emit pickup + loot-dropped (carryover)
tick N+2: inv-pickup rule → __inv_pickup → Inventory 写入 (deferred)
tick N+3: deferred 写入可见
```

掉落链和升级链（见配方 9 第 6 节）**并行**，都在 `died` 后的同一 tick 启动。测试驱动时，击杀后 step 4-5 tick 让两条链都走完。

### 7. 与其他模块组合

- **+ combat**：`died { who, killer }` → loot roll → `pickup` → inventory。这是最常见的接法，三者通常一起 includes。
- **+ inventory**：loot emit `pickup`，inventory 自动接收并加进背包。无胶水代码。
- **+ progression**：`died` 同时触发 loot（掉物品）和 gain-xp（涨经验），两条链并行。玩家击杀 → 获得物品 + 升级，完整的"刷怪成长"循环。
- **+ game-flow**：`loot-dropped` 可以触发 UI 动画、音效；`pickup` 可以累加 score。

### 8. 无 killer 时的行为

如果 `died` 事件没有 `killer`（环境死亡、摔死等），loot 模块**跳过滚动**——没有自动拾取的目标。游戏如果想做"地面掉落"（spawn 一个 Pickup 实体让玩家走过去捡），可以监听 `died` 自己处理，不依赖 loot 模块的自动拾取。

完整可运行示例见 `examples/loot-demo/`，集成测试见 `crates/vitric-cli/tests/loot.rs`；七模块组合（含 loot）见 `examples/rpg-mini/`，集成测试见 `crates/vitric-cli/tests/rpg_mini.rs`。

---

## 配方 11：使用 shop 模块（商店 / 买卖 / 货币 / 库存 / 经济闭环）

**目标**：NPC 商店出售物品，玩家用货币（coin）购买；玩家也可以把不需要的物品卖给商店换钱。这是 RPG 经济闭环的核心——combat → loot → coins → shop → items → stronger。没有商店的 RPG 只是"刷怪"，有商店才是"刷怪攒钱买装备"的商业 RPG 玩法。

### 1. includes

```json
{ "includes": ["../../modules/combat", "../../modules/inventory", "../../modules/loot", "../../modules/shop"] }
```

shop 模块依赖 Inventory 组件（直接读写，原子操作）。通常和 combat + loot + inventory 一起 includes，构成完整经济循环。shop 模块贡献 1 个组件：

- `Shop`（挂在 NPC 商人上，静态）— 并行列表编码商店目录（和 Dialogue/LootTable 同模式）：
  - `currency` — 货币物品 id（默认 `"coin"`）；游戏决定什么算钱
  - `items` — 出售的物品 id 列表（如 `["potion", "sword"]`）
  - `prices` — 每个物品的买价，以货币为单位（如 `[3, 50]`）
  - `stocks` — 每个物品的库存；`-1` = 无限，`0` = 售罄，`N` = 限量（如 `[-1, 2]`）

模块监听 2 个事件（你的规则负责 emit）：
- `shop-buy { who, shop, item, count }` — 从 `shop` 购买 `count` 个 `item` 给 `who`
- `shop-sell { who, shop, item, count }` — 把 `who` 的 `count` 个 `item` 卖给 `shop`

模块 emit 的事件（你的规则可以监听做反馈）：
- `item-bought { who, shop, item, count, total_price }` — 购买成功
- `item-sold { who, shop, item, count, total_price }` — 出售成功
- `shop-not-for-sale { who, shop, item }` — 物品不在商店目录
- `shop-out-of-stock { who, shop, item, available }` — 库存不足
- `shop-insufficient-funds { who, item, count, needed, have }` — 钱不够
- `shop-inventory-full { who, item, count }` — 买家背包满
- `shop-missing-item { who, item, count }` — 卖家没有该物品

### 2. 场景里给 NPC 挂 Shop 组件

```json
{
  "name": "merchant",
  "components": {
    "Npc": {},
    "Shop": {
      "currency": "coin",
      "items": ["potion", "key", "sword"],
      "prices": [3, 10, 50],
      "stocks": [-1, 2, 1]
    }
  }
}
```

这个商店的目录：
- **potion**：3 coin，无限供应（-1）
- **key**：10 coin，库存 2 把
- **sword**：50 coin，库存 1 把

### 3. 在规则里驱动买卖

shop 模块不决定**何时**买卖——那是游戏逻辑。你的规则负责在合适的时机 emit `shop-buy` / `shop-sell`：

```json
{
  "id": "buy-potion-on-b",
  "comment": "Press B: buy 1 potion from the merchant.",
  "on": { "event": "input", "filter": { "action": "b", "phase": "pressed" } },
  "do": [{ "emit": "shop-buy", "data": { "who": "@player", "shop": "@merchant", "item": "potion", "count": 1 } }]
},
{
  "id": "sell-key-on-s",
  "comment": "Press S: sell 1 key to the merchant (sell price = floor(10/2) = 5 coins).",
  "on": { "event": "input", "filter": { "action": "s", "phase": "pressed" } },
  "do": [{ "emit": "shop-sell", "data": { "who": "@player", "shop": "@merchant", "item": "key", "count": 1 } }]
}
```

### 4. 买卖结算是自动的

shop 模块的 `__shop_buy` / `__shop_sell` 直接读写 Inventory 组件（原子操作，避免双花竞态）：

- **买**：读 Inventory → 检查货币是否够 → 扣货币 → 加物品 → 写回 Inventory → 扣库存 → emit `item-bought`
- **卖**：读 Inventory → 检查物品是否够 → 扣物品 → 加货币 → 写回 Inventory → emit `item-sold`

卖价 = `floor(买价 / 2)`，最低 1。不在商店目录的物品不能卖（emit `shop-not-for-sale`）。

> **为什么不 emit pickup/drop 事件？** 因为购买需要"扣货币 + 加物品"原子完成。如果用事件（emit drop + emit pickup），两个事件下一 tick 才结算，同 tick 内多次购买会双花（都看到原始货币量）。直接读写 Inventory 是同步的，不会双花。

### 5. 监听模块发出的事件做反馈

```json
{
  "id": "purchase-feedback",
  "on": { "event": "item-bought" },
  "do": [{ "call": "show_purchase_text", "with": { "item": "event.item", "count": "event.count", "price": "event.total_price", "hud": "@hud" } }]
}
```

```js
vitric.fn("show_purchase_text", (args, ctx) => {
  ctx.setField(args.hud, "Text.content", "Bought " + args.count + "x " + args.item + " for " + args.price + " coins");
});
```

### 6. 完整经济闭环

shop 模块和 combat + loot + inventory 组合，构成 RPG 经济循环：

```
kill enemy → died → loot module → pickup(coin) → inventory += coin
                                                          │
                                                    [enough coins?]
                                                          │
press B → shop-buy → shop module → deduct coin + add item → inventory
                                                          │
press H → use item (game logic) → heal / equip / consume
                                                          │
                                                    stronger player
                                                          │
                                              kill tougher enemy → ...
```

这是商业 RPG 的核心循环：**刷怪 → 攒钱 → 买装备 → 变强 → 刷更强的怪**。loot 模块把战斗和经济连起来，shop 模块把经济和成长连起来。

### 7. 与其他模块组合

- **+ combat + loot**：`died` → loot 掉 coin → 玩家攒 coin → `shop-buy` 买 potion/装备。这是最常见的接法，四模块一起 includes。
- **+ inventory**：shop 直接读写 Inventory 组件（原子操作），不 emit pickup/drop。硬依赖 Inventory。
- **+ progression**：`item-bought` 可以触发"获得新装备 → 升级装备槽"；`item-sold` 可以驱动成就系统。
- **+ game-flow**：`item-bought` 可以触发 UI 动画、音效；商店可以解锁新商品（修改 Shop.items 列表）。

### 8. 库存机制

- `stocks = -1`：无限供应（常见于消耗品如 potion）
- `stocks = N`：限量供应，购买后自动扣减，售罄（`stocks = 0`）时 emit `shop-out-of-stock`
- `stocks` 缺省：视为 -1（无限）

限量库存适合关键道具（如"商店只有 1 把圣剑"），无限库存适合消耗品（如"药水无限买"）。

完整可运行示例见 `examples/shop-demo/`，集成测试见 `crates/vitric-cli/tests/shop.rs`。

---

## 配方 12：使用 equipment 模块（装备槽 / 穿脱 / 自动换装 / 属性奖励事件）

**目标**：玩家有装备槽（weapon/armor/accessory），从背包穿戴物品到槽位，脱下时物品回到背包。穿戴时游戏应用属性奖励（+ATK/+HP），脱下时移除奖励。这是 RPG 从"背包里有装备"变成"角色实际穿戴装备变强"的关键系统——和 shop 模块组合后，形成"刷怪 → 攒钱 → 买装备 → 穿戴变强 → 刷更强的怪"的完整商业 RPG 闭环。

### 1. includes

```json
{ "includes": ["../../modules/combat", "../../modules/inventory", "../../modules/equipment"] }
```

equipment 模块依赖 Inventory 组件（直接读写，原子操作——和 shop 模块同一个设计原则）。通常和 combat + inventory 一起 includes，因为装备的属性奖励要应用到 Health/Attack 上。equipment 模块贡献 1 个组件：

- `Equipment`（挂在可穿戴实体上）— 并行列表编码装备槽（和 Dialogue/LootTable/Shop 同模式）：
  - `slots` — 槽位名列表，静态（如 `["weapon", "armor", "accessory"]`）
  - `items` — 每个槽位当前装备的物品 id，`""` = 空（如 `["sword", "armor", ""]`）

模块监听 2 个事件（你的规则负责 emit）：
- `equip { who, item, slot }` — 从 `who` 的背包取出 `item` 穿戴到 `slot`
- `unequip { who, slot }` — 脱下 `slot` 的物品，放回 `who` 的背包

模块 emit 的事件（你的规则监听做属性奖励 / 反馈）：
- `equipped { who, item, slot }` — 物品已穿戴（游戏应用属性奖励）
- `unequipped { who, item, slot }` — 物品已脱下（游戏移除属性奖励）
- `equip-item-not-found { who, item }` — 背包里没有该物品
- `equip-slot-unknown { who, slot }` — 槽位名不在 `Equipment.slots` 里

> **模块不决定属性奖励**——不同游戏的奖励表不同（+ATK？+HP？+暴击？技能解锁？）。模块只管"物品在背包和槽位之间移动 + emit 事件"，你的规则监听 `equipped`/`unequipped` 决定加什么属性。这和 progression 模块不在 `leveled-up` 时加属性、combat 模块不在 `died` 时 despawn 是同一个设计原则：模块管机制，游戏管策略。

### 2. 场景里给实体挂 Equipment + Inventory

```json
{
  "name": "player",
  "components": {
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 10 },
    "Inventory": {
      "items": ["sword", "armor", "ring"],
      "counts": [1, 1, 1],
      "capacity": 8
    },
    "Equipment": {
      "slots": ["weapon", "armor", "accessory"],
      "items": ["", "", ""]
    }
  }
}
```

`slots` 和 `items` 是并行列表——`items[i]` 是 `slots[i]` 当前装备的物品 id，`""` 表示空槽。初始全空（`["", "", ""]`），物品都在背包里。

### 3. 在规则里驱动穿戴 / 脱下

equipment 模块不决定**何时**穿戴——那是游戏逻辑（碰 NPC 打开装备 UI？按快捷键？升级解锁？）。你的规则负责在合适的时机 emit `equip` / `unequip`：

```json
{
  "id": "equip-sword-on-1",
  "comment": "Press 1: equip sword to weapon slot.",
  "on": { "event": "input", "filter": { "action": "1", "phase": "pressed" } },
  "do": [{ "emit": "equip", "data": { "who": "@player", "item": "sword", "slot": "weapon" } }]
},
{
  "id": "unequip-weapon-on-q",
  "comment": "Press Q: unequip weapon slot (returns item to inventory).",
  "on": { "event": "input", "filter": { "action": "q", "phase": "pressed" } },
  "do": [{ "emit": "unequip", "data": { "who": "@player", "slot": "weapon" } }]
}
```

### 4. 穿脱结算是自动的

equipment 模块的 `__equip` / `__unequip` 直接读写 Inventory + Equipment 组件（原子操作，避免状态不一致）：

- **穿戴**：读 Inventory → 检查物品是否在背包 → 从背包移除 → 如果槽位已有物品，**自动脱下旧物品放回背包** → 把新物品写入槽位 → 写回 Inventory + Equipment → emit `equipped`
- **脱下**：读 Equipment → 检查槽位是否有物品 → 把物品放回背包 → 清空槽位 → 写回 Inventory + Equipment → emit `unequipped`

> **自动换装**是 equipment 模块的核心便利：往已占用的槽位穿戴新物品时，旧物品自动回到背包，不需要先手动脱下。模块会依次 emit `unequipped`（旧物品）和 `equipped`（新物品），你的规则监听这两个事件分别移除旧奖励、应用新奖励——属性不会错乱。

### 5. 属性奖励：游戏决定加什么

```json
{
  "id": "apply-equip-bonus",
  "comment": "On equipped: apply stat bonus based on item id.",
  "on": { "event": "equipped" },
  "if": [["event.who", "==", "@player"]],
  "do": [{ "call": "apply_equip_bonus", "with": { "who": "@player", "item": "event.item" } }]
},
{
  "id": "remove-equip-bonus",
  "comment": "On unequipped: remove stat bonus based on item id.",
  "on": { "event": "unequipped" },
  "if": [["event.who", "==", "@player"]],
  "do": [{ "call": "remove_equip_bonus", "with": { "who": "@player", "item": "event.item" } }]
}
```

```js
// Per-item stat bonus table. The game owns this — the equipment module just
// moves items between inventory and slots and emits events.
function bonusFor(item) {
  switch (item) {
    case "sword":       return { atk: 10, maxHp: 0 };
    case "spare_sword": return { atk: 8,  maxHp: 0 };
    case "armor":       return { atk: 0,  maxHp: 20 };
    case "ring":        return { atk: 5,  maxHp: 0 };
    case "gloves":      return { atk: 3,  maxHp: 0 };
    default:            return { atk: 0,  maxHp: 0 };
  }
}

vitric.fn("apply_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const bonus = bonusFor(item);
  if (bonus.atk !== 0) {
    const power = Number(ctx.getField(who, "Attack.power")) || 0;
    ctx.setField(who, "Attack.power", power + bonus.atk);
  }
  if (bonus.maxHp !== 0) {
    const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
    const hp = Number(ctx.getField(who, "Health.hp")) || 0;
    const newMax = maxHp + bonus.maxHp;
    ctx.setField(who, "Health.max", newMax);
    ctx.setField(who, "Health.hp", Math.min(newMax, hp + bonus.maxHp));
  }
});

vitric.fn("remove_equip_bonus", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const bonus = bonusFor(item);
  if (bonus.atk !== 0) {
    const power = Number(ctx.getField(who, "Attack.power")) || 0;
    ctx.setField(who, "Attack.power", Math.max(0, power - bonus.atk));
  }
  if (bonus.maxHp !== 0) {
    const maxHp = Number(ctx.getField(who, "Health.max")) || 100;
    const hp = Number(ctx.getField(who, "Health.hp")) || 0;
    const newMax = Math.max(1, maxHp - bonus.maxHp);
    ctx.setField(who, "Health.max", newMax);
    ctx.setField(who, "Health.hp", Math.min(hp, newMax));
  }
});
```

这个函数**同时读写 combat 模块的组件**（Health/Attack）——它就是 equipment 和 combat 的桥。模块本身不耦合（equipment 不知道 Health/Attack 的存在），桥接发生在**你的游戏脚本**里。这和配方 9（progression）的 `apply_level_up_bonus` 是同一个模式：模块通过事件通信，游戏脚本桥接属性。

### 6. 自动换装时序

往已占用的槽位穿戴新物品时，`__equip` 在同一 tick 内依次：

```
tick N:   input → emit equip (carryover)
tick N+1: equipment-on-equip → __equip:
            1. remove new item from inventory
            2. read old item from slot (if any)
            3. add old item back to inventory (auto-unequip)
            4. emit unequipped { old item } (carryover)
            5. set slot = new item
            6. emit equipped { new item } (carryover)
tick N+2: remove-equip-bonus rule (for old item) → remove_equip_bonus → ATK/HP 写入 (deferred)
          apply-equip-bonus rule (for new item) → apply_equip_bonus → ATK/HP 写入 (deferred)
tick N+3: deferred 写入可见
```

测试驱动时，穿戴后 step 3-4 tick 让 `equipped` → `apply_equip_bonus` → 属性写入全部走完。自动换装时 `unequipped` 和 `equipped` 同一 tick emit，属性先减后加——最终结果正确。

### 7. 与其他模块组合

- **+ combat**：`equipped` → `apply_equip_bonus` 改 Health/Attack。穿戴武器后 `attack` 事件造成的伤害更高（combat 模块读 `Attack.power`）。这是最常见的接法。
- **+ inventory**：equipment 直接读写 Inventory 组件（原子操作）。硬依赖 Inventory。
- **+ shop**：`item-bought` → emit `equip`（买到装备自动穿戴）；或玩家从背包手动穿戴。shop 提供物品来源，equipment 提供穿戴机制。
- **+ progression**：`leveled-up` 可以解锁新装备槽（往 `Equipment.slots` 追加）；`Level.points` 可以驱动"分配点数 → 装备需求"。
- **+ loot**：`loot-dropped` → `pickup` → inventory → 玩家从背包穿戴掉落物。完整的"刷怪 → 掉装备 → 穿戴变强"循环。

### 8. 完整 RPG 闭环中的位置

equipment 模块把 shop 模块的经济循环延伸为**成长循环**：

```
kill enemy → died → loot module → pickup(coin/item) → inventory
                                                          │
                                                    [enough coins?]
                                                          │
press B → shop-buy → shop module → deduct coin + add item → inventory
                                                          │
press 1 → equip → equipment module → remove from inventory + set slot
                                                          │
                                                    equipped event
                                                          │
                                          apply_equip_bonus → +ATK / +HP
                                                          │
                                                    stronger player
                                                          │
                                              kill tougher enemy → ...
```

没有 equipment 的 RPG：背包里有一把剑但角色用拳头打怪（属性不变）。有 equipment 的 RPG：穿戴剑后攻击力 +10，穿装甲后血量 +20——角色**实际变强**，不只是背包变满。这是"收集游戏"和"装备驱动 RPG"的差别。

### 9. 脱下空槽位是 no-op

如果槽位已经是空的（`items[i] == ""`），`__unequip` 直接 return，不 emit `unequipped`，不报错。这意味着你可以让玩家按 Q 脱武器槽而不担心槽位已经空了——模块自动处理。

完整可运行示例见 `examples/equipment-demo/`，集成测试见 `crates/vitric-cli/tests/equipment.rs`（12 例：check 通过 / 初始状态 / 穿剑 / 穿甲 / 穿戒指 / 脱武器 / 脱空槽 no-op / 自动换装 accessory / 自动换装 weapon / 全套叠加 / 穿戴后攻击力提升 / 脱装甲 HP 钳制）。

---

## 配方 13：使用 status-effects 模块（定时效果 / DoT / HoT / 属性修饰 / 清除）

**目标**：给实体挂定时效果——中毒（每 tick 扣血）、回血（每 tick 加血）、急速（持续时间内 +ATK）、护盾（持续时间内 +max HP）等。效果有生命周期：施加 → 每 tick 计时 → 到期自动移除；也可被清除（解毒药）。这是 RPG 战斗系统的"状态机"层——和 combat 模块组合后，能做出"中毒 → 解毒 → 急速 → 暴击"这种有战术深度的战斗，而不是单纯的"你打我一下、我打你一下"。

### 1. includes

```json
{ "includes": ["../../modules/combat", "../../modules/status-effects"] }
```

status-effects 模块**软依赖** combat——模块本身不读写 Health/Attack，但效果的实际作用（中毒扣血、急速加攻）需要 combat 的 `damage`/`heal` 事件和 Health/Attack 组件。所以实战中通常一起 includes。模块贡献 1 个组件：

- `StatusEffects`（挂在可被施加效果的实体上）— 并行列表编码当前活跃效果（和 Dialogue/LootTable/Equipment 同模式）：
  - `effects` — 效果名列表（如 `["poison", "haste"]`）
  - `durations` — 每个效果剩余 tick 数（如 `[5, 3]`）
  - `magnitudes` — 每个效果的强度（如 `[10, 5]`，含义由游戏定义：poison 是每 tick 伤害、haste 是 +ATK 数值）

模块监听 2 个事件（你的规则负责 emit）：
- `apply-status { who, effect, duration, magnitude }` — 施加效果
- `clear-status { who, effect }` — 清除效果（如解毒药）

模块 emit 的事件（你的规则监听定义效果语义）：
- `status-applied { who, effect, duration, magnitude }` — 效果已施加（新增或刷新）
- `status-ticked { who, effect, magnitude, ticks_remaining }` — 效果 tick 了一次（每 tick 都触发，游戏决定做什么）
- `status-expired { who, effect }` — 效果到期自动消失
- `status-cleared { who, effect }` — 效果被 `clear-status` 移除

> **模块不决定效果语义**——"poison"扣多少血？"haste"加多少 ATK？模块不知道，也不关心。模块只管"计时 + tick + 到期"的生命周期，效果的具体作用由**你的规则**监听 `status-ticked`/`status-applied`/`status-expired` 决定。这和 equipment 模块不知道"sword"加多少 ATK、combat 模块不在 `died` 时 despawn 是同一个设计原则：模块管机制，游戏管策略。

### 2. 场景里给实体挂 StatusEffects + Health

```json
{
  "name": "player",
  "components": {
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 20 },
    "StatusEffects": { "effects": [], "durations": [], "magnitudes": [] }
  }
},
{
  "name": "dummy",
  "components": {
    "Health": { "hp": 200, "max": 200 },
    "Attack": { "power": 0 },
    "StatusEffects": { "effects": [], "durations": [], "magnitudes": [] }
  }
}
```

三个列表初始都为空。施加第一个效果时，三个列表同时 push 一项；到期或清除时，三项同时 splice。并行列表的好处是：规则引擎不支持 map 结构，但支持 list —— 用三个 list 平行存储就实现了"效果字典"。

### 3. 在规则里驱动施加 / 清除

status-effects 模块不决定**何时**施加效果——那是游戏逻辑（敌人攻击附带中毒？喝药水？装备诅咒？）。你的规则负责在合适的时机 emit `apply-status` / `clear-status`：

```json
{
  "id": "apply-poison-on-1",
  "comment": "Press 1: apply poison to the dummy (10 ticks, 10 damage/tick).",
  "on": { "event": "input", "filter": { "action": "1", "phase": "pressed" } },
  "do": [{ "emit": "apply-status", "data": { "who": "@dummy", "effect": "poison", "duration": 10, "magnitude": 10 } }]
},
{
  "id": "apply-haste-on-3",
  "comment": "Press 3: apply haste to the player (10 ticks, +10 ATK while active).",
  "on": { "event": "input", "filter": { "action": "3", "phase": "pressed" } },
  "do": [{ "emit": "apply-status", "data": { "who": "@player", "effect": "haste", "duration": 10, "magnitude": 10 } }]
},
{
  "id": "clear-poison-on-4",
  "comment": "Press 4: clear poison from the dummy (antidote).",
  "on": { "event": "input", "filter": { "action": "4", "phase": "pressed" } },
  "do": [{ "emit": "clear-status", "data": { "who": "@dummy", "effect": "poison" } }]
}
```

### 4. 生命周期是自动的

status-effects 模块内置一个 **tick 系统**（`status-tick`），每 tick 自动遍历所有带 `StatusEffects` 的实体：

1. **每个活跃效果**：emit `status-ticked { who, effect, magnitude, ticks_remaining }`（游戏监听这个事件决定效果作用）
2. **duration 减 1**
3. **如果 duration ≤ 0**：emit `status-expired { who, effect }`，从列表移除
4. **如果 duration > 0**：保留效果，写回新的 duration

模块的 `__status_apply` 在施加时：
- **新效果**：三个列表同时 push
- **已存在**：刷新——取 `max(旧 duration, 新 duration)` 和 `max(旧 magnitude, 新 magnitude)`（RPG 标准的"刷新不叠加"规则，避免重复施加无限累加）

模块的 `__status_clear` 在清除时：从三个列表 splice 掉对应项，emit `status-cleared`。**清除不存在的效果是 no-op**——不报错、不 emit 事件，所以你可以放心让玩家喝解毒药而不担心已经没中毒了。

### 5. 效果语义：游戏决定做什么

这是 status-effects 模块的核心。有**两种组合模式**，对应两类效果：

#### 模式 A：tick 驱动（DoT / HoT）

监听 `status-ticked`，每 tick 触发一次效果作用。适合"持续伤害 / 持续治疗"类效果：

```json
{
  "id": "poison-tick-deals-damage",
  "comment": "Poison tick: deal magnitude damage to the afflicted entity. Bridges status-effects → combat.",
  "on": { "event": "status-ticked" },
  "if": [["event.effect", "==", "poison"]],
  "do": [{ "emit": "damage", "data": { "who": "event.who", "amount": "event.magnitude", "killer": "" } }]
},
{
  "id": "regen-tick-heals",
  "comment": "Regen tick: heal magnitude HP to the afflicted entity.",
  "on": { "event": "status-ticked" },
  "if": [["event.effect", "==", "regen"]],
  "do": [{ "emit": "heal", "data": { "who": "event.who", "amount": "event.magnitude" } }]
}
```

每 tick：模块 emit `status-ticked { effect: "poison", magnitude: 10 }` → 规则过滤 effect == "poison" → emit `damage { amount: 10 }` → combat 模块扣血。这就是中毒扣血的完整链路。

#### 模式 B：状态驱动（属性修饰）

监听 `status-applied` / `status-expired`，在效果开始 / 结束时一次性修改属性。适合"急速 +ATK"、"护盾 +max HP"、"虚弱 -ATK"类效果：

```json
{
  "id": "haste-applied-boosts-atk",
  "comment": "Haste applied: +magnitude ATK. Bridges status-effects → combat (stat modifier pattern).",
  "on": { "event": "status-applied" },
  "if": [["event.effect", "==", "haste"]],
  "do": [{ "call": "apply_haste_bonus", "with": { "who": "event.who", "magnitude": "event.magnitude" } }]
},
{
  "id": "haste-expired-removes-atk",
  "comment": "Haste expired: -magnitude ATK (read from the effect's magnitude before it expired).",
  "on": { "event": "status-expired" },
  "if": [["event.effect", "==", "haste"]],
  "do": [{ "call": "remove_haste_bonus", "with": { "who": "event.who" } }]
}
```

```js
// Haste magnitude is fixed in this demo (+10 ATK). In a real game with variable
// haste strength, track the bonus per entity (e.g. a HasteBonus component or a
// script-side map) so remove knows how much to subtract.
const HASTE_BONUS = 10;

vitric.fn("apply_haste_bonus", (args, ctx) => {
  const who = args.who;
  const magnitude = Number(args.magnitude) || HASTE_BONUS;
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", power + magnitude);
});

vitric.fn("remove_haste_bonus", (args, ctx) => {
  const who = args.who;
  const power = Number(ctx.getField(who, "Attack.power")) || 0;
  ctx.setField(who, "Attack.power", Math.max(0, power - HASTE_BONUS));
});
```

施加急速：`apply-status` → `status-applied` → `apply_haste_bonus` → `Attack.power += 10`。到期：`status-expired` → `remove_haste_bonus` → `Attack.power -= 10`。属性变化是**对称的**——加多少减多少，到期后属性回到原值。

> **模式 A vs 模式 B 的区别**：模式 A 每 tick 都触发效果作用（中毒每 tick 扣 10 血，10 tick 扣 100 血）；模式 B 只在开始和结束触发一次（急速开始 +10 ATK，期间 ATK 一直是 30，结束 -10 ATK 回到 20）。选择哪种取决于效果语义：累计型用 A，瞬时型用 B。

### 6. tick 系统时序

施加一个效果到效果开始作用，需要几个 tick 的 cascade：

```
tick N:   input → emit apply-status (carryover)
tick N+1: status-on-apply rule → __status_apply:
            1. read effects/durations/magnitudes
            2. push (or refresh) new effect
            3. write back lists
            4. emit status-applied (carryover)
tick N+2: status-tick system runs (every tick):
            for the new effect: emit status-ticked (carryover)
            (duration: 10 → 9)
          status-applied rule (for haste) → apply_haste_bonus → ATK 写入 (deferred)
tick N+3: status-ticked rule (for poison) → emit damage (carryover)
          ATK 写入可见
tick N+4: combat-on-damage rule → __combat_damage → HP 写入 (deferred)
tick N+5: HP 写入可见
```

测试驱动时，施加后 step 5 tick 让 `apply-status` → `status-applied` → `status-tick` → `status-ticked` → `damage` → `HP 写入` 全链路走完。每 tick 都会触发 `status-ticked`，所以效果越久、cascade 越多。

### 7. 刷新规则：取 max，不叠加

重复施加同名效果时，`__status_apply` 取 `max(旧 duration, 新 duration)` 和 `max(旧 magnitude, 新 magnitude)`——这是 RPG 标准的"刷新不叠加"规则：

- **不叠加**：中毒 10 tick + 再中毒 10 tick ≠ 中毒 20 tick。否则玩家可以无限叠加伤害。
- **刷新**：如果旧效果还剩 3 tick，重新施加 10 tick，duration 变成 `max(3, 10) = 10`——延长了效果，但没有叠加。

如果你的游戏**想要叠加**（如叠加伤害），可以在 `status-applied` 规则里手动累加 magnitude 到一个独立字段，绕过模块的刷新逻辑。模块默认行为是最安全的"不叠加"。

### 8. 与其他模块组合

- **+ combat**：最常见的组合。模式 A 通过 `damage`/`heal` 事件桥接（poison → damage → 扣血）；模式 B 直接读写 Health/Attack（haste → +ATK）。战斗因此有了战术层：单纯攻击变成"先中毒削弱 → 再急速爆发 → 解毒保命"。
- **+ equipment**：装备可以触发被动效果——穿戴"毒剑"时攻击附带中毒（`attack` 事件 → 检查武器 → `apply-status`）。equipment 提供"穿戴什么"，status-effects 提供"穿戴后有什么持续效果"。
- **+ loot**：稀有掉落物使用时施加效果（喝"力量药水"→ `apply-status haste`）。loot 提供"获得什么"，status-effects 提供"使用后做什么"。
- **+ progression**：升级时刷新被动效果（`leveled-up` → `apply-status` 重置 duration）。或技能树解锁"永久急速"——duration 设极大值（如 999999）模拟永久效果。
- **+ quest**：任务触发剧情效果（"被诅咒"任务 → `apply-status curse`）。quest 提供叙事，status-effects 提供机制。

### 9. 完整 RPG 闭环中的位置

status-effects 模块把 combat 模块的"瞬时伤害"延伸为**持续战斗状态**：

```
kill enemy → died → loot module → pickup(poison_potion) → inventory
                                                            │
press 1 → drink poison_potion → apply-status poison(self, 5t, 5)
                                                            │
                                              [every tick: status-ticked]
                                                            │
                                              poison-tick rule → damage
                                                            │
                                                       HP 缓慢下降
                                                            │
                                              press 4 → clear-status poison
                                                            │
                                                       antidote 救命
```

没有 status-effects 的 RPG：战斗是"瞬间结算"——攻击一次扣一次血，没有持续效果。有 status-effects 的 RPG：中毒让敌人慢慢死、急速让自己短期爆发、护盾扛过致命一击——战斗有了**时间维度**和**状态管理**，这是"回合制/即时 RPG"和"简单街机"的差别。

完整可运行示例见 `examples/status-effects-demo/`，集成测试见 `crates/vitric-cli/tests/status_effects.rs`（10 例：check 通过 / 初始无效果 / 中毒持续扣血 / 回血不溢出 / 急速到期恢复 / 解毒提前清除 / 重复施加刷新时长 / 多效果共存 / 急速提升攻击伤害 / 清除不存在 no-op）。

---

## 配方 14：使用 skills 模块（主动技能 / 法力 / 冷却 / 施法验证 / 三种效果桥接）

**目标**：让实体施放主动技能——火球术（50 伤害，20 法力，10 tick 冷却）、治疗术（30 回血，15 法力，15 tick 冷却）、护盾术（施加护盾状态，10 法力，20 tick 冷却）。技能有法力消耗和冷却时间，施法前自动验证（是否已学 / 冷却是否就绪 / 法力是否足够），验证通过才扣法力、进冷却、emit 事件。这是 RPG 战斗系统的"主动操作"层——和 combat + status-effects 组合后，能做出"火球术输出 → 治疗术续航 → 护盾术防御"这种有策略选择的战斗，而不是单纯平 A。

### 1. includes

```json
{ "includes": ["../../modules/combat", "../../modules/status-effects", "../../modules/skills"] }
```

skills 模块**软依赖** combat 和 status-effects——模块本身只管施法验证（法力 / 冷却 / 已学）和 emit `ability-cast` 事件，效果的实际作用（伤害 / 治疗 / 状态）需要 combat 的 `damage`/`heal` 事件和 status-effects 的 `apply-status` 事件。所以实战中三个模块一起 includes。模块贡献 2 个组件：

- `Abilities`（挂在可施法实体上）— 并行列表编码已知技能（和 Dialogue/LootTable/Equipment/StatusEffects 同模式）：
  - `known` — 技能 id 列表（如 `["fireball", "heal", "shield"]`）
  - `cooldowns` — 每个技能剩余冷却 tick 数（0 = 就绪，运行时变化）
  - `costs` — 每个技能的法力消耗（静态，如 `[20, 15, 10]`）
  - `cooldown_maxs` — 每个技能的冷却总时长（静态，如 `[10, 15, 20]`）

- `Mana`（挂在可施法实体上）— 法力池：
  - `current` — 当前法力（如 `80`）
  - `max` — 最大法力（如 `100`）

模块监听 1 个事件（你的规则负责 emit）：
- `cast { who, ability, target }` — 请求施法（who 施放 ability 到 target）

模块 emit 的事件（你的规则监听定义效果）：
- `ability-cast { who, ability, target }` — 施法成功（游戏定义效果：emit damage/heal/apply-status 等）
- `cast-rejected { who, ability, reason }` — 施法失败，reason ∈ `{"unknown", "cooldown", "mana"}`

> **模块不决定技能效果**——"fireball"打多少伤害？"heal"回多少血？模块不知道，也不关心。模块只管"验证 + 扣费 + 进冷却 + emit 事件"，技能的具体效果由**你的规则**监听 `ability-cast` 决定。这和 status-effects 不知道"poison"扣多少血、equipment 不知道"sword"加多少 ATK 是同一个设计原则：模块管机制，游戏管策略。

### 2. 场景里给实体挂 Abilities + Mana + Health

```json
{
  "name": "player",
  "components": {
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 15 },
    "Mana": { "current": 100, "max": 100 },
    "Abilities": {
      "known": ["fireball", "heal", "shield"],
      "cooldowns": [0, 0, 0],
      "costs": [20, 15, 10],
      "cooldown_maxs": [10, 15, 20]
    },
    "StatusEffects": { "effects": [], "durations": [], "magnitudes": [] }
  }
}
```

四个并行列表（`known` / `cooldowns` / `costs` / `cooldown_maxs`）描述同一组技能。`known` + `costs` + `cooldown_maxs` 是静态的（场景设定时确定），`cooldowns` 是动态的（运行时由模块管理）。初始全 0（全部就绪）。

### 3. 在规则里驱动施法

skills 模块不决定**何时**施法——那是游戏逻辑（按键？AI 决策？物品使用？）。你的规则负责在合适的时机 emit `cast`：

```json
{
  "id": "cast-fireball-on-1",
  "comment": "Press 1: cast fireball on the dummy.",
  "on": { "event": "input", "filter": { "action": "1", "phase": "pressed" } },
  "do": [{ "emit": "cast", "data": { "who": "@player", "ability": "fireball", "target": "@dummy" } }]
},
{
  "id": "cast-heal-on-2",
  "comment": "Press 2: cast heal on self.",
  "on": { "event": "input", "filter": { "action": "2", "phase": "pressed" } },
  "do": [{ "emit": "cast", "data": { "who": "@player", "ability": "heal", "target": "@player" } }]
}
```

### 4. 施法验证是自动的

skills 模块的 `__skills_cast` 在收到 `cast` 事件时，按顺序验证：

1. **是否已学**：检查 `ability` 是否在 `Abilities.known` 里。不在 → emit `cast-rejected { reason: "unknown" }`，return
2. **冷却就绪**：检查 `cooldowns[idx]` 是否为 0。> 0 → emit `cast-rejected { reason: "cooldown" }`，return
3. **法力足够**：检查 `Mana.current >= costs[idx]`。不够 → emit `cast-rejected { reason: "mana" }`，return
4. **扣法力**：`Mana.current -= cost`
5. **进冷却**：`cooldowns[idx] = cooldown_maxs[idx]`
6. **emit 成功**：`ability-cast { who, ability, target }`

模块内置一个 **tick 系统**（`skills-cooldown-tick`），每 tick 自动遍历所有带 `Abilities` 的实体，把所有 > 0 的 cooldown 减 1。冷却到期后自动归零，技能重新就绪。

> **验证顺序很重要**：先检查"已学"（最快失败），再检查"冷却"（无副作用），最后检查"法力"（需要读取 Mana 组件）。这样最常见的失败原因最先被拦截，避免不必要的组件读取。

### 5. 技能效果：游戏决定做什么

这是 skills 模块的核心。有**三种效果桥接模式**，对应三类技能：

#### 模式 A：伤害技能（fireball）

监听 `ability-cast`，emit `damage` 事件。桥接 skills → combat：

```json
{
  "id": "fireball-deals-damage",
  "comment": "Fireball cast: deal 50 damage to target. Bridges skills → combat.",
  "on": { "event": "ability-cast" },
  "if": [["event.ability", "==", "fireball"]],
  "do": [{ "emit": "damage", "data": { "who": "event.target", "amount": 50, "killer": "" } }]
}
```

#### 模式 B：治疗技能（heal）

监听 `ability-cast`，emit `heal` 事件。桥接 skills → combat：

```json
{
  "id": "heal-restores-hp",
  "comment": "Heal cast: restore 30 HP to target.",
  "on": { "event": "ability-cast" },
  "if": [["event.ability", "==", "heal"]],
  "do": [{ "emit": "heal", "data": { "who": "event.target", "amount": 30 } }]
}
```

#### 模式 C：状态技能（shield）

监听 `ability-cast`，emit `apply-status` 事件。桥接 skills → status-effects：

```json
{
  "id": "shield-applies-status",
  "comment": "Shield cast: apply shield status (10 ticks). Bridges skills → status-effects.",
  "on": { "event": "ability-cast" },
  "if": [["event.ability", "==", "shield"]],
  "do": [{ "emit": "apply-status", "data": { "who": "event.target", "effect": "shield", "duration": 10, "magnitude": 0 } }]
}
```

三种模式可以组合：一个技能可以同时造成伤害 AND 施加状态（如"毒刃术"：emit `damage` + emit `apply-status` poison）。只需在 `ability-cast` 规则里 emit 多个事件。

> **技能效果在规则里定义，不在脚本里**——这是和 equipment 的 `apply_equip_bonus` 脚本不同的地方。技能效果通常是"emit 一个事件"（damage/heal/apply-status），规则引擎能直接处理，不需要脚本。只有当效果需要复杂计算（如"根据目标已损失血量增加伤害"）时才走脚本。这和 combat 模块的规则直接 emit `damage` 是同一个模式：简单效果走规则，复杂逻辑走脚本。

### 6. 施法时序

从按键到效果生效，需要几个 tick 的 cascade：

```
tick N:   input → emit cast (carryover)
tick N+1: skills-on-cast rule → __skills_cast:
            1. validate (known / cooldown / mana)
            2. deduct mana, set cooldown
            3. emit ability-cast (carryover)
tick N+2: ability-cast rule (for fireball) → emit damage (carryover)
          skills-cooldown-tick system: decrement all cooldowns by 1
tick N+3: combat-on-damage rule → __combat_damage → HP 写入 (deferred)
tick N+4: HP 写入可见
```

测试驱动时，施法后 step 5 tick 让 `cast` → `ability-cast` → `damage` → `HP 写入` 全链路走完。冷却每 tick 减 1，所以 `cooldown_maxs[i]` 决定了多久能再次施法。

### 7. 法力管理

法力是 skills 模块的资源经济。设计要点：

- **初始满法力**：场景设定 `Mana.current = Mana.max`，玩家开局可以施法
- **法力不自动回复**：模块不内置法力回复（不同游戏的回复机制不同——升级回复？药水回复？每 tick 回复？）。游戏自己实现：可以写一个 tick 规则 `on tick → Mana.current = min(max, current + 1)`
- **法力消耗是静态的**：`costs` 列表在场景设定时确定，运行时不变。如果想要"升级降低消耗"，可以写脚本动态修改 `Abilities.costs`
- **法力为 0 时**：所有有消耗的技能都无法施放（emit `cast-rejected { reason: "mana" }`），但 0 消耗的技能仍可施放

### 8. 与其他模块组合

- **+ combat**：最常见的组合。模式 A 通过 `damage` 事件造成伤害，模式 B 通过 `heal` 事件恢复血量。技能因此有了"输出"和"续航"两个维度。
- **+ status-effects**：模式 C 通过 `apply-status` 施加状态。技能可以施加中毒 / 急速 / 护盾等效果，和 status-effects 模块的状态机无缝衔接。这是三个模块组合的核心：skills 决定"何时施加"，status-effects 决定"如何持续"，combat 决定"扣多少血"。
- **+ equipment**：装备可以修改技能——"法杖"降低 fireball 的法力消耗（修改 `Abilities.costs`），"加速护手"缩短 cooldown（修改 `Abilities.cooldown_maxs`）。equipment 提供"穿戴什么"，skills 提供"能施放什么"。
- **+ progression**：升级时学习新技能（往 `Abilities.known` 追加），或降低现有技能的法力消耗。progression 提供"成长"，skills 提供"成长后能做什么"。
- **+ loot**：稀有掉落物"技能书"使用后永久学习技能（emit `cast` 一个特殊"learn"技能？或直接修改 `Abilities.known`）。

### 9. 完整 RPG 闭环中的位置

skills 模块把 combat 模块的"平 A 互砍"延伸为**有策略选择的战斗**：

```
kill enemy → died → loot module → pickup(mana_potion) → inventory
                                                            │
press 1 → cast fireball (20 mana, 50 damage) → dummy HP -50
                                                            │
                                              [cooldown 10t, mana 80/100]
                                                            │
press 3 → cast shield (10 mana) → apply-status shield(self, 10t)
                                                            │
                                              [shield active, mana 70/100]
                                                            │
press 2 → cast heal (15 mana, 30 HP) → player HP +30
                                                            │
                                              [mana 55/100, all abilities on cooldown]
                                                            │
press 9 → drink mana_potion → Mana.current += 50 → 100/100
                                                            │
                                              [ready to cast again]
```

没有 skills 的 RPG：战斗是"你打我一下、我打你一下"的平 A 互砍——攻击力 15，砍 14 次才能杀死 200 HP 的敌人。有 skills 的 RPG：火球术一次 50 伤害（4 次杀死），治疗术保命，护盾术扛致命一击——战斗有了**资源管理**（法力）和**策略选择**（什么时候输出、什么时候保命），这是"动作 RPG"和"点击游戏"的差别。

完整可运行示例见 `examples/skills-demo/`，集成测试见 `crates/vitric-cli/tests/skills.rs`（11 例：check 通过 / 初始就绪 / 火球伤害+法力消耗 / 治疗回血+法力消耗 / 护盾施加状态 / 冷却阻止重施 / 冷却到期恢复 / 法力不足阻止 / 未知技能拒绝 / 平 A 不耗法力 / 多技能共存）。

---

## 配方 15：使用 crafting 模块（配方 / 材料消耗 / 产出 / 数据驱动配方实体）

**目标**：让玩家用材料合成物品——3 铁矿 + 1 木头 = 1 把剑，2 铁矿 + 2 木头 = 1 面盾。配方是**数据驱动的实体**（不是硬编码在脚本里），玩家知道哪些配方就能合成哪些。合成时自动验证材料是否足够，足够则消耗材料并产出物品，不够则拒绝。这是 RPG 经济系统的"生产"层——和 inventory + equipment + combat 组合后，形成"刷怪 → 收集材料 → 合成装备 → 穿戴变强 → 刷更强的怪"的完整生产消费循环，而不是只能从商店买东西。

### 1. includes

```json
{ "includes": ["../../modules/inventory", "../../modules/combat", "../../modules/equipment", "../../modules/crafting"] }
```

crafting 模块**硬依赖** inventory——合成时直接读写 Inventory 组件（原子操作，和 equipment 模块同一个设计原则）。通常和 inventory + equipment + combat 一起 includes，因为合成的装备要穿戴后通过 equipment → combat 桥接才能实际变强。模块贡献 3 个组件：

- `Crafting`（挂在合成者实体上）— 已知配方列表：
  - `known` — 配方实体名列表（如 `["sword_recipe", "shield_recipe"]`）

- `RecipeDef`（挂在配方实体上）— 静态产出定义：
  - `output` — 产出的物品 id（如 `"sword"`）
  - `output_count` — 产出数量（如 `1`）

- `RecipeInputs`（挂在配方实体上）— 并行列表编码所需材料（和 Dialogue/LootTable 同模式）：
  - `items` — 材料物品 id 列表（如 `["iron", "wood"]`）
  - `counts` — 每种材料需要多少（如 `[3, 1]`）

模块监听 1 个事件（你的规则负责 emit）：
- `craft { who, recipe }` — 请求合成（who 用 recipe 配方合成）

模块 emit 的事件（你的规则监听做反馈）：
- `crafted { who, recipe, output, output_count }` — 合成成功
- `craft-rejected { who, recipe, reason }` — 合成失败，reason ∈ `{"unknown", "missing_materials"}`

> **配方是实体，不是硬编码**——每个配方是一个带 `RecipeDef` + `RecipeInputs` 组件的实体。这意味着配方是**数据**，可以在场景文件里定义、可以动态添加（NPC 教你新配方 → 往 `Crafting.known` 追加）、可以序列化进存档。这和 quest 模块的 quest 实体（带 `QuestDef` + `QuestObjective` + `QuestState`）是同一个数据驱动模式。

### 2. 场景里定义配方实体 + 给玩家挂 Crafting + Inventory

```json
{
  "name": "player",
  "components": {
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 10 },
    "Inventory": {
      "items": ["iron", "wood", "herb"],
      "counts": [5, 3, 2],
      "capacity": 16
    },
    "Equipment": { "slots": ["weapon"], "items": [""] },
    "Crafting": { "known": ["sword_recipe", "shield_recipe"] }
  }
},
{
  "name": "sword_recipe",
  "components": {
    "RecipeDef": { "output": "sword", "output_count": 1 },
    "RecipeInputs": { "items": ["iron", "wood"], "counts": [3, 1] }
  }
},
{
  "name": "shield_recipe",
  "components": {
    "RecipeDef": { "output": "shield", "output_count": 1 },
    "RecipeInputs": { "items": ["iron", "wood"], "counts": [2, 2] }
  }
}
```

配方实体没有 Position/Sprite——它们不在游戏世界里"存在"，只是数据容器。玩家通过 `Crafting.known` 引用配方实体名。初始材料在 Inventory 里（5 iron + 3 wood + 2 herb）。

### 3. 在规则里驱动合成

crafting 模块不决定**何时**合成——那是游戏逻辑（在工坊 UI 里按合成？快捷键？NPC 对话后触发？）。你的规则负责在合适的时机 emit `craft`：

```json
{
  "id": "craft-sword-on-1",
  "comment": "Press 1: craft a sword (requires 3 iron + 1 wood).",
  "on": { "event": "input", "filter": { "action": "1", "phase": "pressed" } },
  "do": [{ "emit": "craft", "data": { "who": "@player", "recipe": "sword_recipe" } }]
}
```

### 4. 合成验证 + 消耗 + 产出是自动的

crafting 模块的 `__crafting_craft` 在收到 `craft` 事件时：

1. **检查已学**：`recipe` 是否在 `Crafting.known` 里。不在 → emit `craft-rejected { reason: "unknown" }`，return
2. **读取配方定义**：从配方实体读 `RecipeDef.output`/`output_count` 和 `RecipeInputs.items`/`counts`
3. **验证材料**：读玩家的 Inventory，检查每种材料的持有量 ≥ 需求量。不够 → emit `craft-rejected { reason: "missing_materials" }`，return
4. **消耗材料**：从 Inventory 扣除每种材料的数量
5. **产出物品**：把 `output` 加到 Inventory（已有则堆叠，没有则追加）
6. **写回 Inventory**：原子写回 items + counts
7. **emit 成功**：`crafted { who, recipe, output, output_count }`

> **原子操作**：步骤 4-6 在同一个函数调用内完成，中间不会被其他系统打断。这避免了"扣了材料但没给产出"的不一致状态。和 equipment 模块的 `__equip`（检查 + 移除 + 添加 + 写回在同一函数内）是同一个设计原则。

### 5. 与其他模块组合

- **+ inventory**：硬依赖。crafting 直接读写 Inventory 组件。材料从 Inventory 扣除，产出加到 Inventory。
- **+ equipment**：合成的装备可以穿戴。`crafted` → 游戏规则 emit `equip` → equipment 模块处理。crafting 提供"获得装备的途径"，equipment 提供"穿戴装备的机制"。
- **+ combat**：合成的武器穿戴后提升攻击力。`crafted` → `equip` → `equipped` → `apply_equip_bonus` → Attack.power += N。crafting 是"变强"的来源之一。
- **+ loot**：杀怪掉材料（`loot-dropped` → `pickup` → Inventory 有了 iron/wood）。crafting 用这些材料合成装备。完整的"刷怪 → 收集 → 合成"循环。
- **+ shop**：商店卖材料（玩家可以买 iron 而不是刷怪掉）。shop 提供"快速获取材料的途径"，crafting 提供"材料变装备的途径"。
- **+ progression**：升级解锁新配方（`leveled-up` → 往 `Crafting.known` 追加新配方实体名）。progression 提供"成长"，crafting 提供"成长后能做什么"。
- **+ quest**：任务奖励新配方（`quest-turned-in` → 往 `Crafting.known` 追加）。quest 提供叙事动力，crafting 提供实际收益。

### 6. 数据驱动配方的好处

配方是实体意味着：

- **动态添加**：NPC 教你新配方 → `ctx.setField(who, "Crafting.known", [...known, "legendary_sword_recipe"])`。不需要改脚本逻辑，只需追加配方实体名。
- **序列化进存档**：配方实体是场景的一部分，`Crafting.known` 是组件数据——存档系统自动处理。
- **运行时检查**：游戏可以读 `RecipeInputs.items`/`counts` 在 UI 里显示"合成这把剑需要 3 铁矿 + 1 木头"，不需要硬编码 UI。
- **配方可以是任务奖励**：quest 模块的 `quest-turned-in` 事件可以触发"学习新配方"，和 crafting 模块无缝衔接。

### 7. 完整 RPG 闭环中的位置

crafting 模块把 shop 模块的"只能买"经济延伸为**生产消费循环**：

```
kill enemy → died → loot module → pickup(iron/wood) → inventory
                                                          │
press 1 → craft → crafting module → consume 3 iron + 1 wood → add sword
                                                          │
                                              [sword in inventory]
                                                          │
press 3 → equip → equipment module → sword to weapon slot
                                                          │
                                              equipped event
                                                          │
                                          apply_equip_bonus → +15 ATK
                                                          │
                                              attack damage 10 → 25
                                                          │
                                              kill tougher enemy → ...
```

没有 crafting 的 RPG：玩家只能从商店买装备（钱从刷怪来），经济是单线的"刷怪 → 攒钱 → 买装备"。有 crafting 的 RPG：玩家可以收集材料自己造装备——这多了"去哪里刷材料"的策略选择（铁矿去矿山？木头去森林？），也多了"先造什么"的优先级决策（先造武器提升输出？先造盾提升生存？）。这是"有深度的 RPG 经济"和"简单商店游戏"的差别。

完整可运行示例见 `examples/crafting-demo/`，集成测试见 `crates/vitric-cli/tests/crafting.rs`（10 例：check 通过 / 初始材料 / 合成剑消耗材料 / 合成盾消耗不同量 / 材料不足阻止 / 未知配方拒绝 / 完整合成-穿戴-攻击循环 / 连续合成验证 / 合成盾验证 / 无关材料不被消耗）。

---

## 配方 16：十二模块旗舰组合（完整商业 RPG 闭环）

**目标**：把全部十二个玩法模块拼成一个完整的 RPG——标题→对话接任务→合成剑→穿戴剑→火球术攻击狼→狼中毒攻击玩家→玩家买药水回血→击杀狼→掉落狼皮→任务自动完成→交付任务→胜利→重开。这是引擎"能产出完整商业游戏"的结构性证明：十二个模块零胶水代码组合，纯靠规则 + 模块事件驱动。

配方 8 证明了七模块闭环；本配方在此基础上加入 shop（经济）、equipment（装备槽）、status-effects（状态效果）、skills（主动技能）、crafting（合成）五个模块，把"收集→交付"的单线闭环扩展为"收集→合成→穿戴→施法→战斗→掉落→升级→商店"的**网状闭环**。

### 1. includes 十二个模块

```json
{
  "includes": [
    "../../modules/inventory",
    "../../modules/quest",
    "../../modules/dialogue",
    "../../modules/game-flow",
    "../../modules/combat",
    "../../modules/progression",
    "../../modules/loot",
    "../../modules/shop",
    "../../modules/equipment",
    "../../modules/status-effects",
    "../../modules/skills",
    "../../modules/crafting"
  ]
}
```

### 2. 玩家实体挂全部十二个模块的组件

```json
{
  "name": "player",
  "components": {
    "Player": {},
    "Position": { "x": 0, "y": 0 },
    "Velocity": { "x": 0, "y": 0 },
    "Collider": { "w": 1, "h": 1 },
    "Speed": { "value": 60 },
    "Sprite": { "w": 1, "h": 1, "color": "#4da3ff" },
    "Health": { "hp": 100, "max": 100 },
    "Attack": { "power": 10 },
    "Mana": { "current": 100, "max": 100 },
    "Inventory": {
      "items": ["iron", "wood", "coin"],
      "counts": [3, 1, 5],
      "capacity": 16
    },
    "Equipment": { "slots": ["weapon"], "items": [""] },
    "Abilities": {
      "known": ["fireball", "heal"],
      "cooldowns": [0, 0],
      "costs": [20, 15],
      "cooldown_maxs": [10, 15]
    },
    "StatusEffects": { "effects": [], "durations": [], "magnitudes": [] },
    "Crafting": { "known": ["sword_recipe"] },
    "XP": { "current": 0, "threshold": 100 },
    "Level": { "value": 1, "points": 0 },
    "QuestLog": { "active": [], "completed": [] },
    "DialogueRunner": { "active_npc": "", "current": -1 }
  }
}
```

### 3. 组合接缝总览

十二个模块之间的接缝全是**事件桥接**——一个模块发事件，另一个模块的规则监听后发新事件，游戏规则在中间定义"语义"：

```
                    ┌─────────────────────────────────────────────┐
                    │                                             │
  press C ──→ craft event ──→ crafting module ──→ item-crafted event
                                  consumes iron+wood      │
                                                          ↓
  press E ──→ equip event ──→ equipment module ──→ equipped event
                                  moves sword to slot     │
                                                          ↓
  game rule: apply_equip_bonus → +15 Attack.power ◄───────┘

  press F ──→ cast event ──→ skills module ──→ ability-cast event
                                validates mana/cd   │
                                                    ↓
  game rule: fireball-deals-damage → damage event ──→ combat module ──→ HP -= 50

  collision ──→ attack + apply-status events ──→ combat module (damage)
                                                   status-effects module (poison)
                                                         │
                                                         ↓
  status-ticked event ──→ game rule: poison-tick-damages-player ──→ damage event

  HP=0 ──→ died event ──→ game rule: stash_wolf + gain-xp
                             │                    │
                             ↓                    ↓
  loot module ──→ pickup event ──→ inventory  progression module ──→ leveled-up event
                                   (+wolf_pelt)        │
                                                       ↓
  quest module ──→ quest auto-completes     game rule: apply_level_up_bonus
  (collect wolf_pelt)                                  → +20 max HP, +10 ATK

  collision elder ──→ quest-turn-in event ──→ quest module ──→ quest-turned-in
                                                                   │
                                                                   ↓
                                                   game rule: win-on-quest-turned-in
                                                           → game-win → phase=won
```

### 4. 关键规则模式

**施法 → 伤害桥接**（skills → combat）：
```json
{
  "id": "fireball-deals-damage",
  "on": { "event": "ability-cast" },
  "if": [["event.ability", "==", "fireball"]],
  "do": [{ "emit": "damage", "data": { "who": "event.target", "amount": 50, "killer": "event.who" } }]
}
```
skills 模块只管"验证法力/冷却/设置冷却→发 ability-cast"，火球术的 50 点伤害由游戏规则定义。这让同一个 skills 模块可以服务于完全不同的技能集。

**状态效果 → 伤害桥接**（status-effects → combat）：
```json
{
  "id": "poison-tick-damages-player",
  "on": { "event": "status-ticked" },
  "if": [["event.effect", "==", "poison"], ["@game.GameState.phase", "==", "playing"]],
  "do": [{ "emit": "damage", "data": { "who": "event.who", "amount": "event.magnitude" } }]
}
```
`phase == "playing"` 守卫防止队列中残留的 poison 事件在游戏结束后继续伤害玩家——这是确定性引擎里"事件已入队但状态已变"的经典时序问题。

**装备奖励桥接**（equipment → combat）：
```json
{
  "id": "apply-equip-bonus",
  "on": { "event": "equipped" },
  "do": [{ "call": "apply_equip_bonus", "with": { "who": "@player", "item": "event.item" } }]
}
```
equipment 模块只管"把物品移到槽位→发 equipped 事件"，+15 ATK 的奖励由游戏脚本定义（`bonusFor("sword") → 15`）。

**对话防重启守卫**：
```json
{
  "id": "elder-start-dialogue",
  "on": { "event": "collision", "between": ["Player", "Npc"] },
  "if": [["self.DialogueRunner.current", "<", 0], ["@wolf-quest.QuestState.state", "==", "inactive"]],
  "do": [{ "emit": "talk", "data": { "npc": "other", "who": "self" } }]
}
```
碰撞事件每 tick 触发，如果只检查 `current < 0`，对话结束的下一 tick 会立刻重启。加 `quest == inactive` 守卫确保对话只在首次接触时启动——这是确定性引擎里"持续碰撞"的经典陷阱。

### 5. 完整游戏循环

```
title ──SPACE──→ playing
  │
  ├── right → collision elder → quest-offer + quest-accept + talk
  │     ├── 1 → dialogue advance
  │     └── 1 → dialogue end
  │
  ├── left → walk away from elder
  │
  ├── C → craft sword (consume 3 iron + 1 wood → add sword)
  ├── E → equip sword (sword → weapon slot, +15 ATK → 25 ATK)
  │
  ├── F → cast fireball (20 mana, 50 damage → wolf 80→30)
  ├── wait 10 ticks (cooldown)
  ├── F → cast fireball (20 mana, 50 damage → wolf 30→0)
  │     ├── wolf died → stash_wolf + gain-xp(100)
  │     ├── loot module → pickup(coin 3-5, wolf_pelt 1)
  │     ├── quest module → auto-complete (have wolf_pelt)
  │     └── progression module → level-up (100 XP ≥ 100 threshold)
  │           └── apply_level_up_bonus → +20 max HP, +10 ATK
  │
  ├── right → collision elder → quest-turn-in
  │     └── quest-turned-in → game-win → phase=won
  │
  └── R → reset_game → phase=title (全状态重置)
```

### 6. 重置全覆盖

`reset_game` 脚本必须重置**所有十二个模块**的状态——漏掉任何一个都会导致重开后状态泄漏：

```javascript
vitric.fn("reset_game", (_args, ctx) => {
  // combat: Health, Attack
  ctx.setField("@player", "Health.hp", 100);
  ctx.setField("@player", "Health.max", 100);
  ctx.setField("@player", "Attack.power", 10);
  // skills: Mana, Abilities cooldowns
  ctx.setField("@player", "Mana.current", 100);
  ctx.setField("@player", "Abilities.cooldowns", [0, 0]);
  // status-effects: clear all effects
  ctx.setField("@player", "StatusEffects.effects", []);
  ctx.setField("@player", "StatusEffects.durations", []);
  ctx.setField("@player", "StatusEffects.magnitudes", []);
  // equipment: clear slots
  ctx.setField("@player", "Equipment.slots", ["weapon"]);
  ctx.setField("@player", "Equipment.items", [""]);
  // inventory: restore starting items
  ctx.setField("@player", "Inventory.items", ["iron", "wood", "coin"]);
  ctx.setField("@player", "Inventory.counts", [3, 1, 5]);
  // progression: XP, Level
  ctx.setField("@player", "XP.current", 0);
  ctx.setField("@player", "Level.value", 1);
  // quest: clear log + reset quest state
  ctx.setField("@player", "QuestLog.active", []);
  ctx.setField("@wolf-quest", "QuestState.state", "inactive");
  // dialogue: clear runner
  ctx.setField("@player", "DialogueRunner.current", -1);
  // wolf: revive + clear status
  ctx.setField("@wolf", "Health.hp", 80);
  ctx.setField("@wolf", "Position.x", 1);
  ctx.setField("@wolf", "Position.y", 2);
  ctx.setField("@wolf", "StatusEffects.effects", []);
  // emit game-restart → game-flow module resets phase/time/score
  ctx.emit("game-restart", {});
});
```

### 7. 为什么这是"完整游戏"而非 demo

| 维度 | demo | rpg-full |
|------|------|----------|
| 模块数 | 1-3 | 12 |
| 系统互联 | 线性 | 网状（每个模块至少和 2 个其他模块有事件接缝） |
| 经济循环 | 无 | kill→loot→coin→shop→potion→heal→survive |
| 生产循环 | 无 | gather iron/wood→craft sword→equip→stronger |
| 成长循环 | 无 | kill→XP→level-up→+HP+ATK→kill tougher |
| 状态效果 | 无 | wolf poisons player → DoT → must heal or die |
| 主动技能 | 无 | fireball (damage) + heal (restore) with mana+cooldown |
| 重置完整性 | 部分 | 全 12 模块状态重置 |
| 测试覆盖 | smoke | 9 integration tests (initial state / craft+equip / fireball+heal / shop+ potion / poison / kill+loot+quest / full win loop / death+restart / check) |

### 8. 与配方 8（七模块）的对比

配方 8 的 rpg-mini 是"最小闭环"：接任务→收集草药→交付→赢。战斗是可选支线（可以绕开狼）。

配方 16 的 rpg-full 是"完整闭环"：必须合成武器、用技能击杀狼、收集战利品才能完成任务。五个额外模块把"可选的战斗"变成了"必须经历的生产-战斗-成长循环"——这正是商业 RPG 和 demo 的本质区别。

完整可运行示例见 `examples/rpg-full/`，集成测试见 `crates/vitric-cli/tests/rpg_full.rs`（11 例：check 通过 / 初始状态 / 合成+穿戴剑 / 火球+治疗术 / 商店买药水 / 狼中毒玩家 / 火球杀狼+掉落+任务完成+升级 / 完整胜利循环 / 玩家死亡+重启 / 存档-读档往返 / 读档缺失槽位优雅报错）。

---

## 配方 17：存档系统（save-game / load-game 约定事件 / 确定性快照 / 原子写入）

**目标**：让玩家随时存档、随时读档，状态精确恢复到存档时刻。存档系统是"完整游戏而非 demo"的必要条件——没有持久化的游戏只是 demo。

Vitric 的存档系统建立在确定性快照（`Sim::snapshot` / `Sim::restore`）之上：存档 = 把完整世界状态（ECS / tick / RNG / 事件队列）序列化为 JSON；读档 = 从 JSON 恢复到那一刻。因为引擎是确定性的，存档不需要记录"操作历史"，只需要一个时刻的"状态切片"。

### 1. 约定事件

存档系统使用两个约定事件（convention events），由引擎的 `Dispatcher` 自动处理，不需要写模块规则：

- `save-game { slot }` — 存档到指定槽位，写出 `<project>/saves/<slot>.json`
- `load-game { slot }` — 从指定槽位读档，`Sim::restore` 恢复世界状态

在游戏规则里 emit 这两个事件即可：

```json
{
  "id": "save-on-s",
  "comment": "Press S to save game to slot1.",
  "on": { "event": "input", "filter": { "action": "s", "phase": "pressed" } },
  "if": [["@game.GameState.phase", "==", "playing"]],
  "do": [{ "emit": "save-game", "data": { "slot": "slot1" } }]
},
{
  "id": "load-on-l",
  "comment": "Press L to load game from slot1.",
  "on": { "event": "input", "filter": { "action": "l", "phase": "pressed" } },
  "if": [["@game.GameState.phase", "==", "playing"]],
  "do": [{ "emit": "load-game", "data": { "slot": "slot1" } }]
}
```

不需要 `includes` 任何模块——存档是引擎内置能力，不是模块。

### 2. 确定性边界

存档和读档跨越了确定性边界，理解这一点很重要：

- **存档（save-game）是纯输出副作用**——和 play-sound 一样，在模拟之外执行。文件是否写入成功不影响世界状态，所以确定性回放不受影响。
- **读档（load-game）重写模拟**——等价于 `Sim::restore`，时间线断裂。因此：
  - 录制中（recording）的读档会被拒绝——录制要求时间线连续，读档会让录制不可回放。
  - 存档在录制中是允许的——只是写文件，不影响时间线。

### 3. 槽名验证

槽名直接成为文件名，所以有严格验证：`[a-z0-9-]{1,32}`。这条规则同时堵死了路径穿越（`../evil` 不合法）：

```
slot1          ✓
auto-save-3    ✓
Slot1          ✗ (大写不合法)
../evil        ✗ (路径穿越)
a/b            ✗ (斜杠不合法)
```

### 4. 存档文件格式

```json
{
  "engine_version": "0.2.0",
  "project": "rpg-full",
  "slot": "slot1",
  "snapshot": {
    "world": { ... },
    "tick": 42,
    "rng": { "state": [12345, 67890] },
    "input_buffer": [],
    "reply_buffer": [],
    "event_queue": []
  }
}
```

- `engine_version` — 引擎版本，读档时如果不匹配则报错（不静默兼容）。
- `project` — 项目名，人类可读。
- `snapshot` — `Sim::snapshot` 的原始输出，包含完整世界状态。

### 5. 原子写入

存档使用"写临时文件 + 原子重命名"策略：先写到 `saves/.slot1.json.tmp`，再 `rename` 为 `saves/slot1.json`。崩溃或断电不会留下半个 JSON 覆盖旧存档。

### 6. 在 rpg-full 中的使用

rpg-full 示例已集成存档系统（按 S 存档、按 L 读档），是"完整游戏"的最后一块拼图：

```
play → craft sword → press S (save) → equip sword → press L (load)
                                                          │
                                                          ↓
                                    state restored to "sword crafted, NOT equipped"
```

集成测试 `rpg_full_save_load_roundtrip` 验证：
1. 存档时记录状态哈希、ATK、装备
2. 继续游戏（穿戴剑，ATK 10→25）
3. 读档后状态哈希必须等于存档时刻
4. ATK 恢复到存档时的值（10，未穿戴）
5. 读档后游戏可继续正常游玩

### 7. 自动存档模式

除了手动按 S 存档，可以在游戏规则里设自动存档触发点：

```json
{
  "id": "auto-save-on-checkpoint",
  "comment": "Auto-save when entering a new area.",
  "on": { "event": "area-entered" },
  "do": [{ "emit": "save-game", "data": { "slot": "auto-save" } }]
},
{
  "id": "auto-save-on-quest-complete",
  "comment": "Auto-save when a quest is turned in.",
  "on": { "event": "quest-turned-in" },
  "do": [{ "emit": "save-game", "data": { "slot": "auto-save" } }]
}
```

### 8. 多槽位管理

游戏可以提供多个存档槽（quick-save / auto-save / manual-1 / manual-2），让玩家自己管理：

```json
{ "emit": "save-game", "data": { "slot": "quick-save" } }
{ "emit": "save-game", "data": { "slot": "manual-1" } }
{ "emit": "save-game", "data": { "slot": "manual-2" } }
```

CLI 命令 `vitric saves` 列出所有存档槽，`vitric run --load slot1` 从指定槽位启动游戏。

### 9. 与其他系统的关系

- **与 game-flow 模块**：存档保存 `GameState.phase`，读档后游戏阶段恢复（playing → 存档 → 读档 → 仍在 playing）。
- **与 combat 模块**：存档保存 `Health.hp` / `Attack.power`，读档后战斗状态恢复。
- **与 inventory 模块**：存档保存 `Inventory.items` / `counts`，读档后背包恢复。
- **与 progression 模块**：存档保存 `XP.current` / `Level.value`，读档后等级恢复。
- **与所有 12 个模块**：存档保存全部组件状态，读档后所有模块状态精确恢复——这是确定性引擎的天然优势。

完整可运行示例见 `examples/rpg-full/`（按 S/L 存读档），存档系统实现见 `crates/vitric-control/src/saves.rs`，集成测试见 `crates/vitric-cli/tests/saves.rs` 和 `crates/vitric-cli/tests/rpg_full.rs`。

---

## 配方 18：程序化生成（ctx.random / ctx.spawn / 确定性种子 / Recipe 组件）

**目标**：用代码生成关卡、敌人、物品——不是手摆每一个实体，而是用参数化的生成器批量产出。这是"内容兼备的"关键：一个生成器可以产出无限种关卡，而手摆只能产出一种。

Vitric 的程序化生成建立在确定性 RNG 之上：`ctx.random()` 从引擎的种子化随机流取值，同一粒种子永远生成同一张地图。这意味着生成结果可回放、可测试、可分享（只分享一个种子号即可）。

### 1. 核心三件套

| API | 作用 |
|-----|------|
| `ctx.random()` | 返回 `[0, 1)` 的随机浮点数，从引擎种子化 PCG32 流取值。同一种子 → 同一序列。 |
| `ctx.spawn({ Component: {...}, ... })` | 运行时创建实体，参数是组件字典。和场景文件里的实体格式完全一致。 |
| `ctx.emit("event", data)` | 生成完成后发事件通知游戏规则。 |

### 2. Recipe 组件：参数化生成

把生成参数放在一个 `Recipe` 组件里，让场景文件控制生成器行为：

```json
{
  "name": "generator",
  "components": {
    "Recipe": { "gems": 10, "hazards": 14, "width": 44, "height": 26 }
  }
}
```

```javascript
vitric.fn("generate", (args, ctx) => {
  const halfW = args.width / 2;
  const halfH = args.height / 2;

  // 安全区：不在出生点附近放危险物
  function place() {
    let x = (ctx.random() * 2 - 1) * (halfW - 1);
    let y = (ctx.random() * 2 - 1) * (halfH - 1);
    if (Math.abs(x) < 4 && Math.abs(y) < 4) {
      x += x >= 0 ? 5 : -5;
      y += y >= 0 ? 5 : -5;
    }
    return { x, y };
  }

  for (let i = 0; i < args.gems; i++) {
    const p = place();
    ctx.spawn({
      Gem: {},
      Position: { x: p.x, y: p.y },
      Collider: { w: 1, h: 1 },
      Sprite: { w: 0.8, h: 0.8, color: "#39e6c3" },
    });
  }
  for (let i = 0; i < args.hazards; i++) {
    const p = place();
    ctx.spawn({
      Hazard: {},
      Position: { x: p.x, y: p.y },
      Collider: { w: 1.2, h: 1.2 },
      Sprite: { w: 1.2, h: 1.2, color: "#ff5470" },
    });
  }
  ctx.emit("level-generated", { gems: args.gems, hazards: args.hazards });
});
```

### 3. 触发生成

规则在游戏开始时调用生成器：

```json
{
  "id": "generate-level",
  "on": { "event": "game-start" },
  "do": [{ "call": "generate", "with": { "gems": "@generator.Recipe.gems", "hazards": "@generator.Recipe.hazards", "width": "@generator.Recipe.width", "height": "@generator.Recipe.height" } }]
}
```

### 4. 确定性保证

- `ctx.random()` 和规则里的 `rng` 使用同一个 PCG32 流。
- 改 `vitric.json` 的 `seed` 值 → 完全不同的地图。
- 不改 seed → 每次运行生成完全相同的地图。
- 生成结果可以通过 `Sim::state_hash()` 验证——同一 seed 的哈希值必定相同。

### 5. 应用场景

| 场景 | Recipe 参数 | 生成内容 |
|------|------------|----------|
| 地牢生成 | rooms, corridors, traps | 房间、走廊、陷阱 |
| 敌人波次 | count, types, spawn_rate | 不同类型敌人的刷怪波 |
| 战利品 | table, count, rarity | 随机物品掉落 |
| NPC 对话 | branches, mood | 随机分支对话 |
| 地图地形 | biomes, size, seed | 不同生态的地形布局 |

完整可运行示例见 `examples/cave-gen/`，集成测试见 `crates/vitric-cli/tests/cave_gen.rs`。

---

## 配方 19：帧动画（animations.json / Anim 组件 / 动画状态切换）

**目标**：让精灵动起来——走路、待机、攻击、旋转。没有动画的游戏是 demo，有动画才是商业游戏。

Vitric 的动画系统是数据驱动的：在 `animations.json` 里定义动画片段（clip），在 `Anim` 组件里追踪当前播放状态，引擎自动推进帧。

### 1. animations.json 格式

项目根目录放 `animations.json`，定义所有动画片段：

```json
{
  "clips": {
    "idle": {
      "frames": ["player.png"],
      "fps": 1,
      "loop": true
    },
    "walk": {
      "frames": ["player-walk-0.png", "player-walk-1.png"],
      "fps": 6,
      "loop": true
    },
    "coin-spin": {
      "frames": ["coin-0.png", "coin-1.png", "coin-2.png", "coin-3.png"],
      "fps": 8,
      "loop": true
    }
  }
}
```

- `frames` — 帧图片文件名列表（相对于 `assets/` 目录）
- `fps` — 帧率（每秒帧数）
- `loop` — 是否循环播放

### 2. Anim 组件

```json
{
  "Anim": { "clip": "idle", "prev": "", "t": 0, "done": false }
}
```

| 字段 | 作用 |
|------|------|
| `clip` | 当前播放的动画片段名 |
| `prev` | 上一个片段名（用于检测切换） |
| `t` | 当前帧时间累计（引擎自动更新） |
| `done` | 非循环动画是否播放完毕 |

引擎的动画系统每 tick 自动更新 `t` 和 `Sprite.image`（设置为当前帧图片）。

### 3. 在 vitric.json 中引用

```json
{
  "name": "my-game",
  "schema": "schema.json",
  "entry": "scenes/main.json",
  "rules": ["rules/game.json"],
  "animations": "animations.json",
  "scripts": ["scripts/systems.js"]
}
```

### 4. 动画状态切换

在脚本里根据游戏状态切换动画：

```javascript
vitric.fn("update_anim", (args, ctx) => {
  const who = args.who;
  const vx = ctx.getField(who, "Velocity.x") || 0;
  const current = ctx.getField(who, "Anim.clip") || "idle";

  let next;
  if (Math.abs(vx) > 0.1) {
    next = "walk";
  } else {
    next = "idle";
  }
  if (next !== current) {
    ctx.setField(who, "Anim.clip", next);
  }
});
```

在规则里每 tick 调用：

```json
{
  "id": "update-player-anim",
  "on": "tick",
  "do": [{ "call": "update_anim", "with": { "who": "@player" } }]
}
```

### 5. 精灵图集（Atlas）

对于大量帧的动画，使用图集（atlas）减少加载开销：

```
assets/
  slide-atlas.png      # 合并后的图集图片
  slide-atlas.json     # 图集元数据（每帧在图集中的位置和尺寸）
  slide/
    frame000.png       # 原始帧（开发用，运行时用图集）
```

引擎自动检测图集并使用，开发者无需修改代码——`animations.json` 仍然引用原始帧名。

完整可运行示例见 `examples/frame-anim/`（基础图集动画）和 `examples/coin-run/`（游戏集成动画），集成测试见 `crates/vitric-cli/tests/animation.rs` 和 `crates/vitric-cli/tests/frames.rs`。

---

## 配方 20：音频系统（play-sound / play-music 约定事件 / 音量控制）

**目标**：让游戏有声音——背景音乐、跳跃音效、受伤音效、收集音效。静默的游戏是 demo，有声音才是商业游戏。

Vitric 的音频系统使用两个约定事件，由引擎自动处理，不需要写模块：

### 1. 两个约定事件

| 事件 | 用途 | 参数 |
|------|------|------|
| `play-sound` | 播放音效（一次性） | `sound`（文件名）, `volume`（0-1，默认 1） |
| `play-music` | 播放背景音乐（循环） | `sound`（文件名）, `volume`（0-1，默认 1） |

音频文件放在项目的 `sounds/` 目录下：

```
my-game/
  sounds/
    bgm.wav
    jump.wav
    hurt.wav
    coin.wav
    win.wav
```

### 2. 在规则中触发音效

```json
{
  "id": "play-bgm-on-start",
  "on": { "event": "game-start" },
  "do": [{ "emit": "play-music", "data": { "sound": "bgm.wav", "volume": 0.3 } }]
},
{
  "id": "play-jump-sound",
  "on": { "event": "input", "filter": { "action": "space", "phase": "pressed" } },
  "do": [{ "emit": "play-sound", "data": { "sound": "jump.wav" } }]
},
{
  "id": "play-hurt-sound",
  "on": { "event": "collision", "between": ["Player", "Hazard"] },
  "do": [
    { "emit": "play-sound", "data": { "sound": "hurt.wav" } },
    { "emit": "damage", "data": { "who": "@player", "amount": 10 } }
  ]
},
{
  "id": "play-win-sound",
  "on": { "event": "game-win" },
  "do": [{ "emit": "play-sound", "data": { "sound": "win.wav" } }]
}
```

### 3. 音量控制

- `volume: 1.0` — 最大音量
- `volume: 0.3` — 背景音乐常用（30% 音量，不盖过音效）
- `volume: 0.0` — 静音

可以在规则中根据游戏状态动态调整音量：

```json
{
  "id": "lower-music-on-dialogue",
  "on": { "event": "talk" },
  "do": [{ "emit": "play-music", "data": { "sound": "bgm.wav", "volume": 0.1 } }]
}
```

### 4. 确定性边界

音频是**输出副作用**——和 `play-sound` 一样在模拟之外执行。文件是否播放成功不影响世界状态，所以确定性回放不受影响。录制回放时，音频会照常播放（因为输入序列相同 → 事件序列相同 → 音效触发相同）。

### 5. 与游戏流程的配合

| 游戏事件 | 音频事件 | 文件 |
|----------|---------|------|
| game-start | play-music | bgm.wav (volume 0.3) |
| input: jump | play-sound | jump.wav |
| collision: Player+Hazard | play-sound | hurt.wav |
| collision: Player+Gem | play-sound | coin.wav |
| game-win | play-sound | win.wav |
| game-lose | play-sound | lose.wav |

完整可运行示例见 `examples/ember/`（BGM + 5 种音效）和 `examples/glow/`（BGM + 音效集成），集成测试见 `crates/vitric-cli/tests/` 相关测试。

---

## 配方 21：UI 系统（Ui 组件 / Button / 布局容器 / 主题 / 场景切换）

**目标**：让游戏有菜单——标题画面、暂停菜单、选项界面。没有 UI 的游戏是 demo，有 UI 才是商业游戏。

Vitric 的 UI 系统是组件驱动的：用 `Ui`（布局）、`Panel`（面板）、`UiLabel`（文字）、`Button`（按钮）、`Container`（布局容器）组件组合出界面，支持键盘/鼠标导航和主题切换。

### 1. UI 组件总览

| 组件 | 作用 | 关键字段 |
|------|------|---------|
| `UiRoot` | UI 根节点 | 无（标记实体为 UI 根） |
| `Ui` | 布局参数 | anchor, ox, oy, w, h, parent |
| `Panel` | 面板背景 | color |
| `UiLabel` | 文字标签 | content, size, color, align |
| `Button` | 可点击按钮 | action, theme, state |
| `Text` | 世界空间文字（HUD） | content |

### 2. 锚点系统

`Ui.anchor` 决定元素相对于父元素的位置：

| 锚点 | 含义 |
|------|------|
| `center` | 居中 |
| `top-center` | 顶部居中 |
| `top-left` | 左上角 |
| `bottom-center` | 底部居中 |
| `stretch` | 拉伸填充父元素（配合 ox/oy 留边距） |

```json
{
  "name": "title",
  "components": {
    "Ui": { "anchor": "top-center", "ox": 0, "oy": 28, "w": 560, "h": 56, "parent": "panel" },
    "UiLabel": { "content": "My Game", "size": 40, "color": "#f0f0f0", "align": "center" }
  }
}
```

### 3. 布局容器

`Container` 组件自动排列子元素：

```json
{
  "name": "menu-vbox",
  "components": {
    "Ui": { "anchor": "stretch", "ox": 60, "oy": 130, "parent": "menu-panel" },
    "Container": { "kind": "VBox", "gap": 24, "pad": 0, "main": "start", "cross": "center" }
  }
}
```

| 字段 | 作用 |
|------|------|
| `kind` | `VBox`（垂直排列）或 `HBox`（水平排列） |
| `gap` | 子元素间距 |
| `main` | 主轴对齐：`start` / `center` / `end` |
| `cross` | 交叉轴对齐：`start` / `center` / `end` |

### 4. 按钮与状态

```json
{
  "name": "btn-start",
  "components": {
    "Ui": { "anchor": "top-left", "w": 460, "h": 72, "parent": "menu-vbox" },
    "Panel": { "color": "#3a4a6b" },
    "Button": { "action": "start", "theme": "dark", "state": "focused" }
  }
}
```

| `state` | 含义 | 视觉 |
|---------|------|------|
| `normal` | 默认状态 | 主题 normal 色 |
| `focused` | 键盘焦点 | 主题 focus 色 |
| `pressed` | 按下中 | 主题 pressed 色 |
| `disabled` | 禁用 | 主题 disabled 色（灰色） |

`Button.action` 是按钮点击时发出的 `ui-activate` 事件的 action 值。

### 5. 主题系统

主题文件是 JSON，定义颜色和尺寸：

```json
{
  "colors": {
    "bg": "#1b1d26",
    "text": "#f0f0f0",
    "focus": "#5a7bb5",
    "disabled": "#555555"
  },
  "font_size": 30,
  "padding": 12,
  "button": {
    "normal":   { "bg": "#3a4a6b", "text": "#e8ecf4" },
    "focused":  { "bg": "#5a7bb5", "text": "#ffffff" },
    "pressed":  { "bg": "#9fc0f0", "text": "#10131a" },
    "disabled": { "bg": "#2a2d36", "text": "#6b6f7a" }
  }
}
```

在 `vitric.json` 中引用：

```json
{
  "themes": ["themes/dark.json"],
  "font": "fonts/DejaVuSans.ttf"
}
```

### 6. 场景切换（菜单 → 游戏）

`vitric.json` 声明多个场景，`entry` 是初始场景：

```json
{
  "entry": "scenes/menu.json",
  "scenes": ["scenes/menu.json", "scenes/game.json"]
}
```

规则监听按钮激活事件，切换场景：

```json
{
  "id": "start-game-on-click",
  "on": { "event": "ui-activate", "filter": { "action": "start" } },
  "do": [
    { "emit": "scene-change", "data": { "scene": "scenes/game.json" } },
    { "emit": "game-started" }
  ]
}
```

### 7. 导航控制

- **键盘**：方向键移动焦点，Enter/Space 确认
- **鼠标**：点击直接激活按钮
- **RPC**：`input/ui-click-by-name` 按实体名激活按钮（用于自动化测试）

```json
{
  "id": "navigate-focus",
  "on": { "event": "input", "filter": { "action": "down", "phase": "pressed" } },
  "do": [{ "call": "ui_focus_next", "with": { "dir": "down" } }]
}
```

### 8. UI 与游戏流程的完整闭环

```
menu scene (title + buttons)
  ↓ Enter on "Start" button
  ↓ scene-change → game scene
game scene (gameplay + HUD)
  ↓ press Esc
  ↓ scene-change → pause scene
pause scene (resume/quit buttons)
  ↓ Enter on "Resume"
  ↓ scene-change → game scene
```

完整可运行示例见 `examples/ui-menu/`（菜单 + 场景切换 + 主题）和 `examples/ui-gallery/`（UI 组件展示），集成测试见 `crates/vitric-cli/tests/ui.rs` 和 `crates/vitric-cli/tests/ui_interact.rs`。

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
