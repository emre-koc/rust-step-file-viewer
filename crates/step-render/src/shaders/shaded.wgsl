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

fn shade(in: VsOut, front: bool) -> vec3<f32> {
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

    let v = select(normalize(globals.camera_pos.xyz - in.world_pos), -globals.view_dir.xyz, globals.view_dir.w > 0.5);
    let l = normalize(globals.light_dir.xyz);
    let fill = max(dot(n, globals.fill_dir.xyz), 0.0) * 0.20;
    let key = max(dot(n, l), 0.0) * 0.56;
    let h = normalize(l + v);
    let hf = normalize(globals.fill_dir.xyz + v);
    let spec = pow(max(dot(n, h), 0.0), 24.0) * 0.12
        + pow(max(dot(n, hf), 0.0), 16.0) * 0.04;
    return base * (0.33 + key + fill) + vec3<f32>(spec);
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (in.color.a * globals.params.x < 1.0) { discard; }
    return vec4<f32>(shade(in, front), 1.0);
}

@fragment
fn fs_transparent(in: VsOut, @builtin(front_facing) front: bool) -> TransparentOut {
    let alpha = in.color.a * globals.params.x;
    if (alpha <= 0.0 || alpha >= 1.0) { discard; }
    return transparent_output(shade(in, front), alpha, in.clip_pos.z);
}
