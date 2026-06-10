# Vitric

[English](README.md) | 中文

**The glass-box game engine for AI agents.**

![coin-run demo](docs/media/coin-run.gif)

*↑ 这个 demo 的每一帧都是引擎自己渲的（CPU 光栅化，无 GPU），由 AI 通过控制面操作游戏并逐帧截图拼成——这正是引擎的卖点本身。*

现有引擎为"人坐在编辑器前"设计，对 AI 是黑盒；Vitric 以 agent API 为中心——引擎的一切状态对 AI 可见、可操作、可验证，AI 能自主地**运行游戏 → 观察画面和状态 → 跑断言 → 修改 → 重来**，整个闭环不需要人插手。

## 现在就能跑

```bash
cargo build --release

# 校验示例项目（错误带路径+错误码+修复提示，一次报全）
./target/release/vitric check examples/coin-run

# 跑起来（无头 + AI 控制面）
./target/release/vitric run examples/coin-run --port 6173

# 素材和谐化：AI 出的图全项目统一到一张色板（见 docs/art-pipeline.md）
./target/release/vitric assets examples/glow --colors 16
```

另开一个终端，像 agent 一样把游戏打通关：

```bash
rpc() { curl -s -X POST http://127.0.0.1:6173/rpc -d "$1"; echo; }
rpc '{"method":"sim/pause"}'
rpc '{"method":"input/inject","params":{"action":"right"}}'
rpc '{"method":"sim/step","params":{"ticks":60}}'                       # 确定性逐帧推进
rpc '{"method":"world/get","params":{"entity":"@player"}}'              # 分数已经是 3
rpc '{"method":"render/screenshot","params":{"path":"shot.png"}}'       # 无头截图，不需要 GPU
rpc '{"method":"events/recent"}'                                        # collision → coin-collected → game-won 因果链全可见
```

完整方法表见 [docs/agent-guide.md](docs/agent-guide.md)。

## 它凭什么是"AI 原生"

- **一切状态都是数据。** 场景、实体、规则、世界的每一帧都是强 schema 的 JSON——写入即校验，运行时可往返读回，存档=快照。没有藏在编辑器二进制里的状态。
- **确定性 + 录像回放。** 固定步长、种子随机（跨 Rust/JS 同一条随机流）、输入录制。`vitric replay` 逐校验点验证，任何 bug 都能精确重放到出错前一帧——AI 调试从"猜"变成"看回放"。
- **规则正门 + 脚本安全带。** 80% 玩法是 `当 X 则 Y` 的声明式规则直译（刻意不图灵完备，级联死循环直接报错）；写不动的落到 JS 系统函数，但必须声明读写哪些组件，越权就拒——引擎永远知道每段逻辑的影响面。
- **报错为 LLM 设计。** 每条错误带精确路径、稳定错误码、修复提示，一次报全不挤牙膏。
- **无头即所见。** 截图是引擎内建能力，不需要 GPU、窗口或图形会话——CI 里、容器里、任何地方，agent 都能亲眼看到画面，像素逐字节确定（截图也能进断言）。

## 架构

```
crates/
  vitric-ecs       确定性可内省 ECS（组件=JSON，迭代有序，快照/哈希）
  vitric-data      声明式数据层（schema 校验、场景实例化、项目加载）
  vitric-rules     当X则Y 规则引擎（触发/条件/动作/级联保护）
  vitric-script    QuickJS 脚本层（读写声明强制、确定性随机、热重载）
  vitric-sim       固定步长模拟（PCG32、录像/重放校验、内建运动+碰撞）
  vitric-control   AI 控制面（HTTP JSON-RPC：查/改/注输入/时间控制/断言/截图）
  vitric-render    CPU 光栅化（world→PNG，无头可用；wgpu 呈现层在路上）
  vitric-cli       vitric check / run / replay
examples/coin-run  示例：吃金币（规则/脚本/动画/分数 HUD/断言全覆盖）
examples/cave-gen  示例：配方生成关卡——改一个 seed 或 Recipe 数字，整张关卡重新生成
examples/jump      示例：平台跳跃（重力/落地/起跳/终局文字），纯规则零脚本
```

设计稿与决策记录：[docs/AI原生游戏引擎-设计稿.md](docs/AI原生游戏引擎-设计稿.md) · 实施计划：[docs/plan.md](docs/plan.md)

## MCP

`mcp/` 内置官方 MCP server（12 个工具）：任何 MCP 客户端（Claude Code / Cursor / Codex…）开箱即用地校验、启动、观察、操作、断言一个 Vitric 游戏。

```json
{ "mcpServers": { "vitric": { "command": "node", "args": ["<repo>/mcp/index.js"], "env": { "VITRIC_BIN": "<repo>/target/release/vitric" } } } }
```

## 状态

核心闭环已跑通（100+ 测试）：确定性录像回放、语义观察（render/describe）、热重载、精灵贴图+素材校验、声明式动画（引擎独占 Sprite.image 写权，动画不可能被打断）、平台物理（重力/Solid 挡停/grounded）、屏上文字（内嵌点阵字体，describe 直出文本内容不用 OCR）、配方生成关卡、窗口呈现+检查器（点选/拖拽写回，选中态人机双向可见）、GPU 呈现（wgpu，`--renderer gpu`，真机已验；无头截图保持纯 CPU 逐字节确定）、音频、TypeScript、MCP server、CI+二进制发布。`vitric assets` 把 AI 出的图统一到一张项目色板（确定性中位切分量化，原件自动备份，`--palette-lock` 让后补素材入伙老色板）。进行中：运行时 LLM 模块。

## License

MIT
