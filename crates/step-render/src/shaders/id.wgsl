// Picking pass. Writes `vec4<u32>(instance_id + 1, shape-global face index, bitcast(ndc depth), 0)`
// into an Rgba32Uint attachment; 0 in `r` means background. Single-sampled, with its own depth
// buffer. Depth travels in the colour attachment because a 1x1 copy out of a depth texture is not
// portable (Metal rejects it).

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) @interpolate(flat) ids: vec2<u32>,
    @location(2) opacity: f32,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) face_slot: u32,
    @builtin(instance_index) ii: u32,
) -> VsOut {
    let inst = instances[ii];
    let wp = (inst.model * vec4<f32>(pos, 1.0)).xyz;

    var out: VsOut;
    out.clip_pos = globals.view_proj * vec4<f32>(wp, 1.0);
    out.world_pos = wp;
    out.opacity = face_color(inst, inst.face_base + face_slot).a;
    out.ids = vec2<u32>(inst.pick_id, inst.face_base + face_slot);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<u32> {
    if (is_clipped(in.world_pos) || in.opacity <= 0.0) {
        discard;
    }
    // `@builtin(position).z` in a fragment is already the 0..1 window depth.
    return vec4<u32>(in.ids.x, in.ids.y, bitcast<u32>(in.clip_pos.z), 0u);
}
