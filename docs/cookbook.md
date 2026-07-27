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
