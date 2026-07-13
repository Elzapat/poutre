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

const CORNERS = array<vec3<f32>, 8>(
    vec3<f32>(-0.3, -1.7, -0.2), vec3<f32>(0.3, -1.7, -0.2),
    vec3<f32>(-0.3, 0.0, -0.2), vec3<f32>(0.3, 0.0, -0.2),
    vec3<f32>(-0.3, -1.7, 0.2), vec3<f32>(0.3, -1.7, 0.2),
    vec3<f32>(-0.3, 0.0, 0.2), vec3<f32>(0.3, 0.0, 0.2),
);

const INDICES = array<u32, 36>(
    0u, 2u, 1u, 1u, 2u, 3u,
    5u, 7u, 4u, 4u, 7u, 6u,
    4u, 6u, 0u, 0u, 6u, 2u,
    1u, 3u, 5u, 5u, 3u, 7u,
    2u, 6u, 3u, 3u, 6u, 7u,
    4u, 0u, 5u, 5u, 0u, 1u,
);

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) player_position: vec4<f32>,
    @location(1) facing: vec4<f32>,
) -> VertexOutput {
    var local = CORNERS[INDICES[vertex_index]];
    let yaw_sin = sin(facing.x);
    let yaw_cos = cos(facing.x);
    local = vec3<f32>(
        local.x * yaw_cos - local.z * yaw_sin,
        local.y,
        local.x * yaw_sin + local.z * yaw_cos,
    );
    let world_position = player_position.xyz + local;

    let yaw = -uniforms.camera.z;
    let pitch = -uniforms.camera.w;
    let cy = cos(yaw);
    let sy = sin(yaw);
    let cx = cos(pitch);
    let sx = sin(pitch);
    let translated = world_position - uniforms.camera_position.xyz;
    let yawed = vec3<f32>(translated.x * cy + translated.z * sy, translated.y, -translated.x * sy + translated.z * cy);
    let rotated = vec3<f32>(yawed.x, yawed.y * cx - yawed.z * sx, yawed.y * sx + yawed.z * cx);
    let camera_depth = -rotated.z;
    let near = 0.03;
    let far = 1000.0;
    let depth = (camera_depth - near) / (far - near);

    var output: VertexOutput;
    output.position = vec4<f32>(
        rotated.x * uniforms.camera.y / uniforms.camera.x,
        rotated.y * uniforms.camera.y,
        depth * camera_depth,
        camera_depth,
    );
    output.normal = normalize(local);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let sun_direction = normalize(vec3<f32>(0.45, 0.85, 0.3));
    let light = 0.45 + max(dot(input.normal, sun_direction), 0.0) * 0.55;
    return vec4<f32>(vec3<f32>(0.95, 0.28, 0.18) * light, 1.0);
}
