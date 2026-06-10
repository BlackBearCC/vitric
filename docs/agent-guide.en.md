# Vitric Agent Guide

A one-page manual for AI agents (and humans): how to autonomously run, observe, test, and modify a Vitric game.

## Four commands

```bash
vitric check <project-dir>                 # validate everything (schema/scenes/rules/scripts/assets); errors carry path + code + fix hint
vitric run <project-dir> [--port 6173] [--window] [--speed X] [--ticks N] [--record out.json]
vitric replay <project-dir> <recording>    # replay a recording, verifying determinism at every checkpoint
vitric assets <project-dir> [--colors N] [--height H] [--palette-lock]  # harmonize all project PNGs onto one shared palette (AI-generated art → one coherent look), see docs/art-pipeline.md
```

The first stdout line of `run` is a JSON banner containing the control-plane URL (plus audio and LLM status).

## Control plane (HTTP JSON-RPC)

`POST http://127.0.0.1:6173/rpc` with body `{"method": "...", "params": {...}}`.
Response: `{"ok": true, "result": ...}` or `{"ok": false, "error": "message with fix hint"}`.

### Observe

| Method | Params | Notes |
|---|---|---|
| `ping` | — | tick / paused / speed |
| `world/entities` | `components?: []` | list entities, optionally filtered by components |
| `world/get` | `entity` | all components of one entity. Entity refs: `"@name"` or handle `"e3v1"` |
| `events/recent` | `since?: tick` | recent events (input/collision plus everything rules & scripts emit) |
| `render/describe` | `width? height?` | **semantic view (primary channel)**: visible entities with screen region words / world & pixel coords / color / image, visual overlap pairs, off-screen entities with direction & distance, plus a text summary. More precise than reading pixels |
| `render/screenshot` | `width? height? path? inline?` | headless PNG (fallback verification / pixel-level asserts), no GPU needed |
| `inspect/selection` | — | what the human clicked in the window (highlighted entity), full components |
| `inspect/select` | `entity` (null clears) | point the other way: highlight an entity for the human |
| `sim/hash` | — | world state hash (compare two runs with one number) |
| `perf/stats` | — | entity count / events per tick / decoded asset memory / budget config |

### Act

| Method | Params |
|---|---|
| `input/inject` | `action`, `phase: pressed/released` |
| `world/set` | `entity`, `path` (e.g. `"Health.hp"`), `value` — schema-validated, out-of-range rejected |
| `world/spawn` | `components`, `name?` |
| `world/despawn` | `entity` |

### Control time

| Method | Params |
|---|---|
| `sim/pause` / `sim/resume` | — |
| `sim/step` | `ticks?` (paused only; response includes fresh assertion failures) |
| `sim/speed` | `multiplier` (no cap — headless can sprint) |
| `sim/snapshot` / `sim/restore` | — / `snapshot` (time travel: save any moment, jump back) |
| `project/reload` | — (**hot reload**: after editing rules/scripts/assets on disk; milliseconds, world state untouched; failure keeps the old logic. Schema/scene changes need a restart) |
| `sim/quit` | — |

### Test

| Method | Params |
|---|---|
| `assert/add` | `id`, `if: [["@player.Health.hp", ">=", 0], ...]` — checked every tick, violations reported automatically (debounced) |
| `assert/remove` / `assert/list` / `assert/failures` | — |

Budget overruns (manifest `budgets.max_entities` / `max_events_per_tick`) show up in `assert/failures` with `kind: "budget"`.

## The typical loop

```bash
vitric check my-game                          # 1. validate after every data edit
vitric run my-game --port 6173 &              # 2. launch
curl -s :6173/rpc -d '{"method":"sim/pause"}'
curl -s :6173/rpc -d '{"method":"assert/add","params":{"id":"hp","if":[["@player.Health.hp",">",0]]}}'
curl -s :6173/rpc -d '{"method":"input/inject","params":{"action":"right"}}'
curl -s :6173/rpc -d '{"method":"sim/step","params":{"ticks":60}}'  # 3. deterministic stepping
curl -s :6173/rpc -d '{"method":"render/describe"}'                 # 4. see, semantically
curl -s :6173/rpc -d '{"method":"world/get","params":{"entity":"@player"}}'
```

Reproducing a bug: `vitric run my-game --ticks 600 --record bug.json`, then
`vitric replay my-game bug.json` replays it frame-exact, and divergence is pinpointed to a checkpoint window.

## Determinism boundaries

What the engine guarantees, and where the guarantee ends:

- **Recordings capture exactly two external channels: the input stream and external replies (LLM).** While recording, `world/set` / `world/spawn` / `world/despawn` / `project/reload` / `sim/restore` are explicitly rejected (out-of-band mutations don't enter the recording, so it would silently become unreplayable), and inspector dragging is disabled. To affect the world during a recording, use `input/inject` — inputs are recorded. LLM replies enter through the engine's inject_reply channel, are recorded too, and are re-injected at the original tick on replay (see "Runtime LLM").
- **Scripts must be stateless.** Cross-tick state belongs in components. Anything stashed in `globalThis` or closures is invisible to snapshots and wiped by hot reload. `Math.random` / `Date.now` / `new Date()` throw and point you to `ctx.random()` / `ctx.tick`; explicit-argument `new Date(0)` is pure computation and allowed.
- **Snapshots are complete.** `sim/snapshot` includes the world, tick, RNG state, pending inputs, and the logic layer's carried-over events; restore-then-continue is bit-identical to the original trajectory (locked by test).
- **The guarantee is per platform, per binary.** Transcendental functions like `Math.sin` depend on the system math library; last-bit results may differ across Linux ↔ Windows. Sharing recordings or comparing hashes across platforms is outside the guarantee.

## The data language

- `vitric.json` manifest: name / schema / entry / scenes / rules / scripts / animations / budgets / seed
- `schema.json`: component fields (number/int/bool/text/vec2/entity/enum/list + default/required/min/max)
- Scenes: entity arrays; missing fields auto-filled from defaults
- Rules (the front door for gameplay): `{"id", "on", "if": [[lhs,op,rhs]...], "do": [actions...]}`
  - triggers: `"tick"` (with `each: [components]` per entity) / `{"event":"collision","between":["Player","Coin"]}` / `{"event":"input","filter":{...}}`
  - actions: `set/add/spawn/despawn/emit/call`
  - paths: `self.Comp.field` / `other.…` / `@entityName.…` / `event.field`
- Scripts (for the logic rules can't express; JS or TS — `.ts` is transpiled via esbuild, needs `esbuild` on PATH or `ESBUILD_BIN`):
  - `vitric.system("name", {query: [...], writes: [...]}, (entities, ctx) => {...})` — writing undeclared components is an error
  - `vitric.fn("name", (args, ctx) => {...})` — callable from rule `call` actions
  - `ctx.random()` (deterministic; `Math.random`/`Date.now` throw on purpose) / `ctx.tick` / `ctx.emit` / `ctx.spawn` / `ctx.despawn`

## Animation

Manifest: `"animations": "animations.json"`; clips: `{"clips": {"walk": {"frames": ["w0.png","w1.png"], "fps": 6, "loop": true}}}`.
Entities carry an `Anim` component (schema must define `clip/prev/t/done`). **The engine owns `Sprite.image` exclusively** — the only way to change animation is setting `Anim.clip` (a rule `set` works); switching restarts the clip; non-looping clips emit `anim-finished` and hold the last frame. All state lives in the component, so snapshots and replays are safe.

## Audio

Convention event: `{"emit": "play-sound", "data": {"sound": "coin.wav"}}` plays a file from the project `sounds/` dir (wav/ogg/mp3/flac). Audio is a pure output side effect — replays are unaffected. With no audio device (containers/CI) the banner says `audio: disabled` and everything else works. `vitric check` validates literal sound references.

## Runtime LLM

Game logic can ask an LLM for content at runtime (NPC dialogue, generated descriptions) **without breaking deterministic replay**.

**Config** is env-only (keys never live in project data): `VITRIC_LLM_URL` (an OpenAI-compatible chat/completions endpoint, e.g. `https://api.openai.com/v1/chat/completions`), `VITRIC_LLM_KEY`, `VITRIC_LLM_MODEL`. With all three set the startup banner shows `llm: ok (model …)`; with any missing it shows `llm: disabled: 未配置 VITRIC_LLM_URL/KEY/MODEL` — and asks then receive an **immediate, explicit** `llm-error` reply instead of silently going nowhere.

**Convention events**:
- Ask: rules/scripts emit `{"emit": "llm-ask", "data": {"id": "npc-1", "prompt": "..."}}`. `id` is a correlation key chosen by game logic; it comes back verbatim on the reply.
- Reply: the engine injects `llm-reply {id, text}`; any failure (unconfigured / network / malformed response) injects `llm-error {id, message}`. The arrival tick depends on network latency — react to the event, don't assume a fixed delay.

**The determinism story**: HTTP happens on one background worker thread (requests are queued and executed serially; the sim loop never waits on the network). Replies enter the sim via `Sim::inject_reply` — a recorded channel on par with key inputs: the reply content is written into the recording (`Recording.replies`) together with the tick that consumed it, and pending replies are part of snapshots. So `vitric replay` of a recording with LLM content never touches the network: llm-ask events have no listener, and every reply is re-injected from the recording, reproducing the run bit-identically offline.

Minimal NPC dialogue wiring (use `filter: {"id": ...}` to route the reply back to the asker):

```json
{"rules": [
  {"id": "npc-greet", "on": {"event": "input", "filter": {"action": "e", "phase": "pressed"}},
   "do": [{"emit": "llm-ask", "data": {"id": "npc-1", "prompt": "You are the blacksmith of Glass Town; say one line to a passing player"}}]},
  {"id": "npc-say", "on": {"event": "llm-reply", "filter": {"id": "npc-1"}},
   "do": [{"set": "@npc.Text.content", "to": "event.text"}]},
  {"id": "npc-fail", "on": {"event": "llm-error"},
   "do": [{"set": "@npc.Text.content", "to": "event.message"}]}
]}
```

## Built-in events

`start` (tick 0 — the standard hook for init / level generation), `input`, `collision`, `anim-finished`.

## Engine component conventions

Built-in systems recognize: `Position{x,y}` + `Velocity{x,y}` → integrated motion each tick;
`Position` + `Collider{w,h}` → AABB collision emits `collision` events;
`Position` + `Sprite{w,h,color,image,rot}` → rendering; `Camera{x,y,scale}` → view.
`Sprite.rot` is optional (degrees): the sprite rotates around its own Position, counter-clockwise positive in world space (which is also counter-clockwise as seen on screen); default 0 = no rotation. On-screen `Text` never rotates, and picking hits the rotated shape, not the original AABB.
Game-feel components (Camera `follow`/`lerp`, `Shake`, `Particle`) are covered in the "Game feel" section below.

## Platformer physics

- `Body{gravity, grounded}` (with Velocity+Collider): each tick `Velocity.y += gravity * DT` (world y is up, so gravity is negative, e.g. -30). `grounded` is engine-maintained — true while standing on a Solid top face; it's the standard jump condition.
- `Solid{}` (with Position+Collider): blocking geometry (ground / walls / platforms). Body entities clip to its edges and zero the blocked axis. Resolution is axis-separated with no sweep — keep per-tick displacement below obstacle thickness.
- A jump is just a rule: `on input(space) if [["@hero.Body.grounded","==",true]] do set @hero.Velocity.y = 14`. See `examples/jump` — a playable platformer in pure rules, zero scripts.

## Game feel

Convention components like Body/Solid: the engine recognizes the names, you define the fields in your schema; all state lives in components, so snapshots and replays are safe. All three systems run after motion/physics and before collision detection.

- **Camera follow**: two optional `Camera` fields — `follow` (entity name to track, empty string = off) and `lerp` (0..1, fraction to close per tick, 1 = hard lock). Each tick, after motion, the engine moves Camera.x/y toward the target's Position — the camera sees this tick's final position, no one-frame lag. A `follow` naming a missing entity is an explicit error (never silently skipped).
- **Screen shake**: put `Shake{amplitude, decay}` on the camera entity. While amplitude > 0, rendering adds a deterministic pseudo-random view offset (a pure function of (tick, amplitude) — it never touches the sim's RNG stream, so shaking has zero effect on the gameplay trajectory); each tick `amplitude *= decay` (snapped to 0 below 0.001). The offset affects the picture only (window/screenshots); `render/describe` and picking read the unshaken camera. No new action needed — a rule `set` triggers it. Shake on collision:
  ```json
  {"id": "hit-shake", "on": {"event": "collision", "between": ["Player", "Enemy"]},
   "do": [{"set": "@camera.Shake.amplitude", "to": 0.5}]}
  ```
- **Particles**: put `Particle{ttl}` (ticks remaining, integer) on an entity; the engine decrements it each tick and despawns the entity at 0 (despawn order = slot order, deterministic). Confetti / dust / explosions = spawn a batch of Sprite+Velocity+Particle entities and forget them — no cleanup rules needed.

## Lighting

Convention components like Body/Solid: the engine recognizes the names, you define the fields in your schema.

- **The master switch is the presence of an Ambient entity.** No entity with an `Ambient` component = the lighting pass is skipped entirely (previous behavior, zero cost); one exists (first one wins) = the lighting pipeline activates and the whole frame is lit.
- `Ambient{color}`: scene ambient base, e.g. `"#202838"` for a dark cave; `"#ffffff"` keeps unlit areas unchanged.
- `Light{radius, color, intensity}` + `Position`: a point light. radius is in world units (light fades to zero at radius); color defaults to `"#ffffff"`, intensity to 1.0. **Hard cap: 64 lights** — exceeding it is an explicit error, never a silent truncation.
- The formula (identical on the CPU screenshot path and the GPU window): `lit = min(ambient + Σ light_color·intensity·(1 - d/r)², 1.5)`, then `out = min(scene · lit, 1.0)`. The 1.5 ceiling allows slight over-brightening (a cheap bloom-ish pop).
- **Everything is lit uniformly** — sprites, text, background; screen-anchored HUD text is not exempt. Keep HUDs readable by placing a light nearby or raising the ambient.
- Lighting is deterministic: it reads only component state; identical world + tick → identical bytes. `render/screenshot` includes lighting — the agent sees what the player sees.
- With lighting active, `render/describe` adds `ambient` (color) and a `lights` array (id/name/world pos/radius/intensity/color) plus a summary line — the full lighting setup is textually observable.
- **Bloom**: put a `Bloom{threshold, strength}` component on any entity (first one wins, like Ambient) to enable the full-screen bloom post-effect — bright areas haze outward into a glow halo; combined with point lights things actually *glow*. threshold ∈ [0,1]: the part of each channel above threshold·255 feeds the bloom; strength ≥ 0: additive multiplier. Both fields are required. Formula: `bright = max(scene - threshold·255, 0)`, separable box blur (3 iterations, approximates gaussian), `out = min(scene + blurred·strength, 255)`. Blur radius = viewport height / 90, floor 2 px — the halo scales with resolution. Bloom runs after lighting; no Bloom entity = the pass is skipped entirely (zero cost, byte-identical). When active, `render/describe` adds a `bloom` field plus a summary line.

```json
{"name": "torch", "components": {"Position": {"x": 10, "y": 4},
  "Light": {"radius": 6, "color": "#ff9040", "intensity": 1.2}}}
```

## On-screen text

`Text{content, size, color}` + `Position`: built-in 8x8 bitmap font (ASCII), each glyph is size×size world units, the string is centered on Position and drawn above sprites. `render/describe` returns `texts[].content` directly — agents never OCR screenshots.
To turn numeric state into text, use the rule format template: `{"set": "@hud.Text.content", "to": {"format": "SCORE {}", "args": ["self.Score.value"]}}` (the number of `{}` slots must match args).
