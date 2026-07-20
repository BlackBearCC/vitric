# Task 10 — GDD + PLAN doc update

**Status:** ✅ Complete
**Commit:** see git log (commit message: `docs(frontier): update GDD + PLAN for deepening completion (Tasks 1-9)`)
**Branch:** main (pushed to origin)

## Summary

Updated `games/frontier/GDD.md` and `games/frontier/PLAN.md` to reflect the completed frontier deepening work (Tasks 1-9). Targeted edits only — no full rewrites; historical content preserved.

## GDD.md changes

1. **One-liner (line 4):** removed "达成'兴旺'即通关(发 `game-won`)"; replaced with the milestone framing — "跟着一条主线任务(修信标→种出第一茬→伙伴心愿达成→聚落兴旺)把小聚落带活,建立定居点后进入自由探索(发 `settlement-founded`),四个循环自驱,无限游玩。"
2. **引擎能力 / 任务胜负 (line 14):** `gates.playthroughs.must_emit` changed from `"game-won"` to `"settlement-founded"` with the "里程碑,不是结局" note.
3. **任务 section (lines 42-52):** rewrote the 4-step quest as the 8-step milestone structure (修复信标 → 种出第一茬 → 伙伴心愿达成 → 立足脚跟 → 食物富足 → 多人聚居 → 建造丰碑 → 自由探索中). Step 3→4 gate is `wish-fulfilled + affinity>=60`; step 6→7 gate is `companion_wish_count>=2`; step 7 emits `settlement-founded`. Added a callout that there's no `game-won`/`settlement-thrived` hard ending.
4. **求生底盘 (line 54):** removed "耀斑事件本版砍掉" — now states 耀斑/夜循环 is implemented and points to the new 深化系统 section.
5. **New 深化系统 section (lines 56-68):** covers 耀斑/夜循环, POI 探索, 伙伴心愿, LLM 记忆对话, 资源点再生, 结构升级 — each with the shipped component fields, fns, systems, rules, and event payloads.
6. **事件表 (line 90):** removed `settlement-thrived{}` and `game-won{}`; added `settlement-founded{}` (里程碑,非结局), `wish-fulfilled{companion,wish_desc,entity}`, `upgrade-structure{id,kind}`.
7. **数据表 任务行 (line 87):** "见机制4环" → "见机制 8 步(里程碑制,`settlement-founded` 是里程碑不是结局)".
8. **QA 地盘行 (line 116):** annotated `qa/clear.json` with "9 天 37247 tick,在 step 8 发 `settlement-founded`".

## PLAN.md changes

1. **总览 (top):** added a completion banner ("状态(2026-07-17):全部完成") + a 12-row status table marking 3a-3f, 4a-4e, 收尾 all ✅. Kept the original "当前状态(历史快照,实施前)" bullets as historical context.
2. **执行顺序:** all 10 steps struck through and marked ✅; added "全部完成" banner.
3. **New 深化增量 (2026-07-17,Tasks 1-9) section (appended at bottom):** table summarizing each of Tasks 1-9 with what shipped, plus a "设计转向" subsection explaining the hard-ending → milestone + free-play shift.
4. Historical implementation detail (3a-3f, 4a-4e, 收尾 bodies) preserved unchanged.

## Verification

- `grep -c "game-won" games/frontier/GDD.md` → **1** (only in the line "> 没有 `game-won`/`settlement-thrived` 硬结局" — historical/negative context, acceptable per task spec)
- `grep -c "settlement-founded" games/frontier/GDD.md` → **7** (≥1 ✓)
- `vitric.json` gates.must_emit confirmed = `"settlement-founded"` (already in place from Task 6)
- `qa/clear.json` `"ticks":37247` confirmed

## Notes / deviations from task brief

- **`upgrade-structure` event payload:** the task brief said to document `{entity,kind,tier}`, but the actual shipped code in `scripts/economy.js` emits `{id, kind}` (no `entity`, no `tier`). Per the constraint "Don't invent features that weren't shipped — only document what Tasks 1-9 actually delivered", I documented the actual `{id,kind}` payload in both GDD.md and PLAN.md.
- **Wish categories count:** the task brief said "12 种心愿类别". The actual `rules/wish.json` ships 9 wish *kinds* (build, build-lamp, harvest, harvest-wheat, gather-ore, enter-poi, upgrade, food-high, see-dawn) advanced by 9 rules + 3 effect rules (wish-fulfilled-toast, wish-fulfilled-memory, toast-show-generic) = 12 wish-related rules total. I documented this as "9 种心愿类别" + "12 条 wish.json 规则" to match what actually shipped.
