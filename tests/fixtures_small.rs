//! Structure and topology tests on the small generated fixtures (tests/fixtures/*.step).

use std::path::PathBuf;

use step_brep::{DiagSink, load_structure};
use step_mesh::topo::BodyKind;
use step_p21::StepFile;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn cube_mm_structure_and_topology() {
    let file = StepFile::open(fixture("cube_mm.step")).unwrap();
    assert_eq!(file.header().ap(), step_p21::Ap::Ap203);
    assert!(file.errors().is_empty());
    let s = load_structure(&file, true);
    assert_eq!(s.assembly.roots.len(), 1);
    assert_eq!(s.assembly.node(s.assembly.roots[0]).name, "Cube 10mm");
    assert_eq!(s.assembly.shapes.len(), 1);
    assert_eq!(s.assembly.instances.len(), 1);
    assert_eq!(s.assembly.shape(step_brep::ShapeId(0)).units.length, 1.0);
    assert_eq!(s.styles.body.len(), 1);
    assert_eq!(s.styles.face.len(), 1);
    let sink = DiagSink::default();
    let topos = s.extract_all(&file, &sink);
    let d = sink.into_inner();
    assert_eq!(d.total(), 0, "{d:?}");
    let t = &topos[0];
    assert_eq!(t.bodies.len(), 1);
    assert_eq!(t.bodies[0].kind, BodyKind::Solid);
    assert_eq!(t.faces.len(), 6);
    assert_eq!(t.edges.len(), 12);
    assert_eq!(t.vertices.len(), 8);
    assert!(t.faces.iter().all(|f| f.surface.is_planar()));
    assert!(t.faces.iter().all(|f| f.loops.len() == 1 && t.loops[f.loops[0].idx()].edges.len() == 4));
    let bb = t.vertex_bbox();
    assert!((bb.size() - glam::DVec3::splat(10.0)).length() < 1e-9);
    // colours: body red, +Z face green override
    let red = [200, 30, 30, 255];
    let green = [30, 200, 30, 255];
    let greens = t.faces.iter().filter(|f| f.color == green).count();
    let reds = t.faces.iter().filter(|f| f.color == red).count();
    assert_eq!((reds, greens), (5, 1));
    assert!((t.tol - 1e-3).abs() < 1e-12);
}

#[test]
fn two_cubes_inch_assembly_transforms_and_units() {
    let file = StepFile::open(fixture("two_cubes_assembly_inch.step")).unwrap();
    let s = load_structure(&file, true);
    let asm = &s.assembly;
    assert_eq!(asm.roots.len(), 1);
    assert_eq!(asm.node(asm.roots[0]).name, "Two cubes");
    assert_eq!(asm.nodes.len(), 3);
    assert_eq!(asm.occurrence_count, 2);
    assert_eq!(asm.shapes.len(), 1, "the cube is one shared shape");
    assert_eq!(asm.instances.len(), 2);
    let mut xs: Vec<f64> = asm.instances.iter().map(|i| i.world.translation.x).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((xs[0]).abs() < 1e-9);
    assert!((xs[1] - 76.2).abs() < 1e-9, "3 inch = 76.2 mm, got {}", xs[1]);
    assert!(asm.nodes.iter().any(|n| n.name == "Cube 10mm [C1]"));
    assert!(asm.nodes.iter().any(|n| n.name == "Cube 10mm [C2]"));
    assert_eq!(s.diags.total(), 0, "{:?}", s.diags);
    let sink = DiagSink::default();
    let topos = s.extract_all(&file, &sink);
    assert_eq!(topos[0].faces.len(), 6);
    let mut bb = step_mesh::Aabb::EMPTY;
    for inst in &asm.instances {
        bb.union(&topos[inst.shape.0 as usize].vertex_bbox().transformed(&inst.world));
    }
    assert!((bb.max.x - 86.2).abs() < 1e-9);
    assert!((bb.size().y - 10.0).abs() < 1e-9);
}

#[test]
fn step_transparency_survives_tessellation_and_cache() {
    let source = include_str!("fixtures/cube_mm.step")
        .replace("#160=SURFACE_SIDE_STYLE('',(#159));", "#160=SURFACE_SIDE_STYLE('',(#159,#175));")
        .replace("ENDSEC;\nEND-ISO", "#175=SURFACE_STYLE_RENDERING_WITH_PROPERTIES(.NORMAL_SHADING.,#156,(#176));\n#176=SURFACE_STYLE_TRANSPARENT(0.65);\nENDSEC;\nEND-ISO");
    let file = StepFile::from_bytes(source.into_bytes()).unwrap();
    let structure = load_structure(&file, true);
    assert_eq!(structure.styles.body[&step_p21::EntityId(155)], [200, 30, 30, 89]);
    let (topology, _) = structure.extract(&file, step_brep::ShapeId(0));
    let mesh = step_mesh::tessellate(&topology, &step_mesh::TessParams::PREVIEW);
    assert_eq!(mesh.stats.faces_skipped, 0);
    assert_eq!(mesh.bodies[0].face_ranges.iter().filter(|f| f.color[3] == 89).count(), 5);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("build/test-opacity-cache");
    let hash = step_cache::file_hash(file.bytes());
    step_cache::store(&dir, &hash, &step_mesh::TessParams::PREVIEW, "opacity", &[Some(mesh.clone())], &[172]).unwrap();
    let cached = step_cache::lookup(&dir, &hash, &step_mesh::TessParams::PREVIEW).unwrap().unwrap();
    assert_eq!(cached.meshes[0].as_ref().unwrap().bodies[0].face_ranges, mesh.bodies[0].face_ranges);
}
