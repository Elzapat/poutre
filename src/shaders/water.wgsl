struct Uniforms {
    camera: vec4<f32>,
    camera_position: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(1) @binding(0)
var scene_color: texture_2d<f32>;

@group(1) @binding(1)
var scene_depth: texture_depth_2d;

@group(1) @binding(2)
var scene_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) @interpolate(flat) material: u32,
};

fn ripple_phase(position: vec2<f32>) -> f32 {
    return length(position - vec2<f32>(960.0)) * 7.0 - uniforms.camera_position.w * 1.2;
}

fn ripple_normal(position: vec2<f32>) -> vec3<f32> {
    let offset = position - vec2<f32>(960.0);
    let radial_direction = offset / max(length(offset), 0.001);
    let slope = cos(ripple_phase(position)) * 0.014;
    return normalize(vec3<f32>(-radial_direction.x * slope, 1.0, -radial_direction.y * slope));
}

fn world_to_view(position: vec3<f32>) -> vec3<f32> {
    let yaw = -uniforms.camera.z;
    let pitch = -uniforms.camera.w;
    let cy = cos(yaw);
    let sy = sin(yaw);
    let cx = cos(pitch);
    let sx = sin(pitch);
    let translated = position - uniforms.camera_position.xyz;
    let yawed = vec3<f32>(translated.x * cy + translated.z * sy, translated.y, -translated.x * sy + translated.z * cy);
    return vec3<f32>(yawed.x, yawed.y * cx - yawed.z * sx, yawed.y * sx + yawed.z * cx);
}

fn project_to_uv(position: vec3<f32>) -> vec3<f32> {
    let view = world_to_view(position);
    let camera_depth = -view.z;
    let ndc = vec2<f32>(
        view.x * uniforms.camera.y / (uniforms.camera.x * camera_depth),
        view.y * uniforms.camera.y / camera_depth,
    );
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, camera_depth);
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) packed: vec4<u32>,
) -> VertexOutput {
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

    var world_position = vec3<f32>(
        f32(packed.x) + corner.x,
        f32(packed.y) + 1.0,
        f32(packed.z) + corner.y,
    ) * 0.1;
    let material = packed.w >> 17u;
    if material == 5u {
        world_position.y += 0.006;
    } else {
        world_position.y += sin(ripple_phase(world_position.xz)) * 0.002;
    }

    let rotated = world_to_view(world_position);
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
    output.world_position = world_position;
    output.material = material;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(scene_depth);
    let pixel = clamp(vec2<i32>(input.position.xy), vec2<i32>(0), vec2<i32>(dimensions) - vec2<i32>(1));
    let opaque_depth = textureLoad(scene_depth, pixel, 0);
    if input.position.z >= opaque_depth {
        discard;
    }

    if input.material == 5u {
        return vec4<f32>(0.82, 0.93, 0.97, 0.74);
    }

    let normal = ripple_normal(input.world_position.xz);
    let incident = normalize(input.world_position - uniforms.camera_position.xyz);
    let view_direction = -incident;
    let reflection_direction = normalize(reflect(incident, normal));
    let fresnel = pow(1.0 - max(dot(view_direction, normal), 0.0), 3.0);
    let deep_water = vec3<f32>(0.006, 0.035, 0.095);
    let sky_reflection = vec3<f32>(0.035, 0.11, 0.22);

    var reflected_color = sky_reflection;
    var reflection_hit = 0.0;
    var previous_delta = -1000.0;
    for (var index = 0; index < 48; index += 1) {
        let progress = f32(index + 1) / 48.0;
        let ray_distance = 0.06 + progress * progress * 240.0;
        let projected = project_to_uv(input.world_position + normal * 0.025 + reflection_direction * ray_distance);
        if projected.z <= 0.03 || any(projected.xy <= vec2<f32>(0.002)) || any(projected.xy >= vec2<f32>(0.998)) {
            break;
        }

        let ray_pixel = clamp(vec2<i32>(projected.xy * vec2<f32>(dimensions)), vec2<i32>(0), vec2<i32>(dimensions) - vec2<i32>(1));
        let sampled_depth = textureLoad(scene_depth, ray_pixel, 0);
        let sampled_camera_depth = 0.03 + sampled_depth * (1000.0 - 0.03);
        let delta = projected.z - sampled_camera_depth;
        if sampled_depth < 0.9999 && delta >= 0.0 && previous_delta < 0.0 {
            reflected_color = textureSampleLevel(scene_color, scene_sampler, projected.xy, 0.0).rgb
                * vec3<f32>(0.55, 0.7, 0.88);
            reflection_hit = 1.0;
            break;
        }
        previous_delta = delta;
    }

    let mirror_strength = 0.82 + fresnel * 0.16;
    var color = mix(deep_water, reflected_color, mirror_strength);
    color += reflection_hit * vec3<f32>(0.015, 0.02, 0.025);

    let distance = length(input.world_position - uniforms.camera_position.xyz);
    color = mix(color, vec3<f32>(0.12, 0.28, 0.48), smoothstep(320.0, 760.0, distance));
    let alpha = 0.7 + fresnel * 0.12;
    return vec4<f32>(color, alpha);
}
