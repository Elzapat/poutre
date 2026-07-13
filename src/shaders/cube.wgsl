struct Uniforms {
    camera: vec4<f32>,
    camera_position: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) @interpolate(flat) material: u32,
    @location(2) normal: vec3<f32>,
    @location(3) grass_position: vec2<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) packed: vec4<u32>,
) -> VertexOutput {
    let face = packed.w & 7u;
    let material = packed.w >> 17u;
    let extent = vec2<f32>(
        f32(((packed.w >> 3u) & 31u) + 1u),
        f32(((packed.w >> 8u) & 511u) + 1u),
    );
    let corner = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    )[vertex_index] * extent;

    var position = vec3<f32>(f32(packed.x), f32(packed.y), f32(packed.z));
    var normal = vec3<f32>(0.0, 1.0, 0.0);
    if face == 0u {
        position += vec3<f32>(corner.x, 1.0, corner.y);
    } else if face == 1u {
        position += vec3<f32>(corner.x, corner.y, extent.x);
        normal = vec3<f32>(0.0, 0.0, 1.0);
    } else if face == 2u {
        position += vec3<f32>(corner.x, corner.y, 0.0);
        normal = vec3<f32>(0.0, 0.0, -1.0);
    } else if face == 3u {
        position += vec3<f32>(extent.x, corner.y, corner.x);
        normal = vec3<f32>(1.0, 0.0, 0.0);
    } else if face == 4u {
        position += vec3<f32>(0.0, corner.y, corner.x);
        normal = vec3<f32>(-1.0, 0.0, 0.0);
    } else {
        position += vec3<f32>(corner.x, 0.0, corner.y);
        normal = vec3<f32>(0.0, -1.0, 0.0);
    }

    let grass_position = position.xz * 0.1;
    if material == 8u {
        var bend = 1.0;
        if face >= 1u && face <= 4u {
            bend = corner.y / extent.y;
        }
        let phase = f32(packed.x) * 0.19 + f32(packed.z) * 0.13 + uniforms.camera_position.w * 1.7;
        let wind = vec2<f32>(sin(phase), cos(phase * 0.73)) * 0.55 * bend * bend;
        position.x += wind.x;
        position.z += wind.y;
    }
    position *= 0.1;

    let aspect = uniforms.camera.x;
    let focal_length = uniforms.camera.y;
    let yaw = -uniforms.camera.z;
    let pitch = -uniforms.camera.w;
    let cy = cos(yaw);
    let sy = sin(yaw);
    let cx = cos(pitch);
    let sx = sin(pitch);
    let translated = position - uniforms.camera_position.xyz;
    let yawed = vec3<f32>(translated.x * cy + translated.z * sy, translated.y, -translated.x * sy + translated.z * cy);
    let rotated = vec3<f32>(yawed.x, yawed.y * cx - yawed.z * sx, yawed.y * sx + yawed.z * cx);
    let camera_depth = -rotated.z;
    let near = 0.03;
    let far = 1000.0;
    let depth = (camera_depth - near) / (far - near);

    var output: VertexOutput;
    output.position = vec4<f32>(
        rotated.x * focal_length / aspect,
        rotated.y * focal_length,
        depth * camera_depth,
        camera_depth,
    );
    output.world_position = position;
    output.material = material;
    output.normal = normal;
    output.grass_position = grass_position;
    return output;
}

fn noise_hash(cell: vec2<f32>) -> f32 {
    return fract(sin(dot(cell, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise_gradient(cell: vec2<f32>) -> vec2<f32> {
    let angle = noise_hash(cell) * 6.2831853;
    return vec2<f32>(cos(angle), sin(angle));
}

fn gradient_noise(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let offset = fract(position);
    let blend = offset * offset * (vec2<f32>(3.0) - 2.0 * offset);
    let bottom_left = dot(noise_gradient(cell), offset);
    let bottom_right = dot(noise_gradient(cell + vec2<f32>(1.0, 0.0)), offset - vec2<f32>(1.0, 0.0));
    let top_left = dot(noise_gradient(cell + vec2<f32>(0.0, 1.0)), offset - vec2<f32>(0.0, 1.0));
    let top_right = dot(noise_gradient(cell + vec2<f32>(1.0, 1.0)), offset - vec2<f32>(1.0, 1.0));
    let bottom = mix(bottom_left, bottom_right, blend.x);
    let top = mix(top_left, top_right, blend.x);
    return clamp(mix(bottom, top, blend.y) * 0.7 + 0.5, 0.0, 1.0);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let broad_grass = gradient_noise(input.grass_position * 0.18);
    let grass_detail = gradient_noise(input.grass_position * 0.62 + vec2<f32>(19.7, -8.3));
    let grass_noise = broad_grass * 0.72 + grass_detail * 0.28;
    var voxel_color = mix(vec3<f32>(0.22, 0.49, 0.16), vec3<f32>(0.45, 0.70, 0.29), grass_noise);
    if input.material == 1u {
        voxel_color = vec3<f32>(0.42, 0.40, 0.37);
    } else if input.material == 2u {
        voxel_color = vec3<f32>(0.91, 0.94, 0.96);
    } else if input.material == 4u {
        voxel_color = vec3<f32>(0.94, 0.96, 0.98);
    } else if input.material == 6u {
        voxel_color = vec3<f32>(0.34, 0.33, 0.30);
    } else if input.material == 7u {
        voxel_color = vec3<f32>(0.36, 0.25, 0.16);
    }

    // The material color is shared by every face; broad ambient and sunlight provide
    // enough soft contrast to reveal the voxel shape without blackening side faces.
    let sun_direction = normalize(vec3<f32>(0.48, 0.82, 0.31));
    let diffuse = max(dot(input.normal, sun_direction), 0.0);
    let light = 0.72 + max(input.normal.y, 0.0) * 0.14 + diffuse * 0.18;

    let daylight = vec3<f32>(1.02, 1.0, 0.94);
    let distance = length(input.world_position - uniforms.camera_position.xyz);
    let fog = smoothstep(320.0, 760.0, distance);
    let sky = vec3<f32>(0.42, 0.68, 0.92);
    return vec4<f32>(mix(voxel_color * daylight * light, sky, fog), 1.0);
}
