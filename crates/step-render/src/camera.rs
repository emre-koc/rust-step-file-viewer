//! f64 orbit camera.
//!
//! The camera is described by an orbit *target*, a *distance* from it and an *orientation*. The
//! camera looks down its own local `-Z`, its local `+Y` is screen-up and its local `+X` is
//! screen-right — the usual right-handed convention, matching `glam`'s `look_at_rh`.
//!
//! Projections use the wgpu/Metal depth convention (NDC `z` in `0..=1`).

use glam::{DMat3, DMat4, DQuat, DVec2, DVec3, Mat4, Vec2};
use serde::{Deserialize, Serialize};
use step_mesh::Aabb;

/// Which world axis points "up". CAD data is overwhelmingly Z-up, hence the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UpAxis {
    #[default]
    Z,
    Y,
}

impl UpAxis {
    /// The world up vector.
    pub fn up(self) -> DVec3 {
        match self {
            UpAxis::Z => DVec3::Z,
            UpAxis::Y => DVec3::Y,
        }
    }
    /// The direction the camera looks along in [`StandardView::Front`].
    pub fn front_forward(self) -> DVec3 {
        match self {
            UpAxis::Z => DVec3::Y,
            UpAxis::Y => DVec3::NEG_Z,
        }
    }
    /// World-space screen-right in [`StandardView::Front`] (always `+X` for both conventions).
    pub fn front_right(self) -> DVec3 {
        self.front_forward().cross(self.up())
    }
}

/// The canonical CAD viewpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StandardView {
    Iso,
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
}

/// Orbit camera. All fields are public so a UI can bind to them directly; the methods keep the
/// derived state (orientation normalisation, clip planes) consistent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Point the camera orbits and looks at, world mm.
    pub target: DVec3,
    /// Distance from `target` to the eye, world mm. Also drives the orthographic extent.
    pub distance: f64,
    /// Camera orientation (local axes → world).
    pub orientation: DQuat,
    /// Vertical field of view in degrees (also used to size the orthographic frustum).
    pub fovy_deg: f64,
    /// Orthographic instead of perspective projection.
    pub ortho: bool,
    /// World up axis used by [`Camera::orbit`] and the standard views.
    pub up_axis: UpAxis,
    /// Near clip distance. Maintained by [`Camera::fit`] / [`Camera::auto_clip`].
    pub near: f64,
    /// Far clip distance. Maintained by [`Camera::fit`] / [`Camera::auto_clip`].
    pub far: f64,
}

impl Default for Camera {
    fn default() -> Self {
        let mut c = Camera {
            target: DVec3::ZERO,
            distance: 10.0,
            orientation: DQuat::IDENTITY,
            fovy_deg: 40.0,
            ortho: false,
            up_axis: UpAxis::Z,
            near: 0.1,
            far: 1000.0,
        };
        c.standard_view(StandardView::Iso);
        c
    }
}

impl Camera {
    // ---------------------------------------------------------------- basis

    /// World-space eye position.
    pub fn eye(&self) -> DVec3 {
        self.target + self.orientation * DVec3::Z * self.distance
    }
    /// Unit vector the camera looks along.
    pub fn forward(&self) -> DVec3 {
        self.orientation * DVec3::NEG_Z
    }
    /// Unit screen-right vector.
    pub fn right(&self) -> DVec3 {
        self.orientation * DVec3::X
    }
    /// Unit screen-up vector.
    pub fn up(&self) -> DVec3 {
        self.orientation * DVec3::Y
    }

    // ----------------------------------------------------------- projection

    /// World → view matrix.
    pub fn view(&self) -> DMat4 {
        DMat4::from_rotation_translation(self.orientation, self.eye()).inverse()
    }

    /// View → clip matrix for the given aspect ratio (width / height), depth range `0..=1`.
    pub fn proj(&self, aspect: f64) -> DMat4 {
        let aspect = if aspect.is_finite() && aspect > 1e-6 { aspect } else { 1.0 };
        let near = self.near.max(1e-6);
        let far = self.far.max(near * (1.0 + 1e-6));
        // `directx` = right-handed Y-up view space, NDC z in 0..=1 — the wgpu/Metal convention.
        use glam::dcamera::rh::proj::directx;
        if self.ortho {
            let half_h = self.distance * (self.fovy_deg.to_radians() * 0.5).tan();
            let half_h = if half_h > 1e-9 { half_h } else { 1e-9 };
            let half_w = half_h * aspect;
            directx::orthographic(-half_w, half_w, -half_h, half_h, near, far)
        } else {
            directx::perspective(self.fovy_deg.to_radians(), aspect, near, far)
        }
    }

    /// `proj * view`, in f64.
    pub fn view_proj(&self, aspect: f64) -> DMat4 {
        self.proj(aspect) * self.view()
    }

    /// `proj * view` rebased so that it consumes *render-space* points (`p_world - render_origin`)
    /// and cast to `f32`. This is the only place a world matrix is narrowed to single precision.
    pub fn view_proj_relative(&self, aspect: f64, render_origin: DVec3) -> Mat4 {
        let vp = self.proj(aspect) * self.view() * DMat4::from_translation(render_origin);
        vp.as_mat4()
    }

    /// Project a world point to NDC (`x`,`y` in `-1..=1`, `z` in `0..=1` when in front).
    pub fn project(&self, p: DVec3, aspect: f64) -> DVec3 {
        let c = self.view_proj(aspect) * p.extend(1.0);
        if c.w.abs() < 1e-12 { DVec3::new(f64::NAN, f64::NAN, f64::NAN) } else { c.truncate() / c.w }
    }

    /// Pick ray for a point in NDC (`-1..=1` on both axes, `y` up).
    ///
    /// Returns `(origin, unit direction)`; works for both projections.
    pub fn ray(&self, ndc: DVec2, aspect: f64) -> (DVec3, DVec3) {
        let inv = self.view_proj(aspect).inverse();
        let a = inv * glam::DVec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let b = inv * glam::DVec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let a = a.truncate() / a.w;
        let b = b.truncate() / b.w;
        (a, (b - a).normalize_or_zero())
    }

    // -------------------------------------------------------------- framing

    /// Frame `bbox` assuming a square viewport, and recompute the clip planes.
    pub fn fit(&mut self, bbox: Aabb) {
        self.fit_aspect(bbox, 1.0);
    }

    /// Frame `bbox` for a viewport of the given aspect ratio (width / height).
    ///
    /// The bounding *sphere* is used, so the result is rotation independent: the box stays in view
    /// while orbiting. A 5 % margin is added.
    pub fn fit_aspect(&mut self, bbox: Aabb, aspect: f64) {
        if bbox.is_empty() {
            self.target = DVec3::ZERO;
            self.distance = 10.0;
            self.near = 0.1;
            self.far = 1000.0;
            return;
        }
        let radius = (bbox.diagonal() * 0.5).max(1e-6);
        self.target = bbox.center();
        let aspect = if aspect.is_finite() && aspect > 1e-6 { aspect } else { 1.0 };
        let half_v = (self.fovy_deg.to_radians() * 0.5).clamp(1e-3, 1.5);
        let half_h = (half_v.tan() * aspect).atan();
        self.distance = radius * 1.05 / half_v.min(half_h).sin();
        self.auto_clip(bbox);
    }

    /// Recompute `near`/`far` so the whole box is inside the frustum with sane depth precision.
    pub fn auto_clip(&mut self, bbox: Aabb) {
        let radius = if bbox.is_empty() { self.distance.max(1e-6) } else { (bbox.diagonal() * 0.5).max(1e-6) };
        let centre_dist = if bbox.is_empty() { self.distance } else { (self.eye() - bbox.center()).length() };
        self.far = (centre_dist + radius * 2.0).max(1e-4);
        // Keep near/far within ~1e5 so a 24/32-bit depth buffer still resolves the model.
        self.near = (centre_dist - radius * 2.0).max(self.far * 1e-4);
    }

    // ------------------------------------------------------------ movement

    /// Turntable orbit: `dx` yaws about the world up axis, `dy` pitches about screen-right.
    /// Angles are radians; a UI typically passes `pixels * 0.01`.
    pub fn orbit(&mut self, dx: f64, dy: f64) {
        let yaw = DQuat::from_axis_angle(self.up_axis.up(), -dx);
        let pitch = DQuat::from_rotation_x(-dy);
        // World yaw on the outside keeps the horizon level; local pitch avoids gimbal snapping.
        self.orientation = (yaw * self.orientation * pitch).normalize();
    }

    /// Roll about the view direction, radians.
    pub fn roll(&mut self, angle: f64) {
        self.orientation = (self.orientation * DQuat::from_rotation_z(angle)).normalize();
    }

    /// Drag the model by a pixel delta (`dy` positive = cursor moved down).
    pub fn pan(&mut self, dx_px: f64, dy_px: f64, viewport: (u32, u32)) {
        let upp = self.world_per_pixel(viewport);
        self.target += self.right() * (-dx_px * upp) + self.up() * (dy_px * upp);
    }

    /// World units covered by one pixel at the focal plane (the plane through `target`).
    pub fn world_per_pixel(&self, viewport: (u32, u32)) -> f64 {
        let h = viewport.1.max(1) as f64;
        2.0 * self.distance * (self.fovy_deg.to_radians() * 0.5).tan() / h
    }

    /// Zoom by `factor` (`> 1` moves closer). With `cursor_ndc`, the world point that sits under the
    /// cursor on the focal plane stays under the cursor.
    pub fn zoom(&mut self, factor: f64, cursor_ndc: Option<Vec2>, aspect: f64) {
        let factor = if factor.is_finite() && factor > 1e-6 { factor } else { return };
        let anchor = cursor_ndc.map(|c| {
            let ndc = DVec2::new(c.x as f64, c.y as f64);
            self.focal_point(ndc, aspect)
        });
        self.distance = (self.distance / factor).clamp(1e-6, 1e12);
        if let (Some(before), Some(ndc)) = (anchor, cursor_ndc) {
            let ndc = DVec2::new(ndc.x as f64, ndc.y as f64);
            let after = self.focal_point(ndc, aspect);
            self.target += before - after;
        }
    }

    /// Intersection of the ray through `ndc` with the plane through `target` facing the camera.
    fn focal_point(&self, ndc: DVec2, aspect: f64) -> DVec3 {
        let (o, d) = self.ray(ndc, aspect);
        let n = self.forward();
        let denom = d.dot(n);
        if denom.abs() < 1e-12 {
            return self.target;
        }
        o + d * ((self.target - o).dot(n) / denom)
    }

    // -------------------------------------------------------- named views

    /// Snap the orientation to a canonical viewpoint (target and distance are untouched).
    pub fn standard_view(&mut self, view: StandardView) {
        let u = self.up_axis.up();
        let f = self.up_axis.front_forward();
        let r = self.up_axis.front_right();
        let (forward, up) = match view {
            StandardView::Front => (f, u),
            StandardView::Back => (-f, u),
            StandardView::Right => (-r, u),
            StandardView::Left => (r, u),
            StandardView::Top => (-u, f),
            StandardView::Bottom => (u, -f),
            // Eye above/right/in-front-of the model, the usual CAD isometric.
            StandardView::Iso => ((f - r - u).normalize(), u),
        };
        self.orientation = orientation_from(forward, up);
    }

    /// Look from a world-space direction towards the current target without changing zoom.
    /// Axis poles use the same screen-up convention as the named top/bottom views.
    pub fn view_from_direction(&mut self, direction: DVec3) {
        if !direction.is_finite() || direction.length_squared() < 1e-20 {
            return;
        }
        let eye_direction = direction.normalize();
        let world_up = self.up_axis.up();
        let up = if eye_direction.dot(world_up).abs() > 0.999999 {
            self.up_axis.front_forward() * eye_direction.dot(world_up).signum()
        } else {
            world_up
        };
        self.orientation = orientation_from(-eye_direction, up);
    }

    /// The view direction [`Camera::standard_view`] would install (useful for tests and UI state).
    pub fn standard_view_forward(&self, view: StandardView) -> DVec3 {
        let u = self.up_axis.up();
        let f = self.up_axis.front_forward();
        let r = self.up_axis.front_right();
        match view {
            StandardView::Front => f,
            StandardView::Back => -f,
            StandardView::Right => -r,
            StandardView::Left => r,
            StandardView::Top => -u,
            StandardView::Bottom => u,
            StandardView::Iso => (f - r - u).normalize(),
        }
    }
}

/// Build an orientation whose local `-Z` is `forward` and whose local `+Y` is as close to
/// `up_hint` as possible.
fn orientation_from(forward: DVec3, up_hint: DVec3) -> DQuat {
    let f = forward.normalize_or(DVec3::NEG_Z);
    let mut hint = up_hint.normalize_or(DVec3::Y);
    if f.cross(hint).length_squared() < 1e-12 {
        // Looking straight along the hint: pick any perpendicular.
        hint = if f.z.abs() < 0.9 { DVec3::Z } else { DVec3::Y };
    }
    let right = f.cross(hint).normalize();
    let up = right.cross(f).normalize();
    DQuat::from_mat3(&DMat3::from_cols(right, up, -f)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_corners(b: Aabb) -> [DVec3; 8] {
        let mut out = [DVec3::ZERO; 8];
        for (i, c) in out.iter_mut().enumerate() {
            *c = DVec3::new(
                if i & 1 == 0 { b.min.x } else { b.max.x },
                if i & 2 == 0 { b.min.y } else { b.max.y },
                if i & 4 == 0 { b.min.z } else { b.max.z },
            );
        }
        out
    }

    #[test]
    fn cube_directions_preserve_focus_and_match_named_axes() {
        for up_axis in [UpAxis::Z, UpAxis::Y] {
            let mut camera = Camera { target: DVec3::new(12.0, -8.0, 40.0), distance: 123.0, up_axis, ..Default::default() };
            for view in [StandardView::Front, StandardView::Back, StandardView::Left, StandardView::Right, StandardView::Top, StandardView::Bottom, StandardView::Iso] {
                let mut expected = camera;
                expected.standard_view(view);
                camera.view_from_direction(-expected.forward());
                assert!((camera.forward() - expected.forward()).length() < 1e-9);
                assert!((camera.up() - expected.up()).length() < 1e-9);
                assert_eq!(camera.target, DVec3::new(12.0, -8.0, 40.0));
                assert_eq!(camera.distance, 123.0);
            }
            for x in -1..=1 { for y in -1..=1 { for z in -1..=1 {
                let direction = DVec3::new(x as f64, y as f64, z as f64);
                if direction == DVec3::ZERO { continue; }
                camera.view_from_direction(direction);
                assert!((camera.forward() + direction.normalize()).length() < 1e-9);
                assert!(camera.orientation.is_finite());
            }}}
            let before = camera;
            camera.view_from_direction(DVec3::ZERO);
            camera.view_from_direction(DVec3::splat(f64::NAN));
            assert_eq!(camera, before);
        }
    }

    #[test]
    fn fit_keeps_the_whole_box_in_ndc() {
        let bbox = Aabb { min: DVec3::new(-3.0, 12.0, 100.0), max: DVec3::new(17.0, 40.0, 130.0) };
        for &aspect in &[1.0_f64, 16.0 / 9.0, 9.0 / 16.0, 4.0 / 3.0] {
            for ortho in [false, true] {
                for view in [StandardView::Iso, StandardView::Front, StandardView::Top, StandardView::Left] {
                    let mut cam = Camera { ortho, ..Camera::default() };
                    cam.standard_view(view);
                    cam.fit_aspect(bbox, aspect);
                    for c in box_corners(bbox) {
                        let ndc = cam.project(c, aspect);
                        assert!(
                            ndc.x >= -1.0 && ndc.x <= 1.0 && ndc.y >= -1.0 && ndc.y <= 1.0,
                            "corner {c:?} at ndc {ndc:?} outside view (aspect {aspect}, ortho {ortho}, {view:?})"
                        );
                        assert!(ndc.z >= 0.0 && ndc.z <= 1.0, "corner {c:?} outside depth range: {ndc:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn fit_square_viewport_uses_plain_fit() {
        let bbox = Aabb { min: DVec3::splat(-1.0), max: DVec3::splat(1.0) };
        let mut cam = Camera::default();
        cam.fit(bbox);
        for c in box_corners(bbox) {
            let ndc = cam.project(c, 1.0);
            assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "{ndc:?}");
        }
    }

    #[test]
    fn standard_views_have_the_expected_forward() {
        let mut cam = Camera::default();
        let cases = [
            (StandardView::Front, DVec3::Y),
            (StandardView::Back, DVec3::NEG_Y),
            (StandardView::Right, DVec3::NEG_X),
            (StandardView::Left, DVec3::X),
            (StandardView::Top, DVec3::NEG_Z),
            (StandardView::Bottom, DVec3::Z),
            (StandardView::Iso, DVec3::new(-1.0, 1.0, -1.0).normalize()),
        ];
        for (view, expect) in cases {
            cam.standard_view(view);
            let f = cam.forward();
            assert!((f - expect).length() < 1e-9, "{view:?}: forward {f:?} != {expect:?}");
            // Right/up must stay orthonormal.
            assert!((cam.right().cross(cam.up()) + f).length() < 1e-9, "{view:?} basis is left-handed");
        }
        // Y-up convention: Front looks along -Z.
        let mut cam = Camera { up_axis: UpAxis::Y, ..Camera::default() };
        cam.standard_view(StandardView::Front);
        assert!((cam.forward() - DVec3::NEG_Z).length() < 1e-9);
        cam.standard_view(StandardView::Top);
        assert!((cam.forward() - DVec3::NEG_Y).length() < 1e-9);
    }

    #[test]
    fn zoom_to_cursor_keeps_the_point_under_the_cursor() {
        let aspect = 16.0 / 9.0;
        for ortho in [false, true] {
            let mut cam = Camera { ortho, ..Camera::default() };
            cam.fit_aspect(Aabb { min: DVec3::new(-5.0, -5.0, -5.0), max: DVec3::new(5.0, 5.0, 5.0) }, aspect);
            let cursor = Vec2::new(0.37, -0.62);
            let ndc = DVec2::new(cursor.x as f64, cursor.y as f64);
            let anchor = cam.focal_point(ndc, aspect);
            let before = cam.project(anchor, aspect);
            let d0 = cam.distance;
            cam.zoom(1.8, Some(cursor), aspect);
            let after = cam.project(anchor, aspect);
            assert!(
                (before.x - after.x).abs() < 1e-9 && (before.y - after.y).abs() < 1e-9,
                "ortho={ortho}: {before:?} -> {after:?}"
            );
            assert!((cam.distance - d0 / 1.8).abs() < 1e-9, "zoom must divide the distance");
        }
    }

    #[test]
    fn zoom_without_cursor_only_changes_distance() {
        let mut cam = Camera::default();
        let t = cam.target;
        cam.zoom(2.0, None, 1.0);
        assert!((cam.distance - 5.0).abs() < 1e-12);
        assert_eq!(cam.target, t);
    }

    #[test]
    fn orbit_keeps_the_horizon_level() {
        let mut cam = Camera::default();
        cam.standard_view(StandardView::Front);
        cam.orbit(0.7, 0.0);
        // Pure yaw about world Z must leave screen-right in the XY plane.
        assert!(cam.right().z.abs() < 1e-9, "{:?}", cam.right());
        assert!((cam.up() - DVec3::Z).length() < 1e-9, "{:?}", cam.up());
    }

    #[test]
    fn pan_moves_the_target_in_the_view_plane() {
        let mut cam = Camera::default();
        cam.standard_view(StandardView::Front);
        cam.distance = 100.0;
        let before = cam.target;
        cam.pan(10.0, 0.0, (800, 600));
        let d = cam.target - before;
        assert!(d.dot(cam.forward()).abs() < 1e-9);
        assert!(d.dot(cam.right()) < 0.0, "dragging right moves the model right");
    }

    #[test]
    fn ray_through_centre_is_the_view_direction() {
        let mut cam = Camera::default();
        cam.fit(Aabb { min: DVec3::splat(-2.0), max: DVec3::splat(2.0) });
        let (o, d) = cam.ray(DVec2::ZERO, 1.0);
        assert!((d - cam.forward()).length() < 1e-9, "{d:?} vs {:?}", cam.forward());
        // The origin sits on the near plane, on the view axis.
        let to_eye = o - cam.eye();
        assert!(to_eye.cross(cam.forward()).length() < 1e-6);
    }

    #[test]
    fn roll_rotates_about_the_view_axis() {
        let mut cam = Camera::default();
        let f = cam.forward();
        cam.roll(std::f64::consts::FRAC_PI_2);
        assert!((cam.forward() - f).length() < 1e-9);
    }
}
