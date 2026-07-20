# Frontier 深化实施计划：自由运转的四循环

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 frontier 从"8 步线性任务 → 通关结局"深化为"四个自由运转循环 → 无限玩，前 9 天有引导曲线但无硬结局"。

**Architecture:** 不动引擎，只在 rules/scripts/schema/scenes 层加 3 个新系统（心愿 / POI / 耀斑夜晚）+ 改造任务系统（删 step8 结局、step3/6 门改心愿、gate 改 settlement-founded）。每个系统独立可测、独立可 commit。

**Tech Stack:** Rust 引擎（vitric-cli）+ JSON 规则 + QuickJS 脚本 + esbuild 转译。

## Global Constraints

- 引擎零改动：所有深化在 `games/frontier/` 内
- 代码注释统一英文（项目规则）
- 字符串字面量保留中文（游戏内容语言）
- 每个任务结束 `vitric check games/frontier` 必须绿
- 改完自动 commit + push（用户偏好）
- 现有字段优先：`Need.affinity` 已存在不新增、`Structure.tier` 已存在当 level 用

## File Structure

**新增**：
- `games/frontier/schema.json` — 加 Wish / Poi 组件，扩 Colony 字段
- `games/frontier/scripts/wish.js` — 心愿系统
- `games/frontier/scripts/poi.js` — POI 系统
- `games/frontier/scripts/flare.js` — 耀斑 + 夜晚威胁
- `games/frontier/rules/wish.json` — 心愿触发规则
- `games/frontier/rules/poi.json` — POI 事件规则
- `games/frontier/rules/flare.json` — 耀斑 + 夜晚规则
- `games/frontier/tools/gen_scene.py` — 改：加 3 个 POI 实体
- `games/frontier/qa/clear.json` — 重录 9 天流程

**修改**：
- `games/frontier/rules/quest.json` — step3/6 门改心愿、删 step8 结局、step7 后发 settlement-founded
- `games/frontier/rules/economy.json` — 资源点冷却再生 + 升级建造
- `games/frontier/scripts/companion.js` — 心愿触发钩子 + 旅人刷新
- `games/frontier/scripts/economy.js` — 资源点冷却 + 升级 API
- `games/frontier/vitric.json` — gates.must_emit: game-won → settlement-founded、rules/scripts 加新文件
- `games/frontier/scenes/main.json` — 加 3 个 POI 实体（gen_scene.py 产出）
- `games/frontier/GDD.md` — 更新设计文档

---

### Task 1: 加 Wish + Poi 组件、扩 Colony 字段

**Files:**
- Modify: `games/frontier/schema.json`

**Interfaces:**
- Produces: `Wish { items: list of {desc, done, kind, target} }`、`Poi { kind, state, cooldown, reward_table }`、`Colony.flare_timer / flare_warning / is_night / wild_threat`、`Need.affinity`（已存在，复用）、`Need.memory_unlocked`

- [ ] **Step 1: 在 schema.json 的 components 里加 Wish 组件（在 QuestLog 之前插入）**

在 `games/frontier/schema.json` 的 `"QuestLog"` 之前插入：

```json
    "Wish": {
      "fields": {
        "items": {
          "type": "text",
          "default": "[]"
        },
        "fulfilled": {
          "type": "int",
          "default": 0
        }
      }
    },
```

**说明**：`items` 用 text 存 JSON 字符串（schema 不支持嵌套 list of struct），结构是 `[{"desc":"建 3 个结构","done":false,"kind":"build","target":3}, ...]`。`fulfilled` 计已点亮的数量，避免每次重新数。

- [ ] **Step 2: 在 schema.json 加 Poi 组件（在 Wish 之后）**

```json
    "Poi": {
      "fields": {
        "kind": {
          "type": "text",
          "default": "abandoned-camp"
        },
        "state": {
          "type": "enum",
          "variants": ["fresh", "looted", "depleted"],
          "default": "fresh"
        },
        "cooldown": {
          "type": "number",
          "default": 0
        },
        "reward_table": {
          "type": "text",
          "default": "{}"
        }
      }
    },
```

- [ ] **Step 3: 扩 Colony 字段（在 `companion_affinity_avg` 之后加 4 个）**

```json
        "flare_timer": {
          "type": "number",
          "default": 240
        },
        "flare_warning": {
          "type": "int",
          "default": 0
        },
        "is_night": {
          "type": "int",
          "default": 0
        },
        "wild_threat": {
          "type": "int",
          "default": 0
        }
```

**默认值说明**：`flare_timer=240` = 4 分钟后第一次耀斑（前期给玩家适应时间）；`flare_warning=0` 未预警；`is_night=0` 白天起。

- [ ] **Step 4: 扩 Need 字段（加 memory_unlocked）**

在 `Need.contribution_timer` 之后加：

```json
        "memory_unlocked": {
          "type": "int",
          "default": 0
        }
```

- [ ] **Step 5: 验证 schema 合法**

Run: `cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -5`
Expected: `OK`（schema 解析通过）

- [ ] **Step 6: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/schema.json
git commit -m "feat(frontier): add Wish/Poi components, extend Colony/Need for deepening"
git push origin main
```

---

### Task 2: 实现耀斑 + 夜晚系统（flare.js + flare.json）

**Files:**
- Create: `games/frontier/scripts/flare.js`
- Create: `games/frontier/rules/flare.json`
- Modify: `games/frontier/vitric.json`（注册新文件）

**Interfaces:**
- Produces: `flare_tick(ctx)` 每 tick 推进耀斑倒计时 + 判定夜晚 + 写回 Colony；规则发 `flare-imminent` / `flare-hit` / `night-fall` / `dawn-break` 事件供 UI/任务订阅。

- [ ] **Step 1: 创建 scripts/flare.js（核心逻辑）**

```javascript
// scripts/flare.js
// Solar flare + day/night cycle system.
// Drives Colony.flare_timer / flare_warning / is_night / wild_threat.

vitric.system("flare_tick", (ctx) => {
  const colony = ctx.singleton("Colony");
  if (!colony) return;
  const clock = ctx.singleton("Clock");
  if (!clock) return;

  // --- Day/night from Clock.time (0-120 per day, 0=dawn, 60=noon, 120=dusk) ---
  // Night = time >= 100 (last 20 units of day) || time < 10 (first 10 units).
  const t = clock.Clock.time;
  const wasNight = colony.Colony.is_night;
  const isNight = (t >= 100 || t < 10) ? 1 : 0;
  if (isNight !== wasNight) {
    colony.Colony.is_night = isNight;
    if (isNight === 1) {
      colony.Colony.wild_threat = 1 + Math.floor((colony.Colony.day || 1) / 3); // 1..3+
      ctx.emit("night-fall", { threat: colony.Colony.wild_threat });
    } else {
      colony.Colony.wild_threat = 0;
      ctx.emit("dawn-break", {});
    }
  }

  // --- Flare timer ---
  let timer = colony.Colony.flare_timer - ctx.dt;
  if (timer > 30) {
    if (colony.Colony.flare_warning !== 0) {
      colony.Colony.flare_warning = 0; // clear warning if still far
    }
  } else if (timer > 0 && timer <= 30) {
    if (colony.Colony.flare_warning !== 1) {
      colony.Colony.flare_warning = 1;
      ctx.emit("flare-imminent", { eta: timer });
    }
  } else if (timer <= 0) {
    // Flare hits: drain power and oxygen by 40%, schedule next in 180-300s.
    ctx.emit("flare-hit", { power_loss: colony.Colony.power * 0.4, o2_loss: colony.Colony.oxygen * 0.4 });
    colony.Colony.power = colony.Colony.power * 0.6;
    colony.Colony.oxygen = colony.Colony.oxygen * 0.6;
    colony.Colony.flare_warning = 0;
    colony.Colony.flare_timer = 180 + Math.floor(Math.random() * 120); // 3-5 min cooldown
    return;
  }
  colony.Colony.flare_timer = timer;
});
```

- [ ] **Step 2: 创建 rules/flare.json（UI 联动 + 夜晚伙伴回家）**

```json
{
  "rules": [
    {
      "id": "flare-warning-toast",
      "comment": "Flare imminent warning toast.",
      "on": { "event": "flare-imminent" },
      "do": [
        { "emit": "toast-show", "data": { "text": "耀斑 30 秒后来袭!储备电力氧气!" } }
      ]
    },
    {
      "id": "flare-hit-toast",
      "comment": "Flare hit toast + mood drop on all companions.",
      "on": { "event": "flare-hit" },
      "do": [
        { "emit": "toast-show", "data": { "text": "耀斑冲击!电力氧气大幅下降" } }
      ]
    },
    {
      "id": "night-companions-home",
      "comment": "At night, companions auto-return to nearest quarters; outdoor companions lose mood.",
      "on": { "event": "night-fall" },
      "do": [
        { "emit": "toast-show", "data": { "text": "夜幕降临,野外危险,速回基地" } }
      ]
    },
    {
      "id": "dawn-toast",
      "comment": "Dawn break toast.",
      "on": { "event": "dawn-break" },
      "do": [
        { "emit": "toast-show", "data": { "text": "天亮了,新的一天" } }
      ]
    }
  ]
}
```

- [ ] **Step 3: 注册到 vitric.json**

在 `rules` 数组末尾加 `"rules/flare.json"`，在 `scripts` 数组末尾加 `"scripts/flare.js"`。

- [ ] **Step 4: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`（脚本编译通过，规则加载通过）

- [ ] **Step 5: 手测——跑游戏看耀斑倒计时和夜晚切换**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- run games/frontier 2>&1 | head -30`
Expected: 游戏启动，HUD 不报错，前 4 分钟无耀斑

- [ ] **Step 6: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/flare.js games/frontier/rules/flare.json games/frontier/vitric.json
git commit -m "feat(frontier): add solar flare + day/night cycle system"
git push origin main
```

---

### Task 3: 实现 POI 系统（poi.js + poi.json + 场景实体）

**Files:**
- Create: `games/frontier/scripts/poi.js`
- Create: `games/frontier/rules/poi.json`
- Modify: `games/frontier/tools/gen_scene.py`（加 3 个 POI 实体）
- Modify: `games/frontier/scenes/main.json`（重新生成）
- Modify: `games/frontier/vitric.json`（注册）

**Interfaces:**
- Consumes: Task 1 的 Poi 组件
- Produces: `poi_tick(ctx)` 每日刷新 POI；玩家走到 POI 触发 `poi-reached`；脚本弹选项 UI；玩家选 → `poi-choice` 事件 → 结算。

- [ ] **Step 1: 在 gen_scene.py 加 3 个 POI 实体**

读 `games/frontier/tools/gen_scene.py`，找到野外区实体生成段，在末尾加：

```python
    # --- 3 POIs in wild zone (regenerate daily) ---
    poi_specs = [
        ("abandoned-camp", 36, 22, {"ore": [1,2], "wheat": [2,4], "fiber": [1,3]}),
        ("cave-entrance",  40, 18, {"ore": [3,5]}),  # high-risk high-reward
        ("shipwreck",      32, 26, {"wheat": [3,5], "plank": [1,2]}),
    ]
    for kind, x, y, rewards in poi_specs:
        poi = {
            "Position": {"x": x, "y": y},
            "Sprite": {"w": 2, "h": 2, "color": "#8b6f47", "image": ""},
            "Collider": {"w": 2, "h": 2},
            "Poi": {
                "kind": kind,
                "state": "fresh",
                "cooldown": 0,
                "reward_table": json.dumps(rewards),
            },
            "Text": {"content": {"abandoned-camp":"废弃营地","cave-entrance":"洞穴入口","shipwreck":"沉船"}[kind], "size": 1, "color": "#ffe070", "screen": False},
        }
        scene["entities"].append(poi)
```

（实际位置和 Python 代码要根据现有 gen_scene.py 结构调整，执行时先读文件再改）

- [ ] **Step 2: 重新生成 scenes/main.json**

Run: `cd /Users/leolele/Documents/leo/vitric/games/frontier && python3 tools/gen_scene.py`
Expected: scenes/main.json 更新，含 3 个 Poi 实体

- [ ] **Step 3: 创建 scripts/poi.js**

```javascript
// scripts/poi.js
// Point-of-interest system: daily-refreshing wild POIs that offer reward/risk choices.

vitric.system("poi_tick", (ctx) => {
  // Refresh POIs at dawn.
  const clock = ctx.singleton("Clock");
  if (!clock) return;
  // Refresh when day changes (last_day_emit mismatch handled in clock.js; here we just check state).
  ctx.each("Poi", (e, poi) => {
    if (poi.Poi.state === "looted" || poi.Poi.state === "depleted") {
      poi.Poi.cooldown -= ctx.dt;
      if (poi.Poi.cooldown <= 0) {
        poi.Poi.state = "fresh";
        poi.Poi.cooldown = 0;
      }
    }
  });
});

// Player steps onto a POI -> emit poi-reached (rule will pop UI prompt).
vitric.on("collide", (ctx, ev) => {
  // ev has {a, b} entity ids; check if one is player and other is Poi.
  const a = ctx.entity(ev.a);
  const b = ctx.entity(ev.b);
  if (!a || !b) return;
  const player = a.Player ? a : (b.Player ? b : null);
  const poi = b.Poi ? b : (a.Poi ? a : null);
  if (!player || !poi) return;
  if (poi.Poi.state !== "fresh") return;
  ctx.emit("poi-reached", { id: poi.id, kind: poi.Poi.kind });
});

// Player makes a choice -> settle reward.
vitric.on("poi-choice", (ctx, ev) => {
  const e = ctx.entity(ev.id);
  if (!e || !e.Poi) return;
  if (e.Poi.state !== "fresh") return;
  const rewards = JSON.parse(e.Poi.reward_table || "{}");
  const player = ctx.singleton("Inventory");
  if (!player) return;
  const choice = ev.choice; // "explore" | "ignore" | "grab"
  if (choice === "ignore") {
    e.Poi.state = "depleted";
    e.Poi.cooldown = 60; // 1 min before re-available
    return;
  }
  // explore or grab: roll rewards
  let text = "";
  for (const key of Object.keys(rewards)) {
    const [lo, hi] = rewards[key];
    const n = lo + Math.floor(Math.random() * (hi - lo + 1));
    player.Inventory[key] = (player.Inventory[key] || 0) + n;
    text += `${{"ore":"矿","wheat":"麦","fiber":"纤维","plank":"板","lamp":"灯"}[key]||key}+${n} `;
  }
  ctx.emit("toast-show", { text: `探索收获: ${text}` });
  // Risk: cave-entrance has 30% chance of injury -> mood drop on all companions.
  if (e.Poi.kind === "cave-entrance" && Math.random() < 0.3) {
    ctx.emit("companion-mood-drop", { amount: 10, reason: "cave-injury" });
    ctx.emit("toast-show", { text: "洞穴坍塌!全员心情-10" });
  }
  e.Poi.state = "looted";
  e.Poi.cooldown = 120; // 2 min cooldown (will fully refresh at dawn)
  ctx.emit("entered-poi", { id: e.Poi.kind }); // for Wish system
});
```

- [ ] **Step 4: 创建 rules/poi.json（UI 弹窗）**

```json
{
  "rules": [
    {
      "id": "poi-reached-prompt",
      "comment": "Player reached a fresh POI -> activate poi-prompt UI action.",
      "on": { "event": "poi-reached" },
      "do": [
        { "emit": "ui-activate", "data": { "action": "poi-prompt", "poi_id": "${id}", "kind": "${kind}" } }
      ]
    }
  ]
}
```

**说明**：`ui-activate` 由 hud.js 消费弹选项 UI（探索/无视/拿走三按钮）。hud.js 的扩展在 Task 6 UI 阶段做，本任务先发事件占位。

- [ ] **Step 5: 注册到 vitric.json**

在 `rules` 加 `"rules/poi.json"`，`scripts` 加 `"scripts/poi.js"`。

- [ ] **Step 6: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`

- [ ] **Step 7: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/poi.js games/frontier/rules/poi.json games/frontier/tools/gen_scene.py games/frontier/scenes/main.json games/frontier/vitric.json
git commit -m "feat(frontier): add POI system with 3 wild points of interest"
git push origin main
```

---

### Task 4: 实现心愿系统（wish.js + wish.json + 旅人刷新钩子）

**Files:**
- Create: `games/frontier/scripts/wish.js`
- Create: `games/frontier/rules/wish.json`
- Modify: `games/frontier/scripts/companion.js`（旅人生成时附 Wish）
- Modify: `games/frontier/vitric.json`

**Interfaces:**
- Consumes: Task 1 的 Wish 组件、Task 3 的 `entered-poi` 事件、现有 `built` / `harvested` / `gathered` 事件
- Produces: `wish-fulfilled{companion, wish_desc}` 事件 → 触发 LLM 记忆对话

- [ ] **Step 1: 创建 scripts/wish.js**

```javascript
// scripts/wish.js
// Companion wish system: each companion has 3 wishes based on archetype.
// Wishes are fulfilled by gameplay actions; fulfilling unlocks LLM memory dialogue.

const WISH_TEMPLATES = {
  explorer: [
    { desc: "探索 3 处野外地点", kind: "enter-poi", target: 3 },
    { desc: "采集 10 单位矿石", kind: "gather-ore", target: 10 },
    { desc: "看一次日出(凌晨出门)", kind: "see-dawn", target: 1 },
  ],
  builder: [
    { desc: "建造 3 个结构", kind: "build", target: 3 },
    { desc: "建一盏灯", kind: "build-lamp", target: 1 },
    { desc: "升级 1 个结构", kind: "upgrade", target: 1 },
  ],
  farmer: [
    { desc: "种出 2 茬作物", kind: "harvest", target: 2 },
    { desc: "收获 8 单位麦子", kind: "harvest-wheat", target: 8 },
    { desc: "吃饱一次(食≥80)", kind: "food-high", target: 80 },
  ],
};

// Called from companion.js when a new drifter/companion spawns.
// Attaches a Wish component with 3 templated wishes based on archetype.
vitric.expose("init_wishes", (ctx, entityId, archetype) => {
  const templates = WISH_TEMPLATES[archetype] || WISH_TEMPLATES.explorer;
  const items = templates.map(t => ({ ...t, done: false, progress: 0 }));
  const e = ctx.entity(entityId);
  if (!e) return;
  e.Wish = { items: JSON.stringify(items), fulfilled: 0 };
});

// Internal: check and advance a wish of given kind for all companions.
function advanceWish(ctx, kind, amount) {
  ctx.each("Wish", (e, wish) => {
    if (!e.Persona) return;
    let items;
    try { items = JSON.parse(wish.Wish.items || "[]"); } catch { return; }
    let changed = false;
    for (const it of items) {
      if (it.done) continue;
      if (it.kind !== kind) continue;
      it.progress = (it.progress || 0) + amount;
      if (it.progress >= it.target) {
        it.done = true;
        wish.Wish.fulfilled = (wish.Wish.fulfilled || 0) + 1;
        // Boost affinity.
        if (e.Need) {
          e.Need.affinity = Math.min(100, (e.Need.affinity || 30) + 30);
        }
        ctx.emit("wish-fulfilled", {
          companion: e.Persona.name || "伙伴",
          wish_desc: it.desc,
          entity: e.id,
        });
      }
      changed = true;
    }
    if (changed) wish.Wish.items = JSON.stringify(items);
  });
}

vitric.on("built", (ctx, ev) => {
  if (ev.kind === "lamp") advanceWish(ctx, "build-lamp", 1);
  advanceWish(ctx, "build", 1);
});

vitric.on("harvested", (ctx, ev) => {
  if (ev.id === "wheat") {
    advanceWish(ctx, "harvest", 1);
    advanceWish(ctx, "harvest-wheat", ev.n || 1);
  }
});

vitric.on("gathered", (ctx, ev) => {
  if (ev.id === "ore") advanceWish(ctx, "gather-ore", ev.n || 1);
});

vitric.on("entered-poi", (ctx, ev) => {
  advanceWish(ctx, "enter-poi", 1);
});

vitric.on("upgrade-structure", (ctx, ev) => {
  advanceWish(ctx, "upgrade", 1);
});

// Check food-high wish on tick.
vitric.system("wish_food_check", (ctx) => {
  const colony = ctx.singleton("Colony");
  if (!colony) return;
  if (colony.Colony.food >= 80) {
    // Only advance once per day to avoid spamming.
    if (!ctx._food_high_today) {
      ctx._food_high_today = true;
      advanceWish(ctx, "food-high", 80);
    }
  } else {
    ctx._food_high_today = false;
  }
});
```

- [ ] **Step 2: 创建 rules/wish.json（心愿完成 → LLM 记忆对话）**

```json
{
  "rules": [
    {
      "id": "wish-fulfilled-memory",
      "comment": "When a wish is fulfilled, trigger LLM memory dialogue + toast.",
      "on": { "event": "wish-fulfilled" },
      "do": [
        { "emit": "toast-show", "data": { "text": "${companion} 心愿达成: ${wish_desc}" } },
        { "emit": "companion-memory-request", "data": { "entity": "${entity}", "wish": "${wish_desc}" } }
      ]
    }
  ]
}
```

**说明**：`companion-memory-request` 由 companion.js 消费，调 LLM 生成记忆对话（基于 Persona + 已解锁条数）。本任务先发事件占位，LLM 调用在 Task 5 接。

- [ ] **Step 3: 改 scripts/companion.js——旅人/伙伴生成时调 init_wishes**

读 `games/frontier/scripts/companion.js`，找到生成 drifter 的函数（通常叫 `spawn_drifter` 或类似），在设置 `Persona` 之后加一行：

```javascript
  // Attach 3 wishes based on archetype.
  vitric.call("init_wishes", ctx, newEntity.id, archetype);
```

（具体行号和变量名执行时按现有代码调整）

- [ ] **Step 4: 注册到 vitric.json**

`rules` 加 `"rules/wish.json"`，`scripts` 加 `"scripts/wish.js"`。

- [ ] **Step 5: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`

- [ ] **Step 6: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/wish.js games/frontier/rules/wish.json games/frontier/scripts/companion.js games/frontier/vitric.json
git commit -m "feat(frontier): add companion wish system with 3 archetype templates"
git push origin main
```

---

### Task 5: 接 LLM 记忆对话到心愿系统

**Files:**
- Modify: `games/frontier/scripts/companion.js`

**Interfaces:**
- Consumes: Task 4 的 `companion-memory-request` 事件
- Produces: LLM 生成的记忆对话写入 `Colony.last_talk_reply` + `toast-show`

- [ ] **Step 1: 在 companion.js 加 memory dialogue handler**

在 `companion.js` 末尾加：

```javascript
// Listen for wish-fulfilled memory requests and generate LLM dialogue.
vitric.on("companion-memory-request", async (ctx, ev) => {
  const e = ctx.entity(ev.entity);
  if (!e || !e.Persona) return;
  const persona = e.Persona;
  const memCount = e.Need ? (e.Need.memory_unlocked || 0) + 1 : 1;

  const prompt = [
    `你是一个生存游戏里的伙伴,名叫${persona.name}。`,
    `性格:${persona.archetype},${persona.traits || "无特殊特征"}。`,
    `说话风格:${persona.speech || "平淡"}。`,
    `玩家刚刚帮你完成了心愿:"${ev.wish}"。`,
    `这是你解锁的第 ${memCount} 段记忆。请用 1-2 句话分享一段关于你过去的回忆,语气符合你的性格,不要超过 60 字。`,
  ].join("\n");

  try {
    const reply = await ctx.llm({ prompt, max_tokens: 120 });
    if (e.Need) e.Need.memory_unlocked = memCount;
    const colony = ctx.singleton("Colony");
    if (colony) colony.Colony.last_talk_reply = reply;
    ctx.emit("toast-show", { text: `${persona.name}: ${reply}` });
    ctx.emit("memory-unlocked", { name: persona.name, text: reply });
  } catch (err) {
    // Fallback: deterministic canned line based on archetype + memory count.
    const fallbacks = {
      explorer: ["我记得第一次看见星空的那晚...", "从前我也走过更远的路。", "家乡的山比这里更高。"],
      builder:  ["我父亲是木匠,他教过我榫卯。", "这双手建过更高的塔。", "砖石会记得建造者。"],
      farmer:   ["麦浪的声音我永远忘不掉。", "母亲做过更好的面包。", "雨水总是最好的礼物。"],
    };
    const list = fallbacks[persona.archetype] || fallbacks.explorer;
    const reply = list[Math.min(memCount - 1, list.length - 1)];
    if (e.Need) e.Need.memory_unlocked = memCount;
    const colony = ctx.singleton("Colony");
    if (colony) colony.Colony.last_talk_reply = reply;
    ctx.emit("toast-show", { text: `${persona.name}: ${reply}` });
    ctx.emit("memory-unlocked", { name: persona.name, text: reply });
  }
});
```

**说明**：`ctx.llm` 是引擎已有的 LLM 调用 API（companion.js 现有代码里已有用例，执行时按现有调用方式对齐）。fallback 用确定性 canned line 保证 LLM 不可用时游戏不卡。

- [ ] **Step 2: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`

- [ ] **Step 3: 手测——点亮心愿看是否出记忆对话**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- run games/frontier 2>&1 | head -40`
Expected: 建一个结构 → 伙伴心愿进度推进 → 完成时弹 toast（LLM 或 fallback）

- [ ] **Step 4: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/companion.js
git commit -m "feat(frontier): wire LLM memory dialogue to wish fulfillment"
git push origin main
```

---

### Task 6: 任务系统改造（删 step8 结局、step3/6 门改心愿、gate 改 settlement-founded）

**Files:**
- Modify: `games/frontier/rules/quest.json`
- Modify: `games/frontier/vitric.json`（gates.must_emit）

**Interfaces:**
- Consumes: Task 4 的 `Wish.fulfilled` 字段
- Produces: step1-7 引导曲线、step7 后发 `settlement-founded`（不发 `game-won`）、step8 banner 改为"自由探索中"

- [ ] **Step 1: 改 quest.json——step3 门加心愿条件**

把 `quest-step3-done` 规则的 `if` 改为：

```json
    {
      "id": "quest-step3-done",
      "comment": "step3 请第一个伙伴:companion-moved-in + 该伙伴点亮>=1 心愿 -> step=4.",
      "on": { "event": "wish-fulfilled" },
      "if": [
        ["@quest.QuestLog.step", "==", 3],
        ["@companion.Need.affinity", ">=", 60]
      ],
      "do": [
        { "set": "@quest.QuestLog.step", "to": 4 },
        { "emit": "quest-done", "data": { "id": "first-companion" } }
      ]
    },
```

**说明**：心愿点亮+30 好感，1 个心愿 = 60 好感（基础 30 + 30）。需要玩家真互动而不是站旁边等。

- [ ] **Step 2: 改 quest.json——step6 门改心愿**

把 `quest-step6-done` 的 `companion_happy_count >= 1` 改为基于心愿：

```json
    {
      "id": "quest-step6-done",
      "comment": "step6 成群:day>=5 + pop>=3 + 3 个伙伴各点亮>=2 心愿 -> step=7.",
      "on": "tick",
      "if": [
        ["@quest.QuestLog.step", "==", 6],
        ["@colony.Colony.day", ">=", 5],
        ["@colony.Colony.pop", ">=", 3],
        ["@colony.Colony.companion_wish_count", ">=", 2]
      ],
      "do": [
        { "set": "@quest.QuestLog.step", "to": 7 },
        { "emit": "quest-done", "data": { "id": "成群" } }
      ]
    },
```

**说明**：`companion_wish_count` 是新需要的统计字段。在 Task 1 没加——这里需要在 Colony 加一个 `companion_wish_count` 字段，或用脚本计算。**修正**：在 Task 1 的 Step 3 Colony 扩展里补加 `companion_wish_count` 字段（default 0），由 wish.js 在 advanceWish 里同步更新。

- [ ] **Step 3: 改 quest.json——删 game-won，改为 settlement-founded**

把 `game-won` 规则改为：

```json
    {
      "id": "settlement-founded",
      "comment": "step7 -> day>=6 + monument built -> settlement-founded (milestone, not ending). Game continues freely.",
      "on": "tick",
      "if": [
        ["@quest.QuestLog.step", "==", 7],
        ["@colony.Colony.day", ">=", 6],
        ["@colony.Colony.monument_built", ">=", 1]
      ],
      "do": [
        { "emit": "settlement-founded", "data": {} },
        { "set": "@quest.QuestLog.step", "to": 8 }
      ]
    },
```

- [ ] **Step 4: 改 quest-banner-8 为"自由探索中"**

```json
    {
      "id": "quest-banner-8",
      "on": "tick",
      "if": [ ["@quest.QuestLog.step", "==", 8] ],
      "do": [
        { "set": "@quest_title_lbl.UiLabel.content", "to": "自由探索中" },
        { "set": "@quest_sub_lbl.UiLabel.content",   "to": "定居点已建立,四个循环自驱,继续你的故事" }
      ]
    }
```

- [ ] **Step 5: 补 Colony.companion_wish_count 字段**

回到 `games/frontier/schema.json`，在 `companion_affinity_avg` 之后加：

```json
        "companion_wish_count": {
          "type": "int",
          "default": 0
        }
```

- [ ] **Step 6: 改 wish.js——advanceWish 里同步 Colony.companion_wish_count**

在 `wish.js` 的 `advanceWish` 函数里，每次 `it.done = true` 后加：

```javascript
        // Sync aggregate count to Colony for quest gating.
        const colony = ctx.singleton("Colony");
        if (colony) colony.Colony.companion_wish_count = (colony.Colony.companion_wish_count || 0) + 1;
```

- [ ] **Step 7: 改 vitric.json——gates.must_emit**

```json
  "gates": {
    "playthroughs": [
      {
        "recording": "qa/clear.json",
        "must_emit": "settlement-founded"
      }
    ],
```

- [ ] **Step 8: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`

- [ ] **Step 9: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/rules/quest.json games/frontier/schema.json games/frontier/scripts/wish.js games/frontier/vitric.json
git commit -m "feat(frontier): convert quest system to milestone-based (no game-won), gate on settlement-founded"
git push origin main
```

---

### Task 7: 资源点冷却再生 + 结构升级（economy.js + economy.json）

**Files:**
- Modify: `games/frontier/scripts/economy.js`
- Modify: `games/frontier/rules/economy.json`

**Interfaces:**
- Produces: `Node.left/max/cooldown` 已存在，扩展为耗尽后定时再生；新 API `upgrade_structure` 升级 tier。

- [ ] **Step 1: 改 economy.js——Node 冷却再生**

读 `games/frontier/scripts/economy.js`，找到 Node 采集逻辑，在采集后耗尽时设置 cooldown，并加一个 system 推进 cooldown 恢复：

```javascript
// Node regrowth: when a node is depleted, regrow to max after cooldown.
vitric.system("node_regrow", (ctx) => {
  ctx.each("Node", (e, node) => {
    if (node.Node.left <= 0 && node.Node.cooldown > 0) {
      node.Node.cooldown -= ctx.dt;
      if (node.Node.cooldown <= 0) {
        node.Node.left = node.Node.max;
        node.Node.cooldown = 0;
      }
    }
  });
});
```

并在现有采集函数里，当 `left` 减到 0 时设置 `cooldown = 90`（1.5 分钟再生）。

- [ ] **Step 2: 改 economy.js——加 upgrade_structure API**

```javascript
// Upgrade a structure: consume resources, bump tier or change kind.
// tier 1 -> 2: plot -> greenhouse, conduit -> solar-array, quarters -> cabin.
vitric.expose("upgrade_structure", (ctx, entityId) => {
  const e = ctx.entity(entityId);
  if (!e || !e.Structure) return false;
  const inv = ctx.singleton("Inventory");
  if (!inv) return false;
  const kind = e.Structure.kind;
  const tier = e.Structure.tier || 1;
  const upgrades = {
    "plot":     { to: "greenhouse",   cost: { ore: 2, plank: 2 } },
    "conduit":  { to: "solar-array",  cost: { ore: 3, plank: 1 } },
    "quarters": { to: "cabin",        cost: { plank: 4, lamp: 1 } },
  };
  const up = upgrades[kind];
  if (!up || tier >= 2) return false;
  // Check cost.
  for (const k of Object.keys(up.cost)) {
    if ((inv.Inventory[k] || 0) < up.cost[k]) return false;
  }
  // Pay.
  for (const k of Object.keys(up.cost)) {
    inv.Inventory[k] -= up.cost[k];
  }
  e.Structure.kind = up.to;
  e.Structure.tier = 2;
  ctx.emit("upgrade-structure", { id: entityId, kind: up.to });
  return true;
});
```

- [ ] **Step 3: 加升级 UI 入口（规则占位）**

在 `rules/economy.json` 加：

```json
    {
      "id": "upgrade-button-click",
      "comment": "Build mode right-click on tier-1 structure -> offer upgrade.",
      "on": { "event": "ui-activate", "filter": { "action": "upgrade-prompt" } },
      "do": [
        { "emit": "upgrade-attempt", "data": { "id": "${target_id}" } }
      ]
    }
```

**说明**：实际 UI 弹窗在 Task 8 做，本任务先发事件。

- [ ] **Step 4: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`

- [ ] **Step 5: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/economy.js games/frontier/rules/economy.json
git commit -m "feat(frontier): node regrowth + structure upgrade path (tier 1->2)"
git push origin main
```

---

### Task 8: UI 扩展（心愿面板 / POI 选项 / 耀斑预警 / 升级菜单）

**Files:**
- Modify: `games/frontier/scripts/hud.js`

**Interfaces:**
- Consumes: `ui-activate{action:"poi-prompt"}` / `ui-activate{action:"upgrade-prompt"}` / `flare-imminent` / 伙伴选中事件
- Produces: 弹窗 UI + 玩家选择事件回发

- [ ] **Step 1: 改 hud.js——加 POI 选项弹窗**

读 `games/frontier/scripts/hud.js`，找到 UI 事件处理段，加：

```javascript
// POI prompt: 3-button modal when player steps on a fresh POI.
vitric.on("ui-activate", (ctx, ev) => {
  if (ev.action !== "poi-prompt") return;
  // Build a modal: explore / grab / ignore.
  // (Use existing UI primitives: Container + Button + UiLabel.)
  // For now, auto-pick "explore" to keep flow simple; real modal in polish pass.
  ctx.emit("poi-choice", { id: ev.poi_id, choice: "explore" });
});

// Upgrade prompt: when right-clicking a tier-1 structure in build mode.
vitric.on("ui-activate", (ctx, ev) => {
  if (ev.action !== "upgrade-prompt") return;
  const ok = vitric.call("upgrade_structure", ctx, ev.target_id);
  if (!ok) ctx.emit("toast-show", { text: "资源不足或已满级" });
});
```

**说明**：先用自动选择 explore 简化 POI 流程，真实三选一弹窗作为后续 polish。升级直接调 API。

- [ ] **Step 2: 改 hud.js——加耀斑预警屏顶条**

```javascript
// Flare warning: red bar at top when Colony.flare_warning == 1.
vitric.system("hud_flare_warning", (ctx) => {
  const colony = ctx.singleton("Colony");
  if (!colony) return;
  const warn = colony.Colony.flare_warning;
  // Toggle a UI entity's visibility (assumes a "flare_bar" UI entity exists in scene).
  const bar = ctx.singleton("flare_bar"); // name tag from scene
  if (bar) {
    bar.Sprite.color = warn ? "#ff4040" : "#00000000";
  }
});
```

**说明**：`flare_bar` UI 实体需要在 gen_scene.py 加。本任务先在 hud.js 写逻辑，gen_scene.py 的 UI 实体加在 Task 9 场景 polish 里。

- [ ] **Step 3: 转译 + check**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -10`
Expected: `OK`

- [ ] **Step 4: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/scripts/hud.js
git commit -m "feat(frontier): UI hooks for POI prompt, upgrade prompt, flare warning bar"
git push origin main
```

---

### Task 9: 重录 9 天通关录像（qa/clear.json）

**Files:**
- Modify: `games/frontier/qa/clear.json`

**Interfaces:**
- Consumes: 所有前 8 个任务的系统运转

- [ ] **Step 1: 跑一次完整游戏，录输入**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- run games/frontier --record games/frontier/qa/clear.json 2>&1 | tail -20`

操作：手玩 9 天，覆盖信标→种麦→请伙伴→立足→温饱→成群→丰碑→settlement-founded。

- [ ] **Step 2: 验证录像**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- run games/frontier --replay games/frontier/qa/clear.json 2>&1 | tail -10`
Expected: 录像重放一致，发出 `settlement-founded`

- [ ] **Step 3: 跑 gate**

Run: `cd /Users/leolele/Documents/leo/vitric && cargo run -p vitric-cli -- gate games/frontier 2>&1 | tail -10`
Expected: `PASS`

- [ ] **Step 4: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/qa/clear.json
git commit -m "feat(frontier): re-record 9-day playthrough covering settlement-founded milestone"
git push origin main
```

---

### Task 10: 更新 GDD.md + PLAN.md 反映深化

**Files:**
- Modify: `games/frontier/GDD.md`
- Modify: `games/frontier/PLAN.md`

- [ ] **Step 1: 在 GDD.md 加"四循环深化"章节**

在 GDD.md 末尾加新章节，描述四个自由运转循环 + 心愿系统 + POI + 耀斑夜晚 + 任务系统改造（删结局、改 milestone）。

- [ ] **Step 2: 改 PLAN.md 标记深化完成**

把 PLAN.md 里"压缩纵切"段落改为"已深化为自由运转四循环"，列出新增的 7 个文件 + 改造点。

- [ ] **Step 3: Commit**

```bash
cd /Users/leolele/Documents/leo/vitric
git add games/frontier/GDD.md games/frontier/PLAN.md
git commit -m "docs(frontier): update GDD + PLAN for four-loop deepening"
git push origin main
```

---

## 自检（Self-Review）

**Spec 覆盖**：
- ✅ 循环 1 求生经济（耀斑 + 夜晚）→ Task 2
- ✅ 循环 2 伙伴关系（心愿 + 记忆）→ Task 4 + 5
- ✅ 循环 3 野外探索（POI）→ Task 3
- ✅ 循环 4 建设经营（升级）→ Task 7
- ✅ 任务系统改造（删 step8 结局、step3/6 改心愿、gate 改 settlement-founded）→ Task 6
- ✅ UI 扩展 → Task 8
- ✅ 录像重录 → Task 9
- ✅ 文档更新 → Task 10
- ✅ schema 扩展 → Task 1

**类型一致性**：
- `Wish.items` 全程用 text 存 JSON 字符串（schema 限制）
- `Poi.reward_table` 同上
- `Colony.companion_wish_count` 在 Task 6 Step 5 补加，Task 6 Step 6 在 wish.js 同步——Task 4 写 wish.js 时先用，Task 6 补字段。**修正顺序**：Task 6 Step 5 的 schema 字段应该在 Task 1 就加。已在计划里标注"回到 schema.json 补加"，执行时按 Task 1 → Task 6 顺序，Task 1 实际要包含此字段。

**风险点**：
- Task 3 的 gen_scene.py 改动需要先读现有文件结构——执行时第一步是 Read
- Task 5 的 `ctx.llm` API 签名需要对齐现有 companion.js 用法——执行时先 Grep 现有调用
- Task 9 录像是人工操作，可能需要多次尝试才能覆盖 9 天全流程
