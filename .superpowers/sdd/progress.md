# Frontier Deepening — SDD Progress Ledger

Plan: docs/superpowers/plans/2026-07-17-frontier-deepening.md
Base commit: af83c61
Branch: main (per user preference: auto-commit + push to main)

## Tasks

- Task 1: complete (commits af83c61..35004ad, review clean)
- Task 2: complete (commits 35004ad..300cf73, review clean after Math.random→ctx.random fix)
- Task 3: complete (commits 300cf73..4c357e0, APPROVED with 4 Minor, F2/F3 resolved in T4)
- Task 4: complete (commits 4c357e0..e01d4c0, APPROVED with 1 Minor)
- Task 5: complete (commits e01d4c0..eb0ac67, APPROVED with 3 Minor)
  - LLM memory dialogue: triggerWishMemory fn + onWishMemoryReply callback + MEMORY_FALLBACKS. Rule wish-fulfilled-memory catches wish-fulfilled → calls triggerWishMemory. Fallback path uses archetype-specific canned lines when LLM unavailable.
  - Minor findings: stash clobber on concurrent fulfillments (low probability), memory-unlocked event has no listener (intentional forward-looking), last_talk_reply shared with talk system (low conflict probability).
- Task 6: complete (commits eb0ac67..06464fb, APPROVED with 0 Critical/Important, 3 expected Minor)
  - Quest system converted to milestone: step3 gate on wish-fulfilled + affinity>=60; step6 gate on companion_wish_count>=2; game-won rule → settlement-founded (emits only settlement-founded, no settlement-thrived/game-won); quest-banner-8 → "自由探索中"; vitric.json gates.must_emit → settlement-founded.
  - Minor findings (all expected, non-blocking): dead ending-show rule in narrative.json left in place (YAGNI); qa/clear.json stale (Task 9 re-records); brief said check output "OK" but actual is JSON report with exit 0 (cosmetic).
- Task 7: complete (commits 06464fb..5a9870f, APPROVED with 0 findings)
  - Node regrowth: interact fn sets Node.cooldown=90 on depletion; node_regrow system decrements by ctx.dt, regrows left=max when cooldown hits 0.
  - Structure upgrade: upgrade_structure fn (plot→greenhouse, conduit→solar-array, quarters→cabin) + upgrade-button-click rule on ui-activate{action:"upgrade-prompt"}.
- Task 8: complete (commits 5a9870f..62c45fc, APPROVED with 1 cosmetic Minor)
  - UI hooks: flare-bar system (writes Colony.flare_bar) + hud-flare-bar rule; kb-mode-upgrade (key "u" → upgrade mode); upgrade-click rule (mouse + Mode=upgrade → upgrade_structure fn).
  - Minor: hud.js missing trailing newline (cosmetic, non-blocking).
  - **Post-review schema regressions fixed**: Task 8 added Colony.flare_bar writes + Mode.value="upgrade" sets + hud-flare-bar rule (reads @colony.Colony.flare_bar), but never added the schema field, the enum variant, or the @flare_lbl scene entity. Fixed in commit ee8cc74 (added Colony.flare_bar text field, "upgrade" to Mode.value enum, flare_lbl entity in main.json). Crashed engine at tick 0.
- Task 9: complete (commits 854c6ff, gate PASS — replay 37247 ticks verified, settlement-founded emitted)
  - Re-recorded 9-day playthrough with new wish-based quest gates.
  - Script bugs found & fixed in record_clear.py: (1) giveGiftNearby consumes ore (ore is first in ITEM_KINDS) → gather 4 ore not 2; (2) inp("e") only sets Mode.value, doesn't show craft_menu (only the mode-craft ui-activate rule shows it) → click mode_craft button before craft_plank.
  - Recording actually runs to day=11 (day-labels in script are aspirational, not literal); gate only checks settlement-founded emission.
  - **Pre-task schema regressions fixed**: Task 4 (commit e01d4c0) introduced Colony._wish_food_day and Colony.last_wish_memory_target writes in wish.js but never added them to schema.json. Fixed in commit 30e6ae7. Crashed engine at tick ~1191 (food reaches 80 after building any plot).
- Task 10: docs update — IN PROGRESS

## Notes

- Task 9 done via RPC-driven record_clear.py (no manual play). Controller rebuilt release binary, ran script, verified replay + gate.
- API: ctx.getField/setField/spawn/despawn/ask/emit/random/dt/tick all real. __onReply is prelude built-in. ctx.ask("llm", prompt, "callbackFnName") routes via llm-reply event → __onReply → callbackFn.
- Engine allows runtime ad-hoc fields on entities BUT rule/system reads via @entity.Comp.field require the field to be declared in schema.json. JS system writes (e.Colony.foo = ...) tolerate undeclared fields, but rule reads (@colony.Colony.foo) do NOT — they crash. Lesson: always declare fields in schema.json if any rule reads them.
- Task 6 changes: quest.json step3/6 gates → wish-based, step7→8 rename game-won→settlement-founded, quest-banner-8 text → "自由探索中", vitric.json gates.must_emit → settlement-founded. Colony.companion_wish_count already in schema (Task 1).
- Schema audit (Task 9): all Colony.* field references in scripts/*.js cross-checked against schema.json. All fields now declared.
