use spacetimedb::{ConnectionId, Identity, ReducerContext, Table, Timestamp};

mod generation;
mod validation;

use generation::{
    EXCAVATION_RADIUS_VOXELS, ExcavationSphere, TREE_HORIZONTAL_EXTENT, WorldGenerator,
};
use validation::{
    CHUNK_SIZE, RENDER_DISTANCE_CHUNKS, VOXEL_SIZE, WORLD_CHUNKS, WORLD_SIZE, validate_transform,
};

const WORLD_ID: u8 = 0;
const WORLD_SEED: u32 = 42;
// Bump this with the client constant when persisted chunk generation changes.
const CHUNK_GENERATION_VERSION: u64 = 1 << 32;
const SPAWN_EYE_HEIGHT: f32 = 25.0;
const MAX_CHUNKS_PER_REQUEST: usize = 64;
const PATCH_FORMAT_FLAG: u64 = 1 << 23;
const STREAM_CELL_MARGIN_CHUNKS: i64 = 16;
const MAX_EXCAVATION_DISTANCE: f32 = 8.25;
const MAX_MESH_HEIGHT_VOXELS: u32 = 640;

#[spacetimedb::table(accessor = world, public)]
pub struct World {
    #[primary_key]
    pub id: u8,
    pub seed: u32,
    pub voxel_size: f32,
    pub chunk_size: u32,
    pub world_chunks: u32,
}

#[spacetimedb::table(accessor = world_chunk, public)]
pub struct WorldChunk {
    #[primary_key]
    pub id: u64,
    pub chunk_x: u32,
    pub chunk_z: u32,
    pub lod: u8,
    pub heights: Vec<u16>,
    pub solid_quads: Vec<u32>,
    pub water_quads: Vec<u32>,
    pub revision: u64,
}

#[spacetimedb::table(accessor = excavation)]
pub struct Excavation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

#[spacetimedb::table(accessor = player, public)]
pub struct Player {
    #[primary_key]
    pub identity: Identity,
    pub online: bool,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub updated_at: Timestamp,
}

#[spacetimedb::table(accessor = connection)]
struct Connection {
    identity: Identity,
    connection_id: ConnectionId,
}

#[spacetimedb::reducer(init)]
pub fn init(ctx: &ReducerContext) {
    ctx.db.world().insert(World {
        id: WORLD_ID,
        seed: WORLD_SEED,
        voxel_size: VOXEL_SIZE,
        chunk_size: CHUNK_SIZE,
        world_chunks: WORLD_CHUNKS,
    });
}

#[spacetimedb::reducer(client_connected)]
pub fn client_connected(ctx: &ReducerContext) {
    ctx.db.connection().insert(Connection {
        identity: ctx.sender(),
        connection_id: ctx
            .connection_id()
            .expect("connected client has no connection id"),
    });

    if let Some(player) = ctx.db.player().identity().find(ctx.sender()) {
        ctx.db.player().identity().update(Player {
            online: true,
            updated_at: ctx.timestamp,
            ..player
        });
    } else {
        let center = WORLD_SIZE * 0.5;
        ctx.db.player().insert(Player {
            identity: ctx.sender(),
            online: true,
            x: center,
            y: SPAWN_EYE_HEIGHT,
            z: center,
            yaw: 0.0,
            pitch: 0.0,
            updated_at: ctx.timestamp,
        });
    }
}

#[spacetimedb::reducer(client_disconnected)]
pub fn client_disconnected(ctx: &ReducerContext) {
    let connection_id = ctx
        .connection_id()
        .expect("disconnected client has no connection id");
    if let Some(connection) = ctx
        .db
        .connection()
        .iter()
        .find(|row| row.identity == ctx.sender() && row.connection_id == connection_id)
    {
        ctx.db.connection().delete(connection);
    }

    let has_another_connection = ctx
        .db
        .connection()
        .iter()
        .any(|row| row.identity == ctx.sender());
    if !has_another_connection && let Some(player) = ctx.db.player().identity().find(ctx.sender()) {
        ctx.db.player().identity().update(Player {
            online: false,
            updated_at: ctx.timestamp,
            ..player
        });
    }
}

#[spacetimedb::reducer]
pub fn update_player_transform(
    ctx: &ReducerContext,
    x: f32,
    y: f32,
    z: f32,
    yaw: f32,
    pitch: f32,
) -> Result<(), String> {
    validate_transform(x, y, z, yaw, pitch)?;

    let player = ctx
        .db
        .player()
        .identity()
        .find(ctx.sender())
        .ok_or("player is not connected")?;
    if !player.online {
        return Err("player is not connected".into());
    }

    ctx.db.player().identity().update(Player {
        x,
        y,
        z,
        yaw,
        pitch,
        updated_at: ctx.timestamp,
        ..player
    });
    Ok(())
}

#[spacetimedb::reducer]
pub fn request_world_chunks(ctx: &ReducerContext, chunk_ids: Vec<u64>) -> Result<(), String> {
    if chunk_ids.is_empty() || chunk_ids.len() > MAX_CHUNKS_PER_REQUEST {
        return Err(format!(
            "request must contain between 1 and {MAX_CHUNKS_PER_REQUEST} chunks"
        ));
    }
    let player = ctx
        .db
        .player()
        .identity()
        .find(ctx.sender())
        .filter(|player| player.online)
        .ok_or("player is not connected")?;
    let chunk_world_size = CHUNK_SIZE as f32 * VOXEL_SIZE;
    let player_chunk_x = (player.x / chunk_world_size).floor() as i64;
    let player_chunk_z = (player.z / chunk_world_size).floor() as i64;
    let generator = WorldGenerator::with_excavations(WORLD_SEED, excavation_spheres(ctx));

    for id in chunk_ids {
        let existing = ctx.db.world_chunk().id().find(id);
        if existing
            .as_ref()
            .is_some_and(|chunk| chunk.revision >= CHUNK_GENERATION_VERSION)
        {
            continue;
        }
        let (chunk_x, chunk_z, lod) = decode_chunk_id(id)?;
        let patch_end_x = (chunk_x + lod as u32).min(WORLD_CHUNKS) as i64;
        let patch_end_z = (chunk_z + lod as u32).min(WORLD_CHUNKS) as i64;
        let distance_x = distance_to_range(player_chunk_x, chunk_x as i64, patch_end_x);
        let distance_z = distance_to_range(player_chunk_z, chunk_z as i64, patch_end_z);
        let distance = distance_x.max(distance_z);
        if distance > RENDER_DISTANCE_CHUNKS as i64 + STREAM_CELL_MARGIN_CHUNKS {
            return Err("requested chunk is outside the player's stream radius".into());
        }
        let generated = generator.generate_patch(chunk_x, chunk_z, lod as usize);
        let heights = generated.heights;
        let solid_quads = flatten_quads(generated.quads);
        let water_quads = flatten_quads(generated.water_quads);
        if let Some(existing) = existing {
            ctx.db.world_chunk().id().update(WorldChunk {
                heights,
                solid_quads,
                water_quads,
                revision: CHUNK_GENERATION_VERSION,
                ..existing
            });
        } else {
            ctx.db.world_chunk().insert(WorldChunk {
                id,
                chunk_x,
                chunk_z,
                lod,
                heights,
                solid_quads,
                water_quads,
                revision: CHUNK_GENERATION_VERSION,
            });
        }
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn excavate(ctx: &ReducerContext, x: u32, y: u32, z: u32) -> Result<(), String> {
    let player = ctx
        .db
        .player()
        .identity()
        .find(ctx.sender())
        .filter(|player| player.online)
        .ok_or("player is not connected")?;
    let world_voxels = CHUNK_SIZE * WORLD_CHUNKS;
    if x >= world_voxels || z >= world_voxels || y >= MAX_MESH_HEIGHT_VOXELS {
        return Err("excavation center is outside the world".into());
    }

    let min_x = x as f32 * VOXEL_SIZE;
    let min_y = y as f32 * VOXEL_SIZE;
    let min_z = z as f32 * VOXEL_SIZE;
    let closest_x = player.x.clamp(min_x, min_x + VOXEL_SIZE);
    let closest_y = player.y.clamp(min_y, min_y + VOXEL_SIZE);
    let closest_z = player.z.clamp(min_z, min_z + VOXEL_SIZE);
    let distance_squared = (closest_x - player.x).powi(2)
        + (closest_y - player.y).powi(2)
        + (closest_z - player.z).powi(2);
    if distance_squared > MAX_EXCAVATION_DISTANCE * MAX_EXCAVATION_DISTANCE {
        return Err("excavation center is out of reach".into());
    }

    let generator = WorldGenerator::with_excavations(WORLD_SEED, excavation_spheres(ctx));
    if !generator.is_solid(x as isize, y as i64, z as isize) {
        return Err("target block is not solid world geometry".into());
    }

    ctx.db.excavation().insert(Excavation { id: 0, x, y, z });
    let generator = WorldGenerator::with_excavations(WORLD_SEED, excavation_spheres(ctx));
    let radius = EXCAVATION_RADIUS_VOXELS as u32 + TREE_HORIZONTAL_EXTENT + 1;
    let min_chunk_x = x.saturating_sub(radius) / CHUNK_SIZE;
    let max_chunk_x = x.saturating_add(radius).min(world_voxels - 1) / CHUNK_SIZE;
    let min_chunk_z = z.saturating_sub(radius) / CHUNK_SIZE;
    let max_chunk_z = z.saturating_add(radius).min(world_voxels - 1) / CHUNK_SIZE;
    let affected_chunks: Vec<_> = ctx
        .db
        .world_chunk()
        .iter()
        .filter(|chunk| {
            chunk.lod == 1
                && (min_chunk_x..=max_chunk_x).contains(&chunk.chunk_x)
                && (min_chunk_z..=max_chunk_z).contains(&chunk.chunk_z)
        })
        .collect();

    for chunk in affected_chunks {
        let generated = generator.generate_patch(chunk.chunk_x, chunk.chunk_z, 1);
        ctx.db.world_chunk().id().update(WorldChunk {
            heights: generated.heights,
            solid_quads: flatten_quads(generated.quads),
            water_quads: flatten_quads(generated.water_quads),
            revision: chunk.revision.wrapping_add(1),
            ..chunk
        });
    }
    Ok(())
}

fn decode_chunk_id(id: u64) -> Result<(u32, u32, u8), String> {
    if id >> 24 != 0 || id & PATCH_FORMAT_FLAG == 0 {
        return Err("invalid chunk id".into());
    }
    let chunk_x = (id & 0x3ff) as u32;
    let chunk_z = ((id >> 10) & 0x3ff) as u32;
    let lod = match ((id >> 20) & 7) as u8 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => return Err("invalid chunk lod".into()),
    };
    if chunk_x >= WORLD_CHUNKS || chunk_z >= WORLD_CHUNKS {
        return Err("chunk is outside the world".into());
    }
    if !chunk_x.is_multiple_of(lod as u32) || !chunk_z.is_multiple_of(lod as u32) {
        return Err("chunk patch is not aligned to its lod".into());
    }
    Ok((chunk_x, chunk_z, lod))
}

fn flatten_quads(quads: Vec<generation::Quad>) -> Vec<u32> {
    quads.into_iter().flat_map(|quad| quad.packed).collect()
}

fn excavation_spheres(ctx: &ReducerContext) -> Vec<ExcavationSphere> {
    ctx.db
        .excavation()
        .iter()
        .map(|excavation| ExcavationSphere {
            x: excavation.x,
            y: excavation.y,
            z: excavation.z,
        })
        .collect()
}

fn distance_to_range(value: i64, start: i64, end: i64) -> i64 {
    if value < start {
        start - value
    } else if value >= end {
        value - (end - 1)
    } else {
        0
    }
}
