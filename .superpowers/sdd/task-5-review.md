# Task 5 Review: LLM Memory Dialogue Wired to Wish Fulfillment

**Reviewer:** Task reviewer (sub-agent)
**Commit range:** `e01d4c0..eb0ac67` (single commit `eb0ac67`)
**Files touched:** `games/frontier/scripts/wish.js` (+87 lines), `games/frontier/rules/wish.json` (+9 lines)

## 1. Spec compliance table

| # | Description | Verdict | Notes |
|---|---|---|---|
| 1 | `triggerWishMemory` fn registered via `vitric.fn`; reads `Persona.name/archetype/traits/speech` + `Need.memory_unlocked` via `ctx.getField`; builds Chinese prompt; stashes handle in `Colony.last_wish_memory_target`; calls `ctx.ask("llm", prompt, "onWishMemoryReply")` | ✅ | wish.js:85-106. All required Persona/Need fields read, prompt is Chinese, stash + ask correct. |
| 2 | `onWishMemoryReply` fn registered via `vitric.fn`; reads stashed handle; detects LLM error fallback (`LLM_ERROR_FALLBACK`); substitutes archetype-specific canned line from `MEMORY_FALLBACKS`; increments `Need.memory_unlocked`; sets `Colony.last_talk_reply`; emits `toast-show` with `name: text`; emits `memory-unlocked`; clears stash | ✅ | wish.js:114-140. All seven behaviors present and in the right order. |
| 3 | `MEMORY_FALLBACKS` constant: 3 archetypes (builder/farmer/explorer), 3 canned lines each; archetype matching uses same regex as `wishesForArchetype` in companion.js (技/电/匠→builder, 厨/医/农→farmer, default explorer) | ✅ | wish.js:64-80 (3×3 lines), wish.js:122-124 (regex). Regex `/技\|电\|匠\|build\|builder/i` and `/厨\|医\|农\|farm\|farmer/i` is character-identical to companion.js:47-48. |
| 4 | `wish-fulfilled-memory` rule in `wish.json`: catches `wish-fulfilled`, calls `triggerWishMemory` with `event.entity` + `event.wish_desc` | ✅ | wish.json:67-75. Rule inserted between `wish-fulfilled-toast` and `toast-show-generic` as the brief specified. `event.entity` resolves correctly — `advance_wish` emits `{ companion, wish_desc, entity: h }` (wish.js:47). |
| 5 | Verification: `vitric check games/frontier` exits 0 | ✅ | Implementer report shows exit 0 with check-output tail. Per instructions, not re-run. |
| 6 | No fake APIs in wish.js additions (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`) | ✅ | Grep across `games/frontier/scripts/wish.js` for the full pattern returned no matches. |
| 7 | Comments in English / strings Chinese in wish.js additions | ✅ | All `//` comments (wish.js:55-60, 62-63, 82, 88, 103, 108-113, 119, 130, 138) are English. Prompt text, fallback memories, toast format, and `LLM_ERROR_FALLBACK` are Chinese. |
| 8 | No dead code / YAGNI in new fns | ✅ | Every declared local (`handle`, `name`, `archetype`, `traits`, `speech`, `memCount`, `prompt`, `text`, `key`, `list`) is read. The two `memCount` declarations are in disjoint block scopes (inner in `if`, outer in fn body) — both used, no shadowing bug. |
| 9 | `LLM_ERROR_FALLBACK` character-for-character match with `rules/companion.json` `llm-error-handler` text | ✅ | wish.js:83 = `"（旅人沉默片刻,点了点头）"`. companion.json:75 = `"（旅人沉默片刻,点了点头）"`. Same full-width parens, same comma (`,`, half-width — both files agree), same characters. |
| 10 | Stash safety: rapid succession of wish fulfillments can clobber `Colony.last_wish_memory_target` before the first LLM reply returns | ⚠️ → Minor | Real risk. See finding F1. |
| 11 | `memory-unlocked` event has no listener | ⚠️ → Minor | Intentional per brief ("for future use / UI"). See finding F2. |
| 12 | `Colony.last_talk_reply` reuse: memory dialogue and talk system share the same field | ⚠️ → Minor | Real risk, low probability. See finding F3. |
| 13 | Prompt quality (1-2 sentences, ≤60 chars, no quotes/prefix) likely to produce good LLM output | ⚠️ | Cannot verify without running the LLM. Prompt is well-formed: role, persona fields, wish context, memory index, explicit format constraint. |
| 14 | Determinism: fallback selection `memCount % list.length` is deterministic | ✅ | wish.js:126-127. `memCount` is read from `Need.memory_unlocked` (deterministic state) before increment; modulo on a fixed-length list (3) is deterministic. LLM path is non-deterministic but engine records/replays via `llm-ask`/`llm-reply` per brief. |
| 15 | `wish-fulfilled` fires both `wish-fulfilled-toast` and `wish-fulfilled-memory` independently | ✅ | Both rules in wish.json:59-75 listen on `{ "event": "wish-fulfilled" }` with no `if` filter that would exclude one. Engine processes all matching rules per the brief's stated semantics. |

**Spec compliance: 12 ✅ / 3 ⚠️ (all three ⚠️ are explicitly permitted by the brief's review instructions — items 10/11/12 are flagged as Minor by design, item 13 is non-verifiable).**

## 2. Findings

### F1 — Stash clobber on concurrent wish fulfillments (Minor)
**File:** `games/frontier/scripts/wish.js:104` (stash) + `wish.js:115` (read) + `wish.js:139` (clear)
**Risk:** `advance_wish` iterates `Colony.companion_handles` in a loop and may emit multiple `wish-fulfilled` events in the same tick (e.g. two builder-archetype companions both have a `build` wish completing on the same `built` event — Kade "电工学徒" and Nell "沉默匠人" both match the builder regex per `DRIFTER_POOL`). Each `wish-fulfilled` triggers `triggerWishMemory`, which stashes the handle and calls `ctx.ask`. The LLM replies return asynchronously on later ticks; only one stash slot exists. The first reply would land on whichever handle was stashed last, applying the memory to the wrong companion. (Same risk for a single companion with two wishes of the same kind completing in one call — though `Wish.items` uses distinct `kind` per item, so intra-companion collision is unlikely.)
**Likelihood:** Higher than the brief suggests — the drifter pool has 2 builder-matching and 2 farmer-matching personas, so the player can plausibly invite two companions of the same archetype. Still, wish fulfillment is rare (3 wishes per companion, lifetime).
**Suggested fix (forward-looking, not required for Task 5):** key the stash by the LLM ask id rather than a single slot — e.g. `Colony.last_wish_memory_target` could be a map `{ [askId]: handle }`, and `onWishMemoryReply` would look up by `reply.id`. The engine's `llm-reply` event payload includes `id`, so this is feasible without engine changes. Alternatively, queue fulfillments and trigger only one LLM ask at a time.

### F2 — `memory-unlocked` event has no listener (Minor, intentional)
**File:** `games/frontier/scripts/wish.js:136` (emit); no matching `on` rule in `games/frontier/rules/*.json`.
**Note:** The brief explicitly says this event is "for future use / UI" — emit-only is intentional. Grep across `games/frontier` found the symbol only at the emit site (wish.js:112 comment, wish.js:136 emit). No listener. Flagging for visibility only; no action needed for Task 5.
**Suggested fix:** none. Future tasks that wire UI to memory unlocks will add a rule listening on `memory-unlocked`.

### F3 — `Colony.last_talk_reply` shared between talk and wish-memory systems (Minor)
**File:** `games/frontier/scripts/wish.js:133` (write) vs. talk system's `onTalkReply` (per brief, also writes `Colony.last_talk_reply`).
**Risk:** The existing `talk-reply-apply-*` systems display whatever is in `last_talk_reply` above the nearest companion/drifter. If a player presses `t` (talk) within the same tick window as a wish fulfillment, the talk reply and the wish memory reply would race for the same field; one would overwrite the other, and the wrong text could float above the wrong entity. The stash-and-clear pattern in both fns minimizes the window, but doesn't eliminate it.
**Likelihood:** Low. Talk is a player-initiated discrete input; wish fulfillment is a rare gameplay event. Same-tick collision is unlikely.
**Suggested fix (forward-looking):** give the wish-memory system its own display field (e.g. `Colony.last_wish_memory_reply`) and a parallel `wish-memory-reply-apply-*` system, so the two display paths don't share state. Not required for Task 5.

## 3. Verdict

**APPROVED**

No Critical or Important findings. The implementation matches the brief character-for-character (the implementer's report acknowledges this — "Followed the brief exactly: same code as provided"). The three Minor findings are all explicitly anticipated by the brief's review instructions (items 10, 11, 12) and are forward-looking concerns, not defects in Task 5's deliverable.

## 4. Summary

The implementer transcribed the brief's code into `wish.js` and `wish.json` verbatim, including the `MEMORY_FALLBACKS` constant, the `LLM_ERROR_FALLBACK` sentinel (character-identical to `companion.json`'s `llm-error-handler` fallback), and the `wish-fulfilled-memory` rule. All 12 verifiable spec items pass; the 3 non-verifiable/concurrency items are flagged as Minor per the brief's own guidance. Task 5 is ready to merge; the stash-clobber and `last_talk_reply`-sharing concerns are real but low-probability and best addressed in a future hardening task rather than blocking this one.
