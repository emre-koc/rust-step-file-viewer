//! Toolbar, assembly tree, properties and status panels.

use egui::{Color32, RichText};
use step_brep::NodeId;
use step_render::{RenderMode, StandardView, UpAxis};

use super::tools::{SectionAxis, format_len};
use super::{App, Quality, apply_prefs_to_settings, group_digits};

pub fn top_bar(app: &mut App, root: &mut egui::Ui) {
    let ctx = root.ctx().clone();
    let ctx = &ctx;
    egui::Panel::top("top").show(root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Window   ⌘N").clicked() {
                    ui.close();
                    app.new_window(None);
                }
                if ui.button("Open…        ⌘O").clicked() {
                    ui.close();
                    app.open_dialog();
                }
                ui.menu_button("Open Recent", |ui| {
                    let recent: Vec<_> = app.prefs.recent.iter().cloned().collect();
                    if recent.is_empty() {
                        ui.label("(empty)");
                    }
                    for p in recent {
                        if ui.button(p.file_name().and_then(|s| s.to_str()).unwrap_or("?")).on_hover_text(p.display().to_string()).clicked() {
                            ui.close();
                            app.open_document(p);
                        }
                    }
                });
                if ui.add_enabled(app.load.path.is_some(), egui::Button::new("Reload        ⌘R")).clicked() {
                    ui.close();
                    app.reload();
                }
                ui.separator();
                if ui.add_enabled(app.model.is_some(), egui::Button::new("Export…      ⌘E")).clicked() {
                    ui.close();
                    app.export_dialog();
                }
                if ui.add_enabled(app.model.is_some(), egui::Button::new("Screenshot…  ⌘S")).clicked() {
                    ui.close();
                    app.screenshot_dialog();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                for (label, v) in [("Front  1", StandardView::Front), ("Back  2", StandardView::Back), ("Left  3", StandardView::Left), ("Right  4", StandardView::Right), ("Top  5", StandardView::Top), ("Bottom  6", StandardView::Bottom), ("Isometric  7", StandardView::Iso)] {
                    if ui.button(label).clicked() {
                        ui.close();
                        app.set_view(v);
                    }
                }
                ui.separator();
                if ui.button("Fit all  F").clicked() {
                    ui.close();
                    app.fit_all();
                }
                let mut ortho = app.camera.ortho;
                if ui.checkbox(&mut ortho, "Orthographic").changed() {
                    app.set_projection(ortho);
                }
                ui.checkbox(&mut app.prefs.turntable, "Turntable orbit");
                ui.menu_button(format!("Model up: {:?}", app.camera.up_axis), |ui| {
                    for (label, axis) in [("Auto (exporter hint)", None), ("Y-up", Some(UpAxis::Y)), ("Z-up", Some(UpAxis::Z))] {
                        if ui.radio(app.up_axis_override == axis, label).clicked() {
                            app.set_up_axis(axis);
                            ui.close();
                        }
                    }
                    ui.weak("Auto uses Y for SolidWorks; otherwise Z. Override for models exported in another orientation.");
                });
                ui.separator();
                ui.checkbox(&mut app.prefs.show_tree, "Assembly tree");
                ui.checkbox(&mut app.prefs.show_props, "Properties");
                ui.checkbox(&mut app.prefs.show_stats, "Statistics");
            });
            ui.menu_button("Display", |ui| {
                for (label, m) in [("Shaded + edges  S", RenderMode::ShadedEdges), ("Shaded", RenderMode::Shaded), ("Wireframe  W", RenderMode::Wireframe), ("X-ray  X", RenderMode::XRay)] {
                    if ui.radio(app.prefs.mode == m, label).clicked() {
                        app.prefs.mode = m;
                        app.settings.mode = m;
                    }
                }
                ui.separator();
                let mut msaa = app.prefs.msaa >= 4;
                if ui.checkbox(&mut msaa, "Anti-aliasing (MSAA 4x)").changed() {
                    app.prefs.msaa = if msaa { 4 } else { 1 };
                    app.settings.msaa = app.prefs.msaa;
                }
                ui.checkbox(&mut app.prefs.colors, "Part colours (reload to apply)");
                ui.separator();
                ui.label("Tessellation quality (reload to apply):");
                for q in [Quality::Coarse, Quality::Preview, Quality::Fine] {
                    if ui.radio(app.prefs.quality == q, q.label()).clicked() {
                        app.prefs.quality = q;
                        app.load_opts.params = q.params();
                    }
                }
                ui.checkbox(&mut app.prefs.use_cache, "Use mesh cache");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Background");
                    let mut c = Color32::from_rgb(app.prefs.background_srgb[0], app.prefs.background_srgb[1], app.prefs.background_srgb[2]);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        app.prefs.background_srgb = [c.r(), c.g(), c.b()];
                        apply_prefs_to_settings(&app.prefs, &mut app.settings);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Edges");
                    let mut c = Color32::from_rgba_unmultiplied(app.prefs.edge_color[0], app.prefs.edge_color[1], app.prefs.edge_color[2], app.prefs.edge_color[3]);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        app.prefs.edge_color = c.to_array();
                        app.settings.edge_color = app.prefs.edge_color;
                    }
                });
                ui.checkbox(&mut app.prefs.units_inch, "Show lengths in inches");
            });
            ui.menu_button("Selection", |ui| {
                if ui.add_enabled(app.selection.is_some(), egui::Button::new("Hide selected  H")).clicked() {
                    ui.close();
                    app.hide_selected();
                }
                if ui.add_enabled(app.selection.is_some(), egui::Button::new("Isolate selected  I")).clicked() {
                    ui.close();
                    app.isolate_selected();
                }
                if ui.button("Show all  ⇧H").clicked() {
                    ui.close();
                    app.unhide_all();
                }
                ui.separator();
                if ui.checkbox(&mut app.tools.measure_active, "Measure distance").changed() && !app.tools.measure_active {
                    app.tools.measure_points.clear();
                }
                ui.checkbox(&mut app.tools.section_enabled, "Section plane");
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Shortcuts & about").clicked() {
                    ui.close();
                    app.about_open = true;
                }
                if ui.add_enabled(app.model.is_some(), egui::Button::new("Diagnostics…")).clicked() {
                    ui.close();
                    app.diag_open = true;
                }
            });
            ui.separator();
            // quick toolbar
            if ui.button("⤢ Fit").on_hover_text("Fit all (F)").clicked() {
                app.fit_all();
            }
            for (label, v) in [("Iso", StandardView::Iso), ("Top", StandardView::Top), ("Front", StandardView::Front), ("Right", StandardView::Right)] {
                if ui.small_button(label).clicked() {
                    app.set_view(v);
                }
            }
            ui.separator();
            for (label, ortho) in [("Ortho", true), ("Perspective", false)] {
                if ui.selectable_label(app.camera.ortho == ortho, label).clicked() {
                    app.set_projection(ortho);
                }
            }
            ui.separator();
            egui::ComboBox::from_id_salt("mode")
                .selected_text(match app.prefs.mode {
                    RenderMode::ShadedEdges => "Shaded + edges",
                    RenderMode::Shaded => "Shaded",
                    RenderMode::Wireframe => "Wireframe",
                    RenderMode::XRay => "X-ray",
                })
                .show_ui(ui, |ui| {
                    for (label, m) in [("Shaded + edges", RenderMode::ShadedEdges), ("Shaded", RenderMode::Shaded), ("Wireframe", RenderMode::Wireframe), ("X-ray", RenderMode::XRay)] {
                        if ui.selectable_label(app.prefs.mode == m, label).clicked() {
                            app.prefs.mode = m;
                            app.settings.mode = m;
                        }
                    }
                });
            ui.toggle_value(&mut app.tools.measure_active, "📏 Measure");
            ui.toggle_value(&mut app.tools.section_enabled, "◫ Section");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if app.load.loading {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("{}/{}", app.load.done, app.load.total));
                }
            });
        });
    });
    // keep clip plane in sync
    if let Some(vp) = &app.viewport {
        app.settings.clip_plane = app.tools.clip_plane(&vp.scene_bbox());
    }
}

pub fn status_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::bottom("status").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.label(&app.status);
            if app.load.loading && app.load.total > 0 {
                ui.add(egui::ProgressBar::new(app.load.done as f32 / app.load.total as f32).desired_width(160.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if app.prefs.show_stats {
                    let avg = if app.frame_times.is_empty() { 0.0 } else { app.frame_times.iter().sum::<f32>() / app.frame_times.len() as f32 };
                    let tris = app.model.as_ref().map_or(0, |m| m.instanced_triangles());
                    let t = &app.load.timings;
                    ui.label(RichText::new(format!(
                        "render {avg:.1} ms · {} tris · index {:.0} ms · structure {:.0} ms · mesh {:.0} ms{}",
                        group_digits(tris),
                        t.index_ms,
                        t.structure_ms,
                        t.mesh_ms,
                        app.load.first_shape_at.map(|f| format!(" · first shape {f:.0} ms")).unwrap_or_default()
                    ))
                    .small()
                    .color(Color32::from_gray(170)));
                }
            });
        });
    });
}

pub fn tree_panel(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::left("tree").default_size(300.0).min_size(180.0).show(root, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Assembly");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Show all").clicked() {
                    app.unhide_all();
                }
            });
        });
        ui.add(egui::TextEdit::singleline(&mut app.tree_filter).hint_text("filter…").desired_width(f32::INFINITY));
        ui.separator();
        let Some(model) = &app.model else {
            ui.weak("No model loaded");
            return;
        };
        let asm = model.assembly.clone();
        let filter = app.tree_filter.to_lowercase();
        let selected_node = app.selection.as_ref().map(|s| s.node);
        let mut action: Option<TreeAction> = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let roots: Vec<NodeId> = asm.roots.clone();
            for r in roots {
                tree_node(ui, model, &asm, r, &filter, selected_node, &mut action, 0);
            }
        });
        drop(asm);
        if let Some(a) = action {
            apply_tree_action(app, a);
        }
    });
}

enum TreeAction {
    Select(NodeId),
    ToggleHidden(NodeId, bool),
    Isolate(NodeId),
    Fit(NodeId),
}

fn node_matches(asm: &step_brep::Assembly, id: NodeId, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let n = &asm.nodes[id.0 as usize];
    if n.name.to_lowercase().contains(filter) {
        return true;
    }
    n.children.iter().any(|c| node_matches(asm, *c, filter))
}

#[allow(clippy::too_many_arguments)]
fn tree_node(ui: &mut egui::Ui, model: &super::LoadedModel, asm: &step_brep::Assembly, id: NodeId, filter: &str, selected: Option<NodeId>, action: &mut Option<TreeAction>, depth: usize) {
    if !node_matches(asm, id, filter) {
        return;
    }
    let n = &asm.nodes[id.0 as usize];
    let hidden = model.hidden.contains(&id);
    let eff_hidden = model.hidden_effective(id);
    let is_sel = selected == Some(id);
    let label_text = if n.children.is_empty() { n.name.clone() } else { format!("{}  ({})", n.name, n.children.len()) };
    let mut text = RichText::new(label_text);
    if eff_hidden {
        text = text.color(Color32::from_gray(110));
    }
    if is_sel {
        text = text.strong();
    }
    let row = |ui: &mut egui::Ui, action: &mut Option<TreeAction>| {
        let mut vis = !hidden;
        if ui.checkbox(&mut vis, "").on_hover_text("visible").changed() {
            *action = Some(TreeAction::ToggleHidden(id, !vis));
        }
        let resp = ui.selectable_label(is_sel, text.clone());
        if resp.clicked() {
            *action = Some(TreeAction::Select(id));
        }
        if resp.double_clicked() {
            *action = Some(TreeAction::Fit(id));
        }
        resp.context_menu(|ui| {
            if ui.button("Fit view").clicked() {
                *action = Some(TreeAction::Fit(id));
                ui.close();
            }
            if ui.button(if hidden { "Show" } else { "Hide" }).clicked() {
                *action = Some(TreeAction::ToggleHidden(id, !hidden));
                ui.close();
            }
            if ui.button("Isolate").clicked() {
                *action = Some(TreeAction::Isolate(id));
                ui.close();
            }
        });
    };
    if n.children.is_empty() {
        ui.horizontal(|ui| row(ui, action));
    } else {
        let default_open = depth < 1 || !filter.is_empty();
        let header_id = ui.make_persistent_id(("node", id.0));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), header_id, default_open)
            .show_header(ui, |ui| row(ui, action))
            .body(|ui| {
                for c in &n.children {
                    tree_node(ui, model, asm, *c, filter, selected, action, depth + 1);
                }
            });
    }
}

fn apply_tree_action(app: &mut App, a: TreeAction) {
    let (Some(m), Some(v)) = (&mut app.model, &mut app.viewport) else { return };
    match a {
        TreeAction::Select(id) => {
            m.select_node(id, v);
            // synthesize a node-only selection
            let inst = m.instances_under_node(id).into_iter().find_map(|i| m.instance_handles[i as usize].map(|h| (i, h)));
            app.selection = inst.map(|(i, h)| super::Selection {
                instance: h,
                instance_index: i,
                node: id,
                shape: m.structure.assembly.instances[i as usize].shape,
                body: u32::MAX,
                face_slot: u32::MAX,
                face_src: 0,
                face_index: u32::MAX,
                point: m.node_bbox[id.0 as usize].center(),
            });
        }
        TreeAction::ToggleHidden(id, hidden) => m.set_node_hidden(id, hidden, v),
        TreeAction::Isolate(id) => m.isolate_node(id, v),
        TreeAction::Fit(id) => {
            let bb = m.node_bbox[id.0 as usize];
            if !bb.is_empty() {
                app.camera.fit(bb);
                app.camera_touched = true;
            }
        }
    }
}

pub fn props_panel(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::right("props").default_size(300.0).min_size(200.0).show(root, |ui| {
        // A vertical-only scroll area expands horizontally to fit its content,
        // forcing the panel back out when a grid or control exceeds the dragged width.
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading("Selection");
            enum PropAction {
                Hide,
                Isolate,
                Fit(step_mesh::Aabb),
                SectionHere(glam::DVec3, glam::DVec3),
            }
            let mut action: Option<PropAction> = None;
            let sel_clone = app.selection.clone();
            match (&sel_clone, app.model.as_ref()) {
                (Some(sel), Some(m)) => {
                    let asm = &m.structure.assembly;
                    let node = &asm.nodes[sel.node.0 as usize];
                    let units_inch = app.prefs.units_inch;
                    egui::Grid::new("sel").num_columns(2).striped(true).show(ui, |ui| {
                        ui.label("Part");
                        ui.label(&node.name);
                        ui.end_row();
                        ui.label("Path");
                        ui.add(egui::Label::new(&node.path).wrap());
                        ui.end_row();
                        if let Some(pd) = node.product_definition {
                            ui.label("Product def.");
                            ui.label(format!("{pd}"));
                            ui.end_row();
                        }
                        ui.label("Shape rep.");
                        ui.label(format!("#{}", asm.shapes[sel.shape.0 as usize].rep.0));
                        ui.end_row();
                        ui.label("Instances");
                        ui.label(format!("{}", asm.by_shape[sel.shape.0 as usize].len()));
                        ui.end_row();
                        let bb = m.node_bbox[sel.node.0 as usize];
                        if !bb.is_empty() {
                            let s = bb.size();
                            ui.label("Size");
                            ui.label(format!("{} × {} × {}", format_len(s.x, units_inch), format_len(s.y, units_inch), format_len(s.z, units_inch)));
                            ui.end_row();
                        }
                        if sel.face_slot != u32::MAX
                            && let Some(fi) = m.face_info(sel) {
                                ui.label("Body");
                                ui.label(format!("{} ({} tris)", fi.body_name, group_digits(fi.body_triangles as u64)));
                                ui.end_row();
                                ui.label("Face");
                                ui.label(format!("#{}  {:?}", sel.face_src, fi.surface_kind));
                                ui.end_row();
                                ui.label("Face tris");
                                ui.label(format!("{}", fi.triangles));
                                ui.end_row();
                                ui.label("Colour");
                                let c = fi.color;
                                let (r, _) = ui.allocate_exact_size(egui::vec2(40.0, 14.0), egui::Sense::hover());
                                ui.painter().rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]));
                                ui.end_row();
                                ui.label("Normal");
                                ui.label(format!("({:.3}, {:.3}, {:.3})", fi.normal.x, fi.normal.y, fi.normal.z));
                                ui.end_row();
                                ui.label("Point");
                                ui.label(format!("({:.2}, {:.2}, {:.2})", sel.point.x, sel.point.y, sel.point.z));
                                ui.end_row();
                            }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Hide").clicked() {
                            action = Some(PropAction::Hide);
                        }
                        if ui.button("Isolate").clicked() {
                            action = Some(PropAction::Isolate);
                        }
                        if ui.button("Fit").clicked() {
                            action = Some(PropAction::Fit(m.node_bbox[sel.node.0 as usize]));
                        }
                        if sel.face_slot != u32::MAX && ui.button("Section here").clicked()
                            && let Some(fi) = m.face_info(sel) {
                                action = Some(PropAction::SectionHere(fi.normal, sel.point));
                            }
                    });
                }
                _ => {
                    ui.weak("Click a face to select. Double-click to fit. Right-click parts in the tree for more.");
                }
            }
            match action {
                Some(PropAction::Hide) => app.hide_selected(),
                Some(PropAction::Isolate) => app.isolate_selected(),
                Some(PropAction::Fit(bb)) => {
                    if !bb.is_empty() {
                        app.camera.fit(bb);
                        app.camera_touched = true;
                    }
                }
                Some(PropAction::SectionHere(normal, point)) => {
                    app.tools.section_axis = SectionAxis::Face;
                    app.tools.section_normal = normal;
                    app.tools.section_enabled = true;
                    if let Some(vp) = &app.viewport {
                        let bb = vp.scene_bbox();
                        let n = normal.normalize_or_zero();
                        let c = bb.center();
                        let half = bb.size() * 0.5;
                        let extent = half.x * n.x.abs() + half.y * n.y.abs() + half.z * n.z.abs();
                        if extent > 0.0 {
                            app.tools.section_t = (((point - c).dot(n) / extent + 1.0) * 0.5).clamp(0.0, 1.0) as f32;
                        }
                    }
                }
                None => {}
            }
            ui.separator();

            ui.heading("Measure");
            ui.toggle_value(&mut app.tools.measure_active, "Pick two points");
            match app.tools.distance() {
                Some(d) => {
                    let [a, b] = [app.tools.measure_points[0], app.tools.measure_points[1]];
                    let dv = b - a;
                    ui.label(RichText::new(format!("Distance: {}", format_len(d, app.prefs.units_inch))).strong());
                    ui.label(format!("ΔX {}   ΔY {}   ΔZ {}", format_len(dv.x, app.prefs.units_inch), format_len(dv.y, app.prefs.units_inch), format_len(dv.z, app.prefs.units_inch)));
                }
                None if app.tools.measure_points.len() == 1 => {
                    ui.label("First point set — click the second point.");
                }
                None => {}
            }
            if !app.tools.measure_points.is_empty() && ui.small_button("Clear").clicked() {
                app.tools.measure_points.clear();
            }
            if let Some(vp) = &app.viewport {
                let bb = vp.scene_bbox();
                if !bb.is_empty() {
                    let s = bb.size();
                    ui.label(format!("Model bbox: {} × {} × {}", format_len(s.x, app.prefs.units_inch), format_len(s.y, app.prefs.units_inch), format_len(s.z, app.prefs.units_inch)));
                }
            }
            ui.separator();

            ui.heading("Section");
            ui.checkbox(&mut app.tools.section_enabled, "Enable clipping plane");
            ui.horizontal(|ui| {
                for (label, ax) in [("X", SectionAxis::X), ("Y", SectionAxis::Y), ("Z", SectionAxis::Z)] {
                    if ui.selectable_label(app.tools.section_axis == ax, label).clicked() {
                        app.tools.section_axis = ax;
                    }
                }
                if ui.selectable_label(app.tools.section_axis == SectionAxis::Face, "Face").on_hover_text("normal of the last picked face").clicked() {
                    app.tools.section_axis = SectionAxis::Face;
                }
                ui.checkbox(&mut app.tools.section_flip, "Flip");
            });
            ui.add(egui::Slider::new(&mut app.tools.section_t, 0.0..=1.0).text("offset"));
            ui.separator();

            if app.prefs.show_stats {
                ui.heading("Model");
                if let Some(m) = &app.model {
                    let asm = &m.structure.assembly;
                    let h = m.file.header();
                    egui::Grid::new("model").num_columns(2).striped(true).show(ui, |ui| {
                        ui.label("File");
                        ui.add(egui::Label::new(app.load.path.as_ref().and_then(|p| p.file_name()).and_then(|s| s.to_str()).unwrap_or("")).wrap());
                        ui.end_row();
                        ui.label("Schema");
                        ui.label(format!("{} ({})", h.ap().label(), h.schema.join(", ")));
                        ui.end_row();
                        ui.label("Written by");
                        ui.add(egui::Label::new(format!("{} — {}", h.originating_system, h.time_stamp)).wrap());
                        ui.end_row();
                        ui.label("Entities");
                        ui.label(group_digits(m.file.len() as u64));
                        ui.end_row();
                        ui.label("Products");
                        ui.label(format!("{} ({} occurrences)", asm.product_count, asm.occurrence_count));
                        ui.end_row();
                        ui.label("Shapes");
                        ui.label(format!("{} unique, {} instances", asm.shapes.len(), asm.instances.len()));
                        ui.end_row();
                        ui.label("Triangles");
                        ui.label(group_digits(m.instanced_triangles()));
                        ui.end_row();
                        let t = &app.load.timings;
                        ui.label("Timings");
                        ui.add(egui::Label::new(format!("index {:.0} ms · structure {:.0} ms · mesh {:.0} ms · total {:.0} ms", t.index_ms, t.structure_ms, t.mesh_ms, app.load.finished_at.unwrap_or(0.0))).wrap());
                        ui.end_row();
                        ui.label("Cache");
                        ui.label(match &app.load.cache {
                            Some(crate::pipeline::CacheOutcome::Hit) => "hit".to_string(),
                            Some(crate::pipeline::CacheOutcome::Written(_)) => "written".to_string(),
                            Some(crate::pipeline::CacheOutcome::Miss) => "miss".to_string(),
                            Some(crate::pipeline::CacheOutcome::Disabled) => "disabled".to_string(),
                            Some(crate::pipeline::CacheOutcome::Error(e)) => format!("error: {e}"),
                            None => "—".to_string(),
                        });
                        ui.end_row();
                        let d = app.load.diags.total() + m.meshes.iter().flatten().map(|x| x.diags.len() as u32).sum::<u32>();
                        ui.label("Diagnostics");
                        if ui.link(format!("{d}")).clicked() {
                            app.diag_open = true;
                        }
                        ui.end_row();
                    });
                }
            }
        });
    });
}

pub fn dialogs(app: &mut App, ctx: &egui::Context) {
    if app.about_open {
        egui::Window::new("StepView").open(&mut app.about_open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new(format!("StepView {}", env!("CARGO_PKG_VERSION"))).strong());
            ui.label("Fast native STEP viewer (pure Rust, Metal via wgpu).");
            ui.separator();
            egui::Grid::new("keys").num_columns(2).show(ui, |ui| {
                for (k, v) in [
                    ("Drag", "orbit"),
                    ("⇧ drag / middle / right drag", "pan"),
                    ("Scroll / pinch", "zoom to cursor"),
                    ("⇧ scroll", "pan"),
                    ("Rotate gesture", "roll"),
                    ("F", "fit all"),
                    ("1–7", "front, back, left, right, top, bottom, iso"),
                    ("S / W / X", "shaded+edges / wireframe / x-ray"),
                    ("H / ⇧H / I", "hide selected / show all / isolate"),
                    ("Click / double-click", "select face / fit to part"),
                    ("Esc", "clear selection"),
                    ("⌘N", "new window"),
                    ("⌘O / ⌘E / ⌘S / ⌘R", "open / export / screenshot / reload"),
                ] {
                    ui.label(RichText::new(k).monospace());
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
    }
    if app.diag_open {
        let mut open = app.diag_open;
        egui::Window::new("Diagnostics").open(&mut open).default_width(520.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                if app.load.diags.total() == 0 && app.model.as_ref().is_none_or(|m| m.meshes.iter().flatten().all(|x| x.diags.is_empty())) {
                    ui.label("No diagnostics. Every face was read and meshed cleanly.");
                }
                for (k, v) in &app.load.diags.counts {
                    ui.label(format!("{v:>6}  {k}"));
                }
                for d in app.load.diags.samples.iter().take(40) {
                    ui.weak(format!("  {:?} {} — {}", d.kind, d.entity.map(|e| e.to_string()).unwrap_or_default(), d.msg));
                }
                if let Some(m) = &app.model {
                    let mut counts: std::collections::BTreeMap<String, u32> = Default::default();
                    let mut samples = Vec::new();
                    for x in m.meshes.iter().flatten() {
                        for d in &x.diags {
                            *counts.entry(format!("{:?}", d.kind)).or_insert(0) += 1;
                            if samples.len() < 40 {
                                samples.push(format!("  {:?} face #{} — {}", d.kind, d.src, d.msg));
                            }
                        }
                    }
                    if !counts.is_empty() {
                        ui.separator();
                        ui.label("Tessellation:");
                        for (k, v) in &counts {
                            ui.label(format!("{v:>6}  {k}"));
                        }
                        for s in samples {
                            ui.weak(s);
                        }
                    }
                }
            });
        });
        app.diag_open = open;
    }
}
