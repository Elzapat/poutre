struct Uniforms {
    camera: vec4<f32>,
    camera_position: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) packed: vec4<u32>,
) -> VertexOutput {
    let face = packed.w & 7u;
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
    } else {
        position += vec3<f32>(0.0, corner.y, corner.x);
        normal = vec3<f32>(-1.0, 0.0, 0.0);
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
    output.normal = normal;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let sun_direction = normalize(vec3<f32>(0.45, 0.85, 0.3));
    let light = 0.35 + max(dot(input.normal, sun_direction), 0.0) * 0.65;
    let voxel_color = vec3<f32>(0.36, 0.62, 0.28);
    return vec4<f32>(voxel_color * light, 1.0);
}
