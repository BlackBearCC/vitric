# Vitric Effect Format — Design Record

Date: 2026-08-06
Status: Design draft, pending implementation

## Problem

Vitric's rendering pipeline has lighting, bloom, and shadows hardcoded as passes inside `vitric-render`. The roadmap calls for a "Render pipeline — post-processing chain, multi-pass, custom shader injection." This document specifies the format and architecture for that.

## Goals

1. **Authors can write custom shaders** (cartoon outline, pixelation, water distortion, color grading, CRT effect) as data, not engine code.
2. **CPU truth-source stays byte-deterministic.** The CPU rasterizer is the verification authority; a custom effect must produce identical pixels on CPU and GPU.
3. **GPU (wgpu) mirrors the CPU path.** Same effect, same output (visually on GPU, byte-identically on CPU).
4. **No engine recompilation to add a shader.** Shaders are project data, loaded at boot like scenes and rules.

## Non-goals

- A node-based visual shader editor (out of scope for v1; the format is text-first, LLM-friendly).
- Compute shaders (v1 is fragment-only; vertex is fixed quad/sprite geometry).
- 3D shaders (Vitric is 2D; the format is designed to extend to 3D later without breaking).

## The format

An **effect file** is `effects/<name>.effect.json`, declared in the manifest's `effects` list:

```json
{
  "name": "cartoon-outline",
  "passes": [
    {
      "name": "edge-detect",
      "stage": "fragment",
      "inputs": ["scene", "depth"],
      "uniforms": {
        "edge_threshold": { "type": "float", "default": 0.8 },
        "edge_color": { "type": "color", "default": "#000000" }
      },
      "source": "effects/cartoon-outline.wgsl"
    }
  ]
}
```

### Pass

A pass is one fullscreen fragment shader. The pipeline executes passes in array order; each pass reads named inputs (previous pass output, scene, depth, normals) and writes to a render target that becomes available as input to subsequent passes.

| Field | Type | Description |
|---|---|---|
| `name` | text | Pass identifier (referenced by later passes' `inputs`) |
| `stage` | `"fragment"` | v1 only supports fullscreen fragment passes |
| `inputs` | list of text | Named textures to bind: `"scene"` (the rendered world so far), `"depth"`, `"normals"`, or a previous pass `name` |
| `uniforms` | object | Declared uniform variables with type + default; settable via rules/scripts at runtime |
| `source` | text | Path to the shader source file (relative to project root) |

### Shader source language: WGSL

**Decision: WGSL, not GLSL.**

Rationale:
- Vitric's GPU path uses wgpu, which speaks WGSL natively — no transpilation step, no external dependency (unlike GLSL→SPIR-V via shaderc).
- WGSL is the WebGPU standard; it's what the WASM playground will use.
- For the CPU truth-source path, the effect format declares a **reference implementation** alongside the GPU shader — see "CPU mirror contract" below.

### Uniform types

| Type | WGSL | CPU (Rust) | Rule/script settable |
|---|---|---|---|
| `float` | `f32` | `f64` | ✅ `set @effect.CartoonOutline.edge_threshold = 0.5` |
| `int` | `i32` | `i64` | ✅ |
| `color` | `vec4<f32>` | `[f32; 4]` | ✅ `set @effect.CartoonOutline.edge_color = "#ff0000"` |
| `vec2` | `vec2<f32>` | `[f64; 2]` | ✅ |
| `bool` | `u32` (0/1) | `bool` | ✅ |

### Effect component

An entity with an `Effect` component activates a post-processing effect:

```json
{
  "name": "cartoon-fx",
  "components": {
    "Effect": { "effect": "cartoon-outline", "enabled": true }
  }
}
```

The engine processes `Effect` entities in scene order (deterministic). `enabled = false` skips the effect (output bytes identical to no effect — backward compatibility).

Multiple `Effect` entities chain: pass 1 output → pass 2 input → ... The first effect's first pass receives `"scene"` as input.

## CPU mirror contract

The core invariant: **same world state → identical pixels on CPU and GPU.** This is what makes screenshots assertable and replays verifiable.

### Approach: dual-mode effect

Each effect declares *one of two* CPU strategies:

1. **`"cpu": "reference"`** — the effect ships a Rust reference implementation (a function `fn pass(width, height, &input_pixels, &uniforms) -> Vec<u8>`). The GPU runs WGSL; the CPU runs the reference function. Both must produce byte-identical output (enforced by a test macro). This is for effects where WGSL can't run on CPU (which is all of them — CPU has no shader compiler).

2. **`"cpu": "passthrough"`** — the CPU path skips this pass entirely (the effect is GPU-only, cosmetic). **Screenshots taken via `render/screenshot` (CPU path) will NOT include this effect.** The effect only appears in windowed GPU mode. Use this for effects that are purely visual and don't affect gameplay assertions. The engine warns at boot if a `passthrough` effect is active and the project has screenshot-based assertions.

### Why not compile WGSL on CPU?

There is no production-grade WGSL→CPU compiler. Options considered:
- **naga** (wgpu's WGSL frontend) can parse WGSL but doesn't execute it — it lowers to IR for backend translation.
- **tint** (Dawn's WGSL compiler) similarly targets backend APIs, not CPU execution.
- A custom WGSL interpreter would be a maintenance nightmare and performance liability.

The dual-mode approach is honest about the constraint: either ship a reference impl (for effects that matter to determinism) or accept GPU-only (for cosmetic effects). This matches the existing pattern where CPU screenshots are the truth source and GPU is a visual mirror.

## Built-in effects (ship with the engine)

| Effect | CPU strategy | Description |
|---|---|---|
| `color-grade` | reference | LUT-based color grading (1D or 3D LUT); deterministic on both paths |
| `vignette` | reference | Radial darkening at edges; parameters: `intensity`, `radius`, `color` |
| `pixelate` | reference | Downsample + nearest-neighbor upscale; parameter: `block_size` |
| `outline` | reference | Sobel edge detection on luminance; parameters: `threshold`, `color` |
| `bloom-gpu` | passthrough | Enhanced bloom (GPU-only, extends the built-in `Bloom` component for windowed mode) |

Custom effects live in the project's `effects/` directory. Built-in effects are referenced by name without a path prefix.

## Post-processing pipeline order

```
[world render (sprites, text, lighting, particles)]
  → [built-in bloom] (if Bloom entity exists)
  → [effect passes in entity order] (if Effect entities exist)
  → [final output]
```

Built-in bloom runs first (it's part of the world render), then custom effects chain. This keeps the existing `Bloom` component byte-identical (backward compatibility).

## Manifest declaration

```json
{
  "effects": ["effects/cartoon-outline.effect.json"],
  "schema": "schema.json",
  ...
}
```

The `Effect` component must be declared in the project's schema (like all components). The engine validates effect files at boot: source files exist, uniform types are legal, input names are valid.

## `vitric check` validation

- Effect file structure: `name` + non-empty `passes` array
- Each pass: `stage` = `"fragment"`, `source` file exists, `inputs` are valid names (`scene`/`depth`/`normals` or a prior pass name)
- Uniform types in the allowed set
- `cpu` strategy is `"reference"` or `"passthrough"`
- If `"reference"`: the named Rust function is registered (engine-internal registry; custom reference impls require engine extension — documented limitation for v1)
- If `"passthrough"`: warn if the project has `render/screenshot`-based assertions in gates

## Interaction with determinism

- Effect uniforms are component state → they enter the world hash → recordings/snapshots include them → replay reproduces the exact effect parameters.
- `enabled` toggles are component state → same treatment.
- `passthrough` effects do NOT enter the world hash (they're GPU-only, no sim state).
- `reference` effects' output is a pure function of `(input pixels, uniforms)` → deterministic.
- Screen shake offset is applied before effect passes (it's part of the world render), so effects see the shaken frame — consistent with current behavior.

## Limitations (v1)

1. **Custom CPU reference impls require engine code.** A project can't ship a new Rust function without forking the engine. v2 could explore a restricted pixel-shader DSL that runs on CPU (like a limited expression language over pixel coordinates and neighbor samples). For v1, projects that need determinism-verified custom shaders must contribute to the engine's effect registry.
2. **No vertex shaders.** All passes are fullscreen quads. Sprite-level vertex manipulation (wave distortion per-sprite) is not supported; use the existing `Sprite.rot` / `Anim` system.
3. **No multi-render-target.** Each pass writes one RGBA buffer. Depth/normals are engine-provided inputs, not writable by passes.
4. **No conditional branching between passes.** The pass chain is static (declared in the effect file, executed in order). Runtime pass toggling = `enabled` on the `Effect` entity, not per-pass.

## Future: restricted CPU pixel DSL

To remove the "custom reference impl needs engine code" limitation, a future version could define a restricted pixel-shader DSL that the CPU can interpret:

```
// effects/my-effect.pixel — Vitric Pixel DSL
input scene: rgba8
input depth: f32
uniform threshold: float = 0.5

output {
  let lum = 0.299 * scene.r + 0.587 * scene.g + 0.114 * scene.b;
  let edge = abs(lum - sample_left.lum) + abs(lum - sample_right.lum);
  if edge > threshold {
    out = vec4(0, 0, 0, 1);
  } else {
    out = scene;
  }
}
```

This DSL would be:
- **Not Turing-complete** (no loops, no recursion — like the rule engine).
- **Pure functions of (pixel position, input samples, uniforms)** — no global state.
- **Interpretable on CPU** (a simple tree-walking evaluator over a fixed AST).
- **Translatable to WGSL** for the GPU path.

This is deferred to v2; the dual-mode approach covers v1 needs.
