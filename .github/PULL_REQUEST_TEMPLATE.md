## What

<!-- One paragraph: what does this PR do and why? Link the issue it closes. -->

## How

<!-- The approach, in a few bullets. Note any alternatives you rejected. -->

## Determinism

<!-- Vitric's core contract: same seed + same inputs = same state hash.
     If this PR touches the simulation, confirm that:
     - all state is JSON-serializable
     - all iteration order is deterministic (BTreeMap / sorted)
     - no wall-clock or undeclared randomness enters the sim
     Otherwise write "N/A". -->

## Checklist

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] Tests added/updated for the changed behavior
- [ ] User-facing docs updated in **both** English and 中文 (if applicable)
- [ ] `CHANGELOG.md` entry added under `[Unreleased]` (if user-visible)
- [ ] New errors carry a path, a stable code, and a fix hint (see `docs/errors.md`)
