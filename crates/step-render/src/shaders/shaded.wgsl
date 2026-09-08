// Shaded triangle pass: per-face colour, hemisphere ambient + one key light, optional clip plane.

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
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
    // CAD placements are rigid (rotation + translation), so the upper 3x3 doubles as the normal
    // matrix; the fragment shader renormalises anyway.
    let m3 = mat3x3<f32>(inst.model[0].xyz, inst.model[1].xyz, inst.model[2].xyz);

    var out: VsOut;
    out.clip_pos = globals.view_proj * vec4<f32>(wp, 1.0);
    out.world_pos = wp;
    out.normal = m3 * nrm;
    out.color = face_color(inst, inst.face_base + face_slot);
    return out;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (is_clipped(in.world_pos)) {
        discard;
    }

    var base = in.color.rgb;
    var n = normalize(in.normal);
    if (!front) {
        n = -n;
        // Looking into a solid through the clip plane: paint the inside as if it were a cap.
        if (globals.modes.x == 1u) {
            base = globals.cap_color.rgb;
        }
    }

    let v = normalize(globals.camera_pos.xyz - in.world_pos);
    let l = normalize(globals.light_dir.xyz);
    let hemi = 0.5 + 0.5 * dot(n, globals.up_axis.xyz);
    let ambient = mix(vec3<f32>(0.13, 0.13, 0.15), vec3<f32>(0.42, 0.44, 0.48), hemi);
    let key = max(dot(n, l), 0.0) * 0.85;
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * 0.20;

    let rgb = base * (ambient + key) + vec3<f32>(spec);
    return to_target(rgb, in.color.a * globals.params.x);
}
