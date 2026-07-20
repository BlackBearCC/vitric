# Task 8 Report — UI Hooks: Upgrade Mode + Flare Warning Bar

## Status: DONE

## Summary

Wired up UI for two Frontier mechanics:
1. **Structure upgrade mode** — key "u" enters a new "upgrade" mode (hides build/craft menus); left-clicking a structure in this mode calls the existing `upgrade_structure` fn (from Task 7).
2. **Flare warning bar** — a new `flare-bar` system writes a Chinese warning string to `Colony.flare_bar` when `Colony.flare_warning == 1`; a `hud-flare-bar` rule copies that string to `@flare_lbl.UiLabel.content` (silent no-op until Task 9 adds the `@flare_lbl` scene entity).

No POI UI was added (Task 3 already handles POI via direct interact-mode click).

## Files modified (4 — exactly as scoped)

- `games/frontier/scripts/hud.js`
- `games/frontier/rules/hud.json`
- `games/frontier/rules/ui.json`
- `games/frontier/rules/economy.json`

## Edits applied (verbatim from brief)

### Edit 1 — `games/frontier/scripts/hud.js`

Appended the `flare-bar` system after the existing `food-bar` system. Queries `["Colony"]`, writes `["Colony"]`, reads `e.Colony.flare_warning`, writes `e.Colony.flare_bar` = `"⚠ 耀斑即将来袭!"` (Unicode-escaped form: `"\u26a0 \u8000\u6591\u5373\u5c06\u6765\u88ad!"`) when warning is active, `""` otherwise.

```javascript
vitric.system("flare-bar", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const warn = e.Colony.flare_warning | 0;
    e.Colony.flare_bar = warn ? "\u26a0 \u8000\u6591\u5373\u5c06\u6765\u88ad!" : "";
  }
});
```

### Edit 2 — `games/frontier/rules/hud.json`

Appended `hud-flare-bar` rule to the end of the `rules` array (added comma after `hud-inv`). On `tick`, sets `@flare_lbl.UiLabel.content` from `@colony.Colony.flare_bar`. Includes a Chinese `comment` explaining that `@flare_lbl` is added in Task 9.

### Edit 3 — `games/frontier/rules/ui.json`

Added `kb-mode-upgrade` rule after `kb-mode-craft` (added comma after `kb-mode-craft`). On key "u" pressed, sets `@uistate.Mode.value` to `"upgrade"` and hides both `@build_menu.Ui.ox` and `@craft_menu.Ui.ox` (to `-3000`, same as interact mode).

### Edit 4 — `games/frontier/rules/economy.json`

Appended `upgrade-click` rule to the end of the `rules` array (added comma after `upgrade-button-click`, no trailing comma after the new rule). On `mouse` event with guard `@uistate.Mode.value == "upgrade"`, calls `upgrade_structure` with `event.entity` + full inventory (ore/wood/fiber/seed/wheat/plank/chair/lamp). The `upgrade_structure` fn handles non-structure clicks gracefully (returns early), so no structure-type filter is needed in the rule.

## Verification

### `cargo run -p vitric-cli -- check games/frontier` (verbatim tail)

```
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

Exit code: **0** (success).

`vitric gate` was NOT run, per the brief (qa/clear.json is stale; Task 9 re-records).

### `git diff --stat HEAD~1`

```
 games/frontier/rules/economy.json | 12 ++++++++++++
 games/frontier/rules/hud.json     |  8 ++++++++
 games/frontier/rules/ui.json      | 10 +++++++++-
 games/frontier/scripts/hud.js     | 10 ++++++++++
 4 files changed, 39 insertions(+), 1 deletion(-)
```

Only the 4 in-scope files changed. No other files modified.

## Commit

- Hash: `62c45fc4091fd284b1ee8eb9c1db8aba1397b5fe` (short: `62c45fc`)
- Message: `feat(frontier): UI hooks for upgrade mode + flare warning bar`
- Pushed to: `origin/main` (`5a9870f..62c45fc  main -> main`)

## Self-check checklist

- [x] hud.js: `flare-bar` system queries `["Colony"]`, writes `["Colony"]`, writes `e.Colony.flare_bar` = warning string or `""` (Edit 1)
- [x] hud.json: `hud-flare-bar` rule on tick sets `@flare_lbl.UiLabel.content` from `@colony.Colony.flare_bar` (Edit 2)
- [x] ui.json: `kb-mode-upgrade` rule on key "u" sets `Mode.value = "upgrade"` + hides both menus (Edit 3)
- [x] economy.json: `upgrade-click` rule on mouse + `Mode=upgrade` calls `upgrade_structure` with entity + full inventory (Edit 4)
- [x] JSON valid in all 3 rule files (commas correct, no trailing commas) — `cargo check` exited 0
- [x] schema.json NOT modified
- [x] vitric.json NOT modified
- [x] `cargo run -p vitric-cli -- check games/frontier` exits 0
- [x] Committed + pushed to `main`

## Concerns

None. All edits applied verbatim per brief; check passed; commit pushed cleanly.

Note (not a concern, just for Task 9 awareness): the `hud-flare-bar` rule's `set` to `@flare_lbl.UiLabel.content` will silently no-op until Task 9 adds the `@flare_lbl` entity to the scene. This is intentional per the brief and matches the engine's behavior of silently failing `set` actions on missing entities.
