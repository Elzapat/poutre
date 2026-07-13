use std::collections::HashMap;

use noise::{NoiseFn, Perlin};

use crate::validation::{CHUNK_SIZE, VOXEL_SIZE, WORLD_CHUNKS};

const CHUNK_SIZE_USIZE: usize = CHUNK_SIZE as usize;
const WORLD_VOXELS: usize = CHUNK_SIZE_USIZE * WORLD_CHUNKS as usize;
const WORLD_SIZE: f32 = WORLD_VOXELS as f32 * VOXEL_SIZE;

const WORLD_HEIGHT_VOXELS: usize = 480;
const SEA_LEVEL_VOXELS: u32 = 140;
const ROCK_LEVEL_VOXELS: u32 = 260;
const SNOW_LEVEL_VOXELS: u32 = 340;
const GRASS_AND_SNOW_DEPTH: u32 = 3;
pub(crate) const EXCAVATION_RADIUS_VOXELS: i64 = 8;
pub(crate) const TREE_HORIZONTAL_EXTENT: u32 = 28;

const TREE_FREQUENCY: u32 = 4096;
const TREE_HASH_VALUE: u32 = 17;

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
    TallGrass = 8,
    Wood = 9,
    Leaves = 10,
    Bush = 11,
    FlowerRed = 12,
    FlowerYellow = 13,
    Stem = 14,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Quad {
    pub packed: [u32; 4],
}

pub(crate) struct GeneratedChunk {
    pub quads: Vec<Quad>,
    pub water_quads: Vec<Quad>,
    pub heights: Vec<u16>,
}

#[derive(Clone, Copy)]
pub(crate) struct ExcavationSphere {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

pub(crate) struct WorldGenerator {
    seed: u32,
    broad: Perlin,
    detail: Perlin,
    mountains: Perlin,
    excavations: Vec<ExcavationSphere>,
}

impl WorldGenerator {
    fn new(seed: u32) -> Self {
        Self {
            seed,
            broad: Perlin::new(seed),
            detail: Perlin::new(seed.wrapping_add(1)),
            mountains: Perlin::new(seed.wrapping_add(2)),
            excavations: Vec::new(),
        }
    }

    pub(crate) fn with_excavations(seed: u32, excavations: Vec<ExcavationSphere>) -> Self {
        Self {
            excavations,
            ..Self::new(seed)
        }
    }

    pub(crate) fn generate_patch(&self, chunk_x: u32, chunk_z: u32, lod: usize) -> GeneratedChunk {
        let mut patch = GeneratedChunk {
            quads: Vec::new(),
            water_quads: Vec::new(),
            heights: Vec::new(),
        };
        let end_x = (chunk_x + lod as u32).min(WORLD_CHUNKS);
        let end_z = (chunk_z + lod as u32).min(WORLD_CHUNKS);
        for z in chunk_z..end_z {
            for x in chunk_x..end_x {
                let mut chunk = self.generate_chunk(x, z, lod);
                patch.quads.append(&mut chunk.quads);
                patch.water_quads.append(&mut chunk.water_quads);
                if lod == 1 {
                    patch.heights = chunk.heights;
                }
            }
        }
        patch
    }

    fn generate_chunk(&self, chunk_x: u32, chunk_z: u32, lod: usize) -> GeneratedChunk {
        let base_x = chunk_x as isize * CHUNK_SIZE as isize;
        let base_z = chunk_z as isize * CHUNK_SIZE as isize;
        let samples = CHUNK_SIZE_USIZE / lod;
        let stride = samples + 2;
        let mut heights = vec![0_u32; stride * stride];
        let mut full_heights = Vec::new();
        if lod == 1 {
            full_heights.reserve(CHUNK_SIZE_USIZE * CHUNK_SIZE_USIZE);
            for z in 0..CHUNK_SIZE_USIZE {
                for x in 0..CHUNK_SIZE_USIZE {
                    full_heights.push(
                        self.effective_height_voxels(base_x + x as isize, base_z + z as isize)
                            as u16,
                    );
                }
            }
        }
        let mut quads = Vec::new();
        let mut water_quads = Vec::new();
        let mut vegetation = HashMap::new();
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

                if lod == 1 && self.column_touched(world_x, height, world_z) {
                    self.mesh_excavated_column(world_x, height, world_z, material, &mut quads);
                    if height + 1 < SEA_LEVEL_VOXELS {
                        water_quads.push(pack_quad(
                            world_x,
                            SEA_LEVEL_VOXELS - 1,
                            world_z,
                            Face::Up,
                            1,
                            1,
                            Material::Water,
                        ));
                    }
                    continue;
                }

                quads.push(pack_quad(
                    world_x,
                    height,
                    world_z,
                    Face::Up,
                    lod,
                    lod,
                    material,
                ));

                if lod == 1 && material == Material::Grass {
                    let neighbor_heights = [
                        heights[x + 1 + stride * (z + 2)],
                        heights[x + 1 + stride * z],
                        heights[x + 2 + stride * (z + 1)],
                        heights[x + stride * (z + 1)],
                    ];
                    let slope = neighbor_heights
                        .into_iter()
                        .map(|neighbor| height.abs_diff(neighbor))
                        .max()
                        .unwrap_or(0);
                    self.generate_vegetation(
                        world_x,
                        height,
                        world_z,
                        slope,
                        &mut vegetation,
                        &mut quads,
                    );
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
                        let surface_bottom = top.saturating_sub(surface_depth(material));
                        while bottom < top {
                            let (extent, material) = if bottom >= surface_bottom {
                                (top - bottom, material)
                            } else {
                                let section_top = ((bottom / CHUNK_SIZE) + 1) * CHUNK_SIZE;
                                (surface_bottom.min(section_top) - bottom, Material::Rock)
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
        vegetation.retain(|&(x, y, z), _| !self.is_excavated(x as isize, y as i64, z as isize));
        mesh_vegetation_voxels(&vegetation, &mut quads);
        self.mesh_cloud_for_chunk(chunk_x, chunk_z, &mut quads);
        GeneratedChunk {
            quads,
            water_quads,
            heights: full_heights,
        }
    }

    fn mesh_cloud_for_chunk(&self, chunk_x: u32, chunk_z: u32, quads: &mut Vec<Quad>) {
        const CLOUD_CELL_CHUNKS: isize = 8;
        if !chunk_x.is_multiple_of(CLOUD_CELL_CHUNKS as u32)
            || !chunk_z.is_multiple_of(CLOUD_CELL_CHUNKS as u32)
        {
            return;
        }
        let cell_x = chunk_x as isize / CLOUD_CELL_CHUNKS;
        let cell_z = chunk_z as isize / CLOUD_CELL_CHUNKS;
        let hash = cloud_hash(cell_x as u32, cell_z as u32);
        if hash % 5 > 1 {
            return;
        }
        let base_x = cell_x * CLOUD_CELL_CHUNKS * CHUNK_SIZE as isize;
        let base_z = cell_z * CLOUD_CELL_CHUNKS * CHUNK_SIZE as isize;
        let x = base_x + ((hash >> 8) % 128) as isize;
        let z = base_z + ((hash >> 16) % 128) as isize;
        let y = 500 + ((hash >> 24) % 80) as isize;
        let size = 24 + ((hash >> 4) % 9) as usize;

        push_cloud_box(quads, x, y, z, size, 5);
        for (offset, y_offset, z_offset) in [(-2, 0, 1), (-1, 2, -1), (1, 1, 1), (2, 0, 0)] {
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

    fn effective_height_voxels(&self, x: isize, z: isize) -> u32 {
        let mut height = self.height_voxels(x, z);
        while height > 0 && self.is_excavated(x, height as i64, z) {
            height -= 1;
        }
        height
    }

    fn column_touched(&self, x: u32, height: u32, z: u32) -> bool {
        let mesh_radius = EXCAVATION_RADIUS_VOXELS + 1;
        let radius_squared = mesh_radius * mesh_radius;
        self.excavations.iter().any(|sphere| {
            let dx = x as i64 - sphere.x as i64;
            let dz = z as i64 - sphere.z as i64;
            let horizontal_squared = dx * dx + dz * dz;
            horizontal_squared <= radius_squared
                && sphere.y.saturating_sub(EXCAVATION_RADIUS_VOXELS as u32) <= height
        })
    }

    fn is_excavated(&self, x: isize, y: i64, z: isize) -> bool {
        let radius_squared = EXCAVATION_RADIUS_VOXELS * EXCAVATION_RADIUS_VOXELS;
        self.excavations.iter().any(|sphere| {
            let dx = x as i64 - sphere.x as i64;
            let dy = y - sphere.y as i64;
            let dz = z as i64 - sphere.z as i64;
            dx * dx + dy * dy + dz * dz <= radius_squared
        })
    }

    pub(crate) fn is_solid(&self, x: isize, y: i64, z: isize) -> bool {
        if y < 0 || x < 0 || z < 0 || x >= WORLD_VOXELS as isize || z >= WORLD_VOXELS as isize {
            return false;
        }
        if self.is_excavated(x, y, z) {
            return false;
        }
        y <= self.height_voxels(x, z) as i64 || self.tree_material_at(x, y as u32, z).is_some()
    }

    fn is_terrain_solid(&self, x: isize, y: i64, z: isize) -> bool {
        y >= 0
            && x >= 0
            && z >= 0
            && x < WORLD_VOXELS as isize
            && z < WORLD_VOXELS as isize
            && y <= self.height_voxels(x, z) as i64
            && !self.is_excavated(x, y, z)
    }

    fn tree_material_at(&self, x: isize, y: u32, z: isize) -> Option<Material> {
        let extent = TREE_HORIZONTAL_EXTENT as isize;
        let min_x = (x - extent).max(0);
        let min_z = (z - extent).max(0);
        let max_x = (x + extent).min(WORLD_VOXELS as isize - 1);
        let max_z = (z + extent).min(WORLD_VOXELS as isize - 1);
        for root_z in min_z..=max_z {
            for root_x in min_x..=max_x {
                let Some((base_y, hash)) = self.tree_root(root_x as u32, root_z as u32) else {
                    continue;
                };
                let mut voxels = VegetationVoxels::new();
                generate_tree(
                    self.seed,
                    root_x as u32,
                    base_y,
                    root_z as u32,
                    hash,
                    &mut voxels,
                );
                if let Some(material) = voxels.get(&(x as u32, y, z as u32)) {
                    return Some(*material);
                }
            }
        }
        None
    }

    fn tree_root(&self, x: u32, z: u32) -> Option<(u32, u32)> {
        let hash = vegetation_hash(self.seed, x, z);
        if hash % TREE_FREQUENCY != TREE_HASH_VALUE {
            return None;
        }
        let ground_y = self.height_voxels(x as isize, z as isize);
        if terrain_material(x, ground_y, z) != Material::Grass {
            return None;
        }
        let slope = [
            self.height_voxels(x as isize, z as isize + 1),
            self.height_voxels(x as isize, z as isize - 1),
            self.height_voxels(x as isize + 1, z as isize),
            self.height_voxels(x as isize - 1, z as isize),
        ]
        .into_iter()
        .map(|neighbor| ground_y.abs_diff(neighbor))
        .max()
        .unwrap_or(0);
        (slope <= 2).then_some((ground_y + 1, hash))
    }

    fn mesh_excavated_column(
        &self,
        x: u32,
        height: u32,
        z: u32,
        surface_material: Material,
        quads: &mut Vec<Quad>,
    ) {
        for y in 0..=height {
            if !self.is_terrain_solid(x as isize, y as i64, z as isize) {
                continue;
            }
            let material = if y + surface_depth(surface_material) > height {
                surface_material
            } else {
                Material::Rock
            };
            for (dx, dy, dz, face) in [
                (0, 1, 0, Face::Up),
                (0, 0, 1, Face::Front),
                (0, 0, -1, Face::Back),
                (1, 0, 0, Face::Right),
                (-1, 0, 0, Face::Left),
                (0, -1, 0, Face::Down),
            ] {
                if y == 0 && dy == -1 {
                    continue;
                }
                if !self.is_terrain_solid(x as isize + dx, y as i64 + dy, z as isize + dz) {
                    quads.push(pack_quad(x, y, z, face, 1, 1, material));
                }
            }
        }
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

fn vegetation_hash(seed: u32, x: u32, z: u32) -> u32 {
    terrain_hash(
        x ^ seed.wrapping_mul(0x9e37_79b1),
        z ^ seed.rotate_left(16).wrapping_mul(0x85eb_ca77),
    )
}

fn vegetation_hash_3d(seed: u32, x: isize, y: isize, z: isize) -> u32 {
    let horizontal = vegetation_hash(
        seed ^ (y as u32).wrapping_mul(0x27d4_eb2d),
        x as u32,
        z as u32,
    );
    horizontal ^ horizontal.rotate_left(13)
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

type VegetationVoxels = HashMap<(u32, u32, u32), Material>;

impl WorldGenerator {
    fn generate_vegetation(
        &self,
        x: u32,
        ground_y: u32,
        z: u32,
        slope: u32,
        voxels: &mut VegetationVoxels,
        quads: &mut Vec<Quad>,
    ) {
        let hash = vegetation_hash(self.seed, x, z);
        let base_y = ground_y + 1;

        if hash % TREE_FREQUENCY == TREE_HASH_VALUE && slope <= 2 {
            generate_tree(self.seed, x, base_y, z, hash, voxels);
        } else if hash % 512 == 33 && slope <= 2 {
            generate_bush(self.seed, x, base_y, z, hash, voxels);
        } else if hash % 128 == 7 && slope <= 3 {
            generate_flower(x, base_y, z, hash, voxels);
        } else if hash.is_multiple_of(32) {
            push_tall_grass(x, base_y, z, 3 + ((hash >> 8) & 1) as usize, quads);
        }
    }
}

fn generate_tree(seed: u32, x: u32, base_y: u32, z: u32, hash: u32, voxels: &mut VegetationVoxels) {
    let trunk_height = 64 + ((hash >> 12) % 24);
    for dy in 0..trunk_height {
        let radius = if dy < 10 {
            4
        } else if dy < trunk_height / 2 {
            3
        } else {
            2
        };
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz <= radius * radius + 1 {
                    insert_vegetation_voxel(
                        voxels,
                        x as isize + dx as isize,
                        base_y + dy,
                        z as isize + dz as isize,
                        Material::Wood,
                    );
                }
            }
        }
    }

    let directions = [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ];
    for branch in 0..8_u32 {
        let direction_index = (((hash >> (branch * 2)) & 3) + branch) as usize % directions.len();
        let direction = directions[direction_index];
        let branch_y = base_y + trunk_height - 12 - (branch % 4) * 7;
        let length = 10 + ((hash.rotate_right(branch * 3) >> 16) & 7) as isize;
        let mut end = (x as isize, branch_y, z as isize);
        for step in 1..=length {
            end = (
                x as isize + direction.0 * step,
                branch_y + (step / 3) as u32,
                z as isize + direction.1 * step,
            );
            let radius = if step <= length / 3 { 2 } else { 1 };
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dz * dz <= radius * radius + 1 {
                        insert_vegetation_voxel(
                            voxels,
                            end.0 + dx,
                            end.1,
                            end.2 + dz,
                            Material::Wood,
                        );
                    }
                }
            }
        }
        add_leaf_cluster(seed, end.0, end.1, end.2, 10, 7, 10, voxels);
    }

    for (index, (direction_x, direction_z)) in directions.into_iter().step_by(2).enumerate() {
        let root_length = 7 + ((hash.rotate_right(index as u32 * 5) >> 24) & 3);
        for step in 1..=root_length as isize {
            insert_vegetation_voxel(
                voxels,
                x as isize + direction_x * step,
                base_y,
                z as isize + direction_z * step,
                Material::Wood,
            );
        }
    }

    add_leaf_cluster(
        seed,
        x as isize,
        base_y + trunk_height,
        z as isize,
        15,
        11,
        15,
        voxels,
    );
}

fn generate_bush(seed: u32, x: u32, base_y: u32, z: u32, hash: u32, voxels: &mut VegetationVoxels) {
    let height = 5 + ((hash >> 9) & 3);
    for dy in 0..height {
        insert_vegetation_voxel(voxels, x as isize, base_y + dy, z as isize, Material::Wood);
    }
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        for step in 1..=3 {
            insert_vegetation_voxel(
                voxels,
                x as isize + dx * step,
                base_y + 2 + step as u32 / 2,
                z as isize + dz * step,
                Material::Wood,
            );
        }
    }
    add_bush_cluster(
        seed,
        x as isize,
        base_y + 4,
        z as isize,
        5 + ((hash >> 15) & 1) as isize,
        4,
        voxels,
    );
}

fn generate_flower(x: u32, base_y: u32, z: u32, hash: u32, voxels: &mut VegetationVoxels) {
    let stem_height = 5 + ((hash >> 8) & 3);
    for dy in 0..stem_height {
        insert_vegetation_voxel(voxels, x as isize, base_y + dy, z as isize, Material::Stem);
    }

    let leaf_direction = if hash & 0x400 == 0 { 1 } else { -1 };
    insert_vegetation_voxel(
        voxels,
        x as isize + leaf_direction,
        base_y + 2,
        z as isize,
        Material::Stem,
    );
    insert_vegetation_voxel(
        voxels,
        x as isize,
        base_y + 3,
        z as isize - leaf_direction,
        Material::Stem,
    );

    let flower_y = base_y + stem_height;
    let petals = if hash & 0x1000 == 0 {
        Material::FlowerRed
    } else {
        Material::FlowerYellow
    };
    let center = if petals == Material::FlowerRed {
        Material::FlowerYellow
    } else {
        Material::Wood
    };
    insert_vegetation_voxel(voxels, x as isize, flower_y, z as isize, center);
    for (dx, dz) in [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ] {
        insert_vegetation_voxel(voxels, x as isize + dx, flower_y, z as isize + dz, petals);
    }
}

#[allow(clippy::too_many_arguments)]
fn add_leaf_cluster(
    seed: u32,
    center_x: isize,
    center_y: u32,
    center_z: isize,
    radius_x: isize,
    radius_y: isize,
    radius_z: isize,
    voxels: &mut VegetationVoxels,
) {
    for dy in -radius_y..=radius_y {
        for dz in -radius_z..=radius_z {
            for dx in -radius_x..=radius_x {
                let distance = (dx as f32 / radius_x as f32).powi(2)
                    + (dy as f32 / radius_y as f32).powi(2)
                    + (dz as f32 / radius_z as f32).powi(2);
                if distance > 1.0
                    || (distance > 0.68
                        && vegetation_hash_3d(
                            seed,
                            center_x + dx,
                            center_y as isize + dy,
                            center_z + dz,
                        )
                        .is_multiple_of(7))
                {
                    continue;
                }
                insert_vegetation_voxel_if_empty(
                    voxels,
                    center_x + dx,
                    (center_y as isize + dy) as u32,
                    center_z + dz,
                    Material::Leaves,
                );
            }
        }
    }
}

fn add_bush_cluster(
    seed: u32,
    center_x: isize,
    center_y: u32,
    center_z: isize,
    radius: isize,
    radius_y: isize,
    voxels: &mut VegetationVoxels,
) {
    for dy in -radius_y..=radius_y {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let distance = (dx as f32 / radius as f32).powi(2)
                    + (dy as f32 / radius_y as f32).powi(2)
                    + (dz as f32 / radius as f32).powi(2);
                if distance > 1.0
                    || (distance > 0.6
                        && vegetation_hash_3d(
                            seed,
                            center_x + dx,
                            center_y as isize + dy,
                            center_z + dz,
                        )
                        .is_multiple_of(5))
                {
                    continue;
                }
                insert_vegetation_voxel_if_empty(
                    voxels,
                    center_x + dx,
                    (center_y as isize + dy) as u32,
                    center_z + dz,
                    Material::Bush,
                );
            }
        }
    }
}

fn insert_vegetation_voxel(
    voxels: &mut VegetationVoxels,
    x: isize,
    y: u32,
    z: isize,
    material: Material,
) {
    if x >= 0 && z >= 0 && x < WORLD_VOXELS as isize && z < WORLD_VOXELS as isize {
        voxels.insert((x as u32, y, z as u32), material);
    }
}

fn insert_vegetation_voxel_if_empty(
    voxels: &mut VegetationVoxels,
    x: isize,
    y: u32,
    z: isize,
    material: Material,
) {
    if x >= 0 && z >= 0 && x < WORLD_VOXELS as isize && z < WORLD_VOXELS as isize {
        voxels.entry((x as u32, y, z as u32)).or_insert(material);
    }
}

fn mesh_vegetation_voxels(voxels: &VegetationVoxels, quads: &mut Vec<Quad>) {
    for (&(x, y, z), &material) in voxels {
        for (dx, dy, dz, face) in [
            (0, 1, 0, Face::Up),
            (0, 0, 1, Face::Front),
            (0, 0, -1, Face::Back),
            (1, 0, 0, Face::Right),
            (-1, 0, 0, Face::Left),
            (0, -1, 0, Face::Down),
        ] {
            let neighbor = (x as i64 + dx, y as i64 + dy, z as i64 + dz);
            let covered = neighbor.0 >= 0
                && neighbor.1 >= 0
                && neighbor.2 >= 0
                && voxels.contains_key(&(neighbor.0 as u32, neighbor.1 as u32, neighbor.2 as u32));
            if !covered {
                quads.push(pack_quad(x, y, z, face, 1, 1, material));
            }
        }
    }
}

fn push_tall_grass(x: u32, y: u32, z: u32, height: usize, quads: &mut Vec<Quad>) {
    for face in [Face::Front, Face::Back, Face::Right, Face::Left] {
        quads.push(pack_quad(x, y, z, face, 1, height, Material::TallGrass));
    }
    quads.push(pack_quad(
        x,
        y + height as u32 - 1,
        z,
        Face::Up,
        1,
        1,
        Material::TallGrass,
    ));
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

fn surface_depth(material: Material) -> u32 {
    if matches!(material, Material::Grass | Material::Snow) {
        GRASS_AND_SNOW_DEPTH
    } else {
        1
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
    debug_assert!(width <= CHUNK_SIZE_USIZE && height <= 512);
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn find_tree_root(world: &WorldGenerator) -> (u32, u32) {
        for z in 9_000..10_200_u32 {
            for x in 9_000..10_200_u32 {
                if world.tree_root(x, z).is_some() {
                    return (x, z);
                }
            }
        }
        panic!("seed should place a tree in the search area");
    }

    #[test]
    fn world_is_ten_times_wider_with_tenth_size_voxels() {
        assert_eq!(WORLD_VOXELS, 19_200);
        assert_eq!(WORLD_SIZE, 1_920.0);
    }

    #[test]
    fn streamed_mesh_contains_small_single_voxel_tops() {
        let world = WorldGenerator::new(42);
        let mesh = world.generate_patch(300, 300, 1);
        assert_eq!(mesh.heights.len(), CHUNK_SIZE_USIZE * CHUNK_SIZE_USIZE);
        assert!(mesh.quads.len() >= CHUNK_SIZE_USIZE * CHUNK_SIZE_USIZE);
        assert!(
            mesh.quads
                .iter()
                .all(|quad| quad.packed[0] < WORLD_VOXELS as u32)
        );
    }

    #[test]
    fn terrain_height_is_voxel_aligned() {
        let world = WorldGenerator::new(42);
        let height = world.height_voxels(9_600, 9_600);
        assert!(height > 0);
    }

    #[test]
    fn chunk_dimensions_and_packing_support_32_voxels() {
        assert_eq!(CHUNK_SIZE_USIZE, 32);
        let quad = pack_quad(
            0,
            0,
            0,
            Face::Up,
            CHUNK_SIZE_USIZE,
            CHUNK_SIZE_USIZE,
            Material::Snow,
        );
        assert_eq!((quad.packed[3] >> 3) & 31, 31);
        assert_eq!((quad.packed[3] >> 8) & 511, 31);
        assert_eq!(quad.packed[3] >> 17, Material::Snow as u32);
    }

    #[test]
    fn terrain_contains_each_biome() {
        let world = WorldGenerator::new(42);
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

    #[test]
    fn grass_and_snow_layers_are_three_voxels_deep() {
        assert_eq!(surface_depth(Material::Grass), 3);
        assert_eq!(surface_depth(Material::Snow), 3);
        assert_eq!(surface_depth(Material::Rock), 1);
    }

    #[test]
    fn procedural_vegetation_generates_every_voxel_material() {
        let world = WorldGenerator::new(42);
        let mut voxels = VegetationVoxels::new();
        let mut quads = Vec::new();
        for z in 100..300 {
            for x in 100..300 {
                world.generate_vegetation(x, SEA_LEVEL_VOXELS, z, 0, &mut voxels, &mut quads);
            }
        }
        mesh_vegetation_voxels(&voxels, &mut quads);

        for material in [
            Material::TallGrass,
            Material::Wood,
            Material::Leaves,
            Material::Bush,
            Material::FlowerRed,
            Material::FlowerYellow,
            Material::Stem,
        ] {
            assert!(
                quads
                    .iter()
                    .any(|quad| quad.packed[3] >> 17 == material as u32),
                "missing {material:?} geometry"
            );
        }

        assert!(
            quads
                .iter()
                .filter(|quad| quad.packed[3] >> 17 == Material::Leaves as u32)
                .count()
                > 100
        );
        assert!(
            quads
                .iter()
                .filter(|quad| quad.packed[3] >> 17 == Material::FlowerRed as u32)
                .count()
                > 8
        );
        assert!(
            quads
                .iter()
                .filter(|quad| quad.packed[3] >> 17 == Material::FlowerYellow as u32)
                .count()
                > 8
        );
        assert!(
            quads
                .iter()
                .filter(|quad| quad.packed[3] >> 17 >= Material::Wood as u32)
                .all(|quad| {
                    ((quad.packed[3] >> 3) & 31) == 0 && ((quad.packed[3] >> 8) & 511) == 0
                })
        );
    }

    #[test]
    fn generated_world_chunks_include_trees_and_flowers() {
        let world = WorldGenerator::new(42);
        let (tree_x, tree_z) = find_tree_root(&world);
        let mut flower_root = None;

        'search: for z in 9_000..10_200_u32 {
            for x in 9_000..10_200_u32 {
                let height = world.height_voxels(x as isize, z as isize);
                if terrain_material(x, height, z) != Material::Grass {
                    continue;
                }
                let slope = [
                    world.height_voxels(x as isize, z as isize + 1),
                    world.height_voxels(x as isize, z as isize - 1),
                    world.height_voxels(x as isize + 1, z as isize),
                    world.height_voxels(x as isize - 1, z as isize),
                ]
                .into_iter()
                .map(|neighbor| height.abs_diff(neighbor))
                .max()
                .unwrap_or(0);
                let hash = vegetation_hash(world.seed, x, z);
                if hash % 128 == 7 && slope <= 3 {
                    flower_root = Some((x, z));
                }
                if flower_root.is_some() {
                    break 'search;
                }
            }
        }

        let (tree_base_y, _) = world.tree_root(tree_x, tree_z).unwrap();
        let tree_chunk = world.generate_patch(tree_x / CHUNK_SIZE, tree_z / CHUNK_SIZE, 1);
        assert!(tree_chunk.quads.iter().any(|quad| {
            matches!(
                quad.packed[3] >> 17,
                material if material == Material::Wood as u32 || material == Material::Leaves as u32
            )
        }));
        assert!(
            tree_chunk
                .quads
                .iter()
                .any(|quad| quad.packed[1] >= tree_base_y + 60)
        );

        let (flower_x, flower_z) = flower_root.expect("seed should place a flower on grass");
        let flower_chunk = world.generate_patch(flower_x / CHUNK_SIZE, flower_z / CHUNK_SIZE, 1);
        assert!(flower_chunk.quads.iter().any(|quad| {
            matches!(
                quad.packed[3] >> 17,
                material if material == Material::FlowerRed as u32
                    || material == Material::FlowerYellow as u32
            )
        }));
        assert!(
            flower_chunk
                .quads
                .iter()
                .any(|quad| quad.packed[3] >> 17 == Material::Stem as u32)
        );
    }

    #[test]
    fn vegetation_placement_is_seeded() {
        assert_eq!(vegetation_hash(42, 100, 200), vegetation_hash(42, 100, 200));
        assert_ne!(vegetation_hash(42, 100, 200), vegetation_hash(43, 100, 200));
    }

    #[test]
    fn tree_voxels_are_authoritative_and_excavatable() {
        let world = WorldGenerator::new(42);
        let (tree_x, tree_z) = find_tree_root(&world);
        let (base_y, _) = world.tree_root(tree_x, tree_z).unwrap();
        let target = (tree_x + 3, base_y + 20, tree_z);

        assert_eq!(
            world.tree_material_at(target.0 as isize, target.1, target.2 as isize),
            Some(Material::Wood)
        );
        assert!(world.is_solid(target.0 as isize, target.1 as i64, target.2 as isize));
        let original = world.generate_patch(tree_x / CHUNK_SIZE, tree_z / CHUNK_SIZE, 1);
        assert!(original.quads.iter().any(|quad| {
            quad.packed[0] == target.0
                && quad.packed[1] == target.1
                && quad.packed[2] == target.2
                && quad.packed[3] >> 17 == Material::Wood as u32
        }));

        let excavated = WorldGenerator::with_excavations(
            42,
            vec![ExcavationSphere {
                x: target.0,
                y: target.1,
                z: target.2,
            }],
        );
        assert!(!excavated.is_solid(target.0 as isize, target.1 as i64, target.2 as isize));
        let modified = excavated.generate_patch(tree_x / CHUNK_SIZE, tree_z / CHUNK_SIZE, 1);
        assert!(!modified.quads.iter().any(|quad| {
            quad.packed[0] == target.0
                && quad.packed[1] == target.1
                && quad.packed[2] == target.2
                && quad.packed[3] >> 17 == Material::Wood as u32
        }));
    }

    #[test]
    fn excavation_lowers_the_surface_and_exposes_cavity_faces() {
        let unmodified = WorldGenerator::new(42);
        let x = 9_600;
        let z = 9_600;
        let surface = unmodified.height_voxels(x, z);
        let excavated = WorldGenerator::with_excavations(
            42,
            vec![ExcavationSphere {
                x: x as u32,
                y: surface,
                z: z as u32,
            }],
        );
        let chunk = excavated.generate_patch(x as u32 / CHUNK_SIZE, z as u32 / CHUNK_SIZE, 1);
        let local =
            x as usize % CHUNK_SIZE_USIZE + (z as usize % CHUNK_SIZE_USIZE) * CHUNK_SIZE_USIZE;

        assert!(chunk.heights[local] < surface as u16);
        assert!(chunk.quads.iter().any(|quad| {
            quad.packed[0] == x as u32 && quad.packed[2] == z as u32 && quad.packed[1] < surface
        }));
    }

    #[test]
    fn excavation_meshes_the_solid_halo_around_the_cavity() {
        let unmodified = WorldGenerator::new(42);
        let x = 9_600;
        let z = 9_600;
        let y = unmodified.height_voxels(x + 9, z).saturating_sub(20);
        let excavated = WorldGenerator::with_excavations(
            42,
            vec![ExcavationSphere {
                x: x as u32,
                y,
                z: z as u32,
            }],
        );
        let chunk = excavated.generate_patch(x as u32 / CHUNK_SIZE, z as u32 / CHUNK_SIZE, 1);

        assert!(chunk.quads.iter().any(|quad| {
            quad.packed[0] == x as u32 + EXCAVATION_RADIUS_VOXELS as u32 + 1
                && quad.packed[1] == y
                && quad.packed[2] == z as u32
                && quad.packed[3] & 7 == Face::Left as u32
        }));
    }
}
