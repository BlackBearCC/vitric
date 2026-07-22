# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- Script engine now enforces resource limits on AI-authored JavaScript: a 64 MiB
  QuickJS heap cap, 1 MiB stack cap, and a per-call instruction budget enforced
  via an interrupt handler. A runaway script (`while (1) {}`, memory bomb,
  unbounded recursion) now fails with a structured error (SC001/SC002/SC003)
  instead of hanging the engine and control plane.
- Script-driven panics can no longer crash the engine or leave dangling raw
  pointers: `SimPtrGuard`/`WorldPtrGuard` RAII guards + `catch_unwind` at the
  logic-step boundary convert panics into `SimError::Logic`. Note: this relies
  on `panic = "unwind"` — do not set `panic = "abort"`.

### Fixed

- Scripts that tamper with prelude internals (e.g. overriding
  `globalThis.__runSystem` or `__list`) now get structured errors
  (SC004/SC010/SC013) instead of triggering `expect()` panics.
- Runtime script errors now carry stable codes and fix hints
  (SC001–SC004, SC010–SC013), matching the validation layer's error format.

### Added

- `examples_check` integration test: every project under `examples/` must pass
  `vitric check` in CI — the README's promise is now enforced by a test.
- Community infrastructure: CONTRIBUTING, CODE_OF_CONDUCT, SECURITY policy,
  issue/PR templates, and crates.io-ready package metadata.

### Fixed (docs)

- README: corrected the GPU presentation attribution (lives in `vitric-cli`,
  not `vitric-render`), completed the subcommand list (incl. `balance`), noted
  the external `esbuild` dependency, and replaced the unverifiable hardware
  claim with the `vitric gpu-probe` self-measurement path.
- `llms.txt` / team playbook: fixed stale `team/roles/` references to the
  actual `team/skills/` layout.

## [0.2.0] - 2026-06-10

### Added

- wgpu GPU presentation path (`--renderer gpu|cpu`), mirroring the CPU
  rasterizer; window mode with F11 borderless fullscreen.
- 2D dynamic lighting (ambient + point/spot/directional), sprite rotation, and
  a bloom post-effect — CPU truth source, GPU mirrors the same formulas.
- `vitric assets` harmonization pipeline: project-wide shared-palette
  quantization for AI-generated art.
- TypeScript systems via esbuild; TTF vector font with proportional
  anti-aliased text incl. CJK.
- Audio: play-sound/music events with volume, looping BGM, explicit
  no-sound-card degradation, and `check` validation of sound references.
- Platform physics (`Body` gravity, `Solid` blocking, engine-maintained
  `grounded`), on-screen text with `format` templates, and game-feel
  primitives (camera follow, screen shake, particle lifetime).
- Runtime LLM module: replies travel through the recorded input channel, so
  replays never touch the network.
- New examples: `jump`, `book`, expanded `glow` and `coin-run`.

### Fixed

- Cross-boundary float fidelity: QuickJS's non-shortest round-trip dtoa is
  bypassed with IEEE-754 bit strings, keeping replay bit-exact.
- Write-detection compares by numeric semantics; recording mode rejects
  side-channel mutations; snapshots include undigested input and pending
  events.

## [0.1.0] - 2026-06-10

Initial public release: a deterministic, glass-box 2D game engine built for AI
agents. Pre-1.0 — the API may change between releases.

### Added

**Deterministic core**

- Fixed-timestep (60 Hz) simulation with a seeded PCG32 RNG shared across Rust
  and JavaScript, ordered iteration, and JSON-serializable world state.
- Input recording and bit-exact replay with checkpoint state hashes.
- Exact snapshot/restore, giving save/load, replay, and lookahead search from
  one primitive.

**Authoring**

- Declarative data layer: `vitric.json` manifest, strongly-schema'd components,
  and scene instantiation with validate-on-write and structured errors
  (path + stable code + fix hint).
- Declarative `when X then Y` rule engine (deliberately not Turing-complete)
  with cascade-loop protection.
- QuickJS scripting with enforced read/write declarations, deterministic RNG
  and clock substitution, TypeScript via esbuild, and hot reload.
- Declarative animation clips with a single animation owner (the engine holds
  exclusive write access to `Sprite.image`).

**Rendering**

- Deterministic CPU rasterizer (the truth source) with a wgpu GPU mirror.
- 2D dynamic lighting (ambient + point/spot/directional), zero-config
  normal-mapped relief, shadow casting, and a bloom post-effect.
- Semantic on-screen text (bitmap font or TTF with CJK support) and headless,
  byte-for-byte deterministic screenshots.

**Agent API**

- HTTP JSON-RPC control plane: query/mutate any world state, inject input,
  pause/step/speed up time, snapshot, assert, poll events.
- `render/describe`: a semantic view of the frame (entities, ego-centric
  spatial relations, ASCII level map, input actions, frame-to-frame diff).

**Verification & playtesting**

- `vitric check` — project validation reporting all errors at once.
- `vitric replay` — bit-exact recording verification.
- `vitric gate` — delivery certificates: bit-exact replay + required terminal
  event + per-tick assertions.
- `vitric playtest` — a deterministic agent swarm reporting clear rate,
  reachability, soft-locks, dead content, pacing, number breakage, and dominant
  strategies, with lookahead search and certificate-seeded exploration;
  self-contained HTML reports; thresholds declarable as a delivery gate.
- Optional LLM playtesters for clarity/continuity feedback.

**Asset & delivery tooling**

- `vitric assets` — palette harmonization, procedural/image-to-image normal
  maps, and a frame-animation pipeline (dedupe, trim, atlas, palette, BC7
  compression).
- `vitric bundle` — release packaging.
- Prebuilt binaries for Linux and Windows.

**AI integration**

- Official MCP server (`mcp/`): validate, launch, observe, drive, and assert on
  a Vitric game from any MCP client.
- `team/` multi-agent protocol with role work orders, territory enforcement
  (`vitric turf`), and a read-only collaboration board (`vitric team`).

**Examples & docs**

- Runnable sample games under `examples/` (`coin-run`, `jump`, `cave-gen`,
  `spire`, `glow`, `ui-menu`, `ui-gallery`, `intro`, and more), each covered by
  tests.
- Bilingual agent guide, error catalog, and design records under `docs/`.

[Unreleased]: https://github.com/BlackBearCC/vitric/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/BlackBearCC/vitric/releases/tag/v0.1.0
