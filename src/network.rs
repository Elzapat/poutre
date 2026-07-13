use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver};

use glam::Vec3;
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table, TableWithPrimaryKey};
use web_time::{Duration, Instant};

use crate::input::Camera;
use crate::module_bindings::{
    DbConnection, PlayerTableAccess, SubscriptionHandle, WorldChunk, WorldChunkTableAccess,
    excavate, playerQueryTableAccess, request_world_chunks, update_player_transform,
    world_chunkQueryTableAccess, worldQueryTableAccess,
};
use crate::world::{World, requested_chunk_ids};

const HOST: &str = "http://127.0.0.1:3000";
const DATABASE: &str = "poutre";
// Keep this in sync with the server so stale persisted chunks are regenerated.
const CHUNK_GENERATION_VERSION: u64 = 1 << 32;
const SEND_INTERVAL: Duration = Duration::from_millis(50);
const CHUNK_REQUEST_INTERVAL: Duration = Duration::from_millis(100);
const CHUNKS_PER_REQUEST: usize = 16;
const CHUNK_UPDATES_PER_TICK: usize = 16;

#[derive(Clone, Copy)]
pub(crate) struct RemotePlayer {
    pub position: Vec3,
    pub yaw: f32,
}

pub(crate) struct NetworkUpdate {
    pub remote_players: Vec<RemotePlayer>,
    pub chunks: Vec<WorldChunk>,
}

pub(crate) struct Network {
    connection: DbConnection,
    last_sent_at: Option<Instant>,
    last_sent_camera: Option<Camera>,
    last_chunk_request_at: Option<Instant>,
    requested_stream_cell: Option<(isize, isize)>,
    chunk_request_queue: VecDeque<u64>,
    chunk_update_receiver: Receiver<(u64, u64)>,
    chunk_update_queue: VecDeque<(u64, u64)>,
    chunk_subscription: Option<SubscriptionHandle>,
    pending_chunks: HashSet<u64>,
    received_chunks: HashSet<u64>,
    chunk_revisions: HashMap<u64, u64>,
    connection_error_logged: bool,
}

impl Network {
    pub(crate) async fn connect() -> Self {
        let (chunk_update_sender, chunk_update_receiver) = mpsc::channel();
        // No token is persisted or reused, so every game process receives a new identity.
        let connection_builder = DbConnection::builder()
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
            });
        #[cfg(not(target_arch = "wasm32"))]
        let connection = connection_builder
            .build()
            .expect("failed to create SpacetimeDB connection");
        #[cfg(target_arch = "wasm32")]
        let connection = connection_builder
            .build()
            .await
            .expect("failed to create SpacetimeDB connection");

        let insert_sender = chunk_update_sender.clone();
        connection.db.world_chunk().on_insert(move |_, chunk| {
            let _ = insert_sender.send((chunk.id, chunk.revision));
        });
        connection.db.world_chunk().on_update(move |_, _, chunk| {
            let _ = chunk_update_sender.send((chunk.id, chunk.revision));
        });

        Self {
            connection,
            last_sent_at: None,
            last_sent_camera: None,
            last_chunk_request_at: None,
            requested_stream_cell: None,
            chunk_request_queue: VecDeque::new(),
            chunk_update_receiver,
            chunk_update_queue: VecDeque::new(),
            chunk_subscription: None,
            pending_chunks: HashSet::new(),
            received_chunks: HashSet::new(),
            chunk_revisions: HashMap::new(),
            connection_error_logged: false,
        }
    }

    pub(crate) fn tick(&mut self, camera: Camera) -> NetworkUpdate {
        if let Err(error) = self.connection.frame_tick() {
            if !self.connection_error_logged {
                tracing::error!(%error, "failed to advance SpacetimeDB connection");
                self.connection_error_logged = true;
            }
            return NetworkUpdate {
                remote_players: Vec::new(),
                chunks: Vec::new(),
            };
        }

        let Some(local_identity) = self.connection.try_identity() else {
            return NetworkUpdate {
                remote_players: Vec::new(),
                chunks: Vec::new(),
            };
        };

        let now = Instant::now();
        self.update_chunk_subscription(camera.position);
        self.chunk_update_queue
            .extend(self.chunk_update_receiver.try_iter());
        let mut chunks = Vec::new();
        let mut processed_updates = 0;
        while processed_updates < CHUNK_UPDATES_PER_TICK {
            let Some((id, notified_revision)) = self.chunk_update_queue.pop_front() else {
                break;
            };
            processed_updates += 1;
            if self.chunk_revisions.get(&id) == Some(&notified_revision) {
                continue;
            }
            let Some(chunk) = self.connection.db.world_chunk().id().find(&id) else {
                continue;
            };
            if chunk.revision < CHUNK_GENERATION_VERSION {
                self.chunk_revisions.insert(id, chunk.revision);
                continue;
            }
            self.received_chunks.insert(id);
            self.chunk_revisions.insert(id, chunk.revision);
            self.pending_chunks.remove(&id);
            chunks.push(chunk);
        }
        self.request_chunks(camera.position, now);
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

        let remote_players = self
            .connection
            .db
            .player()
            .iter()
            .filter(|player| player.online && player.identity != local_identity)
            .map(|player| RemotePlayer {
                position: Vec3::new(player.x, player.y, player.z),
                yaw: player.yaw,
            })
            .collect();
        NetworkUpdate {
            remote_players,
            chunks,
        }
    }

    pub(crate) fn excavate(&self, x: u32, y: u32, z: u32) {
        if let Err(error) = self.connection.reducers.excavate(x, y, z) {
            tracing::warn!(%error, "failed to request terrain excavation");
        }
    }

    fn update_chunk_subscription(&mut self, position: Vec3) {
        let stream_cell = World::stream_cell(position);
        if self.requested_stream_cell == Some(stream_cell) {
            return;
        }

        let (min_x, min_z, max_x, max_z) = World::stream_bounds(position);
        let subscription = self
            .connection
            .subscription_builder()
            .on_error(|_, error| tracing::error!(%error, "chunk subscription failed"))
            .add_query(move |query| {
                query
                    .from
                    .world_chunk()
                    .filter(move |chunk| chunk.chunk_x.gte(min_x))
                    .filter(move |chunk| chunk.chunk_x.lte(max_x))
                    .filter(move |chunk| chunk.chunk_z.gte(min_z))
                    .filter(move |chunk| chunk.chunk_z.lte(max_z))
            })
            .subscribe();
        if let Some(previous) = self.chunk_subscription.replace(subscription)
            && let Err(error) = previous.unsubscribe()
        {
            tracing::warn!(%error, "failed to replace chunk subscription");
        }
    }

    fn request_chunks(&mut self, position: Vec3, now: Instant) {
        let stream_cell = World::stream_cell(position);
        if self.requested_stream_cell != Some(stream_cell) {
            self.chunk_request_queue = requested_chunk_ids(position)
                .into_iter()
                .filter(|id| {
                    !self.received_chunks.contains(id) && !self.pending_chunks.contains(id)
                })
                .collect();
            self.requested_stream_cell = Some(stream_cell);
        }
        if self
            .last_chunk_request_at
            .is_some_and(|last| now.duration_since(last) < CHUNK_REQUEST_INTERVAL)
        {
            return;
        }

        let mut ids = Vec::with_capacity(CHUNKS_PER_REQUEST);
        while ids.len() < CHUNKS_PER_REQUEST {
            let Some(id) = self.chunk_request_queue.pop_front() else {
                break;
            };
            if !self.received_chunks.contains(&id) && !self.pending_chunks.contains(&id) {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return;
        }
        if let Err(error) = self.connection.reducers.request_world_chunks(ids.clone()) {
            tracing::warn!(%error, "failed to request streamed world chunks");
            ids.into_iter()
                .rev()
                .for_each(|id| self.chunk_request_queue.push_front(id));
        } else {
            self.pending_chunks.extend(ids);
            self.last_chunk_request_at = Some(now);
        }
    }
}
