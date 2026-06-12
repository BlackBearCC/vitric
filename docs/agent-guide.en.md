# Vitric Agent Guide

A one-page manual for AI agents (and humans): how to autonomously run, observe, test, and modify a Vitric game.

## Eight commands

```bash
vitric check <project-dir>                 # validate everything (schema/scenes/rules/scripts/assets); errors carry path + code + fix hint
vitric run <project-dir> [--port 6173] [--window] [--speed X] [--ticks N] [--record out.json]
vitric replay <project-dir> <recording>    # replay a recording, verifying determinism at every checkpoint
vitric gate <project-dir>                  # delivery gate: check + playthrough replays + assertions; exit 0 only if ALL pass (see "Delivery gates")
vitric bundle <project-dir> [--out file] [--engine exe]  # ship a self-contained single file; gate must PASS first — no certificate, no release (see "Shipping a bundle")
vitric assets <project-dir> [--colors N] [--height H] [--palette-lock]  # harmonize all project PNGs onto one shared palette (AI-generated art → one coherent look), see docs/art-pipeline.md
vitric team <project-dir>                  # multi-agent team blackboard: per-role deliverable health + contract/gate status + blocking hints (read-only, always exits 0), see team/README.md
vitric turf <project-dir> --role <name> <changed-files...>  # turf enforcement: exit 1 naming every changed file outside the role's turf
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
| `render/describe` | `width? height?` | **semantic view (primary channel)**: visible entities with screen region words / world & pixel coords / color / image, visual overlap pairs, off-screen entities with direction & distance, plus a text summary. More precise than reading pixels. On-screen text gets a legibility check: if its WCAG-style contrast ratio against the backdrop falls below 2.5, the response adds a `warnings[]` entry (kind=`low-contrast-text`, with entity/content/ratio/hint) plus a ⚠ summary line — the engine reads pixels so you don't have to (see "On-screen text") |
| `render/screenshot` | `width? height? path? inline?` | headless PNG (fallback verification / pixel-level asserts), no GPU needed |
| `inspect/selection` | — | what the human clicked in the window (highlighted entity), full components |
| `inspect/select` | `entity` (null clears) | point the other way: highlight an entity for the human |
| `sim/hash` | — | world state hash (compare two runs with one number) |
| `perf/stats` | — | entity count / events per tick / decoded asset memory / budget config |

### Act

| Method | Params |
|---|---|
| `input/inject` | `action`, `phase: pressed/released` |
| `input/click` | `x`, `y` (**world coordinates**), `button?: left/right` (default left) — the headless "mouse": pick resolution shares the window click-pick path, injects a `mouse` / `mouse-alt` event, and the response carries the pick result directly (see "Mouse input") |
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

## Delivery gates (vitric gate)

"Done" is not something an agent gets to claim — the engine verifies delivery mechanically. The core idea: **a deterministic recording is an unforgeable proof-of-completion.** To earn the certificate, a recording must both (1) replay bit-exactly checkpoint-by-checkpoint from a cold boot of the project data (forge a single frame and the state hash diverges at the next checkpoint) and (2) actually emit the terminal event (default `game-won`) during that replay. Neither alone suffices: a clean replay might be an idle run, and an event name alone might be fabricated.

Declare gates in `vitric.json`:

```json
"gates": {
  "playthroughs": [{"recording": "qa/clear.json", "must_emit": "game-won"}],
  "assertions": "qa/asserts.json",
  "check": true,
  "max_ticks": 100000
}
```

- `playthroughs` (must be non-empty): playthrough gates. Each recording is replayed and verified independently; `must_emit` defaults to `"game-won"`.
- `assertions` (optional): an assertion file `[{"id": "...", "if": [[lhs, op, rhs], ...]}, ...]` (same condition syntax as the control plane's `assert/add`). Evaluated **every tick** of every replay; a violation at any point fails the gate, reporting the id and the first violating tick.
- `check` (default true): full project validation first; any error = FAIL.
- `max_ticks` (optional): recording length cap, so a million-tick AFK run can't be padded into a "win".

Workflow: gate never produces recordings itself — QA/the director plays a winning run (by hand or driven via control-plane RPC) with `vitric run my-game --record qa/clear.json`, then `vitric gate my-game` verifies it. The report is one JSON for humans and machines alike (`{"pass": bool, "gates": [{name, status, detail}...]}`) on stdout; **exit 0 only if every gate passes**. A manifest without gates (or with empty playthroughs) exits 1 — no gates, no certificate; an empty gate that passes would be a loophole.

## Shipping a bundle (vitric bundle)

`vitric bundle my-game` packs project + engine into **one distributable file** (a standalone game). The gate comes first: bundle runs `vitric gate` and refuses to ship unless it PASSes — no certificate, no release (on refusal the gate report is printed to stdout as-is). On PASS, the project files (including the qa/ playthrough recordings — the certificate ships with the game; top-level `saves/`, `assets_original/` and hidden files are excluded) are packed into a zlib-compressed archive appended to a copy of the engine binary (footer = 8-byte magic + blob length; format documented in `crates/vitric-cli/src/bundle.rs`). On success it prints one JSON line `{out, bytes, project, files}`; the default file name is `<project>-<platform>[.exe]`, override with `--out`.

A bundled executable (engine with an embedded project in its tail, self-detected at startup) behaves as:

- **No arguments** (player double-click): extracts to `temp/vitric-<hash>/` and runs windowed (CPU renderer — works everywhere). The extraction dir is unique per bundle hash; player saves/ live there and persist per bundle.
- **`run-embedded [run options]`**: runs the embedded project with options passed through — `--ticks 5` for a headless smoke run, `--renderer gpu` for players who want GPU.
- **Any other arguments**: the normal CLI — a bundle is still the full engine.

Cross-platform: to ship a windows build from linux, point `--engine` at a cross-compiled windows engine (`cargo build --release --target x86_64-pc-windows-gnu`) — the footer format is platform-independent; the bundle targets whatever engine it's appended to. A bundle cannot itself be used as `--engine` (no nesting).

## Determinism boundaries

What the engine guarantees, and where the guarantee ends:

- **Recordings capture exactly two external channels: the input stream and external replies (LLM).** While recording, `world/set` / `world/spawn` / `world/despawn` / `project/reload` / `sim/restore` are explicitly rejected (out-of-band mutations don't enter the recording, so it would silently become unreplayable), and inspector dragging is disabled. To affect the world during a recording, use `input/inject` — inputs are recorded. LLM replies enter through the engine's inject_reply channel, are recorded too, and are re-injected at the original tick on replay (see "Runtime LLM"); mouse clicks (window clicks / `input/click`) ride the same reply channel and stay available while recording (see "Mouse input").
- **Scripts must be stateless.** Cross-tick state belongs in components. Anything stashed in `globalThis` or closures is invisible to snapshots and wiped by hot reload. `Math.random` / `Date.now` / `new Date()` throw and point you to `ctx.random()` / `ctx.tick`; explicit-argument `new Date(0)` is pure computation and allowed.
- **Snapshots are complete.** `sim/snapshot` includes the world, tick, RNG state, pending inputs, and the logic layer's carried-over events; restore-then-continue is bit-identical to the original trajectory (locked by test).
- **The guarantee is per platform, per binary.** Transcendental functions like `Math.sin` depend on the system math library; last-bit results may differ across Linux ↔ Windows. Sharing recordings or comparing hashes across platforms is outside the guarantee.

## The data language

- `vitric.json` manifest: name / schema / entry / scenes / rules / scripts / animations / budgets / font / seed
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

Convention event: `{"emit": "play-sound", "data": {"sound": "coin.wav", "volume": 0.6}}` plays a file from the project `sounds/` dir (wav/ogg/mp3/flac). `volume` is optional, 0..=1, default 1.0; out-of-range or non-number values produce a structured `audio_error` line on stderr (no crash, no silent clamping).

Background music: `{"emit": "play-music", "data": {"sound": "bgm.ogg", "volume": 0.4}}` plays looped. There is a single music slot — a new play-music replaces the current track (old one stops first), and music keeps playing across ticks. `{"emit": "stop-music", "data": {}}` stops it (a no-op if nothing is playing).

Audio is a pure output side effect — replays are unaffected. With no audio device (containers/CI) the banner says `audio: disabled` and everything else works. `vitric check` validates literal play-sound / play-music references.

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

## Scenes & flow

A complete game is more than one scene: menu → level → next level → ending. Switching is a convention event, executed entirely inside the deterministic pipeline:

- Rules/scripts emit `{"emit": "load-scene", "data": {"scene": "scenes/level2.json"}}`. `scene` must be one of the manifest's `scenes` — anything else is an explicit error listing the available scenes (add new scene files to vitric.json first).
- The switch runs at the tail of that tick's logic: **every** entity of the old world is properly despawned (stale handles die cleanly, names are released), and the new scene is instantiated from data preloaded at boot. Since the triggering event is itself deterministic, a replay reproduces the switch at the same tick and checkpoint hashes keep matching across it; snapshots/restore work across switches too. Editing scene files on disk mid-run does not affect this process (scenes load at boot, like the schema — restart to pick up changes).
- **Carry-over = the `Persist` marker component.** Entities with `Persist` (define a field-less component in your schema) survive the switch: all their components are moved into the new world and respawned under the same name — player, score, inventory continuity with zero new systems. Two hard constraints: survivors must be named (an anonymous one can't be referenced by rules — explicit error), and the name must not collide with an entity in the target scene (explicit error).
- **The per-scene init hook is `scene-loaded {scene}`** (delivered to rules on the tick after the switch); `start` fires once at tick 0 of the whole run and is **not** re-fired by switches.
- Emitting more than one load-scene in a single tick is an explicit error (there is no right answer for where to go — make your switch rules mutually exclusive).
- `vitric check` instantiates **every** scene in the manifest — bad references in non-entry scenes (missing images, undefined animation clips) fail check instead of exploding at switch time.

```json
{"id": "level-clear", "on": {"event": "collision", "between": ["Player", "Exit"]},
 "do": [{"emit": "load-scene", "data": {"scene": "scenes/level2.json"}}]}
```

## Built-in events

`start` (tick 0 — the standard hook for init / level generation; not re-fired by scene switches), `input`, `mouse` / `mouse-alt` (mouse clicks, see "Mouse input"), `collision`, `anim-finished`, `scene-loaded` (the tick after each scene switch — the per-scene init hook, see "Scenes & flow").

## Mouse input

Clicks are **game input** on the same footing as key presses — menus and card games consume them with plain rules:

- **Events**: left button = `mouse`, right button = `mouse-alt`, both with data `{x, y, entity}` — x/y are **world coordinates** (window clicks are converted through the un-shaken camera: clicks target the world itself, screen shake is visual decoration only), and `entity` is the name of the picked entity (handle text for unnamed entities, null on empty space). Hit-testing is the same as inspector picking / `render/describe` (including `Sprite.rot` rotated shapes). Rules read as usual: trigger `{"event": "mouse"}`, conditions/values via `event.x` / `event.y` / `event.entity`, filtering via `"filter": {"entity": "card"}`.
- **Two entrances, one pipe**: a human clicking in the window and an agent calling `input/click {x, y, button?}` (world coordinates directly) go through the exact same pick-and-inject path — humans and AI are peer players. The RPC response carries the pick result, so a headless agent doesn't need a describe round-trip to know what it hit.
- **Recording semantics**: clicks ride the reply channel (the same recorded channel as LLM replies): they enter the recording together with their tick and pick result (`Recording.replies`), are re-injected at the original tick on replay, and pending clicks are included in snapshots — click-driven sessions replay bit-identically offline, and **clicking stays allowed while recording**. A playthrough recording for a mouse game can be produced entirely via `input/click` over RPC and passes the gate as usual.
- **One click, two meanings**: in the window, a left click injects the `mouse` event *and* still drives inspector selection/dragging (teal outline, `inspect/selection`). The inspector exists only in windowed mode — a game that doesn't want that layer can simply ignore the selection; right clicks never touch the inspector.
- **Boundary**: the mouse *position* by itself is **not** an event — reporting the cursor every tick would bloat recordings; hover effects are out of scope for v1. The engine emits on press only (no release event — a click means one event).

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
- `Ambient{color, shadows}`: scene ambient base, e.g. `"#202838"` for a dark cave; `"#ffffff"` keeps unlit areas unchanged. `shadows` is an optional bool (default false), see "2D shadow casting" below.
- `Light{radius, color, intensity, kind, angle, dir}`: a light source with three `kind`s (defaults to `"point"`; an unknown value is an explicit error listing the valid kinds). **Hard cap: 64 lights total across all kinds** — exceeding it is an explicit error, never a silent truncation.
  - `"point"` (needs `Position`): radius is in world units (light fades to zero at radius); color defaults to `"#ffffff"`, intensity to 1.0. No kind field = point light = previous behavior, byte-identical output.
  - `"spot"` (needs `Position`): all point-light fields, plus required `angle` (full cone width in degrees, 1..=360) and required `dir` (facing direction in degrees, world space, 0 = +x, counter-clockwise positive — the same angle convention as `Sprite.rot`).
  - `"directional"`: required `dir` (the direction the light *travels*, degrees, same convention) plus color/intensity. Ignores Position/radius — the sun is infinitely far away, equally bright everywhere (pixels without a normal map ignore dir; normal-mapped pixels get directionality from it, see below).
- The formula (identical on the CPU screenshot path and the GPU window): `lit = min(ambient + Σ contributions, 1.5)`, then `out = min(scene · lit, 1.0)`. Per-light contribution: point = `light_color·intensity·(1 - d/r)²` (only when d < r); spot = the point formula times an angular falloff `t²` with `t = clamp(1 - Δθ/(angle/2), 0, 1)` (1 at cone center, 0 at cone edge; Δθ is the angle between the pixel direction and dir); directional = `light_color·intensity` (uniform). The 1.5 ceiling allows slight over-brightening (a cheap bloom-ish pop).
- **Normal maps (zero-config naming pair)**: a sprite using `hero.png` is normal-mapped automatically if `hero_n.png` exists in assets/ — without the pair the output is byte-for-byte the old behavior (test-locked). RGB encodes a tangent-space normal (`n = rgb/255·2-1`, z forced outward then normalized; xy axes match screen pixel space — x right, y down); sampled with the same UV as the diffuse, and `Sprite.rot` rotates the normal with the sprite. Normal-mapped pixels multiply each light's contribution by `max(dot(N, L), 0)`: L's xy is the unit direction from the pixel toward the light ×0.8 with z fixed at 0.6 (a flat normal directly under a light still gets 60% — enabling normals never blacks out the scene); for directionals L = (−travel_dir·0.8, 0.6). Generate normal maps with `vitric assets --normals` (docs/art-pipeline.md ⑤).
- **2D shadow casting**: set `"shadows": true` on the `Ambient` component to enable (default false = the pass never runs, byte-identical output). Occluders are entities with `Solid`+`Position`+`Collider` — Solid already means "blocks" (it stops bodies), so with shadows on the same entities also block light, **zero new authoring**; hard cap 256 occluders, exceeding it is an explicit error. Per pixel per light: if the segment from the pixel to the light center crosses any occluder's collision box, that light contributes zero (hard shadows, no penumbra). **Self-shadow rule: a pixel inside an occluder is never shadowed by that occluder** — only by *other* boxes, so walls stay lit instead of turning into black slabs. Only point/spot lights cast shadows; **directional lights do not cast shadows in v1** (they stay uniform everywhere). Don't bury a light center inside a Solid — a buried light can't shine past its own wall. When active, `render/describe` adds `shadows: true` + `occluders` (count) plus a summary line. Performance: adjacent occluders whose edges line up exactly (tile floors) are merged into slabs every frame, then culled per light by radius — **output bytes are unchanged**, but flush-aligned tiles render much faster. The GPU window path additionally has a uniform budget: at most 64 merged boxes per light radius and 256 entries across all lights, exceeded = explicit error (fewer lights, smaller radius, or align tiles so they merge).
- **Everything is lit uniformly** — sprites, text, background; screen-anchored HUD text is not exempt. Keep HUDs readable by placing a light nearby or raising the ambient.
- Lighting is deterministic: it reads only component state; identical world + tick → identical bytes. `render/screenshot` includes lighting — the agent sees what the player sees.
- With lighting active, `render/describe` adds `ambient` (color) and a `lights` array (id/name/kind/world pos/radius/intensity/color; spots add angle/dir, directionals add dir and omit world pos/radius) plus a summary line — the full lighting setup is textually observable.
- **Bloom**: put a `Bloom{threshold, strength}` component on any entity (first one wins, like Ambient) to enable the full-screen bloom post-effect — bright areas haze outward into a glow halo; combined with point lights things actually *glow*. threshold ∈ [0,1]: the part of each channel above threshold·255 feeds the bloom; strength ≥ 0: additive multiplier. Both fields are required. Formula: `bright = max(scene - threshold·255, 0)`, separable box blur (3 iterations, approximates gaussian), `out = min(scene + blurred·strength, 255)`. Blur radius = viewport height / 90, floor 2 px — the halo scales with resolution. Bloom runs after lighting; no Bloom entity = the pass is skipped entirely (zero cost, byte-identical). When active, `render/describe` adds a `bloom` field plus a summary line.

```json
{"name": "torch", "components": {"Position": {"x": 10, "y": 4},
  "Light": {"radius": 6, "color": "#ff9040", "intensity": 1.2}}}
{"name": "beam", "components": {"Position": {"x": 0, "y": 8},
  "Light": {"kind": "spot", "radius": 10, "angle": 50, "dir": 270, "color": "#ffffcc"}}}
{"name": "sun", "components": {
  "Light": {"kind": "directional", "dir": 300, "color": "#fff4e0", "intensity": 0.4}}}
```

## On-screen text

`Text{content, size, color}` + `Position`: the string is centered on Position and drawn above sprites. `render/describe` returns `texts[].content` directly — agents never OCR screenshots.
To turn numeric state into text, use the rule format template: `{"set": "@hud.Text.content", "to": {"format": "SCORE {}", "args": ["self.Score.value"]}}` (the number of `{}` slots must match args).

Two rendering paths, chosen by the manifest `font` field:

- **Default (no font)**: the built-in 8x8 bitmap font (ASCII), each glyph size×size world units, monospaced, hard pixel edges — right for pixel-art games. Output bytes are bit-identical to before this feature existed (locked by tests). Non-ASCII characters render as solid placeholder blocks.
- **`"font": "fonts/myfont.ttf"` in the manifest (path relative to the project root)**: **all** Text components switch to the TTF vector font — proportional advances + kerning, size = glyph height in world units (pixel height = size × camera scale), and any glyph the font contains renders (**including Chinese/CJK, provided the font itself has CJK glyphs** — Latin fonts like DejaVu don't; missing glyphs render the font's .notdef tofu box, so use e.g. Noto Sans SC for Chinese). Vector text is coverage-anti-aliased — the one intentionally smooth element in the engine; sprites stay nearest-neighbor crisp. Use this for hand-drawn/HD styles and runtime LLM replies in Chinese (see examples/book).
- Missing/corrupt font file: `vitric check` and boot both fail with an explicit error naming the path — text never silently disappears at runtime.
- Determinism is unchanged: CPU screenshots (render/screenshot) stay byte-identical per platform/binary and remain assertable; the GPU window matches visually but not byte-exactly (the CPU path stays the source of truth).

**Legibility warnings (`warnings` in describe)**: for every on-screen text, `render/describe` internally renders the frame with that one text skipped, averages the background luminance inside the text's bounding box, and computes the WCAG-style contrast ratio `(L1+0.05)/(L2+0.05)` against `Text.color`. Below 2.5 it emits `{"kind": "low-contrast-text", "entity": ..., "content": ..., "ratio": ..., "hint": ...}` plus a ⚠ line in the summary. This catches the "renders fine, humans can't read it" class of failure (cream text on a cream card). Off-screen texts are not checked. No warnings ⇒ no `warnings` key. Known approximation: the text color is taken raw while the backdrop is sampled after lighting/bloom — the threshold leaves margin for that.

## Static texture-reference scan (check)

`vitric check` has always validated textures referenced by scenes/animations; it now also covers textures spawned **dynamically from scripts and rules**: literal `.png` references in script source (`image: "dust.png"`, `"image": "dust.png"`, single quotes too) and literal `Sprite.image` values inside rule `spawn` actions must all exist under assets/, or check fails naming the file and the texture. Honest limitation: this is a **lint over literals**, not dataflow analysis — dynamically concatenated names (`"dust_" + i + ".png"`) or indirect references are invisible to it, so a green check does not guarantee every runtime image exists. Prefer literal names so the lint can cover you; a missing image at runtime still fails loudly (no placeholder is drawn).
