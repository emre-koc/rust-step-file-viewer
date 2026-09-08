// Feature-edge pass (line list). Lines are nudged towards the viewer in clip space so they win the
// depth test against the coplanar triangles they belong to.

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) color: vec4<f32>,
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
    // z/w is the 0..1 depth; subtracting bias*w shifts NDC depth by exactly `bias`.
    out.clip_pos.z = out.clip_pos.z - globals.params.y * out.clip_pos.w;
    out.world_pos = wp;

    var c = globals.edge_color;
    if (face_is_selected(inst, inst.face_base + face_slot)) {
        c = vec4<f32>(mix(c.rgb, globals.sel_color.rgb, globals.sel_color.a), 1.0);
    }
    out.color = c;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (is_clipped(in.world_pos)) {
        discard;
    }
    return to_target(in.color.rgb, 1.0);
}
