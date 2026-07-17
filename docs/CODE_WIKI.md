# Vitric Code Wiki

> 本文档是 Vitric 仓库的结构化代码百科，覆盖项目整体架构、各模块职责、关键类型与函数、依赖关系与运行方式。
> 所有结论均来自源码摘录，链接指向仓库内真实文件位置。

## 目录

- [1. 项目概览](#1-项目概览)
- [2. 整体架构](#2-整体架构)
- [3. 核心设计原则](#3-核心设计原则)
- [4. Crate 详解](#4-crate-详解)
  - [4.1 vitric-ecs — 确定性可内省 ECS](#41-vitric-ecs--确定性可内省-ecs)
  - [4.2 vitric-data — 声明式数据层](#42-vitric-data--声明式数据层)
  - [4.3 vitric-rules — 当 X 则 Y 规则引擎](#43-vitric-rules--当-x-则-y-规则引擎)
  - [4.4 vitric-script — 戴安全带的 JS 脚本](#44-vitric-script--戴安全带的-js-脚本)
  - [4.5 vitric-sim — 确定性模拟核心](#45-vitric-sim--确定性模拟核心)
  - [4.6 vitric-render — 2D 光栅化与语义观察](#46-vitric-render--2d-光栅化与语义观察)
  - [4.7 vitric-control — AI 控制面](#47-vitric-control--ai-控制面)
  - [4.8 vitric-playtest — Agent 集群试玩](#48-vitric-playtest--agent-集群试玩)
  - [4.9 vitric-cli — 命令行与运行时装配](#49-vitric-cli--命令行与运行时装配)
- [5. 数据语言](#5-数据语言)
- [6. 运行时流水线](#6-运行时流水线)
- [7. 控制面与 MCP](#7-控制面与-mcp)
- [8. 自动试玩与交付门禁](#8-自动试玩与交付门禁)
- [9. CLI 命令参考](#9-cli-命令参考)
- [10. 构建与测试](#10-构建与测试)
- [11. 多 Agent 协同](#11-多-agent-协同)

---

## 1. 项目概览

**Vitric** 是一个**确定性、玻璃箱（glass-box）的 2D 游戏引擎，专为 AI Agent 设计**。现有引擎面向"坐在编辑器前的人"，对 AI 来说是黑箱；Vitric 围绕一套 **Agent API** 重建：引擎的一切状态可见、可操作、可验证，AI 能自主完成「运行游戏 → 观察像素与状态 → 断言 → 修改 → 重复」的闭环，无需人类介入。由于模拟是逐位确定的，引擎还能对游戏**证明**一些事情——一条录像能通关、一群 Agent 跑不软锁。

- **仓库**：`https://github.com/BlackBearCC/vitric`
- **License**：MIT © 2026 BlackBearCC
- **语言**：Rust（edition 2021）+ 嵌入式 QuickJS（脚本）+ TypeScript（esbuild 转译）
- **状态**：pre-1.0，核心已在 CI 中通过 650+ 测试（含一条「Agent 通过 HTTP 通关游戏、录像逐位重放」的端到端用例）

### 关键能力

| 能力 | 说明 |
|------|------|
| 确定性模拟 | 固定步长 1/60s、自实现 PCG32、全迭代有序、输入录制与逐校验点重放 |
| 一切即数据 | 场景/实体/规则/世界帧都是强 schema JSON，写入即校验、运行时可查、可往返 |
| 规则 + 脚本 | 80% 玩法是声明式 `when X then Y` 规则（不图灵完备）；20% 复杂逻辑落 JS/TS 系统（强制读写声明） |
| 无头渲染 | CPU 光栅化是确定性真相源；无 GPU/窗口即可截图，字节逐位可断言 |
| AI 控制面 | HTTP JSON-RPC：查改状态、注输入、控时间、断言、语义观察、截图 |
| Agent 集群试玩 | 多策略 swarm 跑批，聚合地板报告（通关率/软锁/惰性动作/数值崩/不可达结局） |
| 交付门禁 | 通关录像 + 终局事件 + 断言 = 不可伪造证书；可选 playtest 门进契约 |
| MCP server | 14 个工具，任何 MCP 客户端开箱即用 |

---

## 2. 整体架构

### 2.1 分层结构

```
┌─────────────────────────────────────────────────────────────┐
│  vitric-cli  — 命令行入口 + 运行时装配 + 窗口/音频/GPU/打包  │
│  (main.rs / runtime.rs / window.rs / gpu.rs / audio.rs …)    │
├─────────────────────────────────────────────────────────────┤
│  vitric-control     │  vitric-playtest                      │
│  (HTTP JSON-RPC)    │  (scene view + 策略 + swarm + 报告)   │
├─────────────────────────────────────────────────────────────┤
│  vitric-render  —  CPU 光栅化 + wgpu 镜像 + 语义 describe    │
├─────────────────────────────────────────────────────────────┤
│  vitric-script  —  QuickJS + 读写声明 + 热重载              │
├─────────────────────────────────────────────────────────────┤
│  vitric-rules   —  当X则Y规则引擎 + 级联保护                │
├─────────────────────────────────────────────────────────────┤
│  vitric-sim     —  固定步长 + PCG32 + 录像 + 快照           │
├─────────────────────────────────────────────────────────────┤
│  vitric-data    —  项目格式 + schema + 场景实例化 + 校验    │
├─────────────────────────────────────────────────────────────┤
│  vitric-ecs     —  可内省确定性 ECS（组件=JSON，BTreeMap）   │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Crate 依赖关系

依赖方向自下而上，无环（源：各 crate 的 [Cargo.toml](file:///Users/leolele/Documents/leo/vitric/Cargo.toml) `[dependencies]` 段）：

| Crate | 内部依赖 | 关键外部依赖 |
|-------|----------|--------------|
| vitric-ecs | — | serde, serde_json |
| vitric-data | ecs | serde, serde_json |
| vitric-rules | ecs, data | serde, serde_json |
| vitric-sim | ecs, rules | serde, serde_json |
| vitric-script | ecs, data, rules, sim | serde, serde_json, **rquickjs** |
| vitric-render | ecs | serde_json, **png**, **font8x8**, **ab_glyph** |
| vitric-control | ecs, data, rules, sim, render | serde, serde_json, **tiny_http** |
| vitric-playtest | ecs, data, rules, sim | serde, serde_json |
| vitric-cli | 全部 8 个内部 crate | **winit, softbuffer, wgpu, rodio, ureq, flate2, bytemuck, pollster, font8x8, png** |

要点：
- **vitric-ecs 是最底层叶子**，被所有其他内部 crate 依赖。
- **vitric-render 独立性强**——只依赖 ecs 拿 World，不碰规则/模拟，可单独用于纯渲染。
- **vitric-playtest 不依赖 render/control/script**——swarm 只跑模拟即可，无需开窗。
- **vitric-cli 是顶层装配点**，把全部 crate 接起来，并拉所有重外部依赖。

---

## 3. 核心设计原则

源：[README.md](file:///Users/leolele/Documents/leo/vitric/README.md)、[llms.txt](file:///Users/leolele/Documents/leo/vitric/llms.txt)、各 crate 顶部文档注释。

### 3.1 确定性是硬契约

- 固定步长 `DT = 1/60` 秒，不吃墙钟（墙钟只决定「跑几步」，永不进模拟）。
- 随机数自实现 PCG32（[crates/vitric-sim/src/pcg.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/pcg.rs)），状态可快照；Rust 与 JS 共用同一条流（JS 侧 BigInt 实现同算法）。
- 一切迭代有序——`World` 全部存储用 `BTreeMap`，同操作序列 = 同状态哈希。
- `Math.random` / `Date.now` 在脚本里被禁用并指路 `ctx.random()` / `ctx.tick`。
- **同种子 + 同输入序列 = 逐帧相同的世界**，任何 bug 都能拿录像精确重放到出错前一帧。

### 3.2 一切即数据

- 组件值就是 `serde_json::Value`——天生可序列化、可查询、可往返。
- 场景、规则、脚本、动画、序列、主题全是 JSON；存档是世界全量快照，没有状态藏在编辑器二进制里。
- 写入即校验，错误结构化（路径 + 错误码 + 修复提示），且**一次报全**而非第一个就停——给 LLM 看的。

### 3.3 规则正门 + 脚本安全带

- ~80% 玩法是声明式规则（`when X then Y`），**刻意不图灵完备**：条件只有比较和与，没有循环没有变量，防止规则语言长成一门烂编程语言。
- ~20% 复杂逻辑落 JS/TS 系统，但**强制声明 `query`/`writes`**——越权写直接报错，引擎永远知道每段逻辑碰了什么。
- 引擎独占 `Sprite.image` 写权（有 `Anim` 组件时），换动画改 `Anim.clip`——"动画被别的系统打断"不可能发生。

### 3.4 玻璃箱 / Agent API

- 引擎无头运行，内建 HTTP JSON-RPC 控制面：Agent 和人通过同一套数据接口驱动游戏。
- `render/describe` 给模型读的语义视图：实体清单 + 自我中心空间关系 + ASCII 关卡图 + 声明的输入动作 + 帧间 diff。
- 截图无头、字节逐位确定，可进断言。

---

## 4. Crate 详解

### 4.1 vitric-ecs — 确定性可内省 ECS

**职责**：最底层存储。组件值 = JSON，全 `BTreeMap` 有序存储，提供实体生命周期、组件读写、字段路径、查询、快照/哈希、空间关系。

源：[crates/vitric-ecs/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-ecs/src/lib.rs)

#### 关键类型

```rust
// src/entity.rs — 实体句柄，格式 "e<index>v<generation>"，如 "e12v3"
pub struct EntityId { pub index: u32, pub generation: u32 }

// src/world.rs — 世界
pub struct World {
    generations: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,
    components: BTreeMap<String, BTreeMap<u32, Value>>,  // 组件名 -> (槽位 -> 值)
    names: BTreeMap<String, u32>,
    slot_names: BTreeMap<u32, String>,
}
```

#### World 关键方法（[src/world.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-ecs/src/world.rs)）

| 方法 | 作用 |
|------|------|
| `spawn() / spawn_named(name)` | 生成实体（命名实体重名报 `NameTaken`） |
| `despawn(id)` | 销毁实体（代数 +1、清组件、注销名字、槽位进 free） |
| `clear_entities()` | 按槽位序销毁全部（走正规 despawn，旧句柄干净失效） |
| `is_alive(id) / entity(name) / name_of(id)` | 存活/按名查/反查名 |
| `set_component / get_component / has_component / remove_component` | 组件读写 |
| `components_of(id)` | 实体现有组件名（有序） |
| `get_field(id, "Position.x") / set_field(id, path, value)` | 字段路径读写（不隐式建结构） |
| `query(&["Player","Sprite"])` | 拥有全部指定组件的实体（槽位序，确定） |
| `snapshot() / restore(snap)` | 全量 JSON 快照/恢复 |
| `state_hash()` | FNV-1a 64 流式哈希（零中间分配，字节与 `fnv1a_64(snapshot)` 一致） |

#### 空间关系（[src/spatial.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-ecs/src/spatial.rs)）

为"AI 所见"预计算自我中心关系（视觉模型不擅长从绝对坐标做空间推理）：

```rust
pub fn relate(focal: Placement, target: Placement) -> RelativeSpatial;  // 纯函数
pub fn relate_in_world(world, focal, target) -> RelativeSpatial;        // 补视线遮挡
pub fn ascii_map(world, focal, opts: &AsciiMapOpts) -> AsciiMap;        // 焦点中心 ASCII 图
```

`RelativeSpatial { direction, distance, same_row, same_col, adjacent, blocked }`，`Direction` 八方位 + `Coincident`。

#### 哈希（[src/hash.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-ecs/src/hash.rs)）

```rust
pub fn fnv1a_64(bytes: &[u8]) -> u64;        // 一次性
pub struct Fnv1aWriter { ... }               // 流式（impl io::Write），可喂 serde_json::to_writer
```

自实现而非 `std::DefaultHasher`：std 不承诺跨版本稳定，而状态哈希要写进录像做重放校验，必须跨平台、跨版本永远一致。

#### 帧间差量（[src/delta.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-ecs/src/delta.rs)）

```rust
pub fn scene_delta(prev: &Value, cur: &Value) -> Value;  // {appeared, disappeared, changed}
```

对两份 describe 输出按 id 配对比较，供 `render/describe` 的 `changes` 字段用。

---

### 4.2 vitric-data — 声明式数据层

**职责**：项目格式、组件 schema、场景实例化、序列/主题/动画定义、结构化校验报告。是引擎的"心脏"。

源：[crates/vitric-data/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/lib.rs)

#### Project / ProjectManifest（[src/project.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/project.rs)）

```rust
pub struct ProjectManifest {
    pub name: String,
    pub schema: String,                       // schema 文件相对路径
    pub entry: String,                        // 启动场景，必须在 scenes 列表里
    pub scenes: Vec<String>,
    pub rules: Vec<String>,
    pub scripts: Vec<String>,
    pub sequences: Vec<String>,
    pub animations: Option<String>,
    pub themes: Vec<String>,
    pub font: Option<String>,                 // TTF 字体路径
    pub budgets: Budgets,
    pub gates: Option<Gates>,
    pub seed: u64,                            // 默认 0
}

pub struct Project {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub schema: Schema,
    pub scenes: BTreeMap<String, Scene>,
    pub rules: Vec<(String, Value)>,
    pub scripts: Vec<(String, String)>,
    pub sequences: BTreeMap<String, Sequence>,
    pub animations: BTreeMap<String, Clip>,
    pub themes: BTreeMap<String, Theme>,
}

impl Project {
    pub fn load(root: impl AsRef<Path>) -> Result<Project, ValidationReport>;  // IO/解析/校验一次报全
    pub fn entry_scene(&self) -> &Scene;
}
```

#### Schema 系统（[src/schema.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/schema.rs)）

```rust
pub enum FieldType { Number, Int, Bool, Text, Vec2, Entity, Enum(Vec<String>), List(Box<FieldType>) }

pub struct FieldDef { pub ty: FieldType, pub default: Option<Value>, pub required: bool, pub min: Option<f64>, pub max: Option<f64> }

pub struct ComponentSchema { pub name: String, pub fields: BTreeMap<String, FieldDef> }
impl ComponentSchema {
    pub fn normalize(&self, value: &Value, path: &str, report: &mut ValidationReport) -> Value;
    // 校验 + 归一化：填默认值、查未知字段(VD003)、查类型和范围、number 一律存浮点形态
}

pub struct Schema { pub components: BTreeMap<String, ComponentSchema> }
impl Schema {
    pub fn parse(doc: &Value, file: &str) -> Result<Schema, ValidationReport>;
    pub fn component(&self, name: &str) -> Option<&ComponentSchema>;
}
```

`canonicalize` 保证同一值在场景 JSON / JS 往返 / 规则写入后表示唯一，状态哈希不受干扰。

#### Scene 与实例化（[src/scene.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/scene.rs)）

```rust
pub struct Scene { pub file: String, pub doc: Value }
impl Scene { pub fn parse(doc: Value, file: &str, schema: &Schema) -> Result<Scene, ValidationReport>; }

pub fn instantiate_scene(scene: &Scene, schema: &Schema, world: &mut World) -> Result<(), ValidationReport>;
// 两遍走：先建实体（命名/匿名），再填组件（entity 字段名换运行时句柄）
```

#### Sequence / SeqStep（[src/sequence.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/sequence.rs)）

通用时间轴原语（对标 Unity Timeline / Godot AnimationPlayer）：

```rust
pub const SEQ_ACTION_KINDS: &[&str] = &["tween", "set", "spawn", "despawn", "emit", "sound", "wait"];

pub struct SeqStep { pub at: u64, pub kind: String, pub action: Value }  // at 单调不减
pub struct Sequence { pub id: String, pub file: String, pub steps: Vec<SeqStep> }
```

#### Theme / Clip / Gates（[src/project.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/project.rs)、[src/theme.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/theme.rs)）

```rust
pub struct Clip { pub frames: Vec<String>, pub fps: u32, pub looping: bool }  // 动画片段

pub struct Theme { pub name, colors, font_size, padding, margin, button: BTreeMap<String, ButtonStyle> }  // 装配期常量，不进世界状态
pub struct ButtonStyle { pub bg: String, pub text: String }

pub struct Budgets { pub max_entities: u64, pub max_events_per_tick: u64 }  // 0=不限

pub struct PlaythroughGate { pub recording: String, pub must_emit: String }  // 默认 must_emit="game-won"
pub struct PlaytestGate { pub sessions, max_ticks, strategy, horizon, beam, seed_recording, /* 断言字段 */ }
pub struct Gates { pub playthroughs: Vec<PlaythroughGate>, pub assertions: Option<String>, pub check: bool, pub max_ticks: Option<u64>, pub playtest: Option<PlaytestGate> }
```

#### 结构化校验报告（[src/error.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-data/src/error.rs)）

```rust
pub struct ValidationError { pub code: &'static str, pub path: String, pub message: String, pub hint: String }
pub struct ValidationReport { pub errors: Vec<ValidationError> }
impl ValidationReport { pub fn ok(&self) -> bool; pub fn push(...); pub fn merge(...); }
```

错误码命名空间：vitric-data 用 `VDxxx`（VD001-VD084），vitric-rules 用 `VRxxx`（VR001-VR011）。错误格式 `[CODE] path: message（hint）`。

---

### 4.3 vitric-rules — 当 X 则 Y 规则引擎

**职责**：玩法的正门。声明式规则，刻意不图灵完备——条件只有比较和与，没有循环没有变量。

源：[crates/vitric-rules/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-rules/src/lib.rs)、[engine.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-rules/src/engine.rs)、[model.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-rules/src/model.rs)

#### 核心数据结构

```rust
pub struct Event { pub name: String, pub data: Map<String, Value> }  // 内建: input / collision

pub enum Trigger {
    Tick,
    Event { name: String, filter: Map<String, Value>, between: Option<(String, String)> },
}

pub struct Rule {
    pub id: String,
    pub trigger: Trigger,
    pub each: Option<Vec<String>>,                // 配 Tick：对每个有这些组件的实体跑一次，绑 self
    pub conditions: Vec<(String, String, Value)>, // [左, 操作符, 右]
    pub actions: Vec<Value>,
}

pub struct RuleSet { pub rules: Vec<Rule> }
impl RuleSet { pub fn parse(doc: &Value, file: &str) -> Result<RuleSet, ValidationReport>; }

pub struct InputAction { pub action: String, pub phases: Vec<String> }
pub fn input_actions(rules: &RuleSet) -> Vec<InputAction>;  // 自省可注入动作词汇
```

#### Engine（[src/engine.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-rules/src/engine.rs)）

```rust
#[derive(Clone)]
pub struct Engine { pub rules: RuleSet, pub schema: Schema }  // 无内部状态，Clone 为 playtest 绕借用冲突

impl Engine {
    pub fn new(rules: RuleSet, schema: Schema) -> Engine;
    pub fn check(&self, world: &World, conditions: &[(String, String, Value)]) -> Result<bool, RuleError>;  // 断言用
    pub fn process_tick(&self, world: &mut World, inbox: Vec<Event>) -> Result<TickOutput, RuleError>;
}

pub struct ScriptCall { pub function: String, pub args: Value, pub self_entity: Option<EntityId> }
pub struct TickOutput { pub calls: Vec<ScriptCall>, pub fired: Vec<String>, pub emitted: Vec<Event> }
```

`process_tick` 流程：先跑所有 `Tick` 规则，再 FIFO 消化事件队列（含级联）。

#### 级联保护

```rust
const MAX_CASCADE_DEPTH: usize = 8;
```

事件队列元素 `(event, depth, chain)`；`emit` 动作把新事件 push 回队列时 `depth+1` 并把 `"<rule_id>→<event_name>"` 追加进 chain。超深报 `RuleError::CascadeOverflow`，错误信息展示完整触发链。

#### 规则语法

- **`on`**：`"tick"` 或 `{"event": "名字", "filter": {...}, "between": ["CompA","CompB"]}`（between 只配 collision）
- **`if`**：`[路径, 操作符, 值]` 三元组数组，全部成立才执行
- **运算符**（`OPS`）：`==, !=, <, <=, >, >=, exists, !exists`
- **`do`**：动作对象数组，动作集（`ACTION_KINDS`）：`set, add, spawn, despawn, emit, call`
- **路径语法**：`self.组件.字段` / `other.…` / `@实体名.…` / `event.字段`；以这些前缀开头按引用解析，否则字面量；`{"format":"SCORE {}", "args":[路径...]}` 字符串模板

规则 JSON 示例：

```json
{
  "id": "collect-coin",
  "on": {"event": "collision", "between": ["Player", "Coin"]},
  "if": [["other.Coin.value", ">", 0]],
  "do": [
    {"add": "self.Score.value", "by": "other.Coin.value"},
    {"despawn": "other"},
    {"emit": "coin-collected", "data": {"who": "self"}}
  ]
}
```

#### RuleError

```rust
pub enum RuleError {
    CascadeOverflow { depth: usize, chain: Vec<String> },
    Exec { rule: String, at: String, message: String },  // at 如 "if/0" "do/2" "check/1"
}
```

---

### 4.4 vitric-script — 戴安全带的 JS 脚本

**职责**：规则写不动的 20% 复杂逻辑落这里。QuickJS 嵌入，强制读写声明，确定性 RNG 共流，热重载。

源：[crates/vitric-script/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-script/src/lib.rs)、[prelude.js](file:///Users/leolele/Documents/leo/vitric/crates/vitric-script/src/prelude.js)

#### 关键类型

```rust
pub struct SystemDecl { pub name: String, pub query: Vec<String>, pub writes: Vec<String> }  // writes ⊆ query

pub struct ScriptOutput { pub events: Vec<Event> }  // 脚本 emit 的事件

pub enum ScriptError {
    Load { file, message },
    Runtime { location, message },
    UndeclaredWrite { system, entity, component },  // 越权写
    Op { location, message },
}

pub struct ScriptEngine { /* QuickJS Runtime + Context + schema + 注册表 */ }
```

#### 核心方法

```rust
impl ScriptEngine {
    pub fn new(schema: Schema) -> Result<ScriptEngine, ScriptError>;
    pub fn load(&mut self, file: &str, source: &str) -> Result<(), ScriptError>;       // 按顺序求值
    pub fn reload(&mut self, sources: Vec<(String,String)>) -> Result<(), ScriptError>; // 整体重建，世界不动
    pub fn run_systems(&mut self, world: &mut World, rng: &mut Pcg32, tick: u64) -> Result<ScriptOutput, ScriptError>;
    pub fn call_fn(&mut self, function: &str, args: &Value, self_entity: Option<EntityId>, world: &mut World, rng: &mut Pcg32, tick: u64) -> Result<ScriptOutput, ScriptError>;
}
```

#### 安全带机制

1. **读写声明强制**：系统注册时声明 `query`（读哪些组件）和 `writes`（写哪些组件，⊆ query）。返回的 entities 数组不许夹带 query 之外的组件键；只读系统改了字段值（按数值语义比，避免 `0.0→0` 假阳性）且未声明 writes → `UndeclaredWrite` 报错。
2. **commit-on-success**：写回两遍走，先全量校验攒变更，确认整批合法才落地。第 N 个实体非法时世界保持原样，不留半改状态。
3. **随机数共流**：`Math.random` / `Date.now` / `new Date()` 被毒化并指路 `ctx.random()` / `ctx.tick`。`ctx.random()` 推进的是 Rust 侧同一条 PCG32 流（JS 侧 BigInt 实现同算法），测试锁死"JS 抽两个后 Rust 续抽接上同一条流"。
4. **f64 精度无损**：QuickJS 的 `JSON.stringify` 不是最短往返，非整数浮点跨边界走 IEEE754 位串 `{"$f64":"<16hex>"}` 还原（[prelude.js](file:///Users/leolele/Documents/leo/vitric/crates/vitric-script/src/prelude.js) 的 `__numStr`）。
5. **数据进出全 JSON**：脚本看到的实体和场景文件、控制面是同一种语言。`ctx.spawn / ctx.despawn / ctx.emit / ctx.setField / ctx.ask` 是操作原语。

#### 脚本示例

```js
vitric.system("friction", {query: ["Velocity"], writes: ["Velocity"]}, (entities, ctx) => {
    for (const e of entities) { e.Velocity.x *= 0.5; }
});

vitric.fn("explode", (args, ctx) => {
    for (let i = 0; i < args.count; i++) ctx.spawn({Coin: {value: 1}});
    ctx.emit("exploded", {at: args.where});
});
```

---

### 4.5 vitric-sim — 确定性模拟核心

**职责**：固定步长、种子随机、输入录制与重放、快照/恢复、内建物理系统（重力/移动/碰撞/相机/抖动/粒子/补间）。

源：[crates/vitric-sim/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/lib.rs)、[sim.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/sim.rs)、[pcg.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/pcg.rs)、[recording.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/recording.rs)、[tween.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/tween.rs)

#### 常量与 GameLogic trait

```rust
pub const TICKS_PER_SECOND: u64 = 60;
pub const DT: f64 = 1.0 / 60.0;

pub trait GameLogic {
    fn on_tick(&mut self, world: &mut World, events: Vec<Event>, rng: &mut Pcg32, tick: u64) -> Result<(), String>;
    fn drain_observed(&mut self) -> Vec<Event> { Vec::new() }
    fn reload(&mut self) -> Result<Value, String> { Err("不支持热重载".into()) }
    fn snapshot_state(&self) -> Value { Value::Null }
    fn restore_state(&mut self, _snap: &Value) -> Result<(), String> { Ok(()) }
    fn available_actions(&self) -> Vec<(String, Vec<String>)> { Vec::new() }
}
```

`Runtime`（vitric-cli）实现此 trait 把规则/脚本/动画/序列/UI 系统装配进来。

#### Sim 结构体与核心方法（[src/sim.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/sim.rs)）

```rust
pub struct Sim {
    pub world: World,
    pub rng: Pcg32,
    pub tick: u64,
    seed: u64,
    pending_inputs: Vec<(String, String)>,
    pending_replies: Vec<(String, Value)>,
    recorder: Option<Recording>,
}

impl Sim {
    pub fn new(seed: u64) -> Sim;
    pub fn inject_input(&mut self, action: &str, phase: &str);              // phase: "pressed"|"released"
    pub fn inject_reply(&mut self, name: &str, data: Value);                // LLM 等异步内容进模拟的唯一正路
    pub fn start_recording(&mut self) / is_recording(&self) / stop_recording(&mut self) -> Option<Recording>;
    pub fn step(&mut self, logic: &mut dyn GameLogic) -> Result<StepReport, SimError>;
    pub fn replay(&mut self, rec: &Recording, logic: &mut dyn GameLogic) -> Result<(), SimError>;
    pub fn replay_observed(&mut self, rec, logic, observe: FnMut) -> Result<(), SimError>;
    pub fn snapshot(&self, logic: &dyn GameLogic) -> Value;
    pub fn restore(&mut self, snap: &Value, logic: &mut dyn GameLogic) -> Result<(), String>;
}

pub struct StepReport { pub tick: u64, pub events: Vec<Event> }
```

#### step 流水线（顺序固定 = 确定性）

1. tick==0 发 `start` 事件
2. `pending_inputs` → `input` 事件（进录像）
3. `pending_replies` → 同名事件（输入在前、回复在后）
4. `apply_gravity`：Body 实体 `Velocity.y += gravity * DT`
5. `integrate_motion`：Position += Velocity·DT，带 Body+Collider 的实体被 Solid 挡停（轴分离 + 贴边 snap，`grounded` 写回 Body）
6. `follow_camera`：Camera.follow 指向实体的 lerp 跟随
7. `decay_shake`：Shake.amplitude 乘 decay
8. `age_particles`：Particle.ttl 减 1，归 0 销毁
9. `advance_tweens`：补间解析式插值
10. `detect_collisions`：AABB 重叠 → `collision` 事件
11. `logic.on_tick(world, events, rng, tick)` 消化全部事件
12. `tick += 1`，每 60 tick 写一个 `(tick, state_hash)` 校验点

#### Pcg32（[src/pcg.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/pcg.rs)）

PCG-XSH-RR 自实现（不依赖 `rand` crate，状态可序列化）：

```rust
pub struct Pcg32 { state: u64, inc: u64 }
impl Pcg32 {
    pub fn new(seed: u64) -> Pcg32;
    pub fn next_u32(&mut self) -> u32;
    pub fn next_f64(&mut self) -> f64;          // [0,1) 53 位精度
    pub fn range_i64(&mut self, min: i64, max: i64) -> i64;  // [min,max] 闭区间
}
```

#### 录像格式（[src/recording.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/recording.rs)）

```rust
pub struct InputRecord { pub tick: u64, pub action: String, pub phase: String }
pub struct ReplyRecord { pub tick: u64, pub name: String, pub data: Value }

pub struct Recording {
    pub seed: u64,
    pub inputs: Vec<InputRecord>,
    pub replies: Vec<ReplyRecord>,       // #[serde(default)] 兼容旧录像
    pub checkpoints: Vec<(u64, u64)>,    // 周期性 (tick, state_hash)
    pub ticks: u64,
    pub final_hash: u64,
}
```

重放时回复从录像原样注入、**永不重新调网络**，所以带 LLM 内容的录像离线逐位重放一致。

#### 缓动（[src/tween.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/tween.rs)）

```rust
pub enum Ease { Linear, In, Out, InOut, OutBack }  // OutBack 约 10% 过冲
pub const EASE_NAMES: &[&str] = &["linear","ease-in","ease-out","ease-in-out","ease-out-back"];
pub fn tween_value(from: f64, to: f64, ease: Ease, elapsed: u64, duration: u64) -> f64;
```

铁律：**解析式纯函数，禁累加积分**——浮点累加误差会让快照回退后续播分歧。

---

### 4.6 vitric-render — 2D 光栅化与语义观察

**职责**：CPU 光栅化是确定性真相源（无 GPU/窗口即可截图，字节逐位可断言）；wgpu GPU 路径镜像它用于窗口呈现；`describe_world` 给 AI 读的语义视图。

源：[crates/vitric-render/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-render/src/lib.rs)、[assets.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-render/src/assets.rs)、[font.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-render/src/font.rs)、[ui.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-render/src/ui.rs)、[ui_interact.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-render/src/ui_interact.rs)

#### 组件约定（渲染层读取）

| 组件 | 作用 |
|------|------|
| `Sprite {w,h,color,rot,image}` | 有它才画；rot 绕中心旋转，世界空间逆时针为正 |
| `Position {x,y}` | 世界坐标，y 向上 |
| `Camera {x,y,scale}` | 取第一个；无则原点、8 像素/单位 |
| `Shake {amplitude,decay}` | 屏幕抖动（仅画面，describe/pick 读不抖的相机） |
| `Text {content,size,color}` | 屏上文字；无 font 走 8x8 点阵，清单挂 font 走 TTF 矢量（含 CJK） |
| `Ambient {color,shadows}` | 环境光总开关；无 Ambient 实体 = 不跑光照 |
| `Light {radius,color,intensity,kind,angle,dir}` | point/spot/directional 三种，合计上限 64 盏 |
| `Bloom {threshold,strength}` | 泛光后效总开关 |
| `Emitter` | 粒子发射器（纯渲染层产物，不进模拟状态/哈希/存档） |
| `Solid` + `Collider` | 投影遮光体（shadows:true 时） |

#### 关键公开函数

```rust
pub fn render_world(world: &World, assets: &Assets, width: u32, height: u32, tick: u64) -> Result<Vec<u8>, String>;
pub fn screenshot_png(world, assets, width, height, tick, path: Option<&str>) -> Result<Value, String>;
pub fn describe_world(world, width, height) -> Result<Value, String>;
pub fn describe_world_with_assets(world, assets, width, height, actions, last_describe) -> Result<Value, String>;
pub fn pick_world(world, assets, x, y, width, height, tick) -> Result<Option<Value>, String>;
pub fn screen_to_world(...) -> (f64, f64);
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String>;

// 光照/投影
pub fn collect_lights(world) -> Result<Vec<LightSource>, String>;
pub fn collect_occluders(world) -> Result<Vec<Occluder>, String>;
pub fn build_shadow_boxes(...) -> ShadowBoxes;       // 相邻遮光体合并
pub fn cull_shadow_boxes(...) -> Vec<u32>;           // 按灯盘剔除

// 粒子（纯函数，无跨帧状态）
pub fn emitter_particles(e: &EmitterSource, tick: u64) -> Vec<ParticleDot>;
pub fn emitter_seed(id: EntityId) -> u64;            // SplitMix64 散列，与模拟 RNG 流无关

// UI 布局
pub fn solve_layout(world, vw, vh) -> Result<Vec<(EntityId, UiRect)>, String>;
pub fn layout_input_hash(world, vw, vh) -> u64;       // 脏标记
pub fn has_ui(world) -> bool;

// UI 交互
pub fn navigate(focusable: &[Focusable], cur: usize, dir: Dir) -> usize;
pub fn press_scale(press_t: i64) -> f64 / press_modulate(press_t) -> [f64;3] / modulate_rgb(...) -> [f64;3];
```

#### 光照公式（CPU 与 GPU 必须一致）

```
lit = min(ambient + Σ 各灯贡献, 1.5)
out = min(scene · lit, 1.0)
point:       color·intensity·(1 - d/r)²
spot:        color·intensity·(1 - d/r)²·t²,  t = clamp(1 - Δθ/(angle/2), 0, 1)
directional: color·intensity
```

法线贴图零配置命名配对：`hero.png` + `hero_n.png` 自动启用，各灯贡献额外乘 `max(dot(N,L),0)`。

#### 语义 describe 输出

`describe_world_with_assets` 输出 JSON：`{visible, offscreen, actions, ascii_map, changes}`。实体带 ego-centric 空间关系（direction/distance/line-of-sight）；`changes` 是 `scene_delta` 算的帧间差量。

---

### 4.7 vitric-control — AI 控制面

**职责**：引擎进程内建的调试端口。HTTP JSON-RPC，让 Agent 看/动/控时间/测。架构：HTTP 服务线程只做传输，命令由游戏主循环在**帧边界**统一执行——控制面永远不破坏确定性。

源：[crates/vitric-control/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-control/src/lib.rs)、[server.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-control/src/server.rs)、[dispatcher.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-control/src/dispatcher.rs)、[saves.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-control/src/saves.rs)

#### 协议

`POST /rpc` 单对象 `{"method":"...","params":{...}}`，响应 `{"ok":true,"result":...}` 或 `{"ok":false,"error":"..."}`。只绑 `127.0.0.1`。

#### 关键类型

```rust
pub struct ControlServer { pub port: u16, ... }
impl ControlServer { pub fn start(port: u16) -> Result<ControlServer, String>; pub fn drain(&self) -> Vec<PendingRequest>; }

pub struct Dispatcher { /* assets / assertions / events 环形缓冲 / selection / saves */ pub ctl: LoopCtl }
impl Dispatcher {
    pub fn new(schema: Schema) -> Dispatcher;
    pub fn load_assets(&mut self, dir: &Path) / load_font(&mut self, path: &Path) -> Result<...>;
    pub fn set_budgets(&mut self, budgets: Budgets);
    pub fn set_save_store(&mut self, store: SaveStore);
    pub fn record_events(&mut self, tick: u64, events: &[Event]);
    pub fn check_assertions(&mut self, sim: &Sim) -> Vec<Value>;  // 含预算检查
    pub fn handle_save_load_events(&mut self, observed, sim, logic) -> Vec<Value>;
    pub fn handle(&mut self, request: &Value, sim: &mut Sim, logic: &mut dyn GameLogic) -> Value;
}

pub struct LoopCtl { pub paused: bool, pub speed: f64, pub quit: bool }

pub struct SaveStore { ... }  // 槽位 [a-z0-9-]{1,32}，原子写，版本校验
impl SaveStore { pub fn new(project_root, project) -> Self; pub fn write(slot, sim, logic) / read(slot) / list() -> Result<...>; }

pub fn inject_click(sim, x, y, button) -> Result<Value, String>;      // 世界坐标点击，走回复通道可录像
pub fn inject_ui_click(sim, nx, ny, button) -> Result<Value, String>;  // UI 归一化坐标 (0..1)
```

#### 全部 RPC method

录像期间拒绝的 method：`world/set, world/spawn, world/despawn, project/reload, sim/restore, save/load`。

| 分类 | method | 功能 |
|------|--------|------|
| 状态 | `ping` | `{tick, paused, speed}` |
| 看 | `world/entities` | 按 components 过滤列实体 |
| 看 | `world/get` | 取单个实体（`e3v1` 或 `@名字`） |
| 动 | `world/set` | 改字段（过 schema normalize） |
| 动 | `world/spawn` | 创建实体（过 schema normalize） |
| 动 | `world/despawn` | 销毁实体 |
| 输入 | `input/inject` | 注入 input 事件（pressed/released） |
| 输入 | `input/click` | 世界坐标点击（拾取 + 注入事件） |
| 输入 | `input/ui-click` | UI 归一化坐标点击 |
| 时间 | `sim/pause` / `sim/resume` / `sim/speed` / `sim/quit` | 时间控制 |
| 时间 | `sim/step` | 暂停时单步 N tick（每 tick 跑断言） |
| 热重载 | `project/reload` | 热重载规则/脚本/素材 |
| 快照 | `sim/snapshot` / `sim/restore` / `sim/hash` | 快照/恢复/哈希 |
| 存档 | `save/write` / `save/load` / `save/list` | 玩家存档 |
| 观察 | `render/describe` | 语义描述（带 actions + changes 增量） |
| 观察 | `render/screenshot` | PNG 截图（可写文件/inline base64） |
| 性能 | `perf/stats` | tick/实体数/事件数/素材/预算 |
| 检查器 | `inspect/selection` / `inspect/select` | 选中实体 |
| 事件 | `events/recent` | 自 since tick 起的事件流 |
| 断言 | `assert/add` / `assert/remove` / `assert/list` / `assert/failures` | 管理断言 |

---

### 4.8 vitric-playtest — Agent 集群试玩

**职责**：进程内地基，把一局可重放的自动试玩拼起来——scene view（代理所见）+ 策略 + session 循环 + 种子探索 + 报告聚合 + LLM 档 + HTML 渲染。

源：[crates/vitric-playtest/src/lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/lib.rs) 及同目录各模块。

#### SceneView（[src/scene_view.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/scene_view.rs)）

```rust
pub struct Action { pub action: String, pub phase: String }
pub enum Outcome { Win, Lose, Timeout }

pub struct TerminalSpec { pub win_events: Vec<String>, pub lose_events: Vec<String>, pub ending_prefixes: Vec<String> }
impl TerminalSpec { pub fn default() -> Self; pub fn apply_override(&self, ovr) -> Self; pub fn with_manifest_must_emit<I,S>(self, events: I) -> Self; pub fn classify(&self, event_name: &str) -> Option<Outcome>; }

pub struct SceneView { pub observation: Value, pub actions: Vec<Action>, pub done: Option<Outcome> }
impl SceneView { pub fn derive(world, engine, terminal) -> SceneView; pub fn derive_with_config(world, engine, terminal, config) -> SceneView; }
```

投影时剔除装饰组件（`Sprite/Particle/Emitter/Bloom/Ambient/Anim/Camera`）；焦点取第一个 `Camera.follow` 指名实体；有焦点时附 `ascii_map`；actions 来自 `input_actions` × {pressed,released}。

#### Strategy（[src/strategy.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/strategy.rs)）

```rust
pub trait Strategy { fn choose(&mut self, view: &SceneView) -> Option<Action>; fn drain_notes(&mut self) -> Vec<PlaytestNote> { Vec::new() } }

pub struct RandomStrategy { ... }     // [0,n] 含"不操作"槽
pub struct GreedyStrategy { ... }     // 有 goal: 改善则重复，否则随机换；无 goal 退化随机
pub struct CoverageStrategy { ... }   // 轮转每个动作至少一次
pub struct EconomyStrategy { ... }    // 连按一个动作 24-64 次再换（找经济崩）
pub struct ScriptedStrategy { ... }   // 按脚本注入，可接 then_explore 发散
pub struct LlmStrategy { ... }        // LLM 拟人玩 + 吐定性 note
```

所有策略用独立 `Pcg32` 播种，不碰 `sim.rng`，同 seed 同序列。

#### Session（[src/session.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/session.rs)）

```rust
pub struct SessionConfig { pub max_ticks: u64, pub seed: u64, pub terminal: TerminalSpec, pub playtest: PlaytestConfig, pub seed_replies: Vec<ReplyRecord> }

pub struct SessionResult { pub outcome: Outcome, pub ticks: u64, pub recording: Recording, pub state_trace: Vec<u64>, pub fired_events: Vec<String>, pub numeric_summary: BTreeMap<String, NumericStat>, pub notes: Vec<PlaytestNote> }

pub struct LookaheadConfig { pub depth: u64, pub beam_width: usize }  // 默认 depth=8, beam=4

pub fn run_session(sim, logic, engine, strategy, cfg) -> Result<SessionResult, String>;
pub fn run_session_lookahead(sim, logic, engine, cfg, look) -> Result<SessionResult, String>;
```

`run_session_lookahead` 是定向束搜索滚动规划器（MPC），每真 tick 重新规划只执行第一步；投机全程在 `snapshot/restore` 之间，手工攒 Recording（restore 会清 sim recorder）。

#### 种子探索（[src/seed.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/seed.rs)）

```rust
pub enum PerturbOp { Baseline, Drop, Swap, Substitute, Truncate }
pub struct Perturbation { pub op: PerturbOp, pub script: Vec<(u64, Action)>, pub truncate_at: Option<u64> }
pub fn perturb_plan(seed: &Recording, n: usize, rng_seed: u64) -> Vec<Perturbation>;
// 第 0 条=基线原种子；1..n 条按 Drop/Swap/Substitute/Truncate 轮换，各扰动独立不叠加
```

#### Swarm（[src/swarm.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/swarm.rs)）

```rust
pub enum StrategyKind { Random, Greedy, Coverage, Economy, Scripted, Llm, Lookahead { depth: u64 } }
pub struct SessionSpec { pub strategy_kind: StrategyKind, pub seed: u64, pub max_ticks: u64, pub terminal: TerminalSpec }
pub struct LabeledResult { pub spec: SessionSpec, pub result: SessionResult }

pub const DEFAULT_SWARM_LOOKAHEAD_DEPTH: u64 = 12;
pub const DEFAULT_SWARM_LOOKAHEAD_BEAM: usize = 4;

pub fn default_plan(sessions, seed, max_ticks, terminal, has_goal) -> Vec<SessionSpec>;  // 四策略轮换×递增 seed；has_goal 末尾换 lookahead
pub fn run_swarm(factory, plan, threads) -> Result<Vec<LabeledResult>, String>;
pub fn run_swarm_with_config(factory, plan, config, threads) -> Result<Vec<LabeledResult>, String>;
pub fn run_seed_swarm(factory, plan, seed_replies, max_ticks, terminal, explore_seed, threads) -> Result<Vec<LabeledResult>, String>;
pub fn run_llm_sessions(factory, client, count, goal, base_seed, max_ticks, terminal) -> Result<Vec<LabeledResult>, String>;
```

**避 QuickJS 非 Send 坑**：`factory: Fn() -> Result<(Sim, R, Engine), String>`，每条 spec 在调用线程内自己 boot 一份运行时，运行时绝不跨线程边界。

#### Report 聚合（[src/report.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/report.rs)）

```rust
pub struct Report {
    pub sessions: usize,
    pub outcome_distribution: OutcomeDistribution,     // win/lose/timeout + win_rate
    pub reachability: Reachability,                    // reached_events + unbeatable_by_swarm
    pub ending_coverage: Option<EndingCoverage>,       // declared/reached/unreachable endings
    pub stuck_clusters: Vec<StuckCluster>,             // 软锁候选（frozen tail hash 聚类）
    pub pacing: Pacing,                                // 通关 ticks 直方图
    pub inert_actions: Vec<String>,                    // 惰性动作候选
    pub dominant_strategy: DominantStrategy,           // 含 dominant_action（一招鲜）
    pub numeric_breakage: NumericBreakage,             // runaway/collapse/non_finite
    pub qualitative_notes: QualitativeNotes,           // LLM 定性 note（按 kind 聚类）
    pub summary: String,
}
impl Report { pub fn externalize_recordings(&mut self, report_dir: &Path) -> Result<usize, String>; }

pub fn aggregate(results) -> Report;
pub fn aggregate_with_endings(results, engine, terminal) -> Report;
pub fn aggregate_with_endings_and_declared(results, engine, terminal, manifest_declared) -> Report;
```

启发式候选（软锁/惰性动作/数值崩/LLM note）一律诚实标「候选，待人复核」。

#### PlaytestConfig（[src/config.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/config.rs)）

```rust
pub struct PlaytestConfig { pub observation: ObservationConfig, pub goal: Option<GoalSpec>, pub terminal: Option<TerminalOverride> }
// 从项目根 playtest.json 加载；不存在返 None（默认 config 让所有行为逐字节一致）
impl PlaytestConfig { pub fn load(project_dir: &Path) -> Result<Option<PlaytestConfig>, String>; }
```

派生量（`DerivedSpec`）支持 `Distance/alias/Count`，刻意不做 DSL；`goal.quantity` 必须在 `derived` 里声明过。

#### LLM 档（[src/llm_agent.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/llm_agent.rs)）

```rust
pub trait LlmClient: Send + Sync { fn complete(&self, prompt: &str) -> Result<String, String>; }
pub struct LlmStrategy { ... }  // 选的输入走和廉价策略档同一套录像通道 → 可逐位重放
```

解析失败/动作非法/client 报错**绝不 panic**，记一条 note + 退化为 `None`。note kind 归一为 `clarity/continuity/choice/other`。

#### HTML 报告（[src/html.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-playtest/src/html.rs)）

```rust
pub fn report_to_html(report: &Report, project_name: &str) -> String;
```

自包含（CSS 内联、手画 SVG，无外部 CDN/JS），离线能开、确定性（同一份 Report 必出同一页）。

---

### 4.9 vitric-cli — 命令行与运行时装配

**职责**：顶层装配点。`main.rs` 分发子命令；`runtime.rs` 把项目数据+规则+脚本装配成能跑的 `Runtime`（实现 `GameLogic`）；其余子模块负责窗口/音频/GPU/打包/门禁/试玩 LLM 适配等。

源：[crates/vitric-cli/src/main.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/main.rs)、[runtime.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/runtime.rs)、[lib.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/lib.rs)

#### Runtime（装配体，[src/runtime.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/runtime.rs)）

```rust
pub const UI_REFERENCE_VIEWPORT: (u32, u32) = (1920, 1080);  // UI 布局参照视口，与渲染分辨率解耦

pub struct Runtime {
    pub rules: Engine,
    pub scripts: ScriptEngine,
    pub animations: BTreeMap<String, Clip>,
    pub sequences: BTreeMap<String, Sequence>,
    pub themes: BTreeMap<String, Theme>,
    schema: Schema,
    scenes: BTreeMap<String, Scene>,   // 装配期预载，场景切换从这里取（不读磁盘）
    root: Option<PathBuf>,
    carryover: Vec<Event>,             // 脚本上一 tick 发的、本 tick 进规则的事件
    observed: Vec<Event>,              // 本 tick 全部 emit 的事件副本，控制面观测用
}

impl Runtime {
    pub fn build(project: &Project) -> Result<Runtime, String>;  // 规则语义校验 + 脚本求值
    pub fn boot(dir: &Path) -> Result<(Sim, Runtime), String>;   // load + build + 实例化入口场景
}

impl GameLogic for Runtime {
    fn on_tick(&mut self, world, events, rng, tick) -> Result<(), String>;  // 见第 6 节流水线
    fn drain_observed(&mut self) -> Vec<Event>;
    fn reload(&mut self) -> Result<Value, String>;       // 热重载规则+脚本，世界不动，失败保持旧逻辑
    fn snapshot_state(&self) -> Value;                   // carryover 进快照
    fn restore_state(&mut self, snap) -> Result<(), String>;
    fn available_actions(&self) -> Vec<(String, Vec<String>)>;
}
```

`check` 函数（[src/runtime.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/runtime.rs)）只验数据不开跑：项目加载 + 装配 + 实例化入口场景 + 全场景素材/动画/音效/贴图引用扫描 + 帧进口图集校验。

#### 子模块职责

| 模块 | 职责 | 关键函数 |
|------|------|----------|
| [gate.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/gate.rs) | 交付门禁：check + 通关录像重放 + 断言集 + 可选 playtest 门 | `pub fn run(dir) -> Result<(Value, bool), String>` |
| [bundle.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/bundle.rs) | 发行打包：gate PASS 后把项目附进引擎副本出自包含单文件 | `run / seal / open / extract_self / pack_archive / unpack_archive`；`MAGIC=b"VITRICPK"` |
| [balance.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/balance.rs) | 自动配平：二分搜索数值旋钮让通关率落进目标区间 | `run / KnobAddr::parse / read_knob / evaluate / search` |
| [assets_cmd.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/assets_cmd.rs) | 全项目 PNG 统一色板（中位切分量化） | `run / harmonize / extract_palette / quantize_with` |
| [frames.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/frames.rs) | 帧动画进口流水线：去重/裁边/图集/色板/BC7 | `run / collect_sequence / check_atlas_products` |
| [team.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/team.rs) | 多 agent 协同黑板（只读，永远退出 0） | `pub fn run(dir) -> Result<Value, String>` |
| [turf.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/turf.rs) | 地盘执法：改动越界即退出 1 | `pub fn run(args) -> Result<(Value, bool), String>` |
| [llm.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/llm.rs) | 运行时 LLM 客户端（OpenAI 兼容，单工作线程） | `Llm::from_env / handle_ask_events / pump_replies / complete_sync` |
| [window.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/window.rs) | 窗口呈现：CPU softbuffer + GPU wgpu | `enum Renderer { Cpu, Gpu }` / `WindowedGame::run` |
| [audio.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/audio.rs) | 音频（rodio）：play-sound / play-music / stop-music | `Audio::open / handle_sound_events` |
| [gpu.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/gpu.rs) | GPU wgpu 呈现路径（非真相源，截图永远走 CPU） | `GpuPresenter::new / present / gpu_probe` |
| [bc7.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/bc7.rs) | 纯 Rust BC7 mode 6 编码器（4×4=16 字节，带完整 alpha） | `encode_rgba8 / decode_block_mode6 / decode_to_rgba8` |
| [normals.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/normals.rs) | 法线贴图生成（程序化或 Ark Seedream 图生图） | `generate / AiConfig::from_env` |
| [playtest_llm.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/playtest_llm.rs) | playtest 的 LlmClient trait 真实现 | `PlaytestLlmClient::from_env` |

---

## 5. 数据语言

写 Vitric 游戏 = 写数据。项目目录结构（以 [examples/coin-run](file:///Users/leolele/Documents/leo/vitric/examples/coin-run) 为例）：

```
coin-run/
├── vitric.json          # 项目清单
├── schema.json          # 组件 schema 定义
├── scenes/main.json     # 实体摆放
├── rules/game.json      # 玩法规则
├── scripts/systems.js   # 复杂逻辑（可选）
├── animations.json      # 动画片段（可选）
├── sequences/intro.json # 时间轴（可选）
├── themes/dark.json     # 主题（可选）
├── assets/              # PNG 贴图（hero.png + hero_n.png 自动启用法线）
├── sounds/              # wav 音效
├── fonts/               # TTF 字体（可选，挂了走矢量文字）
├── qa/asserts.json      # 断言集（gate 用）
├── recordings/clear.json # 通关录像（gate 证书）
└── saves/               # 玩家存档（运行时生成）
```

### vitric.json 清单

```json
{
  "name": "coin-run",
  "schema": "schema.json",
  "entry": "scenes/main.json",
  "scenes": ["scenes/main.json"],
  "rules": ["rules/game.json"],
  "scripts": ["scripts/systems.js"],
  "seed": 42,
  "animations": "animations.json",
  "sequences": ["sequences/intro.json"],
  "themes": ["themes/dark.json"],
  "font": "fonts/myfont.ttf",
  "budgets": {"max_entities": 1000, "max_events_per_tick": 100},
  "gates": {
    "playthroughs": [{"recording": "recordings/clear.json", "must_emit": "game-won"}],
    "assertions": "qa/asserts.json",
    "check": true,
    "max_ticks": 100000,
    "playtest": {"sessions": 16, "max_ticks": 600, "require_clearable": true, "max_soft_locks": 0}
  }
}
```

### schema.json（[examples/coin-run/schema.json](file:///Users/leolele/Documents/leo/vitric/examples/coin-run/schema.json)）

```json
{
  "components": {
    "Position": {"fields": {"x": {"type":"number","default":0}, "y": {"type":"number","default":0}}},
    "Velocity": {"fields": {"x": {"type":"number","default":0}, "y": {"type":"number","default":0}}},
    "Collider": {"fields": {"w": {"type":"number","required":true,"min":0}, "h": {"type":"number","required":true,"min":0}}},
    "Player": {"fields": {}},
    "Coin": {"fields": {"value": {"type":"int","default":1,"min":1}}},
    "Sprite": {"fields": {"w":{"type":"number","default":1}, "h":{"type":"number","default":1}, "color":{"type":"text","default":"#ffffff"}, "image":{"type":"text","default":""}}}
  }
}
```

字段类型：`number / int / bool / text / vec2 / entity / enum[a|b] / list<...>`。

### 内建组件约定

| 约定 | 含义 |
|------|------|
| `Position` + `Velocity` | 移动 |
| `Position` + `Collider` | 碰撞 |
| `Position` + `Sprite` | 渲染 |
| `Camera` | 取景 |
| `Body` | 受重力 + 挡停（`grounded` 写回） |
| `Solid` | 挡身体 + 挡光（投影开启时） |
| `Persist` | 跨场景幸存（必须命名） |
| `Anim` | 动画播放 |

### 内建事件

`start`（tick 0 一次）、`input {action, phase}`、`collision {a, b}`、`anim-finished`、`tween-finished`、`scene-loaded`、`sequence-finished`、`ui-activate {id, action}`、`save-game` / `load-game`、`play-sound` / `play-music` / `stop-music`、`llm-ask` / `llm-reply` / `llm-error`、`load-scene {scene}`（场景切换约定事件）。

---

## 6. 运行时流水线

一个 tick 的执行顺序（固定，确定性的一部分）。源：[runtime.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/runtime.rs) `Runtime::on_tick` + [sim.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-sim/src/sim.rs) `Sim::step`。

```
Sim::step:
  1. tick==0 → 发 start 事件
  2. pending_inputs → input 事件（进录像）
  3. pending_replies → 同名事件（输入在前、回复在后）
  4. apply_gravity       (Body: Velocity.y += gravity·DT)
  5. integrate_motion    (Position += Velocity·DT, Solid 挡停, grounded 写回)
  6. follow_camera       (Camera.follow lerp 跟随)
  7. decay_shake         (Shake.amplitude *= decay)
  8. age_particles       (Particle.ttl -= 1, 归 0 销毁)
  9. advance_tweens      (补间解析式插值, 到期发 tween-finished)
  10. detect_collisions  (AABB 重叠 → collision 事件)
  11. logic.on_tick(world, events, rng, tick)  ← Runtime 接管
  12. tick += 1, 每 60 tick 写校验点

Runtime::on_tick (GameLogic):
  inbox = carryover + events
  1. rules.process_tick(world, inbox)          → 规则消化本 tick 事件
  2. 规则 call 动作 → scripts.call_fn(...)     → 规则调脚本函数
  3. scripts.run_systems(world, rng, tick)     → 脚本系统按注册序各跑一遍
  4. advance_animations(world, animations)     → 引擎独占 Sprite.image 写权
  4.5 advance_sequences(world, sequences, ...)  → 通用时间轴
  4.6 advance_ui_layout(world, viewport)       → UI 布局（脏标记 + 一趟树遍历）
  4.7 advance_ui_interaction(world, inbox, ...) → 焦点导航 + 点击激活
  4.8 apply_ui_theme(world, themes)            → Button.state 底色写进 Panel.color
  5. switch_scene (若本 tick 有 load-scene)    → 推倒重建, Persist 幸存
  carryover = 脚本/序列/UI emit 的事件（进下一 tick 规则）
```

脚本 emit 的事件不进本 tick 规则，进 `carryover` 交下一 tick——跨 tick 送达约定，确定且重放一致。

---

## 7. 控制面与 MCP

### 7.1 HTTP 控制面

引擎运行时内建 HTTP JSON-RPC 服务（[vitric-control](#47-vitric-control--ai-控制面)）。Agent 驱动示例：

```bash
rpc() { curl -s -X POST http://127.0.0.1:6173/rpc -d "$1"; echo; }
rpc '{"method":"sim/pause"}'
rpc '{"method":"input/inject","params":{"action":"right"}}'
rpc '{"method":"sim/step","params":{"ticks":60}}'
rpc '{"method":"world/get","params":{"entity":"@player"}}'
rpc '{"method":"render/describe"}'
rpc '{"method":"render/screenshot","params":{"path":"shot.png"}}'
rpc '{"method":"events/recent"}'
```

完整方法表见 [4.7 节](#47-vitric-control--ai-控制面)。完整方法参考：[docs/agent-guide.en.md](file:///Users/leolele/Documents/leo/vitric/docs/agent-guide.en.md)（[中文](file:///Users/leolele/Documents/leo/vitric/docs/agent-guide.md)）。

### 7.2 MCP server

[mcp/index.js](file:///Users/leolele/Documents/leo/vitric/mcp/index.js) 用 `@modelcontextprotocol/sdk` 暴露 14 个工具，任何 MCP 客户端开箱即用。配置：

```json
{ "mcpServers": { "vitric": { "command": "node", "args": ["<repo>/mcp/index.js"], "env": { "VITRIC_BIN": "<repo>/target/release/vitric" } } } }
```

| 工具 | 调用 | 功能 |
|------|------|------|
| `vitric_check` | `runCli(["check", dir])` | 校验项目 |
| `vitric_team` | `runCli(["team", dir])` | 协同黑板（只读） |
| `vitric_role` | `readFile(team/skills/vitric-<role>/SKILL.md)` | 领角色工单 |
| `vitric_start` | `spawn(VITRIC_BIN, ["run", dir, "--port","0",...])` | 启动游戏 |
| `vitric_stop` | `rpc("sim/quit")` + kill | 停止游戏 |
| `vitric_observe` | `rpc("render/describe", {width,height})` | 语义观察（主通道） |
| `vitric_screenshot` | `rpc("render/screenshot", {path,width,height})` | 无头截图 |
| `vitric_step` | `rpc("sim/pause")` + `rpc("sim/step", {ticks})` | 暂停并单步 |
| `vitric_input` | `rpc("input/inject", {action, phase})` | 注入输入 |
| `vitric_world` | `rpc("world/<op>", rest)` | 查改世界 |
| `vitric_assert` | `rpc("assert/<op>", {id, if})` | 管理断言 |
| `vitric_time` | `rpc("sim/<op>", ...)` | 时间控制 |
| `vitric_reload` | `rpc("project/reload")` | 热重载 |
| `vitric_rpc` | `rpc(method, params)` | 通用兜底 |

---

## 8. 自动试玩与交付门禁

### 8.1 vitric playtest

`vitric playtest` 跑一群确定性 agent 通过游戏，聚合结构化报告——机械 QA（清地板，不是清天花板）。源：[crates/vitric-playtest](#48-vitric-playtest--agent-集群试玩)。

报告维度：

- **Clear rate & reachability** — 能否通关？哪些声明结局无 run 触达？
- **Soft-locks** — 输入序让 run 卡死的簇（可重放）
- **Dead content** — 没人用的物品/能力/动作
- **Pacing** — 哪里卡、难度尖峰
- **Number breakage** — 经济跑飞/崩盘/溢出
- **Dominant strategy** — 一招鲜
- **Clarity / continuity** — LLM playtester 的定性 note

两个特性让这成为可能：
- **Lookahead search**：sim 确定 + 精确快照/恢复，agent 可投机试动作、滚几 tick、打分、回滚——真玩技巧类游戏（`--strategy lookahead`）
- **Certificates as seeds**：`vitric gate` 通关录像既是"可通关证明"又是 swarm 种子——扰动已知解（reorder/branch/drop）找什么打破它

### 8.2 vitric gate

`vitric gate` 交付门禁。门集（[gate.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/gate.rs)）：

1. **check 门**：完整项目校验（默认开）
2. **通关录像门**：逐校验点重放 + `must_emit` 事件出现 + 长度合规（`max_ticks` 防注水）
3. **断言门**：重放过程中每 tick 全量求值断言集
4. **可选 playtest 门**：清单声明 `gates.playtest` 才跑——真跑 swarm 聚合报告核对契约（clearable/软锁数/不可达结局/惰性动作/数值崩）

全过才出证书——"交付完成"由这里裁决，不由 agent 自述。

### 8.3 vitric bundle

`vitric bundle`（[bundle.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/bundle.rs)）：gate PASS 后把项目附进引擎副本，出自包含单文件（无证书不发行）。档案 = 长度前缀二进制 + zlib 压缩 + 16 字节尾标（`MAGIC=b"VITRICPK"` + blob 长度 u64 LE）。发行包双击解包开窗运行（CPU 渲染），也是完整 CLI。

---

## 9. CLI 命令参考

源：[main.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/main.rs)。

```bash
# 校验项目（schema/场景/规则/脚本/素材，错误带路径+错误码+修复提示）
vitric check <项目目录>

# 无头运行 + AI 控制面
vitric run <项目目录> [--port 6173] [--speed 1.0] [--ticks N] [--record <文件>]
                       [--load <槽名>] [--window] [--renderer gpu|cpu]

# 重放录像并校验确定性
vitric replay <项目目录> <录像.json>

# 自动试玩：单局出录像 / --sessions N 并行 swarm 聚合报告 / --seed-recording 种子探索 / --llm N 拟人玩
vitric playtest <项目目录> [--strategy random|greedy|economy|lookahead] [--horizon 8] [--beam 4]
                           [--seed 0] [--max-ticks 600] [--sessions 1] [--llm 0]
                           [--seed-recording <录像>] [--out <路径>] [--report-dir <目录>] [--html <路径>]

# 自动配平：二分搜索数值旋钮让通关率达标
vitric balance <项目目录> --knob <文件#json-pointer> --target-clear-rate <lo,hi> [...]

# 交付门禁：check + 通关录像重放 + 断言集 + 可选 playtest 门
vitric gate <项目目录>

# 发行打包：gate PASS 后出自包含单文件
vitric bundle <项目目录> [--out <文件>] [--engine <exe>]

# 素材：全项目 PNG 统一色板 / --normals 生成法线 / --frames 帧动画流水线
vitric assets <项目目录> [--colors 32] [--height H] [--palette-lock] [--normals|--normals-ai] [--frames <目录>] [--no-compress]

# 多 agent 协同黑板（只读，永远退出 0）
vitric team <项目目录>

# 地盘执法：改动越界即退出 1
vitric turf <项目目录> --role <角色> <改动文件...>
```

运行时 LLM 经环境变量启用：`VITRIC_LLM_URL` / `VITRIC_LLM_KEY` / `VITRIC_LLM_MODEL`（[llm.rs](file:///Users/leolele/Documents/leo/vitric/crates/vitric-cli/src/llm.rs)）。

---

## 10. 构建与测试

### 构建

```bash
cargo build --release
# 二进制：./target/release/vitric

# 交叉编译 Windows exe（见 .github/workflows/ci.yml）
cargo build --release --target x86_64-pc-windows-gnu -p vitric-cli
# 需 MinGW；.cargo/config.toml 配了 linker = "x86_64-w64-mingw32-gcc"
```

系统依赖（Linux）：ALSA 头（`libasound2-dev`，音频用）、esbuild（`npm i -g esbuild`，TypeScript 脚本转译用，无 .ts 脚本可不装）。

### 测试与质量门禁

```bash
cargo test --workspace                    # 全部测试（650+ 用例）
cargo clippy --workspace --all-targets -- -D warnings   # 零警告门禁
```

CI（[.github/workflows/ci.yml](file:///Users/leolele/Documents/leo/vitric/.github/workflows/ci.yml)）三 job：
- **test**：`cargo test --workspace` + `cargo clippy --workspace --all-targets -D warnings`
- **windows-build**：交叉编译 `vitric.exe` 上传 artifact
- **mcp**：`npm install` + `node scripts/mcp-smoke.mjs`（initialize + check + start/observe/stop 冒烟）

### 示例项目

[examples/](file:///Users/leolele/Documents/leo/vitric/examples) 含可运行示例，每个被测试覆盖：
- `coin-run` — 规则+脚本+动画+音频全覆盖
- `jump` — 纯规则平台跳跃（零脚本）
- `cave-gen` — 配方生成关卡（改 seed 整关重生成）
- `glow` — 动态光照
- `spire` / `ember` — 完整带 GDD 的游戏
- `ui-menu` / `ui-gallery` — UI 系统
- `intro` — 时间轴/序列系统
- `frame-anim` — 帧动画流水线产物

[games/](file:///Users/leolele/Documents/leo/vitric/games) 含更大型的完整游戏：`echo`（卡牌战斗）、`frontier`（殖民模拟经营）。

---

## 11. 多 Agent 协同

引擎随附多 agent 班子协议（[team/](file:///Users/leolele/Documents/leo/vitric/team)），不依赖某家 skill 格式——任何 agent 平台从引擎本体领工单。

### 协议三句话

1. **文件即地盘**：每个角色只写自己的目录，越界即违规（`vitric turf` 机器执法）。跨地盘需求走事件约定提给导演。
2. **schema 即合同**：`GDD.md` + `schema.json` 的组件字段、事件名是全队接口，只有导演能改。
3. **验收门必须客观**：交付 = `vitric gate` PASS，不靠 agent 自述完成。

### 地盘表（`vitric turf` 执法依据）

| 角色 | 可写 |
|------|------|
| art | `assets/`、`animations.json`、`palette.json` |
| level | `scenes/` |
| gameplay | `rules/`、`scripts/` |
| audio | `sounds/` |
| narrative | `scenes/`（与 level 共享，行级分工在 GDD 约定） |
| qa | `qa/`、`recordings/` |
| director | 一切（`GDD.md`、`schema.json`、`vitric.json` 只有导演能动） |

六份角色工单在 [team/skills/vitric-<role>/SKILL.md](file:///Users/leolele/Documents/leo/vitric/team/skills)，MCP 客户端调 `vitric_role` 工具直接取。完整打法见 [docs/team-playbook.md](file:///Users/leolele/Documents/leo/vitric/docs/team-playbook.md)。

---

## 附录：关键文档索引

- [README.md](file:///Users/leolele/Documents/leo/vitric/README.md) / [README.zh-CN.md](file:///Users/leolele/Documents/leo/vitric/README.zh-CN.md) — 定位、快速上手、架构
- [llms.txt](file:///Users/leolele/Documents/leo/vitric/llms.txt) — 给 Agent 读的仓库入口
- [docs/agent-guide.en.md](file:///Users/leolele/Documents/leo/vitric/docs/agent-guide.en.md) / [agent-guide.md](file:///Users/leolele/Documents/leo/vitric/docs/agent-guide.md) — CLI + 全部控制面方法 + 数据语言 + 引擎约定（一页全）
- [docs/errors.md](file:///Users/leolele/Documents/leo/vitric/docs/errors.md) — 全部 VD/VR 错误码与修法
- [docs/AI原生游戏引擎-设计稿.md](file:///Users/leolele/Documents/leo/vitric/docs/AI原生游戏引擎-设计稿.md) — 完整设计决策与理由
- [docs/art-pipeline.md](file:///Users/leolele/Documents/leo/vitric/docs/art-pipeline.md) — 美术流水线
- [docs/design-agent-playtest.md](file:///Users/leolele/Documents/leo/vitric/docs/design-agent-playtest.md) — Agent 试玩设计
- [docs/design-ui.md](file:///Users/leolele/Documents/leo/vitric/docs/design-ui.md) — UI 设计
- [docs/design-frame-animation.md](file:///Users/leolele/Documents/leo/vitric/docs/design-frame-animation.md) — 帧动画设计
- [docs/design-tween-sequence.md](file:///Users/leolele/Documents/leo/vitric/docs/design-tween-sequence.md) — 补间/序列设计
- [docs/team-playbook.md](file:///Users/leolele/Documents/leo/vitric/docs/team-playbook.md) — 多 agent 班子打法
