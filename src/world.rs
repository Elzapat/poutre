use glam::Vec3;
use noise::{NoiseFn, Perlin};
use rayon::prelude::*;

pub const VOXEL_SIZE: f32 = 0.1;
pub const CHUNK_SIZE: usize = 32;
pub const WORLD_CHUNKS: usize = 600;
pub const WORLD_VOXELS: usize = CHUNK_SIZE * WORLD_CHUNKS;
pub const WORLD_SIZE: f32 = WORLD_VOXELS as f32 * VOXEL_SIZE;
pub const RENDER_DISTANCE_CHUNKS: isize = 160;

const WORLD_HEIGHT_VOXELS: usize = 480;

#[derive(Clone, Copy)]
#[repr(u32)]
enum Face {
    Up = 0,
    Front = 1,
    Back = 2,
    Right = 3,
    Left = 4,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Quad {
    pub packed: [u32; 4],
}

pub struct Mesh {
    pub quads: Vec<Quad>,
    pub chunk_count: usize,
}

pub struct World {
    broad: Perlin,
    detail: Perlin,
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            broad: Perlin::new(seed),
            detail: Perlin::new(seed.wrapping_add(1)),
        }
    }

    pub fn spawn_position(&self) -> Vec3 {
        let center = WORLD_SIZE * 0.5;
        Vec3::new(center, self.height_at(center, center) + 1.7, center)
    }

    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let voxel_x = (x / VOXEL_SIZE).floor() as isize;
        let voxel_z = (z / VOXEL_SIZE).floor() as isize;
        (self.height_voxels(voxel_x, voxel_z) + 1) as f32 * VOXEL_SIZE
    }

    pub fn mesh_around(&self, position: Vec3) -> Mesh {
        let center_x = (position.x / (CHUNK_SIZE as f32 * VOXEL_SIZE)).floor() as isize;
        let center_z = (position.z / (CHUNK_SIZE as f32 * VOXEL_SIZE)).floor() as isize;
        let min_x = (center_x - RENDER_DISTANCE_CHUNKS).max(0);
        let min_z = (center_z - RENDER_DISTANCE_CHUNKS).max(0);
        let max_x = (center_x + RENDER_DISTANCE_CHUNKS).min(WORLD_CHUNKS as isize - 1);
        let max_z = (center_z + RENDER_DISTANCE_CHUNKS).min(WORLD_CHUNKS as isize - 1);
        let chunks: Vec<_> = (min_z..=max_z)
            .flat_map(|z| (min_x..=max_x).map(move |x| (x, z)))
            .collect();

        let quads = chunks
            .par_iter()
            .fold(Vec::new, |mut quads, &(chunk_x, chunk_z)| {
                let distance = (chunk_x - center_x).abs().max((chunk_z - center_z).abs());
                let lod = match distance {
                    0..=16 => 1,
                    17..=32 => 2,
                    33..=64 => 4,
                    65..=128 => 8,
                    _ => 16,
                };
                self.mesh_chunk(chunk_x, chunk_z, lod, &mut quads);
                quads
            })
            .reduce(Vec::new, |mut all, mut batch| {
                all.append(&mut batch);
                all
            });

        Mesh {
            quads,
            chunk_count: chunks.len(),
        }
    }

    pub fn stream_cell(position: Vec3) -> (isize, isize) {
        const STREAM_STEP: isize = 16;
        let chunk_size = CHUNK_SIZE as f32 * VOXEL_SIZE;
        let chunk_x = (position.x / chunk_size).floor() as isize;
        let chunk_z = (position.z / chunk_size).floor() as isize;
        (chunk_x / STREAM_STEP, chunk_z / STREAM_STEP)
    }

    fn mesh_chunk(&self, chunk_x: isize, chunk_z: isize, lod: usize, quads: &mut Vec<Quad>) {
        let base_x = chunk_x * CHUNK_SIZE as isize;
        let base_z = chunk_z * CHUNK_SIZE as isize;
        let samples = CHUNK_SIZE / lod;
        let stride = samples + 2;
        let mut heights = vec![0_u32; stride * stride];
        for z in 0..stride {
            for x in 0..stride {
                let sample_x = base_x + (x as isize - 1) * lod as isize;
                let sample_z = base_z + (z as isize - 1) * lod as isize;
                heights[x + stride * z] = self.height_voxels(sample_x, sample_z);
            }
        }

        for z in 0..samples {
            for x in 0..samples {
                let height = heights[x + 1 + stride * (z + 1)];
                let world_x = (base_x + (x * lod) as isize) as u32;
                let world_z = (base_z + (z * lod) as isize) as u32;
                quads.push(pack_quad(world_x, height, world_z, Face::Up, lod, lod));

                let neighbors = [
                    (heights[x + 1 + stride * (z + 2)], Face::Front),
                    (heights[x + 1 + stride * z], Face::Back),
                    (heights[x + 2 + stride * (z + 1)], Face::Right),
                    (heights[x + stride * (z + 1)], Face::Left),
                ];
                for (neighbor, face) in neighbors {
                    if neighbor < height {
                        let mut bottom = neighbor + 1;
                        let top = height + 1;
                        while bottom < top {
                            let section_top =
                                ((bottom / CHUNK_SIZE as u32) + 1) * CHUNK_SIZE as u32;
                            let extent = top.min(section_top) - bottom;
                            quads.push(pack_quad(
                                world_x,
                                bottom,
                                world_z,
                                face,
                                lod,
                                extent as usize,
                            ));
                            bottom += extent;
                        }
                    }
                }
            }
        }
    }

    fn height_voxels(&self, x: isize, z: isize) -> u32 {
        if x < 0 || z < 0 || x >= WORLD_VOXELS as isize || z >= WORLD_VOXELS as isize {
            return 0;
        }

        let physical_x = x as f64 * VOXEL_SIZE as f64 - WORLD_SIZE as f64 * 0.5;
        let physical_z = z as f64 * VOXEL_SIZE as f64 - WORLD_SIZE as f64 * 0.5;
        let hills = self.broad.get([physical_x * 0.014, physical_z * 0.014]) * 5.0;
        let roughness = self.detail.get([physical_x * 0.065, physical_z * 0.065]) * 1.25;
        let height = (18.0 + hills + roughness) / VOXEL_SIZE as f64;
        height.clamp(1.0, (WORLD_HEIGHT_VOXELS - 1) as f64) as u32
    }
}

fn pack_quad(x: u32, y: u32, z: u32, face: Face, width: usize, height: usize) -> Quad {
    debug_assert!(width <= CHUNK_SIZE && height <= 512);
    Quad {
        packed: [
            x,
            y,
            z,
            face as u32 | ((width as u32 - 1) << 3) | ((height as u32 - 1) << 8),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_is_ten_times_wider_with_tenth_size_voxels() {
        assert_eq!(WORLD_VOXELS, 19_200);
        assert_eq!(WORLD_SIZE, 1_920.0);
    }

    #[test]
    fn streamed_mesh_contains_small_single_voxel_tops() {
        let world = World::new(42);
        let mesh = world.mesh_around(world.spawn_position());
        assert_eq!(mesh.chunk_count, 103_041);
        assert!(mesh.quads.len() >= 1_089 * CHUNK_SIZE * CHUNK_SIZE);
        assert!(
            mesh.quads
                .iter()
                .all(|quad| quad.packed[0] < WORLD_VOXELS as u32)
        );
    }

    #[test]
    fn terrain_height_is_voxel_aligned() {
        let world = World::new(42);
        let height = world.height_at(WORLD_SIZE * 0.5, WORLD_SIZE * 0.5);
        assert!((height / VOXEL_SIZE - (height / VOXEL_SIZE).round()).abs() < 0.001);
    }

    #[test]
    fn chunk_dimensions_and_packing_support_32_voxels() {
        assert_eq!(CHUNK_SIZE, 32);
        let quad = pack_quad(0, 0, 0, Face::Up, CHUNK_SIZE, CHUNK_SIZE);
        assert_eq!((quad.packed[3] >> 3) & 31, 31);
        assert_eq!((quad.packed[3] >> 8) & 511, 31);
    }
}
