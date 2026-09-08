//! Tessellator tests. Everything is built from small synthetic B-reps — no fixture files.

use glam::DVec3;
use rustc_hash::FxHashMap;

use crate::discretize::{Tol, discretize_all};
use crate::geom::{Curve3, Frame, NurbsCurve, NurbsSurface, Surface};
use crate::mesh::{BodyMesh, DiagKind, ShapeMesh, TessParams};
use crate::topo::{
    Body, BodyKind, Edge, EdgeId, Face, FaceId, Loop, LoopId, OrientedEdge, ShapeTopology, Shell,
    Vertex, VertexId,
};

const TAU: f64 = std::f64::consts::TAU;

// ------------------------------------------------------------------------------------------
// builders
// ------------------------------------------------------------------------------------------

fn frame_z() -> Frame {
    Frame::IDENTITY
}

fn frame_at(o: DVec3) -> Frame {
    Frame { o, ..Frame::IDENTITY }
}

#[derive(Default)]
struct B {
    t: ShapeTopology,
}

impl B {
    fn new() -> B {
        let mut b = B::default();
        b.t.tol = 1e-4;
        b.t.name = "test".into();
        b
    }

    fn vertex(&mut self, p: DVec3) -> VertexId {
        for (i, v) in self.t.vertices.iter().enumerate() {
            if v.p.distance(p) < 1e-9 {
                return VertexId(i as u32);
            }
        }
        self.t.vertices.push(Vertex { p, src: self.t.vertices.len() as u32 + 1 });
        VertexId(self.t.vertices.len() as u32 - 1)
    }

    fn push_edge(&mut self, curve: Curve3, a: DVec3, b: DVec3, same_sense: bool) -> EdgeId {
        let v0 = self.vertex(a);
        let v1 = self.vertex(b);
        let src = self.t.edges.len() as u32 + 1000;
        self.t.edges.push(Edge { curve, v0, v1, same_sense, src });
        EdgeId(self.t.edges.len() as u32 - 1)
    }

    fn line_edge(&mut self, a: DVec3, b: DVec3) -> EdgeId {
        let d = (b - a).try_normalize().unwrap_or(DVec3::X);
        self.push_edge(Curve3::Line { p: a, d }, a, b, true)
    }

    /// Reuse an existing straight edge between the same two points; returns `(edge, reversed)`.
    fn line_between(&mut self, a: DVec3, b: DVec3) -> (EdgeId, bool) {
        for i in 0..self.t.edges.len() {
            if !matches!(self.t.edges[i].curve, Curve3::Line { .. }) {
                continue;
            }
            let p0 = self.t.vertices[self.t.edges[i].v0.idx()].p;
            let p1 = self.t.vertices[self.t.edges[i].v1.idx()].p;
            if p0.distance(a) < 1e-9 && p1.distance(b) < 1e-9 {
                return (EdgeId(i as u32), false);
            }
            if p0.distance(b) < 1e-9 && p1.distance(a) < 1e-9 {
                return (EdgeId(i as u32), true);
            }
        }
        (self.line_edge(a, b), false)
    }

    /// Circular arc from `a0` to `a1` degrees in the given frame.
    fn circle_edge(&mut self, f: Frame, r: f64, a0: f64, a1: f64) -> EdgeId {
        let (t0, t1) = (a0.to_radians(), a1.to_radians());
        let at = |t: f64| f.from_local(DVec3::new(r * t.cos(), r * t.sin(), 0.0));
        self.push_edge(Curve3::Circle { f, r }, at(t0), at(t1), t1 >= t0)
    }

    fn poly_edge(&mut self, pts: Vec<DVec3>) -> EdgeId {
        let a = pts[0];
        let b = pts[pts.len() - 1];
        self.push_edge(Curve3::Polyline(pts), a, b, true)
    }

    fn bspline_edge(&mut self, ctrl: &[DVec3], deg: usize) -> EdgeId {
        let c = bspline(ctrl, deg, None);
        let (a, b) = c.domain();
        let (pa, pb) = (c.point(a), c.point(b));
        self.push_edge(Curve3::Nurbs(Box::new(c)), pa, pb, true)
    }

    fn loop_(&mut self, edges: &[(EdgeId, bool)], is_outer: bool) -> LoopId {
        let edges = edges.iter().map(|(e, r)| OrientedEdge { edge: *e, reversed: *r }).collect();
        self.t.loops.push(Loop { edges, is_outer });
        LoopId(self.t.loops.len() as u32 - 1)
    }

    /// A loop of straight segments through the given corners (edges are shared between faces).
    fn poly_loop(&mut self, corners: &[DVec3], is_outer: bool) -> LoopId {
        let mut oes = Vec::with_capacity(corners.len());
        for i in 0..corners.len() {
            let (e, rev) = self.line_between(corners[i], corners[(i + 1) % corners.len()]);
            oes.push((e, rev));
        }
        self.loop_(&oes, is_outer)
    }

    fn face(&mut self, surface: Surface, same_sense: bool, loops: &[LoopId]) -> FaceId {
        let src = self.t.faces.len() as u32 + 10_000;
        self.t.faces.push(Face {
            surface,
            loops: loops.to_vec(),
            same_sense,
            color: [200, 200, 200, 255],
            src,
        });
        FaceId(self.t.faces.len() as u32 - 1)
    }

    fn finish(mut self, faces: &[FaceId], closed: bool) -> ShapeTopology {
        self.t.shells.push(Shell { faces: faces.to_vec(), closed, src: 1 });
        self.t.bodies.push(Body {
            shells: vec![crate::topo::ShellId(0)],
            kind: if closed { BodyKind::Solid } else { BodyKind::Sheet },
            name: "body".into(),
            src: 2,
        });
        self.t
    }

    fn sheet(self, faces: &[FaceId]) -> ShapeTopology {
        self.finish(faces, false)
    }

    fn solid(self, faces: &[FaceId]) -> ShapeTopology {
        self.finish(faces, true)
    }
}

/// Clamped uniform B-spline curve through the given control points.
fn bspline(ctrl: &[DVec3], deg: usize, weights: Option<&[f64]>) -> NurbsCurve {
    let n = ctrl.len();
    let p = deg.min(n - 1);
    let inner = n - p - 1;
    let mut mults = vec![(p + 1) as i64];
    let mut knots = vec![0.0];
    for i in 1..=inner {
        mults.push(1);
        knots.push(i as f64 / (inner + 1) as f64);
    }
    mults.push((p + 1) as i64);
    knots.push(1.0);
    NurbsCurve::from_step(p, ctrl, weights, &mults, &knots)
}

// ------------------------------------------------------------------------------------------
// mesh helpers
// ------------------------------------------------------------------------------------------

fn pos(b: &BodyMesh, i: u32) -> DVec3 {
    let p = b.positions[i as usize];
    DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)
}

fn world(b: &BodyMesh, i: u32) -> DVec3 {
    pos(b, i) + b.origin
}

fn mesh_area(m: &ShapeMesh) -> f64 {
    let mut a = 0.0;
    for b in &m.bodies {
        for t in b.indices.chunks_exact(3) {
            a += 0.5 * (pos(b, t[1]) - pos(b, t[0])).cross(pos(b, t[2]) - pos(b, t[0])).length();
        }
    }
    a
}

fn signed_volume(b: &BodyMesh) -> f64 {
    let mut v = 0.0;
    for t in b.indices.chunks_exact(3) {
        v += pos(b, t[0]).dot(pos(b, t[1]).cross(pos(b, t[2]))) / 6.0;
    }
    v
}

/// Every directed edge appears exactly once and its opposite exactly once, and V − E + F == 2.
fn is_closed_manifold(b: &BodyMesh) -> Result<(), String> {
    let mut half: FxHashMap<(u32, u32), i32> = FxHashMap::default();
    for t in b.indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, c) = (t[k], t[(k + 1) % 3]);
            *half.entry((a, c)).or_insert(0) += 1;
        }
    }
    let mut boundary = 0;
    for ((a, c), n) in &half {
        if *n != 1 {
            return Err(format!("edge {a}->{c} used {n} times"));
        }
        if half.get(&(*c, *a)).copied().unwrap_or(0) != 1 {
            boundary += 1;
        }
    }
    if boundary != 0 {
        return Err(format!("{boundary} boundary half-edges"));
    }
    let used: std::collections::BTreeSet<u32> = b.indices.iter().copied().collect();
    let v = used.len() as i64;
    let e = (half.len() / 2) as i64;
    let f = (b.indices.len() / 3) as i64;
    if v - e + f != 2 {
        return Err(format!("euler characteristic {} (V={v} E={e} F={f})", v - e + f));
    }
    Ok(())
}

fn boundary_edge_count(b: &BodyMesh) -> usize {
    let mut half: FxHashMap<(u32, u32), i32> = FxHashMap::default();
    for t in b.indices.chunks_exact(3) {
        for k in 0..3 {
            *half.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    half.keys().filter(|(a, c)| !half.contains_key(&(*c, *a))).count()
}

fn params() -> TessParams {
    TessParams { feature_edges: false, ..TessParams::PREVIEW }
}

fn welded() -> TessParams {
    TessParams { feature_edges: false, weld: true, ..TessParams::PREVIEW }
}

fn kinds(m: &ShapeMesh) -> Vec<DiagKind> {
    let mut k: Vec<DiagKind> = m.diags.iter().map(|d| d.kind).collect();
    k.sort();
    k.dedup();
    k
}

fn ngon_area(n: usize, r: f64) -> f64 {
    0.5 * n as f64 * r * r * (TAU / n as f64).sin()
}

// ------------------------------------------------------------------------------------------
// planes
// ------------------------------------------------------------------------------------------

#[test]
fn plane_three_holes() {
    let mut b = B::new();
    let outer = b.poly_loop(
        &[
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(100.0, 0.0, 0.0),
            DVec3::new(100.0, 50.0, 0.0),
            DVec3::new(0.0, 50.0, 0.0),
        ],
        true,
    );
    let mut loops = vec![outer];
    for cx in [25.0, 50.0, 75.0] {
        let pts: Vec<DVec3> = (0..12)
            .map(|i| {
                let a = TAU * i as f64 / 12.0;
                DVec3::new(cx + 5.0 * a.cos(), 25.0 + 5.0 * a.sin(), 0.0)
            })
            .collect();
        let mut closed = pts.clone();
        closed.push(pts[0]);
        let e = b.poly_edge(closed);
        loops.push(b.loop_(&[(e, false)], false));
    }
    let f = b.face(Surface::Plane { f: frame_z() }, true, &loops);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());

    assert_eq!(m.triangle_count(), 44, "expected 44 triangles, got {}", m.triangle_count());
    let expect = 100.0 * 50.0 - 3.0 * ngon_area(12, 5.0);
    assert!((mesh_area(&m) - expect).abs() / expect < 1e-6, "area {} != {expect}", mesh_area(&m));
    for n in &m.bodies[0].normals {
        assert!((n[2] - 1.0).abs() < 1e-6, "normal {n:?}");
    }
    assert!(m.diags.is_empty(), "unexpected diags {:?}", m.diags);
}

#[test]
fn same_sense_false() {
    let mut b = B::new();
    let l = b.poly_loop(
        &[
            DVec3::ZERO,
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(10.0, 10.0, 0.0),
            DVec3::new(0.0, 10.0, 0.0),
        ],
        true,
    );
    let f = b.face(Surface::Plane { f: frame_z() }, false, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let body = &m.bodies[0];
    assert!(m.triangle_count() >= 2);
    for n in &body.normals {
        assert!((n[2] + 1.0).abs() < 1e-6, "normal should be -z, got {n:?}");
    }
    for t in body.indices.chunks_exact(3) {
        let fnm = (pos(body, t[1]) - pos(body, t[0])).cross(pos(body, t[2]) - pos(body, t[0]));
        let n = body.normals[t[0] as usize];
        assert!(fnm.dot(DVec3::new(n[0] as f64, n[1] as f64, n[2] as f64)) > 0.0);
    }
}

// ------------------------------------------------------------------------------------------
// cylinders
// ------------------------------------------------------------------------------------------

fn full_cylinder_topo(r: f64, h: f64) -> (ShapeTopology, usize) {
    let mut b = B::new();
    let e0 = b.circle_edge(frame_z(), r, 0.0, 360.0);
    let e1 = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 360.0);
    let l0 = b.loop_(&[(e0, false)], true);
    let l1 = b.loop_(&[(e1, false)], false);
    let f = b.face(Surface::Cylinder { f: frame_z(), r }, true, &[l0, l1]);
    let topo = b.sheet(&[f]);
    let tol = Tol::new(&params(), topo.vertex_bbox().diagonal(), topo.tol);
    let n = discretize_all(&topo, &tol)[0].pts.len() - 1;
    (topo, n)
}

#[test]
fn full_cylinder() {
    let (topo, n) = full_cylinder_topo(10.0, 20.0);
    let m = crate::tessellate(&topo, &params());
    assert_eq!(m.triangle_count(), 2 * n, "expected a {n}-quad strip");
    let perimeter = n as f64 * 2.0 * 10.0 * (std::f64::consts::PI / n as f64).sin();
    assert!(
        (mesh_area(&m) - perimeter * 20.0).abs() < 1e-4,
        "area {} != {}",
        mesh_area(&m),
        perimeter * 20.0
    );
    assert_eq!(kinds(&m), vec![DiagKind::SeamSynthesized]);
}

#[test]
fn cylinder_half_with_hole() {
    let (r, h) = (10.0, 20.0);
    let mut b = B::new();
    let bottom = b.circle_edge(frame_z(), r, 0.0, 180.0);
    let top = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 180.0);
    let right = b.line_edge(DVec3::new(-r, 0.0, 0.0), DVec3::new(-r, 0.0, h));
    let left = b.line_edge(DVec3::new(r, 0.0, 0.0), DVec3::new(r, 0.0, h));
    let outer =
        b.loop_(&[(bottom, false), (right, false), (top, true), (left, true)], true);

    // A rectangular hole in (u, v): u ∈ [1.4, 1.8] rad, v ∈ [8, 12].
    let on = |u: f64, v: f64| DVec3::new(r * u.cos(), r * u.sin(), v);
    let hpts = vec![on(1.4, 8.0), on(1.8, 8.0), on(1.8, 12.0), on(1.4, 12.0), on(1.4, 8.0)];
    let he = b.poly_edge(hpts);
    let hole = b.loop_(&[(he, false)], false);

    let f = b.face(Surface::Cylinder { f: frame_z(), r }, true, &[outer, hole]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let expect = std::f64::consts::PI * r * h - 0.4 * r * 4.0;
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.01, "area {got} vs {expect}");
}

#[test]
fn cylinder_across_seam() {
    let (r, h) = (10.0, 20.0);
    let mut b = B::new();
    let at = |a: f64, z: f64| {
        let t: f64 = a.to_radians();
        DVec3::new(r * t.cos(), r * t.sin(), z)
    };
    let bottom = b.circle_edge(frame_z(), r, 150.0, 210.0);
    let top = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 150.0, 210.0);
    let right = b.line_edge(at(210.0, 0.0), at(210.0, h));
    let left = b.line_edge(at(150.0, 0.0), at(150.0, h));
    let outer = b.loop_(&[(bottom, false), (right, false), (top, true), (left, true)], true);
    let f = b.face(Surface::Cylinder { f: frame_z(), r }, true, &[outer]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let expect = (60.0 / 360.0) * TAU * r * h;
    let got = mesh_area(&m);
    // Wrapping the long way round would give five times this.
    assert!((got - expect).abs() / expect < 0.01, "area {got} vs {expect}");
}

// ------------------------------------------------------------------------------------------
// spheres, cones, tori
// ------------------------------------------------------------------------------------------

#[test]
fn sphere_octant() {
    let r = 10.0;
    let mut b = B::new();
    let equator = b.circle_edge(frame_z(), r, 0.0, 90.0);
    let mer_x = b.circle_edge(Frame::new(DVec3::ZERO, Some(DVec3::X), Some(DVec3::Y)), r, 0.0, 90.0);
    let mer_y = b.circle_edge(Frame::new(DVec3::ZERO, Some(DVec3::Y), Some(DVec3::Z)), r, 0.0, 90.0);
    let l = b.loop_(&[(equator, false), (mer_x, false), (mer_y, false)], true);
    let f = b.face(Surface::Sphere { f: frame_z(), r }, true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let expect = std::f64::consts::PI * r * r / 2.0;
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.02, "area {got} vs {expect}");
}

#[test]
fn sphere_cap() {
    let r = 10.0;
    let lat = 30.0f64.to_radians();
    let mut b = B::new();
    let e = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, r * lat.sin())), r * lat.cos(), 0.0, 360.0);
    let l = b.loop_(&[(e, false)], true);
    let f = b.face(Surface::Sphere { f: frame_z(), r }, true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let expect = TAU * r * r * (1.0 - lat.sin());
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.02, "area {got} vs {expect}");
    // No triangle may be fully collapsed onto the pole.
    let body = &m.bodies[0];
    for t in body.indices.chunks_exact(3) {
        let a = (pos(body, t[1]) - pos(body, t[0])).cross(pos(body, t[2]) - pos(body, t[0]));
        assert!(a.length() > 1e-9);
    }
}

#[test]
fn cone_with_apex() {
    let (r, alpha) = (10.0, std::f64::consts::FRAC_PI_4);
    let mut b = B::new();
    let e = b.circle_edge(frame_z(), r, 0.0, 360.0);
    let l = b.loop_(&[(e, false)], true);
    let f = b.face(Surface::Cone { f: frame_z(), r, alpha }, true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let slant = (r * r + r * r).sqrt();
    let expect = std::f64::consts::PI * r * slant;
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.02, "area {got} vs {expect}");
    for n in &m.bodies[0].normals {
        assert!(n.iter().all(|c| c.is_finite()), "non-finite normal {n:?}");
    }
}

fn torus_v_frame(u_deg: f64, big_r: f64) -> Frame {
    // Frame of the tube circle at angle `u`: x = radial, y = +Z.
    let t = u_deg.to_radians();
    let er = DVec3::new(t.cos(), t.sin(), 0.0);
    Frame::new(er * big_r, Some(er.cross(DVec3::Z)), Some(er))
}

#[test]
fn torus_segment() {
    let (big_r, r) = (20.0, 5.0);
    let mut b = B::new();
    let outer_arc = b.circle_edge(frame_z(), big_r + r, 0.0, 90.0);
    let inner_arc = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, r)), big_r, 0.0, 90.0);
    let tube_0 = b.circle_edge(torus_v_frame(0.0, big_r), r, 0.0, 90.0);
    let tube_90 = b.circle_edge(torus_v_frame(90.0, big_r), r, 0.0, 90.0);
    let l = b.loop_(
        &[(outer_arc, false), (tube_90, false), (inner_arc, true), (tube_0, true)],
        true,
    );
    let f = b.face(Surface::Torus { f: frame_z(), big_r, r }, true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let expect = r * (std::f64::consts::FRAC_PI_2) * (big_r * std::f64::consts::FRAC_PI_2 + r);
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.02, "area {got} vs {expect}");
}

#[test]
fn torus_full_v() {
    let (big_r, r) = (20.0, 5.0);
    let mut b = B::new();
    let tube_0 = b.circle_edge(torus_v_frame(0.0, big_r), r, 0.0, 360.0);
    let tube_90 = b.circle_edge(torus_v_frame(90.0, big_r), r, 0.0, 360.0);
    let l0 = b.loop_(&[(tube_0, false)], true);
    let l1 = b.loop_(&[(tube_90, false)], false);
    let f = b.face(Surface::Torus { f: frame_z(), big_r, r }, true, &[l0, l1]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let expect = r * std::f64::consts::FRAC_PI_2 * big_r * TAU;
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.03, "area {got} vs {expect}");
    assert!(kinds(&m).contains(&DiagKind::SeamSynthesized));
}

// ------------------------------------------------------------------------------------------
// solids
// ------------------------------------------------------------------------------------------

#[test]
fn cube_solid() {
    let s = 10.0;
    let c = |x: f64, y: f64, z: f64| DVec3::new(x * s, y * s, z * s);
    let mut b = B::new();
    let mut faces = Vec::new();
    // (corners in order, outward normal)
    let specs: [([DVec3; 4], DVec3); 6] = [
        ([c(0., 0., 0.), c(0., 1., 0.), c(1., 1., 0.), c(1., 0., 0.)], -DVec3::Z),
        ([c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)], DVec3::Z),
        ([c(0., 0., 0.), c(1., 0., 0.), c(1., 0., 1.), c(0., 0., 1.)], -DVec3::Y),
        ([c(0., 1., 0.), c(0., 1., 1.), c(1., 1., 1.), c(1., 1., 0.)], DVec3::Y),
        ([c(0., 0., 0.), c(0., 0., 1.), c(0., 1., 1.), c(0., 1., 0.)], -DVec3::X),
        ([c(1., 0., 0.), c(1., 1., 0.), c(1., 1., 1.), c(1., 0., 1.)], DVec3::X),
    ];
    for (corners, n) in specs {
        let l = b.poly_loop(&corners, true);
        let f = Frame::new(corners[0], Some(n), None);
        faces.push(b.face(Surface::Plane { f }, true, &[l]));
    }
    let topo = b.solid(&faces);
    assert_eq!(topo.edges.len(), 12, "cube edges must be shared");
    let m = crate::tessellate(&topo, &welded());
    let body = &m.bodies[0];
    is_closed_manifold(body).expect("cube should be a closed manifold");
    assert_eq!(boundary_edge_count(body), 0);
    assert!((signed_volume(body) - 1000.0).abs() < 1e-3, "volume {}", signed_volume(body));
    assert!(!body.double_sided);
}

#[test]
fn cylinder_solid() {
    let (r, h) = (10.0, 20.0);
    let mut b = B::new();
    let bottom = b.circle_edge(frame_z(), r, 0.0, 360.0);
    let top = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 360.0);

    let side_l0 = b.loop_(&[(bottom, false)], true);
    let side_l1 = b.loop_(&[(top, false)], false);
    let side = b.face(Surface::Cylinder { f: frame_z(), r }, true, &[side_l0, side_l1]);

    let cap_b_loop = b.loop_(&[(bottom, false)], true);
    let cap_b = b.face(
        Surface::Plane { f: Frame::new(DVec3::ZERO, Some(-DVec3::Z), Some(DVec3::X)) },
        true,
        &[cap_b_loop],
    );
    let cap_t_loop = b.loop_(&[(top, false)], true);
    let cap_t = b.face(
        Surface::Plane { f: frame_at(DVec3::new(0.0, 0.0, h)) },
        true,
        &[cap_t_loop],
    );

    let topo = b.solid(&[side, cap_b, cap_t]);
    let m = crate::tessellate(&topo, &welded());
    let body = &m.bodies[0];
    is_closed_manifold(body).expect("cylinder should be a closed manifold");
    assert_eq!(boundary_edge_count(body), 0);
    let v = signed_volume(body);
    assert!(v > 0.0, "volume {v} should be positive");
    let ideal = std::f64::consts::PI * r * r * h;
    assert!((v - ideal).abs() / ideal < 0.01, "volume {v} vs {ideal}");
}

// ------------------------------------------------------------------------------------------
// NURBS
// ------------------------------------------------------------------------------------------

#[test]
fn nurbs_bilinear() {
    let (a, c) = (30.0, 20.0);
    let rows = vec![
        vec![DVec3::ZERO, DVec3::new(0.0, c, 0.0)],
        vec![DVec3::new(a, 0.0, 0.0), DVec3::new(a, c, 0.0)],
    ];
    let s = NurbsSurface::from_step([1, 1], &rows, None, &[2, 2], &[0.0, 1.0], &[2, 2], &[0.0, 1.0]);
    let mut b = B::new();
    let l = b.poly_loop(
        &[DVec3::ZERO, DVec3::new(a, 0.0, 0.0), DVec3::new(a, c, 0.0), DVec3::new(0.0, c, 0.0)],
        true,
    );
    let f = b.face(Surface::Nurbs(Box::new(s)), true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    assert!((mesh_area(&m) - a * c).abs() < 1e-4, "area {}", mesh_area(&m));
}

#[test]
fn nurbs_rational_quarter_cylinder() {
    let (r, h) = (10.0, 8.0);
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let rows = vec![
        vec![DVec3::new(r, 0.0, 0.0), DVec3::new(r, 0.0, h)],
        vec![DVec3::new(r, r, 0.0), DVec3::new(r, r, h)],
        vec![DVec3::new(0.0, r, 0.0), DVec3::new(0.0, r, h)],
    ];
    let weights = vec![vec![1.0, 1.0], vec![w, w], vec![1.0, 1.0]];
    let s = NurbsSurface::from_step(
        [2, 1],
        &rows,
        Some(&weights),
        &[3, 3],
        &[0.0, 1.0],
        &[2, 2],
        &[0.0, 1.0],
    );
    let mut b = B::new();
    let bottom = b.circle_edge(frame_z(), r, 0.0, 90.0);
    let top = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 90.0);
    let right = b.line_edge(DVec3::new(0.0, r, 0.0), DVec3::new(0.0, r, h));
    let left = b.line_edge(DVec3::new(r, 0.0, 0.0), DVec3::new(r, 0.0, h));
    let l = b.loop_(&[(bottom, false), (right, false), (top, true), (left, true)], true);
    let f = b.face(Surface::Nurbs(Box::new(s)), true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());

    let expect = std::f64::consts::FRAC_PI_2 * r * h;
    let got = mesh_area(&m);
    assert!((got - expect).abs() / expect < 0.01, "area {got} vs {expect}");
    let body = &m.bodies[0];
    for i in 0..body.positions.len() as u32 {
        let p = world(body, i);
        let d = (p.x * p.x + p.y * p.y).sqrt();
        // f32 output precision bounds how exact this can be.
        assert!((d - r).abs() < 1e-4, "vertex at radius {d}");
    }
}

// ------------------------------------------------------------------------------------------
// edge discretisation
// ------------------------------------------------------------------------------------------

#[test]
fn edge_sampling() {
    let mut b = B::new();
    let r = 10.0;
    let circle = b.circle_edge(frame_z(), r, 0.0, 360.0);
    let ctrl = [
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(10.0, 30.0, 0.0),
        DVec3::new(30.0, -20.0, 0.0),
        DVec3::new(40.0, 10.0, 5.0),
    ];
    let spline = b.bspline_edge(&ctrl, 3);
    let l = b.loop_(&[(circle, false)], true);
    let f = b.face(Surface::Plane { f: frame_z() }, true, &[l]);
    let topo = b.sheet(&[f]);
    let tol = Tol::new(&params(), topo.vertex_bbox().diagonal(), topo.tol);
    let polys = discretize_all(&topo, &tol);

    let c = &polys[circle.idx()];
    assert!(c.pts.len() >= 25, "full circle needs ≥ 24 segments, got {}", c.pts.len() - 1);
    for w in c.pts.windows(2) {
        let mid = (w[0] + w[1]) * 0.5;
        let sag = r - mid.length();
        assert!(sag <= tol.chord + 1e-9, "chord deviation {sag} > {}", tol.chord);
    }

    let sp = &polys[spline.idx()];
    let curve = &topo.edges[spline.idx()].curve;
    assert!(sp.pts.len() > 4);
    let (t0, t1) = match curve {
        Curve3::Nurbs(n) => n.domain(),
        _ => unreachable!(),
    };
    let steps = sp.pts.len() - 1;
    for i in 0..steps {
        let ta = t0 + (t1 - t0) * i as f64 / steps as f64;
        let tb = t0 + (t1 - t0) * (i + 1) as f64 / steps as f64;
        let ang = crate::discretize::angle_between(curve.tangent(ta), curve.tangent(tb));
        assert!(ang.to_degrees() <= 15.5, "tangent turn {} deg", ang.to_degrees());
    }
}

// ------------------------------------------------------------------------------------------
// robustness
// ------------------------------------------------------------------------------------------

#[test]
fn gap_loop() {
    let mut b = B::new();
    let e0 = b.line_edge(DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0));
    let e1 = b.line_edge(DVec3::new(10.5, 0.0, 0.0), DVec3::new(10.0, 10.0, 0.0));
    let e2 = b.line_edge(DVec3::new(10.0, 10.0, 0.0), DVec3::new(0.0, 10.0, 0.0));
    let e3 = b.line_edge(DVec3::new(0.0, 10.0, 0.0), DVec3::ZERO);
    let l = b.loop_(&[(e0, false), (e1, false), (e2, false), (e3, false)], true);
    let f = b.face(Surface::Plane { f: frame_z() }, true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    assert!(m.triangle_count() > 0, "a mesh should still be produced");
    assert!(kinds(&m).contains(&DiagKind::LoopGap), "diags: {:?}", kinds(&m));
}

#[test]
fn self_intersecting_loop() {
    let mut b = B::new();
    let l = b.poly_loop(
        &[
            DVec3::ZERO,
            DVec3::new(10.0, 10.0, 0.0),
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(0.0, 10.0, 0.0),
        ],
        true,
    );
    let f = b.face(Surface::Plane { f: frame_z() }, true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    // The only hard requirement is that nothing panics and the failure is reported.
    assert!(!m.diags.is_empty(), "a diagnostic should be recorded");
    assert!(m.triangle_count() > 0 || m.stats.faces_skipped == 1);
}

#[test]
fn unsupported_surface_is_reported() {
    let mut b = B::new();
    let l = b.poly_loop(
        &[DVec3::ZERO, DVec3::new(1.0, 0.0, 0.0), DVec3::new(1.0, 1.0, 0.0)],
        true,
    );
    let f = b.face(Surface::Unsupported("dupin_cyclide".into()), true, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    assert_eq!(kinds(&m), vec![DiagKind::UnsupportedSurface]);
    assert_eq!(m.stats.faces_skipped, 1);
    assert_eq!(m.triangle_count(), 0);
}

#[test]
fn empty_topology_is_harmless() {
    let m = crate::tessellate(&ShapeTopology::default(), &params());
    assert_eq!(m.triangle_count(), 0);
    assert!(m.bodies.is_empty());
}

#[test]
fn feature_edges_are_emitted_on_creases() {
    let s = 10.0;
    let c = |x: f64, y: f64, z: f64| DVec3::new(x * s, y * s, z * s);
    let mut b = B::new();
    let mut faces = Vec::new();
    let specs: [([DVec3; 4], DVec3); 6] = [
        ([c(0., 0., 0.), c(0., 1., 0.), c(1., 1., 0.), c(1., 0., 0.)], -DVec3::Z),
        ([c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)], DVec3::Z),
        ([c(0., 0., 0.), c(1., 0., 0.), c(1., 0., 1.), c(0., 0., 1.)], -DVec3::Y),
        ([c(0., 1., 0.), c(0., 1., 1.), c(1., 1., 1.), c(1., 1., 0.)], DVec3::Y),
        ([c(0., 0., 0.), c(0., 0., 1.), c(0., 1., 1.), c(0., 1., 0.)], -DVec3::X),
        ([c(1., 0., 0.), c(1., 1., 0.), c(1., 1., 1.), c(1., 0., 1.)], DVec3::X),
    ];
    for (corners, n) in specs {
        let l = b.poly_loop(&corners, true);
        let f = Frame::new(corners[0], Some(n), None);
        faces.push(b.face(Surface::Plane { f }, true, &[l]));
    }
    let topo = b.solid(&faces);
    let m = crate::tessellate(&topo, &TessParams { weld: true, ..TessParams::PREVIEW });
    assert_eq!(m.bodies[0].edge_indices.len(), 24, "12 cube edges → 12 line segments");
}

#[test]
fn stats_are_filled() {
    let (topo, _) = full_cylinder_topo(5.0, 5.0);
    let m = crate::tessellate(&topo, &params());
    assert_eq!(m.stats.faces, 1);
    assert_eq!(m.stats.faces_ok, 1);
    assert_eq!(m.stats.faces_skipped, 0);
    assert!(m.stats.chord > 0.0);
    assert!(m.stats.vertices > 0 && m.stats.triangles > 0);
    assert!(!m.bbox.is_empty());
}

// ------------------------------------------------------------------------------------------
// swept surfaces
// ------------------------------------------------------------------------------------------

#[test]
fn extrusion_full_ring() {
    let (r, h) = (10.0, 20.0);
    let mut b = B::new();
    let e0 = b.circle_edge(frame_z(), r, 0.0, 360.0);
    let e1 = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 360.0);
    let l0 = b.loop_(&[(e0, false)], true);
    let l1 = b.loop_(&[(e1, false)], false);
    let surf = Surface::Extrusion {
        profile: Curve3::Circle { f: frame_z(), r },
        dir: DVec3::new(0.0, 0.0, h),
    };
    let f = b.face(surf, true, &[l0, l1]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let n = m.triangle_count() / 2;
    let perimeter = n as f64 * 2.0 * r * (std::f64::consts::PI / n as f64).sin();
    assert!((mesh_area(&m) - perimeter * h).abs() < 1e-3, "area {}", mesh_area(&m));
    for n in &m.bodies[0].normals {
        let d = (n[0] as f64).hypot(n[1] as f64);
        assert!((d - 1.0).abs() < 1e-5 && n[2].abs() < 1e-5, "normal {n:?}");
    }
}

#[test]
fn revolution_full_ring() {
    let (r, h) = (10.0, 20.0);
    let mut b = B::new();
    let e0 = b.circle_edge(frame_z(), r, 0.0, 360.0);
    let e1 = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 360.0);
    let l0 = b.loop_(&[(e0, false)], true);
    let l1 = b.loop_(&[(e1, false)], false);
    let surf = Surface::Revolution {
        profile: Curve3::Line { p: DVec3::new(r, 0.0, 0.0), d: DVec3::Z },
        axis: frame_z(),
    };
    let f = b.face(surf, true, &[l0, l1]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let got = mesh_area(&m);
    let expect = TAU * r * h;
    assert!((got - expect).abs() / expect < 0.01, "area {got} vs {expect}");
    assert!(kinds(&m).contains(&DiagKind::SeamSynthesized));
}

#[test]
fn nurbs_seed_is_carried_along_a_ring() {
    // Same patch as `nurbs_rational_quarter_cylinder`, but with `same_sense = false`: the mesh must
    // stay on the surface and the normals must point inwards.
    let (r, h) = (10.0, 8.0);
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let rows = vec![
        vec![DVec3::new(r, 0.0, 0.0), DVec3::new(r, 0.0, h)],
        vec![DVec3::new(r, r, 0.0), DVec3::new(r, r, h)],
        vec![DVec3::new(0.0, r, 0.0), DVec3::new(0.0, r, h)],
    ];
    let weights = vec![vec![1.0, 1.0], vec![w, w], vec![1.0, 1.0]];
    let s = NurbsSurface::from_step(
        [2, 1], &rows, Some(&weights), &[3, 3], &[0.0, 1.0], &[2, 2], &[0.0, 1.0],
    );
    let mut b = B::new();
    let bottom = b.circle_edge(frame_z(), r, 0.0, 90.0);
    let top = b.circle_edge(frame_at(DVec3::new(0.0, 0.0, h)), r, 0.0, 90.0);
    let right = b.line_edge(DVec3::new(0.0, r, 0.0), DVec3::new(0.0, r, h));
    let left = b.line_edge(DVec3::new(r, 0.0, 0.0), DVec3::new(r, 0.0, h));
    let l = b.loop_(&[(bottom, false), (right, false), (top, true), (left, true)], true);
    let f = b.face(Surface::Nurbs(Box::new(s)), false, &[l]);
    let topo = b.sheet(&[f]);
    let m = crate::tessellate(&topo, &params());
    let body = &m.bodies[0];
    for i in 0..body.positions.len() as u32 {
        let p = world(body, i);
        let n = body.normals[i as usize];
        let radial = DVec3::new(p.x, p.y, 0.0).normalize();
        let nn = DVec3::new(n[0] as f64, n[1] as f64, n[2] as f64);
        assert!(radial.dot(nn) < -0.99, "normal {nn:?} should point inwards");
    }
    for t in body.indices.chunks_exact(3) {
        let fnm = (pos(body, t[1]) - pos(body, t[0])).cross(pos(body, t[2]) - pos(body, t[0]));
        let n = body.normals[t[0] as usize];
        assert!(fnm.dot(DVec3::new(n[0] as f64, n[1] as f64, n[2] as f64)) > 0.0);
    }
}

#[test]
fn pathological_input_never_panics() {
    let nan = DVec3::splat(f64::NAN);
    let cases: Vec<(&str, ShapeTopology)> = vec![
        ("nan vertices", {
            let mut b = B::new();
            let l = b.poly_loop(&[nan, DVec3::X, DVec3::Y], true);
            let f = b.face(Surface::Plane { f: frame_z() }, true, &[l]);
            b.sheet(&[f])
        }),
        ("zero radius circle", {
            let mut b = B::new();
            let e = b.circle_edge(frame_z(), 0.0, 0.0, 360.0);
            let l = b.loop_(&[(e, false)], true);
            let f = b.face(Surface::Cylinder { f: frame_z(), r: 0.0 }, true, &[l]);
            b.sheet(&[f])
        }),
        ("degenerate cone", {
            let mut b = B::new();
            let e = b.circle_edge(frame_z(), 5.0, 0.0, 360.0);
            let l = b.loop_(&[(e, false)], true);
            let f = b.face(Surface::Cone { f: frame_z(), r: 5.0, alpha: 0.0 }, true, &[l]);
            b.sheet(&[f])
        }),
        ("malformed nurbs surface", {
            let s = NurbsSurface {
                degree: [3, 3],
                n: [2, 2],
                ctrl: vec![glam::DVec4::W; 4],
                knots: [vec![0.0, 1.0], vec![0.0, 1.0]],
                closed: [false, false],
            };
            let mut b = B::new();
            let l = b.poly_loop(
                &[DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y],
                true,
            );
            let f = b.face(Surface::Nurbs(Box::new(s)), true, &[l]);
            b.sheet(&[f])
        }),
        ("collapsed loop", {
            let mut b = B::new();
            let e = b.line_edge(DVec3::ZERO, DVec3::new(1e-14, 0.0, 0.0));
            let l = b.loop_(&[(e, false), (e, true)], true);
            let f = b.face(Surface::Plane { f: frame_z() }, true, &[l]);
            b.sheet(&[f])
        }),
        ("edge with no loops", {
            let mut b = B::new();
            let f = b.face(Surface::Plane { f: frame_z() }, true, &[]);
            b.sheet(&[f])
        }),
        ("zero-length extrusion", {
            let mut b = B::new();
            let e = b.circle_edge(frame_z(), 4.0, 0.0, 360.0);
            let l = b.loop_(&[(e, false)], true);
            let surf = Surface::Extrusion {
                profile: Curve3::Circle { f: frame_z(), r: 4.0 },
                dir: DVec3::ZERO,
            };
            let f = b.face(surf, true, &[l]);
            b.sheet(&[f])
        }),
    ];
    for (name, topo) in cases {
        for p in [params(), welded(), TessParams::FINE, TessParams::COARSE] {
            let m = crate::tessellate(&topo, &p);
            // The only contract here: it returns, and every produced index is in range.
            for b in &m.bodies {
                assert!(
                    b.indices.iter().all(|i| (*i as usize) < b.positions.len()),
                    "{name}: index out of range"
                );
                assert_eq!(b.positions.len(), b.normals.len(), "{name}");
                assert_eq!(b.positions.len(), b.face_slot.len(), "{name}");
                assert!(
                    b.face_slot.iter().all(|s| (*s as usize) < b.face_ranges.len()),
                    "{name}: face slot out of range"
                );
            }
        }
    }
}
