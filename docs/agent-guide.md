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
| `perf/stats` | — | 实体数/单 tick 事件数/素材解码内存/预算配置。清单 `budgets` 设上限后超标会进 assert/failures（kind=budget） |

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

## 确定性边界

引擎保证什么、不保证什么，边界说清楚：

- **录像只记输入流。** 录制期间 `world/set` / `world/spawn` / `world/despawn` / `project/reload` / `sim/restore` 会被明确拒绝（改了的状态不进录像，录出来必然重放分歧），检查器拖拽也会被禁用。要在录制中影响世界，用 `input/inject`——输入会被录下来。
- **脚本必须无状态。** 跨 tick 的状态只能放组件里。`globalThis`/闭包里存的东西不进快照、热重载时清零，restore 之后必然分歧。`Math.random` / `Date.now` / `new Date()` 直接 throw 并指路 `ctx.random()` / `ctx.tick`；显式传参的 `new Date(0)` 是纯计算，放行。
- **快照是全量的。** `sim/snapshot` 含世界、tick、随机数状态、未消化的输入、逻辑层暂存事件，restore 后继续跑和原轨迹逐位一致（有测试锁着）。
- **确定性保证范围 = 同平台同二进制。** `Math.sin` 这类超越函数依赖系统数学库，跨平台（Linux ↔ Windows）末位可能不同；跨平台分享录像/比对哈希不在保证内。

## 写游戏的数据语言

- `vitric.json` 清单：name / schema / entry / scenes / rules / scripts / seed
- `schema.json`：组件字段类型（number/int/bool/text/vec2/entity/enum/list + default/required/min/max）
- 场景：实体数组，组件值缺省自动补 default
- 规则（玩法正门）：`{"id", "on", "if": [[左,op,右]...], "do": [动作...]}`
  - 触发 `on`: `"tick"`（配 `each: [组件]` 逐实体） / `{"event": "collision", "between": ["Player","Coin"]}` / `{"event":"input","filter":{...}}`
  - 动作: `set/add/spawn/despawn/emit/call`
  - 路径: `self.组件.字段` / `other.…` / `@实体名.…` / `event.字段`
- 脚本（复杂逻辑落点，JS 或 TS——`.ts` 文件自动经 esbuild 转译，需要 PATH 上有 esbuild 或设 ESBUILD_BIN）：
  - `vitric.system("名", {query: [...], writes: [...]}, (entities, ctx) => {...})` — writes 没声明的组件改了就报错
  - `vitric.fn("名", (args, ctx) => {...})` — 给规则 `call`
  - `ctx.random()`（确定性，别用 Math.random，会直接 throw）/ `ctx.tick` / `ctx.emit` / `ctx.spawn` / `ctx.despawn`

## 动画

清单挂 `"animations": "animations.json"`，文件里定义片段：`{"clips": {"walk": {"frames": ["w0.png","w1.png"], "fps": 6, "loop": true}}}`。
实体挂 `Anim` 组件（schema 需定义 clip/prev/t/done 四字段），**引擎独占 Sprite.image 的写权**——换动画唯一正路是改 `Anim.clip`（规则 set 即可），切换自动从头播；非循环片段播完发 `anim-finished` 事件并停末帧。状态全在组件里，快照/回放安全。

## 内建事件

`start`（tick 0，初始化/生成关卡的标准入口）、`input`、`collision`、`anim-finished`。

## 音效

约定事件：规则/脚本 `{"emit": "play-sound", "data": {"sound": "coin.wav"}}`，引擎播放项目 `sounds/` 目录下的文件（wav/ogg/mp3/flac）。音频是纯输出副作用不进模拟，确定性回放不受影响；无声卡环境（容器/CI）启动横幅会标 `audio: disabled` 但事件照常流动。`vitric check` 会静态校验字面引用的音效文件存在。

## 引擎约定组件

内建系统只认这些名字：`Position{x,y}` + `Velocity{x,y}` → 每 tick 积分移动；
`Position` + `Collider{w,h}` → AABB 碰撞发 `collision` 事件；
`Position` + `Sprite{w,h,color,image,rot}` → 渲染；`Camera{x,y,scale}` → 取景。
`Sprite.rot` 可选（度数）：精灵绕自身 Position 旋转，世界空间逆时针为正（画面上看也是逆时针），缺省 0 = 不旋转；屏上文字（Text）永远直立不旋转，点选（pick）按旋转后的真实形状命中。
游戏感组件（Camera 的 follow/lerp、`Shake`、`Particle`）见下面「游戏感」一节。

## 平台物理

- `Body{gravity, grounded}`（搭配 Velocity+Collider）：每 tick `Velocity.y += gravity * DT`（世界 y 朝上，重力填负数如 -30）；`grounded` 由引擎维护，落在 Solid 顶面时为 true——起跳规则的标准条件。
- `Solid{}`（搭配 Position+Collider）：挡停体（地面/墙/平台）。带 Body 的实体撞上会贴边停、该轴速度清零；轴分离裁剪，单 tick 位移别超过障碍厚度（无扫掠，速度预算留余量）。
- 起跳就是一条规则：`on input(space) if [["@hero.Body.grounded","==",true]] do set @hero.Velocity.y = 14`。完整可玩示例见 `examples/jump`（纯规则零脚本）。

## 游戏感 / Game feel

跟 Body/Solid 一样的约定组件：引擎认名字，字段自己在 schema 里定义；状态全在组件里，快照/回放安全。三个系统都跑在运动/物理之后、碰撞检测之前。

- **相机跟随**：`Camera` 加两个可选字段 `follow`（要跟随的实体名，空串 = 不跟随）和 `lerp`（0..1，每 tick 逼近比例，1 = 硬锁定）。引擎每 tick 在运动之后把 Camera.x/y 拉向目标 Position——相机看的是本 tick 的最终位置，不滞后一帧。follow 指向不存在的实体直接报错（不静默跳过）。
- **屏幕抖动**：相机实体挂 `Shake{amplitude, decay}`。amplitude > 0 时渲染取景叠加确定性伪随机偏移（(tick, amplitude) 的纯函数，不碰模拟的随机数流——抖屏对 gameplay 轨迹零影响）；每 tick `amplitude *= decay`（低于 0.001 归零）。偏移只作用于画面（窗口/截图），`render/describe` 和点选读的是不抖的相机。触发不需要新动作，规则 set 就行——碰撞抖一下：
  ```json
  {"id": "hit-shake", "on": {"event": "collision", "between": ["Player", "Enemy"]},
   "do": [{"set": "@camera.Shake.amplitude", "to": 0.5}]}
  ```
- **粒子**：实体挂 `Particle{ttl}`（剩余 tick 数，整数），引擎每 tick 减 1，到 0 自动销毁（销毁顺序 = 槽位序，确定性）。五彩纸屑/尘土/爆炸 = spawn 一批 Sprite+Velocity+Particle 然后不管，不用写清扫规则。

## 光照 / Lighting

跟 Body/Solid 一样的约定组件：引擎认名字，字段自己在 schema 里定义。

- **总开关 = Ambient 实体的存在**。场上没有带 `Ambient` 组件的实体 = 完全不跑光照（旧行为、零开销）；有一个（取第一个）= 光照管线启动，整帧打光。
- `Ambient{color}`：场景环境光底色，如暗色洞穴 `"#202838"`；`"#ffffff"` = 无灯处保持原样。
- `Light{radius, color, intensity}` + `Position`：点光源。radius 世界单位（到 radius 处衰减为零）；color 缺省 `"#ffffff"`、intensity 缺省 1.0。**上限 64 盏**，超了显式报错（不静默截断）。
- 公式（CPU 截图和 GPU 窗口同一套）：`lit = min(ambient + Σ 灯色·intensity·(1 - d/r)², 1.5)`，`out = min(场景色 · lit, 1.0)`。1.5 的上限允许轻微过曝（廉价泛光感）。
- **所有东西一视同仁被打光**——精灵、文字、背景，屏幕锚定的 HUD 也不豁免。HUD 要保持可读，自己在旁边放盏灯或调亮 Ambient。
- 光照确定性：只读组件状态，同一世界同一 tick 渲出的字节逐位相同；`render/screenshot` 含光照——agent 截到的就是玩家看到的。
- `render/describe` 在光照开启时多给 `ambient`（环境色）和 `lights` 数组（id/name/世界坐标/radius/intensity/color）+ 一行摘要——光照设置全部可文字化观察。

```json
{"name": "torch", "components": {"Position": {"x": 10, "y": 4},
  "Light": {"radius": 6, "color": "#ff9040", "intensity": 1.2}}}
```

## 屏上文字

`Text{content, size, color}` + `Position`：内嵌 8x8 点阵字体（ASCII），每字符 size×size 世界单位、整串居中于 Position，画在精灵之上。`render/describe` 直接给出 `texts[].content`——agent 不用从截图认字。
数字状态转文字用规则的 format 模板：`{"set": "@hud.Text.content", "to": {"format": "SCORE {}", "args": ["self.Score.value"]}}`（`{}` 与 args 个数必须一致）。
