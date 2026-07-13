use spacetimedb::{ConnectionId, Identity, ReducerContext, Table, Timestamp};

mod validation;

use validation::{CHUNK_SIZE, VOXEL_SIZE, WORLD_CHUNKS, WORLD_SIZE, validate_transform};

const WORLD_ID: u8 = 0;
const WORLD_SEED: u32 = 42;
const SPAWN_EYE_HEIGHT: f32 = 25.0;

#[spacetimedb::table(accessor = world, public)]
pub struct World {
    #[primary_key]
    pub id: u8,
    pub seed: u32,
    pub voxel_size: f32,
    pub chunk_size: u32,
    pub world_chunks: u32,
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
