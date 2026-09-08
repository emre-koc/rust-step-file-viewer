//! End-to-end tests that run the built `stepview` binary on the small fixtures.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_stepview"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stepview-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn info_json_reports_structure_and_mesh() {
    let out = bin().args(["info", "--json", "--mesh", "--no-cache"]).arg(fixture("two_cubes_assembly_inch.step")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ap"], "AP203");
    assert_eq!(v["entities"], 203);
    assert_eq!(v["products"], 2);
    assert_eq!(v["occurrences"], 2);
    assert_eq!(v["instances"], 2);
    assert_eq!(v["shapes"], 1);
    assert_eq!(v["faces"], 6);
    assert_eq!(v["faces_by_surface"]["plane"], 6);
    assert_eq!(v["mesh_faces_skipped"], 0);
    assert_eq!(v["mesh_triangles_unique"], 12);
    assert_eq!(v["mesh_triangles_drawn"], 24);
    assert_eq!(v["bbox_size_mm"][0], 86.2);
    assert_eq!(v["roots"][0], "Two cubes");
}

#[test]
fn render_produces_a_non_empty_png() {
    let png = tmp("cube.png");
    let out = bin().args(["render", "--no-cache", "--size", "320x240", "--view", "iso", "-o"]).arg(&png).arg(fixture("cube_mm.step")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let img = image::open(&png).unwrap().to_rgba8();
    assert_eq!((img.width(), img.height()), (320, 240));
    let bg = *img.get_pixel(2, 2);
    let differing = img.pixels().filter(|p| **p != bg).count();
    assert!(differing > 320 * 240 / 20, "only {differing} non-background pixels");
    // the body is red with a green top face: both colours must appear
    let reddish = img.pixels().filter(|p| p[0] > 120 && p[1] < 90 && p[2] < 90).count();
    let greenish = img.pixels().filter(|p| p[1] > 120 && p[0] < 90 && p[2] < 90).count();
    assert!(reddish > 200 && greenish > 200, "red {reddish} green {greenish}");
}

#[test]
fn render_only_filter_and_yaw_pitch_view() {
    let png = tmp("one_cube.png");
    let out = bin()
        .args(["render", "--no-cache", "--size", "200x200", "--view", "45,30", "--only", "C2", "-o"])
        .arg(&png)
        .arg(fixture("two_cubes_assembly_inch.step"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1 of 2 instances"), "{text}");
    let bad = bin().args(["render", "--no-cache", "--only", "nonexistent", "-o"]).arg(tmp("x.png")).arg(fixture("cube_mm.step")).output().unwrap();
    assert!(!bad.status.success());
}

#[test]
fn export_stl_obj_glb() {
    let stl = tmp("cube.stl");
    let out = bin().args(["export", "--no-cache", "-o"]).arg(&stl).arg(fixture("cube_mm.step")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(&stl).unwrap();
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap());
    assert_eq!(count, 12);
    assert_eq!(bytes.len(), 84 + 12 * 50);

    let glb = tmp("two.glb");
    let out = bin().args(["export", "--no-cache", "-o"]).arg(&glb).arg(fixture("two_cubes_assembly_inch.step")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(&glb).unwrap();
    assert_eq!(&bytes[0..4], b"glTF");
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
    assert_eq!(json["meshes"].as_array().unwrap().len(), 1, "one shared mesh");
    let mesh_nodes = json["nodes"].as_array().unwrap().iter().filter(|n| n.get("mesh").is_some()).count();
    assert_eq!(mesh_nodes, 2, "two instances reference it");

    let obj = tmp("cube.obj");
    let out = bin().args(["export", "--no-cache", "-o"]).arg(&obj).arg(fixture("cube_mm.step")).output().unwrap();
    assert!(out.status.success());
    let text = std::fs::read_to_string(&obj).unwrap();
    assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 12);
    assert!(obj.with_extension("mtl").exists());
}
