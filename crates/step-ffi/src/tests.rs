//! FFI-level tests: everything is driven through the exported C functions, with raw pointers, so a
//! break in the ABI surface (not just in the Rust helpers) fails the suite.

use std::ffi::{CStr, CString};

use super::*;

fn fixture(name: &str) -> Option<CString> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name);
    if !p.exists() {
        eprintln!("skipping: fixture {} is missing", p.display());
        return None;
    }
    Some(CString::new(p.to_str().unwrap()).unwrap())
}

fn last_error() -> String {
    // SAFETY: `sv_last_error` always returns a valid NUL-terminated pointer.
    unsafe { CStr::from_ptr(sv_last_error()) }.to_string_lossy().into_owned()
}

#[test]
fn thumbnail_png_is_a_valid_image() {
    let Some(path) = fixture("cube_mm.step") else { return };
    let mut png: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let rc = sv_thumbnail_png(path.as_ptr(), 256, 5_000, &mut png, &mut len);
    assert!(rc == SV_OK || rc == SV_DEGRADED, "rc {rc}: {}", last_error());
    assert!(!png.is_null() && len > 0);

    // SAFETY: `png`/`len` were just filled in by a successful call.
    let bytes = unsafe { std::slice::from_raw_parts(png, len) };
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    let img = image::load_from_memory(bytes).expect("decodes as PNG").to_rgba8();
    assert_eq!((img.width(), img.height()), (256, 256));

    // The background is transparent, so "not blank" means some pixel is opaque.
    let opaque = img.pixels().filter(|p| p.0[3] > 128).count();
    assert!(opaque > 256, "only {opaque} opaque pixels — image looks blank (rc {rc})");
    // ...and the corners must stay transparent.
    assert_eq!(img.get_pixel(0, 0).0[3], 0, "corner is not transparent");

    sv_free_bytes(png, len);
}

#[test]
fn thumbnail_rejects_a_missing_file() {
    let bogus = CString::new("/nonexistent/definitely-not-here.step").unwrap();
    let mut png: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let rc = sv_thumbnail_png(bogus.as_ptr(), 128, 1_000, &mut png, &mut len);
    assert_eq!(rc, SV_ERR_OPEN);
    assert!(png.is_null() && len == 0);
    assert!(last_error().contains("opening"), "unhelpful message: {:?}", last_error());
}

#[test]
fn thumbnail_null_arguments_do_not_crash() {
    assert_eq!(sv_thumbnail_png(std::ptr::null(), 64, 0, std::ptr::null_mut(), std::ptr::null_mut()), SV_ERR_ARGS);
    sv_free_bytes(std::ptr::null_mut(), 0);
    sv_free(std::ptr::null_mut());
    assert_eq!(sv_mesh_count(std::ptr::null()), 0);
    assert_eq!(sv_instance_count(std::ptr::null()), 0);
    assert_eq!(sv_mesh_info(std::ptr::null(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), SV_ERR_ARGS);
}

#[test]
fn load_exposes_meshes_instances_and_bounds() {
    let Some(path) = fixture("cube_mm.step") else { return };
    let m = sv_load(path.as_ptr(), 0);
    assert!(!m.is_null(), "sv_load failed: {}", last_error());

    let mesh_count = sv_mesh_count(m);
    let inst_count = sv_instance_count(m);
    assert!(mesh_count >= 1, "expected at least one body mesh");
    assert!(inst_count >= mesh_count, "every mesh needs at least one placement");

    let mut bbox = [0.0f64; 6];
    sv_model_bbox(m, bbox.as_mut_ptr());
    for k in 0..3 {
        assert!(bbox[k] < bbox[k + 3], "degenerate bbox axis {k}: {bbox:?}");
    }
    // cube_mm.step is a 10 mm cube.
    let size: Vec<f64> = (0..3).map(|k| bbox[k + 3] - bbox[k]).collect();
    assert!(size.iter().all(|s| (*s - 10.0).abs() < 0.01), "expected a 10 mm cube, got {size:?}");

    for mesh in 0..mesh_count {
        let (mut vc, mut ic, mut ec, mut ds) = (0u32, 0u32, 0u32, 0u8);
        assert_eq!(sv_mesh_info(m, mesh, &mut vc, &mut ic, &mut ec, &mut ds), SV_OK);
        assert!(vc > 0 && ic > 0 && ic % 3 == 0, "mesh {mesh}: vc {vc} ic {ic}");
        assert!(ec % 2 == 0, "edge indices must come in pairs, got {ec}");

        let mut pos = vec![0.0f32; vc as usize * 3];
        let mut nrm = vec![0.0f32; vc as usize * 3];
        let mut col = vec![0u8; vc as usize * 4];
        let mut idx = vec![0u32; ic as usize];
        let mut edg = vec![0u32; ec as usize];
        assert_eq!(
            sv_mesh_buffers(m, mesh, pos.as_mut_ptr(), nrm.as_mut_ptr(), col.as_mut_ptr(), idx.as_mut_ptr(), edg.as_mut_ptr()),
            SV_OK
        );
        assert!(idx.iter().all(|&i| i < vc), "index out of range in mesh {mesh}");
        assert!(edg.iter().all(|&i| i < vc), "edge index out of range in mesh {mesh}");
        assert!(pos.iter().all(|v| v.is_finite()), "non-finite position");
        assert!(nrm.chunks_exact(3).any(|n| n.iter().any(|v| *v != 0.0)), "all normals are zero");
        assert!(col.chunks_exact(4).all(|c| c[3] > 0), "a vertex colour is fully transparent");
    }

    // Partial reads: every destination is optional.
    assert_eq!(sv_mesh_buffers(m, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), SV_OK);

    for i in 0..inst_count {
        let mut mesh = u32::MAX;
        let mut mat = [0.0f32; 16];
        let mut name = [0i8; 256];
        assert_eq!(sv_instance(m, i, &mut mesh, mat.as_mut_ptr(), name.as_mut_ptr(), name.len()), SV_OK);
        assert!(mesh < mesh_count, "instance {i} points at mesh {mesh}");
        assert!(mat.iter().all(|v| v.is_finite()));
        // Column-major: the last column is the translation and w must be 1.
        assert_eq!(mat[15], 1.0, "matrix {mat:?} is not affine");
        // SAFETY: `sv_instance` NUL-terminates within `name`.
        let _ = unsafe { CStr::from_ptr(name.as_ptr()) }.to_str().expect("name is valid UTF-8");
    }

    assert_eq!(sv_mesh_info(m, mesh_count, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), SV_ERR_RANGE);
    assert_eq!(sv_instance(m, inst_count, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0), SV_ERR_RANGE);

    sv_free(m);
}

#[test]
fn assembly_shares_geometry_between_instances() {
    let Some(path) = fixture("two_cubes_assembly_inch.step") else { return };
    let m = sv_load(path.as_ptr(), 1);
    assert!(!m.is_null(), "sv_load failed: {}", last_error());
    let inst_count = sv_instance_count(m);
    assert!(inst_count >= 2, "expected at least two placements, got {inst_count}");

    // Inch input must arrive in millimetres: a 1" cube is 25.4 mm.
    let mut bbox = [0.0f64; 6];
    sv_model_bbox(m, bbox.as_mut_ptr());
    assert!(bbox[3] - bbox[0] > 20.0, "bbox {bbox:?} looks like inches, not mm");

    // Distinct placements of the same part must differ in their translation column.
    let mut translations = Vec::new();
    for i in 0..inst_count {
        let mut mat = [0.0f32; 16];
        assert_eq!(sv_instance(m, i, std::ptr::null_mut(), mat.as_mut_ptr(), std::ptr::null_mut(), 0), SV_OK);
        translations.push([mat[12], mat[13], mat[14]]);
    }
    assert!(translations.windows(2).any(|w| w[0] != w[1]), "all instances sit at the same place");
    sv_free(m);
}

#[test]
fn instance_names_truncate_safely() {
    let Some(path) = fixture("cube_mm.step") else { return };
    let m = sv_load(path.as_ptr(), 0);
    assert!(!m.is_null());
    let mut tiny = [0x7fi8; 4];
    assert_eq!(sv_instance(m, 0, std::ptr::null_mut(), std::ptr::null_mut(), tiny.as_mut_ptr(), tiny.len()), SV_OK);
    // SAFETY: the call must have written a terminator inside `tiny`.
    let s = unsafe { CStr::from_ptr(tiny.as_ptr()) };
    assert!(s.to_bytes().len() < 4);
    assert!(s.to_str().is_ok(), "truncation split a UTF-8 sequence");
    sv_free(m);
}

#[test]
fn cpu_box_rasteriser_draws_something() {
    let bb = step_mesh::Aabb { min: glam::DVec3::new(-5.0, -5.0, -5.0), max: glam::DVec3::new(5.0, 5.0, 5.0) };
    let img = raster::render_boxes(&[(bb, glam::DAffine3::IDENTITY)], 128).expect("renders");
    assert_eq!((img.width(), img.height()), (128, 128));
    assert!(img.pixels().filter(|p| p.0[3] > 200).count() > 1000, "box silhouette is nearly empty");
    assert_eq!(img.get_pixel(0, 0).0[3], 0, "corner is not transparent");
}
