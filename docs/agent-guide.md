# Vitric Agent 指南

给 AI agent（和人）的一页纸操作手册：怎么自主地跑、看、测、改一个 Vitric 游戏。

## 八个命令

```bash
vitric check <项目目录>                  # 校验一切（schema/场景/规则/脚本），错误带路径+错误码+修法
vitric run <项目目录> [--port 6173] [--speed X] [--ticks N] [--record 录像.json] [--load 槽名]
vitric replay <项目目录> <录像.json>      # 重放录像并逐校验点验证确定性
vitric gate <项目目录>                   # 交付门禁：check + 通关录像重放 + 断言集，全过才退出 0（见「交付门禁」节）
vitric bundle <项目目录> [--out 文件] [--engine 引擎exe]  # 发行打包：gate PASS 后出自包含单文件，无证书不发行（见「发行打包」节）
vitric assets <项目目录> [--colors N] [--height H] [--palette-lock]  # 全项目 PNG 统一色板，AI 出图规整成一个调，详见 docs/art-pipeline.md
vitric team <项目目录>                   # 多 agent 班子协同黑板：各角色交付物健康度+合同/门禁状态+卡点提示（只读，永远退出 0），详见 team/README.md
vitric turf <项目目录> --role <角色> <改动文件...>  # 地盘执法：改动文件越出角色地盘即退出 1，逐条点名
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
| `render/describe` | `width? height?` | **语义观察（主通道）**：画面翻译成精确文字——可见实体的九宫格方位/世界与屏幕坐标/颜色尺寸、视觉遮挡对、视野外实体的方向和距离，附中文摘要。当画面有"焦点"（被 `Camera.follow` 跟随的那个实体）时，还会给每个实体加一段 `relative_to_focal`（相对焦点的方位/距离/视线有没有被挡）、顶层多一张 `ascii_map`（以焦点为中心的关卡格子图），并把实体按"有名优先、近的优先"排序；顶层 `actions` 始终列出能按哪些键；从第二次调用起，顶层 `changes` 给出跟上一次相比变了啥——详见「给模型读的画面」节。比看像素更精准。屏上文字会做可读性体检：与底色对比度（WCAG 式比值）低于 2.5 时多给 `warnings[]`（kind=`low-contrast-text`，含 entity/content/ratio/hint）+ 摘要 ⚠ 行——你"看不见"像素，引擎替你看（见「屏上文字」节） |
| `render/screenshot` | `width? height? path? inline?` | 无头截图（兜底验证：怀疑渲染本身有问题、或要做像素级断言时用），不需要 GPU/窗口 |
| `inspect/selection` | — | **人指哪你看哪**：窗口里人点选的实体（青色描边高亮），完整组件回传 |
| `inspect/select` | `entity`（null 清空） | 反向指给人看：你选中的实体在窗口里高亮 |
| `sim/hash` | — | 世界状态哈希（断言两次运行一致就比它） |
| `perf/stats` | — | 实体数/单 tick 事件数/素材解码内存/预算配置。清单 `budgets` 设上限后超标会进 assert/failures（kind=budget） |

### 动

| 方法 | 参数 |
|---|---|
| `input/inject` | `action`, `phase: pressed/released` |
| `input/click` | `x`, `y`（**世界坐标**）, `button?: left/right`（缺省 left）— 无头"鼠标"：拾取解析和窗口点选同一条路径，注入 `mouse` / `mouse-alt` 事件，返回里直接带拾取结果（见「鼠标输入」节） |
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
| `save/write` / `save/load` / `save/list` | `slot` / `slot` / —（玩家存档槽位 `saves/<slot>.json`，详见「存档」节；`save/load` 录像期间拒绝） |
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

## 给人看整局回放（web 端）

`render/describe` / `render/screenshot` 是给 agent 自己看的。要把**整局过程**给人（产品/策划）回看、或自己一眼扫完体验是否顺，用**状态流回放**——纯数据、无图片无视频，浏览器现画：

1. **抓状态流**：经控制面驱动一局（或跑现成通关脚本），每隔若干 tick 调一次 `sim/snapshot` 收成数组；同时给关键帧标一句"在做什么"（建造/种收/邀请…）当操作日志。产物是 `{"snaps":[...每帧世界状态...], "acts":[...每帧动作或""...]}` 的 JSON。范例：`games/frontier/tools/capture_replay.py`。
2. **生成回放页**：`python scripts/build_replay.py <状态流.json> <out.html>` → 一个**自包含 HTML 回放页**（项目的 web 端）：canvas 逐帧重画世界 + UI，带操作日志侧栏（点击跳帧）、玩家高亮、角色/颜色图例、播放/暂停/拖进度。整局 delta 编码 + gzip 仅约 10KB——无任何图片或视频，纯状态数据在浏览器现画（引擎渲染的简化同构版）。
3. **看**：HTML 任意浏览器直接打开；放到你自己的静态 host / 项目 web 端即可分享。

确定性保证：`sim/snapshot` 是全量的（世界/tick/RNG/未消化输入），回放与原局逐位一致。

## 给模型读的画面

除了逐个实体列清单，`render/describe` 还多给四样东西，让模型不用对着原始坐标自己算几何就能读懂画面。前两样只在画面有**焦点**时出现——焦点就是被 `Camera.follow` 跟随的那个实体（`follow` 为空 / 没配 / 指向不存在的实体 = 没焦点，这两样直接不出现，所以没用相机跟随的游戏拿到的还是和以前逐字节一样的旧响应）。`actions` 和 `changes` 跟有没有焦点无关，照常出。

**1. `relative_to_focal`**（每个可见/视野外实体上都有，焦点自己除外）：以焦点为原点量出来的空间关系，模型不用从绝对坐标自己算"它在我左边还是右边、够不够得着、被没被挡"：

```json
"relative_to_focal": {
  "direction": "left",        // 8 个方位词之一：right/up-right/up/up-left/left/down-left/down/down-right
  "distance": 12.62,          // 中心到中心，世界单位，保留两位小数
  "same_row": false,          // 竖向偏移在半个 sprite 高以内（大致同一横排）
  "same_col": false,          // 横向偏移在半个 sprite 宽以内（大致同一竖列）
  "adjacent": false,          // 两个碰撞盒贴着或重叠
  "blocked": true             // 焦点到它的视线被第三方 Solid 挡住了
}
```

有焦点时，实体还会按**有名优先、近的优先**排序（焦点自己排最前）。

**2. `ascii_map`**（顶层）：以焦点为中心、有边界的 ASCII 格子图——读关卡结构比读坐标列表或截图都强。`@` = 焦点，`#` = Solid（挡路的几何体），字母 = 其它实体（在 `legend` 里查名字）：

```json
"ascii_map": {
  "grid": [
    "d  g#   #      ",
    "##      #      ",
    "   b    #      ",
    "  ##    #      ",
    "   e  ###      ",
    "    ##  #      ",
    " a      #      ",
    "## f   @#      ",
    "########       "
  ],
  "legend": { "a": "brazier-1", "b": "brazier-2", "d": "spike-1",
              "e": "txt-brazier", "f": "txt-controls", "g": "txt-spikes" },
  "cell_size": 2.0,           // 每个格子代表多少世界单位
  "focal_at": [7, 7]          // @ 在格子里的 [行, 列]
}
```

**3. `actions`**（顶层）：项目规则真正声明出来的输入词汇（也就是"能干啥"）——describe 不只告诉模型"画面里有啥、在哪"，还告诉它"你能按哪些键"。每条是 `{action, phase}`，pressed / released 平铺成两条：

```json
"actions": [
  {"action": "left",  "phase": "pressed"}, {"action": "left",  "phase": "released"},
  {"action": "right", "phase": "pressed"}, {"action": "right", "phase": "released"}
]
```

**4. `changes`**（顶层，从第二次 describe 起才有）：相对上一次 describe 的帧间变化（这次比上次变了啥），模型追"自上次以来变了啥"比每次重读整屏强。第一次调用没有 `changes` 键。结构：`appeared`（新出现的实体对象）、`disappeared`（只给 id）、`changed`（`id → {字段: [旧值, 新值]}`）：

```json
"changes": {
  "appeared": [],
  "disappeared": [],
  "changed": {
    "e0v0": {
      "world":     [{"x": 0.0, "y": 1.05}, {"x": 1.05, "y": 1.05}],
      "sprite":    [{"color": "#6cc6ff", "h": 2.2, "image": "hero-idle.png",  "w": 1.7},
                    {"color": "#6cc6ff", "h": 2.2, "image": "hero-walk1.png", "w": 1.7}],
      "screen_px": [{"x": 160.0, "y": 120.0}, {"x": 168.0, "y": 120.0}]
    }
  }
}
```

（一个实体从屏内挪到屏外算 `changed`、不算 `disappeared`——它还在。开了相机跟随时，焦点一动，其它实体的 `screen_px` 全跟着变，`changed` 就会把它们都列出来。）

## 交付门禁（vitric gate）

"做完了"不能靠 agent 自述——引擎机械地验证交付。核心：**确定性录像是不可伪造的通关证书**。一份录像要拿到证书，必须同时做到①从项目数据冷启动逐校验点逐位重放一致（伪造任何一帧，状态哈希必然跑偏）②重放过程中真的观测到终局事件（默认 `game-won`）。两个条件缺一不可：光重放一致可能是挂机局，光有事件名可能是编的录像。

清单 `vitric.json` 声明门禁：

```json
"gates": {
  "playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}],
  "assertions": "qa/asserts.json",
  "check": true,
  "max_ticks": 100000
}
```

- `playthroughs`（必须非空）：通关录像门。每条录像独立重放验证；`must_emit` 缺省 `"game-won"`。
- `assertions`（可选）：断言集文件，格式 `[{"id": "...", "if": [[左, op, 右], ...]}, ...]`（条件写法同控制面 `assert/add`）。重放过程中**每个 tick** 全量求值，任何一刻违反都拒发证书（报告列出 id + 首次违反的 tick）。
- `check`（缺省 true）：先过完整项目校验，任何错误 = FAIL。
- `max_ticks`（可选）：录像长度上限，防"挂机一百万 tick 总会赢"式注水。

工作流：录像不是 gate 自己生成的——QA/导演真打（或经控制面 RPC 驱动）一局通关，`vitric run my-game --record qa/clear.json` 录下来，然后 `vitric gate my-game` 验证。报告是人机同一份 JSON（`{"pass": bool, "gates": [{name, status, detail}...]}`）打到 stdout；**全部门禁 pass 才退出 0**。清单没声明 gates、或 playthroughs 为空，直接退出 1——无门禁项目不出证书，空门禁放行就是后门。

## 发行打包（vitric bundle）

`vitric bundle my-game` 把项目和引擎打成**一个可分发的单文件**（独立单机）。门禁先行：先跑 `vitric gate`，不 PASS 不出包——无证书不发行（拒绝时门禁报告原样打到 stdout）。通过后把项目文件（含 qa/ 通关录像——证书本体随包；顶层 `saves/`、`assets_original/` 和隐藏文件除外）打成 zlib 压缩档，附在引擎二进制副本尾部（尾标 = 8 字节魔数 + 包长，格式见 `crates/vitric-cli/src/bundle.rs`）。成功输出单行 JSON `{out, bytes, project, files}`；缺省文件名 `<项目名>-<平台>[.exe]`，`--out` 改名。

发行包的行为（exe 尾部有内嵌项目，启动时自检）：

- **无参数**（玩家双击）：解包到 `temp/vitric-<哈希>/` 后开窗运行（CPU 渲染，处处能跑）。解包目录按包哈希唯一，玩家存档 saves/ 长在那里随包持久。
- **`run-embedded [run 选项]`**：运行内嵌项目，选项透传——`--ticks 5` 无头冒烟、`--renderer gpu` 玩家要 GPU 都走这。
- **其他参数**：正常 CLI——发行包同时也是完整引擎。

跨平台：在 linux 上给 windows 出包，`--engine` 指交叉编译好的 windows 引擎（`cargo build --release --target x86_64-pc-windows-gnu`）——尾标格式与平台无关，附在哪个引擎上就是哪个平台的发行包。发行包不能再当 `--engine`（拒绝套娃）。

## 确定性边界

引擎保证什么、不保证什么，边界说清楚：

- **录像只记两条外部通道：输入流 + 外部回复（LLM）。** 录制期间 `world/set` / `world/spawn` / `world/despawn` / `project/reload` / `sim/restore` 会被明确拒绝（改了的状态不进录像，录出来必然重放分歧），检查器拖拽也会被禁用。要在录制中影响世界，用 `input/inject`——输入会被录下来。LLM 回复经引擎的 inject_reply 进模拟，同样被录、重放时原 tick 重新注入（见「运行时 LLM」）；鼠标点击（窗口点击 / `input/click`）也走这条回复通道，录制中照常可用（见「鼠标输入」）。
- **脚本必须无状态。** 跨 tick 的状态只能放组件里。`globalThis`/闭包里存的东西不进快照、热重载时清零，restore 之后必然分歧。`Math.random` / `Date.now` / `new Date()` 直接 throw 并指路 `ctx.random()` / `ctx.tick`；显式传参的 `new Date(0)` 是纯计算，放行。
- **快照是全量的。** `sim/snapshot` 含世界、tick、随机数状态、未消化的输入、逻辑层暂存事件，restore 后继续跑和原轨迹逐位一致（有测试锁着）。
- **确定性保证范围 = 同平台同二进制。** `Math.sin` 这类超越函数依赖系统数学库，跨平台（Linux ↔ Windows）末位可能不同；跨平台分享录像/比对哈希不在保证内。

## 写游戏的数据语言

- `vitric.json` 清单：name / schema / entry / scenes / rules / scripts / font / seed
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
实体挂 `Anim` 组件（schema 需定义 clip/prev/t/done 四字段，`t` 用 `int`），**引擎独占 Sprite.image 的写权**——换动画唯一正路是改 `Anim.clip`（规则 set 即可），切换自动从头播；非循环片段播完发 `anim-finished` 事件并停末帧。状态全在组件里，快照/回放安全。

### 帧动画进口（`--frames`）

AI 出动画的现实路径 = 生成视频/序列图 → 切帧（骨骼绑定是 AI 最不擅长的活）。`vitric assets <项目> --frames <序列图目录>` 把一堆帧图一键变成优化过的动画素材，全程确定性（同输入逐字节同产物）：

1. **相邻帧去重**：逐像素近乎相同的相邻帧只留一张，记「停留多少帧」（AI 动画常有静止段，去重砍磁盘+显存）。
2. **裁空白边 trim**：每帧裁透明边、记偏移（播放摆回原位，视觉不变）。
3. **打包图集 atlas**：所有帧拼一张大图（减 GPU 纹理切换），记每帧 uv 矩形。
4. **统一色板**：复用 median-cut，整组帧一套（`--colors N`，默认 32；`--colors 0` 跳过）。
5. **写动画配置**：
   - `animations.json` 标准 clip——去重后的帧名，**停留 = 在 frames 列表里重复帧名**，`advance_animations` 按 fps=60（每 tick 一帧）逐帧确定播放，render 核心一字不改。
   - `<片段>-atlas.png` + `<片段>-atlas.json`（帧表：uv + rect + trim 偏移 + 停留）——极致内存路径产物。

产物落进项目 `assets/`（去重帧 `assets/<片段>/frameNNN.png`）和清单旁的 `animations.json`。片段名取序列图**目录名**。视频不内置解码器：检测到 mp4/mov 等会明确提示先 `ffmpeg -i in.mp4 frame%04d.png` 转序列图（不静默失败）。`--frames` 与色板和谐化/法线互斥；它自己接受 `--colors` 和 `--no-compress`。

`vitric check` 校验帧进口产物：图集存在、帧表合法、uv/rect 不越界、引用的帧图都在——缺了红灯并点名路径 + VDxxx 码。

示例项目：`examples/frame-anim`（程序生成的滑动+静止段占位帧跑 `--frames` 出的产物）。

### 压缩纹理（BC7）

显存大头是 RGBA8 全驻留（前车之鉴：桌宠 2400 帧不压缩 → 8.5G 显存）。`--frames` 默认把图集离线压成 **BC7（BPTC）**：一块 4×4 像素恒 16 字节 = 8bpp，是 RGBA8（32bpp）的 **1/4（4×）**，加去重额外省。产物 `<片段>-atlas.bc7`（自描述头 + 块数据），报告里给 `compression_ratio`。`--no-compress` 关掉只出 RGBA8 图集。

- **离线压、运行时只上传**：BC7 编码在 assets 流水线做，不在运行时实时压。
- **GPU 上传需要 device feature `TEXTURE_COMPRESSION_BC`**；不支持的设备**显式报错、不 fallback** 到 RGBA8 静默膨胀（让问题暴露）。
- **CPU 真相源路径不变**：CPU 参考渲染仍用 RGBA8（它不吃显存，截图逐字节确定）。
- **确定性**：压缩纹理只影响 GPU 视觉，**不进模拟状态/哈希**——模拟只认帧索引/clip 名，不认像素。

## 场景与流程 / Scenes & flow

完整的游戏不止一个场景：菜单 → 关卡 → 下一关 → 结局。切换是一个约定事件，整个发生在确定性流水线之内：

- 规则/脚本 `{"emit": "load-scene", "data": {"scene": "scenes/level2.json"}}`。`scene` 必须在清单 `scenes` 列表里——不在就显式报错并列出可用场景（新场景文件先加进 vitric.json）。
- 切换在该 tick 逻辑的尾部执行：旧世界的实体**全部**正规销毁（旧句柄干净失效、名字释放），新场景按启动时预载的数据实例化。因为触发事件本身是确定性的，录像重放会在同一 tick 复现同一切换，跨切换的校验点哈希照常成立；快照/restore 同样跨切换可用。运行中改磁盘上的场景文件不影响本进程的切换结果（场景和 schema 一样启动时装载，改了要重启）。
- **跨场景携带 = `Persist` 标记组件**。挂了 `Persist`（schema 里定义一个无字段组件即可）的实体在切换中幸存：全部组件原样搬进新世界、按原名重建——玩家、分数、背包的延续零新系统。两条硬约束：幸存者必须有名字（匿名的没法被规则引用，显式报错）；名字不许和目标场景的实体重名（重名显式报错）。
- **每场景的初始化钩子是 `scene-loaded {scene}`**（切换完成后的下一 tick 送达规则）；`start` 只在整局 tick 0 发一次，切换**不会**重发它。
- 同一 tick 发出多个 load-scene = 显式报错（去哪个场景没有答案，给切换规则加互斥条件）。
- `vitric check` 实例化清单里**每个**场景——非入口场景的坏引用（缺图、未定义的动画片段）也在 check 期红灯，不会拖到切换那一刻才炸。

```json
{"id": "level-clear", "on": {"event": "collision", "between": ["Player", "Exit"]},
 "do": [{"emit": "load-scene", "data": {"scene": "scenes/level2.json"}}]}
```

## 存档 / Saves

完整的游戏要能"存档随时续玩"。存档和音效/场景切换一样走约定事件，但两个方向住在确定性边界的不同侧：

- **存档**：规则/脚本 `{"emit": "save-game", "data": {"slot": "slot1"}}`。引擎把完整快照（同 `sim/snapshot`：世界/tick/随机数状态/未消化的输入与回复/逻辑层暂存事件）写到项目 `saves/<slot>.json`（目录自动创建，文件带 `engine_version` 和项目名）。写盘是**纯输出副作用**——和 play-sound 一个待遇，在模拟之外执行、不回流进世界，确定性回放不受影响，录像期间照常放行；写失败在 stderr 上报结构化 `save_error` 行（不崩游戏）。
- **读档**：`{"emit": "load-game", "data": {"slot": "slot1"}}`。运行循环在帧边界把模拟整体恢复到存档时刻（等价 `sim/restore`），检查器选中态一并清空。**读档是"会话边界"操作：它不进录像、回放不会复现它——与 `--record` 互斥**。录像期间发 load-game 会收到 stderr 的结构化拒绝（理由同 `sim/restore` 的录像守卫：时间线断裂，录像必然不可重放），录像保持有效。
- **槽名规则**：`[a-z0-9-]{1,32}`。槽名直接成为文件名，越界（含 `../` 路径穿越）显式拒绝。
- **版本策略**：存档记录写入时的引擎版本，读档时不匹配显式报错（"存档来自引擎 vX，当前 vY，不保证兼容"），不做静默兼容尝试——确认要读就手改文件里的 `engine_version`；JSON 损坏同样显式报错。
- **续玩**：`vitric run my-game --load slot1` 在 tick 0 之前恢复到存档（`start` 事件不重发）；槽位不存在的启动错误会列出现有存档。`--load` 与 `--record` 互斥——录像要求从项目数据冷启动可重放。
- **控制面对称入口**：`save/write {slot}` / `save/load {slot}` / `save/list`，与约定事件同一条代码路径、同一套守卫。注意约定事件由运行循环执行：`sim/pause` + `sim/step` 单步驱动时 save-game/load-game 和 play-sound 一样**不执行**（`vitric replay` 重放中同理），agent 单步调试时直接用 save/* RPC。

```json
{"id": "save-point", "on": {"event": "collision", "between": ["Player", "Checkpoint"]},
 "do": [{"emit": "save-game", "data": {"slot": "auto"}}]}
```

## 内建事件

`start`（tick 0，初始化/生成关卡的标准入口；场景切换不重发）、`input`、`mouse` / `mouse-alt`（鼠标点击，见「鼠标输入」）、`collision`、`anim-finished`、`scene-loaded`（每次场景切换后的下一 tick，每场景初始化钩子，见「场景与流程」）。

## 鼠标输入 / Mouse input

点击是和按键同级的**游戏输入**——菜单、卡牌这类鼠标游戏直接用规则消化：

- **事件**：左键 = `mouse`，右键 = `mouse-alt`，data 都是 `{x, y, entity}`——x/y 是**世界坐标**（窗口点击经不抖的相机换算：点击对的是世界本体，抖屏只是视觉装饰），`entity` 是命中实体的名字（无名实体给句柄文本，空地是 null），命中规则和检查器点选/`render/describe` 同一套（含 `Sprite.rot` 旋转形状）。规则照常写：触发 `{"event": "mouse"}`，条件/取值用 `event.x` / `event.y` / `event.entity`，过滤用 `"filter": {"entity": "card"}`。
- **两个入口同一条管道**：窗口里人点的，和 agent 调 `input/click {x, y, button?}`（直接给世界坐标）注入的，走完全相同的拾取+注入路径——人和 AI 是同级玩家；RPC 返回里直接带拾取结果，无头 agent 不用先 describe 再算坐标。
- **录像语义**：点击走回复通道（和 LLM 回复同级的录制通道），连同 tick、拾取结果一起进录像（`Recording.replies`）、重放原 tick 原样注入、快照含未消化的点击——点击驱动的局照样离线逐位重放，**录制中点击照常放行**。鼠标游戏的通关录像可以全程用 `input/click` 经 RPC 打出来，照常过 gate。
- **同一击两个含义**：窗口里左键点击在注入 `mouse` 事件的同时照旧驱动检查器点选/拖拽（青色描边、`inspect/selection`）。检查器只在窗口模式存在，游戏不想要这层行为忽略选中态即可；右键不动检查器。
- **边界**：鼠标**位置本身不是事件**——逐 tick 上报光标会把录像灌爆，悬停高亮 v1 不在范围内；引擎只对"按下"发事件（不发 release，点击语义一次一发）。

## 音效

约定事件：规则/脚本 `{"emit": "play-sound", "data": {"sound": "coin.wav", "volume": 0.6}}`，引擎播放项目 `sounds/` 目录下的文件（wav/ogg/mp3/flac）。`volume` 可选，0..=1，默认 1.0；越界或非数字会在 stderr 上报结构化 `audio_error` 行（不崩游戏，也不静默截断）。

背景音乐：`{"emit": "play-music", "data": {"sound": "bgm.ogg", "volume": 0.4}}` 循环播放；全局只有一个音乐槽，再发一次 play-music 就换歌（旧的先停再起新的），音乐跨 tick 持续播。`{"emit": "stop-music", "data": {}}` 停掉当前音乐（没在播也合法）。

音频是纯输出副作用不进模拟，确定性回放不受影响；无声卡环境（容器/CI）启动横幅会标 `audio: disabled` 但事件照常流动。`vitric check` 会静态校验 play-sound / play-music 字面引用的文件存在。

## 运行时 LLM

游戏逻辑可以在运行时向 LLM 要内容（NPC 台词、生成式描述），**不破坏确定性回放**。

**配置**只认环境变量（密钥不进项目数据）：`VITRIC_LLM_URL`（OpenAI 兼容 chat/completions 端点，如 `https://api.openai.com/v1/chat/completions`）、`VITRIC_LLM_KEY`、`VITRIC_LLM_MODEL`。配齐了启动横幅标 `llm: ok (model …)`；缺任何一个标 `llm: disabled: 未配置 VITRIC_LLM_URL/KEY/MODEL`——此时提问会**立刻**收到显式的 `llm-error` 回复，不是静默没下文。

**约定事件**：
- 提问：规则/脚本 `{"emit": "llm-ask", "data": {"id": "npc-1", "prompt": "..."}}`。`id` 是游戏逻辑自选的关联键，回复原样带回，用来对回提问方。
- 回复：引擎注入 `llm-reply {id, text}`；任何失败（未配置/网络/响应格式不对）注入 `llm-error {id, message}`。回复哪个 tick 到取决于网络快慢，规则按事件响应，别假设固定延迟。

**确定性故事**：HTTP 在引擎的一个后台工作线程里排队串行执行，模拟循环从不等网络；回复经 `Sim::inject_reply` 进入模拟——这是和按键输入同级的**录制通道**：回复内容连同被消化的 tick 一起写进录像（`Recording.replies`），快照也包含未消化的回复。所以 `vitric replay` 重放带 LLM 内容的录像时，llm-ask 无人监听、回复全部从录像注入，**重放永远不碰网络**，离线逐位复现原局。

NPC 对话最小写法（用 `filter: {"id": ...}` 把回复对回提问方）：

```json
{"rules": [
  {"id": "npc-greet", "on": {"event": "input", "filter": {"action": "e", "phase": "pressed"}},
   "do": [{"emit": "llm-ask", "data": {"id": "npc-1", "prompt": "你是玻璃镇铁匠，对路过的玩家说一句话"}}]},
  {"id": "npc-say", "on": {"event": "llm-reply", "filter": {"id": "npc-1"}},
   "do": [{"set": "@npc.Text.content", "to": "event.text"}]},
  {"id": "npc-fail", "on": {"event": "llm-error"},
   "do": [{"set": "@npc.Text.content", "to": "event.message"}]}
]}
```

## 引擎约定组件

内建系统只认这些名字：`Position{x,y}` + `Velocity{x,y}` → 每 tick 积分移动；
`Position` + `Collider{w,h}` → AABB 碰撞发 `collision` 事件；
`Position` + `Sprite{w,h,color,image,rot}` → 渲染；`Camera{x,y,scale}` → 取景。
`Sprite.rot` 可选（度数）：精灵绕自身 Position 旋转，世界空间逆时针为正（画面上看也是逆时针），缺省 0 = 不旋转；屏上文字（Text）永远直立不旋转，点选（pick）按旋转后的真实形状命中。
游戏感组件（Camera 的 follow/lerp、`Shake`、`Particle`）见下面「游戏感」一节；批量粒子（`Emitter`）见「粒子发射器」一节。

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
- **粒子**：实体挂 `Particle{ttl}`（剩余 tick 数，整数），引擎每 tick 减 1，到 0 自动销毁（销毁顺序 = 槽位序，确定性）。五彩纸屑/尘土/爆炸 = spawn 一批 Sprite+Velocity+Particle 然后不管，不用写清扫规则。批量持续的火花/烟雾别用它——那是下面「粒子发射器」的活（零实体开销、零状态）。

## 粒子发射器 / Particle emitter

`Emitter` + `Position`：批量粒子（火把火花、喷泉、爆发）。与 spawn 实体的 `Particle{ttl}` 本质不同——**粒子是纯渲染层产物，不是实体、不进模拟状态**：每个粒子在第 T tick 的位置/颜色/大小是 `f(发射器字段, 粒子序号, T, 实体id派生的种子)` 的纯函数，无积分器（解析式 `pos = origin + v0·t + ½g·t²`）、无跨帧状态。所以：发射器字段本身照常进状态哈希/存档（它是普通组件），但几百个粒子**不带来任何额外状态**——录像重放、快照回退（sim/restore）后粒子画面自动逐字节正确。每粒子随机（方向/初速）用实体 id ⊕ 粒子序号的确定性散列（SplitMix64），不碰模拟的随机数流。

字段（缺/错显式报错；上限 64 个发射器、单发射器同屏 1024 粒）：

- `kind`（必填）：`"stream"`（持续流，按 `rate` 粒子/秒，发射时间轴从 tick 0 起算）或 `"burst"`（单次爆发）。
- `lifetime`（必填）：粒子寿命（tick，整数 ≥ 1）。`size`（必填）：起始大小（世界单位 > 0）。
- stream 用 `rate`（粒子/秒，> 0）；burst 用 `count`（≥ 1）+ `burst`（触发 tick 号，负数 = 未触发，缺省 -1）。**触发爆发 = 规则把当前 tick 写进 burst 字段**——爆发期（burst ≤ T < burst+lifetime）由字段值纯函数推出，渲染层不记历史；规则写字段被录像/快照如实捕获，重放自动复现。
- `speed_min`/`speed_max`：初速范围（世界单位/秒，缺省 0；speed_max 缺省 = speed_min）。
- `dir`：发射朝向（度数，0 = +x、逆时针为正——和 Sprite.rot/Light.dir 同一约定，缺省 0）；`spread`：扩散角全宽（0..=360，缺省 360 = 全方向）。
- `gravity`：重力加速度（世界单位/秒²，作用在 y 轴，通常负数；缺省 0）。
- `color`/`color_end`：起始/结束颜色（color_end 缺省/空串 = 不渐变）；alpha 内建随寿命线性淡出（255 → 0）。`size_end`：结束大小（≥ 0，0 = 缩小到消失；字段缺失 = 同 size 不渐变）。
- `active`：开关（缺省 true）。false = 一个粒子都不画——纯函数的取舍：中途关掉，在途粒子当帧消失（画面只看当前字段值）。

与光照的协作（**简化约定，写明白省得猜**）：粒子**自发光**——不被 Ambient 压暗、不被灯衰减、不投影也不受影；画在光照之后、泛光之前，所以亮粒子配 `Bloom` 照常晕开（火花"真的在烧"）。CPU 截图画方点，GPU 窗口画同几何的方块——位置/数量/颜色两条路径同一份数据。已知取舍：发射器实体移动时，所有在途粒子整体跟着移（位置相对当前原点——无状态的代价）；stream 的发射时间轴从 tick 0 起算（中途 spawn 的发射器一出场就是稳态）。

`render/describe` 按发射器汇总一行（粒子不逐个列）：`- 发射器 lantern-sparks: stream 活跃，~11 粒子可见（世界 47,2.9）`，并给 `emitters[]` 结构化字段（kind/active/rate 或 count+burst/lifetime/visible_estimate）。

schema 定义（字段名固定，默认值自己定）+ 场景写法（完整示例见 examples/glow 的 lantern-sparks）：

```json
"Emitter": {"fields": {
  "kind": {"type": "enum", "variants": ["stream", "burst"]},
  "rate": {"type": "number", "default": 0, "min": 0},
  "count": {"type": "int", "default": 0, "min": 0},
  "burst": {"type": "int", "default": -1},
  "lifetime": {"type": "int", "default": 30, "min": 1},
  "speed_min": {"type": "number", "default": 0, "min": 0},
  "speed_max": {"type": "number", "default": 0, "min": 0},
  "dir": {"type": "number", "default": 0},
  "spread": {"type": "number", "default": 360, "min": 0, "max": 360},
  "gravity": {"type": "number", "default": 0},
  "color": {"type": "text", "default": "#ffffff"},
  "color_end": {"type": "text", "default": ""},
  "size": {"type": "number", "default": 0.3, "min": 0},
  "size_end": {"type": "number", "default": 0, "min": 0},
  "active": {"type": "bool", "default": true}
}}
```

```json
{"name": "torch-sparks", "components": {"Position": {"x": 10, "y": 4},
  "Emitter": {"kind": "stream", "rate": 12, "lifetime": 55, "dir": 90, "spread": 55,
              "speed_min": 0.8, "speed_max": 2.2, "gravity": -1.2,
              "color": "#ffd75e", "color_end": "#ff5a20", "size": 0.22, "size_end": 0}}}
```

触发爆发要写"当前 tick"，规则动作没有 tick 变量——用脚本系统（`ctx.tick`）：

```js
// 命中标记（规则 set @boom.Hit.flag = true 之类）后，脚本把当前 tick 写进 burst
vitric.system("fire-burst", { query: ["Emitter", "Hit"], writes: ["Emitter", "Hit"] }, (entities, ctx) => {
  for (const e of entities) {
    if (e.Hit.flag) { e.Emitter.burst = ctx.tick; e.Hit.flag = false; }
  }
});
```

## 演出层：补间 · 序列（时间轴）

定时演出（淡入、推镜、打字机、开场、Boss 登场、结局演职员表……）由两块通用原语组合，对标 Unity Timeline / Godot AnimationPlayer。**引擎不提供"过场系统"——过场是序列的一种用法**，活在游戏项目里，引擎侧一个"过场""漫画页""卡牌"的字样都没有。动作集只有引擎已有的通用动词，题材专属概念全靠这套积木拼。

**补间 `Tween`**：数值从 A 平滑变到 B 的确定性插值底座（淡入淡出、镜头推拉、UI 弹出、颜色渐变、位移缩放全靠它）。独立实体挂 `Tween{target, field, from, to, duration, ease, start, id}`（schema 定义这些字段，`target` 是目标实体名/句柄、`field` 是 `"组件.字段"` 路径）。曲线固定五条：`linear` / `ease-in` / `ease-out` / `ease-in-out` / `ease-out-back`（末端过冲回弹）。第 T tick 的值是 `from + (to-from)·ease(elapsed/duration)` 的**解析式**（禁累加，快照回退续播逐位一致）；到期那刻精确写终值（不留浮点尾巴）+ 发 `tween-finished {id, target, field}` 事件 + 补间实体自动移除。同实体同字段同时只一个活跃补间，后来者顶掉前者。状态全在组件里，进哈希/存档/快照。

**序列 `Sequence`（通用时间轴）**：一条按相对 tick 推进的动作轨道。序列定义是项目静态数据（`sequences/<名>.json`，清单 `sequences` 列表声明），形态 = 有序条目 `{ "at": <相对 tick>, "do": <动作> }`，`at` 相对序列**起跑点**（同一序列任何时刻起跑都从自己的 t=0 放，区别于规则的绝对 tick 触发），必须按 `at` 单调不减。运行时一个 `Sequence` 组件实例化它（schema 定义 `track`/`cursor`/`start`/`wait`/`id` 五字段），**只持有最小播放状态**——引用哪条序列 + 游标下标 + 起跑 tick + barrier 等待标志；静态轨道不进每实例快照，快照便宜。动作集 v1 固定（不图灵完备、不嵌脚本），全部镜像引擎已有动词：

- `tween`：起一个补间。镜头推拉 = tween 相机字段，淡入淡出 = tween alpha/颜色字段，位移/缩放全靠它。**序列负责编排，补间负责执行**，零重复。
- `set`：瞬时设字段（`{"set": "@字幕.Text.content", "to": "..."}`，镜像规则 set）。
- `spawn` / `despawn`：生成/销毁实体（镜像规则）。插画入场 = spawn 一个带 Sprite 的实体。
- `emit`：发事件让规则/脚本接龙。**序列借此与"场景"解耦**——要切场景就 `{"emit": "load-scene", "data": {"scene": "scenes/next.json"}}`，由项目里一条规则接去 load-scene；序列本身不认识"场景""关卡"。
- `sound`：播音效（`{"sound": "chime.wav", "volume": 0.6}`，翻成 play-sound 事件，和规则同一条音频通道）。
- `wait`：barrier——游标停住，直到某命名事件出现（`{"wait": "player-confirm"}`，由一条规则把玩家输入翻成该事件）或 `skip` 输入到达才放行。这是规则不易表达的状态机能力，序列存在的另一半理由。

序列跑到末尾发 `sequence-finished {id, track}` 事件、序列实体自动移除（后续切场景由规则接，不内置）。**跳过**：一条 `skip` 输入（`input/inject {action: "skip"}`）把未执行条目的终态全部落定后发完成事件——skip 是输入、进录像、重放一致。整段推进是普通系统按 tick 走（无墙钟无随机），snapshot/restore 中途回退续播逐位一致。`vitric check` 验序列：`at` 单调不减、动作名在固定集合、动作字段过 schema、spawn 的字面贴图 / sound 的字面音效 / emit load-scene 的目标场景都存在——错误带路径 + VDxxx 码 + 修复提示。

为什么序列不是规则的重复：规则触发器是**绝对 tick**、无游标、无 barrier；序列给的是**相对起跑的有序轨道 + 等待点**，这正是 Timeline 比裸脚本多出来的那层。序列编排"一条定时演出"，规则处理"条件反应"。

schema 定义（字段名固定，默认值照抄）：

```json
"Sequence": {"fields": {
  "track":  {"type": "text", "default": ""},
  "cursor": {"type": "int",  "default": 0},
  "start":  {"type": "int",  "default": -1},
  "wait":   {"type": "text", "default": ""},
  "id":     {"type": "text", "default": ""}
}}
```

漫画过场 = 拿原语拼出来（**完整可跑示例见 examples/intro**，纯色占位精灵 + 字幕打字机，引擎零过场代码）：

```json
{ "id": "opening", "steps": [
  { "at": 0,   "do": { "spawn": { "name": "illustration", "components": {
                         "Position": {"x": 0, "y": -12}, "Sprite": {"w": 6, "h": 6, "color": "#5b6ee1"} } } } },
  { "at": 0,   "do": { "sound": "chime.wav", "volume": 0.6 } },
  { "at": 0,   "do": { "tween": { "target": "illustration", "field": "Position.y",
                         "from": -12, "to": 1.5, "duration": 30, "ease": "ease-out" } } },
  { "at": 30,  "do": { "tween": { "target": "camera", "field": "Camera.view_h",
                         "from": 18, "to": 15, "duration": 120, "ease": "ease-in-out" } } },
  { "at": 30,  "do": { "tween": { "target": "subtitle", "field": "Text.reveal",
                         "from": 0, "to": 1, "duration": 60 } } },
  { "at": 150, "do": { "wait": "player-confirm" } },
  { "at": 151, "do": { "tween": { "target": "illustration", "field": "Position.y",
                         "from": 1.5, "to": -12, "duration": 20, "ease": "ease-in" } } },
  { "at": 171, "do": { "emit": "intro-done" } }
]}
```

换成技能演出、Boss 登场、结局演职员表，全是同一套积木换条 Sequence——这才是通用引擎。

## UI 控件（布局）/ UI widgets (layout)

菜单/背包/设置/HUD 这类界面用一套**声明式、确定性**的控件原语拼，对标 Godot Control / Unity UI。**引擎只给通用控件原语；具体界面（主菜单长什么样、背包几列）是项目用积木拼的用法**，引擎侧没有"对话框""技能栏""结算面板"的字样——和序列同一条原则。UI 也是实体 + 组件，进世界状态/哈希/存档/录像；布局是 `(UI 树, 视口)` 的确定性纯计算（无墙钟无随机），快照/回放往返一致。

**当前阶段 = 1.2：在 1.1 布局之上加交互（Button 状态机 + 焦点导航 + 点击激活 + `ui-activate` 事件）和主题 Theme**。1.1 的布局地基（锚点、VBox/HBox/Grid 容器、Panel、Label、脏标记、屏幕空间渲染）见下，交互/主题见本节末尾两小节。**不做 hover**（v1 只有焦点导航 + 点击双轨；hover 是纯鼠标概念、手柄/触屏没有，留 v2）。

**坐标系：UI 是屏幕空间叠加层**。UI 元素锚定**视口（屏幕）**、渲染走屏幕空间正交投影，**不经相机变换**——镜头移动/缩放/抖动时 UI 不飘（像 HUD）。这与精灵/粒子走世界空间相反。UI 紧接世界渲染（含光照/粒子/泛光）之后叠加，不被打光，无离屏缓冲（复用同一顶点流）。CPU 真相源 + GPU 镜像，两路读同一份布局结果（`solve_layout`），不在 GPU 侧重算布局。

约定组件（引擎认名字，字段自己在 schema 里定义）：

- `UiRoot{layout_hash}`：标记一棵 UI 树的根，布局从它起、对着视口解算。**场上没有 UiRoot = 完全没有 UI = 每 tick 零成本 early-return**（零分配零遍历）。`layout_hash`（可选文本字段）缓存上次布局输入的结构哈希——结构/尺寸没变就跳过重算（静止 UI 连播 N tick，布局重算 0 次）。声明它才享受脏标记；不声明退化成每 tick 解算，仍正确只是不省那一趟。
- `Ui{anchor, ax, ay, ox, oy, w, h, parent, weight, rx, ry, rw, rh}`：每个 UI 节点。
  - `anchor`：锚点预设名——`top-left`/`top-center`/`top-right`/`center-left`/`center`/`center-right`/`bottom-left`/`bottom-center`/`bottom-right`（贴四角/四边/居中）、`stretch`（拉伸填满父框，`ox`/`oy` 当四边内缩、忽略 `w`/`h`）、`manual`（用自己的 `ax`/`ay` 当父框内 0..1 比例锚点）。
  - `ox`/`oy`：像素偏移；`w`/`h`：尺寸（像素，stretch/容器拉伸时被覆盖）。
  - `parent`：父 UI 节点的实体引用（`entity` 类型；空 = 锚到视口）。
  - `weight`：容器主轴拉伸权重（0 = 用自身尺寸；>0 = 按权重瓜分剩余主轴空间）。
  - `rx`/`ry`/`rw`/`rh`：**布局输出**——解算后的屏幕像素矩形（左上原点）。布局系统写、渲染读，进哈希进存档（快照/录像安全）。这些是引擎填的，作者写 0 占位即可。
- `Container{kind, gap, pad, columns, main, cross}`：挂了它，子节点（`parent` 指向本实体的 Ui 节点）由容器**自动排版**，子节点不自摆坐标。`kind` ∈ `{VBox, HBox, Grid}`（竖排/横排/网格）；`gap` 子间距、`pad` 四边内边距；`Grid` 用 `columns`（≥1，行高列宽等分）；`main`/`cross` 主轴/交叉轴对齐（`start`/`center`/`end`）。
- `Panel{color, image}`：背景框。`color` 纯色（支持 `#rrggbb` 或带透明度 `#rrggbbaa`），或 `image` 精灵贴图（最近邻缩放）。**NinePatch 九宫格留 1.2**（纯色 + 精灵 1.1 必做）。
- `UiLabel{content, size, color, reveal, align}`：文字控件，复用矢量字体版面缓存 + 逐字显示（`reveal` 0..1，同 `Text.reveal`）。`size` = **屏幕像素**字号（不经相机）；`align` 在节点框内水平对齐（`start`/`center`/`end`），竖向居中于框。

布局是脏标记 + 一趟树遍历（O(UI 节点数)），不每 tick 全量。布局参照视口在模拟状态里固定为 1920×1080（解算结果进哈希要与窗口分辨率解耦，跨机器确定）；渲染时 CPU/GPU 各自按真实窗口分辨率重解算（同一份 `solve_layout` 纯函数），UI 锚定视口自然缩放。

`vitric check` 验 UI：锚点预设名合法（`VD070`）、容器类型在 `{VBox,HBox,Grid}`（`VD071`）、Grid 列数 ≥1（`VD072`）、对齐名合法（`VD073`）、`Panel.image` 引用的贴图存在（和 `Sprite.image` 同口径）、字段过 schema——错误带路径 + VDxxx 码 + 修复提示。

**完整可跑灰盒示例见 examples/ui-gallery**：居中菜单面板 + 标题 + 三个按钮（VBox 竖排，每个按钮 = 一个 Panel + 一个 stretch 的 UiLabel）+ 底部提示。一个主菜单的样子，全是上面的积木拼出来的，引擎零界面代码。场景写法（节选）：

```json
{ "entities": [
  { "name": "ui", "components": { "UiRoot": {} } },
  { "name": "menu-panel", "components": {
      "Ui": { "anchor": "center", "w": 600, "h": 420, "parent": "ui" },
      "Panel": { "color": "#1b1d26" } } },
  { "name": "menu-vbox", "components": {
      "Ui": { "anchor": "stretch", "ox": 40, "oy": 110, "parent": "menu-panel" },
      "Container": { "kind": "VBox", "gap": 24, "main": "start", "cross": "center" } } },
  { "name": "btn-start", "components": {
      "Ui": { "anchor": "top-left", "w": 480, "h": 72, "parent": "menu-vbox" },
      "Panel": { "color": "#3a4a6b" } } },
  { "name": "btn-start-label", "components": {
      "Ui": { "anchor": "stretch", "parent": "btn-start" },
      "UiLabel": { "content": "Start", "size": 30, "color": "#ffffff", "align": "center" } } }
] }
```

换成背包格子（Grid + columns）、设置项列表（VBox）、角落小地图（bottom-right 锚点），全是同一套积木——题材专属界面是项目用法，不在引擎里。

### 交互：焦点导航 + 点击（双轨，不做 hover）

可交互界面在 1.1 的布局之上加 `Button` 组件。**两条激活轨道，发同一个 `ui-activate {id, action}` 命名事件**，规则/序列接（切场景、开背包都走 emit→规则，UI 不内置题材动作）：

- **焦点导航**：可聚焦按钮组成焦点环，方向输入 `ui-up`/`ui-down`/`ui-left`/`ui-right`（标准 `input/inject`）按**布局相邻关系**（矩形几何，对标 Godot 方向焦点）移焦点，到边停住不环绕；`ui-confirm` 激活当前焦点按钮。窗口模式下方向键/回车在有 UI 时自动注入 `ui-*`（游戏自己的 left/jump 不受影响）。
- **点击激活**：注入屏幕**归一化坐标** `(nx, ny) ∈ [0,1]`（`input/ui-click` RPC，或窗口鼠标自动换算 = 物理像素 / 视口尺寸），运行时把它乘回参照系 1920×1080 再判断落在哪个按钮矩形（`rx/ry/rw/rh`）内——命中 = 激活，顺带把焦点移过去。**注意这和世界点击 `input/click` 是两套坐标系**：世界点击拾取 Sprite（经相机），UI 点击拾取屏幕空间叠加层（不经相机）。归一化 + 参照系换算让命中判定与真实分辨率解耦，点击走回复通道进录像、重放逐位一致。
- **按名激活**：`input/ui-click-by-name {name, button?}` — 直接按场景里的实体名（如 `"mode_craft"`、`"craft_plank"`）激活按钮，跑和坐标点击同一条 `activate_button` 路径（设 pressed + press_t + 移焦点 + emit `ui-activate`）。**布局无关**：改 `mode_row` 的 gap 或按钮宽度不会让脚本失效。比坐标点击**更严格** —— name 不存在 / 实体没有 `Button` 组件 / 按钮 `Disabled` 都直接报错（坐标点击是边界外静默不命中）。录像里存 `{name, button}`，name 是确定性场景状态，重放逐位一致。Agent 脚本优先用 by-name，坐标点击留给"我不知道按钮叫什么，只知道大概位置"的探索场景。

`Button{action, theme, state, press_t, min_scale}`：

- `action`：激活时 `ui-activate` 带的 action 名（非空——空 action 没规则能接，check 红灯）。规则按 `{"event":"ui-activate","filter":{"action":"start"}}` 接。
- `theme`：引用的主题名（见下小节；空 = 不套主题，保留场景写死的 `Panel.color`）。
- `state`：状态机当前态 ∈ `normal`/`focused`（被焦点选中，高亮）/`pressed`（激活那几 tick 的反馈）/`disabled`（不可聚焦、不响应点击/确认）。**引擎写**，作者只设初始态（如菜单第一项写 `focused`）。
- `press_t`：按下反馈计时（-1 = 不在反馈中；引擎写）。按下反馈 = **scale + modulate**（缩放 + 提亮），是 `press_t` 的**解析式**纯函数（`press_scale`/`press_modulate` 的三角包络，禁累加）——渲染装饰，快照回退续播一致。
- `min_scale`：按到最深时的缩放（缺省 0.92 = 缩到 92%）。

当前焦点存在 `UiRoot.focus`（实体名，引擎写、进哈希进存档）。焦点态/`state`/`press_t` 全进组件 → 快照/录像安全，snapshot/restore 中途回退焦点态续播一致。焦点/点击判定 O(可聚焦按钮数)，不全表扫；空 UI 仍零成本。

### 主题 Theme（换肤）

`themes/<名>.json`，清单 `themes` 列表声明，控件 `Button.theme` 按名字引用。一处定义、全局引用，换肤 = 换一份引用。**主题是装配期常量，不进世界状态**；运行时按 `Button.state` 从主题取该状态底色写进 `Panel.color`（渲染只读 `Panel.color`，不依赖主题表）。

```json
{
  "colors": { "bg": "#1b1d26", "text": "#f0f0f0", "focus": "#5a7bb5", "disabled": "#555555" },
  "font_size": 30, "padding": 12, "margin": 24,
  "button": {
    "normal":   { "bg": "#3a4a6b", "text": "#e8ecf4" },
    "focused":  { "bg": "#5a7bb5", "text": "#ffffff" },
    "pressed":  { "bg": "#9fc0f0", "text": "#10131a" },
    "disabled": { "bg": "#2a2d36", "text": "#6b6f7a" }
  }
}
```

`colors` 是全局颜色卷，`button.<state>` 是四态背景/文字色（只写 `colors` 也行，四态会从 `colors` 推全：focused→focus 色、disabled→disabled 色、其余→bg）。`vitric check` 验交互/主题：按钮状态合法（`VD074`）、action 非空（`VD075`）、`Button.theme` 引用的主题存在、主题颜色是合法 `#rrggbb(aa)`（`VD081`）、字号/边距非负（`VD082`）、按钮状态名合法（`VD083`）、`UiRoot.focus` 引用的实体存在（entity 字段的 `VD033`）。

**完整可跑交互示例见 examples/ui-menu**：三按钮竖排菜单（Start/Options(disabled)/Quit），focus-nav + mouse-click 两条 gate 录像都激活"开始"→ emit `game-started` → 规则 `load-scene` 切到 game 场景，逐位重放一致。按钮带 `Button{action, theme:"dark", state}`，主题在 `themes/dark.json`。

## 光照 / Lighting

跟 Body/Solid 一样的约定组件：引擎认名字，字段自己在 schema 里定义。

- **总开关 = Ambient 实体的存在**。场上没有带 `Ambient` 组件的实体 = 完全不跑光照（旧行为、零开销）；有一个（取第一个）= 光照管线启动，整帧打光。
- `Ambient{color, shadows}`：场景环境光底色，如暗色洞穴 `"#202838"`；`"#ffffff"` = 无灯处保持原样。`shadows` 可选 bool（缺省 false），见下面"2D 投影"。
- `Light{radius, color, intensity, kind, angle, dir}`：光源，三种 `kind`（缺省 `"point"`，未知值显式报错列出可选项）。**三种合计上限 64 盏**，超了显式报错（不静默截断）。
  - `"point"`（点光源，需要 `Position`）：radius 世界单位（到 radius 处衰减为零）；color 缺省 `"#ffffff"`、intensity 缺省 1.0。不写 kind = 点光源 = 旧行为，输出字节不变。
  - `"spot"`（聚光灯，需要 `Position`）：点光源全部字段，外加必填 `angle`（锥角全宽，度数，1..=360）和必填 `dir`（朝向，度数，世界空间，0 = +x、逆时针为正——和 `Sprite.rot` 同一个角度约定）。
  - `"directional"`（平行光）：必填 `dir`（光**行进**的方向，度数，约定同上）+ color/intensity。不读 Position/radius——太阳在无穷远，处处同亮（没有法线贴图的像素 dir 不参与计算；有法线的像素按 dir 出方向感，见下）。
- 公式（CPU 截图和 GPU 窗口同一套）：`lit = min(ambient + Σ 各灯贡献, 1.5)`，`out = min(场景色 · lit, 1.0)`。各灯贡献：point = `灯色·intensity·(1 - d/r)²`（d < r 才有）；spot = point 公式再乘角度衰减 `t²`，`t = clamp(1 - Δθ/(angle/2), 0, 1)`（锥心 1、锥边 0；Δθ 是像素方向与 dir 的夹角）；directional = `灯色·intensity`（处处均匀）。1.5 的上限允许轻微过曝（廉价泛光感）。
- **法线贴图（零配置命名配对）**：精灵贴图 `hero.png` 在 assets/ 里有 `hero_n.png` 就自动启用，没有就完全是旧行为（字节锁死）。RGB 编码切线空间法线（`n = rgb/255·2-1`，z 强制朝外归一化；xy 对齐屏幕像素空间——x 右、y 下）；采样用和漫反射同一套 UV，`Sprite.rot` 转精灵时法线跟着转。有法线的像素各灯贡献额外乘 `max(dot(N, L), 0)`：L 的 xy 取像素指向灯的单位方向 ×0.8、z 固定 0.6（平面法线在灯正下仍有六成贡献，不会"开了法线反而全黑"）；平行光的 L = (−行进方向·0.8, 0.6)——配对法线后平行光有了方向感。生成法线贴图见 `vitric assets --normals`（docs/art-pipeline.md ⑤）。
- **2D 投影（shadow casting）**：`Ambient` 上加 `"shadows": true` 开启（缺省 false = 完全不跑，输出字节不变）。遮光体 = 带 `Solid`+`Position`+`Collider` 的实体——Solid 本来就是"挡"（挡停身体），开了投影就顺便挡光，**不需要任何新组件**；上限 256 个，超了显式报错。逐像素逐灯：像素到灯心的线段穿过任何遮光体的碰撞盒就把这盏灯的贡献清零（硬影，无半影）。**自遮挡规则：像素落在某个遮光体内部时，那个遮光体不挡它**——只被别的遮光体遮挡，所以墙体自己仍被照亮、不会变成黑块。只有 point/spot 投影；**directional 在 v1 不投影**（平行光照旧处处均匀）。灯心别埋进 Solid 里——埋进去的灯照不出那面墙。开启时 `render/describe` 多给 `shadows: true` + `occluders`（遮光体数量）+ 一行摘要。性能：边缘正好贴齐的相邻遮光体（瓦片地板）每帧自动合并成大箱、再按灯的半径逐灯剔除——**输出字节不变**，但瓦片摆贴齐会快得多。GPU 窗口路径另有 uniform 预算：单盏灯半径内合并后 ≤ 64 箱、全部灯合计 ≤ 256 条，超了显式报错（减灯、减半径，或把瓦片摆贴齐让它们合并）。
- **所有东西一视同仁被打光**——精灵、文字、背景，屏幕锚定的 HUD 也不豁免。HUD 要保持可读，自己在旁边放盏灯或调亮 Ambient。
- 光照确定性：只读组件状态，同一世界同一 tick 渲出的字节逐位相同；`render/screenshot` 含光照——agent 截到的就是玩家看到的。
- `render/describe` 在光照开启时多给 `ambient`（环境色）和 `lights` 数组（id/name/kind/世界坐标/radius/intensity/color，聚光灯多 angle/dir、平行光多 dir 且无坐标/radius）+ 一行摘要——光照设置全部可文字化观察。
- **泛光（Bloom）**：挂一个带 `Bloom{threshold, strength}` 组件的实体（取第一个，同 Ambient）就开启全屏泛光后效——亮处向四周晕开光圈，配合点光源就是"真的在发光"。threshold ∈ [0,1]：通道值超过 threshold·255 的部分进泛光；strength ≥ 0：叠加倍率。两个字段必填。公式：`bright = max(场景色 - threshold·255, 0)`，盒式模糊（水平/垂直可分离、3 次迭代近似高斯），`out = min(场景色 + blurred·strength, 255)`。模糊半径 = 视口高/90、下限 2 像素——光晕占画面比例与分辨率无关。泛光在光照之后跑；没有 Bloom 实体 = 完全不跑（零开销，字节不变）。开启时 `render/describe` 多给 `bloom` 字段 + 一行摘要。

```json
{"name": "torch", "components": {"Position": {"x": 10, "y": 4},
  "Light": {"radius": 6, "color": "#ff9040", "intensity": 1.2}}}
{"name": "beam", "components": {"Position": {"x": 0, "y": 8},
  "Light": {"kind": "spot", "radius": 10, "angle": 50, "dir": 270, "color": "#ffffcc"}}}
{"name": "sun", "components": {
  "Light": {"kind": "directional", "dir": 300, "color": "#fff4e0", "intensity": 0.4}}}
```

## 屏上文字

`Text{content, size, color}` + `Position`：整串居中于 Position，画在精灵之上。`render/describe` 直接给出 `texts[].content`——agent 不用从截图认字。
数字状态转文字用规则的 format 模板：`{"set": "@hud.Text.content", "to": {"format": "SCORE {}", "args": ["self.Score.value"]}}`（`{}` 与 args 个数必须一致）。

两条渲染路径，清单 `font` 字段二选一：

- **默认（不写 font）**：内嵌 8x8 点阵字体（ASCII），每字符 size×size 世界单位、等宽、硬边像素——像素风游戏的正解，输出字节与该功能出现之前逐位相同（测试锁死）。非 ASCII 字符画实心方块占位。
- **清单写 `"font": "fonts/myfont.ttf"`（路径相对项目根目录）**：**所有** Text 改走 TTF 矢量字体——比例字距 + 字距调整，size = 字形总高的世界单位数（像素高 = size×相机 scale），字体里有的字形都能画（**中文/CJK 也行，前提是字体本身含 CJK 字形**——DejaVu 这类拉丁字体没有，中文会画成字体自带的 .notdef 豆腐块；要中文请换 Noto Sans SC 等）。矢量文字带覆盖率**抗锯齿**——这是引擎里唯一刻意平滑的元素，精灵贴图仍是最近邻硬边。手绘/高清画风、运行时 LLM 中文回复都走这条路（示例见 examples/book）。
- 字体文件缺失/损坏：`vitric check` 和启动都显式报错点名路径，不会跑起来文字消失。
- 确定性不变：CPU 截图（render/screenshot）同平台同二进制逐字节相同，照常可进断言；GPU 窗口与 CPU 视觉对齐但不逐字节（截图真相源永远是 CPU 路径）。

**文字显隐 `Text.reveal`（逐字显示 / 打字机）**：给 `Text` 加一个可选 `reveal` 字段（schema 里 `"reveal": {"type":"number","default":1}`，0..=1 比例），渲染只画到该进度的字符——`reveal=1`（或不写该字段）全显、`reveal=0` 一个字不画、`0.5` 显前一半（按字符数下取整，CJK 一次显一个字形）。它是**通用文本属性，不绑任何"过场"**：谁来驱动都行——打字机 = 用补间把 reveal 从 0 推到 1（`{"tween": {"target": "subtitle", "field": "Text.reveal", "from": 0, "to": 1, "duration": 60}}`）、瞬显 = `set` 成 1、倒着隐 = 补间回 0。`reveal` 缺省或 ≥1 时输出与该字段出现之前逐字节相同（向后兼容）。性能上整段版面只算一次（按文字 id memo 缓存），逐字显示每 tick 只改"画到第几个字"，绝不重排——再长的台词一字一字蹦也不掉帧。

**可读性警告（describe 的 `warnings`）**：屏上每条文字，`render/describe` 会在内部把"少画这条文字"的画面渲一帧，取文字包围盒内的平均背景亮度，与 `Text.color` 算 WCAG 式对比度 `(L1+0.05)/(L2+0.05)`；低于 2.5 给一条 `{"kind": "low-contrast-text", "entity": ..., "content": ..., "ratio": ..., "hint": ...}`，中文摘要里同步一行 `⚠ 文字"XX"与底色对比度过低`。米色字叠米色卡面这种"渲染正常、人眼读不出来"的事故由它兜住。只测屏内文字；视野外不测。没警告时没有 `warnings` 键。已知近似：文字色取原始值、底色取打光后像素（开光照/泛光时比值略有偏差，阈值已留余量）。

## 贴图引用的静态扫描（check）

场景/动画里的贴图引用 `vitric check` 一直会查；**脚本和规则动态 spawn 的贴图**也查：脚本源码里的字面 `.png` 引用（`image: "dust.png"`、`"image": "dust.png"`、单引号同理）和规则 `spawn` 动作里 `Sprite.image` 的字面值，每个都必须在 assets/ 里，缺了 check 红灯并点名文件+贴图名。诚实的局限：这是**字面量 lint** 不是数据流分析——动态拼接（`"dust_" + i + ".png"`）、变量间接引用扫不到，check 绿灯不等于运行期一定有图。所以尽量用字面名引用贴图，让 check 兜得住；运行期缺图仍会显式报错（不画占位符）。
