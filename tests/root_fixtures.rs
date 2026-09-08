//! Integration tests against the shared STEP inputs in the workspace root. Each test skips (with a
//! message) when its fixture is absent so the suite is runnable elsewhere.

use std::path::PathBuf;
use std::time::Instant;

use std::sync::atomic::AtomicBool;

use step_brep::{DiagSink, Structure, load_structure};
use step_p21::StepFile;

/// Tessellate every shape and return (faces_ok, faces_skipped, unique triangles, elapsed ms).
fn mesh_all(file: &StepFile, s: &Structure) -> (u32, u32, u64, f32) {
    let sink = DiagSink::default();
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let order: Vec<usize> = (0..s.assembly.shapes.len()).collect();
    let mut ok = 0;
    let mut skipped = 0;
    let mut tris = 0u64;
    for i in order {
        let (topo, d) = s.extract(file, step_brep::ShapeId(i as u32));
        sink.merge(d);
        let m = step_mesh::tessellate(&topo, &step_mesh::TessParams::PREVIEW);
        ok += m.stats.faces_ok;
        skipped += m.stats.faces_skipped;
        tris += m.triangle_count() as u64;
    }
    (ok, skipped, tris, t.elapsed().as_secs_f32() * 1000.0)
}

fn root_fixture(rel: &str) -> Option<PathBuf> {
    let base = std::env::var_os("STEPVIEW_FIXTURES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    let p = base.join(rel);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipping: fixture {} not found", p.display());
        None
    }
}

struct Summary {
    faces: usize,
    edges: usize,
    vertices: usize,
    bodies: usize,
    by_kind: std::collections::BTreeMap<&'static str, usize>,
    bbox: step_mesh::Aabb,
    diags: u32,
    structure_ms: f32,
}

fn summarize(file: &StepFile, with_colors: bool) -> (step_brep::Structure, Summary) {
    let t0 = Instant::now();
    let s = load_structure(file, with_colors);
    let structure_ms = t0.elapsed().as_secs_f32() * 1000.0;
    let sink = DiagSink::default();
    let topos = s.extract_all(file, &sink);
    let mut sum = Summary { faces: 0, edges: 0, vertices: 0, bodies: 0, by_kind: Default::default(), bbox: step_mesh::Aabb::EMPTY, diags: 0, structure_ms };
    for (i, t) in topos.iter().enumerate() {
        sum.faces += t.faces.len();
        sum.edges += t.edges.len();
        sum.vertices += t.vertices.len();
        sum.bodies += t.bodies.len();
        for f in &t.faces {
            *sum.by_kind.entry(f.surface.kind()).or_insert(0) += 1;
        }
        let local = t.vertex_bbox();
        for &inst in &s.assembly.by_shape[i] {
            sum.bbox.union(&local.transformed(&s.assembly.instances[inst as usize].world));
        }
    }
    sum.diags = sink.into_inner().total() + s.diags.total();
    (s, sum)
}

#[test]
fn a114_solidworks_part() {
    let Some(p) = root_fixture("A-114.STEP") else { return };
    let file = StepFile::open(&p).unwrap();
    assert_eq!(file.len(), 3778);
    assert_eq!(file.stats().complex, 30);
    assert!(file.errors().is_empty());
    assert_eq!(file.header().originating_system, "SolidWorks 2021");
    let (s, sum) = summarize(&file, true);
    assert_eq!(s.assembly.product_count, 3);
    assert_eq!(s.assembly.node(s.assembly.roots[0]).name, "A-114");
    assert_eq!(sum.faces, 89);
    assert_eq!(sum.by_kind["cylinder"], 39);
    assert_eq!(sum.by_kind["plane"], 34);
    assert_eq!(sum.by_kind["nurbs"], 16);
    assert_eq!(sum.diags, 0);
    let size = sum.bbox.size();
    assert!((size.x - 45.0).abs() < 1e-6 && (size.z - 50.0).abs() < 1e-6, "size {size}");
    let (ok, skipped, tris, _) = mesh_all(&file, &s);
    assert_eq!((ok, skipped), (89, 0));
    assert!(tris > 2000, "tris {tris}");
}

#[test]
fn jetson_io_base_b_assembly() {
    let Some(p) = root_fixture("JETSON-ORIN-IO-BASE-B/jetson-orin-io-base-b_asm.stp") else { return };
    let file = StepFile::open(&p).unwrap();
    assert_eq!(file.len(), 578106);
    assert!(file.errors().is_empty());
    let (s, sum) = summarize(&file, true);
    assert_eq!(s.assembly.product_count, 170);
    assert_eq!(s.assembly.occurrence_count, 263);
    assert_eq!(s.assembly.node(s.assembly.roots[0]).name, "JETSON-ORIN-IO-BASE-B_ASM");
    assert_eq!(sum.faces, 13162);
    assert_eq!(sum.diags, 0);
    // mixed units: at least one inch and one mm shape
    let inch = s.assembly.shapes.iter().filter(|sh| (sh.units.length - 25.4).abs() < 1e-9).count();
    let mm = s.assembly.shapes.iter().filter(|sh| (sh.units.length - 1.0).abs() < 1e-9).count();
    assert!(inch >= 1 && mm >= 1, "inch={inch} mm={mm}");
    let size = sum.bbox.size();
    assert!(size.x > 100.0 && size.x < 106.0 && size.y > 88.0 && size.y < 94.0, "size {size}");
    let (ok, skipped, _, _) = mesh_all(&file, &s);
    assert!(skipped <= 4, "skipped {skipped}");
    assert!(ok >= 13158, "ok {ok}");
}

#[test]
fn jetson_orin_nano_assembly() {
    let Some(p) = root_fixture("JetsonOrinNano8GBSingleEthernet.stp") else { return };
    let t0 = Instant::now();
    let file = StepFile::open(&p).unwrap();
    let index_ms = t0.elapsed().as_secs_f32() * 1000.0;
    assert_eq!(file.len(), 1385512);
    assert_eq!(file.stats().complex, 2528);
    assert!(file.errors().is_empty());
    assert_eq!(file.header().name, "000_ARVALA_TLA_ASM");
    let (s, sum) = summarize(&file, true);
    assert_eq!(s.assembly.product_count, 132);
    assert_eq!(s.assembly.occurrence_count, 816);
    assert_eq!(s.assembly.nodes.len(), 817);
    assert_eq!(s.assembly.node(s.assembly.roots[0]).name, "000_ARVALA_TLA_ASM");
    assert_eq!(sum.faces, 31694);
    assert_eq!(sum.edges, 86925);
    assert_eq!(sum.vertices, 57257);
    assert_eq!(sum.bodies, 759);
    assert_eq!(sum.by_kind["plane"], 24329);
    assert_eq!(sum.by_kind["cylinder"], 6381);
    assert_eq!(sum.by_kind["cone"], 232);
    assert_eq!(sum.by_kind["torus"], 181);
    assert_eq!(sum.by_kind["sphere"], 83);
    assert_eq!(sum.by_kind["extrusion"], 66);
    assert_eq!(sum.by_kind["nurbs"], 422);
    assert!(!sum.by_kind.contains_key("unsupported"));
    assert_eq!(sum.diags, 0);
    assert!(s.styles.face.len() > 10_000);
    let (ok, skipped, tris, mesh_ms) = mesh_all(&file, &s);
    assert_eq!(skipped, 0, "skipped faces");
    assert_eq!(ok, 31694);
    assert!(tris > 400_000, "tris {tris}");
    // budgets (release builds only; debug is slower)
    if !cfg!(debug_assertions) {
        assert!(index_ms < 300.0, "index {index_ms} ms");
        assert!(sum.structure_ms < 250.0, "structure {} ms", sum.structure_ms);
        assert!(mesh_ms < 3000.0, "mesh {mesh_ms} ms (sequential)");
    }
}
