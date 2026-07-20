# Frontier Sandbox Expansion — Design Spec

**Date**: 2026-07-20
**Status**: Approved (brainstorming complete, pending plan + implementation)
**Owner**: Vitric project

## 1. Goal & Scope

Expand frontier from a 9-day short demo into a deep, sandbox-grade game with normal pacing. Two simultaneous objectives:

1. **A genuinely good sandbox game** — playable for hours, with strategic depth.
2. **An engine capability showcase** — every system exercises a real Vitric feature (deterministic replay, RPC control, rules + scripts split, agent observability).

### Deliverables (one-shot, no phases)

- **6 new game systems**: seasons/weather, combat, tech tree, expanded companions, trading/diplomacy, map expansion.
- **5 engine capabilities** (E1-E5, see §3): Region dormant/active, catch_up scheduling, seeded RNG substreams, view-frustum culling, snapshot/replay/describe plumbing.
- **Pacing rebalance**: day length, stage thresholds, gate recording.
- **README upgrade**: synchronized with game development, showcases the sandbox.

### Non-goals

- Multiplayer.
- Procedural narrative generation beyond existing LLM-dialogue hooks.
- New art pipeline work (reuse existing assets; new sprites are out of scope).
- Mobile/WASM targets (separate roadmap items).

## 2. Architecture Boundary — Engine vs Game

**Principle**: everything stays data. No component-external state channels.

### Engine capabilities (5)

| ID | Capability | Why engine |
|---|---|---|
| **E1** | `Region` component + dormant/active state; `world.query`, renderer, sim tick all skip dormant; state_hash still covers dormant entities | Game layer cannot intercept `world.query` returns, renderer traversal, or hash content |
| **E2** | Systems declare optional `catch_up(entity, ctx, dormant_ticks)`; engine invokes on thaw | "How long was this frozen" is engine-only knowledge; game-layer computation would desync from hash checkpoints |
| **E3** | Seeded RNG substreams: `ctx.random_stream("region:mountain")`; seed = f(world_seed, stream_name); substream state persisted into snapshot + hashed | On-demand region generation must be deterministic regardless of thaw timing; global RNG would fork on replay if thaw time differs |
| **E4** | View-frustum culling in renderer (CPU rasterizer + GPU mirror); culls dormant AND off-screen entities from draw pipeline | Renderer-only info (camera transform, viewport); no state mutation, no hash impact |
| **E5** | Snapshot/restore, `render/describe` (dormant dimension in offscreen classification), `vitric replay` consistency for spawn-on-thaw | Plumbing for E1+E3; not standalone but must be explicit to avoid gaps |

### Game-layer systems (6 + pacing)

All implemented via the existing rules JSON + scripts JS split. No new engine concepts.

| System | Implementation surface |
|---|---|
| Seasons/Weather | New `Season` + `Weather` components on Clock/Colony entities; modify existing `crop-grow`, `colony` tally, `flare` systems to read multipliers |
| Combat | New `Hp`, `Enemy`, `Weapon`, `Guard` components; new `combat` mode; enemies spawned by `night-fall{threat}` events |
| Tech Tree | New `Research`, `TechPoint` components; new `research` mode + tech panel UI; recipes gain `requires` field |
| Companions expansion | Extend `DRIFTER_POOL`; add `Persona.role`; archetype templates 3 → 6 |
| Trading/Diplomacy | New `Faction` component (JSON on colony); trader NPCs; reuse `ctx.ask("llm")` channel |
| Map content | Per-region POI tables, node densities, reward tables — data consumed by E3 generator |
| Pacing rebalance | Constants and stage threshold edits |

### Explicitly rejected

- **Engine "env system" for global weather modifiers** — would create a component-external state channel, violating "everything is data". Weather is a component on a global entity; systems read it themselves (same pattern as existing `Colony.is_night`).

## 3. Engine Capabilities (Detailed)

### E1 — Region dormant/active

**Component** (new, declared in frontier's `schema.json` — game-level, since it's just data; the engine behavior that reads it is what's engine-level):

```
Region {
  id: text,                    // "home", "wild", "mountain", ...
  biome: text,                 // "home", "wild", "mountain", "swamp", "desert"
  state: enum[dormant, active, frozen],
  discovered: int,             // 0 = never entered, 1 = has been entered
  anchor_x: number,            // top-left world coord
  anchor_y: number,
  w: int,
  h: int
}
```

**Engine changes**:
- `world.query(&[...])` filters out entities whose `Region.state == dormant` (entity must have `Region` component to be filtered; entities without `Region` are always visible — backward compatible).
- Renderer's `render_world` skips entities with dormant `Region`.
- Sim's logic system dispatch skips dormant-region entities (motion/collision still runs for active entities only).
- `state_hash()` includes dormant entities — they're still in the world, just inactive.

**State transitions**:
- `dormant → active`: player crosses region boundary AND unlock conditions met → emit `region-thaw{id}` → engine invokes all declared `catch_up` for systems that have one (only if `discovered == 1`, i.e., re-entry; first discovery has nothing to catch up).
- `active → frozen`: explicit game-layer rule sets state to `frozen` (e.g., player leaves region; optional behavior, default: regions stay active once thawed — see §3.1 for decision).
- `frozen → active`: same as dormant → active but always triggers catch_up.

**§3.1 Decision: do regions refreeze on leave?** Default **no** — once thawed, stays active. Refreeze is an optimization for very large worlds; 256×256 with 5 regions doesn't need it. If perf requires later, can be added without changing the API.

### E2 — Catch_up declaration

**System API extension** (scripts JS):

```javascript
vitric.system("crop-grow", { query: ["Crop", "Position"], writes: ["Crop"] },
  function(entities, ctx) { /* normal per-tick logic */ },
  function catch_up(entity, ctx, dormant_ticks) {
    // Called once on thaw. Compute accumulated state in deterministic manner.
    // e.g., for Crop: advance stage by floor(dormant_ticks / STAGE_TICKS)
  }
);
```

- `catch_up` is optional. Systems that don't declare it are Type A (truly frozen).
- Engine calls catch_up exactly once per thawed entity, in system declaration order, before the next tick's normal logic runs.
- Inside catch_up, `ctx` provides `random_stream("catch_up:<entity_id>")` for any randomized settlement (deterministic per entity).

**Type A vs B classification** (per existing system):
- Type A (no catch_up): enemy position, combat state, build queue, NPC wander target.
- Type B (catch_up declared): crop growth, resource node cooldown, affinity decay, season effect accumulation, faction relation passive drift.

### E3 — Seeded RNG substreams

**API** (scripts JS):
```javascript
ctx.random_stream(name)  // returns { next(): number [0,1), nextInt(min,max): int }
```

- Seed derivation: `f(world_seed, name)` — deterministic, independent of call timing.
- Substream state (current counter) is part of world snapshot and included in `state_hash`.
- Multiple calls to `random_stream("region:mountain")` return the same stream object (resumed from last position).
- Used for: region content generation on first thaw, catch_up random outcomes, any future "per-region deterministic content" need.

**Why not global RNG**: global RNG consumed during region generation would advance the global stream; if replay runs with different thaw timing (e.g., player explores mountain on day 8 vs day 12), global RNG diverges → all downstream rolls differ → state_hash breaks.

### E4 — View-frustum culling

**Renderer changes**:
- Both CPU rasterizer (`render_world`) and wgpu GPU mirror.
- Cull entities whose `Position + Sprite` AABB doesn't intersect camera viewport (expanded by small margin for shadow casters).
- Also cull dormant-region entities (E1).
- No semantic change — `render/describe` still reports off-screen entities for agent observability (just doesn't draw them).

**Performance target**: render-time scaling should be O(visible entities) not O(total entities). Verify with benchmark.

### E5 — Plumbing

- `snapshot()` / `restore()`: already full-world; just verify dormant entities round-trip correctly.
- `state_hash()`: verify dormant entities contribute (already should — they're still in world).
- `render/describe`: extend offscreen classification with `dormant` dimension (entity is dormant and offscreen → `dormant`; dormant and onscreen shouldn't happen but flag if it does).
- `vitric replay`: test that a recording with region-thaw events replays hash-identical.
- `vitric gate`: test that gate recording still passes after region support added.

## 4. Game Systems (Detailed)

### 4.1 Seasons & Weather

**Components** (on Clock entity):
```
Season { current: enum[spring, summer, autumn, winter], day_in_season: int, year: int }
Weather { current: enum[clear, cloudy, rain, storm, flare], timer: number, next: enum[...] }
```

**Calendar**:
- 1 day = 90 seconds (was 60).
- 1 season = 12 days.
- 1 year = 48 days = 4 seasons.

**Season effects** (multipliers applied by `crop-grow` and `colony` tally):

| Season | Crop growth | Resource yield | Threat | Companion mood |
|---|---|---|---|---|
| Spring | ×1.2 | ×1.0 | 0 | +5 |
| Summer | ×1.0 | ×0.8 | 1 | 0 |
| Autumn | ×1.5 | ×1.2 | 0 | +3 |
| Winter | ×0.3 | ×0.5 | 2 | -5 |

Winter additionally: fuel consumption ×1.5.

**Weather**:
- Switches every 30-90s, weighted by season.
- States: clear (standard), cloudy (solar ×0.7), rain (water +1/s, crop _tend_t accelerated), storm (power ×0.3, move ×0.7, tier-1 structures 5% degrade risk), flare (replaces current flare-hit; summer only; -40% power + oxygen).
- 7-day forecast bar in HUD.

**Integration**:
- `scripts/clock.js` advances season (every 12 days).
- `scripts/flare.js` refactored to weather system; flare becomes one weather variant.
- `scripts/crops.js` reads Season for STAGE_SECONDS modifier.
- `scripts/colony.js` tally reads Weather for production rate modifier.

### 4.2 Combat

**Components** (new):
```
Hp { value: number, max: number }
Enemy { kind: text, damage: number, aggro_range: number, home_region: text }
Weapon { kind: text, damage: number, range: number, cooldown: number, _cd_t: number }
Guard { post_x: number, post_y: number, patrol_r: number }
```

**Enemy types** (unlock schedule):
- Gnawer (day 1+): melee, prioritizes structures.
- Raider (after mountain unlocked): ranged, targets player.
- Sandbeast (desert region only): area-resident, doesn't enter home.

**Spawn logic**:
- `night-fall{threat}` event → rule calls `spawn_wave` fn → wave size = f(threat, active_region_count, day).
- Spawn position: just outside home region boundary, on wild side.

**Combat loop**:
- Enemy AI: straight-line path + collision-bypass (no A* — deterministic and sufficient).
- Player: new `combat` mode; keys 1-3 select weapon (stone-axe / spear / arc-gun — latter two tech-tree locked).
- Companion: if `role == guard` AND `affinity >= 60`, auto-defends (extension of existing `companion-contribution` pattern).
- Structure: turret (tech-locked) auto-attacks enemies in range.

**Settlement**:
- Enemy dies → drop into Inventory (new materials: hide, crystal-core — inputs for tech tree + trade).
- Structure Hp = 0 → downgrade tier (cheaper to repair than rebuild).
- Player Hp = 0 → respawn at lander, -20% daily resources (no game-over; sandbox).

**Freeze semantics**:
- Type A (no catch_up): enemy position, combat state, build queue.
- Type B (catch_up): wild nest respawn cooldown.

### 4.3 Tech Tree

**Components** (new):
```
Research { known: text(JSON array), current: text, progress: number, cost_total: int }
TechPoint { value: int }
```

**Tree** (4 branches × 3 tiers):

| Branch | T1 | T2 | T3 |
|---|---|---|---|
| Survival | Improved Well | Water Recycling | Atmosphere Dome (weather-immune) |
| Agriculture | Greenhouse (winter ×0.6) | Drip Irrigation (rain skips tending) | Hydroponics (year-round ×1.0) |
| Exploration | **Unlock mountain region** | Radar (POIs visible on map) | Beacon Network (region teleport) |
| Industry | Arc-gun + Turret | Alloy Structures (storm-immune) | **Unlock desert trade route** |

**Mechanics**:
- Research consumes TechPoints + real time (T1 = 0.5 day, T3 = 2 days; runs in background).
- On completion: emit `researched{id}` → rule unlocks recipe (adds `requires` field to BUILD/CRAFT tables; `affordability` rule extended to check `requires`).
- TechPoints earned from: POI exploration (existing POI system extended), research-station structure output, trade.

**Integration with existing**:
- `Structure.tier` retained — vertical upgrade via existing `upgrade` mode.
- Tech tree is horizontal unlock (new recipes / new regions).
- Two concepts stay separate; don't merge.

**UI**: new `research` mode (5th in Mode enum); tech panel reuses build_menu layout pattern.

### 4.4 Companions Expansion

**Persona extension**:
- `Persona.role` field added: enum[builder, farmer, explorer, guard, trader, scholar].
- `DRIFTER_POOL`: 6 → 12 entries, 2 per role.
- `drifters_spawned` cap: 4 → 8.
- Spawn cadence: from fixed 2-day to modulated by stage + faction relation.

**Role-driven contribution** (extension of `companion-contribution`):
- builder: passive build-speed bonus when nearby active construction.
- farmer: passive crop _tend_t acceleration (existing behavior, now role-gated).
- explorer: unlocks swamp region when in party.
- guard: auto-defends during combat (see §4.2).
- trader: enables随身 trade menu (see §4.5).
- scholar: passive TechPoint generation when near research-station.

**Wish system extension**:
- Templates: 3 → 6 (one per archetype).
- New "collective wish" type: colony-level goals (e.g., "granary reserve 50"), fulfillable by any companion contributing.

### 4.5 Trading & Diplomacy

**Faction component** (JSON field on colony entity):
```
Faction { relations: text(JSON) }  // {"nomads": 30, "caravan": 0, "remnant": -10}
```

Derived `tier` per faction (computed by rule): hostile (-100..-50), wary (-49..10), neutral (11..40), friendly (41..75), allied (76..100).

**Three factions**:
- Nomads (荒原游民): native to wild region; baseline neutral.
- Caravan (商队): desert region; trade-focused.
- Remnant (遗民): mountain region; lore/tech-focused.

**Relation drivers**:
- Trade with faction member: +2 per transaction.
- Complete faction commission (派系委托): +10.
- Attack faction member: -20.
- Refuse faction request: -5.

**Tier effects**:
- wary or below: caravans don't visit.
- neutral: basic trade rates.
- friendly: unlock faction-specific recipes.
- allied: unlock faction's region (desert for caravan, mountain深处 for remnant) + joint defense (faction reinforcements during night raids).

**Trading UI**: `trade_menu` reuses `craft_menu` layout. Barter only (no currency) — exchange rates modulated by `Faction.relations[faction_id] / 100`.

**Negotiation**: reuse `ctx.ask("llm")` channel (same pattern as wish-memory). LLM failure falls back to deterministic lines + fixed relation delta. No new engine concept.

### 4.6 Map Expansion & Region Layout

**World layout** (256×256 total):

| Region | Coords | Size | Initial state | Unlock condition |
|---|---|---|---|---|
| home | (0,0)-(28,12) | 28×12 | active | starting |
| wild | (28,0)-(60,30) | 32×30 | active | starting (extends current wild) |
| mountain | (0,12)-(30,40) | 30×28 | dormant | Tech: Exploration T1 |
| swamp | (28,12)-(60,40) | 32×28 | dormant | Party has explorer-role companion |
| desert | (60,0)-(120,60) | 60×60 | dormant | Faction caravan relation ≥ neutral AND Tech: Industry T3 |

**Activation flow**:
1. Player walks to region boundary → rule emits `region-approach{id}`.
2. Rule checks unlock condition.
3. If pass: rule sets `Region.state = active` → engine emits `region-thaw{id}`.
4. Engine invokes all declared `catch_up` (first discovery: no-op since `discovered == 0`; sets `discovered = 1`).
5. Region content generator runs (using E3 substream `random_stream("region:<id>")`) — spawns tiles, POIs, nodes per region spec.

**Region content specs** (data, fed to generator):
- mountain: dense ore nodes, mountain-peak POI (ancient ruins → TechPoint reward).
- swamp: rich fiber + water, dangerous flora (combat triggers).
- desert: caravan POI, sandstorm weather, tomb POI (high-tier tech).

**Camera extension**: `Camera` schema gains optional `world_bounds` field (game-level data). Engine's motion integration reads it to clamp player position to discovered region bounds. (Engine behavior change — small, fits under E5 plumbing.)

### 4.7 Pacing Rebalance

**Constants**:
- `DAY_SEC`: 60 → 90.
- Stage thresholds (from single-condition to compound):
  - 起步 (day 1-3): no requirement.
  - 立足 (end of spring): survival T1 researched AND struct ≥ 5.
  - 成形 (end of summer): pop ≥ 3 AND agriculture T1 researched.
  - 成群 (end of year 1): pop ≥ 5 AND any faction relation ≥ neutral.
  - 兴旺 (end of year 2): all tech tree branches T2+ AND monument built AND any faction allied.
- Sandbox unlocked after 兴旺: all systems keep running, no ending.

**Gate adjustment**:
- `gates.must_emit = "settlement-founded"` still fires at 兴旺 stage.
- Estimated real-time for gate recording: 60-90 min (can compress to 30-45 min via acceleration mode in `record_clear.py`).

## 5. README Upgrade

**Goal**: README showcases the sandbox, not just the engine.

**Sections to add/upgrade**:
1. **New hero GIF**: 60-second sandbox playthrough showing season change, combat, tech panel, region transition. Replace `glow.gif` as the primary demo (glow.gif moves to Features section).
2. **"Frontier" featured section** (new, between Quick Start and Agent API): 3-paragraph game description + screenshot grid (seasons, combat, tech tree, diplomacy, regions) + "play in browser" CTA (placeholder until WASM).
3. **Engine capabilities matrix** (new): table mapping each frontier feature to the engine capability it exercises (e.g., "Region dormant/active → catch_up on thaw" with link to source).
4. **Updated Status**: mention sandbox game completion, 12+ systems, ~2-hour playthrough.
5. **Roadmap update**: mark "Cookbook" as partially addressed (frontier IS the cookbook now); keep WASM playground as top priority.

**Out of scope**: full docs site restructure, localization beyond existing bilingual README.

## 6. Testing Strategy

### Engine tests (new)
- Region dormant/active: query filtering, render skip, hash coverage.
- Catch_up: declared vs not, invocation order, determinism across freeze/thaw cycles.
- RNG substreams: same seed → same sequence regardless of call timing; snapshot/restore preserves stream position.
- View culling: render time scales with visible entities, not total.
- Replay: recording with region-thaw events replays hash-identical.

### Game tests (new)
- Seasons: 48-day cycle completes, multipliers apply correctly.
- Weather: state transitions, season-weighted probabilities.
- Combat: enemy spawn on night-fall, damage application, structure downgrade, player respawn.
- Tech tree: each node researchable, recipes unlock correctly, TechPoint economy balanced.
- Companions: all 6 roles spawn, contribution types differ, collective wish completes.
- Factions: relation changes, tier transitions, tier-gated unlocks.
- Regions: each region thaws correctly on unlock, content generation deterministic across runs.

### Gate
- `vitric gate games/frontier` passes with new `settlement-founded` timing.
- `qa/clear.json` re-recorded (estimated 30-45 min with acceleration).

## 7. Implementation Order (high-level — detailed plan via writing-plans)

1. **Engine E1-E5** (foundation; blocks all game systems except seasons).
2. **Seasons/Weather** (independent of regions; can develop in parallel with E1-E5 if needed).
3. **Map expansion** (depends on E1, E3).
4. **Tech tree** (depends on nothing engine-level; data + UI).
5. **Companions expansion** (depends on nothing engine-level; extends existing).
6. **Combat** (depends on tech tree for weapon unlocks).
7. **Trading/Diplomacy** (depends on companions expansion for trader role; depends on map expansion for desert region).
8. **Pacing rebalance** (after all systems in, tune numbers).
9. **README upgrade** (after game is feature-complete; screenshots need final game).

## 8. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Engine E1-E5 scope larger than expected | Implement E1+E5 first (smallest); E2+E3 next; E4 last (perf optimization, can defer if needed) |
| Region generation determinism bugs | Test early: same world_seed, different thaw timing, assert identical content |
| Combat balance wrong (too hard/easy) | Pacing rebalance step (§4.7) includes combat tuning pass; playtest swarm |
| LLM dependency in diplomacy makes tests flaky | All LLM paths have deterministic fallback (existing wish-memory pattern) |
| README screenshots need final game | README is last in implementation order |
| Scope creep | 6 systems + 5 engine caps is the ceiling; no "would be nice" additions |

## 9. Open Questions (to resolve in plan, not now)

- Exact TechPoint economy numbers (cost per tier, earn rates).
- Exact faction relation delta per action.
- Final enemy stat tuning.

These are intentionally deferred — they're tuning constants, not architectural decisions. The plan can pin them and implementation can adjust during playtest.

## 10. Success Criteria

- `vitric gate games/frontier` passes with new gate timing.
- All 5 engine capabilities have integration tests passing.
- All 6 game systems have at least one end-to-end playtest assertion.
- README showcases the sandbox with real screenshots/GIF.
- A 30+ minute playthrough is mechanically fun (subjective; verified by user playtest).
