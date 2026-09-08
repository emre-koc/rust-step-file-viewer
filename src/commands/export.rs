use std::path::Path;

use step_export::{ExportOpts, Scene};

use crate::pipeline::{LoadOpts, load_all};

pub fn run(file: &Path, out: &Path, flatten: bool, ascii: bool, opts: &LoadOpts) -> anyhow::Result<()> {
    let loaded = load_all(file, opts)?;
    let scene = Scene { assembly: &loaded.structure.assembly, meshes: &loaded.meshes, only_instances: None };
    let eo = ExportOpts { ascii, gltf_convention: !flatten };
    let stats = step_export::export(out, &scene, eo)?;
    let t = &loaded.timings;
    println!(
        "wrote {}: {} bodies, {} triangles, {} vertices, {:.1} MB  (index {:.0} ms, structure {:.0} ms, mesh {:.0} ms, cache: {:?})",
        out.display(),
        stats.bodies,
        stats.triangles,
        stats.vertices,
        stats.bytes as f64 / 1e6,
        t.index_ms,
        t.structure_ms,
        t.mesh_ms,
        loaded.cache
    );
    if loaded.diags.total() > 0 {
        println!("diagnostics: {}", loaded.diags.counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
    }
    let mesh_diags: std::collections::BTreeMap<String, u32> = loaded
        .meshes
        .iter()
        .flatten()
        .flat_map(|m| m.diags.iter())
        .fold(Default::default(), |mut acc: std::collections::BTreeMap<String, u32>, d| {
            *acc.entry(format!("{:?}", d.kind)).or_insert(0) += 1;
            acc
        });
    if !mesh_diags.is_empty() {
        println!("mesh diagnostics: {}", mesh_diags.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}
