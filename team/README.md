# team/ — 多 Agent 班子（随引擎发货的员工）

多个 AI agent 按专业角色分工开发一个 Vitric 游戏的协同制度。本目录是**协议的唯一权威源**：
角色工单、GDD 合同骨架都在这里，引擎的 `vitric team` / `vitric turf` 命令和 MCP 的
`vitric_role` 工具直接消费它——任何 agent 平台（Claude Code / Cursor / Codex / 自研编排器…）
都能从引擎本体领到工单，不依赖某家的 skill 格式。

## 协议三句话

1. **文件即地盘**：每个角色只写自己的目录，越界即违规（`vitric turf` 机器执法）。
   跨地盘需求走事件约定提给导演，不直接改别人的文件。
2. **schema 即合同**：`GDD.md` + `schema.json` 的组件字段、事件名是全队接口，只有导演能改。
3. **验收门必须客观**：交付 = `vitric gate` PASS（通关录像逐位重放 + 终局事件 + 断言全程绿），
   不靠 agent 自述完成。协同状态随时看 `vitric team <项目目录>`（只读黑板，永远退出 0）。

完整打法（流程五拍 / 并行规则 / 打回规则 / 灰盒约定）见 [docs/team-playbook.md](../docs/team-playbook.md)。

## 目录

- `roles/<role>.md` — 六份角色工单（art / level / gameplay / audio / narrative / qa）。
  把全文里的 `{PROJECT_DIR}` 替换成真实项目路径后，整篇就是该角色 subagent 的 prompt。
  MCP 客户端可调 `vitric_role` 工具直接取（带 project 参数则占位符已替换好）。
- `templates/GDD-template.md` — 合同骨架，导演开工第一件事就是按它写 `{PROJECT_DIR}/GDD.md`。

## 地盘表（`vitric turf` 的执法依据，引擎内置同一张表）

| 角色 | 可写 |
|---|---|
| art | `assets/`、`animations.json`、`palette.json` |
| level | `scenes/` |
| gameplay | `rules/`、`scripts/` |
| audio | `sounds/` |
| narrative | `scenes/`（文案住在场景的 Text 里，与 level 共享目录——行级分工在 GDD 里约定） |
| qa | `qa/`、`recordings/` |
| director | 一切（`GDD.md`、`schema.json`、`vitric.json` **只有**导演能动） |

执法命令：`vitric turf <项目目录> --role <角色> <改动文件...>`——有越界文件就退出 1 并逐条点名。

## 两个引擎命令

```bash
vitric team <项目目录>                     # 协同黑板：各角色交付物健康度 + 门禁/合同状态 + 卡点提示（JSON，永远退出 0）
vitric turf <项目目录> --role art a.png   # 地盘执法：改动文件越界即退出 1（JSON 报告同 gate 风格）
```
