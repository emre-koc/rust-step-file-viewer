// Globals is shared with the geometry shaders. Texture type is specialized for MSAA.
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var opaque: TEXTURE_TYPE<f32>;
@group(1) @binding(1) var accum: TEXTURE_TYPE<f32>;
@group(1) @binding(2) var reveal: TEXTURE_TYPE<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vertex], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(pos.xy);
    var color = vec4<f32>(0.0);
    for (var sample = 0; sample < SAMPLE_COUNT; sample++) {
        var c = textureLoad(opaque, pixel, sample);
        if (globals.modes.w == 1u) {
            let a = textureLoad(accum, pixel, sample);
            let r = clamp(textureLoad(reveal, pixel, sample).r, 0.0, 1.0);
            let alpha = 1.0 - r;
            let rgb = a.rgb / max(a.a, 0.00001);
            c = vec4<f32>(rgb * alpha + c.rgb * r, alpha + c.a * r);
        }
        color += c;
    }
    color /= f32(SAMPLE_COUNT);
    // Return straight alpha, including MSAA silhouettes, for PNG and native texture consumers.
    var rgb = color.rgb / max(color.a, 0.00001);
    if (globals.modes.y == 0u) {
        rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        rgb = select(1.055 * pow(rgb, vec3<f32>(1.0 / 2.4)) - 0.055, rgb * 12.92, rgb <= vec3<f32>(0.0031308));
    }
    return vec4<f32>(rgb, color.a);
}
