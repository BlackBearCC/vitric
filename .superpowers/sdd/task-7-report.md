# Task 7 Report — Node Regrowth + Structure Upgrade Path

## Status: DONE

## What changed

### Edit 1: `games/frontier/scripts/economy.js` — set cooldown on node depletion
In the `interact` fn's Node gathering block, computed `newLeft = (node.left | 0) - 1` once, wrote it via `ctx.setField(a.entity, "Node.left", newLeft)`, and added a guard: when `newLeft <= 0`, also `ctx.setField(a.entity, "Node.cooldown", 90)`. Comment line added explaining the regrow timer.

### Edit 2: `games/frontier/scripts/economy.js` — appended `node_regrow` system
New `vitric.system("node_regrow", { query: ["Node"], writes: ["Node"] }, ...)` at the end of the file (after `craft`). For each entity with `left <= 0 && cooldown > 0`, decrements cooldown by `ctx.dt`; when cooldown hits 0 it sets `left = max` and clears cooldown.

### Edit 3: `games/frontier/scripts/economy.js` — appended `upgrade_structure` fn
New `vitric.fn("upgrade_structure", (a, ctx) => {...})` after `node_regrow`. Reads `Structure.kind`/`Structure.tier` via `ctx.getField`, refuses if tier >= 2; UPGRADES table maps `plot→greenhouse (ore2+plank2)`, `conduit→solar-array (ore3+plank1)`, `quarters→cabin (plank4+lamp1)`. Checks cost via `canPay`, pays via `pay`+`emitInv`, writes `Structure.kind`/`Structure.tier=2` via `ctx.setField`, emits `upgrade-structure` + `toast-show`.

### Edit 4: `games/frontier/rules/economy.json` — appended `upgrade-button-click` rule
Added trailing comma to the previously-last rule `seed-start` and appended a new rule `upgrade-button-click` that triggers on `ui-activate{action:"upgrade-prompt"}` and calls `upgrade_structure` with `entity: "event.target_id"` + full inventory fields (same shape as `build-click`).

## Verification

```
$ cargo run -p vitric-cli -- check games/frontier 2>&1 | tail -15
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

Exit code: 0.

## Scope check

`git diff --stat HEAD~1 HEAD`:

```
 games/frontier/rules/economy.json | 13 +++++++++
 games/frontier/scripts/economy.js | 59 ++++++++++++++++++++++++++++++++++++++-
 2 files changed, 71 insertions(+), 1 deletion(-)
```

Only the two target files were modified. `schema.json`, `vitric.json`, `hud.js`, `wish.js`, `main.json` untouched.

## Self-check checklist

- [x] `interact` fn sets `Node.cooldown = 90` when `newLeft <= 0` (Edit 1)
- [x] `node_regrow` system queries `["Node"]`, writes `["Node"]`, decrements cooldown by `ctx.dt`, regrows `left = max` when cooldown hits 0 (Edit 2)
- [x] `upgrade_structure` fn reads kind/tier via `ctx.getField`, checks cost via `canPay`, pays via `pay`+`emitInv`, writes kind/tier via `ctx.setField`, emits `upgrade-structure` + `toast-show` (Edit 3)
- [x] `upgrade-button-click` rule in economy.json calls `upgrade_structure` with `entity: "event.target_id"` + full inventory (Edit 4)
- [x] schema.json NOT modified
- [x] vitric.json NOT modified
- [x] `cargo run -p vitric-cli -- check games/frontier` exits 0
- [x] Committed + pushed to `main`

## Commit

- Hash: `5a9870f`
- Message: `feat(frontier): node regrowth + structure upgrade path (tier 1->2)`
- Pushed: `06464fb..5a9870f  main -> main`

## Concerns

None. All 4 edits applied verbatim per brief; check passes; only the two target files were modified.
