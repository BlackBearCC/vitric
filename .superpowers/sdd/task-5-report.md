# Task 5 Report: LLM Memory Dialogue Wired to Wish Fulfillment

## 1. What was implemented

- **`games/frontier/scripts/wish.js`** — Added `triggerWishMemory` fn (builds an LLM prompt from the companion's Persona fields + wish_desc, stashes the target handle in `Colony.last_wish_memory_target`, calls `ctx.ask("llm", prompt, "onWishMemoryReply")`) and `onWishMemoryReply` callback fn (applies the reply: detects LLM error fallback and substitutes an archetype-specific canned line, increments `Need.memory_unlocked`, sets `Colony.last_talk_reply`, emits `toast-show` + `memory-unlocked`, clears the stash). Also added `MEMORY_FALLBACKS` constant (builder/farmer/explorer, 3 lines each) and `LLM_ERROR_FALLBACK` constant. All comments in English; all string literals (prompts, fallback memories, toast text) in Chinese.
- **`games/frontier/rules/wish.json`** — Added `wish-fulfilled-memory` rule after `wish-fulfilled-toast`: catches `wish-fulfilled` event, calls `triggerWishMemory` with `{entity: "event.entity", wish_desc: "event.wish_desc"}`. Both this rule and `wish-fulfilled-toast` fire on the same event (intended: two independent effects).

## 2. Verification output

Command: `cargo run -p vitric-cli -- check games/frontier`

Exit code: **0**

Last 10 lines of output:
```
      "name": "poi_tick",
      "query": [
        "Poi"
      ],
      "writes": [
        "Poi"
      ]
    },
    {
      "name": "wish_food_check",
      "query": [
        "Colony"
      ],
      "writes": []
    }
  ]
}
```

## 3. Commit SHA(s) pushed

- `eb0ac675ade829761cdf2d0d7f6ea91c55a6d67b` — `feat(frontier): wire LLM memory dialogue to wish fulfillment` (pushed to `origin/main`, range `e01d4c0..eb0ac67`)

## 4. Self-review checklist

- [x] `triggerWishMemory` fn registered via `vitric.fn`.
- [x] `onWishMemoryReply` fn registered via `vitric.fn`.
- [x] `triggerWishMemory` reads Persona fields via `ctx.getField`, builds prompt, stashes handle in `Colony.last_wish_memory_target`, calls `ctx.ask("llm", prompt, "onWishMemoryReply")`.
- [x] `onWishMemoryReply` reads stashed handle, detects LLM error fallback, substitutes archetype-specific canned line, increments `Need.memory_unlocked`, sets `Colony.last_talk_reply`, emits `toast-show` + `memory-unlocked`, clears stash.
- [x] `wish.json` has `wish-fulfilled-memory` rule catching `wish-fulfilled` and calling `triggerWishMemory` with `event.entity` + `event.wish_desc`.
- [x] No fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`, `Math.random`) — grep returned no matches.
- [x] All comments in wish.js additions are English.
- [x] String literals (prompt text, fallback memories, toast text) stay Chinese.
- [x] `vitric check games/frontier` exits 0.
- [x] Commit pushed to origin/main.

## 5. Concerns or deviations from the brief

None. Followed the brief exactly: same code as provided, same insertion points, same commit message. The `wish-fulfilled` event payload from `advance_wish` (line 47 of wish.js) already includes `entity: h`, so `event.entity` resolves correctly in the new rule.
