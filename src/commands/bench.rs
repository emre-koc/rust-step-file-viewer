use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use step_brep::DiagSink;

use crate::pipeline::LoadOpts;

pub fn run(file: &Path, iters: u32, stage: &str, opts: &LoadOpts) -> anyhow::Result<()> {
    let iters = iters.max(1);
    let all = stage == "all";
    let mut rows: Vec<(String, Vec<f32>)> = Vec::new();
    let mut add = |name: &str, v: Vec<f32>| rows.push((name.to_string(), v));

    // index
    let mut idx = Vec::new();
    let mut file_opt = None;
    for _ in 0..iters {
        let t = Instant::now();
        let f = crate::pipeline::index(file)?;
        idx.push(t.elapsed().as_secs_f32() * 1000.0);
        file_opt = Some(f);
    }
    let f = file_opt.unwrap();
    add("index", idx);
    let mb = f.bytes().len() as f32 / 1e6;

    if all || stage == "structure" || stage == "extract" || stage == "tess" || stage == "eager" {
        let mut st = Vec::new();
        let mut s_opt = None;
        for _ in 0..iters {
            let t = Instant::now();
            let s = step_brep::load_structure(&f, opts.colors);
            st.push(t.elapsed().as_secs_f32() * 1000.0);
            s_opt = Some(s);
        }
        let s = s_opt.unwrap();
        add("structure", st);

        if all || stage == "extract" || stage == "tess" {
            let mut ex = Vec::new();
            let mut topos_faces = 0usize;
            for _ in 0..iters {
                let sink = DiagSink::default();
                let t = Instant::now();
                let topos = s.extract_all(&f, &sink);
                ex.push(t.elapsed().as_secs_f32() * 1000.0);
                topos_faces = topos.iter().map(|t| t.faces.len()).sum();
            }
            add("extract", ex);
            println!("faces: {topos_faces}");

            if all || stage == "tess" {
                let mut te = Vec::new();
                let mut tris = 0u64;
                for _ in 0..iters {
                    let sink = DiagSink::default();
                    let cancel = AtomicBool::new(false);
                    let t = Instant::now();
                    let meshes = crate::pipeline::tessellate_shapes(&f, &s, &opts.params, &cancel, &sink, &|_, _, _, _| {});
                    te.push(t.elapsed().as_secs_f32() * 1000.0);
                    tris = meshes.iter().flatten().map(|m| m.triangle_count() as u64).sum();
                }
                add("extract+tess", te);
                println!("triangles (unique shapes): {tris}");
            }
        }
    }

    println!("{:<14} {:>9} {:>9} {:>9}   ({} iters, {:.1} MB)", "stage", "min ms", "median", "max ms", iters, mb);
    for (name, mut v) in rows {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = v[v.len() / 2];
        println!("{:<14} {:>9.1} {:>9.1} {:>9.1}", name, v[0], med, v[v.len() - 1]);
    }
    Ok(())
}
