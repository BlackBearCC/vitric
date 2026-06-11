# 多 Agent 游戏开发班子（Team Playbook）

用 Vitric 做游戏时，多个 AI agent 按专业角色分工协作的标准打法。
引擎的项目结构天然就是组织架构图：文件即地盘，schema 即合同，录像即评审。

## 地基原则

1. **文件即地盘**：每个角色只写自己的目录，越界即违规。跨角色需求走事件约定提给导演，不直接改别人的文件。
2. **schema 即合同**：组件字段、事件名是全队接口，只有导演能改；改 schema = 改合同，全队重新对齐。
3. **验收门必须客观**：交付以机器可判的标准为准（check 通过 / 断言绿 / 截图自检 / 录像重放一致），不靠"我觉得行了"。
4. **录像即评审**：任何争议用录像复现。确定性重放保证所有人看到的逐帧一致。

## 班子（7 角色）

| 角色 | 地盘 | 验收门 |
|---|---|---|
| 导演 | vitric.json、schema.json、事件约定表 | `vitric check` 全绿 + 亲自通关一遍的录像 |
| 玩法 | rules/、scripts/ | e2e 断言全绿 + 手感参数表（速度/跳高/镜头系数列全） |
| 美术 | assets/、animations.json、palette.json | `vitric assets` 规整过 + 关键画面截图自检 |
| 关卡 | scenes/ | 自己经控制面把关卡打通（可达性是试玩出来的，不是声称的） |
| 音频 | sounds/ | check 引用全过 + 事件挂接表 |
| 剧情/文案 | 文案表、llm-ask 的 prompt 设计、Text 内容 | 文案全量过一遍 describe 截屏走查 |
| QA | 断言集、录像库 | `vitric gate` 全门 PASS（录像回归走 gate）+ soak 报告 + 体验指标 |
| 交付（机器裁决） | 清单 `gates` 声明（导演写） | `vitric gate` 退出 0：通关录像逐位重放 + 终局事件触发 + 断言全程绿——交付的定义，不是 agent 自述完成 |

## 流程五拍

1. **立项**（不并行）：导演写一页 GDD → 定 schema、事件表、**各实体的尺寸**（Collider/Sprite w/h 此刻锁定，给灰盒用）。
2. **并行生产**：各角色同时开工。关卡用纯色块灰盒先搭（尺寸已锁，美术出图后只换贴图不动尺寸）；美术先交 palette.json 锁全队视觉基调。
3. **集成**：导演合体、check、起进程。从这一刻起游戏永远是活的。
4. **闭环迭代**：各角色对着运行中的游戏自验自改（热重载不停机）。
5. **终验发布**：QA 录像回归走 `vitric gate`（通关录像是不可伪造的交付证书）+ soak；美术 `--palette-lock` 锁色板；打 tag。

## 运行规则

- **各开各的进程**：项目是纯数据，每个角色自起一份 `vitric run`（不同端口）互不干扰。共用一个实例时，暂停/单步/restore 只归当前持锁角色，其他角色只读（describe/截图随便）。
- **git 并行**：各角色一个分支/worktree，导演负责合。地盘不重叠，合并近乎零冲突。
- **打回规则**：同一验收门连红两次，导演介入拆问题，不无限重试。
- **"好玩"的验收**：断言验对错，验不了乐趣。QA 盲玩报体验指标（死亡次数分布、卡点位置、通关时长方差），主观终审归导演与人类。

## 灰盒约定

立项时导演产出"实体尺寸表"（名字 → Collider/Sprite 的 w/h）。关卡以此用纯色块搭关；
美术产出贴图时尺寸必须匹配该表；换贴图只改 `Sprite.image`，物理与布局零波动。

## 落地形态

本打法已产品化为 Claude Code skill：[`.claude/skills/vitric-team/`](../.claude/skills/vitric-team/SKILL.md)。
`SKILL.md` 是导演编排循环，`templates/GDD-template.md` 是合同骨架，`roles/` 下六份角色工单
（art/level/gameplay/audio/narrative/qa）替换 `{PROJECT_DIR}` 后即可整篇作为 subagent prompt 派出。
合同范例：`examples/ember/GDD.md`、`examples/spire/GDD.md`。
