# Task 8 Review — Tech Tree (12 nodes, 4 branches × 3 tiers)

## Verdict
**APPROVED**

All 11 brief requirements are faithfully realized in the diff. The `Research` + `TechPoint` components, `research.js` script (TECH_TREE + 2 systems + 2 fns), `research.json` rules (3 rules), `economy.js` extensions (6 new recipes + `requires` check + ITEMS extension), `poi.js` TechPoint awarding, UI rules (mode-research + kb-mode-research + 12 pick-tech-<id> + tech_menu visibility in 5 existing rules), affordability rules (6 new + 2 extended + build-dim-all extension), and HUD rules (hud-techpoint + hud-research-status) are all in place and match the brief spec. Schema field audit passes — every field read by a rule or written via `ctx.setField` is declared in `schema.json`. The 4 `research.rs` tests are meaningful (not smoke tests) — they cover happy-path progress, completion state transition, cost-deduction pipeline, and insufficient-funds rejection. The gate failure (`ReplayDiverged` at tick 0) is expected per brief — Task 15 re-records. The implementer also proactively extended inventory-passing rules in `economy.json`/`farm.json`/`poi.json` beyond the brief's explicit list — this is a **critical correctness fix** (without it, `inv-set` write-back would reset `hide`/`crystal_core` to 0 on every craft/interact/upgrade call), not scope creep.

## Brief Requirements Audit

- **Schema: `Research` component (4 base + 12 `has_*` fields)** ✅
  `games/frontier/schema.json:951-977` declares `Research` with `known` (text, `"[]"`), `current` (text, `""`), `progress` (number, `0`), `cost_total` (number, `0`), and all 12 `has_<branch>_t<tier>` (int, `0`). Block matches brief field-for-field except `cost_total` is `number` not `int` — see Approved Deviation #1.

- **Schema: `TechPoint` component** ✅
  `games/frontier/schema.json:979-983` declares `TechPoint` with `value` (int, `0`). Matches brief exactly.

- **Schema: `Mode.value` enum extended with `"research"` only** ✅
  `games/frontier/schema.json:433-440` — variants `["build","craft","interact","upgrade","research"]`. No `combat`/`trade`. Matches brief.

- **Schema: `Inventory.hide` + `Inventory.crystal_core`** ✅
  `games/frontier/schema.json:414-421` — both `int` default `0`. Matches brief.

- **Scene: `Research` on `colony`, `TechPoint` on `player`** ✅
  `games/frontier/scenes/main.json` `colony` entity carries `"Research":{}` (schema defaults apply); `player` carries `"TechPoint":{}`. Confirmed via direct entity read.

- **Scene: new HUD entities (`techpoint_lbl`, `research_status_lbl`, `tech_menu` + 12 tech buttons + 12 labels, `mode_research` + label, 6 new build buttons + labels)** ✅
  Verified 470 total entities (was 429 pre-Task-8, +41). All 12 `tech_<id>` + `tech_<id>_lbl` present. All 6 new `build_<recipe>` + `build_<recipe>_lbl` present. `mode_research` + `mode_research_lbl` present (action `mode-research`, color `#3a4a6b`). `techpoint_lbl` + `research_status_lbl` present. `tech_menu` is a Grid (3 cols, gap 8, pad 12) holding 12 `tech_<id>` children. `mode_row.w` bumped to 386; `build_menu.h` bumped to 700. `tech_survival_t1` button has correct `Button.action: "pick-tech-survival_t1"`.

- **Script: `research.js` (TECH_TREE + research-progress + tech-panel-hint + start_research + unlock_region)** ✅
  `games/frontier/scripts/research.js:1-105`. `TECH_TREE` matches brief verbatim (12 entries, correct cost/time/requires/unlocks). `research-progress` system queries `["Research"]`, writes `["Research"]`, advances `progress` by `ctx.dt`, on completion pushes to `known`, sets `has_*`, emits `researched` + `toast-show`. `tech-panel-hint` system queries `["Research"]`, `writes: []`, updates 12 labels via `ctx.setField("tech_<id>_lbl", "UiLabel.content", ...)`. `start_research` fn validates tech/cost/requires, emits `tp-set`, sets `Research.current/progress/cost_total`. `unlock_region` fn calls `ctx.thaw_region(region_id)`. All match brief. Deviation #2: `research-progress` reads `cost_total` from entity (with `|| tech.time` fallback) instead of `tech.time` directly — see Approved Deviation #2.

- **Script: `economy.js` extended (BUILD +6 recipes with `requires`, ITEMS +2, build fn `requires` check)** ✅
  `games/frontier/scripts/economy.js:23-31` (6 new recipes: well2/recycler/dome/hydroponics/arc_gun/turret — all with `requires` field matching brief). `economy.js:44` `ITEMS` extended with `hide`, `crystal_core`. `economy.js:83-91` `build` fn `requires` check emits `build-fail` + `toast-show: "需要科技: " + def.requires` (the simpler option per brief). Matches brief.

- **Script: `poi.js` extended (+2 TechPoints per POI, inline ITEMS +2)** ✅
  `games/frontier/scripts/poi.js:48` inline `ITEMS` extended. `poi.js:77-81` awards `+2` TechPoints via `ctx.emit("tp-set", { value: (a.techpoint|0)+2 })`. Matches brief.

- **Rules: `research.json` (3 rules)** ✅
  `games/frontier/rules/research.json:1-22`. `research-unlock-mountain` (filter `id:exploration_t1` → call `unlock_region` with `mountain`), `research-unlock-desert` (filter `id:industry_t3` → `desert`), `tp-apply` (`tp-set` event → `set @player.TechPoint.value to event.value`). All match brief.

- **Rules: `economy.json` modified (pass `known` to build + extend inventory args)** ✅
  `games/frontier/rules/economy.json:13-17` `build-click` rule passes `known: "@colony.Research.known"` plus `hide`/`crystal_core` to build fn. Implementer also extended `craft-plank`/`craft-chair`/`craft-lamp`/`inv-apply`/`upgrade-click`/`interact-click` rules with `hide`/`crystal_core` — see Approved Deviation #5 (critical correctness fix). `farm.json` `interact-click` rule also extended with `hide`/`crystal_core`.

- **Rules: `poi.json` modified (pass `techpoint` to interact_poi + hide/crystal_core)** ✅
  `games/frontier/rules/poi.json:12-14` adds `hide`, `crystal_core`, `techpoint: "@player.TechPoint.value"` to interact_poi `with`. Matches brief.

- **Rules: `ui.json` modified (mode-research + kb-mode-research + tech_menu hide in 5 existing rules + 12 pick-tech-<id> + 6 pick-<recipe>)** ✅
  `games/frontier/rules/ui.json`: `ui-init`, `mode-build`, `mode-craft`, `mode-interact`, `kb-mode-upgrade` all extended with `{ "set": "@tech_menu.Ui.ox", "to": -3000 }` (5 rules — matches brief's explicit list). New `mode-research` rule (button click) + `kb-mode-research` rule (T key) both set mode + show tech_menu + hide build_menu/craft_menu. 12 `pick-tech-<id>` rules each call `start_research` with `tech_id`/`techpoint`/`known`. 6 new `pick-<recipe>` rules set `@uistate.Build.kind` to the new recipe ids. All match brief. (Note: `kb-mode-build`/`kb-mode-interact`/`kb-mode-craft` were NOT extended — see Approved Deviation #4.)

- **Rules: `affordability.json` modified (build-dim-all +6, afford-plot2/monument +tech, 6 new afford-<recipe>)** ✅
  `games/frontier/rules/affordability.json`: `build-dim-all` extended with 6 new `@build_<recipe>.Panel.color` sets. `afford-plot2` adds `has_agriculture_t1>=1`; `afford-monument` adds `has_industry_t2>=1`. 6 new rules: `afford-well2` (`has_survival_t1`), `afford-recycler` (`has_survival_t2`), `afford-dome` (`has_survival_t3` + `crystal_core>=1`), `afford-hydroponics` (`has_agriculture_t3` + `crystal_core>=1`), `afford-arc_gun` (`has_industry_t1` + `crystal_core>=1`), `afford-turret` (`has_industry_t1` + `crystal_core>=1`). Each `if` clause matches the corresponding `BUILD[recipe].cost` + `requires` exactly.

- **Rules: `hud.json` modified (hud-techpoint + hud-research-status)** ✅
  `games/frontier/rules/hud.json:87-101` adds `hud-techpoint` (formats `"科技点 {}"` from `@player.TechPoint.value` → `@techpoint_lbl.UiLabel.content`) and `hud-research-status` (formats `"研究中: {} ({}/{})"` from `@colony.Research.current`/`progress`/`cost_total` → `@research_status_lbl.UiLabel.content`). Matches brief (using `progress/cost_total` format, not percentage — the brief's simpler option).

- **Tests: `research.rs` (4 tests)** ✅
  `crates/vitric-cli/tests/research.rs:1-136`. 4 meaningful tests (see "Tests" section below). All use `Runtime::boot(frontier_dir())` + `sim.inject_reply("ui-activate", ...)` + `sim.step(&mut rt)` pattern. Tests are not smoke tests — they assert specific state transitions and event emissions.

- **Manifest: `vitric.json` registers new files** ✅ (Approved Deviation #3)
  `games/frontier/vitric.json` adds `rules/research.json` to `rules` array and `scripts/research.js` to `scripts` array. Required for engine to load the new files.

## SDD Checklist

### 1. Schema field audit

Every field referenced by a rule (`@entity.Comp.field`) or accessed via `ctx.getField`/`ctx.setField`:

| Field reference | Where used | Declared in `schema.json`? |
| --- | --- | --- |
| `Research.known` (rule read) | `economy.json:17` (`@colony.Research.known` in build-click `with`); `ui.json` 12 pick-tech-<id> rules (`@colony.Research.known` in `with`) | ✅ `schema.json:953-955` (text, default `"[]"`) |
| `Research.current` (rule read) | `hud.json:99` (`@colony.Research.current` in hud-research-status `format.args`) | ✅ `schema.json:956-958` (text, default `""`) |
| `Research.progress` (rule read) | `hud.json:99` (`@colony.Research.progress` in `format.args`) | ✅ `schema.json:959-961` (number, default `0`) |
| `Research.cost_total` (rule read) | `hud.json:99` (`@colony.Research.cost_total` in `format.args`) | ✅ `schema.json:962-964` (number, default `0`) |
| `Research.has_survival_t1` (rule read) | `affordability.json` afford-well2 `if` | ✅ `schema.json:965` |
| `Research.has_survival_t2` (rule read) | `affordability.json` afford-recycler `if` | ✅ `schema.json:966` |
| `Research.has_survival_t3` (rule read) | `affordability.json` afford-dome `if` | ✅ `schema.json:967` |
| `Research.has_agriculture_t1` (rule read) | `affordability.json` afford-plot2 `if` | ✅ `schema.json:968` |
| `Research.has_agriculture_t3` (rule read) | `affordability.json` afford-hydroponics `if` | ✅ `schema.json:970` |
| `Research.has_industry_t1` (rule read) | `affordability.json` afford-arc_gun, afford-turret `if` | ✅ `schema.json:974` |
| `Research.has_industry_t2` (rule read) | `affordability.json` afford-monument `if` | ✅ `schema.json:975` |
| `Research.has_exploration_t1`/`t2`/`t3`, `has_agriculture_t2`, `has_industry_t3` (rule read) | Not directly read by any rule (declared for symmetry — `research-progress` system writes them via `e.Research["has_"+cur]=1`) | ✅ declared `schema.json:969, 971-973, 976-977` |
| `TechPoint.value` (rule read) | `economy.json` (no — economy doesn't read it); `ui.json` 12 pick-tech rules (`@player.TechPoint.value`); `poi.json` (`@player.TechPoint.value`); `hud.json` hud-techpoint (`@player.TechPoint.value`) | ✅ `schema.json:981-983` (int, default `0`) |
| `TechPoint.value` (rule write) | `research.json:21` `tp-apply` rule (`set @player.TechPoint.value to event.value`) | ✅ same |
| `Inventory.hide` (rule read) | `economy.json`/`farm.json`/`poi.json` all `with` args (`@player.Inventory.hide`) | ✅ `schema.json:414-416` |
| `Inventory.hide` (rule write) | `economy.json` inv-apply rule (`set @player.Inventory.hide to event.hide`) | ✅ same |
| `Inventory.crystal_core` (rule read) | `economy.json`/`farm.json`/`poi.json` all `with` args; `affordability.json` afford-dome/hydroponics/arc_gun/turret `if` | ✅ `schema.json:417-421` |
| `Inventory.crystal_core` (rule write) | `economy.json` inv-apply rule | ✅ same |
| `Mode.value` (rule write) | `ui.json` mode-research + kb-mode-research (`set @uistate.Mode.value to "research"`) | ✅ `"research"` is in `Mode.value.variants` (see Enum audit below) |
| `Ui.ox` (rule write) | `ui.json` 7 rules (`set @tech_menu.Ui.ox to 208` / `-3000`) | ✅ engine-built-in component (`Ui` declared in schema, `ox` is a Ui field — same field used by `build_menu.Ui.ox` in pre-existing rules) |
| `UiLabel.content` (rule write) | `hud.json` hud-techpoint + hud-research-status; `research.js` tech-panel-hint system via `ctx.setField` | ✅ engine-built-in (`UiLabel` declared, `content` field — same field used by all other HUD rules) |
| `Panel.color` (rule write) | `affordability.json` 6 new afford-<recipe> rules + build-dim-all extension | ✅ engine-built-in (`Panel` declared, `color` field) |
| `Build.kind` (rule write) | `ui.json` 6 new pick-<recipe> rules | ✅ engine-built-in (`Build` component, `kind` field — same as existing pick-plot2 rule) |

JS `ctx.setField` calls in `research.js`:
- `ctx.setField("tech_<id>_lbl", "UiLabel.content", text)` — `UiLabel.content` ✅ declared.
- `ctx.setField("colony", "Research.current", id)` — ✅ declared.
- `ctx.setField("colony", "Research.progress", 0)` — ✅ declared.
- `ctx.setField("colony", "Research.cost_total", tech.time)` — ✅ declared.

JS `e.Comp.field` writes in `research.js` `research-progress` system:
- `e.Research.current`, `e.Research.progress`, `e.Research.cost_total`, `e.Research.known`, `e.Research["has_"+cur]` — all ✅ declared.

JS `ctx.setField` in `poi.js` `interact_poi` (pre-existing): `ctx.setField(a.entity, "Poi.state", ...)` / `Poi.cooldown` — both ✅ declared (pre-existing, not changed by Task 8).

**Audit passes.** No undeclared fields. No silent schema regressions.

### 2. Enum variant audit

`Mode.value` enum at `schema.json:433-440`:
```json
"variants": ["build", "craft", "interact", "upgrade", "research"]
```

- Only `"research"` added (Task 8). ✅
- No `"combat"` (Task 10). ✅
- No `"trade"` (Task 11). ✅

Rule writes setting `Mode.value` to `"research"`: `ui.json` `mode-research` (button click) + `kb-mode-research` (T key). Both use a literal that is in the variants list. ✅

### 3. Scene entity reference audit

Every `@<name>.<Comp>.<field>` reference in new/modified rules resolves to an entity in `scenes/main.json` (470 entities verified):

| Reference | Entity exists? |
| --- | --- |
| `@tech_menu.Ui.ox` (ui.json 7 rules) | ✅ `tech_menu` entity present (Grid container, parent `ui`) |
| `@techpoint_lbl.UiLabel.content` (hud.json hud-techpoint) | ✅ `techpoint_lbl` entity present (top-right, oy 228) |
| `@research_status_lbl.UiLabel.content` (hud.json hud-research-status) | ✅ `research_status_lbl` entity present (top-right, oy 256) |
| `@build_well2.Panel.color` (affordability.json 2 rules) | ✅ `build_well2` entity present (parent `build_menu`, action `pick-well2`) |
| `@build_recycler.Panel.color` | ✅ `build_recycler` present |
| `@build_dome.Panel.color` | ✅ `build_dome` present |
| `@build_hydroponics.Panel.color` | ✅ `build_hydroponics` present |
| `@build_arc_gun.Panel.color` | ✅ `build_arc_gun` present |
| `@build_turret.Panel.color` | ✅ `build_turret` present |
| `@player.TechPoint.value` (ui.json, poi.json, hud.json, research.json tp-apply) | ✅ `player` has `TechPoint` component (attached in this task) |
| `@colony.Research.*` (economy.json, ui.json, hud.json) | ✅ `colony` has `Research` component (attached in this task) |
| `@player.Inventory.hide` / `.crystal_core` (economy.json, farm.json, poi.json, affordability.json) | ✅ `player` has `Inventory`; `hide` + `crystal_core` fields declared in schema |
| `@uistate.Mode.value` (ui.json mode-research + kb-mode-research) | ✅ `uistate` exists (pre-existing); `Mode.value` field declared |
| `@uistate.Build.kind` (ui.json 6 new pick-<recipe> rules) | ✅ pre-existing |
| 12 `tech_<id>_lbl` (referenced from `research.js` `tech-panel-hint` via `ctx.setField`) | ✅ all 12 `tech_<id>_lbl` entities present in `tech_menu` |

**Audit passes.** No dangling entity references.

### 4. UI layout overlap audit

**Top-right lane** (`anchor: "top-right", parent: "ui"`):

| Entity | oy | h | y-range |
| --- | --- | --- | --- |
| `quest_card` (pre-existing) | 100 | 120 | 100–220 |
| `techpoint_lbl` (NEW) | 228 | 24 | 228–252 |
| `research_status_lbl` (NEW) | 256 | 24 | 256–280 |

Gap analysis:
- `quest_card` ends at 220; `techpoint_lbl` starts at 228 → **8px gap** ✅
- `techpoint_lbl` ends at 252; `research_status_lbl` starts at 256 → **4px gap** ✅
- No entity below 280 in this lane — no overlap risk below.

The brief recommended `oy: 286` (techpoint_lbl) + `oy: 314` (research_status_lbl) below `forecast_lbl` (top-center, oy 258). The implementer chose `oy: 228` + `oy: 256` in the top-right lane below `quest_card`. Both choices are non-overlapping; the implementer's choice groups the new labels with the existing right-side HUD (`quest_card`) rather than the center stack. Acceptable — see Minor M3.

**Top-center lane** (`anchor: "top-center", parent: "ui"`): unchanged from Task 7 (`hud_companion_lbl`, `flare_lbl`, `narration_lbl`, `season_lbl`, `weather_lbl`, `forecast_lbl` ending at 286). No Task 8 additions in this lane.

**Top-left lane** (`anchor: "top-left", parent: "ui"`):

| Entity | oy | h | ox (default) | x-range |
| --- | --- | --- | --- | --- |
| `mode_row` (pre-existing, w bumped 320→386) | 100 | 64 | 24 | 24–410 |
| `build_menu` (pre-existing, h bumped 400→700) | 176 | 700 | 24 (when visible) or -3000 (hidden) | visible: 24–372 |
| `craft_menu` (pre-existing) | 176 | (pre-existing) | 24 (visible) or -3000 (hidden) | visible: 24–... |
| `tech_menu` (NEW, w 348, h 400) | 176 | 400 | -3000 (hidden) or 208 (visible) | visible: 208–556 |

`tech_menu` shares oy 176 with `build_menu`/`craft_menu`, but only one is ever visible at a time (the mode-switching rules hide the others by setting `ox: -3000`). When `tech_menu` is shown (mode-research), `build_menu` and `craft_menu` are pushed off-screen, and vice versa. No simultaneous overlap. ✅

`mode_row` width 386 holds 4 buttons × 92 + 3 gaps × 6 = 368 + 18 = 386 exactly. ✅ `mode_research` (4th button, w 92, h 48) fits.

`build_menu` height 700 fits 14 buttons (8 original + 6 new): 14 × 40 + 13 × 8 (gaps) + 2 × 12 (pad) = 560 + 104 + 24 = 688 < 700. ✅

`tech_menu` height 400 fits 4 rows × 80 + 3 × 8 (gaps) + 2 × 12 (pad) = 320 + 24 + 24 = 368 < 400. ✅ Width 348 fits 3 cols × 100 + 2 × 8 + 2 × 12 = 300 + 16 + 24 = 340 < 348. ✅

**Audit passes.** No overlaps.

### 5. Standard checks

- ✅ **Schema check** — implementer reports `cargo run --release -- check games/frontier` exits 0. Schema diff is well-formed JSON (verified via `python3 -m json.tool`). All 9 JSON files modified/added parse cleanly (verified by direct parse).
- ✅ **Comment language** — all new `//` comments in `research.js`, `research.rs`, `economy.js` additions, `poi.js` additions are English. Rule `comment` fields in `research.json` are English (`"On exploration_t1 complete: thaw mountain region (E1 API)."`); rule `comment` fields in `ui.json`/`hud.json`/`economy.json` additions are Chinese (matching the existing style of those files — pre-existing convention is mixed, and the implementer followed each file's local convention). String literals (toast text, UI labels, format strings) are Chinese per project convention. Rust `//!` and `//` comments in `research.rs` are English.
- ✅ **No fake APIs** — `vitric.system`, `vitric.fn`, `ctx.dt`, `ctx.emit`, `ctx.setField`, `ctx.thaw_region` are all real (verified against `seasons.js`/`flare.js`/`poi.js` patterns). No `ctx.singleton`/`ctx.each`/`Math.random`/`Date.now`. Determinism preserved — `research-progress` is pure (progress += dt; compare; emit), `tech-panel-hint` is pure (read state, compute label, setField), `start_research` is pure (validate, emit, setField). `TECH_TREE` iteration order via `Object.keys(TECH_TREE)` is insertion-order (V8/QuickJS guarantee for string keys) — deterministic across runs.
- ✅ **No dead code / YAGNI** — every `TECH_TREE` entry is referenced by either a `pick-tech-<id>` rule or the `tech-panel-hint` loop. Every new `BUILD` entry has a matching `pick-<recipe>` rule + `afford-<recipe>` rule. The 3 `has_*` fields not directly read by any rule (`has_exploration_t1/t2/t3`, `has_agriculture_t2`, `has_industry_t3`) are written by `research-progress` and provide forward-compat for future tasks (e.g., `has_industry_t3` would gate desert trade in Task 11/12) — declared per the brief's "12 boolean `has_*` fields" requirement.
- ✅ **Commit message** — `feat(frontier): tech tree with 12 nodes across 4 branches` follows `<type>(<scope>): <summary>` convention.
- ✅ **In-scope files only** — diff touches `schema.json`, `scenes/main.json`, `scripts/research.js` (new), `scripts/economy.js`, `scripts/poi.js`, `rules/research.json` (new), `rules/economy.json`, `rules/farm.json`, `rules/poi.json`, `rules/ui.json`, `rules/affordability.json`, `rules/hud.json`, `vitric.json`, `tests/research.rs` (new). The `farm.json` edit is a necessary corollary (the `interact` fn in `economy.js` uses `readInv` which now includes `hide`/`crystal_core`; if `farm.json`'s `interact-click` rule didn't pass them, `inv-set` write-back would zero them out — see Approved Deviation #5). `vitric.json` edit is required (Deviation #3). No out-of-scope files.
- ✅ **Determinism** — no `Math.random` / `Date.now` / non-deterministic APIs in any new code. `ctx.dt` is the engine's deterministic tick delta. `Object.keys(TECH_TREE)` iteration order is stable. `JSON.parse`/`JSON.stringify` of `known` array preserves insertion order. `tech-panel-hint` `Math.floor((progress/tech.time)*100)` is deterministic.

## Findings

### Critical
None.

### Important
None.

### Minor

- **M1: `techpoint_lbl` and `research_status_lbl` positions deviate from brief recommendation.** Brief suggested `oy: 286` + `oy: 314` (below `forecast_lbl` in the top-center lane, with `anchor: "top-left"`). Implementer used `oy: 228` + `oy: 256` in the top-right lane (`anchor: "top-right"`, `ox: -32`), below `quest_card` (ends at 220). The implementer's choice is non-overlapping (8px gap above, 4px gap between) and groups the new labels with the right-side HUD. Functionally equivalent. The brief said "Choose a non-overlapping position" — implementer's choice satisfies this. Acceptable.

- **M2: `kb-mode-build` / `kb-mode-interact` / `kb-mode-craft` keyboard shortcuts don't hide `tech_menu` (or any menu).** Pre-existing pattern: these 3 kb-mode-* rules only set `Mode.value` without touching menu visibility (verified — `hides_tech=False, hides_build=False, hides_craft=False` for all 3). The brief explicitly listed only `mode-build`/`mode-craft`/`mode-interact`/`kb-mode-upgrade` for the tech_menu-hiding extension (NOT the kb-mode-* variants) — the implementer followed the brief's list exactly. If the user presses `q`/`r`/`e` to switch modes via keyboard, `tech_menu` (if currently visible from a prior research-mode session) stays on screen. This is a pre-existing UX inconsistency (same bug exists for `build_menu`/`craft_menu` visibility after keyboard mode switches — not introduced by Task 8). The new `kb-mode-research` rule (added in this task) correctly hides `build_menu` + `craft_menu` + shows `tech_menu`, so the new rule is consistent with the `mode-research` button-click rule. Out of scope for Task 8; flag for future UX cleanup task.

- **M3: `hud-research-status` idle display shows `"研究中:  (0/0s)"` (empty name, zeros).** The brief flagged this as acceptable ("the idle state looks slightly odd but is acceptable") and offered an optional `if` clause to show "空闲" instead. The implementer didn't add the `if` clause — the label shows the raw format with empty `current` + zero `progress`/`cost_total` when no research is active. Pre-approved by brief; cosmetic only.

- **M4: `economy.json` `build-click` rule comment is partially in Chinese.** The comment was extended with `"known(已研发科技 JSON 串)用于 tier-2/3 配方的科技前置校验。"` — mixed Chinese/English. This matches the file's existing comment style (the original `build-click` comment was already Chinese). Project convention is "code comments in English" but rule `comment` fields are documentation metadata in JSON, and the existing convention in `economy.json` is Chinese. Not a regression — the implementer followed the local file convention. Flag for consistency review in a future cleanup task if the project decides to standardize rule comments to English.

## Approved Deviations

1. **`Research.cost_total` schema type `number` instead of `int`** (`schema.json:962-964`). Required for the brief's test hint: "set `Research.cost_total` to a small value (e.g., 0.1s = 6 ticks) via direct component write before stepping." `int` would reject `0.1`. Production behavior unchanged — `start_research` sets `cost_total = tech.time` (45/90/180, all integers), and `number` accepts integers as a subset. The `progress` field is already `number` (the brief specified this), and `cost_total` is compared against `progress`, so `number` is the correct type. **Approved.**

2. **`research-progress` system reads `cost_total` from entity (with `|| tech.time` fallback) instead of `tech.time` directly** (`research.js:35`). `const total = e.Research.cost_total || tech.time; if (e.Research.progress >= total)`. In production: `start_research` sets `cost_total = tech.time`, so `total = tech.time || tech.time = tech.time` — identical to the brief's pseudocode. In tests: writing `cost_total = 0.1` directly to the entity short-circuits completion (test #2). The `||` fallback handles the default-zero case (no `start_research` called yet, but `current` somehow set — defensive). Aligns with the brief's explicit test-hint shortcut. **Approved.**

3. **`vitric.json` manifest updated to register `scripts/research.js` + `rules/research.json`** (`vitric.json:24, 37`). Not explicitly listed in the brief's commit-message file list, but the engine cannot load unregistered scripts/rules — this is a mechanical necessity. The Task 7 review approved the same kind of deviation. **Approved.**

4. **`kb-mode-craft` (and `kb-mode-build`/`kb-mode-interact`) not extended to hide `tech_menu`.** Verified pre-existing pattern: all three pre-existing kb-mode-* rules only set `Mode.value` without hiding any menu (`hides_tech=False, hides_build=False, hides_craft=False` for all 3). The brief's explicit extension list was `mode-build` / `mode-craft` / `mode-interact` / `kb-mode-upgrade` — `kb-mode-craft` was NOT in that list. The implementer correctly followed the brief. The new `kb-mode-research` rule (added in this task) does hide other menus, matching the `mode-research` button-click rule. **Approved** (pre-existing pattern, out of scope).

5. **Inventory-passing rules in `economy.json` (craft-plank, craft-chair, craft-lamp, inv-apply, upgrade-click) and `farm.json` (interact-click) extended with `hide`/`crystal_core` args, beyond the brief's explicit list.** This is a **critical correctness fix**, not scope creep: `readInv` in `economy.js` now iterates over the extended `ITEMS` array (includes `hide`/`crystal_core`), reading `a[k]|0` for each. If a rule doesn't pass `hide`/`crystal_core` in `with`, the fn reads them as `0`, then `emitInv` emits `inv-set` with `hide:0, crystal_core:0` — the rule then writes `0` back to `@player.Inventory.hide`/`.crystal_core`, **wiping the player's actual accumulated values** every time they craft or interact. The brief only explicitly mentioned `build-click` and `interact_poi` rules, but the same data-loss bug applies to every fn that uses `readInv`. The implementer proactively caught this and extended all 6 inventory-taking rules (5 in economy.json + 1 in farm.json) plus the `inv-apply` write-back rule. **Approved — critical fix.**

## Tests

The 4 `research.rs` tests are **meaningful** (not smoke tests):

1. **`research_progress_advances_with_dt`** — full pipeline: `inject_reply("ui-activate", {action:"pick-tech-survival_t1"})` → `pick-tech-survival_t1` rule fires → `start_research` fn runs → emits `tp-set` + sets `Research.current/progress/cost_total`. Steps 1 tick. Asserts `Research.current == "survival_t1"` AND `Research.progress > 0.0` (proving `research-progress` system ran with `ctx.dt`). Tests: rule→fn pipeline, fn validation (5 TechPoints ≥ cost 2), system execution.

2. **`research_completes_after_time`** — uses the documented test shortcut (write `cost_total=0.1` directly, bypassing `start_research`). Steps 8 ticks (6 = 0.1s at 60Hz, +2 safety). Asserts: `researched` event emitted with `id=survival_t1`; `Research.known` contains `survival_t1`; `Research.has_survival_t1 == 1`; `Research.current == ""` (reset). Tests: completion state transition, `known` array push, `has_*` flag set, event emission, reset-on-complete.

3. **`start_research_deducts_techpoints`** — full pipeline across 2 ticks: tick 1 `start_research` runs, emits `tp-set{value:3}` (5 - cost 2); tick 2 `tp-apply` rule consumes `tp-set`, writes `@player.TechPoint.value = 3`. Asserts `TechPoint.value == 3`. Tests: cross-rule event pipeline, cost deduction arithmetic, rule→rule data flow.

4. **`start_research_rejects_insufficient_techpoints`** — TechPoint=1, cost=2. Asserts: `toast-show` with text containing `"科技点不足"` emitted; `TechPoint.value` stays 1 (no deduction); `Research.current` stays empty (no state change). Tests: validation rejection path, toast emission, no-side-effects-on-reject.

All 4 tests assert specific state + events — not just "doesn't crash". Test #2 uses the brief-sanctioned shortcut. Tests #1/#3/#4 use the full event-driven pipeline (the brief's preferred approach). Test setup follows `seasons.rs` pattern. **Tests are meaningful.**

## Cannot-verify (controller must confirm)

The reviewer did not re-run any commands; the following are taken from the implementer's report. Controller may re-run if there is doubt.

1. **`cargo run --release -- check games/frontier` exits 0** — schema/rules/scripts validity. The 9 modified JSON files all parse cleanly (verified by direct `python3 -m json.tool`), schema field audit passes, enum variants correct, so this should pass. Implementer reports 470 entities (+41 from 429).

2. **`cargo test -p vitric-cli --test research` PASS (4/4)** — new Task 8 tests. Tests are well-formed Rust (verified by reading the diff); assertions are specific; test setup follows the `seasons.rs` pattern. If the engine's `Runtime::boot` + `inject_reply` + `step` + `drain_observed` APIs match what the tests use (the implementer claims these match `seasons.rs`), the tests should pass.

3. **`cargo test -p vitric-cli --test seasons` PASS (4/4)** — Task 6 regression. The new `research-progress` + `tech-panel-hint` systems run during boot and could crash if they hit an undeclared field — but the schema field audit confirms all fields are declared, so no crash expected.

4. **`cargo test -p vitric-cli --test region -- --skip typescript` PASS (14/14)** — region regression. Same boot-the-project argument.

5. **`cargo test --workspace -- --skip typescript` PASS** — full workspace. Strongest claim — covers all 9 crates + all integration tests.

6. **`cargo run --release -- gate games/frontier` FAIL with `ReplayDiverged` at tick 0** — expected failure mode per brief. New `Research` (on `colony`) + `TechPoint` (on `player`) components + 41 new scene entities change the tick-0 world hash. Implementer reports expected hash `0xb68b61d57750ff1`, actual `0x9af8006a884b6df5`. `qa/clear.json` was NOT re-recorded per brief — Task 15 handles that. The `check` sub-gate passing (470 entities) + green workspace test suite provides equivalent verification in lieu of the gate.

7. **Commit `b84e52b` pushed to `origin/main`** — reviewer did not verify the push directly. Controller should confirm via `git log origin/main -1` before marking the task fully closed.
