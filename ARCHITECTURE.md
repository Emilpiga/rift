# Rift — Architecture

One-page map of how the crates fit together. Update this when
the dependency graph or top-level responsibilities shift, not
on every feature.

## Game

Action-RPG rift crawler in the Diablo / PoE lineage. Run loop:
descend a procedurally generated multi-floor rift, fight scaled
enemies, kill the boss, extract loot back to the hub, gear up,
go again. Multiplayer is server-authoritative.

## Crate map

```
                 +---------------+
                 |   rift-math   |   pure math + physics primitives
                 +-------+-------+   (glam wrappers, raycast, A*, LOS)
                         ^
                 +-------+-------+
                 | rift-dungeon  |   procgen: BSP, rooms, props,
                 +-------+-------+   Floor::path / line_of_sight
                         ^
                 +-------+-------+
                 |   rift-game   |   gameplay rules: stats, items,
                 +-------+-------+   abilities, kinematics, classes.
                         ^           No Vulkan, no hecs.
        +----------------+----------------+
        |                |                |
+-------+-------+ +------+------+  +------+--------+
|  rift-net    | | rift-engine | | rift-persist.  |
+--------------+ +-------------+  +---------------+
| wire types,  | | Vulkan,     |  | Postgres /    |
| renet setup, | | hecs ECS,   |  | sqlx — saves, |
| protocol ver | | renderer,   |  | accounts,     |
+------+-------+ | UI, ai nav, |  | inventory,    |
       |         | input, vfx, |  | stash         |
       |         | overlay     |  +------+--------+
       |         +-----+-------+         |
       |               ^                 |
       |        +------+-------+         |
       |        | rift-audio   |         |
       |        +------+-------+         |
       |               ^                 |
       v               |                 |
+--------+--------+    |    +------------+----+
|   rift-server   |<---+    |   rift-client   |
+-----------------+         +-----------------+
|  headless sim   |         | window, input,  |
|  authoritative  |         | renderer driver,|
|  step + dispatch|         | game state,     |
+-----------------+         | net client      |
                            +-----------------+
```

### Edges that matter

- `rift-server` does **not** depend on `rift-engine` (no
  Vulkan / winit on the headless build). Enforced by
  `Cargo.toml`. See `DEPLOYMENT.md`.
- `rift-net` depends on nothing game-specific. Wire types
  reference `rift-game` content tables only by stable id /
  index, never by import.
- `rift-game` is the single source of truth for gameplay
  rules. Both server (auth) and client (prediction / display)
  call into the same functions.

## Tech stack

- **Language:** Rust (workspace, edition 2021).
- **Graphics API:** Vulkan via `ash` raw bindings.
- **Windowing:** `winit`.
- **Math:** `glam`.
- **ECS:** `hecs` (lightweight).
- **Networking:** `renet` over UDP (netcode.io).
- **Persistence:** `sqlx` + Postgres.
- **Audio:** owned `rift-audio` crate (cpal-based).
- **Asset loading:** `gltf` for models, `image` for textures,
  GLSL → SPIR-V via `shaderc`.

## Server / client split

`rift-server` is a separate binary (`rift-server`) intended
for cloud deploy; `rift-client` is the player binary
(`rift` / `rift.exe`). Both binaries share `rift-game`,
`rift-net`, `rift-dungeon`, `rift-math` so the simulation runs
identically on either side. Operations and deploy details live
in `DEPLOYMENT.md`.

## Authority model

Server-authoritative. Client sends inputs (`ClientMsg`); server
runs the simulation and broadcasts snapshots + reliable events
(`ServerMsg`). Client-side prediction is limited to local
movement; everything else (damage, loot, state transitions) is
applied on receipt of a server message.

## Rendering pipeline

Forward, single-pass over a depth pre-pass:

1. **Shadow pass** — directional cascade (4096² D32, ortho frustum)
   plus a point-light cubemap-array atlas with 8-tap rotated
   Poisson PCF.
2. **Skin compute** — `skin.comp` writes posed vertices into
   per-character VBOs.
3. **Forward opaque** — `triangle.vert` / `triangle.frag` Blinn-
   Phong + 5×5 separable Gaussian PCF on the directional shadow.
4. **VFX particles & ribbons** — declarative
   `Effect = Vec<Layer>` system, additive / alpha blends.
5. **Sky** — full-screen `sky.vert`/`sky.frag` quad after opaque.
6. **Post chain** — bloom (`post_bright` → `post_blur`) +
   composite (`post_composite`) + overlay (UI / icons /
   damage text) on top.

Lighting / fog parameters are pumped into the per-frame UBO so
each floor can drive its own atmosphere theme (see
`BEFORE_PUBLISHING.md` → "Atmosphere — lighting progression").

## Where to look

- `crates/rift-engine/src/renderer/` — Vulkan + render passes.
- `crates/rift-engine/src/ecs/systems.rs` — frame logic.
- `crates/rift-game/src/` — pure rules (stats, items,
  abilities, kinematics, classes).
- `crates/rift-server/src/sim/` — authoritative tick:
  `step.rs`, `ability.rs`, `channel.rs`, `projectile.rs`,
  `enemies/`.
- `crates/rift-net/src/messages.rs` — wire schema.
- `crates/rift-dungeon/src/lib.rs` — `Floor`, A\* (`path`),
  LOS (`line_of_sight`).

## What lives outside the crates

- `assets/` — shaders, models, textures, icons. Loaded
  relative to the binary's working dir.
- `migrations/` (under `rift-persistence/`) — sqlx forward-
  only schema changes.
- `Dockerfile.server` + `fly.toml` — server image + deploy.
- `scripts/` — packaging + dev helpers.
