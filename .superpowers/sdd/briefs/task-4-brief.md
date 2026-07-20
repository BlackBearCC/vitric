# Task 4: E4 — View-frustum culling

**Files:**
- Modify: `crates/vitric-render/src/lib.rs`
- Test: `crates/vitric-cli/tests/region.rs`

**Interfaces:**
- Produces: `render_world` skips entities outside camera viewport (in addition to dormant skip from Task 1).

## Step 1: Write failing perf test for culling

```rust
#[test]
fn render_time_scales_with_visible_not_total() {
    let mut sim = TestSim::with_scene("games/frontier/scenes/main.json");
    for i in 0..1000 {
        let e = sim.spawn();
        sim.add_component(e, "Position", &format!(r#"{{"x":{0},"y":{1}}}"#, 1000 + i, 1000));
        sim.add_component(e, "Sprite", r#"{"w":1,"h":1,"image":"rock.png"}"#);
    }

    let start = std::time::Instant::now();
    sim.render_frame();
    let elapsed_with_culling = start.elapsed();

    let mut sim2 = TestSim::with_scene("games/frontier/scenes/main.json");
    let start2 = std::time::Instant::now();
    sim2.render_frame();
    let elapsed_baseline = start2.elapsed();

    assert!(elapsed_with_culling < elapsed_baseline * 3,
        "culling should keep render time bounded");
}
```

## Step 2: Run test to verify it fails

Run: `cargo test -p vitric-cli --test region render_time_scales_with_visible_not_total`
Expected: FAIL.

## Step 3: Implement view-frustum culling in render_world

In `crates/vitric-render/src/lib.rs`:

```rust
pub fn render_world(world: &World, camera: &Camera, frame: &mut Frame) {
    let viewport = camera.viewport_bounds();
    let margin = 4.0;

    for &id in world.query(&["Position", "Sprite"]).iter() {
        if !is_renderable(world, id) { continue; } // Dormant skip

        let pos = world.get_component(id, "Position").unwrap();
        let x = pos["x"].as_f64().unwrap();
        let y = pos["y"].as_f64().unwrap();
        let sprite = world.get_component(id, "Sprite").unwrap();
        let w = sprite["w"].as_f64().unwrap_or(1.0);
        let h = sprite["h"].as_f64().unwrap_or(1.0);

        if x + w < viewport.0 - margin { continue; }
        if x > viewport.2 + margin { continue; }
        if y + h < viewport.1 - margin { continue; }
        if y > viewport.3 + margin { continue; }

        // ... existing render logic
    }
}

impl Camera {
    pub fn viewport_bounds(&self) -> (f64, f64, f64, f64) {
        let half_w = self.view_w / 2.0 / self.scale;
        let half_h = self.view_h / 2.0 / self.scale;
        (self.x - half_w, self.y - half_h, self.x + half_w, self.y + half_h)
    }
}
```

## Step 4: Run culling test to verify it passes

Run: `cargo test -p vitric-cli --test region render_time_scales_with_visible_not_total`
Expected: PASS.

## Step 5: Apply same culling to wgpu GPU mirror

Mirror the AABB filter in GPU draw call queuing.

## Step 6: Run full test suite + frontier gate

Run: `cargo test --workspace && cargo run --release -- gate games/frontier`
Expected: all pass.

## Step 7: Commit

```bash
git add crates/vitric-render/src/lib.rs crates/vitric-cli/tests/region.rs
git commit -m "feat(render): E4 view-frustum culling

render_world skips entities outside camera viewport (with margin for
shadow casters). Applies to both CPU rasterizer and wgpu GPU mirror.
Performance scales with visible entities, not total."
git push origin main
```
