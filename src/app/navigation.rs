//! Short, interruptible orientation changes. Projection and focus remain owned by Camera.
use glam::DQuat;
use std::time::{Duration, Instant};

pub struct ViewAnimation {
    from: DQuat,
    to: DQuat,
    started: Instant,
}

impl ViewAnimation {
    pub fn new(from: DQuat, to: DQuat) -> Self {
        Self {
            from,
            to,
            started: Instant::now(),
        }
    }

    pub fn sample(&self, elapsed: Duration) -> (DQuat, bool) {
        let t = (elapsed.as_secs_f64() / 0.18).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        (self.from.slerp(self.to, smooth).normalize(), t >= 1.0)
    }
}

pub fn advance(app: &mut super::App, ctx: &egui::Context) {
    if let Some(animation) = &app.view_animation {
        let (orientation, done) = animation.sample(animation.started.elapsed());
        app.camera.orientation = orientation;
        if done {
            app.view_animation = None;
        } else {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> crate::app::App {
        use crate::app::*;
        App {
            prefs: Prefs::default(),
            camera: Camera::default(),
            up_axis_override: None,
            view_animation: None,
            settings: RenderSettings::default(),
            loader: Loader::new(Arc::new(|| {})),
            load: LoadState::default(),
            model: None,
            viewport: None,
            selection: None,
            tools: tools::Tools::default(),
            tree_filter: String::new(),
            status: String::new(),
            frame_times: VecDeque::new(),
            last_frame: Instant::now(),
            camera_touched: false,
            pending_fit: false,
            load_opts: LoadOpts::default(),
            about_open: false,
            diag_open: false,
        }
    }

    #[test]
    fn mouse_orbit_moves_the_near_surface_with_all_four_drag_directions() {
        use step_render::{StandardView, UpAxis};
        for up_axis in [UpAxis::Z, UpAxis::Y] {
            for view in [StandardView::Front, StandardView::Iso] {
                for delta in [egui::vec2(4.0, 0.0), egui::vec2(-4.0, 0.0),
                              egui::vec2(0.0, 4.0), egui::vec2(0.0, -4.0)] {
                    let mut app = app();
                    app.camera.up_axis = up_axis;
                    app.camera.standard_view(view);
                    let surface = app.camera.target - app.camera.forward();
                    let before = app.camera.project(surface, 1.0);
                    let target = app.camera.target;
                    let distance = app.camera.distance;
                    app.drag_orbit(delta);
                    let after = app.camera.project(surface, 1.0);
                    // NDC is Y-up, whereas mouse deltas are Y-down.
                    let motion = egui::vec2((after.x - before.x) as f32,
                                           (before.y - after.y) as f32);
                    assert!(motion.dot(delta) > 0.0, "{up_axis:?} {view:?} {delta:?}: {motion:?}");
                    assert_eq!(app.camera.target, target);
                    assert_eq!(app.camera.distance, distance);
                }
            }
        }
    }

    #[test]
    fn navigation_keeps_focus_synchronizes_projection_and_cancels_animation() {
        use glam::DVec3;
        use step_render::StandardView;
        let mut app = app();
        app.camera.target = DVec3::new(10.0, 20.0, 30.0);
        app.camera.distance = 123.0;
        app.set_view(StandardView::Top);
        assert!(app.camera.ortho && app.prefs.ortho);
        assert!(app.view_animation.is_some());
        app.orbit_camera(0.2, 0.1);
        assert!(!app.camera.ortho && !app.prefs.ortho);
        assert!(app.view_animation.is_none());
        app.set_view(StandardView::Iso);
        assert!(!app.camera.ortho);
        app.set_projection(true);
        assert!(app.view_animation.is_none());
        app.camera.pan(10.0, 0.0, (800, 600));
        app.camera.zoom(1.1, None, 4.0 / 3.0);
        assert!(app.camera.ortho);
        app.camera.target = DVec3::new(10.0, 20.0, 30.0);
        app.camera.distance = 123.0;
        app.snap_direction(DVec3::ONE, false);
        assert_eq!(app.camera.target, DVec3::new(10.0, 20.0, 30.0));
        assert_eq!(app.camera.distance, 123.0);
        app.direct_camera_input();
        assert!(app.view_animation.is_none());
        assert!(app.camera_touched);
    }

    #[test]
    fn changing_up_axis_levels_the_view_without_moving_the_camera() {
        use glam::DVec3;
        use step_render::UpAxis;
        let mut app = app();
        app.camera.target = DVec3::new(12.0, 30.0, -8.0);
        app.camera.distance = 321.0;
        app.camera.ortho = true;
        let eye = app.camera.eye();
        let target = app.camera.target;
        app.set_up_axis(Some(UpAxis::Y));
        assert_eq!(app.camera.up_axis, UpAxis::Y);
        assert!(app.camera.eye().distance(eye) < 1e-9);
        assert_eq!(app.camera.target, target);
        assert_eq!(app.camera.distance, 321.0);
        assert!(app.camera.ortho);
        assert!(app.camera.right().dot(DVec3::Y).abs() < 1e-9);
        assert!(app.camera.up().dot(DVec3::Y) > 0.0);
        app.set_up_axis(None);
        assert_eq!(app.camera.up_axis, UpAxis::Z);
        assert!(app.camera.eye().distance(eye) < 1e-9);
    }

    #[test]
    fn up_override_survives_reload_but_not_a_different_document() {
        use step_render::UpAxis;
        fn finish(app: &mut crate::app::App) {
            let started = Instant::now();
            while app.load.loading {
                app.drain_messages();
                assert!(started.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.load.error.is_none(), "{:?}", app.load.error);
        }
        let mut app = app();
        app.prefs.use_cache = false;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        app.open_path(root.join("cube_mm.step"));
        finish(&mut app);
        app.set_up_axis(Some(UpAxis::Y));
        app.reload();
        finish(&mut app);
        assert_eq!(app.up_axis_override, Some(UpAxis::Y));
        assert_eq!(app.camera.up_axis, UpAxis::Y);
        app.open_path(root.join("two_cubes_assembly_inch.step"));
        finish(&mut app);
        assert_eq!(app.up_axis_override, None);
        assert_eq!(app.camera.up_axis, UpAxis::Z);
    }

    #[test]
    fn appearance_migration_preserves_custom_preferences() {
        let mut app = app();
        app.prefs.background_srgb = [58, 62, 70];
        app.prefs.edge_color = [26, 28, 32, 255];
        app.prefs.migrate_appearance();
        assert_eq!(app.prefs.background_srgb, [53, 53, 53]);
        assert_eq!(app.prefs.edge_color, [38, 38, 38, 190]);
        app.prefs.background_srgb = [240, 240, 240];
        app.prefs.edge_color = [0, 0, 0, 255];
        app.prefs.migrate_appearance();
        assert_eq!(app.prefs.background_srgb, [240, 240, 240]);
        assert_eq!(app.prefs.edge_color, [0, 0, 0, 255]);
    }

    #[test]
    fn animation_has_exact_endpoints_and_unit_rotations() {
        let from = DQuat::IDENTITY;
        let to = DQuat::from_rotation_z(2.0);
        let animation = ViewAnimation::new(from, to);
        assert_eq!(animation.sample(Duration::ZERO), (from, false));
        let (mid, done) = animation.sample(Duration::from_millis(90));
        assert!(!done);
        assert!(mid.angle_between(DQuat::from_rotation_z(1.0)).abs() < 1e-7);
        let (end, done) = animation.sample(Duration::from_millis(180));
        assert!(done);
        assert!(end.angle_between(to).abs() < 1e-7);
    }
}
