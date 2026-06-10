# Vitric

English | [中文](README.zh-CN.md)

**The glass-box game engine for AI agents.**

![coin-run demo](docs/media/coin-run.gif)

*↑ Every frame of this demo was rendered by the engine itself (CPU rasterizer, no GPU), captured frame-by-frame by an AI driving the game through the control plane — which is exactly what this engine is for.*

Existing engines were designed for a human sitting in front of an editor; to an AI they are black boxes. Vitric is designed around an **agent API**: every piece of engine state is visible, operable, and verifiable, so an AI can autonomously **run the game → observe pixels and state → assert → modify → repeat** without a human in the loop.

## Try it now

```bash
cargo build --release

# Validate the sample project (errors come with paths, stable codes, and fix hints — all at once)
./target/release/vitric check examples/coin-run

# Run it (headless + AI control plane)
./target/release/vitric run examples/coin-run --port 6173
```

In another terminal, beat the game the way an agent would:

```bash
rpc() { curl -s -X POST http://127.0.0.1:6173/rpc -d "$1"; echo; }
rpc '{"method":"sim/pause"}'
rpc '{"method":"input/inject","params":{"action":"right"}}'
rpc '{"method":"sim/step","params":{"ticks":60}}'                  # deterministic frame-by-frame stepping
rpc '{"method":"world/get","params":{"entity":"@player"}}'         # score is already 3
rpc '{"method":"render/describe"}'                                 # semantic view: what is on screen, where, who overlaps whom
rpc '{"method":"render/screenshot","params":{"path":"shot.png"}}'  # headless screenshot, no GPU required
rpc '{"method":"events/recent"}'                                   # the full causal chain: collision → coin-collected → game-won
```

Full method reference: [docs/agent-guide.en.md](docs/agent-guide.en.md) ([中文](docs/agent-guide.md)).

## What makes it AI-native

- **Everything is data.** Scenes, entities, rules, and every frame of the world are strongly-schema'd JSON — validated on write, queryable at runtime, round-trippable. Saves are snapshots. There is no state hiding inside editor binaries.
- **Determinism + replay.** Fixed timestep, seeded RNG (one stream shared across Rust and JS), input recording. `vitric replay` verifies checkpoint hashes; any bug can be replayed exactly to the frame before it broke. AI debugging goes from *guessing* to *watching the replay*.
- **Rules first, scripts with a seatbelt.** ~80% of gameplay is declarative `when X then Y` rules (deliberately not Turing-complete; cascade loops are a hard error). The rest goes into JS/TS systems that must declare which components they read and write — undeclared writes are rejected, so the engine always knows the blast radius of every piece of logic.
- **Errors written for LLMs.** Every error carries a precise path, a stable code, and a fix hint — reported all at once, not one at a time.
- **Headless is first-class.** Screenshots and semantic scene descriptions are built into the engine — no GPU, no window, no display session needed. In CI, in a container, anywhere: the agent sees the actual picture, byte-for-byte deterministic (screenshots can be asserted on).
- **One animation owner.** Declarative clips + an `Anim` component; the engine has exclusive write access to `Sprite.image`. The "my animation got interrupted by some other system" class of bugs cannot exist.

## Architecture

```
crates/
  vitric-ecs       deterministic, introspectable ECS (components = JSON, ordered iteration, snapshots/hashing)
  vitric-data      declarative data layer (schema validation, scene instantiation, project loading)
  vitric-rules     when-X-then-Y rule engine (triggers/conditions/actions/cascade protection)
  vitric-script    QuickJS scripting (enforced read/write declarations, deterministic RNG, hot reload, TS via esbuild)
  vitric-sim       fixed-timestep simulation (PCG32, record/replay verification, built-in motion + collision)
  vitric-control   AI control plane (HTTP JSON-RPC: query/mutate/inject input/time control/asserts/screenshots)
  vitric-render    CPU rasterizer (world→PNG headless; sprite images with alpha; semantic describe)
  vitric-cli       vitric check / run / replay  (+ window presentation, inspector, audio)
examples/coin-run  sample game: rules/scripts/animation/audio, fully covered by e2e tests
examples/cave-gen  sample game: recipe-generated levels — change one number, get a whole new level
```

Design doc & decision record: [docs/AI原生游戏引擎-设计稿.md](docs/AI原生游戏引擎-设计稿.md) · Plan: [docs/plan.md](docs/plan.md) · Error catalog: [docs/errors.md](docs/errors.md)

## MCP

`mcp/` ships an official MCP server (12 tools): any MCP client (Claude Code / Cursor / Codex…) can validate, launch, observe, drive, and assert on a Vitric game out of the box.

```json
{ "mcpServers": { "vitric": { "command": "node", "args": ["<repo>/mcp/index.js"], "env": { "VITRIC_BIN": "<repo>/target/release/vitric" } } } }
```

The repo also ships a Claude Code skill (`.claude/skills/vitric/`) and an [llms.txt](llms.txt).

## Status

The core loop is real and tested (90+ tests, including an e2e where an agent beats the game over HTTP and a recording replays hash-identically): deterministic replay, semantic observation, hot reload, sprite assets with validation, declarative animation, recipe-generated levels, window + inspector (click/drag writes back to the data layer; selection is visible to both human and AI), audio, TypeScript scripts, MCP server, CI + binary releases.

In progress: GPU (wgpu) renderer, runtime LLM module, more built-in systems.

## License

MIT
