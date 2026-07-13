use std::time::{Duration, Instant};

use glam::Vec3;
use spacetimedb_sdk::{DbContext, Table};

use crate::input::Camera;
use crate::module_bindings::{
    DbConnection, PlayerTableAccess, playerQueryTableAccess, update_player_transform,
    worldQueryTableAccess,
};

const HOST: &str = "http://127.0.0.1:3000";
const DATABASE: &str = "poutre";
const SEND_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub struct RemotePlayer {
    pub position: Vec3,
    pub yaw: f32,
}

pub struct Network {
    connection: DbConnection,
    last_sent_at: Option<Instant>,
    last_sent_camera: Option<Camera>,
    connection_error_logged: bool,
}

impl Network {
    pub fn connect() -> Self {
        let connection = DbConnection::builder()
            .with_uri(HOST)
            .with_database_name(DATABASE)
            .on_connect(|ctx, identity, _token| {
                tracing::info!(%identity, "connected to SpacetimeDB");
                ctx.subscription_builder()
                    .on_applied(|_| tracing::info!("world and player subscriptions ready"))
                    .on_error(|_, error| tracing::error!(%error, "subscription failed"))
                    .add_query(|query| query.from.world())
                    .add_query(|query| query.from.player())
                    .subscribe();
            })
            .on_connect_error(|_, error| tracing::error!(%error, "SpacetimeDB connection failed"))
            .on_disconnect(|_, error| {
                if let Some(error) = error {
                    tracing::error!(%error, "disconnected from SpacetimeDB");
                } else {
                    tracing::info!("disconnected from SpacetimeDB");
                }
            })
            // No token is persisted or reused, so every game process receives a new identity.
            .build()
            .expect("failed to create SpacetimeDB connection");

        Self {
            connection,
            last_sent_at: None,
            last_sent_camera: None,
            connection_error_logged: false,
        }
    }

    pub fn tick(&mut self, camera: Camera) -> Vec<RemotePlayer> {
        if let Err(error) = self.connection.frame_tick() {
            if !self.connection_error_logged {
                tracing::error!(%error, "failed to advance SpacetimeDB connection");
                self.connection_error_logged = true;
            }
            return Vec::new();
        }

        let Some(local_identity) = self.connection.try_identity() else {
            return Vec::new();
        };

        let now = Instant::now();
        let send_due = self
            .last_sent_at
            .is_none_or(|last_sent| now.duration_since(last_sent) >= SEND_INTERVAL);
        let transform_changed = self.last_sent_camera.is_none_or(|previous| {
            previous.position != camera.position
                || previous.yaw != camera.yaw
                || previous.pitch != camera.pitch
        });
        if send_due && transform_changed {
            if let Err(error) = self.connection.reducers.update_player_transform(
                camera.position.x,
                camera.position.y,
                camera.position.z,
                camera.yaw,
                camera.pitch,
            ) {
                tracing::warn!(%error, "failed to send local player transform");
            } else {
                self.last_sent_at = Some(now);
                self.last_sent_camera = Some(camera);
            }
        }

        self.connection
            .db
            .player()
            .iter()
            .filter(|player| player.online && player.identity != local_identity)
            .map(|player| RemotePlayer {
                position: Vec3::new(player.x, player.y, player.z),
                yaw: player.yaw,
            })
            .collect()
    }
}
