# Vitric Agent 指南

给 AI agent（和人）的一页纸操作手册：怎么自主地跑、看、测、改一个 Vitric 游戏。

## 四个命令

```bash
vitric check <项目目录>                  # 校验一切（schema/场景/规则/脚本），错误带路径+错误码+修法
vitric run <项目目录> [--port 6173] [--speed X] [--ticks N] [--record 录像.json]
vitric replay <项目目录> <录像.json>      # 重放录像并逐校验点验证确定性
vitric assets <项目目录> [--colors N] [--height H] [--palette-lock]  # 全项目 PNG 统一色板，AI 出图规整成一个调，详见 docs/art-pipeline.md
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

- **录像只记两条外部通道：输入流 + 外部回复（LLM）。** 录制期间 `world/set` / `world/spawn` / `world/despawn` / `project/reload` / `sim/restore` 会被明确拒绝（改了的状态不进录像，录出来必然重放分歧），检查器拖拽也会被禁用。要在录制中影响世界，用 `input/inject`——输入会被录下来。LLM 回复经引擎的 inject_reply 进模拟，同样被录、重放时原 tick 重新注入（见「运行时 LLM」）。
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
实体挂 `Anim` 组件（schema 需定义 clip/prev/t/done 四字段），**引擎独占 Sprite.image 的写权**——换动画唯一正路是改 `Anim.clip`（规则 set 即可），切换自动从头播；非循环片段播完发 `anim-finished` 事件并停末帧。状态全在组件里，快照/回放安全。

## 内建事件

`start`（tick 0，初始化/生成关卡的标准入口）、`input`、`collision`、`anim-finished`。

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
- `Light{radius, color, intensity, kind, angle, dir}`：光源，三种 `kind`（缺省 `"point"`，未知值显式报错列出可选项）。**三种合计上限 64 盏**，超了显式报错（不静默截断）。
  - `"point"`（点光源，需要 `Position`）：radius 世界单位（到 radius 处衰减为零）；color 缺省 `"#ffffff"`、intensity 缺省 1.0。不写 kind = 点光源 = 旧行为，输出字节不变。
  - `"spot"`（聚光灯，需要 `Position`）：点光源全部字段，外加必填 `angle`（锥角全宽，度数，1..=360）和必填 `dir`（朝向，度数，世界空间，0 = +x、逆时针为正——和 `Sprite.rot` 同一个角度约定）。
  - `"directional"`（平行光）：必填 `dir`（光**行进**的方向，度数，约定同上）+ color/intensity。不读 Position/radius——太阳在无穷远，处处同亮（没有法线贴图的像素 dir 不参与计算；有法线的像素按 dir 出方向感，见下）。
- 公式（CPU 截图和 GPU 窗口同一套）：`lit = min(ambient + Σ 各灯贡献, 1.5)`，`out = min(场景色 · lit, 1.0)`。各灯贡献：point = `灯色·intensity·(1 - d/r)²`（d < r 才有）；spot = point 公式再乘角度衰减 `t²`，`t = clamp(1 - Δθ/(angle/2), 0, 1)`（锥心 1、锥边 0；Δθ 是像素方向与 dir 的夹角）；directional = `灯色·intensity`（处处均匀）。1.5 的上限允许轻微过曝（廉价泛光感）。
- **法线贴图（零配置命名配对）**：精灵贴图 `hero.png` 在 assets/ 里有 `hero_n.png` 就自动启用，没有就完全是旧行为（字节锁死）。RGB 编码切线空间法线（`n = rgb/255·2-1`，z 强制朝外归一化；xy 对齐屏幕像素空间——x 右、y 下）；采样用和漫反射同一套 UV，`Sprite.rot` 转精灵时法线跟着转。有法线的像素各灯贡献额外乘 `max(dot(N, L), 0)`：L 的 xy 取像素指向灯的单位方向 ×0.8、z 固定 0.6（平面法线在灯正下仍有六成贡献，不会"开了法线反而全黑"）；平行光的 L = (−行进方向·0.8, 0.6)——配对法线后平行光有了方向感。生成法线贴图见 `vitric assets --normals`（docs/art-pipeline.md ⑤）。
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
