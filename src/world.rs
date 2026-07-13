use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::Vec3;

pub(crate) const VOXEL_SIZE: f32 = 0.1;
const CHUNK_SIZE: usize = 32;
const WORLD_CHUNKS: usize = 600;
const WORLD_VOXELS: usize = CHUNK_SIZE * WORLD_CHUNKS;
pub(crate) const WORLD_SIZE: f32 = WORLD_VOXELS as f32 * VOXEL_SIZE;
const RENDER_DISTANCE_CHUNKS: isize = 160;

const STREAM_STEP: isize = 16;
const FALLBACK_GROUND_HEIGHT: f32 = 23.3;
const PATCH_FORMAT_FLAG: u64 = 1 << 23;

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Quad {
    pub packed: [u32; 4],
}

pub(crate) struct Mesh {
    pub quads: Vec<Quad>,
    pub water_quads: Vec<Quad>,
    pub chunk_count: usize,
}

struct Chunk {
    solid_quads: Vec<Quad>,
    water_quads: Vec<Quad>,
    collision_voxels: HashSet<[u32; 3]>,
}

#[derive(Default)]
pub(crate) struct World {
    chunks: HashMap<u64, Arc<Chunk>>,
    heights: HashMap<(u32, u32), Vec<u16>>,
    collision_voxels: HashMap<[u32; 3], usize>,
    revision: u64,
}

impl World {
    pub(crate) fn spawn_position() -> Vec3 {
        let center = WORLD_SIZE * 0.5;
        Vec3::new(center, FALLBACK_GROUND_HEIGHT + 1.7, center)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn insert_chunk(
        &mut self,
        id: u64,
        chunk_x: u32,
        chunk_z: u32,
        heights: Vec<u16>,
        solid_quads: Vec<u32>,
        water_quads: Vec<u32>,
    ) {
        if decode_chunk_id(id).is_none() {
            return;
        }
        let Some(solid_quads) = unpack_quads(solid_quads) else {
            tracing::warn!(id, "discarding malformed streamed solid geometry");
            return;
        };
        let Some(water_quads) = unpack_quads(water_quads) else {
            tracing::warn!(id, "discarding malformed streamed water geometry");
            return;
        };
        if !heights.is_empty() && heights.len() != CHUNK_SIZE * CHUNK_SIZE {
            tracing::warn!(id, "discarding streamed chunk with malformed heights");
            return;
        }

        if !heights.is_empty() {
            self.heights.insert((chunk_x, chunk_z), heights);
        }
        let collision_voxels = solid_quads
            .iter()
            .filter(|quad| matches!(quad.packed[3] >> 17, 9 | 10))
            .map(|quad| [quad.packed[0], quad.packed[1], quad.packed[2]])
            .collect::<HashSet<_>>();
        if let Some(previous) = self.chunks.remove(&id) {
            for voxel in &previous.collision_voxels {
                let count = self
                    .collision_voxels
                    .get_mut(voxel)
                    .expect("loaded chunk collision voxel is not indexed");
                *count -= 1;
                if *count == 0 {
                    self.collision_voxels.remove(voxel);
                }
            }
        }
        for voxel in &collision_voxels {
            *self.collision_voxels.entry(*voxel).or_default() += 1;
        }
        self.chunks.insert(
            id,
            Arc::new(Chunk {
                solid_quads,
                water_quads,
                collision_voxels,
            }),
        );
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn raycast_solid(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<[u32; 3]> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }

        let mut nearest = max_distance;
        let mut hit = None;
        for id in self.rendered_chunk_ids(origin) {
            if decode_chunk_id(id).is_none_or(|(_, _, lod)| lod != 1) {
                continue;
            }
            for quad in &self.chunks[&id].solid_quads {
                let material = quad.packed[3] >> 17;
                if !matches!(material, 0 | 1 | 2 | 6 | 7 | 9 | 10) {
                    continue;
                }
                let Some(distance) = ray_quad_distance(origin, direction, quad) else {
                    continue;
                };
                if distance > nearest {
                    continue;
                }

                let inside = origin + direction * (distance + VOXEL_SIZE * 0.01);
                if inside.x < 0.0 || inside.y < 0.0 || inside.z < 0.0 {
                    continue;
                }
                nearest = distance;
                hit = Some([
                    (inside.x / VOXEL_SIZE).floor() as u32,
                    (inside.y / VOXEL_SIZE).floor() as u32,
                    (inside.z / VOXEL_SIZE).floor() as u32,
                ]);
            }
        }
        hit
    }

    pub(crate) fn height_at(&self, x: f32, z: f32) -> f32 {
        let voxel_x = (x / VOXEL_SIZE).floor().max(0.0) as usize;
        let voxel_z = (z / VOXEL_SIZE).floor().max(0.0) as usize;
        let chunk_x = (voxel_x / CHUNK_SIZE) as u32;
        let chunk_z = (voxel_z / CHUNK_SIZE) as u32;
        let Some(heights) = self.heights.get(&(chunk_x, chunk_z)) else {
            return FALLBACK_GROUND_HEIGHT;
        };
        let local_x = voxel_x % CHUNK_SIZE;
        let local_z = voxel_z % CHUNK_SIZE;
        (heights[local_x + CHUNK_SIZE * local_z] as f32 + 1.0) * VOXEL_SIZE
    }

    pub(crate) fn intersects_solid_voxels(&self, min: Vec3, max: Vec3) -> bool {
        let Some((min_x, max_x)) = voxel_range(min.x, max.x) else {
            return false;
        };
        let Some((min_y, max_y)) = voxel_range(min.y, max.y) else {
            return false;
        };
        let Some((min_z, max_z)) = voxel_range(min.z, max.z) else {
            return false;
        };

        for z in min_z..=max_z {
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    if self.collision_voxels.contains_key(&[x, y, z]) {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(crate) fn highest_solid_top(
        &self,
        min_x: f32,
        max_x: f32,
        min_z: f32,
        max_z: f32,
        min_height: f32,
        max_height: f32,
    ) -> Option<f32> {
        let (min_x, max_x) = voxel_range(min_x, max_x)?;
        let (min_z, max_z) = voxel_range(min_z, max_z)?;
        let (min_y, max_y) = voxel_range(min_height - VOXEL_SIZE, max_height)?;
        let mut highest: Option<f32> = None;

        for z in min_z..=max_z {
            for y in min_y..=max_y {
                let top = (y + 1) as f32 * VOXEL_SIZE;
                if top < min_height || top > max_height {
                    continue;
                }
                for x in min_x..=max_x {
                    if self.collision_voxels.contains_key(&[x, y, z]) {
                        highest = Some(highest.map_or(top, |height| height.max(top)));
                    }
                }
            }
        }
        highest
    }

    pub(crate) fn lowest_solid_bottom(
        &self,
        min_x: f32,
        max_x: f32,
        min_z: f32,
        max_z: f32,
        min_height: f32,
        max_height: f32,
    ) -> Option<f32> {
        let (min_x, max_x) = voxel_range(min_x, max_x)?;
        let (min_z, max_z) = voxel_range(min_z, max_z)?;
        let (min_y, max_y) = voxel_range(min_height, max_height + VOXEL_SIZE)?;
        let mut lowest: Option<f32> = None;

        for z in min_z..=max_z {
            for y in min_y..=max_y {
                let bottom = y as f32 * VOXEL_SIZE;
                if bottom < min_height || bottom > max_height {
                    continue;
                }
                for x in min_x..=max_x {
                    if self.collision_voxels.contains_key(&[x, y, z]) {
                        lowest = Some(lowest.map_or(bottom, |height| height.min(bottom)));
                    }
                }
            }
        }
        lowest
    }

    #[cfg(test)]
    fn mesh_around(&self, position: Vec3) -> Mesh {
        self.mesh_request(position).build()
    }

    pub(crate) fn mesh_request(&self, position: Vec3) -> MeshRequest {
        let rendered_ids = self.rendered_chunk_ids(position);
        let chunks = rendered_ids
            .iter()
            .map(|id| self.chunks[id].clone())
            .collect();
        MeshRequest { chunks }
    }

    pub(crate) fn stream_bounds(position: Vec3) -> (u32, u32, u32, u32) {
        let (center_x, center_z) = stream_center_chunk(position);
        let min_x = ((center_x - RENDER_DISTANCE_CHUNKS).max(0) / 16 * 16) as u32;
        let min_z = ((center_z - RENDER_DISTANCE_CHUNKS).max(0) / 16 * 16) as u32;
        let max_x = (center_x + RENDER_DISTANCE_CHUNKS).min(WORLD_CHUNKS as isize - 1) as u32;
        let max_z = (center_z + RENDER_DISTANCE_CHUNKS).min(WORLD_CHUNKS as isize - 1) as u32;
        (min_x, min_z, max_x, max_z)
    }

    pub(crate) fn stream_cell(position: Vec3) -> (isize, isize) {
        let chunk_size = CHUNK_SIZE as f32 * VOXEL_SIZE;
        let chunk_x = (position.x / chunk_size).floor() as isize;
        let chunk_z = (position.z / chunk_size).floor() as isize;
        (chunk_x / STREAM_STEP, chunk_z / STREAM_STEP)
    }

    fn rendered_chunk_ids(&self, position: Vec3) -> HashSet<u64> {
        let mut selected = HashSet::new();
        for desired_id in desired_chunk_ids(position) {
            if let Some(loaded_id) = self.loaded_ancestor(desired_id) {
                selected.insert(loaded_id);
            }
        }

        let all_selected = selected.clone();
        selected.retain(|id| {
            let Some((mut chunk_x, mut chunk_z, mut lod)) = decode_chunk_id(*id) else {
                return false;
            };
            while lod < 16 {
                lod *= 2;
                chunk_x = (chunk_x / lod as u32) * lod as u32;
                chunk_z = (chunk_z / lod as u32) * lod as u32;
                if all_selected.contains(&chunk_id(chunk_x, chunk_z, lod)) {
                    return false;
                }
            }
            true
        });
        selected
    }

    fn loaded_ancestor(&self, id: u64) -> Option<u64> {
        let (mut chunk_x, mut chunk_z, mut lod) = decode_chunk_id(id)?;
        loop {
            let id = chunk_id(chunk_x, chunk_z, lod);
            if self.chunks.contains_key(&id) {
                return Some(id);
            }
            if lod == 16 {
                return None;
            }
            lod *= 2;
            chunk_x = (chunk_x / lod as u32) * lod as u32;
            chunk_z = (chunk_z / lod as u32) * lod as u32;
        }
    }
}

pub(crate) struct MeshRequest {
    chunks: Vec<Arc<Chunk>>,
}

impl MeshRequest {
    pub(crate) fn build(self) -> Mesh {
        let solid_quad_count = self
            .chunks
            .iter()
            .map(|chunk| chunk.solid_quads.len())
            .sum();
        let water_quad_count = self
            .chunks
            .iter()
            .map(|chunk| chunk.water_quads.len())
            .sum();
        let mut quads = Vec::with_capacity(solid_quad_count);
        let mut water_quads = Vec::with_capacity(water_quad_count);
        for chunk in &self.chunks {
            quads.extend_from_slice(&chunk.solid_quads);
            water_quads.extend_from_slice(&chunk.water_quads);
        }
        Mesh {
            quads,
            water_quads,
            chunk_count: self.chunks.len(),
        }
    }
}

fn desired_chunk_ids(position: Vec3) -> Vec<u64> {
    let (center_x, center_z) = stream_center_chunk(position);
    let mut ids = Vec::new();
    let (min_x, min_z, max_x, max_z) = World::stream_bounds(position);
    for z in (min_z..=max_z).step_by(16) {
        for x in (min_x..=max_x).step_by(16) {
            select_tiles(x, z, 16, center_x, center_z, &mut ids);
        }
    }
    ids.sort_unstable_by_key(|id| tile_sort_key(*id, center_x, center_z));
    ids
}

pub(crate) fn requested_chunk_ids(position: Vec3) -> Vec<u64> {
    let desired = desired_chunk_ids(position);
    let (center_x, center_z) = stream_center_chunk(position);
    let mut ids = Vec::with_capacity(desired.len() + 512);
    ids.extend(
        desired
            .iter()
            .copied()
            .filter(|id| decode_chunk_id(*id).is_some_and(|(_, _, lod)| lod == 1)),
    );

    let (min_x, min_z, max_x, max_z) = World::stream_bounds(position);
    let mut coverage = Vec::new();
    for z in (min_z..=max_z).step_by(16) {
        for x in (min_x..=max_x).step_by(16) {
            if tile_distance(x, z, 16, center_x, center_z) <= RENDER_DISTANCE_CHUNKS {
                coverage.push(chunk_id(x, z, 16));
            }
        }
    }
    coverage.sort_unstable_by_key(|id| tile_sort_key(*id, center_x, center_z));
    ids.extend(coverage);
    ids.extend(desired);
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(*id));
    ids
}

fn chunk_id(chunk_x: u32, chunk_z: u32, lod: u8) -> u64 {
    let lod_code = match lod {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        16 => 4,
        _ => unreachable!("unsupported chunk lod"),
    };
    PATCH_FORMAT_FLAG | chunk_x as u64 | ((chunk_z as u64) << 10) | (lod_code << 20)
}

fn decode_chunk_id(id: u64) -> Option<(u32, u32, u8)> {
    if id >> 24 != 0 || id & PATCH_FORMAT_FLAG == 0 {
        return None;
    }
    let lod = match (id >> 20) & 7 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => return None,
    };
    Some(((id & 0x3ff) as u32, ((id >> 10) & 0x3ff) as u32, lod))
}

fn stream_center_chunk(position: Vec3) -> (isize, isize) {
    let (cell_x, cell_z) = World::stream_cell(position);
    (
        cell_x * STREAM_STEP + STREAM_STEP / 2,
        cell_z * STREAM_STEP + STREAM_STEP / 2,
    )
}

fn select_tiles(
    chunk_x: u32,
    chunk_z: u32,
    lod: u8,
    center_x: isize,
    center_z: isize,
    ids: &mut Vec<u64>,
) {
    let distance = tile_distance(chunk_x, chunk_z, lod, center_x, center_z);
    if distance > RENDER_DISTANCE_CHUNKS {
        return;
    }
    let refinement_distance = match lod {
        16 => 128,
        8 => 64,
        4 => 32,
        2 => 16,
        1 => {
            ids.push(chunk_id(chunk_x, chunk_z, lod));
            return;
        }
        _ => unreachable!("unsupported chunk lod"),
    };
    if distance > refinement_distance {
        ids.push(chunk_id(chunk_x, chunk_z, lod));
        return;
    }

    let child_lod = lod / 2;
    for offset_z in [0, child_lod as u32] {
        for offset_x in [0, child_lod as u32] {
            let child_x = chunk_x + offset_x;
            let child_z = chunk_z + offset_z;
            if child_x < WORLD_CHUNKS as u32 && child_z < WORLD_CHUNKS as u32 {
                select_tiles(child_x, child_z, child_lod, center_x, center_z, ids);
            }
        }
    }
}

fn tile_distance(chunk_x: u32, chunk_z: u32, lod: u8, center_x: isize, center_z: isize) -> isize {
    let end_x = (chunk_x + lod as u32).min(WORLD_CHUNKS as u32) as isize;
    let end_z = (chunk_z + lod as u32).min(WORLD_CHUNKS as u32) as isize;
    distance_to_range(center_x, chunk_x as isize, end_x).max(distance_to_range(
        center_z,
        chunk_z as isize,
        end_z,
    ))
}

fn distance_to_range(value: isize, start: isize, end: isize) -> isize {
    if value < start {
        start - value
    } else if value >= end {
        value - (end - 1)
    } else {
        0
    }
}

fn tile_sort_key(id: u64, center_x: isize, center_z: isize) -> (isize, u8) {
    let (chunk_x, chunk_z, lod) = decode_chunk_id(id).expect("generated invalid chunk id");
    (
        tile_distance(chunk_x, chunk_z, lod, center_x, center_z),
        lod,
    )
}

fn unpack_quads(values: Vec<u32>) -> Option<Vec<Quad>> {
    if !values.len().is_multiple_of(4) {
        return None;
    }
    Some(
        values
            .chunks_exact(4)
            .map(|packed| Quad {
                packed: [packed[0], packed[1], packed[2], packed[3]],
            })
            .collect(),
    )
}

fn voxel_range(min: f32, max: f32) -> Option<(u32, u32)> {
    if !min.is_finite() || !max.is_finite() || max <= 0.0 || min >= max {
        return None;
    }
    let epsilon = VOXEL_SIZE * 0.001;
    let first = (min.max(0.0) / VOXEL_SIZE).floor() as u32;
    let last = ((max - epsilon).max(0.0) / VOXEL_SIZE).floor() as u32;
    (first <= last).then_some((first, last))
}

fn ray_quad_distance(origin: Vec3, direction: Vec3, quad: &Quad) -> Option<f32> {
    let [x, y, z, packed] = quad.packed;
    let face = packed & 7;
    let width = ((packed >> 3) & 31) + 1;
    let height = ((packed >> 8) & 511) + 1;
    let min = Vec3::new(x as f32, y as f32, z as f32) * VOXEL_SIZE;
    let width = width as f32 * VOXEL_SIZE;
    let height = height as f32 * VOXEL_SIZE;
    let epsilon = 0.0001;

    let (distance, first, second, first_range, second_range) = match face {
        0 | 5 => {
            let plane = min.y + if face == 0 { VOXEL_SIZE } else { 0.0 };
            let distance = (plane - origin.y) / direction.y;
            let point = origin + direction * distance;
            (
                distance,
                point.x,
                point.z,
                (min.x, min.x + width),
                (min.z, min.z + height),
            )
        }
        1 | 2 => {
            let plane = min.z + if face == 1 { width } else { 0.0 };
            let distance = (plane - origin.z) / direction.z;
            let point = origin + direction * distance;
            (
                distance,
                point.x,
                point.y,
                (min.x, min.x + width),
                (min.y, min.y + height),
            )
        }
        3 | 4 => {
            let plane = min.x + if face == 3 { width } else { 0.0 };
            let distance = (plane - origin.x) / direction.x;
            let point = origin + direction * distance;
            (
                distance,
                point.z,
                point.y,
                (min.z, min.z + width),
                (min.y, min.y + height),
            )
        }
        _ => return None,
    };
    if !distance.is_finite() || distance < 0.0 {
        return None;
    }
    if first + epsilon < first_range.0
        || first - epsilon > first_range.1
        || second + epsilon < second_range.0
        || second - epsilon > second_range.1
    {
        return None;
    }
    Some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desired_chunks_are_nearest_first_and_use_distance_lod() {
        let ids = desired_chunk_ids(World::spawn_position());
        assert!(ids.len() > 1_000);
        assert!(ids.len() < 5_000);
        assert_eq!(decode_chunk_id(ids[0]).unwrap().2, 1);
        assert!(ids.iter().any(|id| decode_chunk_id(*id).unwrap().2 == 16));
    }

    #[test]
    fn streamed_heights_drive_collision_height() {
        let mut world = World::default();
        world.insert_chunk(
            chunk_id(0, 0, 1),
            0,
            0,
            vec![149; CHUNK_SIZE * CHUNK_SIZE],
            vec![],
            vec![],
        );
        assert_eq!(world.height_at(0.1, 0.1), 15.0);
    }

    #[test]
    fn loaded_parent_patch_fills_missing_finer_tiles() {
        let position = World::spawn_position();
        let (center_x, center_z) = stream_center_chunk(position);
        let parent_x = (center_x as u32 / 16) * 16;
        let parent_z = (center_z as u32 / 16) * 16;
        let mut world = World::default();
        world.insert_chunk(
            chunk_id(parent_x, parent_z, 16),
            parent_x,
            parent_z,
            Vec::new(),
            vec![0, 0, 0, 0],
            Vec::new(),
        );

        assert_eq!(world.mesh_around(position).chunk_count, 1);
    }

    #[test]
    fn raycast_returns_the_block_behind_a_terrain_face() {
        let mut world = World::default();
        world.insert_chunk(
            chunk_id(0, 0, 1),
            0,
            0,
            vec![20; CHUNK_SIZE * CHUNK_SIZE],
            vec![10, 20, 30, 1 << 17],
            Vec::new(),
        );

        assert_eq!(
            world.raycast_solid(Vec3::new(1.05, 4.0, 3.05), Vec3::NEG_Y, 5.0),
            Some([10, 20, 30])
        );
    }

    #[test]
    fn raycast_targets_tree_materials() {
        let mut world = World::default();
        world.insert_chunk(
            chunk_id(0, 0, 1),
            0,
            0,
            vec![20; CHUNK_SIZE * CHUNK_SIZE],
            vec![10, 30, 30, 3 | (9 << 17)],
            Vec::new(),
        );

        assert_eq!(
            world.raycast_solid(Vec3::new(2.0, 3.05, 3.05), Vec3::NEG_X, 5.0),
            Some([10, 30, 30])
        );
    }

    #[test]
    fn tree_quads_are_indexed_as_solid_world_voxels() {
        let mut world = World::default();
        world.insert_chunk(
            chunk_id(0, 0, 1),
            0,
            0,
            vec![20; CHUNK_SIZE * CHUNK_SIZE],
            vec![10, 30, 30, 3 | (9 << 17)],
            Vec::new(),
        );

        assert!(
            world
                .intersects_solid_voxels(Vec3::new(0.95, 2.95, 2.95), Vec3::new(1.15, 3.15, 3.15),)
        );
        assert_eq!(
            world.highest_solid_top(0.95, 1.15, 2.95, 3.15, 3.0, 3.2),
            Some(3.1000001)
        );
        assert_eq!(
            world.lowest_solid_bottom(0.95, 1.15, 2.95, 3.15, 2.9, 3.1),
            Some(3.0)
        );
    }

    #[test]
    fn replacing_a_chunk_removes_its_old_tree_collision() {
        let mut world = World::default();
        let id = chunk_id(0, 0, 1);
        world.insert_chunk(
            id,
            0,
            0,
            vec![20; CHUNK_SIZE * CHUNK_SIZE],
            vec![10, 30, 30, 3 | (10 << 17)],
            Vec::new(),
        );
        world.insert_chunk(
            id,
            0,
            0,
            vec![20; CHUNK_SIZE * CHUNK_SIZE],
            Vec::new(),
            Vec::new(),
        );

        assert!(
            !world.intersects_solid_voxels(Vec3::new(1.0, 3.0, 3.0), Vec3::new(1.1, 3.1, 3.1),)
        );
    }
}
