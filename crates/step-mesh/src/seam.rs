//! Periodic bookkeeping: unwrapping rings across the seam, synthesizing a cut for Creo-style
//! "full cylinder = two closed circles" faces, and closing degenerate poles/apexes.

use glam::DVec2;

use crate::discretize::Tol;
use crate::mesh::DiagKind;
use crate::param::{Param, wrap_half};
use crate::triangulate::UvRing;

#[inline]
fn comp(p: DVec2, i: usize) -> f64 {
    if i == 0 { p.x } else { p.y }
}

#[inline]
fn set_comp(p: &mut DVec2, i: usize, v: f64) {
    if i == 0 { p.x = v } else { p.y = v }
}

/// Make a ring continuous across the seam and report its winding per periodic direction.
///
/// Consecutive edge samples are at most ~15° apart, so the "shortest wrap" choice is unambiguous.
/// The winding includes the closing segment, so it is the exact integer number of turns.
pub(crate) fn unwrap_ring(uv: &mut [DVec2], period: [Option<f64>; 2]) -> [i32; 2] {
    let mut w = [0i32; 2];
    if uv.len() < 2 {
        return w;
    }
    for dir in 0..2 {
        let Some(p) = period[dir] else { continue };
        if !p.is_finite() || p <= 0.0 {
            continue;
        }
        let mut total = 0.0;
        for i in 1..uv.len() {
            let d = wrap_half(comp(uv[i], dir) - comp(uv[i - 1], dir), p);
            total += d;
            let v = comp(uv[i - 1], dir) + d;
            set_comp(&mut uv[i], dir, v);
        }
        // closing segment
        total += wrap_half(comp(uv[0], dir) - comp(uv[uv.len() - 1], dir), p);
        w[dir] = (total / p).round() as i32;
    }
    w
}

/// Result of the seam analysis for one face.
pub(crate) struct SeamResult {
    pub rings: Vec<UvRing>,
    pub diags: Vec<(DiagKind, String)>,
}

/// Turn the raw projected rings into rings that live in a simply-connected uv patch.
///
/// `orient` is `+1` for `same_sense` faces and `-1` otherwise; it converts the loop's traversal
/// direction into the parametric-space interior side.
pub(crate) fn resolve(
    mut rings: Vec<UvRing>,
    param: &Param<'_>,
    orient: f64,
    tol: &Tol,
) -> SeamResult {
    let mut diags = Vec::new();
    let period = param.period();

    if period[0].is_none() && period[1].is_none() {
        return SeamResult { rings, diags };
    }

    let mut wind: Vec<[i32; 2]> = Vec::with_capacity(rings.len());
    for r in &mut rings {
        wind.push(unwrap_ring(&mut r.uv, period));
    }

    if rings.is_empty() {
        // A periodic face with no bounds at all covers the whole natural domain.
        if let Some(r) = full_domain_ring(param, tol) {
            diags.push((DiagKind::SeamSynthesized, "closed surface without bounds".into()));
            return SeamResult { rings: vec![r], diags };
        }
        return SeamResult { rings, diags };
    }

    // Bring every non-wrapping ring onto the same periodic branch as the reference ring, so a hole
    // is not projected a whole period away from the bound it lives in.
    let refi = rings.iter().position(|r| r.is_outer).unwrap_or(0);
    for dir in 0..2 {
        let Some(p) = period[dir] else { continue };
        let mean = |r: &UvRing| {
            r.uv.iter().map(|q| comp(*q, dir)).sum::<f64>() / r.uv.len().max(1) as f64
        };
        let base = mean(&rings[refi]);
        for i in 0..rings.len() {
            if i == refi || wind[i][dir] != 0 {
                continue;
            }
            let k = ((base - mean(&rings[i])) / p).round();
            if k != 0.0 {
                for q in &mut rings[i].uv {
                    let v = comp(*q, dir) + k * p;
                    set_comp(q, dir, v);
                }
            }
        }
    }

    // Any ring winding in both directions at once is not something we can cut.
    if let Some(i) = wind.iter().position(|w| w[0] != 0 && w[1] != 0) {
        diags.push((
            DiagKind::UnexpectedWinding,
            format!("ring {} winds {:?}; using the full parametric domain", i, wind[i]),
        ));
        if let Some(r) = full_domain_ring(param, tol) {
            return SeamResult { rings: vec![r], diags };
        }
        return SeamResult { rings, diags };
    }
    if let Some(i) = wind.iter().position(|w| w[0].abs() > 1 || w[1].abs() > 1) {
        diags.push((
            DiagKind::UnexpectedWinding,
            format!("ring {} winds {:?}", i, wind[i]),
        ));
    }

    for dir in 0..2 {
        let Some(p) = period[dir] else { continue };
        let wrapped: Vec<usize> =
            (0..rings.len()).filter(|i| wind[*i][dir].abs() >= 1).collect();
        match wrapped.len() {
            0 => {}
            1 => {
                let i = wrapped[0];
                let step = across_step(param, tol, dir);
                let w = wind[i][dir];
                match pole_across(param, &rings[i], dir, w, orient) {
                    Some(pole) => {
                        let closed = cap_ring(&rings[i], dir, p, w, pole, step);
                        rings[i] = closed;
                        wind[i][dir] = 0;
                        diags.push((
                            DiagKind::SeamSynthesized,
                            format!("degenerate pole closed at {pole:.4}"),
                        ));
                    }
                    None => {
                        diags.push((
                            DiagKind::UnexpectedWinding,
                            "single wrapping ring without a degenerate pole".into(),
                        ));
                        if let Some(r) = full_domain_ring(param, tol) {
                            return SeamResult { rings: vec![r], diags };
                        }
                    }
                }
            }
            2 => {
                let (i, j) = (wrapped[0], wrapped[1]);
                let step = across_step(param, tol, dir);
                let merged = synthesize_cut(
                    &rings[i], wind[i][dir], &rings[j], wind[j][dir], dir, p, step,
                );
                diags.push((
                    DiagKind::SeamSynthesized,
                    "closed surface cut between two wrapping rings".into(),
                ));
                // Replace ring i with the merged one and drop ring j.
                rings[i] = merged;
                wind[i][dir] = 0;
                rings.remove(j);
                wind.remove(j);
            }
            _ => {
                diags.push((
                    DiagKind::UnexpectedWinding,
                    format!("{} rings wrap direction {dir}", wrapped.len()),
                ));
                if let Some(r) = full_domain_ring(param, tol) {
                    return SeamResult { rings: vec![r], diags };
                }
            }
        }
    }

    SeamResult { rings, diags }
}

/// Steiner spacing along the direction *across* the cut (`1 - dir`).
fn across_step(param: &Param<'_>, tol: &Tol, dir: usize) -> f64 {
    let other = 1 - dir;
    let radii = param.curvature_radii(0.0, 0.0);
    let scale = param.scale_uv(0.0);
    let s = if other == 0 { scale.x } else { scale.y };
    match radii[other] {
        Some(r) if r > 1e-9 => {
            let ang = (2.0 * (1.0 - (2.0 * tol.chord / r).min(1.0)).clamp(-1.0, 1.0).acos())
                .min(tol.max_angle_surf)
                .max(1e-3);
            (ang * r / s).max(1e-9)
        }
        _ => f64::INFINITY,
    }
}

/// Which degenerate parameter line (if any) closes a single wrapping ring.
fn pole_across(param: &Param<'_>, ring: &UvRing, dir: usize, w: i32, orient: f64) -> Option<f64> {
    let other = 1 - dir;
    let dom = param.natural_domain()[other]?;
    // Mean `across` coordinate of the ring.
    let mid = ring.uv.iter().map(|p| comp(*p, other)).sum::<f64>() / ring.uv.len().max(1) as f64;
    let (lo, hi) = dom;
    let deg_lo = param_degenerate(param, dir, other, lo);
    let deg_hi = param_degenerate(param, dir, other, hi);
    match (deg_lo, deg_hi) {
        (true, false) => Some(lo),
        (false, true) => Some(hi),
        (true, true) => {
            // Both ends degenerate (sphere): the interior side follows the traversal direction.
            let up = (w as f64) * orient > 0.0;
            Some(if up { hi } else { lo })
        }
        (false, false) => {
            let _ = mid;
            None
        }
    }
}

/// Is the parameter line `across == value` a single point (pole / apex / axis)?
fn param_degenerate(param: &Param<'_>, dir: usize, other: usize, value: f64) -> bool {
    let mk = |along: f64| -> glam::DVec2 {
        let mut p = DVec2::ZERO;
        set_comp(&mut p, dir, along);
        set_comp(&mut p, other, value);
        p
    };
    let base = param.natural_domain()[dir].unwrap_or((0.0, std::f64::consts::TAU));
    let a = mk(base.0 + (base.1 - base.0) * 0.1);
    let b = mk(base.0 + (base.1 - base.0) * 0.6);
    let pa = param.eval(a.x, a.y);
    let pb = param.eval(b.x, b.y);
    pa.distance(pb) < 1e-7
}

/// Join two rings that each wrap the periodic direction into one hole-free polygon.
fn synthesize_cut(
    a: &UvRing,
    wa: i32,
    b: &UvRing,
    wb: i32,
    dir: usize,
    period: f64,
    step: f64,
) -> UvRing {
    let other = 1 - dir;
    let mut ra = a.clone();
    if wa < 0 {
        ra.reverse();
    }
    let mut rb = b.clone();
    if wb > 0 {
        rb.reverse();
    }
    re_unwrap(&mut ra.uv, dir, period);
    let a0 = comp(ra.uv[0], dir);

    // Rotate B so that it starts at the same `along` coordinate as A (modulo the period).
    let k = (0..rb.uv.len())
        .min_by(|x, y| {
            let dx = wrap_half(comp(rb.uv[*x], dir) - a0, period).abs();
            let dy = wrap_half(comp(rb.uv[*y], dir) - a0, period).abs();
            dx.partial_cmp(&dy).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    rb.rotate_left(k);
    re_unwrap(&mut rb.uv, dir, period);
    let shift = a0 + period - comp(rb.uv[0], dir);
    for p in &mut rb.uv {
        set_comp(p, dir, comp(*p, dir) + shift);
    }

    let mut out = UvRing { uv: Vec::new(), src: Vec::new(), is_outer: true };
    // A, traversed forward, then its wrapped start point at a0 + period.
    for (p, s) in ra.uv.iter().zip(&ra.src) {
        out.push(*p, *s);
    }
    let mut wrap_a = ra.uv[0];
    set_comp(&mut wrap_a, dir, a0 + period);
    out.push(wrap_a, ra.src[0]);

    // Seam down to B at along = a0 + period.
    seam_points(&mut out, dir, a0 + period, comp(wrap_a, other), comp(rb.uv[0], other), step);

    for (p, s) in rb.uv.iter().zip(&rb.src) {
        out.push(*p, *s);
    }
    let mut wrap_b = rb.uv[0];
    set_comp(&mut wrap_b, dir, a0);
    out.push(wrap_b, rb.src[0]);

    // Seam back up at along = a0, closing onto A[0].
    seam_points(&mut out, dir, a0, comp(wrap_b, other), comp(ra.uv[0], other), step);
    out
}

/// Close a ring that wraps once against a degenerate parameter line.
fn cap_ring(ring: &UvRing, dir: usize, period: f64, w: i32, pole: f64, step: f64) -> UvRing {
    let other = 1 - dir;
    let mut r = ring.clone();
    if w < 0 {
        r.reverse();
    }
    re_unwrap(&mut r.uv, dir, period);
    let a0 = comp(r.uv[0], dir);
    let base = comp(r.uv[0], other);

    let mut out = UvRing { uv: Vec::new(), src: Vec::new(), is_outer: ring.is_outer };
    for (p, s) in r.uv.iter().zip(&r.src) {
        out.push(*p, *s);
    }
    let mut wrap = r.uv[0];
    set_comp(&mut wrap, dir, a0 + period);
    out.push(wrap, r.src[0]);

    // The two seam segments run the whole way from the ring to the pole; without intermediate
    // points the triangulator would bridge them with a couple of huge triangles.
    seam_points(&mut out, dir, a0 + period, base, pole, step);

    // Fan line along the pole, traversed backwards so the polygon stays simple.
    let pole_pt = |along: f64, out: &mut UvRing| {
        let mut p = DVec2::ZERO;
        set_comp(&mut p, dir, along);
        set_comp(&mut p, other, pole);
        out.push(p, None);
    };
    pole_pt(a0 + period, &mut out);
    for i in (1..r.uv.len()).rev() {
        pole_pt(comp(r.uv[i], dir), &mut out);
    }
    pole_pt(a0, &mut out);
    seam_points(&mut out, dir, a0, pole, base, step);
    out
}

/// Insert intermediate points along a constant-`along` seam segment (endpoints excluded).
fn seam_points(out: &mut UvRing, dir: usize, along: f64, from: f64, to: f64, step: f64) {
    if !step.is_finite() || step <= 0.0 {
        return;
    }
    let span = to - from;
    let n = (span.abs() / step).ceil() as usize;
    if !(2..=4096).contains(&n) {
        return;
    }
    let other = 1 - dir;
    for i in 1..n {
        let mut p = DVec2::ZERO;
        set_comp(&mut p, dir, along);
        set_comp(&mut p, other, from + span * (i as f64) / (n as f64));
        out.push(p, None);
    }
}

/// Re-run the unwrap walk on one direction after a rotation or reversal.
fn re_unwrap(uv: &mut [DVec2], dir: usize, period: f64) {
    for i in 1..uv.len() {
        let d = wrap_half(comp(uv[i], dir) - comp(uv[i - 1], dir), period);
        let v = comp(uv[i - 1], dir) + d;
        set_comp(&mut uv[i], dir, v);
    }
}

/// A rectangle covering the whole natural domain, subdivided so the mesh follows the curvature.
fn full_domain_ring(param: &Param<'_>, tol: &Tol) -> Option<UvRing> {
    let dom = param.natural_domain();
    let (u0, u1) = dom[0]?;
    let (v0, v1) = dom[1]?;
    if !(u1 > u0 && v1 > v0) {
        return None;
    }
    let per = param.period();
    // A periodic direction must not close exactly on itself, so nudge the far edge inwards.
    let eps_u = if per[0].is_some() { (u1 - u0) * 1e-9 } else { 0.0 };
    let eps_v = if per[1].is_some() { (v1 - v0) * 1e-9 } else { 0.0 };
    let (u1, v1) = (u1 - eps_u, v1 - eps_v);

    let nu = subdivisions(param, tol, 0, u1 - u0);
    let nv = subdivisions(param, tol, 1, v1 - v0);
    let mut r = UvRing { uv: Vec::new(), src: Vec::new(), is_outer: true };
    for i in 0..nu {
        r.push(DVec2::new(u0 + (u1 - u0) * i as f64 / nu as f64, v0), None);
    }
    for j in 0..nv {
        r.push(DVec2::new(u1, v0 + (v1 - v0) * j as f64 / nv as f64), None);
    }
    for i in (1..=nu).rev() {
        r.push(DVec2::new(u0 + (u1 - u0) * i as f64 / nu as f64, v1), None);
    }
    for j in (1..=nv).rev() {
        r.push(DVec2::new(u0, v0 + (v1 - v0) * j as f64 / nv as f64), None);
    }
    Some(r)
}

fn subdivisions(param: &Param<'_>, tol: &Tol, dir: usize, span: f64) -> usize {
    let radii = param.curvature_radii(0.0, 0.0);
    match radii[dir] {
        Some(r) if r > 1e-9 => {
            let scale = param.scale_uv(0.0);
            let s = if dir == 0 { scale.x } else { scale.y };
            let ang = (2.0 * (1.0 - (2.0 * tol.chord / r).min(1.0)).clamp(-1.0, 1.0).acos())
                .min(tol.max_angle_surf)
                .max(1e-3);
            let h = (ang * r / s).max(1e-9);
            ((span / h).ceil() as usize).clamp(2, 256)
        }
        _ => 2,
    }
}
