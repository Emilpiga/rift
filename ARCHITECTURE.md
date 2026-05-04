# Rift Engine — Architecture Plan

## Game Concept

- Action RPG rift crawler (Diablo/PoE-inspired loop)
- Timed rifts with scaling difficulty & rewards
- Item-driven build system amplifying abilities
- Low-poly / stylized art direction

## Tech Stack

- **Language:** Rust
- **Graphics API:** Vulkan (via `ash` raw bindings)
- **Windowing:** `winit`
- **Math:** `glam` (fast, game-oriented)
- **Asset Loading:** `gltf` for models, `image` for textures
- **ECS:** `hecs` (lightweight) — for game layer later
- **Build:** Cargo workspace

## Rendering Engine — Phased Plan

### Phase 1: Foundation (Current Focus)

Get a triangle on screen with proper Vulkan infrastructure.

- [ ] Vulkan instance, device, swapchain setup
- [ ] Render pass & framebuffers
- [ ] Graphics pipeline (vertex + fragment shaders)
- [ ] Command buffer recording & submission
- [ ] Synchronization (fences, semaphores)
- [ ] Window integration via winit
- [ ] Basic camera (perspective projection)

### Phase 2: Geometry & Scene

Render actual 3D content.

- [ ] Vertex buffer / Index buffer abstractions
- [ ] Mesh loading (glTF)
- [ ] Transform hierarchy (model/view/projection)
- [ ] Basic material system (albedo color + texture)
- [ ] Depth buffer
- [ ] Frustum culling

### Phase 3: Lighting & Shading

Forward rendering with stylized look.

- [ ] Directional light
- [ ] Point lights (capped count for forward)
- [ ] Stylized shading (cel/toon options, rim lighting)
- [ ] Shadow mapping (directional, single cascade)
- [ ] Ambient occlusion (SSAO or baked)

### Phase 4: Effects & Polish

Make rifts feel impactful.

- [ ] Particle system (GPU-driven)
- [ ] Post-processing pipeline (bloom, color grading, vignette)
- [ ] Outline/glow effects (for items, abilities)
- [ ] Screen-space reflections (optional)
- [ ] Skeletal animation
- [ ] Instanced rendering (enemy hordes)

### Phase 5: Performance & Scale

Handle rift density.

- [ ] GPU frustum culling (compute shader)
- [ ] Level-of-detail (LOD) system
- [ ] Occlusion culling
- [ ] Multi-threaded command buffer recording
- [ ] Memory allocator (GPU memory management via `gpu-allocator`)

## Project Structure

```
rift/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── rift-engine/        # Core rendering engine
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── vulkan/     # Vulkan backend
│   │   │   │   ├── mod.rs
│   │   │   │   ├── instance.rs
│   │   │   │   ├── device.rs
│   │   │   │   ├── swapchain.rs
│   │   │   │   ├── pipeline.rs
│   │   │   │   ├── commands.rs
│   │   │   │   └── sync.rs
│   │   │   ├── renderer/   # High-level render logic
│   │   │   │   ├── mod.rs
│   │   │   │   ├── forward.rs
│   │   │   │   ├── camera.rs
│   │   │   │   └── mesh.rs
│   │   │   ├── resources/  # Asset management
│   │   │   │   ├── mod.rs
│   │   │   │   ├── texture.rs
│   │   │   │   └── shader.rs
│   │   │   └── window.rs   # Winit integration
│   │   └── Cargo.toml
│   ├── rift-math/          # Math utilities (thin wrapper over glam)
│   │   ├── src/lib.rs
│   │   └── Cargo.toml
│   └── rift-game/          # Game logic (later)
│       ├── src/lib.rs
│       └── Cargo.toml
├── assets/
│   ├── shaders/            # GLSL → SPIR-V
│   ├── models/
│   └── textures/
├── examples/
│   └── triangle.rs         # First milestone
└── ARCHITECTURE.md
```

## Key Design Decisions

1. **Raw `ash` over `wgpu`** — Full Vulkan control, learn the API properly, no abstraction overhead for a custom engine.
2. **Cargo workspace** — Separate crates for engine, math, game. Clean dependency boundaries.
3. **Forward rendering first** — Simpler, works well for stylized art with limited light counts. Can evolve to Forward+ later.
4. **GLSL shaders compiled to SPIR-V** — Use `shaderc` or offline compilation via `glslc`.
5. **Low-poly stylized** — Faster asset iteration, distinctive look, less demanding on the renderer early on.

## Immediate Next Steps

1. Initialize Cargo workspace with crate structure
2. Set up Vulkan instance creation + validation layers
3. Window creation with winit + surface
4. Get a colored triangle rendering (the "Hello World" of graphics)
