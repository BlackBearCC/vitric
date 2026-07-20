# Task 4 Report: Wish System + Toast/Mood Listeners

## 1. What was implemented

| File | Summary |
|------|---------|
| `games/frontier/scripts/wish.js` (new) | `advance_wish` fn (advance all companions' wishes of a kind by N, +30 affinity / +1 fulfilled / +1 Colony.companion_wish_count / emit `wish-fulfilled` on completion), `apply_mood_drop` fn (decrement all companions' Need.comfort by N, resolves Task 3 F3), `wish_food_check` system (emit `food-high` once per day when Colony.food >= 80, guarded by `Colony._wish_food_day`). |
| `games/frontier/rules/wish.json` (new) | 12 rules: `wish-advance-build`, `wish-advance-build-lamp`, `wish-advance-harvest`, `wish-advance-harvest-wheat`, `wish-advance-gather-ore`, `wish-advance-poi`, `wish-advance-upgrade`, `wish-advance-food-high`, `wish-advance-see-dawn` (with `@player.Position.x >= 16` guard), `wish-fulfilled-toast`, `toast-show-generic` (resolves Task 3 F2), `companion-mood-drop-apply`. |
| `games/frontier/scripts/companion.js` (modified) | Added `WISH_TEMPLATES` constant (builder / farmer / explorer, 3 wishes each, Chinese desc literals) + `wishesForArchetype(archetype)` keyword-matching helper (技/电/匠→builder, 厨/医/农→farmer, default explorer) after `COMP_TICK_PER_SEC`. Added `Wish: { items: JSON.stringify(wishesForArchetype(args.archetype)), fulfilled: 0 }` to the `ctx.spawn({...})` call in `consumeDrifter`. |
| `games/frontier/scenes/main.json` (modified) | Added `Wish` component (builder-template items, archetype "话痨技工" matches builder) to the initial `companion` entity (Pip). Edit done via the brief's Python script (single-line compact JSON preserved). |
| `games/frontier/vitric.json` (modified) | Registered `rules/wish.json` (after `rules/affordability.json`) and `scripts/wish.js` (after `scripts/poi.js`). |

## 2. Verification output

Command: `cargo run -p vitric-cli -- check games/frontier`
Exit code: **0**

Last 25 lines of the diagnostic JSON report (systems section, ending with the new `wish_food_check` system):

```
        "Colony",
        "Clock"
      ],
      "writes": [
        "Colony"
      ]
    },
    {
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

Additional grep checks on `games/frontier/scripts/wish.js`:
- `Math\.random` → no matches (wish advancement is deterministic).
- Fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`, `ctx.entity`, `ctx.llm`) → no matches.

`wish.json` parses as valid JSON with exactly 12 rules (verified via Python `json.load`).

## 3. Commit SHA(s) pushed

- `e01d4c01a2058d2e3073d300b6ad2ad1e7ef0cfa` — `feat(frontier): add companion wish system + toast-show/mood-drop listeners`
- Pushed to `origin/main` (`4c357e0..e01d4c0 main -> main`).

Files in commit (5):
- `games/frontier/scripts/wish.js` (new)
- `games/frontier/rules/wish.json` (new)
- `games/frontier/scripts/companion.js` (modified)
- `games/frontier/scenes/main.json` (modified)
- `games/frontier/vitric.json` (modified)

`schema.json` was NOT modified (see §5).

## 4. Entity count verification

```
$ python3 -c "import json; print(len(json.load(open('games/frontier/scenes/main.json'))['entities']))"
424
```

main.json still has 424 entities (edit only added a component to the existing `companion` entity; no entities added or removed).

## 5. Colony._wish_food_day schema change

**Not required.** `vitric check` passed on the first run (exit code 0) without any schema modification. The engine permits runtime ad-hoc fields on entities (consistent with the existing `Colony._day_anchor` pattern used in companion.js), so `Colony._wish_food_day` works as an undeclared int field. `schema.json` was left untouched and is not part of the commit.

## 6. Concerns / deviations from the brief

None. All 8 steps followed exactly as specified:

- All wish.js / companion.js additions are in English (project convention); game string literals (wish descriptions, toast text `"{} 心愿达成: {}"`) stay Chinese.
- No `Math.random` and no fake APIs in wish.js (grep-confirmed).
- `apply_mood_drop` fn is in wish.js as instructed by the brief's Step 2 note (the brief's prose places it "after `advance_wish`" — placed between `advance_wish` and `wish_food_check` system).
- Step 4 main.json edit used the brief's Python script verbatim; single-line compact JSON format preserved (no trailing newline).
- Step 6 conditional schema change was NOT triggered (check passed without it).
- Self-review checklist items all satisfied.

Two Task 3 deferred findings resolved as specified:
- **F2** (`toast-show` listener): `toast-show-generic` rule in `wish.json`.
- **F3** (`companion-mood-drop` listener): `companion-mood-drop-apply` rule + `apply_mood_drop` fn in `wish.js`.
