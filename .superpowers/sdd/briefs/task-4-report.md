# Task 4 Report — View-frustum culling (E4)

## 1. Status

**DONE**

- CPU rasterizer skips entities whose rotated AABB is entirely outside the camera viewport.
- Determinism contract preserved: same world + same tick → byte-identical pixels (locked by an explicit byte-identical test and the existing `frames.rs` / `glow.rs` / `particles.rs` screenshot-hash tests, all still pass).
- Gate still passes with the exact expected hash `0xab58ec29d99275df`.
- Existing `examples/` (coin-run gate test) and frontier gate recording still pass byte-identical.

## 2. Commits

(tbd — filled in after `git commit` + `git push` complete; see Section 7 update.)

## 3. Test results

| Command | Result |
|---|---|
| `cargo test -p vitric-cli --test region --` | **PASS** — 10 passed, 0 failed (includes new `offscreen_entities_not_rendered`, `onscreen_entities_rendered`, `culling_preserves_byte_identical_output_for_onscreen_entities`). The `catch_up_advances_dormant_crop_on_thaw` test takes ~80s in debug (3600-tick sim, pre-existing). |
| `cargo test -p vitric-render` | **PASS** — 110 passed, 0 failed, 1 ignored. The render crate's unit tests include rotation/lighting/bloom/font/particle regression tests — all still byte-identical. |
| `cargo test --workspace -- --skip typescript` | **PASS** — every test result line shows `0 failed`. The 2 `typescript` tests are skipped because they require `esbuild` (env var `ESBUILD_BIN` or `cd mcp && npm install`); they fail on `origin/main` too (verified by `git stash` + rerun), so this is a pre-existing environment issue, NOT a regression from Task 4. |
| `cargo test -p vitric-cli --test frames --test glow --test particles --test region --test ui --test ui_interact` | **PASS** — the screenshot/render-sensitive test suites all green. |
| `cargo run --release -- check games/frontier` | **PASS** — exit code 0, schema validates. |
| `cargo run --release -- gate games/frontier` | **PASS** — `pass: true`, `playthrough:qa/clear.json` status=`pass`, `verified: true`, `final_hash: 0xab58ec29d99275df` (matches the expected hash byte-for-byte). 426 initial entities. |

## 4. Files touched

| File | Lines |
|---|---|
| `crates/vitric-render/src/lib.rs` | +39 (viewport computation at top of `render_with` + AABB cull inside the sprite loop, after the dormant check). |
| `crates/vitric-cli/src/gpu.rs` | +8 (comment documenting that GPU culling is intentionally not mirrored, pointing to the CPU logic). |
| `crates/vitric-cli/tests/region.rs` | +88 (3 new tests + a `MAGENTA` constant). |
| `.superpowers/sdd/briefs/task-4-report.md` | (this file) |

**Total: +135 lines of code+tests (excluding this report).**

## 5. Deviations from brief

1. **Replaced the brief's flaky perf test with three deterministic correctness tests** (`offscreen_entities_not_rendered`, `onscreen_entities_rendered`, `culling_preserves_byte_identical_output_for_onscreen_entities`). The brief's `render_time_scales_with_visible_not_total` was timing-based (`elapsed_with_culling < elapsed_baseline * 3`), which the task description explicitly flagged as flaky and recommended replacing. The replacement tests lock the culling contract without depending on wall-clock timing:
   - `offscreen_entities_not_rendered` — magenta sprite at (1000, 1000) must not appear anywhere in a 64×64 frame (default camera, viewport (-4..=4, -4..=4)). Regression guard: would fail if culling ever WRITES the wrong color.
   - `onscreen_entities_rendered` — magenta sprite at (0, 0) must appear in the buffer. Correctness: would fail if culling is over-aggressive (e.g., a wrong sign in the AABB check).
   - `culling_preserves_byte_identical_output_for_onscreen_entities` — same world twice → byte-identical, AND the on-screen 32×32 sprite footprint is entirely magenta. This is the determinism-contract regression guard the brief warns about: would fail if culling accidentally skips an on-screen entity.

   **Note on TDD:** these tests are regression guards rather than strict TDD-failing-tests. The existing clamp logic (`x0 = (cx - half_w).floor().max(0.0)`, `x1 = (cx + half_w).ceil().min(width as f64)`) already prevents off-screen sprites from writing pixels — so `offscreen_entities_not_rendered` passes even without my culling implementation. The test is still useful: it locks the contract so any future refactor that accidentally breaks culling (e.g., writes to the wrong buffer index from the culling branch) would be caught. The `onscreen_entities_rendered` test would fail if the culling math is wrong.

2. **GPU mirror (Step 5) skipped** — per the brief this step is OPTIONAL. The CPU path is the source of truth for screenshots/gate/assertions; the GPU path (`crates/vitric-cli/src/gpu.rs`) only drives the live interactive window display. Implementing GPU culling would be a pure perf optimization with two risks:
   - Diverging from the CPU's culling math (causing visible differences between the window and the screenshot).
   - Adding test surface that doesn't lock any new correctness contract.
   
   I added a comment in `gpu.rs` (line 1887) explicitly documenting this choice and pointing future maintainers to the CPU culling logic in `vitric_render::render_with`.

3. **No `describe_world` change** — the architectural guidance suggested considering culling in `describe_world`. Inspection of `describe_world_with_assets` (lines 2686+) shows it already classifies entities as visible/offscreen using the SAME boundary check (`dx.abs() - sw / 2.0 < half_w_units` — line 2746) and never renders off-screen entities; it lists them in the `offscreen` array with direction/distance info for agent navigation. The text-contrast measurement (lines 2898+) re-uses `render_with`, so it automatically benefits from the culling without any change to `describe_world`. Adding culling to `describe_world`'s own iteration would lose semantic information (off-screen entities would no longer be listed in `offscreen`), breaking the agent's observation channel. So no change was made there.

4. **Culling math uses rotation-aware AABB, no fixed margin** — the brief's pseudocode used `margin = 4.0` "for shadow casters". Inspection shows shadow casters are `Solid + Position + Collider` entities collected separately by `collect_occluders` (line 608), NOT through the sprite render loop — so the sprite cull doesn't affect shadow casting. For sprite culling correctness, I use the sprite's exact AABB for `rot == 0` and the rotated bounding-box extent for `rot != 0` (matching the rotation path's own `ext_x`/`ext_y` computation at line 417-418). This means culling and rendering agree bit-exactly on what is "on screen" — no over-aggressive culling of rotated sprites whose corners extend past their un-rotated AABB.

## 6. Concerns

1. **Pre-existing typescript test failures** — `crates/vitric-cli/tests/typescript.rs::typescript_system_runs_after_transpile` and `typescript_syntax_error_names_the_file` both panic with "测试需要 esbuild：仓库里跑 `cd mcp && npm install`，或设 ESBUILD_BIN". This is an environment issue (missing `esbuild` binary), NOT a regression from Task 4. Verified by `git stash`-ing my changes and re-running — the same 2 tests fail on `origin/main`. Reviewer should run `cd mcp && npm install` to confirm.

2. **Working tree has leftover uncommitted changes from prior tasks** — `.superpowers/sdd/progress.md` (Task 2 + Task 3 entries), `.superpowers/sdd/briefs/task-2-review.{diff,md}`, `.superpowers/sdd/briefs/task-3-review.{diff,md}`. These were left in the working tree by previous tasks (Task 3 was committed at c286ca5 but its progress.md and review artifacts weren't). My Task 4 commit will NOT include these — only the files I changed for Task 4. The reviewer may want to commit them separately or roll them into a "review-catch-up" commit.

3. **Culling test depth** — As noted in Section 5, my `offscreen_entities_not_rendered` test passes even without the culling implementation (the existing clamp logic already prevents off-screen sprites from writing pixels). The test is still a useful regression guard but does not strictly drive the implementation via TDD. If a stricter failing test is desired, the render API would need to expose a "cull count" counter or similar observable signal — I judged that adding API surface for testability was out of scope. The `onscreen_entities_rendered` and `culling_preserves_byte_identical_output_for_onscreen_entities` tests ARE meaningful correctness checks that would fail if the culling math is wrong.

4. **`describe_world` not culled** — intentional (see Section 5.3). Off-screen entities continue to appear in the `offscreen` array of semantic observation. This is correct behavior for the agent observation channel; culling there would lose information.

5. **`Emitter` particles not culled** — the brief doesn't ask for this. Particles are drawn by `draw_particles` (separate from the sprite loop) and can extend beyond the emitter's position; culling them would require their own AABB check. Left for a future optimization if particle counts become a bottleneck. Currently bounded by `MAX_EMITTERS` (64) × `MAX_PARTICLES_PER_EMITTER` (1024).

6. **`Light` sources not culled** — intentional. An off-screen Light entity can still illuminate on-screen pixels (its radius extends into the viewport); culling lights by their position would break lighting for scenes with lights just outside the camera. The lighting formula already does its own per-pixel radius culling (point/spot lights only scan their own light-disc bounding box). No change needed.

7. **`Shake` interaction** — the culling uses the shaken camera (cam_x, cam_y from `camera_of`, which includes Shake offset). This is correct: the picture uses the shaken camera, so culling must use the same camera to avoid skipping entities that the shake would have brought into view. `describe_world` uses the non-shaken camera (`camera_base`) — but that's its own iteration and not affected by my change.
