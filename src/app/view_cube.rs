//! Camera-oriented cube, with six faces, twelve edge views, and eight corner views.
//! Each visible face has nine hit regions; adjacent regions share the same destination.
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use glam::DVec3;
use step_render::Camera;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Snap { direction: DVec3, ortho: bool },
    Orbit(Vec2),
}

pub struct CubeResponse {
    pub rect: Rect,
    pub dragging: bool,
    pub action: Option<Action>,
}

struct Region {
    polygon: Vec<Pos2>,
    direction: DVec3,
    normal: DVec3,
    depth: f64,
}

fn regions(camera: &Camera, rect: Rect) -> Vec<Region> {
    let rotation = camera.orientation.inverse();
    let eye = camera.orientation * DVec3::Z;
    let scale = rect.width() * 0.29;
    let project = |p: DVec3| {
        let p = rotation * p;
        rect.center() + Vec2::new(p.x as f32, -p.y as f32) * scale
    };
    let mut out = Vec::new();
    let boundaries = [-1.0, -0.55, 0.55, 1.0];
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let normal = DVec3::AXES[axis] * sign;
            if normal.dot(eye) <= 1e-8 {
                continue;
            }
            let u = DVec3::AXES[(axis + 1) % 3];
            let v = DVec3::AXES[(axis + 2) % 3];
            for row in 0..3 {
                for col in 0..3 {
                    let corners = [
                        normal + u * boundaries[col] + v * boundaries[row],
                        normal + u * boundaries[col + 1] + v * boundaries[row],
                        normal + u * boundaries[col + 1] + v * boundaries[row + 1],
                        normal + u * boundaries[col] + v * boundaries[row + 1],
                    ];
                    let direction = normal + u * (col as f64 - 1.0) + v * (row as f64 - 1.0);
                    out.push(Region {
                        polygon: corners.into_iter().map(project).collect(),
                        direction,
                        normal,
                        depth: (rotation * (corners.into_iter().sum::<DVec3>() / 4.0)).z,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    out
}

fn contains(polygon: &[Pos2], p: Pos2) -> bool {
    let mut positive = false;
    let mut negative = false;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        positive |= cross > 0.001;
        negative |= cross < -0.001;
    }
    !(positive && negative)
}

fn is_axis(direction: DVec3) -> bool {
    direction.abs().element_sum() == 1.0
}

fn label(direction: DVec3) -> String {
    [(direction.x, "X"), (direction.y, "Y"), (direction.z, "Z")]
        .into_iter()
        .filter(|(v, _)| *v != 0.0)
        .map(|(v, name)| format!("{}{name}", if v > 0.0 { "+" } else { "−" }))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn show(ui: &mut egui::Ui, camera: &Camera, viewport: Rect) -> CubeResponse {
    let size = 110.0_f32.min(viewport.width()).min(viewport.height());
    let rect = Rect::from_min_size(Pos2::new(viewport.right() - size - 8.0, viewport.top() + 8.0), Vec2::splat(size)).intersect(viewport);
    let response = ui.interact(rect, ui.id().with("view_cube"), Sense::click_and_drag());
    let regions = regions(camera, rect);
    let hovered = response
        .hover_pos()
        .and_then(|pos| regions.iter().rev().find(|r| contains(&r.polygon, pos)));
    let hover_direction = hovered.map(|r| r.direction);
    let painter = ui.painter().with_clip_rect(viewport);
    // Paint each face once. Painting the nine adjacent hit polygons separately creates
    // antialiasing seams, even when all of them have the same fill.
    for face in regions.iter().filter(|r| is_axis(r.direction)) {
        let axis = if face.normal.x != 0.0 {
            0
        } else if face.normal.y != 0.0 {
            1
        } else {
            2
        };
        let u = DVec3::AXES[(axis + 1) % 3];
        let v = DVec3::AXES[(axis + 2) % 3];
        let project = |p: DVec3| {
            let p = camera.orientation.inverse() * p;
            rect.center() + Vec2::new(p.x as f32, -p.y as f32) * rect.width() * 0.29
        };
        let polygon = [face.normal - u - v, face.normal + u - v, face.normal + u + v, face.normal - u + v]
            .into_iter()
            .map(project)
            .collect();
        let facing = face.normal.dot(camera.orientation * DVec3::Z) as f32;
        let gray = (105.0 + 60.0 * facing) as u8;
        painter.add(egui::Shape::convex_polygon(polygon, Color32::from_gray(gray), Stroke::NONE));
        for region in regions
            .iter()
            .filter(|r| r.normal == face.normal && Some(r.direction) == hover_direction)
        {
            painter.add(egui::Shape::convex_polygon(
                region.polygon.clone(),
                Color32::from_rgb(116, 177, 215),
                Stroke::NONE,
            ));
        }
        let color = match axis {
            0 => Color32::from_rgb(120, 25, 25),
            1 => Color32::from_rgb(15, 80, 35),
            _ => Color32::from_rgb(20, 45, 125),
        };
        painter.text(
            project(face.normal),
            egui::Align2::CENTER_CENTER,
            label(face.normal),
            egui::FontId::proportional(14.0),
            color,
        );
    }
    // Outer edges only: the nine hit regions are invisible until hovered.
    let rotation = camera.orientation.inverse();
    for axis in 0..3 {
        for a in [-1.0, 1.0] {
            for b in [-1.0, 1.0] {
                let u = DVec3::AXES[(axis + 1) % 3] * a;
                let v = DVec3::AXES[(axis + 2) % 3] * b;
                let eye = camera.orientation * DVec3::Z;
                if u.dot(eye) <= 1e-8 && v.dot(eye) <= 1e-8 {
                    continue;
                }
                let points = [-1.0, 1.0].map(|s| {
                    let p = rotation * (u + v + DVec3::AXES[axis] * s);
                    rect.center() + Vec2::new(p.x as f32, -p.y as f32) * rect.width() * 0.29
                });
                painter.line_segment(points, Stroke::new(1.0, Color32::from_gray(48)));
            }
        }
    }
    if let Some(direction) = hover_direction {
        response
            .clone()
            .on_hover_text(format!(
                "{} · {}\nDrag to orbit in perspective",
                label(direction),
                if is_axis(direction) { "Orthographic" } else { "Perspective" }
            ))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let action = if response.dragged_by(egui::PointerButton::Primary) && response.drag_motion() != Vec2::ZERO {
        Some(Action::Orbit(response.drag_motion()))
    } else if response.clicked() {
        hover_direction.map(|direction| Action::Snap {
            direction,
            ortho: is_axis(direction),
        })
    } else {
        None
    };
    CubeResponse {
        rect,
        dragging: response.dragged(),
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, camera: &Camera, events: Vec<egui::Event>) -> CubeResponse {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0));
        let mut result = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..Default::default()
            },
            |ui| {
                // The viewport lies under the cube, as in the real application.
                ui.interact(viewport, ui.id().with("model"), Sense::click_and_drag());
                result = Some(show(ui, camera, viewport));
            },
        );
        result.unwrap()
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn clicks_snap_and_drags_capture_beyond_the_cube() {
        for direction in [DVec3::X, DVec3::NEG_Y, DVec3::Z, DVec3::ONE, DVec3::new(-1.0, 1.0, -1.0)] {
            let ctx = egui::Context::default();
            let mut camera = Camera::default();
            camera.view_from_direction(direction);
            let initial = frame(&ctx, &camera, vec![]);
            let region = regions(&camera, initial.rect)
                .into_iter()
                .find(|r| r.direction == direction)
                .unwrap();
            let pos = (region.polygon.iter().fold(Vec2::ZERO, |sum, p| sum + p.to_vec2()) / 4.0).to_pos2();
            frame(&ctx, &camera, vec![egui::Event::PointerMoved(pos)]);
            frame(&ctx, &camera, vec![button(pos, true)]);
            let clicked = frame(&ctx, &camera, vec![button(pos, false)]);
            assert_eq!(
                clicked.action,
                Some(Action::Snap {
                    direction,
                    ortho: is_axis(direction)
                })
            );
            frame(&ctx, &camera, vec![button(pos, true)]);
            let outside = pos - Vec2::new(160.0, 0.0);
            let dragged = frame(&ctx, &camera, vec![egui::Event::PointerMoved(outside)]);
            assert!(dragged.dragging);
            assert!(matches!(dragged.action, Some(Action::Orbit(_))));
            let released = frame(&ctx, &camera, vec![button(outside, false)]);
            assert_eq!(released.action, None, "drag release must not snap a face");
        }
    }

    #[test]
    fn all_twenty_six_destinations_are_reachable_and_classified() {
        let mut destinations = std::collections::BTreeSet::new();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(110.0));
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    let mut camera = Camera::default();
                    camera.view_from_direction(DVec3::new(x, y, z));
                    for region in regions(&camera, rect) {
                        let center = (region.polygon.iter().fold(Vec2::ZERO, |sum, p| sum + p.to_vec2()) / 4.0).to_pos2();
                        assert!(contains(&region.polygon, center));
                        assert!(!contains(&region.polygon, Pos2::new(-100.0, -100.0)));
                        destinations.insert(region.direction.to_array().map(|v| v as i32));
                    }
                }
            }
        }
        assert_eq!(destinations.len(), 26);
        assert_eq!(
            destinations
                .iter()
                .filter(|d| is_axis(DVec3::new(d[0] as f64, d[1] as f64, d[2] as f64)))
                .count(),
            6
        );
    }
}
