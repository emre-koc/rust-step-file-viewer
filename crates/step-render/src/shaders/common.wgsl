// Shared bindings and helpers. Prepended to every other shader in this directory.
//
// Spaces: `render space` is world space translated by the scene's render origin, so all f32
// coordinates stay small. Colours coming from the mesh are sRGB-encoded bytes; lighting is done in
// linear space and re-encoded on output when the target format is not `*Srgb`.

struct Globals {
    view_proj: mat4x4<f32>,
    // xyz = camera position in render space.
    camera_pos: vec4<f32>,
    // xyz = plane normal in render space, w = -d. Keep `dot(n, p) + w <= 0`.
    clip_plane: vec4<f32>,
    // Linear RGB used for back faces of clipped solids (the faked cap).
    cap_color: vec4<f32>,
    // Linear RGB of feature edges.
    edge_color: vec4<f32>,
    // Linear RGB of the selection tint; `a` is the blend weight.
    sel_color: vec4<f32>,
    // xyz = unit vector towards the key light, in render space.
    light_dir: vec4<f32>,
    // xyz = world up axis (hemisphere ambient).
    up_axis: vec4<f32>,
    // x = output alpha multiplier (x-ray), y = edge depth bias in NDC, z/w unused.
    params: vec4<f32>,
    // x = clip plane enabled, y = target is *Srgb, z = selection highlight enabled, w unused.
    modes: vec4<u32>,
};

struct Instance {
    model: mat4x4<f32>,
    flags: u32,
    color_override: u32,
    face_base: u32,
    node_id: u32,
    sel_base: u32,
    pick_id: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> face_sel: array<u32>;
@group(2) @binding(0) var<storage, read> face_colors: array<u32>;

const FLAG_SELECTED: u32 = 1u;
const FLAG_COLOR_OVERRIDE: u32 = 2u;
const NO_FACE_SELECTION: u32 = 0xffffffffu;

fn unpack_rgba(v: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(v & 0xffu),
        f32((v >> 8u) & 0xffu),
        f32((v >> 16u) & 0xffu),
        f32((v >> 24u) & 0xffu),
    ) * (1.0 / 255.0);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c * (1.0 / 12.92);
    let hi = pow((c + vec3<f32>(0.055)) * (1.0 / 1.055), vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

// True when the fragment is on the discarded side of the clip plane.
fn is_clipped(p: vec3<f32>) -> bool {
    return globals.modes.x == 1u && dot(globals.clip_plane.xyz, p) + globals.clip_plane.w > 0.0;
}

// Encode a linear colour for the bound colour target.
fn to_target(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    if (globals.modes.y == 1u) {
        return vec4<f32>(rgb, a);
    }
    return vec4<f32>(linear_to_srgb(rgb), a);
}

fn face_is_selected(inst: Instance, face_index: u32) -> bool {
    if (globals.modes.z != 1u) {
        return false;
    }
    if ((inst.flags & FLAG_SELECTED) != 0u) {
        return true;
    }
    if (inst.sel_base == NO_FACE_SELECTION) {
        return false;
    }
    let word = face_sel[inst.sel_base + (face_index >> 5u)];
    return (word & (1u << (face_index & 31u))) != 0u;
}

// Base colour of a face, in linear RGB, honouring override and selection tint.
fn face_color(inst: Instance, face_index: u32) -> vec4<f32> {
    var rgba = unpack_rgba(face_colors[face_index]);
    if ((inst.flags & FLAG_COLOR_OVERRIDE) != 0u) {
        rgba = unpack_rgba(inst.color_override);
    }
    var lin = srgb_to_linear(rgba.rgb);
    if (face_is_selected(inst, face_index)) {
        lin = mix(lin, globals.sel_color.rgb, globals.sel_color.a);
    }
    return vec4<f32>(lin, rgba.a);
}
