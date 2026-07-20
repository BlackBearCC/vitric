# Task 4 Review — View-frustum culling (E4)

## Verdict: APPROVED

## Spec compliance

- Step 1 (perf test): ✅ — Replaced the brief's flaky timing-based `render_time_scales_with_visible_not_total` with three deterministic correctness tests (`offscreen_entities_not_rendered`, `onscreen_entities_rendered`, `culling_preserves_byte_identical_output_for_onscreen_entities`). The brief explicitly allows this replacement. Implementer is honest that the off-screen test passes even without culling (existing clamp logic already prevents off-screen writes) — it's a regression guard, not a TDD-failing test.
- Step 2 (run-fail): ✅ — N/A in the strict sense (the replacement tests are regression guards, not strict TDD-failing-tests). The implementer's report acknowledges this honestly in Section 5.1 and Concerns #3. The `onscreen_entities_rendered` test would fail if the culling math is wrong (e.g., a sign flip), which is the meaningful correctness driver.
- Step 3 (implement culling): ✅ — Culling implemented in `render_with` at `crates/vitric-render/src/lib.rs:310-359`. Computes world-space viewport bounds from `(cam_x, cam_y, scale)` + framebuffer dimensions, then per-entity AABB check (with rotation-aware `ext_x`/`ext_y` for `rot != 0`) before the pixel loop. Skipped entities are exactly those whose rotated AABB is entirely outside the viewport.
- Step 4 (run-pass): ✅ — `cargo test -p vitric-cli --test region` passes 10/10 (per report).
- Step 5 (GPU mirror): SKIP (optional) — Brief marks Step 5 as OPTIONAL. Implementer added a documented comment in `crates/vitric-cli/src/gpu.rs:1887-1894` explaining the intentional skip and pointing future maintainers to the CPU logic. Approved deviation.
- Step 6 (full suite + gate): ✅ — `cargo test --workspace -- --skip typescript` passes (the 2 typescript failures are pre-existing env issues, see ⚠️ below). `cargo run --release -- gate games/frontier` passes with `final_hash: 0xab58ec29d99275df` matching expected byte-for-byte.
- Step 7 (commit): ✅ — Commit `58ab058cd354950faf0211ac48c4ba12c0d0df94` on `origin/main`, 5 files +308 lines. Commit message is accurate and detailed (notes the GPU mirror skip rather than copying the brief's "applies to both" template).

## Findings

### Critical
(none)

### Important
(none)

### Minor

1. **Test header comment undercounts tests** — `crates/vitric-cli/tests/region.rs:297` says "Replaces the brief's flaky timing-based perf test with **two** deterministic correctness tests" but there are **three** tests below (`offscreen_entities_not_rendered`, `onscreen_entities_rendered`, `culling_preserves_byte_identical_output_for_onscreen_entities`). The third test is documented in the report but not in the in-file header comment. Fix: change "two" → "three" and add a sentence about the byte-identical guard.

2. **`offscreen_entities_not_rendered` is a weak TDD driver** — `crates/vitric-cli/tests/region.rs:311-328`. The implementer is explicitly honest about this (report §5.1, §6.3): the test passes even without culling because the existing clamp logic at `lib.rs:385-388` (`x0 = (cx - half_w).floor().max(0.0)`, `x1 = (cx + half_w).ceil().min(width as f64)`) already makes the pixel loop range empty for off-screen sprites. It is still a useful regression guard (would catch a future refactor that writes to the wrong buffer index from the culling branch), but does not by itself prove the culling logic is present or correct. The `onscreen_entities_rendered` and `culling_preserves_byte_identical_output_for_onscreen_entities` tests are the meaningful correctness checks. No fix required — noted for transparency.

3. **Test name `culling_preserves_byte_identical_output_for_onscreen_entities` is slightly misleading** — `crates/vitric-cli/tests/region.rs:349`. The test does NOT compare "before culling" vs "after culling" output (impossible after the change). It asserts (a) two renders of the same world are byte-identical (`assert_eq!(a, b)`) and (b) the on-screen 32×32 AABB is fully magenta. The actual "culling doesn't change pixels" lock comes from the existing `frames.rs` / `glow.rs` / `particles.rs` screenshot-hash tests + the frontier gate hash. The implementer acknowledges this in the test's own comment. No fix required — noted for transparency.

### ⚠️ Cannot verify from diff

1. **Pre-existing typescript test failures** — `crates/vitric-cli/tests/typescript.rs::typescript_system_runs_after_transpile` and `typescript_syntax_error_names_the_file` fail with "测试需要 esbuild". The implementer verified these fail on `origin/main` via `git stash` + rerun. Per the controller's instructions, this is a known pre-existing issue from prior tasks and should not block. Controller follow-up: optionally run `cd mcp && npm install` to confirm.

2. **Frontier gate hash** — The report claims `cargo run --release -- gate games/frontier` passes with `final_hash: 0xab58ec29d99275df`. Cannot be verified from the diff alone (requires running the gate). The implementer's report is detailed and the existing screenshot-hash tests in `particles.rs:99` and `glow.rs:60` (`assert_eq!(a, b, "同一 tick 两次渲染逐字节一致")`) are visible in-repo and would catch any culling-induced pixel change. Controller may re-run the gate to confirm.

3. **Leftover uncommitted files from prior tasks** — `.superpowers/sdd/progress.md`, `.superpowers/sdd/briefs/task-2-review.{diff,md}`, `.superpowers/sdd/briefs/task-3-review.{diff,md}` per the implementer's report §6.2. These were NOT included in the Task 4 commit (verified: the commit only touches 5 files). Controller should decide whether to commit them separately or roll into a review-catch-up commit.

## Notes

- **Culling math is provably conservative.** The check at `lib.rs:355-358` uses strict `<` / `>` against the world-space viewport bounds. For `rot == 0`, `ext_x = sw/2` matches the render path's `half_w = sw * scale / 2` (just in world units vs screen units). For `rot != 0`, the culling's `ext_x = half_w_world * cs.abs() + half_h_world * sn.abs()` is bit-for-bit the same formula as the rotation path's `ext_x = half_w * cs.abs() + half_h * sn.abs()` at `lib.rs:456-457` (scaled). I verified algebraically that whenever the culling skips (e.g., `px + ext_x < view_x0`), the render path's `cx + ext_x_screen < 0`, so `x1 = (cx + ext_x_screen).ceil().min(width) ≤ 0` and the pixel loop is empty. No on-screen pixel can be lost.

- **Dropped `margin = 4.0` is justified.** The brief's pseudocode used `margin = 4.0` "for shadow casters". Verified at `lib.rs:647` that `collect_occluders` queries `["Solid", "Position", "Collider"]` — a completely separate code path from the sprite loop. Shadow casters are NOT affected by the sprite cull, so no margin is needed for shadow correctness. For sprite culling, the exact AABB is the right boundary (any margin would be arbitrary over-conservatism). Approved.

- **Shaken camera is used.** Verified at `lib.rs:259` (`render_world` calls `camera_of`) and `lib.rs:3291-3302` (`camera_of` applies `Shake` offset via `shake_offset(tick, amplitude)`). So `cam_x, cam_y` passed into `render_with` IS the shaken camera. The culling at `lib.rs:318-323` uses these same `cam_x, cam_y`. Correct — the picture uses the shaken camera, so culling must too.

- **Lights/Emitters not culled — correct.** Lights are drawn via the lighting formula which already does its own per-pixel radius culling (point/spot lights scan only their own light-disc bounding box). An off-screen Light can illuminate on-screen pixels (radius extends into viewport), so culling by position would break lighting. Emitters are drawn by `draw_particles`, a separate path. Neither is touched by the sprite-loop cull. The brief does not ask for either to be culled. Approved.

- **`describe_world` not culled — correct.** `describe_world_with_assets` classifies entities as visible/offscreen using its own boundary check (per report, `dx.abs() - sw / 2.0 < half_w_units` at line 2746) and lists off-screen entities in the `offscreen` array with direction/distance for agent navigation. Culling there would lose semantic information. The text-contrast measurement re-uses `render_with`, so it benefits from the culling automatically. Approved.

- **All comments are English** (project convention). Verified across `vitric-render/src/lib.rs:310-358`, `vitric-cli/src/gpu.rs:1887-1894`, `vitric-cli/tests/region.rs:295-378`. String literals (panic messages like `"分辨率 {width}x{height} 不合法"` at `lib.rs:293`) correctly retain their original language per the project convention.

- **No YAGNI violations.** No speculative API surface added (`Camera::viewport_bounds` from the brief's pseudocode was NOT added — the implementer inlined the viewport computation in `render_with` instead, which is cleaner since `Camera` isn't a struct in this codebase; the camera is a `(f64, f64, f64)` tuple). No dead code. No unused constants (`MAGENTA` is used in all 3 tests).

- **Determinism contract lock.** The new `assert_eq!(a, b)` at `region.rs:365` is `Vec<u8>` equality — true byte-equality. The 32×32 magenta footprint check at `region.rs:378` (`assert_eq!(magenta_in_aabb, 32 * 32)`) verifies the on-screen entity is fully rendered (no partial skip). Combined with the existing screenshot-hash tests (`particles.rs:99` replay byte-identical, `glow.rs:60` same-tick byte-identical) and the frontier gate hash, the contract is well-locked.
