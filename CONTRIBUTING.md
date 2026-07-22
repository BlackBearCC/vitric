# Contributing to Vitric

Thanks for your interest in contributing. Vitric is a deterministic, glass-box 2D
game engine built for AI agents — contributions that keep the engine observable,
operable, and verifiable are the most valuable kind.

By participating, you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to contribute

- **Bug reports and feature requests** — open an issue using the templates. A
  minimal game project that reproduces the problem (or one of the `examples/`
  projects) is the fastest path to a fix.
- **Pull requests** — see the [PR process](#pull-request-process) below.
- **Documentation** — user-facing docs are bilingual; see
  [documentation conventions](#documentation-conventions).

## Development setup

Prerequisites:

- A stable Rust toolchain (edition 2021; see `rust-version` in the workspace
  `Cargo.toml` for the minimum supported version).
- Linux only: ALSA development headers for audio (`sudo apt-get install
  libasound2-dev` on Debian/Ubuntu).
- `esbuild` on your `PATH` (or pointed to by the `ESBUILD_BIN` environment
  variable) if you work with TypeScript game scripts; CI installs it via
  `sudo npm install -g esbuild`.
- Node.js 20+, for the MCP server under `mcp/`.

Build and test:

```bash
# Build the whole workspace (CLI binary: target/debug/vitric)
cargo build --workspace

# Run the full test suite (what CI runs)
cargo test --workspace

# Lint — CI enforces zero warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Useful smoke checks against the sample games:

```bash
cargo build --release
BIN=./target/release/vitric
$BIN check examples/coin-run                          # validate a project
$BIN playtest examples/coin-run --sessions 16         # agent-swarm playtest
$BIN gate examples/spire                              # replay a clearing recording bit-exactly
```

MCP server smoke test (mirrors the `mcp` CI job):

```bash
cargo build -p vitric-cli
cd mcp
npm install --no-fund --no-audit
node ../scripts/mcp-smoke.mjs
```

Windows cross-build (mirrors the `windows-build` CI job; requires `mingw-w64`):

```bash
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu -p vitric-cli
```

## Code style

- Format with `cargo fmt` (default settings) and keep `cargo clippy --workspace
  --all-targets -- -D warnings` clean — CI rejects warnings.
- Code comments and identifiers are written in English.
- Errors are part of the API: every user-facing error carries a precise path, a
  stable code (see `docs/errors.md`), and a fix hint. Report all problems at
  once rather than failing on the first one. No silent fallbacks.

### The determinism rules

The engine's core contract is bit-exact determinism: same seed + same inputs =
same state hash on every frame. Any change to engine systems must preserve it:

- All simulation state is JSON-serializable; no private state hidden in Rust types.
- All iteration order is deterministic (`BTreeMap`, or explicitly sorted).
- No wall-clock time or undeclared randomness in the simulation — use the seeded
  sim RNG (scripts use `ctx.random()`; `Math.random` / `Date.now` are banned in
  the sandbox).
- Control-plane commands apply at frame boundaries only.
- Script systems must declare their component reads/writes; undeclared writes
  are rejected.
- When an `Anim` component is present, the engine owns writes to `Sprite.image`.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <summary>
```

- `<type>`: `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `build`, `ci`,
  `chore`.
- `<scope>` (optional): the crate or area, e.g. `sim`, `render`, `rules`,
  `cli`, `mcp`.
- Summary in imperative mood, lower case, no trailing period
  (e.g. `fix(sim): reject duplicate entity ids on spawn`).

Breaking changes get a `!` after the type/scope and a `BREAKING CHANGE:` footer.

## Pull request process

1. Open an issue first for anything beyond a small fix, so the design can be
   discussed before you invest in it.
2. Keep PRs focused — one concern per PR.
3. Before submitting, make sure the CI gates pass locally:
   `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
   warnings`.
4. Add or update tests for behavior changes; update docs (both languages, see
   below) and add a `CHANGELOG.md` entry under `[Unreleased]` when the change is
   user-visible.
5. Fill in the PR template; a maintainer will review. We may ask for changes —
   that's normal and not a judgment on you.

## Documentation conventions

- **User-facing documentation is bilingual**: English is the source of truth,
  with a Simplified Chinese counterpart kept in sync. Pairs follow the existing
  naming scheme: `README.md` / `README.zh-CN.md`, `docs/agent-guide.en.md` /
  `docs/agent-guide.md`. Update both sides in the same PR.
- **Code comments are English only.**
- `llms.txt`, `team/`, and `.claude/skills/` are the agent-facing on-ramps to
  this repository. `.claude/skills/vitric/` is a Claude Code skill that teaches
  AI coding agents how to build, run, and debug Vitric games; keep it in sync
  when the agent-facing workflow (CLI commands, control-plane methods, project
  layout) changes.

## AI-assisted contributions

Vitric is built by humans and AI agents working together; agent-authored PRs are
welcome. The same bar applies to everyone: tests and clippy pass, determinism
rules hold, and the PR description explains *why* the change is correct.
