//! `stepview info`: file summary; structure and topology when requested.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Context;
use step_brep::{DiagSink, Structure};
use step_mesh::mesh::Aabb;
use step_p21::{EntityId, StepFile};

pub struct InfoOpts {
    pub json: bool,
    pub types: bool,
    pub dump: Option<String>,
    pub tree: bool,
    pub no_geometry: bool,
    pub mesh: bool,
    pub colors: bool,
    pub params: step_mesh::TessParams,
}

pub fn run(path: &Path, opts: &InfoOpts) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let file = StepFile::open(path).with_context(|| format!("opening {}", path.display()))?;
    let open_ms = t0.elapsed().as_secs_f32() * 1000.0;

    if let Some(dump) = &opts.dump {
        return dump_entity(&file, dump);
    }

    let h = file.header();
    let st = file.stats();
    let mut o = serde_json::Map::new();
    o.insert("file".into(), path.display().to_string().into());
    o.insert("bytes".into(), (file.bytes().len() as u64).into());
    o.insert("schema".into(), h.schema.clone().into());
    o.insert("ap".into(), h.ap().label().into());
    o.insert("name".into(), h.name.clone().into());
    o.insert("time_stamp".into(), h.time_stamp.clone().into());
    o.insert("preprocessor".into(), h.preprocessor_version.clone().into());
    o.insert("originating_system".into(), h.originating_system.clone().into());
    o.insert("entities".into(), st.entities.into());
    o.insert("complex_entities".into(), st.complex.into());
    o.insert("max_id".into(), file.max_id().into());
    o.insert("chunks".into(), st.chunks.into());
    o.insert("parse_errors".into(), st.errors.into());
    o.insert("index_ms".into(), serde_json::json!(round1(file.index_ms())));

    if !opts.json {
        println!("file:        {} ({:.1} MB)", path.display(), file.bytes().len() as f64 / 1e6);
        println!("schema:      {}  [{}]", h.schema.join(", "), h.ap().label());
        println!("name:        {}", h.name);
        println!("written:     {}  by {} / {}", h.time_stamp, h.preprocessor_version, h.originating_system);
        println!("entities:    {}  ({} complex, max id #{}, {} chunks, {} parse errors)", st.entities, st.complex, file.max_id(), st.chunks, st.errors);
        println!("index time:  {:.1} ms  (open+index {:.1} ms)", file.index_ms(), open_ms);
        if st.sequential_fallback {
            println!("note:        parallel chunking fell back to a sequential scan");
        }
        for (off, msg) in file.errors().iter().take(10) {
            println!("  parse error at byte {off}: {msg}");
        }
    }

    if opts.types {
        if opts.json {
            let types: serde_json::Map<String, serde_json::Value> = file.type_counts().iter().map(|(n, c)| (n.clone(), (*c).into())).collect();
            o.insert("types".into(), types.into());
            let ctypes: serde_json::Map<String, serde_json::Value> = file.complex_type_counts().iter().map(|(n, c)| (n.clone(), (*c).into())).collect();
            o.insert("complex_part_types".into(), ctypes.into());
        } else {
            println!("\nsimple entity types:");
            for (n, c) in file.type_counts() {
                println!("{c:>9}  {n}");
            }
            if !file.complex_type_counts().is_empty() {
                println!("\npartial entity types inside complex instances:");
                for (n, c) in file.complex_type_counts() {
                    println!("{c:>9}  {n}");
                }
            }
        }
    }

    // ----- product structure -----
    let structure = step_brep::load_structure(&file, opts.colors);
    let asm = &structure.assembly;
    let mut units: BTreeMap<String, u32> = BTreeMap::new();
    for s in &asm.shapes {
        *units.entry(s.units.length_name_str().to_string()).or_insert(0) += 1;
    }
    o.insert("products".into(), asm.product_count.into());
    o.insert("occurrences".into(), asm.occurrence_count.into());
    o.insert("nodes".into(), (asm.nodes.len() as u64).into());
    o.insert("roots".into(), asm.roots.iter().map(|r| asm.node(*r).name.clone()).collect::<Vec<_>>().into());
    o.insert("shapes".into(), (asm.shapes.len() as u64).into());
    o.insert("instances".into(), (asm.instances.len() as u64).into());
    o.insert("max_depth".into(), asm.max_depth().into());
    o.insert("shape_units".into(), units.iter().map(|(k, v)| (k.clone(), serde_json::Value::from(*v))).collect::<serde_json::Map<_, _>>().into());
    o.insert("styled_faces".into(), (structure.styles.face.len() as u64).into());
    o.insert("styled_bodies".into(), (structure.styles.body.len() as u64).into());
    o.insert("structure_ms".into(), serde_json::json!(round1(structure.styles_ms + structure.assembly_ms)));
    if !opts.json {
        println!("\nproducts:    {}   occurrences: {}   nodes: {}   roots: {}", asm.product_count, asm.occurrence_count, asm.nodes.len(), asm.roots.len());
        println!("shapes:      {} unique representations, {} instances, tree depth {}", asm.shapes.len(), asm.instances.len(), asm.max_depth());
        println!("units:       {}", units.iter().map(|(k, v)| format!("{v}x {k}")).collect::<Vec<_>>().join(", "));
        println!(
            "colours:     {} face, {} shell, {} body, {} other  ({} styled items, {} curve-only skipped)",
            structure.styles.face.len(),
            structure.styles.shell.len(),
            structure.styles.body.len(),
            structure.styles.other.len(),
            structure.styles.styled_items_seen,
            structure.styles.styled_items_skipped_curves
        );
        println!("structure:   styles {:.1} ms + assembly {:.1} ms", structure.styles_ms, structure.assembly_ms);
        for r in &asm.roots {
            println!("root:        {}", asm.node(*r).name);
        }
    }
    if opts.tree && !opts.json {
        println!("\nassembly tree:");
        print_tree(&structure, 0, 400);
    }

    // ----- topology -----
    let mut diags = structure.diags.clone();
    if !opts.no_geometry {
        let t1 = Instant::now();
        let sink = DiagSink::default();
        let topos = structure.extract_all(&file, &sink);
        let extract_ms = t1.elapsed().as_secs_f32() * 1000.0;
        diags.merge(sink.into_inner());

        let mut faces_by_kind: BTreeMap<&'static str, u32> = BTreeMap::new();
        let (mut bodies, mut solids, mut shells, mut faces, mut edges, mut vertices) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        let mut bbox = Aabb::EMPTY;
        for (i, t) in topos.iter().enumerate() {
            bodies += t.bodies.len() as u64;
            solids += t.bodies.iter().filter(|b| b.kind == step_mesh::topo::BodyKind::Solid).count() as u64;
            shells += t.shells.len() as u64;
            faces += t.faces.len() as u64;
            edges += t.edges.len() as u64;
            vertices += t.vertices.len() as u64;
            for f in &t.faces {
                *faces_by_kind.entry(f.surface.kind()).or_insert(0) += 1;
            }
            let local = t.vertex_bbox();
            for &inst in &asm.by_shape[i] {
                bbox.union(&local.transformed(&asm.instances[inst as usize].world));
            }
        }
        o.insert("bodies".into(), bodies.into());
        o.insert("solids".into(), solids.into());
        o.insert("shells".into(), shells.into());
        o.insert("faces".into(), faces.into());
        o.insert("edges".into(), edges.into());
        o.insert("vertices".into(), vertices.into());
        o.insert("faces_by_surface".into(), faces_by_kind.iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(*v))).collect::<serde_json::Map<_, _>>().into());
        if !bbox.is_empty() {
            o.insert("bbox_min_mm".into(), serde_json::json!([round3(bbox.min.x), round3(bbox.min.y), round3(bbox.min.z)]));
            o.insert("bbox_max_mm".into(), serde_json::json!([round3(bbox.max.x), round3(bbox.max.y), round3(bbox.max.z)]));
            o.insert("bbox_size_mm".into(), serde_json::json!([round3(bbox.size().x), round3(bbox.size().y), round3(bbox.size().z)]));
        }
        o.insert("extract_ms".into(), serde_json::json!(round1(extract_ms)));
        if !opts.json {
            println!("\ntopology:    {bodies} bodies ({solids} solid), {shells} shells, {faces} faces, {edges} edges, {vertices} vertices  (extract {extract_ms:.1} ms)");
            println!("faces:       {}", faces_by_kind.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", "));
            if !bbox.is_empty() {
                let s = bbox.size();
                println!("bbox (mm):   min [{:.2}, {:.2}, {:.2}]  max [{:.2}, {:.2}, {:.2}]  size [{:.2} x {:.2} x {:.2}]", bbox.min.x, bbox.min.y, bbox.min.z, bbox.max.x, bbox.max.y, bbox.max.z, s.x, s.y, s.z);
            }
        }
    }

    // ----- mesh -----
    if opts.mesh && !opts.no_geometry {
        let t2 = Instant::now();
        let sink = DiagSink::default();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let meshes = crate::pipeline::tessellate_shapes(&file, &structure, &opts.params, &cancel, &sink, &|_, _, _, _| {});
        let mesh_ms = t2.elapsed().as_secs_f32() * 1000.0;
        diags.merge(sink.into_inner());
        let mut stats = step_mesh::TessStats::default();
        let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
        let mut samples: Vec<String> = Vec::new();
        let mut unique_tris = 0u64;
        for m in meshes.iter().flatten() {
            stats.faces += m.stats.faces;
            stats.faces_ok += m.stats.faces_ok;
            stats.faces_fallback += m.stats.faces_fallback;
            stats.faces_skipped += m.stats.faces_skipped;
            stats.vertices += m.stats.vertices;
            stats.triangles += m.stats.triangles;
            stats.chord = m.stats.chord;
            unique_tris += m.triangle_count() as u64;
            for d in &m.diags {
                let k = format!("{:?}", d.kind);
                *kinds.entry(k.clone()).or_insert(0) += 1;
                if samples.len() < 30 && matches!(d.kind, step_mesh::DiagKind::DegenerateFace | step_mesh::DiagKind::TriangulationFallback | step_mesh::DiagKind::Panic | step_mesh::DiagKind::UnexpectedWinding) {
                    samples.push(format!("  {k} face #{} (level {}): {}", d.src, d.level, d.msg));
                }
            }
        }
        let drawn = crate::pipeline::instanced_triangles(&structure, &meshes);
        o.insert("mesh_ms".into(), serde_json::json!(round1(mesh_ms)));
        o.insert("mesh_triangles_unique".into(), unique_tris.into());
        o.insert("mesh_triangles_drawn".into(), drawn.into());
        o.insert("mesh_faces_ok".into(), stats.faces_ok.into());
        o.insert("mesh_faces_fallback".into(), stats.faces_fallback.into());
        o.insert("mesh_faces_skipped".into(), stats.faces_skipped.into());
        o.insert("mesh_chord_mm".into(), serde_json::json!(stats.chord));
        o.insert("mesh_diagnostics".into(), kinds.iter().map(|(k, v)| (k.clone(), serde_json::Value::from(*v))).collect::<serde_json::Map<_, _>>().into());
        if !opts.json {
            println!(
                "\nmesh:        {} faces → {} ok, {} fallback, {} skipped; {} unique tris ({} drawn), {} vertices, chord {:.4} mm  ({mesh_ms:.0} ms)",
                stats.faces, stats.faces_ok, stats.faces_fallback, stats.faces_skipped, unique_tris, drawn, stats.vertices, stats.chord
            );
            if !kinds.is_empty() {
                println!("mesh diags:  {}", kinds.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", "));
                for s in &samples {
                    println!("{s}");
                }
            }
        }
    }

    // ----- diagnostics -----
    o.insert("diagnostics".into(), diags.counts.iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(*v))).collect::<serde_json::Map<_, _>>().into());
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::Value::Object(o))?);
    } else {
        if diags.total() > 0 {
            println!("\ndiagnostics: {} total", diags.total());
            for (k, v) in &diags.counts {
                println!("{v:>9}  {k}");
            }
            for d in diags.samples.iter().take(12) {
                println!("  {:?} {}: {}", d.kind, d.entity.map(|e| e.to_string()).unwrap_or_default(), d.msg);
            }
        } else {
            println!("\ndiagnostics: none");
        }
    }
    Ok(())
}

fn print_tree(s: &Structure, _depth: usize, max_lines: usize) {
    let asm = &s.assembly;
    let mut lines = 0usize;
    let mut stack: Vec<(step_brep::NodeId, usize)> = asm.roots.iter().rev().map(|r| (*r, 0)).collect();
    while let Some((id, d)) = stack.pop() {
        let n = asm.node(id);
        if lines >= max_lines {
            println!("  ... ({} more nodes)", asm.nodes.len() - lines);
            break;
        }
        let shapes = if n.shapes.is_empty() { String::new() } else { format!("  [{} shape{}]", n.shapes.len(), if n.shapes.len() == 1 { "" } else { "s" }) };
        let t = n.local.translation;
        let pos = if n.parent.is_some() { format!("  @({:.1}, {:.1}, {:.1})", t.x, t.y, t.z) } else { String::new() };
        println!("{}{}{}{}", "  ".repeat(d), n.name, shapes, pos);
        lines += 1;
        for c in n.children.iter().rev() {
            stack.push((*c, d + 1));
        }
    }
}

fn dump_entity(file: &StepFile, dump: &str) -> anyhow::Result<()> {
    let id: u32 = dump.trim_start_matches('#').parse().context("--dump expects #<id>")?;
    let id = EntityId(id);
    match file.entity_source(id) {
        Some(src) => {
            println!("{}", src.trim());
            println!("type: {}", file.type_name(id).unwrap_or_default());
            if let Some(args) = file.args(id) {
                for (i, a) in args.iter().enumerate() {
                    println!("  [{i}] {a:?}");
                }
            }
        }
        None => println!("{id} not found"),
    }
    Ok(())
}

fn round1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
