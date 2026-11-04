struct MeshUniforms {
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    base_color: vec4<f32>,
    edge_color: vec4<f32>,
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> mesh: MeshUniforms;

struct MeshInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) color: vec4<f32>,
}

struct MeshOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) view_position: vec3<f32>,
    @location(1) view_normal: vec3<f32>,
    @location(2) albedo: vec3<f32>,
    @location(3) barycentric: vec3<f32>,
}

fn linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((color + 0.055) / 1.055, vec3<f32>(2.4)), color / 12.92, color <= vec3<f32>(0.04045));
}

fn encoded(value: vec3<f32>) -> vec3<f32> {
    let color = clamp(value, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(color, vec3<f32>(1.0 / 2.4)) - 0.055, color * 12.92, color <= vec3<f32>(0.0031308));
}

@vertex
fn mesh_vertex(input: MeshInput, @builtin(vertex_index) vertex_id: u32) -> MeshOutput {
    let position = mesh.view * vec4<f32>(input.position, 1.0);
    let corner = vertex_id % 3u;
    var output: MeshOutput;
    output.position = mesh.projection * position;
    output.view_position = position.xyz;
    output.view_normal = (mesh.view * vec4<f32>(input.normal.xyz, 0.0)).xyz;
    output.albedo = select(mesh.base_color.rgb, linear(input.color.rgb), input.color.a > 0.5);
    output.barycentric = vec3<f32>(select(0.0, 1.0, corner == 0u), select(0.0, 1.0, corner == 1u), select(0.0, 1.0, corner == 2u));
    return output;
}

@fragment
fn mesh_fragment(input: MeshOutput) -> @location(0) vec4<f32> {
    let key_color = vec3<f32>(1.0, 0.96, 0.9);
    let eye = normalize(-input.view_position);
    var normal = input.view_normal;
    if (dot(normal, normal) < 1e-20) { normal = eye; }
    normal = normalize(normal);
    if (dot(normal, eye) < 0.0) { normal = -normal; }
    let key_direction = normalize(vec3<f32>(-0.45, 0.65, 0.6));
    let fill_direction = normalize(vec3<f32>(0.7, 0.05, 0.5));
    let rim_direction = normalize(vec3<f32>(0.15, 0.45, -0.9));
    let key_half = normalize(key_direction + vec3<f32>(0.0, 0.0, 1.0));
    let sky = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    let ambient = mix(vec3<f32>(0.12, 0.11, 0.1), vec3<f32>(0.3, 0.31, 0.34), sky);
    let key = max(dot(normal, key_direction), 0.0);
    let fill = max(dot(normal, fill_direction), 0.0);
    let grazing = 1.0 - clamp(dot(normal, eye), 0.0, 1.0);
    let rim = max(dot(normal, rim_direction), 0.0) * grazing;
    let light = ambient + key_color * key + vec3<f32>(0.26, 0.29, 0.34) * fill + vec3<f32>(0.32, 0.32, 0.34) * rim;
    let specular = pow(max(dot(normal, key_half), 0.0), 48.0) * 0.22 * sqrt(min(key, 1.0));
    var color = input.albedo * light + key_color * specular;
    if (mesh.edge_color.a > 0.0) {
        let pixels = input.barycentric / max(fwidth(input.barycentric), vec3<f32>(1e-6));
        let distance = min(pixels.x, min(pixels.y, pixels.z));
        let coverage = clamp(mesh.params.x + 0.5 - distance, 0.0, 1.0) * mesh.edge_color.a;
        color = mix(color, mesh.edge_color.rgb, coverage);
    }
    return vec4<f32>(encoded(color), 1.0);
}
