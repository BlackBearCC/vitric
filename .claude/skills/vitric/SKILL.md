---
name: vitric
description: Use when developing, running, testing, or debugging a Vitric game project — covers vitric check/run/replay commands, the HTTP control plane (query/mutate world state, inject input, time control, assertions, headless screenshots), writing schema/scenes/rules/scripts, and the deterministic replay debugging workflow.
---

# 用 Vitric 开发游戏

Vitric 是 glass-box 游戏引擎：一切状态对 agent 可见、可操作、可验证。你（agent）可以自主完成"改 → 校验 → 跑 → 看 → 测 → 修"全闭环，不需要人帮你看屏幕。
（要多个 agent 分角色组队开发，用 `vitric-team` skill——本 skill 是单人开发手册。）

## 铁律

1. **改完数据先 `vitric check <项目目录>`** 再跑。错误带路径+错误码+修复提示，一次报全。
   游戏已经在跑时改了规则/脚本：调 `project/reload` 热重载，不用重启（世界状态保留）。
2. **验证行为用控制面，不要猜。** 暂停 + 单步 + 查状态 + `render/describe`，每一步都是确定性的。观察画面**优先用 `render/describe`**（语义描述：方位/坐标/遮挡/视野外，比看像素精准），截图只在怀疑渲染本身有问题时兜底。
3. **复现 bug 用录像。** `--record` 录下来，`vitric replay` 逐帧复现；重放跑偏说明逻辑里混进了非确定性。
4. 调试时先 `sim/pause`，再 `sim/step`——自由运行状态下世界一直在变，你看到的状态可能已过期。

## 完整参考

读 [docs/agent-guide.md](../../../docs/agent-guide.md)：三个 CLI 命令、控制面全部方法（看/动/控时间/测）、数据语言（schema/场景/规则/脚本）、引擎约定组件。

## 最常用的闭环

```bash
vitric check my-game
vitric run my-game --port 6173 &      # stdout 第一行 JSON 有控制面地址
rpc() { curl -s -X POST http://127.0.0.1:6173/rpc -d "$1"; }
rpc '{"method":"sim/pause"}'
rpc '{"method":"assert/add","params":{"id":"hp","if":[["@player.Health.hp",">",0]]}}'
rpc '{"method":"input/inject","params":{"action":"right"}}'
rpc '{"method":"sim/step","params":{"ticks":60}}'      # 返回里带断言失败
rpc '{"method":"render/describe"}'                     # 语义观察（主通道）：谁在哪、谁挡谁、谁在视野外
rpc '{"method":"world/get","params":{"entity":"@player"}}'
# 兜底: rpc '{"method":"render/screenshot","params":{"path":"shot.png"}}' 再 Read 看图
rpc '{"method":"sim/quit"}'
```

## 写玩法的优先级

规则（`rules/*.json`，"当 X 则 Y"直译）> 脚本系统（JS，必须声明 query/writes）。
规则表达不了再落脚本；脚本里禁 `Math.random`/`Date.now`，用 `ctx.random()`/`ctx.tick`。
