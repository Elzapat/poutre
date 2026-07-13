# poutre

Poutre is a small voxel engine written in Rust with `wgpu` and `winit`.

It includes a streamed procedural Perlin-noise world with 32 x 32 x 32 chunks and
0.1-unit, single-color voxels, simple directional lighting, depth-tested GPU
rendering, terrain collision, a walking controller, and an egui performance overlay.

## Run

```sh
cargo run
```

Click the viewport to capture the mouse. Use `WASD` to move, `Space` to jump, and
`Escape` to release the mouse.

## Multiplayer server

The SpacetimeDB module in `server` stores the shared procedural-world settings and
authoritative player transforms. Install the SpacetimeDB CLI, then build and publish it:

```sh
spacetime build --module-path server
spacetime publish --server local poutre --module-path server
```

Regenerate the checked-in Rust client bindings after changing the module schema:

```sh
spacetime generate --lang rust --out-dir src/module_bindings --module-path server --yes
```

Each game process connects to `http://127.0.0.1:3000` without a persisted token, so it
receives a separate identity. The client subscribes to `world` and `player`, sends its
local transform through `update_player_transform`, and renders other online players.
