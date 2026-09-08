//! `step-brep`: from an indexed STEP file to (a) an assembly tree with instances and colours and
//! (b) per-shape B-rep topology ready for tessellation.

pub mod assembly;
pub mod diag;
pub mod style;
pub mod topology;

use std::time::Instant;

use rayon::prelude::*;
use step_mesh::topo::ShapeTopology;
use step_model::Model;
use step_p21::StepFile;

pub use assembly::{Assembly, Instance, Node, NodeId, ShapeId, ShapeRef};
pub use diag::{DiagKind, DiagSink, Diagnostic, Diagnostics};
pub use style::{Rgba8, StyleIndex};

/// Everything known about a file before geometry is tessellated.
pub struct Structure {
    pub assembly: Assembly,
    pub styles: StyleIndex,
    pub diags: Diagnostics,
    pub styles_ms: f32,
    pub assembly_ms: f32,
}

/// Build the product structure and colour index.
pub fn load_structure(file: &StepFile, with_colors: bool) -> Structure {
    let model = Model::new(file);
    let mut diags = Diagnostics::default();
    let t0 = Instant::now();
    let styles = if with_colors { style::build_styles(&model, &mut diags) } else { StyleIndex::default() };
    let styles_ms = t0.elapsed().as_secs_f32() * 1000.0;
    let t1 = Instant::now();
    let assembly = assembly::build(&model, &mut diags);
    let assembly_ms = t1.elapsed().as_secs_f32() * 1000.0;
    Structure { assembly, styles, diags, styles_ms, assembly_ms }
}

impl Structure {
    /// Display name for a shape: its owning product's first node name, or the representation name.
    pub fn shape_name(&self, shape: ShapeId) -> String {
        let s = self.assembly.shape(shape);
        self.assembly
            .by_shape
            .get(shape.0 as usize)
            .and_then(|v| v.first())
            .map(|&i| self.assembly.nodes[self.assembly.instances[i as usize].node.0 as usize].name.clone())
            .unwrap_or_else(|| format!("rep{}", s.rep.0))
    }

    /// Extract one shape's topology.
    pub fn extract(&self, file: &StepFile, shape: ShapeId) -> (ShapeTopology, Diagnostics) {
        let model = Model::new(file);
        let name = self.shape_name(shape);
        topology::extract_shape(&model, self.assembly.shape(shape), &name, &self.styles)
    }

    /// Extract every shape in parallel, most-instanced first. Returns topologies indexed by shape.
    pub fn extract_all(&self, file: &StepFile, sink: &DiagSink) -> Vec<ShapeTopology> {
        let n = self.assembly.shapes.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.assembly.by_shape[i].len() as u64 * (1 + self.assembly.shapes[i].body_items as u64)));
        let mut results: Vec<Option<ShapeTopology>> = (0..n).map(|_| None).collect();
        let extracted: Vec<(usize, ShapeTopology)> = order
            .par_iter()
            .map(|&i| {
                let (t, d) = self.extract(file, ShapeId(i as u32));
                sink.merge(d);
                (i, t)
            })
            .collect();
        for (i, t) in extracted {
            results[i] = Some(t);
        }
        results.into_iter().map(|t| t.unwrap_or_default()).collect()
    }
}
