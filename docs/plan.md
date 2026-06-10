# Vitric 实施计划（v0 里程碑：闭环跑通）

目标：AI 能自主地「改 → 校验 → 跑 → 观察 → 测 → 修」一个真实小游戏，全程无人插手。

## 实施顺序

1. **vitric-ecs** — 可内省的确定性 ECS。组件值=JSON（一切状态天生可序列化），BTreeMap 存储保证迭代顺序确定；快照/恢复/状态哈希（自实现 FNV-1a，跨平台稳定）。
2. **vitric-data** — 声明式项目格式：`vitric.json` 清单 + 组件 schema + `scenes/*.json`。写入即校验，错误结构化（路径/错误码/修复提示，为 LLM 设计）。
3. **vitric-sim** — 固定步长 60Hz、自实现 PCG32 种子随机、输入录制(jsonl)/重放、断言钩子。重放一致性靠状态哈希验证。
4. **vitric-rules** — 「当X则Y」规则引擎。触发器(tick/事件/输入)、最小条件表达式（只有比较和与或，**刻意不图灵完备**）、动作(改字段/生成/销毁/发事件/调脚本)。级联深度上限 8，超限显式报错。
5. **vitric-script** — rquickjs 嵌入。JS 系统函数注册时声明 reads/writes，越权访问报错；Math.random/Date.now 替换为 sim RNG/时钟保确定性。
6. **vitric-control** — 调试控制面：HTTP JSON-RPC（127.0.0.1），查/改任意状态、注入输入、暂停/单步/倍速、快照、断言、事件轮询。命令在帧边界应用，不破坏确定性。v0 不用 WS，轮询对 agent 足够，依赖最小。
7. **vitric-cli** — `vitric check / run / replay`。
8. **examples/coin-run** — 示例游戏端到端自测：注入输入→吃金币→断言分数→录制→重放哈希一致。
9. **vitric-render** — wgpu 2D（容器无 GPU，窗口验证在 Windows GPU 机做）。

## 架构铁律

- 一切组件值可 JSON 序列化，没有藏在 Rust 类型里的私有状态。
- 一切迭代顺序确定（BTreeMap/排序），同种子同输入=同哈希。
- 失败显式暴露，不写 fallback；错误信息必须带路径和修复提示。
- 控制面外的代码不持有锁跨帧；控制面命令只在帧边界生效。

## 暂缓项

- WebSocket 推送、TS 转译（esbuild 接入）、素材管线、关卡笔刷、MCP server 包装、多人。
