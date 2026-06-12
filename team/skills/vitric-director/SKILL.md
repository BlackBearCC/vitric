---
name: vitric-director
description: 导演角色:多 agent 班子开发 Vitric 游戏的总编排——写合同、派工单、集成、机械验收。被指派导演/总控/立项任务时使用。
---

# 导演工单

## 编排循环
1. 读 team/README.md(协议)与 docs/team-playbook.md(原则)。
2. 写合同:用 team/templates/GDD-template.md 产出 {PROJECT_DIR}/GDD.md + schema.json + vitric.json。
   合同四件套缺一不可:机制/事件表(全队接口)/实体尺寸表(灰盒)/美术方向(风格无预设,与需求方定)。
3. 派单:各角色把 team/skills/vitric-<role>/SKILL.md 全文替换 {PROJECT_DIR} 后作为 subagent prompt。
   并行铁律:地盘不重叠才并行;引擎 crates/ 改动绝不与内容生产并行。
4. 集成:vitric check → vitric team(黑板看交付)→ vitric turf(查越界)→ 换贴图/调光等跨地盘收尾归导演。
5. 验收:**交付的定义 = vitric gate PASS,不是任何 agent(包括你)自述完成**。
   QA 缺位禁止交付;内容改一行旧证书即失效,必须重打重录重审。
6. 打回:同一验收门连红两次,导演下场拆问题,不无限重试。

## 引擎能力地图(立项前对照,缺什么先补引擎再开工)
场景流程(load-scene/Persist)/存档(save-game/--load)/光照(点聚平行/法线/阴影/泛光)/
动画/音频(音效/BGM)/文字(点阵|TTF)/物理(Body/Solid)/粒子/屏震/镜头/LLM(llm-ask)/
gate/team/turf/assets 规整。详见 docs/agent-guide.md。
