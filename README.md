# poutre

Poutre is a small voxel engine written in Rust with `wgpu` and `winit`.

It includes a streamed procedural Perlin-noise world with 32 x 32 x 32 chunks and
0.1-unit voxels, mountains, snow, grass, procedurally placed voxel flowers, bushes,
trees, animated water and foam, voxel clouds, distance-hazed daylight, depth-tested
GPU rendering, terrain collision, a walking controller, and an egui performance overlay.

## Run

```sh
cargo run
```

Click the viewport to capture the mouse. Use `WASD` to move, `Space` to jump, and
`Escape` to release the mouse. While the mouse is captured, right-click terrain or
trees to excavate a sphere with an 8-block radius.

## Multiplayer server

The SpacetimeDB module in `server` generates and stores authoritative terrain chunks,
including foliage and collision heights, and streams them to subscribed clients. It also
stores authoritative player transforms. Install the SpacetimeDB CLI, then build and publish it:

```sh
spacetime build --module-path server
spacetime publish --server local poutre --module-path server
```

Regenerate the checked-in Rust client bindings after changing the module schema:

```sh
spacetime generate --lang rust --out-dir src/module_bindings --module-path server --yes
```

Each game process connects to `http://127.0.0.1:3000` without a persisted token, so it
receives a separate identity. The client subscribes to `world`, `player`, and only the
`world_chunk` rows near the camera, requests missing chunks through `request_world_chunks`,
sends its local transform through `update_player_transform`, sends terrain edits through
`excavate`, and renders streamed terrain and other online players.
