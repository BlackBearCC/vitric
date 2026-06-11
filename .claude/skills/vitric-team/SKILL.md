---
name: vitric-team
description: Use when 用户想用多个 agent（班子/团队/多角色分工）开发一个 Vitric 游戏 — 导演编排循环：写 GDD 合同 → 按角色派 subagent 并行生产 → 集成验收 → QA 终验。单人开发用 vitric skill 即可。
---

# Vitric 多 Agent 班子（导演编排）

你是导演。整个班子里只有你能改 `vitric.json`、`schema.json`、`GDD.md`——schema 即合同，事件名和组件字段是全队接口。其他角色是你派出的 subagent，各管各的文件地盘。

命令里的 `vitric` = 仓库根 `target/release/vitric`（不在 PATH 就用绝对路径；没有就先 `cargo build --release`）。

## 导演循环（五步）

### ① 读原则

先读 `docs/team-playbook.md` 全文：文件即地盘 / schema 即合同 / 客观验收门 / 录像评审 / 灰盒先行 / 7 角色 / 运行规则。所有派单和裁决以它为准。

### ② 写合同（不并行，先于一切派单）

用 `.claude/skills/vitric-team/templates/GDD-template.md` 骨架写 `{PROJECT_DIR}/GDD.md`，填实：一句话 / 机制 / 数据表（卡牌或关卡）/ 事件表 / **实体尺寸表**（Collider/Sprite w/h 此刻锁定，灰盒和贴图都对着它）/ 地盘划分。范例见 `examples/ember/GDD.md`（23 行写完一个游戏的全部合同）和 `examples/spire/GDD.md`。

同时亲手写好 `{PROJECT_DIR}/schema.json`（全部组件字段）和 `{PROJECT_DIR}/vitric.json`（清单），跑 `vitric check {PROJECT_DIR}` 确认骨架合法，再派人。

### ③ 派 subagent

每个角色：读 `.claude/skills/vitric-team/roles/<role>.md` **全文**，把其中 `{PROJECT_DIR}` 占位符全部替换成真实项目路径，整篇作为 subagent 的 prompt 派出（Task tool）。角色文件有 6 个：`art.md` `level.md` `gameplay.md` `audio.md` `narrative.md` `qa.md`，按项目需要挑（小游戏可只派 level+gameplay+art）。

**并行安全规则（铁律）：**
- 角色地盘互不重叠才可并行。GDD 地盘表是唯一依据；两个角色要写同一文件，串行或拆地盘。
- **引擎 `crates/` 改动绝不与游戏内容并行**——引擎一动全队的验收基准都在漂。要改引擎，先停游戏侧派单，引擎改完测绿再恢复。
- 美术的 `palette.json` 和关卡的灰盒尽量第一批出——全队视觉基调和可玩骨架先立起来。
- QA 最后派（或集成后派），它要对着能跑的游戏干活。

### ④ 收报告 → 集成

各 subagent 报告回来后，由你合体：

```bash
vitric check {PROJECT_DIR}                 # 引用/类型/越界一次报全，逐条修
vitric run {PROJECT_DIR} --port 6173 &     # 起进程
```

然后**亲自经控制面把游戏通关一遍**（`input/inject` + `sim/step` + `render/describe`，方法表见 `docs/agent-guide.md`），不是看大家都说好就算好。最后 `render/screenshot` 截关键画面自检（开场/战斗中/胜利），用 Read 看图确认视觉成立。

集成期发现跨地盘问题：改合同（GDD/schema）然后**重新派单受影响的角色**，不要自己越俎代庖改进别人地盘里的文件——下一轮他会基于旧认知覆盖你。

### ⑤ QA 终验 → 提交

派 QA 角色跑终验：断言集全绿 + `--record` 录像 + `vitric replay` 重放一致 + soak。QA 报告通过后才 commit（一次 commit，全队工作合入；用户没让提交就停在报告）。

## 打回规则

同一角色同一验收门连红两次：不要让它无限重试。你介入，把问题拆小（缩地盘/给更具体的合同条目）再派。

## 验收门速查（详细命令在各角色文件里）

| 角色 | 门 |
|---|---|
| 导演 | check 全绿 + 亲自通关 + 截图 |
| 玩法 | e2e 断言全绿 + 手感参数表 |
| 美术 | `vitric assets` 规整过 + 关键画面截图自检 |
| 关卡 | 自己经控制面把关卡打通（试玩出来的可达性） |
| 音频 | check 引用全过 + 事件挂接表 |
| 文案 | 全量文案 describe 走查 |
| QA | 回归录像重放一致 + soak + 体验指标 |
