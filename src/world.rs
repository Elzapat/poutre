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
const SEA_LEVEL_VOXELS: u32 = 140;
const ROCK_LEVEL_VOXELS: u32 = 260;
const SNOW_LEVEL_VOXELS: u32 = 340;

#[derive(Clone, Copy)]
#[repr(u32)]
enum Face {
    Up = 0,
    Front = 1,
    Back = 2,
    Right = 3,
    Left = 4,
    Down = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Material {
    Grass = 0,
    Rock = 1,
    Snow = 2,
    Water = 3,
    Cloud = 4,
    Foam = 5,
    Gravel = 6,
    Dirt = 7,
    Foliage = 8,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Quad {
    pub packed: [u32; 4],
}

pub struct Mesh {
    pub quads: Vec<Quad>,
    pub water_quads: Vec<Quad>,
    pub chunk_count: usize,
}

pub struct World {
    broad: Perlin,
    detail: Perlin,
    mountains: Perlin,
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            broad: Perlin::new(seed),
            detail: Perlin::new(seed.wrapping_add(1)),
            mountains: Perlin::new(seed.wrapping_add(2)),
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

        let (mut quads, water_quads) = chunks
            .par_iter()
            .fold(
                || (Vec::new(), Vec::new()),
                |(mut quads, mut water_quads), &(chunk_x, chunk_z)| {
                    let distance = (chunk_x - center_x).abs().max((chunk_z - center_z).abs());
                    let lod = match distance {
                        0..=16 => 1,
                        17..=32 => 2,
                        33..=64 => 4,
                        65..=128 => 8,
                        _ => 16,
                    };
                    self.mesh_chunk(chunk_x, chunk_z, lod, &mut quads, &mut water_quads);
                    (quads, water_quads)
                },
            )
            .reduce(
                || (Vec::new(), Vec::new()),
                |(mut all, mut all_water), (mut batch, mut batch_water)| {
                    all.append(&mut batch);
                    all_water.append(&mut batch_water);
                    (all, all_water)
                },
            );

        self.mesh_clouds(center_x, center_z, &mut quads);

        Mesh {
            quads,
            water_quads,
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

    fn mesh_chunk(
        &self,
        chunk_x: isize,
        chunk_z: isize,
        lod: usize,
        quads: &mut Vec<Quad>,
        water_quads: &mut Vec<Quad>,
    ) {
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
                let material = terrain_material(world_x, height, world_z);
                quads.push(pack_quad(
                    world_x,
                    height,
                    world_z,
                    Face::Up,
                    lod,
                    lod,
                    material,
                ));

                if lod == 1
                    && material == Material::Grass
                    && terrain_hash(world_x, world_z).is_multiple_of(32)
                {
                    let foliage_height = 3 + (terrain_hash(world_z, world_x) & 1) as usize;
                    for face in [Face::Front, Face::Back, Face::Right, Face::Left] {
                        quads.push(pack_quad(
                            world_x,
                            height + 1,
                            world_z,
                            face,
                            1,
                            foliage_height,
                            Material::Foliage,
                        ));
                    }
                    quads.push(pack_quad(
                        world_x,
                        height + foliage_height as u32,
                        world_z,
                        Face::Up,
                        1,
                        1,
                        Material::Foliage,
                    ));
                }

                if height + 1 < SEA_LEVEL_VOXELS {
                    water_quads.push(pack_quad(
                        world_x,
                        SEA_LEVEL_VOXELS - 1,
                        world_z,
                        Face::Up,
                        lod,
                        lod,
                        Material::Water,
                    ));

                    let touches_shore = [
                        heights[x + 1 + stride * (z + 2)],
                        heights[x + 1 + stride * z],
                        heights[x + 2 + stride * (z + 1)],
                        heights[x + stride * (z + 1)],
                    ]
                    .into_iter()
                    .any(|neighbor| neighbor + 1 >= SEA_LEVEL_VOXELS);
                    if touches_shore {
                        water_quads.push(pack_quad(
                            world_x,
                            SEA_LEVEL_VOXELS - 1,
                            world_z,
                            Face::Up,
                            lod,
                            lod,
                            Material::Foam,
                        ));
                    }
                }

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
                            let (extent, material) = if bottom == height {
                                (1, material)
                            } else {
                                let section_top =
                                    ((bottom / CHUNK_SIZE as u32) + 1) * CHUNK_SIZE as u32;
                                (height.min(section_top) - bottom, Material::Rock)
                            };
                            quads.push(pack_quad(
                                world_x,
                                bottom,
                                world_z,
                                face,
                                lod,
                                extent as usize,
                                material,
                            ));
                            bottom += extent;
                        }
                    }
                }
            }
        }
    }

    fn mesh_clouds(&self, center_x: isize, center_z: isize, quads: &mut Vec<Quad>) {
        const CLOUD_CELL_CHUNKS: isize = 8;
        const CLOUD_RADIUS_CELLS: isize = 22;
        let center_cell_x = center_x.div_euclid(CLOUD_CELL_CHUNKS);
        let center_cell_z = center_z.div_euclid(CLOUD_CELL_CHUNKS);

        for cell_z in center_cell_z - CLOUD_RADIUS_CELLS..=center_cell_z + CLOUD_RADIUS_CELLS {
            for cell_x in center_cell_x - CLOUD_RADIUS_CELLS..=center_cell_x + CLOUD_RADIUS_CELLS {
                if cell_x < 0 || cell_z < 0 {
                    continue;
                }
                let hash = cloud_hash(cell_x as u32, cell_z as u32);
                if hash % 5 > 1 {
                    continue;
                }

                let base_x = cell_x * CLOUD_CELL_CHUNKS * CHUNK_SIZE as isize;
                let base_z = cell_z * CLOUD_CELL_CHUNKS * CHUNK_SIZE as isize;
                if base_x >= WORLD_VOXELS as isize || base_z >= WORLD_VOXELS as isize {
                    continue;
                }
                let x = base_x + ((hash >> 8) % 128) as isize;
                let z = base_z + ((hash >> 16) % 128) as isize;
                let y = 500 + ((hash >> 24) % 80) as isize;
                let size = 24 + ((hash >> 4) % 9) as usize;

                push_cloud_box(quads, x, y, z, size, 5);
                for (offset, y_offset, z_offset) in [(-2, 0, 1), (-1, 2, -1), (1, 1, 1), (2, 0, 0)]
                {
                    push_cloud_box(
                        quads,
                        x + offset * size as isize * 3 / 4,
                        y + y_offset,
                        z + z_offset * size as isize / 3,
                        size,
                        4 + (offset.unsigned_abs() == 1) as usize,
                    );
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
        let continents = self.broad.get([physical_x * 0.0028, physical_z * 0.0028]) * 6.0;
        let hills = self.broad.get([physical_x * 0.014, physical_z * 0.014]) * 3.0;
        let roughness = self.detail.get([physical_x * 0.065, physical_z * 0.065]);
        let mountain_region = ((self
            .mountains
            .get([physical_x * 0.0035 + 19.7, physical_z * 0.0035 - 8.3])
            - 0.08)
            / 0.32)
            .clamp(0.0, 1.0);
        let ridge = 1.0
            - self
                .mountains
                .get([physical_x * 0.011 - 4.1, physical_z * 0.011 + 13.9])
                .abs();
        let mountain_height = mountain_region * (7.0 + ridge.powi(3) * 19.0);
        let height = (15.0 + continents + hills + roughness + mountain_height) / VOXEL_SIZE as f64;
        height.clamp(1.0, (WORLD_HEIGHT_VOXELS - 1) as f64) as u32
    }
}

fn cloud_hash(x: u32, z: u32) -> u32 {
    let mut value = x.wrapping_mul(0x9e37_79b1) ^ z.wrapping_mul(0x85eb_ca77) ^ 42;
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^ (value >> 15)
}

fn terrain_hash(x: u32, z: u32) -> u32 {
    let mut value = x.wrapping_mul(0x85eb_ca6b) ^ z.wrapping_mul(0xc2b2_ae35);
    value ^= value >> 16;
    value = value.wrapping_mul(0x27d4_eb2d);
    value ^ (value >> 15)
}

fn push_cloud_box(quads: &mut Vec<Quad>, x: isize, y: isize, z: isize, size: usize, height: usize) {
    if x < 0
        || z < 0
        || x + size as isize >= WORLD_VOXELS as isize
        || z + size as isize >= WORLD_VOXELS as isize
    {
        return;
    }
    let (x, y, z) = (x as u32, y as u32, z as u32);
    for face in [
        Face::Up,
        Face::Down,
        Face::Front,
        Face::Back,
        Face::Right,
        Face::Left,
    ] {
        let face_height = if matches!(face, Face::Up | Face::Down) {
            size
        } else {
            height
        };
        let face_y = if matches!(face, Face::Up) {
            y + height as u32 - 1
        } else {
            y
        };
        quads.push(pack_quad(
            x,
            face_y,
            z,
            face,
            size,
            face_height,
            Material::Cloud,
        ));
    }
}

fn surface_material(height: u32) -> Material {
    if height >= SNOW_LEVEL_VOXELS {
        Material::Snow
    } else if height >= ROCK_LEVEL_VOXELS {
        Material::Rock
    } else {
        Material::Grass
    }
}

fn terrain_material(x: u32, height: u32, z: u32) -> Material {
    if height + 1 < SEA_LEVEL_VOXELS {
        if terrain_hash(x / 4, z / 4).is_multiple_of(3) {
            Material::Gravel
        } else {
            Material::Dirt
        }
    } else {
        surface_material(height)
    }
}

fn pack_quad(
    x: u32,
    y: u32,
    z: u32,
    face: Face,
    width: usize,
    height: usize,
    material: Material,
) -> Quad {
    debug_assert!(width <= CHUNK_SIZE && height <= 512);
    Quad {
        packed: [
            x,
            y,
            z,
            face as u32
                | ((width as u32 - 1) << 3)
                | ((height as u32 - 1) << 8)
                | ((material as u32) << 17),
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
        let quad = pack_quad(0, 0, 0, Face::Up, CHUNK_SIZE, CHUNK_SIZE, Material::Snow);
        assert_eq!((quad.packed[3] >> 3) & 31, 31);
        assert_eq!((quad.packed[3] >> 8) & 511, 31);
        assert_eq!(quad.packed[3] >> 17, Material::Snow as u32);
    }

    #[test]
    fn terrain_contains_each_biome() {
        let world = World::new(42);
        let mut materials = [false; 4];
        for z in (0..WORLD_VOXELS).step_by(64) {
            for x in (0..WORLD_VOXELS).step_by(64) {
                let height = world.height_voxels(x as isize, z as isize);
                materials[surface_material(height) as usize] = true;
                if height + 1 < SEA_LEVEL_VOXELS {
                    materials[Material::Water as usize] = true;
                }
            }
        }
        assert!(materials.into_iter().all(|present| present));
    }
}
