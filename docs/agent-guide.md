# Vitric Agent 指南

给 AI agent（和人）的一页纸操作手册：怎么自主地跑、看、测、改一个 Vitric 游戏。

## 三个命令

```bash
vitric check <项目目录>                  # 校验一切（schema/场景/规则/脚本），错误带路径+错误码+修法
vitric run <项目目录> [--port 6173] [--speed X] [--ticks N] [--record 录像.json]
vitric replay <项目目录> <录像.json>      # 重放录像并逐校验点验证确定性
```

`run` 启动后 stdout 第一行是 JSON 横幅，里面有控制面地址。

## 控制面（HTTP JSON-RPC）

`POST http://127.0.0.1:6173/rpc`，请求体 `{"method": "...", "params": {...}}`，
响应 `{"ok": true, "result": ...}` 或 `{"ok": false, "error": "带修复提示的错误"}`。

### 看

| 方法 | 参数 | 说明 |
|---|---|---|
| `ping` | — | tick / 暂停状态 / 倍速 |
| `world/entities` | `components?: []` | 列实体（可按组件过滤） |
| `world/get` | `entity` | 一个实体的全部组件。实体写法：`"@名字"` 或句柄 `"e3v1"` |
| `events/recent` | `since?: tick` | 最近事件（输入/碰撞/规则和脚本 emit 的全部可见） |
| `render/describe` | `width? height?` | **语义观察（主通道）**：画面翻译成精确文字——可见实体的九宫格方位/世界与屏幕坐标/颜色尺寸、视觉遮挡对、视野外实体的方向和距离，附中文摘要。比看像素更精准 |
| `render/screenshot` | `width? height? path? inline?` | 无头截图（兜底验证：怀疑渲染本身有问题、或要做像素级断言时用），不需要 GPU/窗口 |
| `inspect/selection` | — | **人指哪你看哪**：窗口里人点选的实体（青色描边高亮），完整组件回传 |
| `inspect/select` | `entity`（null 清空） | 反向指给人看：你选中的实体在窗口里高亮 |
| `sim/hash` | — | 世界状态哈希（断言两次运行一致就比它） |

### 动

| 方法 | 参数 |
|---|---|
| `input/inject` | `action`, `phase: pressed/released` |
| `world/set` | `entity`, `path`（如 `"Health.hp"`）, `value` — 写入过 schema，越界直接拒 |
| `world/spawn` | `components`, `name?` |
| `world/despawn` | `entity` |

### 控时间

| 方法 | 参数 |
|---|---|
| `sim/pause` / `sim/resume` | — |
| `sim/step` | `ticks?`（只在暂停时可用；返回里带新发生的断言失败） |
| `sim/speed` | `multiplier`（无上限，无头狂奔随便开） |
| `sim/snapshot` / `sim/restore` | — / `snapshot`（时间旅行：存档任意时刻、跳回去） |
| `project/reload` | —（**热重载**：改完磁盘上的规则/脚本文件后调用，毫秒级生效，世界状态不动；失败保持旧逻辑。schema/场景改动需重启） |
| `sim/quit` | — |

### 测

| 方法 | 参数 |
|---|---|
| `assert/add` | `id`, `if: [["@player.Health.hp", ">=", 0], ...]` — 每 tick 检查，违反自动上报 |
| `assert/remove` / `assert/list` / `assert/failures` | — |

## 典型闭环

```bash
vitric check my-game                          # 1. 改完数据先校验
vitric run my-game --port 6173 &              # 2. 起进程
curl -s :6173/rpc -d '{"method":"sim/pause"}'                       # 3. 暂停
curl -s :6173/rpc -d '{"method":"assert/add","params":{"id":"hp","if":[["@player.Health.hp",">",0]]}}'
curl -s :6173/rpc -d '{"method":"input/inject","params":{"action":"right"}}'
curl -s :6173/rpc -d '{"method":"sim/step","params":{"ticks":60}}'  # 4. 确定性单步
curl -s :6173/rpc -d '{"method":"render/describe"}'                 # 5. 语义观察：画面上有什么、在哪、谁挡谁
curl -s :6173/rpc -d '{"method":"world/get","params":{"entity":"@player"}}'         # 6. 查状态
# 怀疑渲染不对劲再截图对照: {"method":"render/screenshot","params":{"path":"shot.png"}}
```

复现 bug：`vitric run my-game --ticks 600 --record bug.json` 录下来，
`vitric replay my-game bug.json` 任何时候逐帧复现；重放跑偏会精确报告在哪个校验点开始不一致。

## 写游戏的数据语言

- `vitric.json` 清单：name / schema / entry / scenes / rules / scripts / seed
- `schema.json`：组件字段类型（number/int/bool/text/vec2/entity/enum/list + default/required/min/max）
- 场景：实体数组，组件值缺省自动补 default
- 规则（玩法正门）：`{"id", "on", "if": [[左,op,右]...], "do": [动作...]}`
  - 触发 `on`: `"tick"`（配 `each: [组件]` 逐实体） / `{"event": "collision", "between": ["Player","Coin"]}` / `{"event":"input","filter":{...}}`
  - 动作: `set/add/spawn/despawn/emit/call`
  - 路径: `self.组件.字段` / `other.…` / `@实体名.…` / `event.字段`
- 脚本（复杂逻辑落点，JS）：
  - `vitric.system("名", {query: [...], writes: [...]}, (entities, ctx) => {...})` — writes 没声明的组件改了就报错
  - `vitric.fn("名", (args, ctx) => {...})` — 给规则 `call`
  - `ctx.random()`（确定性，别用 Math.random，会直接 throw）/ `ctx.tick` / `ctx.emit` / `ctx.spawn` / `ctx.despawn`

## 动画

清单挂 `"animations": "animations.json"`，文件里定义片段：`{"clips": {"walk": {"frames": ["w0.png","w1.png"], "fps": 6, "loop": true}}}`。
实体挂 `Anim` 组件（schema 需定义 clip/prev/t/done 四字段），**引擎独占 Sprite.image 的写权**——换动画唯一正路是改 `Anim.clip`（规则 set 即可），切换自动从头播；非循环片段播完发 `anim-finished` 事件并停末帧。状态全在组件里，快照/回放安全。

## 内建事件

`start`（tick 0，初始化/生成关卡的标准入口）、`input`、`collision`、`anim-finished`。

## 引擎约定组件

内建系统只认这些名字：`Position{x,y}` + `Velocity{x,y}` → 每 tick 积分移动；
`Position` + `Collider{w,h}` → AABB 碰撞发 `collision` 事件；
`Position` + `Sprite{w,h,color}` → 渲染；`Camera{x,y,scale}` → 取景。
