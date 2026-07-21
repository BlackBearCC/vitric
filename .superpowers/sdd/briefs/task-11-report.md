# Task 11 — Trading & Diplomacy: Implementation Report

## Summary

Implemented the Trading & Diplomacy feature for the Frontier sandbox expansion:
- New `Faction` component (4 fields: `relations`, `tier_nomads`, `tier_caravan`, `tier_remnant`)
- 3 factions (nomads / caravan / remnant) with relation tiers (hostile / wary / neutral / friendly / allied)
- Barter trades via the trade menu (give/receive item pairs, tier-discounted rates)
- LLM-driven diplomatic negotiation (`ctx.ask("llm")` → `onNegotiateReply` callback with canned fallback)
- Forward-compat reinforcement hook (emits `faction-reinforcements-available` on night-fall when any faction is allied)

## Commit

- **Hash**: `f39c5f99e5d3b708790db319ba7cc9d9725e1543` (short: `f39c5f9`)
- **Branch**: `main`
- **Message**: `feat(frontier): trading & diplomacy — Faction component, 3 factions, barter trades, LLM negotiation`
- **Diff**: 9 files changed, +510 / -19
- **Pushed**: `568dc73..f39c5f9 main -> main` (origin SSH)

## Files changed (9 in-scope)

| # | File | Status |
|---|------|--------|
| 1 | `games/frontier/schema.json` | Modified — added `Faction` component; extended `Colony` with `_negotiate_target`; added `"trade"` to `Mode.value` enum |
| 2 | `games/frontier/scenes/main.json` | Modified — attached `Faction:{}` to colony entity; added `trade_menu` + 6 trade buttons/labels + 6 negotiate buttons/labels; added `mode_trade` + `mode_trade_lbl`; added `relation_lbl` HUD entity |
| 3 | `games/frontier/scripts/faction.js` | New — constants (`FACTION_IDS`, `FACTION_INFO`, `RATE_MULT_BY_TIER`, `TRADE_OFFERS`, `NEGOTIATION_FALLBACKS`), `tierFromRelation` helper, `faction-tick` system, fns `change_relation` / `complete_trade` / `negotiate` / `onNegotiateReply` / `emit_reinforcement_hook` |
| 4 | `games/frontier/rules/faction.json` | New — 6 rules: `trader-companion-relation`, 3× `negotiate-*`, `faction-allied-notify`, `faction-reinforcements-hook` |
| 5 | `games/frontier/rules/trade.json` | New — 3 rules: `trade-nomads` / `trade-caravan` / `trade-remnant` (ui-activate → `complete_trade`) |
| 6 | `games/frontier/rules/ui.json` | Modified — extended `ui-init`, mode-build/craft/interact/research/combat, kb-mode-upgrade/research/combat to hide `@trade_menu`; added `mode-trade` + `kb-mode-trade` (B key) |
| 7 | `games/frontier/rules/hud.json` | Modified — added `hud-faction-relation` rule (formats `游民[{}] 商队[{}] 遗民[{}]` from `@colony.Faction.tier_*`) |
| 8 | `games/frontier/vitric.json` | Modified — registered `scripts/faction.js` (end of scripts array) + `rules/faction.json` + `rules/trade.json` (end of rules array) |
| 9 | `crates/vitric-cli/tests/faction.rs` | New — 4 integration tests |

## Test results

### New tests (`faction`)
```
running 4 tests
test faction_tick_derives_tier_from_relations ... ok
test change_relation_clamps_to_100 ... ok
test on_negotiate_reply_applies_plus_3_relation ... ok
test complete_trade_barters_items_and_adds_relation ... ok

test result: ok. 4 passed; 0 failed
```

### Regression tests
| Suite | Result |
|-------|--------|
| `combat` | 4 passed |
| `research` | 4 passed |
| `seasons` | 4 passed |
| `companions` | 4 passed |
| `region` | 14 passed |

### Engine verification
- `cargo run --release -- check games/frontier` → **exit 0**, `check` gate status `pass` (490 entities loaded, `initial_hash: 0xdd361301f0b0a6bb`); new `faction-tick` system registered with `query: ["Colony","Faction"]`, `writes: ["Faction"]`.
- `cargo run --release -- gate games/frontier` → `pass: false` — **EXPECTED-FAIL at tick 0**:
  - `check` gate: pass
  - `playthrough:qa/clear.json` gate: fail — replay hash divergence at tick 0 (`0xb68b61d57750ff1` expected vs `0xdd361301f0b0a6bb` actual). This is expected: `qa/clear.json` is a pre-existing recorded playthrough that does not include the new `Faction` component's initial state, so the world hash diverges immediately. Not a regression — the recording needs to be re-captured separately (out of scope for Task 11).

## Self-audit checklist (`.superpowers/sdd/review-checklist.md`)

### 1. Schema field audit — ✅ pass
Every field written by a JS system OR read by a rule OR accessed via `ctx.getField`/`ctx.setField` is declared in `schema.json`:

| Field | Declared at | Used by |
|-------|-------------|---------|
| `Faction.relations` | schema.json:1082 | `faction-tick`, `change_relation`, `complete_trade`, `negotiate`, `onNegotiateReply`, `emit_reinforcement_hook` (via `ctx.getField`/`ctx.setField`) |
| `Faction.tier_nomads` | schema.json (Faction.fields) | `faction-tick` (write), `change_relation` (read for old/new), `hud-faction-relation` rule (read), `emit_reinforcement_hook` (read) |
| `Faction.tier_caravan` | schema.json (Faction.fields) | same as above |
| `Faction.tier_remnant` | schema.json (Faction.fields) | same as above |
| `Colony._negotiate_target` | schema.json:826 | `negotiate` (write), `onNegotiateReply` (read + clear) |
| `Mode.value = "trade"` | schema.json:516 (enum variant) | `mode-trade`, `kb-mode-trade` rules |

JS bracket-notation writes `c.Faction[field] = tier` (where `field` ∈ `tier_nomads`/`tier_caravan`/`tier_remnant`) — all declared. No undeclared fields introduced.

### 2. Enum variant audit — ✅ pass
- `@uistate.Mode.value` set to `"trade"` in `mode-trade` (ui.json:79) and `kb-mode-trade` (ui.json:169) → `"trade"` is declared in `Mode.value.variants` (schema.json:516).
- All other `Mode.value` sets use pre-existing variants (`build`/`craft`/`interact`/`research`/`combat`/`upgrade`).

### 3. Scene entity reference audit — ✅ pass
All `@<name>.*` references in new/modified rules resolve to entities in `scenes/main.json`:
- `@trade_menu` → added (trade menu panel + 12 child buttons/labels)
- `@relation_lbl` → added (HUD label, top-right)
- `@toast_lbl`, `@uistate`, `@colony`, `@player`, `@build_menu`, `@craft_menu`, `@tech_menu` → pre-existing

### 4. UI layout overlap audit — ✅ pass
**Top-right anchor (ox=-32):**
| Entity | oy | h | y-range |
|--------|----|---|---------|
| `hp_lbl` | 312 | 24 | [312, 336] |
| `relation_lbl` | 340 | 24 | [340, 364] |

Gap = 4px. No overlap.

**Top-left anchor (menus):**
- `build_menu` oy=176 h=700 (ox=24 when visible)
- `trade_menu` oy=176 h=360 (ox=208 when visible, ox=-3000 when hidden)

`trade_menu` and `build_menu` overlap geometrically, but **mode rules enforce mutual exclusion**: `mode-trade` sets `build_menu.Ui.ox=-3000` (ui.json:81), and `mode-build`/`mode-craft`/`mode-interact`/`mode-research`/`mode-combat` all set `trade_menu.Ui.ox=-3000`. They are never visible simultaneously. Acceptable per brief.

### 5. Standard checks — ✅ pass
- `cargo run --release -- check games/frontier` exits 0.
- All new `//` comments in English (project convention). Verified the `LLM_ERROR_FALLBACK` reuse comment in `faction.js:74`.
- String literals keep authored language (Chinese game dialogue, UI labels, panic messages preserved).
- No fake APIs — grep for `ctx.singleton`/`ctx.each`/`ctx.entity`/`ctx.llm`/`vitric.on`/`vitric.expose`/`vitric.call`/`Math.random` in `faction.js` returned no matches. Only verified APIs (`ctx.getField`, `ctx.setField`, `ctx.ask`, `ctx.emit`, `vitric.system`, `vitric.fn`) are used.
- No dead code — all 5 fns are invoked by rules (`change_relation` ← `trader-companion-relation`; `negotiate` ← 3 `negotiate-*` rules; `complete_trade` ← 3 `trade-*` rules; `emit_reinforcement_hook` ← `faction-reinforcements-hook`; `onNegotiateReply` ← `__onReply` dispatcher via `ctx.ask`).
- Commit message follows `feat(frontier): <summary>` convention.
- Only the 9 in-scope files committed (the brief file `task-11-brief.md` was deliberately left untracked — it is the spec, not a deliverable).

## Deviations from the brief

The brief's literal test code in `crates/vitric-cli/tests/faction.rs` had three bugs that required deviation (documented in the test file's top docstring):

1. **`Faction.relations` is a schema `text` field, not an object.** The brief's `set_field(&mut sim, "colony", "Faction.relations", json!({...}))` would store a JSON object, but `faction-tick`'s `JSON.parse(c.Faction.relations || "{}")` requires a string. An object value would make `JSON.parse` throw and fall back to `{}`, breaking all tier derivation.
   - **Fix**: Added a `set_relations` helper that serializes the JSON object to a string first via `serde_json::to_string(&rels)`.

2. **`complete_trade` test fixture clobbered by `seed-start`.** The brief sets `Inventory.wheat=3` / `Inventory.fiber=0` before tick 0, but the `seed-start` rule (in `rules/economy.json`) runs on the `start` event at tick 0 and overrides `fiber` to 2 (its default fixture), which would make the post-trade `fiber=2` assertion ambiguous (was it the trade or the seed-start?).
   - **Fix**: Set `wheat`/`fiber` **after** tick 0 (after `seed-start` has run), so the trade's effect is unambiguous.

3. **`llm-reply` event requires an `id` field.** The brief's `on_negotiate_reply` test injects `llm-reply` with only `{ "text": "..." }`, but the prelude's `__onReply` dispatcher (in `crates/vitric-script/src/prelude.js`) throws `"回复缺 id 或 id 不含回调名"` if the `id` field is missing or doesn't contain `#`. The engine's `ctx.ask("llm", prompt, "onNegotiateReply")` embeds the callback name as the id prefix: `"onNegotiateReply#<tick>#<idx>"`.
   - **Fix**: Capture the `llm-ask` event's `id` field from `rt.drain_observed()` after step 1, then inject `llm-reply` with `{ "id": ask_id, "text": "..." }`.

A fourth on-the-fly fix: `faction.js` originally redeclared `const LLM_ERROR_FALLBACK`, which is already declared in `scripts/wish.js`. Since QuickJS loads all scripts into a single shared global scope, the redeclaration threw `Error: redeclaration of 'LLM_ERROR_FALLBACK'` at load time. Removed the duplicate declaration in `faction.js:74` (replaced with an English comment noting the reuse); the usage at `faction.js:178` (`text === LLM_ERROR_FALLBACK`) resolves to the `wish.js` declaration. No behavior change.

## Known issues / concerns

- **`qa/clear.json` playthrough diverges at tick 0** (EXPECTED). The recording predates the `Faction` component, so the world's initial hash differs. Re-capturing the playthrough is a separate task (out of scope for Task 11). The `check` gate still passes, confirming the scene and logic load cleanly.
- **`trade_menu` / `build_menu` geometric overlap** is intentional and safe — the mode rules enforce mutual exclusion (only one menu visible at a time). Documented in the UI layout audit above.
- **`faction-reinforcements-hook`** emits `faction-reinforcements-available` on every `night-fall` if any faction is allied. No consumer exists yet (forward-compat hook for Task 12/13 region unlock / joint defense). The hook is a pure emit — no logic, deterministic.
- **`faction-allied-notify`** triggers a `Toast` (3s timer) when a faction crosses the 76-relation allied threshold. If relation oscillates around 76, this could fire repeatedly; in practice, relation deltas are small (+1 from trader companion, +2 from trades, +3 from negotiation) and clamped at 100, so oscillation is unlikely in normal play.

## Authoring notes

- `scenes/main.json` remains a single-line compact JSON file (preserved via `json.dumps(d, separators=(",",":"), ensure_ascii=False)`).
- All new code comments are in English per the 2026-07-17 project convention.
- Game dialogue / UI labels / panic messages keep their authored Chinese.
