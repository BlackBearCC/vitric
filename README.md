# Vitric

[![CI](https://github.com/BlackBearCC/vitric/actions/workflows/ci.yml/badge.svg)](https://github.com/BlackBearCC/vitric/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/BlackBearCC/vitric)](https://github.com/BlackBearCC/vitric/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

English | [中文](README.zh-CN.md)

**A deterministic, glass-box 2D game engine built for AI agents.**

![glow demo](docs/media/glow.gif)

*↑ AI-generated pixel art, dynamic 2D lighting, particles, camera follow — every frame captured headlessly by an AI driving the game through the control plane.*

Existing engines were designed for a human in front of an editor; to an AI they are black boxes. Vitric is built around an **agent API**: every piece of engine state is visible, operable, and verifiable, so an AI can autonomously **run the game → observe pixels and state → assert → modify → repeat** without a human in the loop. Because the simulation is bit-exact deterministic, the engine can also *prove* things about a game — that a recording clears it, that a swarm of agents can't soft-lock it.

## Contents

- [Quick start](#quick-start) · [The agent API](#the-agent-api) · [Automated playtesting](#automated-playtesting)
- [Features](#features) · [Architecture](#architecture) · [Examples](#examples) · [Docs](#docs)
- [MCP & multi-agent](#mcp--multi-agent) · [Status](#status) · [Contributing](#contributing) · [License](#license)

## Quick start

```bash
cargo build --release
BIN=./target/release/vitric

# Validate a project — errors come with paths, stable codes, and fix hints, all at once
$BIN check examples/coin-run

# Run it (headless + AI control plane on port 6173)
$BIN run examples/coin-run --port 6173

# Auto-playtest: a swarm of deterministic agents plays the game and reports what's broken
$BIN playtest examples/coin-run --sessions 16 --html report.html

# Delivery gate: replay a winning recording bit-exactly and require the win event
$BIN gate examples/spire
```

Prebuilt binaries for Linux and Windows are on the [releases page](https://github.com/BlackBearCC/vitric/releases).

## The agent API

The engine runs headless and exposes an HTTP JSON-RPC control plane. An agent drives the game the same way a player would, but through data:

```bash
rpc() { curl -s -X POST http://127.0.0.1:6173/rpc -d "$1"; echo; }
rpc '{"method":"sim/pause"}'
rpc '{"method":"input/inject","params":{"action":"right"}}'
rpc '{"method":"sim/step","params":{"ticks":60}}'                  # deterministic frame-by-frame stepping
rpc '{"method":"world/get","params":{"entity":"@player"}}'         # read any entity's state
rpc '{"method":"render/describe"}'                                 # semantic view: what's on screen, where, who overlaps whom
rpc '{"method":"render/screenshot","params":{"path":"shot.png"}}'  # headless screenshot, no GPU needed
rpc '{"method":"events/recent"}'                                   # the causal chain: collision → coin-collected → game-won
```

`render/describe` is built for a model to read: alongside the entity list it gives ego-centric spatial relations to the camera's focal entity (direction / distance / line-of-sight), an ASCII map of the level, the declared input actions, and a frame-to-frame diff of what changed since the last call.

Full method reference: [docs/agent-guide.en.md](docs/agent-guide.en.md) ([中文](docs/agent-guide.md)).

## Automated playtesting

`vitric playtest` runs a swarm of deterministic agents through a game and aggregates a structured report — the kind of mechanical QA that's slow and incomplete by hand. It clears the *floor* (mechanical correctness and balance), not the *ceiling* (fun, feel, art — those still need a human).

What the swarm reports:

- **Clear rate & reachability** — can it be beaten at all? Which declared endings does no run reach?
- **Soft-locks** — input orderings that strand a run in an unwinnable state, clustered and replayable.
- **Dead content** — items, abilities, or actions no run ever uses.
- **Pacing** — where runs stall; difficulty spikes.
- **Number breakage** — economy runaway / collapse, overflow.
- **Dominant strategy** — one action or build that makes every other choice meaningless.
- **Clarity / continuity** — optional LLM playtesters flag "I couldn't tell what to do" or "this dialogue contradicts an earlier scene."

Two properties of the engine make this work and set it apart:

- **Lookahead search.** Because the sim is deterministic with exact snapshot/restore, an agent can *speculatively* try an action, roll forward a few ticks, score the result, and roll back — actually playing skill-based games instead of flailing randomly. (`--strategy lookahead`.)
- **Certificates as seeds.** A `vitric gate` clearing recording is both a proof the game is beatable *and* a seed for the swarm: it perturbs the known solution (reorder, branch, drop a step) to find what breaks it — the tractable way to test puzzle and narrative games.

Playtest thresholds — no soft-locks, a minimum clear rate, no unreachable endings — can be declared as a delivery gate (`gates.playtest` in `vitric.json`), so "the swarm couldn't break it" becomes part of the contract. Reports render to a self-contained HTML page. See [docs/design-agent-playtest.md](docs/design-agent-playtest.md).

## Features

**Determinism & verification**
- Fixed timestep, seeded PCG32 RNG (one stream shared across Rust and JS), input recording.
- `vitric replay` re-runs a recording and verifies checkpoint hashes bit-exactly — any bug replays to the exact frame it broke.
- `vitric gate` turns a winning recording into an unforgeable delivery certificate: bit-exact replay + a required terminal event + assertions evaluated every tick. "Done" is decided by the machine, not claimed by the agent.

**Everything is data**
- Scenes, entities, rules, and every frame of the world are strongly-schema'd JSON — validated on write, queryable at runtime, round-trippable. Saves are full snapshots; there is no state hiding inside an editor binary.
- Snapshot/restore is exact, which is what makes save/load, replay, and lookahead search all fall out of one primitive.

**Authoring**
- **Rules first.** ~80% of gameplay is declarative `when X then Y` rules (deliberately not Turing-complete; cascade loops are a hard error).
- **Scripts with a seatbelt.** The rest is JS/TS systems that must declare the components they read and write — undeclared writes are rejected, so the engine always knows the blast radius of every system. TypeScript compiles via esbuild; hot reload supported.
- **One animation owner.** Declarative clips + an `Anim` component; the engine has exclusive write access to `Sprite.image`, so "my animation got interrupted by another system" cannot happen.

**Rendering** (CPU rasterizer is the deterministic truth source; wgpu GPU path mirrors it)
- 2D dynamic lighting (ambient + point/spot/directional) with zero-config normal-mapped relief (`hero.png` + `hero_n.png`), 2D shadow casting, and a bloom post-effect.
- On-screen text described semantically (no OCR); built-in bitmap font, or a TTF for proportional anti-aliased vector text including CJK.
- Headless screenshots are byte-for-byte deterministic and can be asserted on — no GPU, window, or display session required.
- Frame-animation pipeline (`vitric assets --frames`): dedupe, trim, atlas, palette, and BC7 texture compression — runtime VRAM for the texture atlas drops ~4× on BC-capable GPUs (verified on an RTX 4090).

**AI-generated art**
- `vitric assets` harmonizes a project's PNGs onto one shared palette (deterministic median-cut), and generates normal maps procedurally or via image-to-image.

**Errors for LLMs**
- Every error carries a precise path, a stable code, and a fix hint — all reported at once, not one at a time.

## Architecture

```
crates/
  vitric-ecs       deterministic, introspectable ECS (components = JSON, ordered iteration, snapshots/hashing)
  vitric-data      declarative data layer (schema validation, scene instantiation, project loading)
  vitric-rules     when-X-then-Y rule engine (triggers/conditions/actions/cascade protection)
  vitric-script    QuickJS scripting (enforced read/write declarations, deterministic RNG, hot reload, TS via esbuild)
  vitric-sim       fixed-timestep simulation (PCG32, record/replay, snapshot/restore, motion + collision)
  vitric-render    CPU rasterizer + wgpu GPU mirror (world→PNG headless, lighting/shadows/bloom, semantic describe)
  vitric-control   AI control plane (HTTP JSON-RPC: query/mutate/inject input/time control/asserts/screenshots)
  vitric-playtest  agent swarm playtesting (scene view, strategies incl. lookahead, seed exploration, report)
  vitric-cli       vitric check / run / replay / gate / playtest / assets / bundle (+ window, inspector, audio)
```

## Examples

`examples/` holds runnable sample games, each fully covered by tests: `coin-run` (rules + scripts + animation + audio), `jump` (a platformer in pure rules, zero scripts), `cave-gen` (recipe-generated levels), `spire`, `glow` (dynamic lighting), `ui-menu`/`ui-gallery`, `intro` (the timeline/sequence system), and more.

## Docs

- [Agent API reference](docs/agent-guide.en.md) ([中文](docs/agent-guide.md)) · [Error catalog](docs/errors.md) · [Art pipeline](docs/art-pipeline.md)
- Design records: [agent playtesting](docs/design-agent-playtest.md), [UI](docs/design-ui.md), [frame animation](docs/design-frame-animation.md), [tween/sequence](docs/design-tween-sequence.md)
- [llms.txt](llms.txt) for agents reading the repo.

## MCP & multi-agent

`mcp/` ships an official MCP server: any MCP client (Claude Code / Cursor / Codex …) can validate, launch, observe, drive, and assert on a Vitric game out of the box.

```json
{ "mcpServers": { "vitric": { "command": "node", "args": ["<repo>/mcp/index.js"], "env": { "VITRIC_BIN": "<repo>/target/release/vitric" } } } }
```

The engine also carries a multi-agent team harness: role work tickets (`team/`), a coordination blackboard (`vitric team`), and turf enforcement (`vitric turf`), so an agent platform can run a multi-agent game team off the engine itself. See the [team playbook](docs/team-playbook.md).

## Status

Pre-1.0 and under active development; the API may change. The core is real and tested (650+ tests in CI, including an end-to-end run where an agent beats a game over HTTP and the recording replays hash-identically). Determinism, replay, gates, headless rendering, lighting/shadows/bloom, rules + TypeScript, save/load, scene flow, GPU presentation, audio, asset harmonization, frame animation, the playtest swarm, and the MCP server are all in place. Binaries ship for Linux and Windows.

## Contributing

Issues and pull requests are welcome. `cargo test --workspace` and `cargo clippy --workspace --all-targets` should pass clean (CI enforces both). New engine systems should keep the determinism rules: all state JSON-serializable, all iteration ordered, no wall-clock or undeclared randomness in the simulation.

## License

[MIT](LICENSE) © 2026 BlackBearCC
