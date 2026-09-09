//! Headless GPU tests. They skip with a message when no adapter is available so the suite still
//! passes in a container; on a machine with a GPU they run for real.

use std::ops::Range;
use std::path::PathBuf;

use glam::{DAffine3, DVec3};
use step_mesh::mesh::SurfaceKind;
use step_mesh::topo::{BodyId, FaceId};
use step_mesh::{Aabb, BodyMesh, FaceRange, ShapeMesh};
use step_render::{Camera, ClipPlane, RenderMode, RenderSettings, Renderer, StandardView, TargetViews};

// ---------------------------------------------------------------- fixtures

/// Distinct sRGB colours, one per cube face. Deliberately far apart in hue so the shaded image can
/// be classified without matching exact values.
const FACE_COLORS: [[u8; 4]; 6] = [
    [220, 40, 40, 255],   // -X red
    [40, 200, 60, 255],   // +X green
    [50, 90, 230, 255],   // -Y blue
    [230, 200, 40, 255],  // +Y yellow
    [40, 210, 210, 255],  // -Z cyan
    [235, 235, 235, 255], // +Z white
];

const CAP_COLOR: [u8; 4] = [255, 0, 255, 255];

/// Axis-aligned box with one `FaceRange` per side, 4 vertices per side (hard normals) and the 12
/// feature edges as a line list.
fn box_body(min: DVec3, max: DVec3, colors: &[[u8; 4]; 6], double_sided: bool) -> BodyMesh {
    // (normal, four corner offsets) for the six sides, wound counter-clockwise seen from outside.
    let c = |i: usize| -> [f64; 3] {
        [
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        ]
    };
    // Corner indices per side, CCW from outside (0..7 = xyz bit pattern above).
    let sides: [([f64; 3], [usize; 4]); 6] = [
        ([-1.0, 0.0, 0.0], [0, 4, 6, 2]), // -X
        ([1.0, 0.0, 0.0], [1, 3, 7, 5]),  // +X
        ([0.0, -1.0, 0.0], [0, 1, 5, 4]), // -Y
        ([0.0, 1.0, 0.0], [2, 6, 7, 3]),  // +Y
        ([0.0, 0.0, -1.0], [0, 2, 3, 1]), // -Z
        ([0.0, 0.0, 1.0], [4, 5, 7, 6]),  // +Z
    ];

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut face_slot = Vec::new();
    let mut indices = Vec::new();
    let mut face_ranges = Vec::new();

    for (slot, (n, corners)) in sides.iter().enumerate() {
        let base = positions.len() as u32;
        let start = indices.len() as u32;
        for &ci in corners {
            let p = c(ci);
            positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
            normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
            face_slot.push(slot as u32);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        face_ranges.push(FaceRange {
            face: FaceId(slot as u32),
            src: 100 + slot as u32,
            indices: Range { start, end: indices.len() as u32 },
            color: colors[slot],
            surface_kind: SurfaceKind::Plane,
        });
    }

    // 12 feature edges. Vertices are duplicated per side, so pick the -X/+X quads (slots 0 and 1)
    // for the eight axial edges and the -Z/+Z quads for the four remaining ones.
    let mut edge_indices = Vec::new();
    for slot in [0u32, 1] {
        let b = slot * 4;
        edge_indices.extend_from_slice(&[b, b + 1, b + 1, b + 2, b + 2, b + 3, b + 3, b]);
    }
    for slot in [4u32, 5] {
        let b = slot * 4;
        edge_indices.extend_from_slice(&[b, b + 1, b + 2, b + 3]);
    }

    BodyMesh {
        body: BodyId(0),
        name: "cube".into(),
        origin: DVec3::ZERO,
        positions,
        normals,
        face_slot,
        indices,
        face_ranges,
        edge_indices,
        double_sided,
        bbox: Aabb { min, max },
    }
}

/// A flat one-face quad in the XY plane, marked double sided (a sheet body).
fn sheet_body(origin: DVec3, size: f64) -> BodyMesh {
    let h = size * 0.5;
    let pts = [[-h, -h, 0.0], [h, -h, 0.0], [h, h, 0.0], [-h, h, 0.0]];
    let positions: Vec<[f32; 3]> = pts.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
    BodyMesh {
        body: BodyId(1),
        name: "sheet".into(),
        origin,
        positions,
        normals: vec![[0.0, 0.0, 1.0]; 4],
        face_slot: vec![0; 4],
        indices: vec![0, 1, 2, 0, 2, 3],
        face_ranges: vec![FaceRange {
            face: FaceId(0),
            src: 200,
            indices: Range { start: 0, end: 6 },
            color: [255, 140, 0, 255],
            surface_kind: SurfaceKind::Plane,
        }],
        edge_indices: vec![0, 1, 1, 2, 2, 3, 3, 0],
        double_sided: true,
        bbox: Aabb {
            min: origin + DVec3::new(-h, -h, 0.0),
            max: origin + DVec3::new(h, h, 0.0),
        },
    }
}

/// Cube at the origin sitting on a double-sided sheet, so the image centre is always geometry.
fn test_shape() -> ShapeMesh {
    let cube = box_body(DVec3::splat(-1.0), DVec3::splat(1.0), &FACE_COLORS, false);
    let sheet = sheet_body(DVec3::new(0.0, 0.0, -2.0), 6.0);
    let mut bbox = Aabb::EMPTY;
    bbox.union(&cube.bbox);
    bbox.union(&sheet.bbox);
    ShapeMesh { bodies: vec![cube, sheet], bbox, ..Default::default() }
}

/// Only the cube, for tests that must not be confused by the sheet.
fn cube_only_shape() -> ShapeMesh {
    let cube = box_body(DVec3::splat(-1.0), DVec3::splat(1.0), &FACE_COLORS, false);
    let bbox = cube.bbox;
    ShapeMesh { bodies: vec![cube], bbox, ..Default::default() }
}

/// Test settings: a black background and a bright edge colour keep [`hue_bucket`] unambiguous.
fn test_settings() -> RenderSettings {
    RenderSettings {
        background: [0.0, 0.0, 0.0, 1.0],
        edge_color: [210, 210, 210, 255],
        ..Default::default()
    }
}

fn build_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build");
    std::fs::create_dir_all(&dir).expect("create build/");
    dir
}

/// `None` (with a printed reason) when no GPU is available.
macro_rules! gpu {
    () => {
        match Renderer::headless() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: no GPU available ({e})");
                return;
            }
        }
    };
}

/// Classify a pixel into a coarse hue bucket so shaded output can be compared with the source
/// colours without matching exact values.
fn hue_bucket(p: &[u8; 4]) -> Option<&'static str> {
    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
    let max = r.max(g).max(b);
    if max < 26 {
        return None; // background / very dark
    }
    let hi = |v: i32| v * 100 >= max * 62;
    match (hi(r), hi(g), hi(b)) {
        (true, false, false) => Some("red"),
        (false, true, false) => Some("green"),
        (false, false, true) => Some("blue"),
        (true, true, false) => Some("yellow"),
        (true, false, true) => Some("magenta"),
        (false, true, true) => Some("cyan"),
        (true, true, true) => Some("white"),
        _ => None,
    }
}

// -------------------------------------------------------------------- tests

#[test]
fn renders_a_cube_offscreen() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &test_shape());
    scene.add_instance(shape, DAffine3::IDENTITY, 7);

    let mut camera = Camera::default();
    camera.standard_view(StandardView::Iso);
    camera.fit(scene.bbox());

    let settings = RenderSettings { msaa: 4, ..test_settings() };
    let img = renderer
        .render_offscreen(&device, &queue, &mut scene, &camera, &settings, 512, 512)
        .expect("offscreen render");

    let path = build_dir().join("render_test_cube.png");
    img.save(&path).expect("write png");
    eprintln!("wrote {}", path.display());

    // The centre of an iso-framed cube must be geometry, not background.
    let centre = img.get_pixel(256, 256).0;
    let bg = img.get_pixel(2, 2).0;
    assert_ne!(centre, bg, "centre pixel is background ({centre:?})");

    // At least two of the six face colours must survive shading.
    let mut buckets: Vec<&str> = Vec::new();
    for p in img.pixels() {
        if let Some(b) = hue_bucket(&p.0)
            && !buckets.contains(&b)
        {
            buckets.push(b);
        }
    }
    assert!(buckets.len() >= 2, "expected >= 2 distinct face colours, saw {buckets:?}");
    // The iso view of this cube shows +X (green), -Y (blue) and +Z (white).
    for want in ["green", "blue", "white"] {
        assert!(buckets.contains(&want), "missing the {want} face; saw {buckets:?}");
    }
    // The double-sided sheet under the cube must be there too (its orange reads as "yellow").
    assert!(buckets.contains(&"yellow"), "sheet body missing; saw {buckets:?}");
}

#[test]
fn renders_every_mode() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &test_shape());
    let inst = scene.add_instance(shape, DAffine3::IDENTITY, 1);
    scene.set_selected_faces(inst, &[0, 3]);

    let mut camera = Camera::default();
    camera.fit(scene.bbox());

    for (mode, name) in [
        (RenderMode::Shaded, "shaded"),
        (RenderMode::ShadedEdges, "shaded_edges"),
        (RenderMode::Wireframe, "wireframe"),
        (RenderMode::XRay, "xray"),
    ] {
        for msaa in [1u32, 4] {
            let settings = RenderSettings { mode, msaa, ..test_settings() };
            let img = renderer
                .render_offscreen(&device, &queue, &mut scene, &camera, &settings, 128, 128)
                .unwrap_or_else(|e| panic!("{name} msaa{msaa}: {e}"));
            let bg = img.get_pixel(1, 1).0;
            let painted = img.pixels().filter(|p| p.0 != bg).count();
            assert!(painted > 20, "{name} msaa{msaa}: only {painted} non-background pixels");
        }
    }
}

#[test]
fn picks_the_cube_at_the_centre_and_nothing_in_the_corner() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &cube_only_shape());
    let inst = scene.add_instance(shape, DAffine3::IDENTITY, 42);

    let mut camera = Camera::default();
    camera.standard_view(StandardView::Iso);
    camera.fit(scene.bbox());

    // Render into the renderer's internal targets via a colour texture we own.
    let size = 256u32;
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pick target"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.color_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = color.create_view(&wgpu::TextureViewDescriptor::default());

    let settings = test_settings();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    renderer.render(
        &device,
        &queue,
        &mut encoder,
        &mut scene,
        &camera,
        &settings,
        &TargetViews { color: &view },
        (size, size),
    );
    queue.submit(Some(encoder.finish()));

    let hit = renderer.pick(&device, &queue, &scene, size / 2, size / 2).expect("centre must hit the cube");
    assert_eq!(hit.instance, inst);
    assert_eq!(hit.node_id, 42);
    assert_eq!(hit.body, 0);
    assert!(hit.face_slot < 6, "face_slot {} out of range", hit.face_slot);
    assert!(hit.face_index < scene.shape_face_count(shape));
    assert_eq!(hit.face_src, 100 + hit.face_slot);
    assert!(hit.depth > 0.0 && hit.depth < 1.0, "depth {} not in the frustum", hit.depth);
    // The centre of an iso view of a cube is the corner nearest the camera: +X, +Z or -Y.
    assert!(matches!(hit.face_slot, 1 | 2 | 5), "unexpected face {}", hit.face_slot);

    assert!(renderer.pick(&device, &queue, &scene, 1, 1).is_none(), "corner must be background");
    assert!(renderer.pick(&device, &queue, &scene, size - 2, size - 2).is_none());
    assert!(renderer.pick(&device, &queue, &scene, size, 0).is_none(), "out of range must be None");
}

#[test]
fn clip_plane_shows_the_cap_colour() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &cube_only_shape());
    scene.add_instance(shape, DAffine3::IDENTITY, 0);

    let mut camera = Camera::default();
    camera.standard_view(StandardView::Iso);
    camera.fit(scene.bbox());

    let base = RenderSettings { mode: RenderMode::Shaded, msaa: 1, ..test_settings() };
    let plain = renderer
        .render_offscreen(&device, &queue, &mut scene, &camera, &base, 256, 256)
        .expect("uncut render");
    let magenta_before = plain.pixels().filter(|p| hue_bucket(&p.0) == Some("magenta")).count();
    assert_eq!(magenta_before, 0, "the uncut cube must not contain the cap colour");

    // The iso camera sits at +X/-Y/+Z; cutting away x > 0 opens the solid towards it.
    let clipped = RenderSettings {
        clip_plane: Some(ClipPlane::through(DVec3::ZERO, DVec3::X, CAP_COLOR)),
        ..base
    };
    let img = renderer
        .render_offscreen(&device, &queue, &mut scene, &camera, &clipped, 256, 256)
        .expect("clipped render");
    let path = build_dir().join("render_test_clip.png");
    img.save(&path).expect("write png");
    eprintln!("wrote {}", path.display());

    let magenta = img.pixels().filter(|p| hue_bucket(&p.0) == Some("magenta")).count();
    assert!(magenta > 200, "expected a visible cap, saw {magenta} cap-coloured pixels");

    // And the half-space really is gone: fewer painted pixels than the uncut render.
    let bg = img.get_pixel(1, 1).0;
    let painted = img.pixels().filter(|p| p.0 != bg).count();
    let painted_before = plain.pixels().filter(|p| p.0 != bg).count();
    assert!(painted < painted_before, "clipping did not remove anything ({painted} vs {painted_before})");
}

#[test]
fn instancing_visibility_and_overrides() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &cube_only_shape());
    let a = scene.add_instance(shape, DAffine3::from_translation(DVec3::new(-3.0, 0.0, 0.0)), 1);
    let b = scene.add_instance(shape, DAffine3::from_translation(DVec3::new(3.0, 0.0, 0.0)), 2);

    // Two instances of one shape: the bbox spans both.
    let bbox = scene.bbox();
    assert!(bbox.min.x < -3.5 && bbox.max.x > 3.5, "{bbox:?}");

    scene.set_visible(b, false);
    let bbox = scene.bbox();
    assert!(bbox.max.x < 0.0, "hidden instance still in the bbox: {bbox:?}");
    scene.set_visible(b, true);

    scene.set_instance_color_override(a, Some([255, 0, 255, 255]));
    scene.set_selected_instances(&[b]);

    let mut camera = Camera::default();
    camera.standard_view(StandardView::Front);
    camera.fit(scene.bbox());
    let settings = RenderSettings { mode: RenderMode::Shaded, msaa: 1, ..test_settings() };
    let img = renderer
        .render_offscreen(&device, &queue, &mut scene, &camera, &settings, 256, 128)
        .expect("render");
    // The override paints the left cube magenta; the right one keeps its own (blue -Y) face.
    let magenta = img.pixels().filter(|p| hue_bucket(&p.0) == Some("magenta")).count();
    assert!(magenta > 200, "colour override not visible ({magenta} px)");

    scene.clear();
    assert_eq!(scene.instance_count(), 0);
    assert_eq!(scene.shape_count(), 0);
    assert!(scene.bbox().is_empty());
}

#[test]
fn far_from_origin_models_stay_sharp() {
    // 10 km away from the origin: with a naive f32 pipeline the cube would fall apart.
    let far = DVec3::new(1.0e7, -5.0e6, 2.0e6);
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &cube_only_shape());
    scene.add_instance(shape, DAffine3::from_translation(far), 1);

    let mut camera = Camera::default();
    camera.standard_view(StandardView::Iso);
    camera.fit(scene.bbox());

    let settings = RenderSettings { mode: RenderMode::Shaded, msaa: 1, ..test_settings() };
    let img = renderer
        .render_offscreen(&device, &queue, &mut scene, &camera, &settings, 128, 128)
        .expect("render");
    let bg = img.get_pixel(1, 1).0;
    let painted = img.pixels().filter(|p| p.0 != bg).count();
    assert!(painted > 2000, "far-away cube barely rendered ({painted} px)");
    let mut buckets: Vec<&str> = Vec::new();
    for p in img.pixels() {
        if let Some(x) = hue_bucket(&p.0)
            && !buckets.contains(&x)
        {
            buckets.push(x);
        }
    }
    assert!(buckets.len() >= 2, "far-away cube lost its face colours: {buckets:?}");
}

fn colored_sheet(z: f64, color: [u8; 4]) -> ShapeMesh {
    let mut body = sheet_body(DVec3::new(0.0, 0.0, z), 3.0);
    body.face_ranges[0].color = color;
    ShapeMesh { bbox: body.bbox, bodies: vec![body], ..Default::default() }
}

fn linear_channel(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

#[test]
fn replacing_selection_clears_old_face_tints_and_restores_original_materials() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let shape = scene.upload_shape(&device, &queue, &cube_only_shape());
    let instances = [
        scene.add_instance(shape, DAffine3::from_translation(DVec3::new(-3.0, 0.0, 0.0)), 1),
        scene.add_instance(shape, DAffine3::IDENTITY, 2),
        scene.add_instance(shape, DAffine3::from_translation(DVec3::new(3.0, 0.0, 0.0)), 3),
    ];
    scene.set_instance_color_override(instances[0], Some([110, 200, 230, 89]));
    scene.set_instance_color_override(instances[1], Some([35, 40, 45, 255]));
    let mut camera = Camera::default();
    camera.standard_view(StandardView::Iso);
    camera.fit_aspect(scene.bbox(), 2.0);
    for msaa in [1, 4] {
        let settings = RenderSettings { mode: RenderMode::ShadedEdges, msaa, ..Default::default() };
        let baseline = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 192, 96).unwrap();
        for instance in instances {
            // Viewport picks set both the whole-instance flag and a face bit.
            scene.set_selected_instances(&[instance]);
            scene.set_selected_faces(instance, &[2, 5]);
            let selected = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 192, 96).unwrap();
            assert_ne!(selected, baseline, "selection must visibly highlight the part");
            scene.set_selected_instances(&[]);
            let cleared = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 192, 96).unwrap();
            assert_eq!(cleared, baseline, "deselection left a face tint (MSAA {msaa})");
        }
        // Selecting another part or tree node must also discard older face bits.
        scene.set_selected_instances(&[instances[2]]);
        let expected = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 192, 96).unwrap();
        for instance in &instances[..2] {
            scene.set_selected_instances(&[*instance]);
            scene.set_selected_faces(*instance, &[2, 5]);
            renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 192, 96).unwrap();
        }
        scene.set_selected_instances(&[instances[2]]);
        let switched = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 192, 96).unwrap();
        assert_eq!(switched, expected, "switching parts retained an earlier face highlight");
        scene.set_selected_instances(&[]);
    }
}

#[test]
fn opacity_blends_in_linear_space_and_zero_alpha_is_not_pickable() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let handle = scene.upload_shape(&device, &queue, &colored_sheet(0.0, [110, 200, 230, 255]));
    let inst = scene.add_instance(handle, DAffine3::IDENTITY, 42);
    let mut camera = Camera { ortho: true, ..Default::default() };
    camera.standard_view(StandardView::Top);
    camera.fit(scene.bbox());
    for msaa in [1, 4] {
        let settings = RenderSettings { mode: RenderMode::Shaded, msaa, background: [0.05, 0.05, 0.05, 1.0], ..Default::default() };
        scene.set_instance_color_override(inst, None);
        let opaque = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 96, 96).unwrap();
        scene.set_instance_color_override(inst, Some([110, 200, 230, 89]));
        let transparent = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 96, 96).unwrap();
        for c in 0..3 {
            let expected = linear_channel(opaque[(48, 48)][c]) * (89.0 / 255.0) + 0.05 * (166.0 / 255.0);
            let actual = linear_channel(transparent[(48, 48)][c]);
            assert!((actual - expected).abs() < 0.008, "channel {c}: {actual} != {expected}");
        }
        assert_eq!(renderer.pick(&device, &queue, &scene, 48, 48).unwrap().node_id, 42);
        scene.set_selected_instances(&[inst]);
        let selected = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &RenderSettings { background: [0.0; 4], ..settings.clone() }, 96, 96).unwrap();
        assert!((selected[(48, 48)][3] as i16 - 89).abs() <= 1, "selection changed opacity");
        scene.set_selected_instances(&[]);
        scene.set_instance_color_override(inst, Some([110, 200, 230, 0]));
        let hidden = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &RenderSettings { mode: RenderMode::ShadedEdges, ..settings }, 96, 96).unwrap();
        assert!(hidden.pixels().all(|p| p == &hidden[(0, 0)]), "invisible faces left color or edges");
        assert!(renderer.pick(&device, &queue, &scene, 48, 48).is_none());
    }
}

#[test]
fn transparent_layers_are_order_independent_and_respect_opaque_depth() {
    let (device, queue, mut renderer) = gpu!();
    let mut scene = renderer.new_scene(&device);
    let red = scene.upload_shape(&device, &queue, &colored_sheet(0.0, [230, 40, 40, 100]));
    let blue = scene.upload_shape(&device, &queue, &colored_sheet(0.5, [40, 70, 230, 150]));
    let a = scene.add_instance(red, DAffine3::IDENTITY, 1);
    let b = scene.add_instance(blue, DAffine3::IDENTITY, 2);
    let mut camera = Camera { ortho: true, ..Default::default() };
    camera.standard_view(StandardView::Top);
    camera.fit(scene.bbox());
    for msaa in [1, 4] {
        let settings = RenderSettings { mode: RenderMode::Shaded, msaa, ..Default::default() };
        scene.set_visible(a, true);
        scene.set_instance_color_override(b, None);
        let forward = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 96, 96).unwrap();
        let mut reverse_scene = renderer.new_scene(&device);
        let blue = reverse_scene.upload_shape(&device, &queue, &colored_sheet(0.5, [40, 70, 230, 150]));
        let red = reverse_scene.upload_shape(&device, &queue, &colored_sheet(0.0, [230, 40, 40, 100]));
        reverse_scene.add_instance(blue, DAffine3::IDENTITY, 2);
        reverse_scene.add_instance(red, DAffine3::IDENTITY, 1);
        let reverse = renderer.render_offscreen(&device, &queue, &mut reverse_scene, &camera, &settings, 96, 96).unwrap();
        assert!(forward.as_raw().iter().zip(reverse.as_raw()).all(|(a,b)| a.abs_diff(*b) <= 1));
        scene.set_instance_color_override(b, Some([40, 70, 230, 255]));
        let with_hidden_red = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 96, 96).unwrap();
        scene.set_visible(a, false);
        let no_red = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 96, 96).unwrap();
        assert_eq!(with_hidden_red, no_red, "transparent geometry leaked through opaque foreground");
    }
}

#[test]
fn mixed_face_opacity_survives_clipping_xray_and_png_output() {
    let (device, queue, mut renderer) = gpu!();
    let mut colors = FACE_COLORS;
    colors[1][3] = 89;
    colors[2][3] = 0;
    let body = box_body(DVec3::splat(-1.0), DVec3::splat(1.0), &colors, false);
    let mut scene = renderer.new_scene(&device);
    let handle = scene.upload_shape(&device, &queue, &ShapeMesh { bbox: body.bbox, bodies: vec![body], ..Default::default() });
    let inst = scene.add_instance(handle, DAffine3::IDENTITY, 1);
    let mut camera = Camera::default();
    camera.fit(scene.bbox());
    for msaa in [1, 4] {
        for mode in [RenderMode::Shaded, RenderMode::ShadedEdges, RenderMode::XRay] {
            let settings = RenderSettings { mode, msaa, background: [0.0; 4], ..Default::default() };
            let img = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 128, 128).unwrap();
            assert_eq!(img[(0,0)][3], 0);
            assert!(img.pixels().any(|p| p[3] > 0 && p[3] < 255));
            scene.set_selected_faces(inst, &[1]);
            let clipped = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &RenderSettings {
                clip_plane: Some(ClipPlane::through(DVec3::ZERO, DVec3::Z, CAP_COLOR)), ..settings
            }, 128, 128).unwrap();
            assert!(clipped.pixels().any(|p| p[3] > 0));
            scene.set_selected_faces(inst, &[]);
        }
    }
    // A single transparent sheet has straight RGB even at antialiased silhouettes.
    let mut sheet_scene = renderer.new_scene(&device);
    let h = sheet_scene.upload_shape(&device, &queue, &colored_sheet(0.0, [190, 190, 190, 89]));
    sheet_scene.add_instance(h, DAffine3::IDENTITY, 1);
    let settings = RenderSettings { mode: RenderMode::Shaded, background: [0.0; 4], ..Default::default() };
    camera.fit(sheet_scene.bbox());
    let img = renderer.render_offscreen(&device, &queue, &mut sheet_scene, &camera, &settings, 128, 128).unwrap();
    assert!(img.pixels().any(|p| p[3] > 0 && p[3] < 80));
    for pixel in img.pixels().filter(|p| p[3] > 0) {
        assert!(pixel[0] > 100, "premultiplied RGB would produce dark PNG halos: {pixel:?}");
    }
}

#[test]
fn srgb_and_plain_output_targets_encode_the_same_colors() {
    let (device, queue, mut srgb) = gpu!();
    let mut plain = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
    for msaa in [1, 4] {
        let settings = RenderSettings { mode: RenderMode::Shaded, msaa, ..Default::default() };
        let mut images = Vec::new();
        for renderer in [&mut srgb, &mut plain] {
            let mut scene = renderer.new_scene(&device);
            let h = scene.upload_shape(&device, &queue, &colored_sheet(0.0, [230, 45, 100, 89]));
            scene.add_instance(h, DAffine3::IDENTITY, 1);
            let mut camera = Camera::default();
            camera.fit(scene.bbox());
            images.push(renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, 96, 96).unwrap());
        }
        assert!(images[0].as_raw().iter().zip(images[1].as_raw()).all(|(a,b)| a.abs_diff(*b) <= 2), "target encoding mismatch");
        assert!(images[0][(0, 0)][0].abs_diff(53) <= 1);
    }
}
