//! Viewport panes: layout, panel menus, mouse interaction (navigation, selection, manipulators), HUD.

use crate::app::{IncaApp, Layout, Tool};
use crate::theme;
use crate::viewport::{self, CamView, Comp, LightMode, ShadeMode};
use egui::{pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke};
use glam::{DMat4, DQuat, DVec3};
use inca_core::mesh::epair;
use inca_core::scene::{CompMode, SelMode, Shape};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use transform_gizmo_egui::math::Transform;
use transform_gizmo_egui::prelude::*;

/// Camera navigation mode of an Alt+drag in progress (JS `onDown`).
#[derive(Clone, Copy, PartialEq)]
pub enum NavMode {
    Tumble,
    Track,
    Dolly,
}

/// A Shift+MMB / Shift+LMB drag waiting for enough movement to choose an axis (JS `middleDrag` auto mode).
#[derive(Clone, Copy)]
pub struct AutoDrag {
    pub vp: usize,
    pub button: egui::PointerButton,
    /// press position; the axis is chosen from the drag since this point
    pub origin: Pos2,
    pub tool: Tool,
    pub pivot: DVec3,
    pub rot: DQuat,
}

pub struct VpState {
    pub gizmos: Vec<Gizmo>,
    pub drag: Option<GizmoDrag>,
    pub marquee: Option<(usize, Pos2)>,
    /// Lasso outline / paint stroke points of the drag in progress.
    pub stroke: Vec<Pos2>,
    pub nav: Option<(usize, NavMode)>,
    pub snap_step: f64,
    pub snap_angle: f64,
    /// points placed by the curve / polygon creation tools
    pub tool_pts: Vec<DVec3>,
    /// the manipulator's active handle per mode (move, rotate, scale), as JS `Manip.activeAxis`;
    /// MMB drags it from anywhere and a click on a handle remembers it
    pub active_handles: [Option<GizmoMode>; 3],
    /// an MMB manipulator drag is in progress in this viewport
    pub mmb: Option<usize>,
    /// a forced handle drag in this viewport (Ctrl+LMB plane / auto axis) and the button driving it
    pub force_handle: Option<(usize, GizmoMode, egui::PointerButton)>,
    /// a Shift+drag waiting to choose the axis nearest the mouse direction
    pub auto_drag: Option<AutoDrag>,
    /// the handle under the cursor when the button went down
    pub press_handle: Option<GizmoMode>,
    /// Three Point Arc editing: point being dragged (0-2, 3 = centre) and the last drag position
    pub arc_drag: Option<usize>,
    pub arc_anchor: Option<DVec3>,
}

impl Default for VpState {
    fn default() -> Self {
        VpState {
            gizmos: (0..4).map(|_| Gizmo::default()).collect(),
            drag: None,
            marquee: None,
            stroke: vec![],
            nav: None,
            snap_step: 1.0,
            snap_angle: 15.0,
            tool_pts: vec![],
            active_handles: [None; 3],
            mmb: None,
            force_handle: None,
            auto_drag: None,
            press_handle: None,
            arc_drag: None,
            arc_anchor: None,
        }
    }
}

/// Manipulator mode slot: move, rotate, scale (the JS `Manip.activeAxis` keys).
pub fn mode_index(tool: Tool) -> usize {
    match tool {
        Tool::Rotate => 1,
        Tool::Scale => 2,
        _ => 0,
    }
}

impl VpState {
    pub fn active_handle(&self, tool: Tool) -> Option<GizmoMode> {
        self.active_handles[mode_index(tool)]
    }
    pub fn reset_active_handle(&mut self, tool: Tool) {
        self.active_handles[mode_index(tool)] = None;
    }
}

/// Geometry snapshot at drag start: components move differently for meshes (polyTweak
/// history) and curves (CVs are the shape).
pub enum DragGeom {
    Mesh(inca_core::mesh::PolyMesh),
    Curve(inca_core::curves::Curve),
}

pub struct GizmoDrag {
    pub vp: usize,
    pub start: Transform,
    pub cur: Transform,
    pub objects: Vec<(String, DMat4)>,
    pub comps: Vec<(String, Vec<u32>, DragGeom)>,
    pub tool: Tool,
    /// Edit Pivot drag: start world pivots of the objects
    pub pivots: Option<Vec<DVec3>>,
    /// per comps entry: soft selection weights (empty = all 1) and symmetry partner map
    pub weights: Vec<Vec<f64>>,
    pub mirror: Vec<Option<Vec<i64>>>,
    /// right after an Extrude: the Move drag drives that node's translate (mesh, history index, start translate)
    pub extrude: Option<(String, usize, DVec3)>,
    /// the handle being dragged
    pub handle: Option<GizmoMode>,
}

impl VpState {
    pub fn live_meshes(&self) -> HashSet<String> {
        self.drag.as_ref().map(|d| d.comps.iter().map(|c| c.0.clone()).collect()).unwrap_or_default()
    }
}

fn tf_matrix(t: &Transform) -> DMat4 {
    DMat4::from_scale_rotation_translation(DVec3::from(t.scale), DQuat::from(t.rotation), DVec3::from(t.translation))
}

pub fn viewports(app: &mut IncaApp, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
    let full = ui.available_rect_before_wrap();
    let panes: Vec<(usize, Rect)> = match app.layout {
        Layout::Single => vec![(app.active_vp, full)],
        Layout::Four => {
            let (w, h) = (full.width() / 2.0, full.height() / 2.0);
            let r = |x: f32, y: f32| Rect::from_min_size(pos2(full.min.x + x * w, full.min.y + y * h), vec2(w, h)).shrink(1.0);
            vec![(1, r(0.0, 0.0)), (0, r(1.0, 0.0)), (2, r(0.0, 1.0)), (3, r(1.0, 1.0))]
        }
        Layout::TwoSide => {
            let w = full.width() / 2.0;
            vec![(0, Rect::from_min_size(full.min, vec2(w, full.height())).shrink(1.0)), (1, Rect::from_min_size(pos2(full.min.x + w, full.min.y), vec2(w, full.height())).shrink(1.0))]
        }
        Layout::TwoStack => {
            let h = full.height() / 2.0;
            vec![(0, Rect::from_min_size(full.min, vec2(full.width(), h)).shrink(1.0)), (2, Rect::from_min_size(pos2(full.min.x, full.min.y + h), vec2(full.width(), h)).shrink(1.0))]
        }
        Layout::Three => {
            let (w, h) = (full.width() / 2.0, full.height() / 2.0);
            let r = |x: f32, y: f32, hh: f32| Rect::from_min_size(pos2(full.min.x + x * w, full.min.y + y * h), vec2(w, hh)).shrink(1.0);
            vec![(0, r(0.0, 0.0, full.height())), (1, r(1.0, 0.0, h)), (2, r(1.0, 1.0, h))]
        }
    };
    ui.allocate_rect(full, Sense::hover());
    for (idx, rect) in panes {
        pane(app, ui, frame, idx, rect);
    }
    // drop GPU buffers of deleted meshes
    if let Some(g) = app.gpu.as_mut() {
        let keep: HashSet<String> = app.s.nodes.values().filter(|n| n.mesh().is_some()).map(|n| n.id.clone()).collect();
        g.retain_meshes(&keep);
    }
    // drop cached mesh statistics of deleted meshes
    let alive: HashSet<&str> = app.s.nodes.keys().map(|k| k.as_str()).collect();
    app.stats_cache.retain(|k, _| alive.contains(k.as_str()));
    app.mesh_cache.retain(&alive);
}

fn pane(app: &mut IncaApp, ui: &mut egui::Ui, frame: &mut eframe::Frame, idx: usize, rect: Rect) {
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 20.0));
    let tools = Rect::from_min_size(pos2(rect.min.x, bar.max.y), vec2(rect.width(), 24.0));
    let img_rect = Rect::from_min_max(pos2(rect.min.x, tools.max.y), rect.max);
    // ---- panel menu bar and icon toolbar
    ui.painter().rect_filled(bar.union(tools), 0.0, theme::BG);
    let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    panel_menus(app, &mut bar_ui, idx);
    let mut tool_ui = ui.new_child(egui::UiBuilder::new().max_rect(tools.shrink2(vec2(2.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    tool_ui.set_clip_rect(tools);
    panel_toolbar(app, &mut tool_ui, idx);
    app.vps[idx].rect = img_rect;
    let ppp = ui.ctx().pixels_per_point();
    let px = [(img_rect.width() * ppp).round().max(1.0) as u32, (img_rect.height() * ppp).round().max(1.0) as u32];
    let cv = viewport::cam_view(&app.s, &app.vps[idx].camera.clone(), [img_rect.width(), img_rect.height()]);
    // ---- render
    if let (Some(rs), Some(gpu)) = (frame.wgpu_render_state(), app.gpu.as_mut()) {
        let tex = {
            let mut r = rs.renderer.write();
            gpu.target(&mut r, idx, px)
        };
        let live = app.vp_state.live_meshes();
        let soft = app.soft.on.then_some((app.soft.radius, app.soft.linear));
        let mut draws = viewport::build_draws(&app.s, &app.vps[idx], &cv, gpu, &mut app.smooth_cache, &live, &mut app.mesh_cache, soft);
        draws.extend(crate::anim_extras::draws(&app.viz));
        let fu = viewport::frame_uniform(&app.s, &app.vps[idx], &cv, app.bg_mode);
        gpu.render(idx, &fu, &draws);
        ui.painter().image(tex, img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    } else {
        ui.painter().rect_filled(img_rect, 0.0, Color32::from_rgb(60, 64, 70));
        ui.painter().text(img_rect.center(), egui::Align2::CENTER_CENTER, "wgpu renderer unavailable", egui::FontId::proportional(14.0), Color32::WHITE);
    }
    let resp = ui.interact(img_rect, egui::Id::new(("vp", idx)), Sense::click_and_drag());
    let painter = ui.painter_at(img_rect);
    viewport::draw_particles(&app.s, &app.vps[idx], &cv, &painter);
    viewport::draw_annotations(&app.s, &app.vps[idx], &cv, &painter);
    crate::anim_extras::overlay(app, &cv, img_rect, &painter);
    let hover = if app.vp_state.drag.is_none() && app.tool.is_select() && app.prefs.preselect { resp.hover_pos() } else { None };
    let hover_comp = hover.and_then(|h| hover_pick(app, ui.ctx(), idx, &cv, h));
    viewport::draw_component_overlay(&app.s, &app.vps[idx], &cv, &painter, hover_comp.as_ref().map(|(id, c)| (id.as_str(), c)));
    viewport::draw_curve_components(&app.s, &app.vps[idx], &cv, &painter);
    viewport::draw_gate(&app.s, &app.vps[idx], &painter);
    mesh_overlays(app, &cv, img_rect, &painter);
    if (resp.hovered() || resp.dragged()) && app.active_vp != idx && (resp.clicked() || resp.drag_started() || ui.input(|i| i.pointer.any_pressed())) {
        app.active_vp = idx;
    }
    let active = app.active_vp == idx;
    if active && app.layout != Layout::Single {
        painter.rect_stroke(img_rect.shrink(0.5), 0.0, Stroke::new(2.0, theme::HI), egui::StrokeKind::Inside);
    }
    interact(app, ui, idx, &resp, &cv, &painter);
    hud(app, idx, &cv, &painter);
    if active {
        manip_size_readout(app, ui, img_rect);
        inview_editor(app, ui, img_rect);
    }
}

fn panel_menus(app: &mut IncaApp, ui: &mut egui::Ui, idx: usize) {
    ui.spacing_mut().button_padding = vec2(4.0, 0.0);
    let small = |t: &str| RichText::new(t).size(11.5);
    let cam = app.vps[idx].camera.clone();
    let check = |ui: &mut egui::Ui, on: bool, label: &str| ui.add(egui::Button::new(format!("{} {label}", if on { "✔" } else { "  " })));
    ui.menu_button(small("View"), |ui| {
        if ui.button("Select Camera").clicked() {
            select_camera(app, idx);
            ui.close();
        }
        if ui.button("Default View").clicked() {
            viewport::default_view(&mut app.s, &cam);
            ui.close();
        }
        if ui.button("Look at Selection").clicked() {
            let b = viewport::selection_bbox(&app.s);
            if !b.is_empty() {
                viewport::look_at(&mut app.s, &cam, b.center());
            }
            ui.close();
        }
        if ui.add(egui::Button::new("Frame All").shortcut_text("A")).clicked() {
            app.active_vp = idx;
            app.action("frameAll");
            ui.close();
        }
        if ui.add(egui::Button::new("Frame Selection").shortcut_text("F")).clicked() {
            app.active_vp = idx;
            app.action("frameSelected");
            ui.close();
        }
        ui.separator();
        ui.menu_button("Camera Settings", |ui| {
            let v = &mut app.vps[idx];
            if check(ui, v.res_gate, "Resolution Gate").clicked() {
                v.res_gate = !v.res_gate;
                v.film_gate = false;
            }
            if check(ui, v.film_gate, "Film Gate").clicked() {
                v.film_gate = !v.film_gate;
                v.res_gate = false;
            }
        });
        if ui.button("Camera Attribute Editor...").clicked() {
            select_camera(app, idx);
            app.action("attributeEditor");
            ui.close();
        }
        ui.separator();
        ui.menu_button("Predefined Bookmarks", |ui| {
            for (l, t, r) in BOOKMARKS {
                if ui.button(*l).clicked() {
                    let ortho = viewport::camera_id(&app.s, &cam).and_then(|id| app.s.node(&id)).is_some_and(|n| matches!(&n.shape, Shape::Camera(c) if c.ortho));
                    if !ortho {
                        viewport::set_view(&mut app.s, &cam, *t, *r, Some((t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt()));
                    }
                    ui.close();
                }
            }
        });
    });
    ui.menu_button(small("Shading"), |ui| {
        let v = &mut app.vps[idx];
        ui.radio_value(&mut v.shade, ShadeMode::Wire, "Wireframe  4");
        ui.radio_value(&mut v.shade, ShadeMode::Shaded, "Smooth Shade All  5");
        ui.separator();
        ui.checkbox(&mut v.wire_on_shaded, "Wireframe on Shaded");
        ui.checkbox(&mut v.xray, "X-Ray  Alt+A");
        ui.checkbox(&mut v.backface, "Backface Culling");
        ui.separator();
        let mut tex = v.shade == ShadeMode::Textured;
        if ui.checkbox(&mut tex, "Hardware Texturing  6").clicked() {
            v.shade = if tex { ShadeMode::Textured } else { ShadeMode::Shaded };
        }
    });
    ui.menu_button(small("Lighting"), |ui| {
        let v = &mut app.vps[idx];
        ui.radio_value(&mut v.lights, LightMode::Default, "Use Default Lighting");
        ui.radio_value(&mut v.lights, LightMode::All, "Use All Lights  7");
        ui.radio_value(&mut v.lights, LightMode::Flat, "Use Flat Lighting");
        ui.radio_value(&mut v.lights, LightMode::None, "Use No Lights");
        ui.separator();
        ui.add_enabled(false, egui::Button::new("Shadows (planned)"));
    });
    ui.menu_button(small("Show"), |ui| {
        ui.menu_button("Isolate Select", |ui| {
            if check(ui, app.vps[idx].isolate.is_some(), "View Selected").clicked() {
                app.active_vp = idx;
                app.action("isolateSelect");
                ui.close();
            }
            if ui.button("Add Selected Objects").clicked() {
                let sel = app.s.sel.clone();
                let iso = app.vps[idx].isolate.get_or_insert_with(Vec::new);
                for id in sel {
                    if !iso.contains(&id) {
                        iso.push(id);
                    }
                }
                ui.close();
            }
            if ui.button("Remove Selected Objects").clicked() {
                let sel = app.s.sel.clone();
                if let Some(iso) = app.vps[idx].isolate.as_mut() {
                    iso.retain(|i| !sel.contains(i));
                }
                ui.close();
            }
        });
        ui.separator();
        let v = &mut app.vps[idx];
        if ui.button("All").clicked() {
            v.show_all(true);
            v.hud = true;
            v.show_manips = true;
        }
        if ui.button("None").clicked() {
            v.show_all(false);
        }
        ui.separator();
        ui.checkbox(&mut v.show_curves, "NURBS Curves");
        ui.checkbox(&mut v.show_meshes, "Polygons");
        ui.checkbox(&mut v.show_lights, "Lights");
        ui.checkbox(&mut v.show_cameras, "Cameras");
        ui.checkbox(&mut v.show_locators, "Locators");
        ui.separator();
        ui.checkbox(&mut v.grid, "Grid");
        ui.checkbox(&mut v.show_manips, "Manipulators");
        ui.checkbox(&mut v.show_sel_highlight, "Selection Highlighting");
        ui.checkbox(&mut v.hud, "HUD");
    });
    ui.menu_button(small("Renderer"), |ui| {
        check(ui, true, "Viewport 2.0 (Inca wgpu)");
        if ui.button("   Inca Path Tracer (Render View)").clicked() {
            app.action("renderView");
            ui.close();
        }
    });
    ui.menu_button(small("Panels"), |ui| {
        let cams: Vec<(String, bool)> = app.s.all_dag(true).into_iter().filter_map(|id| app.s.node(&id)).filter_map(|n| match &n.shape {
            Shape::Camera(c) => Some((n.name.clone(), c.ortho)),
            _ => None,
        }).collect();
        ui.menu_button("Perspective", |ui| {
            for (c, _) in cams.iter().filter(|c| !c.1) {
                if check(ui, app.vps[idx].camera == *c, c).clicked() {
                    app.vps[idx].camera = c.clone();
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("New").clicked() {
                let before: HashSet<String> = app.s.nodes.keys().cloned().collect();
                app.action("createCamera");
                if let Some(n) = app.s.nodes.values().find(|n| !before.contains(&n.id) && matches!(n.shape, Shape::Camera(_))) {
                    app.vps[idx].camera = n.name.clone();
                }
                ui.close();
            }
        });
        ui.menu_button("Orthographic", |ui| {
            for (c, _) in cams.iter().filter(|c| c.1) {
                if check(ui, app.vps[idx].camera == *c, c).clicked() {
                    app.vps[idx].camera = c.clone();
                    ui.close();
                }
            }
        });
        if ui.button("Look Through Selected").clicked() {
            match app.s.sel.iter().find(|i| matches!(app.s.node(i).map(|n| &n.shape), Some(Shape::Camera(_)))).cloned() {
                Some(c) => app.vps[idx].camera = app.s.name_of(&c),
                None => app.last_result = "Warning: Select a camera".into(),
            }
            ui.close();
        }
        ui.separator();
        ui.menu_button("Panel", |ui| {
            for (l, a) in [("Outliner", "outlinerWindow"), ("Graph Editor", "graphEditor"), ("Dope Sheet", "dopeSheet"), ("Hypershade", "hypershade"), ("Render View", "renderView")] {
                if ui.button(l).clicked() {
                    app.action(a);
                    ui.close();
                }
            }
        });
        ui.menu_button("Layouts", |ui| {
            for (l, lay) in [("Single Pane", Layout::Single), ("Two Panes Side by Side", Layout::TwoSide), ("Two Panes Stacked", Layout::TwoStack), ("Three Panes Split Left", Layout::Three), ("Four Panes", Layout::Four)] {
                if check(ui, app.layout == lay, l).clicked() {
                    app.active_vp = idx;
                    app.layout = lay;
                    ui.close();
                }
            }
        });
    });
}

/// View > Predefined Bookmarks (label, translate, rotate) from the JS panel menu.
const BOOKMARKS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("Perspective", [28.0, 21.0, 28.0], [-27.938, 45.0, 0.0]),
    ("Front", [0.0, 0.0, 40.0], [0.0, 0.0, 0.0]),
    ("Back", [0.0, 0.0, -40.0], [0.0, 180.0, 0.0]),
    ("Left", [-40.0, 0.0, 0.0], [0.0, -90.0, 0.0]),
    ("Right", [40.0, 0.0, 0.0], [0.0, 90.0, 0.0]),
    ("Top", [0.0, 40.0, 0.0], [-90.0, 0.0, 0.0]),
    ("Bottom", [0.0, -40.0, 0.0], [90.0, 0.0, 0.0]),
];

fn select_camera(app: &mut IncaApp, idx: usize) {
    if let Some(id) = viewport::camera_id(&app.s, &app.vps[idx].camera) {
        app.select_names(&[id], SelMode::Replace);
    }
}

/// The panel's icon toolbar row (JS buildToolbar).
fn panel_toolbar(app: &mut IncaApp, ui: &mut egui::Ui, idx: usize) {
    use crate::icons::{self, separator};
    ui.spacing_mut().item_spacing.x = 1.0;
    let ib = |ui: &mut egui::Ui, i: &str, tip: &str, on: bool| icons::button_ex(ui, i, tip, on, 20.0, 16.0, true).clicked();
    if ib(ui, "selectCamera", "Select camera", false) {
        select_camera(app, idx);
    }
    if ib(ui, "camAttrs", "Camera attributes", false) {
        select_camera(app, idx);
        app.action("attributeEditor");
    }
    if ib(ui, "bookmark", "Default view", false) {
        let c = app.vps[idx].camera.clone();
        viewport::default_view(&mut app.s, &c);
    }
    separator(ui);
    let v = &mut app.vps[idx];
    if ib(ui, "grid", "Grid", v.grid) {
        v.grid = !v.grid;
    }
    if ib(ui, "filmGate", "Film gate", v.film_gate) {
        v.film_gate = !v.film_gate;
        v.res_gate = false;
    }
    if ib(ui, "resGate", "Resolution gate", v.res_gate) {
        v.res_gate = !v.res_gate;
        v.film_gate = false;
    }
    separator(ui);
    if ib(ui, "wireframe", "Wireframe (4)", v.shade == ShadeMode::Wire) {
        v.shade = ShadeMode::Wire;
    }
    if ib(ui, "smoothShade", "Smooth shade all (5)", v.shade == ShadeMode::Shaded) {
        v.shade = ShadeMode::Shaded;
    }
    if ib(ui, "wireOnShaded", "Wireframe on shaded", v.wire_on_shaded) {
        v.wire_on_shaded = !v.wire_on_shaded;
    }
    if ib(ui, "textured", "Textured (6)", v.shade == ShadeMode::Textured) {
        v.shade = if v.shade == ShadeMode::Textured { ShadeMode::Shaded } else { ShadeMode::Textured };
    }
    if ib(ui, "useLights", "Use all lights (7)", v.lights == LightMode::All) {
        v.lights = if v.lights == LightMode::All { LightMode::Default } else { LightMode::All };
    }
    icons::button_ex(ui, "shadows", "Shadows (planned)", false, 20.0, 16.0, false);
    separator(ui);
    let iso = v.isolate.is_some();
    if ib(ui, "isolate", "Isolate select", iso) {
        app.active_vp = idx;
        app.action("isolateSelect");
    }
    let v = &mut app.vps[idx];
    if ib(ui, "xray", "X-Ray (Alt+A)", v.xray) {
        v.xray = !v.xray;
    }
    if ib(ui, "backface", "Backface culling", v.backface) {
        v.backface = !v.backface;
    }
    if ib(ui, "hud", "Poly count HUD", app.hud.poly_count) {
        app.hud.poly_count = !app.hud.poly_count;
    }
    let v = &mut app.vps[idx];
    separator(ui);
    let r = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover()).0;
    icons::paint(ui, r.shrink(1.0), "exposure", Color32::WHITE);
    ui.add(egui::DragValue::new(&mut v.exposure).speed(0.02).fixed_decimals(2).range(-10.0..=10.0)).on_hover_text("Exposure");
    let r = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover()).0;
    icons::paint(ui, r.shrink(1.0), "gamma", Color32::WHITE);
    ui.add(egui::DragValue::new(&mut v.gamma).speed(0.01).fixed_decimals(2).range(0.1..=5.0)).on_hover_text("Gamma (display)");
    ui.add_space(4.0);
    egui::ComboBox::from_id_salt(("viewTransform", idx)).width(100.0).selected_text("sRGB gamma").show_ui(ui, |ui| {
        let _ = ui.selectable_label(true, "sRGB gamma");
    });
}

/// Selection mode from Maya modifiers: Shift toggles, Ctrl removes, Ctrl+Shift adds.
fn sel_mode(m: egui::Modifiers) -> SelMode {
    let ctrl = m.ctrl || m.command;
    match (ctrl, m.shift) {
        (true, true) => SelMode::Add,
        (true, false) => SelMode::Remove,
        (false, true) => SelMode::Toggle,
        _ => SelMode::Replace,
    }
}

fn interact(app: &mut IncaApp, ui: &mut egui::Ui, idx: usize, resp: &egui::Response, cv: &CamView, painter: &egui::Painter) {
    let (mods, pointer, scroll) = ui.input(|i| (i.modifiers, i.pointer.clone(), i.smooth_scroll_delta.y));
    // ---- marking menus: RMB (Shift / Ctrl variants), W / E / R + LMB
    if app.marking.is_some() {
        return;
    }
    let now = ui.input(|i| i.time);
    if resp.hovered() && !mods.alt {
        if let Some(p) = pointer.press_origin().filter(|_| pointer.button_pressed(egui::PointerButton::Secondary)) {
            crate::marking::open_rmb(app, idx, cv, p, mods, now);
            return;
        }
        if pointer.button_pressed(egui::PointerButton::Primary) {
            let held = ui.input(|i| [(egui::Key::W, Tool::Move), (egui::Key::E, Tool::Rotate), (egui::Key::R, Tool::Scale)].into_iter().find(|(k, _)| i.key_down(*k)).map(|x| x.1));
            if let (Some(t), Some(p)) = (held, pointer.press_origin()) {
                crate::marking::open_tool(app, idx, t, p, now);
                return;
            }
        }
    }
    let cam = app.vps[idx].camera.clone();
    // ---- navigation (Alt + mouse, wheel)
    if resp.hovered() && scroll != 0.0 {
        // JS wheel: dolly(-deltaY * 0.35); dolly uses exp(-d * 0.006)
        let amount = scroll as f64 * 0.0021;
        match resp.hover_pos() {
            Some(c) => dolly_at(app, idx, cv, c, amount),
            None => viewport::dolly(&mut app.s, &cam, amount),
        }
    }
    if mods.alt && resp.drag_started() {
        // Alt+LMB tumble, Alt+Ctrl+LMB dolly (the JS "boxzoom" branch), MMB track, RMB dolly
        let mode = if pointer.button_down(egui::PointerButton::Middle) {
            NavMode::Track
        } else if pointer.button_down(egui::PointerButton::Secondary) {
            NavMode::Dolly
        } else if mods.ctrl || mods.command {
            NavMode::Dolly
        } else {
            NavMode::Tumble
        };
        app.vp_state.nav = Some((idx, mode));
    }
    if let Some((v, mode)) = app.vp_state.nav {
        if v == idx {
            let d = pointer.delta();
            match mode {
                NavMode::Tumble => viewport::tumble(&mut app.s, &cam, d.x, d.y, app.prefs.ortho_tumble),
                NavMode::Track => viewport::track(&mut app.s, cv, &cam, d.x, d.y),
                NavMode::Dolly => viewport::dolly(&mut app.s, &cam, (d.x - d.y) as f64 * 0.0072),
            }
            if !pointer.any_down() {
                app.vp_state.nav = None;
            }
            ui.ctx().request_repaint();
            return;
        }
    }
    if mods.alt {
        return;
    }
    // ---- B + LMB drag sets the Soft Selection radius (Maya)
    if ui.input(|i| i.key_down(egui::Key::B)) && resp.dragged_by(egui::PointerButton::Primary) {
        app.soft.on = true;
        let k = cv.units_per_px();
        app.soft.radius = (app.soft.radius + resp.drag_delta().x as f64 * k).max(0.01);
        app.help = format!("Soft Select radius: {:.3}", app.soft.radius);
        if let Some(h) = resp.hover_pos() {
            let r = (app.soft.radius / k) as f32;
            painter.circle_stroke(h, r, Stroke::new(1.0, Color32::from_rgb(255, 200, 60)));
        }
        return;
    }
    // ---- mesh tools and creation tools take the clicks
    if crate::mesh_tools::is_mesh_tool(app.tool) {
        crate::mesh_tools::interact(app, idx, resp, cv, painter);
        return;
    }
    if crate::rig_tools::is_rig_tool(app.tool) {
        crate::rig_tools::interact(app, idx, resp, cv, painter);
        return;
    }
    if app.tool.is_creation() {
        creation_tool(app, idx, resp, cv, painter);
        return;
    }
    // ---- Channel Box virtual slider (MMB with channels picked); an LMB click disarms it
    if crate::panels::virtual_slider(app, resp) {
        return;
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        app.panels.cb_armed = false;
    }
    // ---- manipulator
    let gizmo_used = gizmo(app, ui, idx, resp, cv, painter);
    if gizmo_used {
        return;
    }
    // ---- selection: click, marquee, lasso or paint stroke (LMB)
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let p0 = p - resp.drag_delta();
            app.vp_state.marquee = Some((idx, p0));
            app.vp_state.stroke = vec![p0];
        }
    }
    if let Some((v, start)) = app.vp_state.marquee {
        if v == idx {
            if let Some(cur) = pointer.interact_pos() {
                let tool = app.tool;
                if tool != Tool::Select && app.vp_state.stroke.last().is_none_or(|l| l.distance(cur) > 2.0) {
                    app.vp_state.stroke.push(cur);
                }
                let stroke = app.vp_state.stroke.clone();
                let white = Stroke::new(1.0, Color32::WHITE);
                match tool {
                    Tool::Lasso => {
                        let mut pts = stroke.clone();
                        pts.push(stroke[0]);
                        painter.add(egui::Shape::dashed_line(&pts, white, 4.0, 3.0));
                    }
                    Tool::Paint => {
                        let r = app.paint_radius;
                        painter.add(egui::Shape::line(stroke.clone(), Stroke::new(r * 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 30))));
                        painter.circle_stroke(cur, r, Stroke::new(1.0, Color32::from_rgb(255, 80, 80)));
                    }
                    _ => {
                        painter.rect_stroke(Rect::from_two_pos(start, cur), 0.0, white, egui::StrokeKind::Inside);
                    }
                }
                if resp.drag_stopped() || !pointer.primary_down() {
                    app.vp_state.marquee = None;
                    app.vp_state.stroke.clear();
                    let mode = sel_mode(mods);
                    match tool {
                        Tool::Lasso if stroke.len() >= 3 => region_select(app, idx, cv, &|q| viewport::in_polygon(&stroke, q), None, mode),
                        Tool::Paint => {
                            let r = app.paint_radius;
                            // paint adds; Ctrl paints a deselection
                            let mode = if mods.ctrl { SelMode::Remove } else if mode == SelMode::Replace { SelMode::Add } else { mode };
                            region_select(app, idx, cv, &|q| viewport::on_stroke(&stroke, r, q), None, mode)
                        }
                        Tool::Lasso => {}
                        _ => {
                            let r = Rect::from_two_pos(start, cur);
                            if r.width() >= 3.0 || r.height() >= 3.0 {
                                region_select(app, idx, cv, &|q| r.contains(q), Some(r.center()), mode);
                            }
                        }
                    }
                }
            }
        }
    }
    if app.tool == Tool::Paint && resp.hovered() && app.vp_state.marquee.is_none() {
        if let Some(h) = resp.hover_pos() {
            painter.circle_stroke(h, app.paint_radius, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 80, 80, 160)));
        }
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        app.panels.ol_active = false;
        if let Some(p) = resp.interact_pointer_pos() {
            click_select(app, idx, cv, p, sel_mode(mods));
        }
    }
    // double-click: edge/face loops in component mode, Tool Settings on an object (JS onDbl)
    if resp.double_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            double_click(app, idx, cv, p, mods.shift);
        }
    }

}

/// Preselection under the cursor, cached: re-picking every frame is what made component
/// mode crawl on heavy meshes (the JS app also picks on pointer events only).
fn hover_pick(app: &mut IncaApp, ctx: &egui::Context, idx: usize, cv: &CamView, h: Pos2) -> Option<(String, Comp)> {
    let mode = app.s.comp_mode?;
    if !app.vps[idx].rect.contains(h) {
        return None;
    }
    let ids = app.s.hilite();
    let mut hh = std::collections::hash_map::DefaultHasher::new();
    idx.hash(&mut hh);
    h.x.to_bits().hash(&mut hh);
    h.y.to_bits().hash(&mut hh);
    match mode {
        CompMode::Vertex => 0u8,
        CompMode::Edge => 1,
        CompMode::Face => 2,
    }
    .hash(&mut hh);
    let vp = &app.vps[idx];
    vp.xray.hash(&mut hh);
    vp.backface.hash(&mut hh);
    vp.rect.width().to_bits().hash(&mut hh);
    vp.rect.height().to_bits().hash(&mut hh);
    for x in cv.view_proj().to_cols_array() {
        x.to_bits().hash(&mut hh);
    }
    // picking depends on the hilited meshes (and their geometry), not on unrelated scene edits
    for id in &ids {
        id.hash(&mut hh);
        if let Some(n) = app.s.node(id) {
            n.rev.hash(&mut hh);
        }
    }
    let key = hh.finish();
    if let Some((i, k, c)) = &app.hover_pick {
        if *i == idx && *k == key {
            return c.clone();
        }
    }
    // like the JS `onHover`, re-pick at most every 30 ms while the cursor moves: at 400k
    // faces an edge pick is ~15 ms and would otherwise nearly double the frame time
    if app.hover_pick.as_ref().is_some_and(|(i, _, _)| *i == idx) && app.hover_at.elapsed() < std::time::Duration::from_millis(30) {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        return app.hover_pick.as_ref().and_then(|(_, _, c)| c.clone());
    }
    let vp = app.vps[idx].clone();
    // the JS picker compares components across every highlighted mesh
    let found = viewport::pick_best_component(&app.s, &vp, cv, &ids, mode, h, &mut app.mesh_cache);
    app.hover_pick = Some((idx, key, found.clone()));
    app.hover_at = std::time::Instant::now();
    found
}

fn click_select(app: &mut IncaApp, idx: usize, cv: &CamView, p: Pos2, mode: SelMode) {
    if app.lock_selection {
        return;
    }
    let vp = app.vps[idx].clone();
    if let Some(cm) = app.s.comp_mode {
        let hil = app.s.hilite();
        let best = viewport::pick_best_component(&app.s, &vp, cv, &hil, cm, p, &mut app.mesh_cache);
        match best {
            Some((id, c)) => {
                let name = app.s.name_of(&id);
                let topo = app.mesh_cache.topo(&app.s, &id);
                // curve CVs echo as `.cv[...]` (the JS MEL parser)
                let is_curve = app.s.node(&id).is_some_and(|n| n.curve().is_some());
                let comp = match c {
                    Comp::V(v) => format!("{name}.{}[{v}]", if is_curve { "cv" } else { "vtx" }),
                    Comp::E(a, b) => format!("{name}.e[{}]", topo.as_ref().and_then(|t| t.edge(a, b)).unwrap_or(0)),
                    Comp::F(f) => format!("{name}.f[{f}]"),
                };
                let flag = match mode {
                    SelMode::Replace => "-r",
                    SelMode::Add => "-add",
                    SelMode::Toggle => "-tgl",
                    SelMode::Remove => "-d",
                };
                if mode == SelMode::Toggle {
                    // toggle a single component
                    let cs = app.s.comp.entry(id.clone()).or_default();
                    let on = match c {
                        Comp::V(v) => cs.v.contains(&v),
                        Comp::E(a, b) => cs.e.contains(&epair(a, b)),
                        Comp::F(f) => cs.f.contains(&f),
                    };
                    let _ = app.exec_mel(&format!("select {} {comp};", if on { "-d" } else { "-add" }), true);
                } else {
                    let _ = app.exec_mel(&format!("select {flag} {comp};", ), true);
                }
            }
            None => {
                // clicking another object in component mode selects and hilites it; the JS
                // keeps the current components and only clears them on a replace-click
                // over the already-hilited object (or empty space)
                if let Some(o) = pick_masked(app, &vp, cv, p) {
                    if !hil.contains(&o) {
                        let flag = if mode == SelMode::Replace { SelMode::Replace } else { SelMode::Add };
                        app.select_names(&[o], flag);
                        return;
                    }
                }
                if mode == SelMode::Replace {
                    for id in hil {
                        app.s.comp.remove(&id);
                    }
                    app.s.rev += 1;
                }
            }
        }
        return;
    }
    let hit = pick_masked(app, &vp, cv, p).and_then(|h| filter_picks(app, vec![h]).pop());
    match hit {
        Some(id) => app.select_names(&[id], mode),
        None => {
            if mode == SelMode::Replace {
                app.select_names(&[], SelMode::Replace);
            }
        }
    }
}

/// Object pick honouring the status-line selection mask, as the JS `visibleNodes`.
fn pick_masked(app: &mut IncaApp, vp: &viewport::Viewport, cv: &CamView, p: Pos2) -> Option<String> {
    let s = &app.s;
    let off = &app.sel_mask_off;
    viewport::pick_object_if(s, vp, cv, p, &mut app.mesh_cache, &|id| crate::toolbar::mask_allows_scene(s, off, id))
}

/// Wheel dolly; in ortho views keep the world point under the cursor fixed (JS `dolly(cursor)`).
fn dolly_at(app: &mut IncaApp, idx: usize, cv: &CamView, cursor: Pos2, amount: f64) {
    let cam = app.vps[idx].camera.clone();
    if !cv.ortho {
        viewport::dolly(&mut app.s, &cam, amount);
        return;
    }
    let rect = app.vps[idx].rect;
    let before = cv.ray(cursor, rect).origin;
    viewport::dolly(&mut app.s, &cam, amount);
    let after = viewport::cam_view(&app.s, &cam, [rect.width(), rect.height()]).ray(cursor, rect).origin;
    if let Some(id) = viewport::camera_id(&app.s, &cam) {
        if let Some(n) = app.s.nodes.get_mut(&id) {
            n.xf.t = (DVec3::from_array(n.xf.t) + before - after).to_array();
        }
        app.s.rev += 1;
    }
}

/// JS `onDbl`: double-clicking an edge selects its loop, a face selects the faces of the
/// edge ring under the cursor, and an object opens Tool Settings.
fn double_click(app: &mut IncaApp, idx: usize, cv: &CamView, p: Pos2, shift: bool) {
    if app.lock_selection {
        return;
    }
    let vp = app.vps[idx].clone();
    let Some(mode) = app.s.comp_mode else {
        if let Some(id) = pick_masked(app, &vp, cv, p) {
            if app.s.node(&id).is_some_and(|n| n.kind != "camera") {
                app.action("toolSettings");
            }
        }
        return;
    };
    if mode == CompMode::Vertex {
        return;
    }
    let mut ids = app.s.hilite();
    if mode == CompMode::Face {
        // the JS `forTool` picker uses hilited plus selected meshes (never curves)
        ids.retain(|id| app.s.node(id).is_some_and(|n| n.mesh().is_some()));
        for id in &app.s.sel {
            if !ids.contains(id) && app.s.node(id).is_some_and(|n| n.mesh().is_some()) {
                ids.push(id.clone());
            }
        }
    }
    let Some((id, Comp::E(a, b))) = viewport::pick_best_component(&app.s, &vp, cv, &ids, CompMode::Edge, p, &mut app.mesh_cache) else { return };
    let topo = app.mesh_cache.topo(&app.s, &id);
    let Some(pm) = app.s.node(&id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { return };
    let Some(topo) = topo else { return };
    let name = app.s.name_of(&id);
    let list: Vec<String> = if mode == CompMode::Edge {
        inca_core::mesh::edge_loop(pm, &topo, a, b).iter().filter_map(|&(x, y)| topo.edge(x, y)).map(|i| i.to_string()).collect()
    } else {
        inca_core::mesh::edge_ring_faces(pm, &topo, a, b).1.iter().map(|f| f.to_string()).collect()
    };
    if list.is_empty() {
        return;
    }
    let _ = app.exec_mel(&format!("select {} {name}.{}[{}];", if shift { "-add" } else { "-r" }, if mode == CompMode::Edge { "e" } else { "f" }, list.join(",")), true);
}

/// Status line settings applied to picked objects: masks, hierarchy mode and the lock.
fn filter_picks(app: &IncaApp, ids: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in ids {
        if !crate::toolbar::mask_allows(app, &id) {
            continue;
        }
        let mut top = id;
        if app.select_hierarchy {
            while let Some(p) = app.s.node(&top).and_then(|n| n.parent.clone()) {
                top = p;
            }
        }
        if !out.contains(&top) {
            out.push(top);
        }
    }
    out
}

fn region_select(app: &mut IncaApp, idx: usize, cv: &CamView, region: &dyn Fn(Pos2) -> bool, center: Option<Pos2>, mode: SelMode) {
    if app.lock_selection {
        return;
    }
    let vp = app.vps[idx].clone();
    if let Some(cm) = app.s.comp_mode {
        let mut parts = vec![];
        for id in app.s.hilite() {
            if !app.s.is_visible(&id) {
                continue;
            }
            let name = app.s.name_of(&id);
            let topo = app.mesh_cache.topo(&app.s, &id);
            let is_curve = app.s.node(&id).is_some_and(|n| n.curve().is_some());
            for c in viewport::region_components(&app.s, &vp, cv, &id, cm, region, &mut app.mesh_cache) {
                parts.push(match c {
                    Comp::V(v) => format!("{name}.{}[{v}]", if is_curve { "cv" } else { "vtx" }),
                    Comp::E(a, b) => format!("{name}.e[{}]", topo.as_ref().and_then(|t| t.edge(a, b)).unwrap_or(0)),
                    Comp::F(f) => format!("{name}.f[{f}]"),
                });
            }
        }
        let flag = match mode {
            SelMode::Replace => "-r",
            SelMode::Remove => "-d",
            _ => "-add",
        };
        if parts.is_empty() {
            if mode == SelMode::Replace {
                for id in app.s.hilite() {
                    app.s.comp.remove(&id);
                }
                app.s.rev += 1;
            }
            return;
        }
        // keep the echo short for big selections
        let mel = format!("select {flag} {};", parts.join(" "));
        let _ = app.exec_mel(&mel, parts.len() < 200);
        return;
    }
    let ids = {
        let s = &app.s;
        viewport::region_objects(s, &vp, cv, region, center, &mut app.mesh_cache)
    };
    let ids = filter_picks(app, ids);
    app.select_names(&ids, if mode == SelMode::Toggle && ids.is_empty() { SelMode::Add } else { mode });
}


/// Move / rotate / scale manipulator. Returns true while it owns the mouse.
/// Transform channels picked in the Channel Box (all translate, all rotate or all scale): the
/// manipulator shows just those axes, as in Maya.
fn channel_modes(app: &IncaApp) -> Option<(Tool, EnumSet<GizmoMode>)> {
    if !app.panels.cb_armed || app.panels.cb_sel.is_empty() || app.s.comp_mode.is_some() {
        return None;
    }
    let mut kind = None;
    let mut modes = EnumSet::new();
    for c in &app.panels.cb_sel {
        let (k, axis) = c.split_at(c.len().saturating_sub(1));
        let (tool, m) = match (k, axis) {
            ("translate", "X") => (Tool::Move, GizmoMode::TranslateX),
            ("translate", "Y") => (Tool::Move, GizmoMode::TranslateY),
            ("translate", "Z") => (Tool::Move, GizmoMode::TranslateZ),
            ("rotate", "X") => (Tool::Rotate, GizmoMode::RotateX),
            ("rotate", "Y") => (Tool::Rotate, GizmoMode::RotateY),
            ("rotate", "Z") => (Tool::Rotate, GizmoMode::RotateZ),
            ("scale", "X") => (Tool::Scale, GizmoMode::ScaleX),
            ("scale", "Y") => (Tool::Scale, GizmoMode::ScaleY),
            ("scale", "Z") => (Tool::Scale, GizmoMode::ScaleZ),
            _ => return None,
        };
        if kind.is_some_and(|t| t != tool) {
            return None;
        }
        kind = Some(tool);
        modes.insert(m);
    }
    kind.map(|k| (k, modes))
}

/// The manipulator handle under the cursor (probing each handle on its own).
fn handle_at(config: &GizmoConfig, cursor: Pos2) -> Option<GizmoMode> {
    // planes / centre last: an axis drawn over a plane wins
    let mut order: Vec<GizmoMode> = config.modes.iter().collect();
    order.sort_by_key(|m| (m.axes().len(), matches!(m, GizmoMode::TranslateView | GizmoMode::RotateView | GizmoMode::ScaleUniform)));
    order.into_iter().find(|m| {
        let mut c = *config;
        c.modes = EnumSet::only(*m);
        c.mode_override = None;
        Gizmo::new(c).pick_preview((cursor.x, cursor.y))
    })
}

/// The central handle of a manipulator (JS `XYZ` / `E`).
fn is_center_handle(m: GizmoMode) -> bool {
    matches!(m, GizmoMode::TranslateView | GizmoMode::RotateView | GizmoMode::ScaleUniform | GizmoMode::Arcball)
}

/// Ctrl on an axis locks it: the drag happens in the plane of the other two (JS `PLANE_OF`).
///
/// transform-gizmo's plane modes are named after the corner their handle is drawn at, not after
/// the axes they free: `TranslateXY` sits between +Y and +Z and its plane normal is X, so it
/// keeps X fixed (the YZ plane); `TranslateYZ` keeps Z fixed (the XY plane). `XZ` is symmetric.
/// The scale plane modes follow the same convention.
fn plane_of(m: GizmoMode) -> GizmoMode {
    match m {
        GizmoMode::TranslateX => GizmoMode::TranslateXY,
        GizmoMode::TranslateY => GizmoMode::TranslateXZ,
        GizmoMode::TranslateZ => GizmoMode::TranslateYZ,
        GizmoMode::ScaleX => GizmoMode::ScaleXY,
        GizmoMode::ScaleY => GizmoMode::ScaleXZ,
        GizmoMode::ScaleZ => GizmoMode::ScaleYZ,
        _ => m,
    }
}

/// The X / Y / Z handle for a translate or scale mode (JS auto-axis drag).
fn axis_handle(tool: Tool, i: usize) -> GizmoMode {
    if tool == Tool::Scale {
        [GizmoMode::ScaleX, GizmoMode::ScaleY, GizmoMode::ScaleZ][i]
    } else {
        [GizmoMode::TranslateX, GizmoMode::TranslateY, GizmoMode::TranslateZ][i]
    }
}

/// Shift+drag: the axis whose screen direction best matches the drag (JS `middleDrag` auto).
fn auto_axis(tool: Tool, pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, origin: Pos2, cursor: Pos2) -> GizmoMode {
    let (dx, dy) = (cursor.x - origin.x, cursor.y - origin.y);
    let mut best = axis_handle(tool, 0);
    let mut bd = -1.0f32;
    if let Some(a) = cv.project(pivot, rect) {
        for (i, axis) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate() {
            let Some(b) = cv.project(pivot + rot * axis, rect) else { continue };
            let (sx, sy) = (b.x - a.x, b.y - a.y);
            let l = (sx * sx + sy * sy).sqrt();
            if l < 1e-6 {
                continue;
            }
            let d = ((sx * dx + sy * dy) / l).abs();
            if d > bd {
                bd = d;
                best = axis_handle(tool, i);
            }
        }
    }
    best
}

fn gizmo(app: &mut IncaApp, ui: &mut egui::Ui, idx: usize, resp: &egui::Response, cv: &CamView, painter: &egui::Painter) -> bool {
    let chan = channel_modes(app);
    if (!app.tool.is_manip() && chan.is_none()) || !app.vps[idx].show_manips {
        return false;
    }
    if let Some(d) = &app.vp_state.drag {
        if d.vp != idx {
            return false;
        }
    }
    // target frame: pivot + orientation
    let comp_targets: Vec<(String, Vec<u32>)> = if app.s.comp_mode.is_some() { app.s.hilite().into_iter().map(|id| { let v = app.s.comp_verts(&id); (id, v) }).filter(|(_, v)| !v.is_empty()).collect() } else { vec![] };
    let objects: Vec<String> = if app.s.comp_mode.is_some() { vec![] } else { app.s.sel.iter().filter(|i| app.s.node(i).is_some_and(|n| !n.startup())).cloned().collect() };
    if comp_targets.is_empty() && objects.is_empty() && app.vp_state.drag.is_none() {
        return false;
    }
    let pivot_mode = app.pivot_mode && app.s.comp_mode.is_none();
    let tool = chan.map(|c| c.0).unwrap_or(app.tool);
    let (pivot, rot) = if let Some(d) = &app.vp_state.drag {
        (DVec3::from(d.cur.translation), DQuat::from(d.cur.rotation))
    } else if !comp_targets.is_empty() {
        let mut c = DVec3::ZERO;
        let mut n = 0;
        // the JS `placeProxy`: component rotate/scale (and object-space translate) use the
        // last hilited object's world quaternion for the manipulator axes
        let comp_local = tool != Tool::Move || !app.space_world(Tool::Move);
        let mut rot = DQuat::IDENTITY;
        for (id, vs) in &comp_targets {
            let wm = app.s.world_matrix(id);
            let Some(node) = app.s.node(id) else { continue };
            for v in vs {
                let p = match node.curve() {
                    Some(cu) => cu.cvs.get(*v as usize).copied(),
                    None => node.mesh().and_then(|m| m.mesh.v.get(*v as usize).copied()),
                };
                if let Some(p) = p {
                    c += wm.transform_point3(p);
                    n += 1;
                }
            }
            if comp_local {
                rot = wm.to_scale_rotation_translation().1;
            }
        }
        (c / n.max(1) as f64, rot)
    } else {
        let lead = objects.last().unwrap();
        let rot = if app.space_world(tool) && tool != Tool::Scale { DQuat::IDENTITY } else { app.s.world_matrix(lead).to_scale_rotation_translation().1 };
        (app.s.world_pivot(lead), rot)
    };
    let modes = match tool {
        _ if pivot_mode => GizmoMode::all_translate(),
        _ if chan.is_some() => chan.unwrap().1,
        Tool::Move => GizmoMode::all_translate(),
        Tool::Rotate => GizmoMode::all_rotate(),
        _ => GizmoMode::all_scale(),
    };
    // the active handle must be one of the shown handles (JS remembers one per mode)
    let mode_tool = if pivot_mode { Tool::Move } else { tool };
    let mi = mode_index(mode_tool);
    let default_handle = match mode_tool {
        Tool::Rotate => GizmoMode::RotateView,
        Tool::Scale => GizmoMode::ScaleUniform,
        _ => GizmoMode::TranslateView,
    };
    let mut active = app.vp_state.active_handles[mi].filter(|m| modes.contains(*m)).or_else(|| modes.contains(default_handle).then_some(default_handle)).or_else(|| modes.iter().next());
    let ppp = ui.ctx().pixels_per_point();
    let space_world = app.space_world(tool);
    let g = &mut app.vp_state.gizmos[idx];
    let mut visuals = GizmoVisuals { gizmo_size: 75.0 * app.manip_size, stroke_width: 3.0, ..Default::default() };
    visuals.x_color = Color32::from_rgb(230, 57, 62);
    visuals.y_color = Color32::from_rgb(92, 214, 92);
    visuals.z_color = Color32::from_rgb(74, 132, 255);
    visuals.s_color = Color32::from_rgb(240, 225, 80);
    let pointer = ui.input(|i| i.pointer.clone());
    let mods = ui.input(|i| i.modifiers);
    let ctrl = mods.ctrl || mods.command;
    let cursor = pointer.hover_pos().unwrap_or_default();
    // a forced / MMB drag ends with its button
    if app.vp_state.force_handle.is_some_and(|(v, _, b)| v == idx && !pointer.button_down(b)) {
        app.vp_state.force_handle = None;
    }
    if app.vp_state.mmb == Some(idx) && !pointer.button_down(egui::PointerButton::Middle) {
        app.vp_state.mmb = None;
    }
    let mut base = GizmoConfig {
        view_matrix: cv.view.into(),
        projection_matrix: cv.proj.into(),
        viewport: resp.rect,
        modes,
        orientation: if space_world && tool != Tool::Scale && app.s.comp_mode.is_none() { GizmoOrientation::Global } else { GizmoOrientation::Local },
        // MMB: drag the active handle from anywhere in the view; Ctrl+LMB / auto-axis force a handle
        mode_override: app.vp_state.force_handle.filter(|f| f.0 == idx).map(|f| f.1).or(if app.vp_state.mmb == Some(idx) { active } else { None }),
        // hold X: grid snap while moving; hold J: discrete rotate (Maya)
        snapping: app.snap_grid || ui.input(|i| (app.tool == Tool::Move && i.key_down(egui::Key::X)) || (app.tool == Tool::Rotate && i.key_down(egui::Key::J))),
        snap_distance: app.vp_state.snap_step as f32,
        snap_angle: (app.vp_state.snap_angle as f32).to_radians(),
        snap_scale: 0.1,
        visuals,
        pixels_per_point: ppp,
        ..Default::default()
    };
    g.update_config(base);
    let tf = Transform::from_scale_rotation_translation(DVec3::ONE, rot, pivot);
    // A click / MMB press on a handle makes it the active one. Ctrl turns move/scale axes into
    // their drag plane and starts that drag at once; Shift on the centre (or Shift+MMB away from
    // a handle) picks the axis closest to the drag after 6 px, as the JS `middleDrag` does.
    let lmb_pressed = resp.hovered() && pointer.button_pressed(egui::PointerButton::Primary);
    let mmb_pressed = resp.hovered() && pointer.button_pressed(egui::PointerButton::Middle) && !mods.alt;
    let mut consume_primary = false;
    let mut start_mmb = false;
    let mut start_auto = false;
    let mut start_force: Option<GizmoMode> = None;
    let mut press = None;
    let hover = if lmb_pressed || mmb_pressed { handle_at(g.config(), cursor) } else { None };
    if let Some(h) = hover {
        let center = is_center_handle(h);
        let mut set_active = Some(h);
        let mut drive = h;
        if mmb_pressed {
            if ctrl && mode_tool != Tool::Rotate {
                drive = plane_of(h);
                set_active = Some(drive);
            }
            start_mmb = true;
            press = Some(drive);
        } else if lmb_pressed {
            if ctrl && !mods.shift && mode_tool != Tool::Rotate {
                drive = plane_of(h);
                set_active = Some(drive);
                start_force = Some(drive);
                press = Some(drive);
                consume_primary = true;
            } else if mods.shift && !ctrl && mode_tool != Tool::Rotate && center {
                // JS remembers the centre ('XYZ') until the drag picks an axis
                set_active = Some(h);
                start_auto = true;
                consume_primary = true;
            } else {
                press = Some(h);
            }
        }
        app.vp_state.active_handles[mi] = set_active;
    } else if mmb_pressed {
        // MMB away from a handle: Shift picks an axis from the drag, otherwise the remembered one
        if mods.shift && mode_tool != Tool::Rotate {
            start_auto = true;
        } else {
            start_mmb = true;
        }
    }
    if let Some(h) = start_force {
        app.vp_state.force_handle = Some((idx, h, egui::PointerButton::Primary));
    }
    if start_mmb {
        app.vp_state.mmb = Some(idx);
    }
    if lmb_pressed {
        app.vp_state.press_handle = press;
    }
    if start_auto {
        app.vp_state.auto_drag = Some(AutoDrag {
            vp: idx,
            button: if mmb_pressed { egui::PointerButton::Middle } else { egui::PointerButton::Primary },
            origin: pointer.press_origin().unwrap_or(cursor),
            tool: mode_tool,
            pivot,
            rot,
        });
    }
    // resolve a pending auto-axis drag; consume the press until it has a direction
    let mut auto_started = false;
    let mut auto_pending = false;
    let mut auto_cursor = cursor;
    if let Some(a) = app.vp_state.auto_drag.filter(|a| a.vp == idx) {
        if !pointer.button_down(a.button) {
            app.vp_state.auto_drag = None;
            auto_pending = true; // swallow the click release too
        } else if (cursor - a.origin).length() >= 6.0 {
            let h = auto_axis(a.tool, a.pivot, a.rot, cv, app.vps[idx].rect, a.origin, cursor);
            app.vp_state.active_handles[mode_index(a.tool)] = Some(h);
            app.vp_state.press_handle = Some(h);
            if a.button == egui::PointerButton::Middle {
                app.vp_state.mmb = Some(idx);
            } else {
                app.vp_state.force_handle = Some((idx, h, egui::PointerButton::Primary));
            }
            app.vp_state.auto_drag = None;
            auto_started = true;
            // start the drag at the press point, so the first threshold movement is not lost
            auto_cursor = a.origin;
        } else {
            auto_pending = true;
        }
    }
    active = app.vp_state.active_handles[mi].filter(|m| modes.contains(*m)).or_else(|| modes.contains(default_handle).then_some(default_handle)).or_else(|| modes.iter().next());
    let override_mode = app.vp_state.force_handle.filter(|f| f.0 == idx).map(|f| f.1).or(if app.vp_state.mmb == Some(idx) { active } else { None });
    if override_mode != g.config().mode_override {
        base.mode_override = override_mode;
        g.update_config(base);
    }
    let drag_handle = app.vp_state.force_handle.filter(|f| f.0 == idx).map(|f| f.1).or(if app.vp_state.mmb == Some(idx) { active } else { app.vp_state.press_handle });
    let interaction = GizmoInteraction {
        cursor_pos: if auto_started { (auto_cursor.x, auto_cursor.y) } else { (cursor.x, cursor.y) },
        hovered: !auto_pending && (resp.hovered() || app.vp_state.drag.is_some() || app.vp_state.mmb == Some(idx) || app.vp_state.force_handle.is_some_and(|f| f.0 == idx)),
        drag_started: !auto_pending && (lmb_pressed || (app.vp_state.mmb == Some(idx) && pointer.button_pressed(egui::PointerButton::Middle)) || auto_started),
        dragging: !auto_pending && (pointer.button_down(egui::PointerButton::Primary) || app.vp_state.mmb == Some(idx)),
    };
    let focused_before = g.is_focused();
    let mut result = g.update(interaction, &[tf]);
    if auto_started {
        // apply the movement already dragged past the 6 px threshold in this same frame
        result = g.update(GizmoInteraction { cursor_pos: (cursor.x, cursor.y), ..interaction }, &[tf]);
    }
    let draw = g.draw();
    // manipulator guide: a faint line along the axis being dragged (Preferences can hide it)
    if app.prefs.manip_guides {
        if let Some(d) = app.vp_state.drag.as_ref().filter(|d| d.vp == idx) {
            let axis = match d.handle {
                Some(GizmoMode::TranslateX | GizmoMode::ScaleX | GizmoMode::RotateX) => Some(DVec3::X),
                Some(GizmoMode::TranslateY | GizmoMode::ScaleY | GizmoMode::RotateY) => Some(DVec3::Y),
                Some(GizmoMode::TranslateZ | GizmoMode::ScaleZ | GizmoMode::RotateZ) => Some(DVec3::Z),
                _ => None,
            };
            if let Some(ax) = axis {
                let ax = DQuat::from(d.start.rotation) * ax;
                let o = DVec3::from(d.start.translation);
                let far = cv.coi.max(10.0) * 50.0;
                if let (Some(a), Some(b)) = (cv.project(o - ax * far, app.vps[idx].rect).or(cv.project(o, app.vps[idx].rect)), cv.project(o + ax * far, app.vps[idx].rect).or(cv.project(o, app.vps[idx].rect))) {
                    painter.line_segment([a, b], Stroke::new(1.0, Color32::from_rgba_unmultiplied(220, 220, 220, 90)));
                }
            }
        }
    }
    // Edit Pivot: a blue ring around the pivot, kept while dragging; an inner ring while point snap is on
    if pivot_mode {
        if let Some(c) = cv.project(pivot, app.vps[idx].rect) {
            let blue = Color32::from_rgb(95, 211, 255);
            let r = 14.0 * app.manip_size;
            painter.circle_stroke(c, r, Stroke::new(2.0, blue));
            if app.snap_point || ui.input(|i| i.key_down(egui::Key::V)) {
                painter.circle_stroke(c, r * 0.55, Stroke::new(2.0, blue));
            }
        }
    }
    painter.add(egui::Mesh {
        indices: draw.indices,
        vertices: draw.vertices.into_iter().zip(draw.colors).map(|(p, [r, gg, b, a])| egui::epaint::Vertex { pos: p.into(), uv: Pos2::ZERO, color: egui::Rgba::from_rgba_premultiplied(r, gg, b, a).into() }).collect(),
        ..Default::default()
    });
    match result {
        Some((_, new_tfs)) => {
            if app.vp_state.drag.is_none() {
                // drag start: snapshot what we move (one undo step per drag)
                let label = match app.tool {
                    Tool::Move => "move",
                    Tool::Rotate => "rotate",
                    _ => "scale",
                };
                app.s.checkpoint(label);
                let objs: Vec<(String, DMat4)> = objects.iter().filter(|o| !objects.iter().any(|p| p != *o && app.s.is_ancestor(p, o))).map(|o| (o.clone(), app.s.world_matrix(o))).collect();
                // soft selection widens the moved set with falloff weights; symmetry adds mirror partners
                let mut comps = vec![];
                let (mut weights, mut mirror) = (vec![], vec![]);
                for (id, v) in &comp_targets {
                    let Some(node) = app.s.node(id) else { continue };
                    // the JS applies soft selection to meshes only (`kind === 'mesh'`)
                    let soft_mesh = app.soft.on && node.mesh().is_some();
                    let (mut verts, mut w): (Vec<u32>, Vec<f64>) = if soft_mesh {
                        viewport::soft_weights(&app.s, id, v, app.soft.radius, app.soft.linear).into_iter().unzip()
                    } else {
                        (v.clone(), vec![])
                    };
                    let m = viewport::symmetry_partners(&app.s, id, &mut verts, &mut w, app.symmetry);
                    let geom = if let Shape::Mesh(m) = &node.shape {
                        DragGeom::Mesh(m.mesh.clone())
                    } else if let Some(c) = node.curve() {
                        DragGeom::Curve(c.clone())
                    } else {
                        continue;
                    };
                    comps.push((id.clone(), verts, geom));
                    weights.push(w);
                    mirror.push(m);
                }
                let pivots = pivot_mode.then(|| objs.iter().map(|(o, _)| app.s.world_pivot(o)).collect());
                // the extrude just made (still shown in the In-View Editor) takes the move, as in Maya/JS:
                // its divisions then spread along the moved distance
                let extrude = app.inview.clone().filter(|_| app.tool == Tool::Move && !pivot_mode).and_then(|(id, i)| {
                    let m = app.s.node(&id)?.mesh()?;
                    let h = m.history.get(i).filter(|h| h.ty == "polyExtrudeFace" && i + 1 == m.history.len())?;
                    let g = |k: &str| h.params.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    Some((id.clone(), i, DVec3::new(g("translateX"), g("translateY"), g("translateZ"))))
                });
                let (comps, weights, mirror) = if extrude.is_some() { (vec![], vec![], vec![]) } else { (comps, weights, mirror) };
                app.vp_state.drag = Some(GizmoDrag { vp: idx, start: tf, cur: tf, objects: objs, comps, tool: if pivot_mode { Tool::Move } else { app.tool }, pivots, weights, mirror, extrude, handle: drag_handle });
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            if let Some(n) = new_tfs.first() {
                d.cur = *n;
            }
            // centre handle with grid snap in a perspective view: slide on the ground plane through the
            // start point and snap X/Z to the grid (not every axis in 3D)
            if d.tool == Tool::Move && d.handle == Some(GizmoMode::TranslateView) && !cv.ortho && (app.snap_grid || ui.input(|i| i.key_down(egui::Key::X))) {
                let y0 = DVec3::from(d.start.translation).y;
                let ray = cv.ray(cursor, app.vps[idx].rect);
                if ray.dir.y.abs() > 1e-6 {
                    let t = (y0 - ray.origin.y) / ray.dir.y;
                    if t > 0.0 {
                        let p = ray.at(t);
                        let st = app.vp_state.snap_step.max(1e-6);
                        d.cur.translation = DVec3::new((p.x / st).round() * st, y0, (p.z / st).round() * st).into();
                    }
                }
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            // hold C (or the status line toggle): the moved pivot snaps to the nearest curve point
            if d.tool == Tool::Move && (app.snap_curve || ui.input(|i| i.key_down(egui::Key::C))) {
                let moving: HashSet<String> = d.objects.iter().map(|o| o.0.clone()).collect();
                if let Some(w) = snap_curve_point(&app.s, &app.vps[idx], cv, cursor, &moving) {
                    let d = app.vp_state.drag.as_mut().unwrap();
                    d.cur.translation = w.into();
                }
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            // hold V (or the status line toggle): the moved pivot snaps to the vertex nearest the cursor
            if d.tool == Tool::Move && (app.snap_point || ui.input(|i| i.key_down(egui::Key::V))) {
                let moving: HashSet<String> = d.comps.iter().map(|c| c.0.clone()).chain(d.objects.iter().map(|o| o.0.clone())).collect();
                if let Some(w) = snap_vertex(&app.s, &app.vps[idx], cv, cursor, &moving) {
                    let d = app.vp_state.drag.as_mut().unwrap();
                    d.cur.translation = w.into();
                }
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            if let Some((id, i, t0)) = d.extrude.clone() {
                let world = DVec3::from(d.cur.translation) - DVec3::from(d.start.translation);
                let local = app.s.world_matrix(&id).inverse().transform_vector3(world);
                let t = t0 + local;
                if let Some(h) = app.s.nodes.get_mut(&id).and_then(|n| n.mesh_mut()).and_then(|m| m.history.get_mut(i)) {
                    h.params.insert("translateX".into(), serde_json::json!(t.x));
                    h.params.insert("translateY".into(), serde_json::json!(t.y));
                }
                app.s.set_history_param(&id, i, "translateZ", serde_json::json!(t.z));
                return true;
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            let delta = tf_matrix(&d.cur) * tf_matrix(&d.start).inverse();
            let objs = d.objects.clone();
            let pivots = d.pivots.clone();
            let comps: Vec<(String, Vec<u32>, DragGeom)> = d.comps.iter().map(|(a, b, c)| match c {
                DragGeom::Mesh(pm) => (a.clone(), b.clone(), DragGeom::Mesh(pm.clone())),
                DragGeom::Curve(cu) => (a.clone(), b.clone(), DragGeom::Curve(cu.clone())),
            }).collect();
            let (weights, mirror) = (d.weights.clone(), d.mirror.clone());
            let sym = app.symmetry;
            match pivots {
                Some(pv) => {
                    for ((id, _), p0) in objs.iter().zip(pv) {
                        app.s.set_pivot_world(id, delta.transform_point3(p0));
                    }
                }
                None => {
                    for (id, start) in objs {
                        app.s.set_world_matrix(&id, delta * start);
                    }
                }
            }
            for (ci, (id, verts, orig)) in comps.into_iter().enumerate() {
                let wm = app.s.world_matrix(&id);
                let inv = wm.inverse();
                let local = inv * delta * wm;
                let w = &weights[ci];
                let src: &[DVec3] = match &orig {
                    DragGeom::Mesh(pm) => &pm.v,
                    DragGeom::Curve(c) => &c.cvs,
                };
                let mut out: Vec<DVec3> = verts.iter().enumerate().map(|(i, v)| {
                    let o = src[*v as usize];
                    o.lerp(local.transform_point3(o), w.get(i).copied().unwrap_or(1.0))
                }).collect();
                if let Some(map) = &mirror[ci] {
                    for (i, &src) in map.iter().enumerate() {
                        if src >= 0 {
                            let p = out[src as usize];
                            out[i] = if sym == 1 { viewport::mirror_point(p, sym) } else { inv.transform_point3(viewport::mirror_point(wm.transform_point3(p), sym)) };
                        }
                    }
                }
                match orig {
                    DragGeom::Mesh(_) => {
                        if let Some(m) = app.s.nodes.get_mut(&id).and_then(|n| n.mesh_mut()) {
                            for (v, p) in verts.iter().zip(out) {
                                m.mesh.v[*v as usize] = p;
                            }
                        }
                    }
                    // a curve has no history for its shape: the CVs are it (the JS rebuildShape)
                    DragGeom::Curve(_) => {
                        if let Some(c) = app.s.nodes.get_mut(&id).and_then(|n| n.curve_mut()) {
                            for (v, p) in verts.iter().zip(out) {
                                c.cvs[*v as usize] = p;
                            }
                        }
                    }
                }
                if let Some(n) = app.s.nodes.get_mut(&id) {
                    n.rev += 1;
                }
                app.s.refresh_dependents(&id);
            }
            app.s.touch();
            ui.ctx().request_repaint();
            true
        }
        None => {
            if auto_pending || consume_primary {
                return true;
            }
            if let Some(d) = app.vp_state.drag.take() {
                finish_drag(app, d);
                return true;
            }
            // a press on a handle belongs to the gizmo
            focused_before && pointer.primary_down()
        }
    }
}

/// Where a click lands: the ground grid in perspective, the view plane in orthographic views.
fn place_point(cv: &CamView, rect: Rect, p: Pos2, snap: Option<f64>) -> Option<DVec3> {
    let r = cv.ray(p, rect);
    let n = if cv.ortho { -cv.fwd } else { DVec3::Y };
    let d = r.dir.dot(n);
    if d.abs() < 1e-9 {
        return None;
    }
    let t = -r.origin.dot(n) / d;
    if t < 0.0 && !cv.ortho {
        return None;
    }
    let mut w = r.at(t);
    if let Some(st) = snap {
        w = (w / st).round() * st;
    }
    // keep exactly on the plane
    w -= n * w.dot(n);
    Some(w)
}

fn creation_tool(app: &mut IncaApp, idx: usize, resp: &egui::Response, cv: &CamView, painter: &egui::Painter) {
    let rect = app.vps[idx].rect;
    let snap = app.snap_grid.then_some(app.vp_state.snap_step);
    // a live surface takes the points (Make Live)
    let live = |app: &IncaApp, p: Pos2| crate::mesh_tools::live_point(app, cv, rect, p).or_else(|| place_point(cv, rect, p, snap));
    if app.tool == Tool::Pencil {
        // freehand: collect points while dragging, simplify and make an EP curve on release
        let wpp = cv.units_per_px();
        if resp.dragged_by(egui::PointerButton::Primary) || resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(w) = resp.interact_pointer_pos().and_then(|p| live(app, p)) {
                if app.vp_state.tool_pts.last().is_none_or(|l| l.distance(w) > wpp * 6.0) {
                    app.vp_state.tool_pts.push(w);
                }
            }
        }
        let sp: Vec<Pos2> = app.vp_state.tool_pts.iter().filter_map(|p| cv.project(*p, rect)).collect();
        painter.add(egui::Shape::line(sp, Stroke::new(1.5, Color32::from_rgb(255, 225, 74))));
        if resp.drag_stopped() {
            let pts = std::mem::take(&mut app.vp_state.tool_pts);
            let keep = simplify(&pts, wpp * 4.0);
            if keep.len() >= 2 {
                let f = inca_core::math::fmt_num;
                let eps: Vec<String> = keep.iter().map(|p| format!("-ep {} {} {}", f(p.x), f(p.y), f(p.z))).collect();
                let _ = app.run_mel("pencil curve", &format!("curve -d 3 {};", eps.join(" ")));
            }
        }
        return;
    }
    if app.tool == Tool::Arc && app.vp_state.tool_pts.len() == 3 && arc_edit(app, idx, resp, cv, painter) {
        return;
    }
    if app.tool == Tool::Joint {
        if resp.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos().and_then(|p| live(app, p)) {
                let f = inca_core::math::fmt_num;
                let _ = app.run_mel("joint", &format!("joint -p {} {} {};", f(p.x), f(p.y), f(p.z)));
            }
        }
        return;
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos().and_then(|p| live(app, p)) {
            app.vp_state.tool_pts.push(p);
            if app.tool == Tool::Distance && app.vp_state.tool_pts.len() == 2 {
                finish_creation(app);
            }
        }
    }
    if resp.clicked_by(egui::PointerButton::Secondary) {
        finish_creation(app);
    }
    // preview
    let pts = app.vp_state.tool_pts.clone();
    let mut preview = pts.clone();
    if let Some(h) = resp.hover_pos().and_then(|p| live(app, p)) {
        preview.push(h);
    }
    let line: Vec<DVec3> = match app.tool {
        Tool::CvCurve if preview.len() >= 2 => inca_core::curves::Curve::new(preview.clone(), 3.min(preview.len() - 1), "open", "cv").sample(12),
        Tool::EpCurve if preview.len() >= 2 => inca_core::curves::Curve::new(preview.clone(), 3, "open", "ep").sample(12),
        Tool::Arc if preview.len() == 3 => inca_core::curves::arc_through_points(preview[0], preview[2], preview[1], 0.05).unwrap_or(preview.clone()),
        Tool::Polygon if preview.len() >= 2 => {
            let mut l = preview.clone();
            l.push(preview[0]);
            l
        }
        _ => preview.clone(),
    };
    let sp: Vec<Pos2> = line.iter().filter_map(|p| cv.project(*p, rect)).collect();
    painter.add(egui::Shape::line(sp, Stroke::new(1.5, Color32::from_rgb(255, 225, 74))));
    for p in &pts {
        if let Some(q) = cv.project(*p, rect) {
            painter.rect_filled(Rect::from_center_size(q, vec2(6.0, 6.0)), 0.0, Color32::from_rgb(92, 255, 92));
        }
    }
}

/// Three Point Circular Arc, once its three points are placed: drag a point or the centre, click the
/// arc to flip it to the other side of the chord, live radius readout; Enter / RMB finishes,
/// Backspace steps back, Esc cancels. Returns true when it handled the input.
fn arc_edit(app: &mut IncaApp, idx: usize, resp: &egui::Response, cv: &CamView, painter: &egui::Painter) -> bool {
    let rect = app.vps[idx].rect;
    let snap = app.snap_grid.then_some(app.vp_state.snap_step);
    let [a, m, b] = [app.vp_state.tool_pts[0], app.vp_state.tool_pts[1], app.vp_state.tool_pts[2]];
    let yellow = Color32::from_rgb(255, 225, 74);
    let line = inca_core::curves::arc_through_points(a, b, m, 0.04).unwrap_or(vec![a, m, b]);
    let centre = circumcentre(a, m, b);
    let sp: Vec<Pos2> = line.iter().filter_map(|p| cv.project(*p, rect)).collect();
    painter.add(egui::Shape::line(sp.clone(), Stroke::new(2.0, yellow)));
    for p in [a, m, b] {
        if let Some(q) = cv.project(p, rect) {
            painter.rect_filled(Rect::from_center_size(q, vec2(8.0, 8.0)), 0.0, Color32::from_rgb(92, 255, 92));
        }
    }
    if let Some((c, r)) = centre {
        if let Some(q) = cv.project(c, rect) {
            painter.circle_stroke(q, 5.0, Stroke::new(1.5, yellow));
            painter.text(q + vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, format!("r = {}", inca_core::math::fmt_num(r)), egui::FontId::proportional(12.0), Color32::WHITE);
        }
    }
    app.help = "Three Point Arc: drag a point or the centre, click the arc to flip it; Enter finishes, Backspace steps back, Esc cancels".into();
    let press = resp.ctx.input(|i| i.pointer.press_origin());
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = press {
            let near = |w: DVec3| cv.project(w, rect).is_some_and(|q| q.distance(p) < 10.0);
            app.vp_state.arc_drag = [a, m, b].iter().position(|w| near(*w)).or_else(|| centre.filter(|(c, _)| near(*c)).map(|_| 3));
            app.vp_state.arc_anchor = place_point(cv, rect, p, None);
        }
    }
    if let (Some(k), Some(q)) = (app.vp_state.arc_drag, resp.interact_pointer_pos()) {
        if let Some(w) = place_point(cv, rect, q, snap) {
            if k < 3 {
                app.vp_state.tool_pts[k] = w;
            } else if let Some(anchor) = app.vp_state.arc_anchor {
                // the centre moves the whole arc
                let d = w - anchor;
                for p in app.vp_state.tool_pts.iter_mut() {
                    *p += d;
                }
                app.vp_state.arc_anchor = Some(w);
            }
        }
        if resp.drag_stopped() {
            app.vp_state.arc_drag = None;
        }
        return true;
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            if sp.windows(2).any(|w| viewport::seg_dist(p, w[0], w[1]) < 6.0) {
                // flip: mirror the middle point across the chord
                let u = (b - a).normalize_or_zero();
                let foot = a + u * (m - a).dot(u);
                app.vp_state.tool_pts[1] = foot * 2.0 - m;
            }
        }
        return true;
    }
    if resp.clicked_by(egui::PointerButton::Secondary) {
        finish_creation(app);
    }
    true
}

/// Centre and radius of the circle through three points.
fn circumcentre(a: DVec3, b: DVec3, c: DVec3) -> Option<(DVec3, f64)> {
    let (ab, ac) = (b - a, c - a);
    let n = ab.cross(ac);
    let d = 2.0 * n.length_squared();
    if d < 1e-12 {
        return None;
    }
    let o = a + (n.cross(ab) * ac.length_squared() + ac.cross(n) * ab.length_squared()) / d;
    Some((o, (o - a).length()))
}

/// Ramer–Douglas–Peucker: drop points closer than `tol` to the simplified polyline.
fn simplify(pts: &[DVec3], tol: f64) -> Vec<DVec3> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let ab = b - a;
    let dist = |p: DVec3| {
        let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        (a + ab * t - p).length()
    };
    let (i, d) = pts.iter().enumerate().skip(1).take(pts.len() - 2).map(|(i, p)| (i, dist(*p))).fold((0, 0.0), |m, x| if x.1 > m.1 { x } else { m });
    if d <= tol {
        return vec![a, b];
    }
    let mut left = simplify(&pts[..=i], tol);
    left.pop();
    left.extend(simplify(&pts[i..], tol));
    left
}

/// Complete the active creation tool (Enter).
pub fn finish_creation(app: &mut IncaApp) {
    if app.tool == Tool::Joint {
        // finish the chain: orient it, then the next click starts a new one
        let root = app.s.lead().filter(|l| app.s.node(l).is_some_and(|n| n.kind == "joint")).map(|l| {
            let mut r = l.clone();
            while let Some(p) = app.s.node(&r).and_then(|n| n.parent.clone()).filter(|p| app.s.node(p).is_some_and(|n| n.kind == "joint")) {
                r = p;
            }
            r
        });
        if let Some(r) = root {
            let _ = app.run_mel("orient joints", &format!("joint -e -oj xyz -sao yup -ch {};", app.s.name_of(&r)));
        }
        app.select_names(&[], SelMode::Replace);
        return;
    }
    let pts = std::mem::take(&mut app.vp_state.tool_pts);
    let f = |p: &DVec3| format!("-p {} {} {}", inca_core::math::fmt_num(p.x), inca_core::math::fmt_num(p.y), inca_core::math::fmt_num(p.z));
    let flags = |pts: &[DVec3]| pts.iter().map(f).collect::<Vec<_>>().join(" ");
    let mel = match app.tool {
        Tool::CvCurve if pts.len() >= 2 => format!("curve -d {} {};", 3.min(pts.len() - 1), flags(&pts)),
        Tool::EpCurve if pts.len() >= 2 => format!("curve -d 3 {};", pts.iter().map(|p| f(p).replacen("-p", "-ep", 1)).collect::<Vec<_>>().join(" ")),
        Tool::Arc if pts.len() == 3 => format!("curve -arc {};", flags(&pts)),
        Tool::Distance if pts.len() == 2 => format!("distanceDimension {} {};", f(&pts[0]).replacen("-p", "-sp", 1), f(&pts[1]).replacen("-p", "-ep", 1)),
        Tool::Polygon if pts.len() >= 3 => {
            // face the camera that drew it
            let cam = viewport::cam_view(&app.s, &app.vps[app.active_vp].camera.clone(), [1.0, 1.0]);
            let n = inca_core::mesh::newell(&pts);
            let mut p = pts.clone();
            if n.dot(-cam.fwd) < 0.0 {
                p.reverse();
            }
            format!("polyCreateFacet {};", flags(&p))
        }
        _ => return,
    };
    let _ = app.run_mel("create", &mel);
}

/// Nearest vertex of a visible mesh (not one being moved) within 40 px of the cursor.
/// Nearest point on a visible curve (not one being moved) within 40 px of the cursor.
fn snap_curve_point(s: &inca_core::scene::Scene, vp: &viewport::Viewport, cv: &CamView, cursor: Pos2, skip: &HashSet<String>) -> Option<DVec3> {
    let mut best: Option<(f32, DVec3)> = None;
    for id in s.all_dag(false) {
        if skip.contains(&id) || !s.is_visible(&id) {
            continue;
        }
        let Some(inca_core::scene::Shape::Curve(c)) = s.node(&id).map(|n| &n.shape) else { continue };
        let wm = s.world_matrix(&id);
        let pts: Vec<DVec3> = c.sample(16).into_iter().map(|p| wm.transform_point3(p)).collect();
        for w in pts.windows(2) {
            let (Some(a), Some(b)) = (cv.project(w[0], vp.rect), cv.project(w[1], vp.rect)) else { continue };
            let ab = b - a;
            let t = ((cursor - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
            let d = (a + ab * t).distance(cursor);
            if d < 40.0 && best.as_ref().is_none_or(|x| d < x.0) {
                best = Some((d, w[0].lerp(w[1], t as f64)));
            }
        }
    }
    best.map(|b| b.1)
}

fn snap_vertex(s: &inca_core::scene::Scene, vp: &viewport::Viewport, cv: &CamView, cursor: Pos2, skip: &HashSet<String>) -> Option<DVec3> {
    let mut best: Option<(f32, DVec3)> = None;
    for id in s.all_dag(false) {
        if skip.contains(&id) || !s.is_visible(&id) {
            continue;
        }
        let Some(pm) = s.node(&id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { continue };
        let wm = s.world_matrix(&id);
        for p in &pm.v {
            let w = wm.transform_point3(*p);
            if let Some(q) = cv.project(w, vp.rect) {
                let d = q.distance(cursor);
                if d < 40.0 && best.is_none_or(|b| d < b.0) {
                    best = Some((d, w));
                }
            }
        }
    }
    best.map(|b| b.1)
}

fn finish_drag(app: &mut IncaApp, d: GizmoDrag) {
    let delta = tf_matrix(&d.cur) * tf_matrix(&d.start).inverse();
    let (s, r, t) = delta.to_scale_rotation_translation();
    let names: Vec<String> = d.objects.iter().map(|o| app.s.name_of(&o.0)).collect();
    let f = inca_core::math::fmt_num;
    // components: commit as a polyTweak history node (curves have no shape history: the
    // JS skips them in `endDrag`, their CVs are the curve)
    for (id, verts, orig) in &d.comps {
        let DragGeom::Mesh(orig) = orig else { continue };
        let cur = app.s.node(id).unwrap().mesh().unwrap().mesh.clone();
        let deltas: Vec<[f64; 3]> = verts.iter().map(|&v| (cur.v[v as usize] - orig.v[v as usize]).to_array()).collect();
        if let Some(m) = app.s.nodes.get_mut(id).and_then(|n| n.mesh_mut()) {
            m.mesh = orig.clone();
        }
        let mut p = inca_core::params::Params::new();
        p.insert("verts".into(), serde_json::json!(verts));
        p.insert("deltas".into(), serde_json::json!(deltas));
        app.s.apply_op(id, "polyTweak", p, true);
    }
    if d.pivots.is_some() {
        for (id, _) in &d.objects {
            let p = app.s.world_pivot(id);
            let n = app.s.name_of(id);
            app.echo(format!("xform -ws -piv {} {} {} {n};", f(p.x), f(p.y), f(p.z)));
        }
        return;
    }
    let echo = match d.tool {
        Tool::Move => format!("move -r {} {} {}{};", f(t.x), f(t.y), f(t.z), if names.is_empty() { String::new() } else { format!(" {}", names.join(" ")) }),
        Tool::Rotate => {
            let e = inca_core::math::euler_deg_from_quat(r, "xyz");
            format!("rotate -r -ws {} {} {};", f(e[0]), f(e[1]), f(e[2]))
        }
        _ => format!("scale -r {} {} {};", f(s.x), f(s.y), f(s.z)),
    };
    app.echo(echo);
    if app.s.time.auto_key {
        for (id, _) in &d.objects {
            let attrs: Vec<String> = inca_core::scene::CHANNELS.iter().filter(|c| app.s.anim.is_keyed(id, c)).map(|c| c.to_string()).collect();
            for a in attrs {
                app.s.set_key(id, &a, None, None);
            }
        }
    }
}

fn hud(app: &mut IncaApp, idx: usize, cv: &CamView, p: &egui::Painter) {
    let r = app.vps[idx].rect;
    let camera = app.vps[idx].camera.clone();
    let isolate = app.vps[idx].isolate.is_some();
    let hud_on = app.vps[idx].hud;
    let h = app.hud;
    // axis triad
    if h.view_axis {
        let o = pos2(r.min.x + 32.0, r.max.y - 32.0);
        for (ax, c, l) in [(DVec3::X, Color32::from_rgb(224, 57, 62), "x"), (DVec3::Y, Color32::from_rgb(92, 214, 92), "y"), (DVec3::Z, Color32::from_rgb(74, 132, 255), "z")] {
            let v = cv.view.transform_vector3(ax);
            let e = o + vec2(v.x as f32, -v.y as f32) * 20.0;
            p.line_segment([o, e], Stroke::new(2.0, c));
            p.text(e, egui::Align2::CENTER_CENTER, l, egui::FontId::proportional(11.0), c);
        }
    }
    if !hud_on {
        return;
    }
    let text = Color32::from_gray(0xe8);
    if h.cam_names {
        p.text(pos2(r.center().x, r.max.y - 12.0), egui::Align2::CENTER_CENTER, &camera, egui::FontId::proportional(11.0), Color32::from_gray(0xe6));
    }
    // poly count: totals, the selected objects, selected components (JS drawHud)
    if h.poly_count {
        let (tot, sel) = app.hud_poly_stats();
        let hil = app.s.hilite();
        let mut comp = [0usize; 3];
        for id in &hil {
            let cs = app.s.comp_sel(id);
            comp[0] += cs.v.len();
            comp[1] += cs.e.len();
            comp[2] += cs.f.len();
        }
        let rows: String = ["Verts:", "Edges:", "Faces:", "Tris:", "UVs:"]
            .iter()
            .enumerate()
            .map(|(k, l)| format!("{l:<7}{:>9}{:>9}{:>9}\n", tot[k], sel[k], comp.get(k).copied().unwrap_or(0)))
            .collect();
        p.text(pos2(r.min.x + 8.0, r.min.y + 6.0), egui::Align2::LEFT_TOP, rows.trim_end(), egui::FontId::monospace(11.0), text);
    }
    let mut right = String::new();
    if h.frame {
        right += &format!("Current Frame: {}\n", inca_core::math::fmt_num(app.s.time.current));
    }
    if h.fps {
        right += &format!("{:.1} fps\n", app.fps_measured);
    }
    let mut y = r.min.y + 6.0;
    if !right.is_empty() {
        let g = p.text(pos2(r.max.x - 8.0, y), egui::Align2::RIGHT_TOP, right.trim_end(), egui::FontId::monospace(11.0), text);
        y = g.max.y + 4.0;
    }
    let mut tag = |t: &str, c: Color32| {
        let g = p.text(pos2(r.max.x - 8.0, y), egui::Align2::RIGHT_TOP, t, egui::FontId::proportional(12.0), c);
        y = g.max.y + 2.0;
    };
    if let Some(m) = app.s.comp_mode {
        let m = match m {
            CompMode::Vertex => "Vertex",
            CompMode::Edge => "Edge",
            CompMode::Face => "Face",
        };
        let m = if app.uv_mode { "UV" } else { m };
        tag(&format!("{m} mode"), Color32::from_rgb(255, 200, 120));
    }
    if isolate {
        tag("Isolate Select", Color32::from_rgb(255, 230, 120));
    }
    if app.pivot_mode {
        tag("Edit Pivot", Color32::from_rgb(95, 211, 255));
    }
}

/// In-View Editor: the channels of the history node an op just made, in the viewport (JS showHistoryEditor).
fn inview_editor(app: &mut IncaApp, ui: &mut egui::Ui, rect: Rect) {
    let Some((id, i)) = app.inview.clone() else { return };
    let valid = app.s.node(&id).and_then(|n| n.mesh()).and_then(|m| m.history.get(i)).map(|h| h.name.clone());
    let lead_ok = app.s.sel.contains(&id) || app.s.hilite().contains(&id);
    let Some(name) = valid.filter(|_| lead_ok) else {
        app.inview = None;
        return;
    };
    // dragged by its title bar; kept inside the viewport
    let off = app.inview_offset;
    let min = rect.min + vec2(8.0, 96.0) + off;
    let min = pos2(min.x.clamp(rect.min.x, (rect.max.x - 250.0).max(rect.min.x)), min.y.clamp(rect.min.y, (rect.max.y - 40.0).max(rect.min.y)));
    let r = Rect::from_min_size(min, vec2(250.0, 10.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(r.min, vec2(250.0, rect.height() - 110.0))));
    let mut close = false;
    egui::Frame::new().fill(Color32::from_rgba_unmultiplied(55, 55, 55, 235)).stroke(Stroke::new(1.0, Color32::from_gray(0x22))).corner_radius(3).inner_margin(egui::Margin::same(6)).show(&mut child, |ui| {
        ui.set_width(238.0);
        ui.horizontal(|ui| {
            let title = ui.add(egui::Label::new(RichText::new(&name).strong()).sense(Sense::drag())).on_hover_cursor(egui::CursorIcon::Grab).on_hover_text("Drag to move the In-View Editor");
            if title.dragged() {
                app.inview_offset += title.drag_delta();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui.small_button("✕").on_hover_text("Close the In-View Editor").clicked();
            });
        });
        crate::panels::history_channels(app, ui, &id, i);
    });
    if close || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.inview = None;
    }
}

/// After +/-: the manipulator size with a bar and a Reset button (JS panel-manipsize).
fn manip_size_readout(app: &mut IncaApp, ui: &mut egui::Ui, rect: Rect) {
    if app.manip_size_shown <= 0.0 {
        return;
    }
    app.manip_size_shown -= ui.input(|i| i.unstable_dt).min(0.1);
    ui.ctx().request_repaint();
    let r = Rect::from_min_size(pos2(rect.max.x - 230.0, rect.max.y - 34.0), vec2(222.0, 26.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
    egui::Frame::new().fill(Color32::from_rgba_unmultiplied(40, 40, 40, 225)).stroke(Stroke::new(1.0, Color32::from_gray(0x1d))).corner_radius(3).inner_margin(egui::Margin::symmetric(8, 3)).show(&mut child, |ui| {
        ui.label(RichText::new("Manipulator size").size(11.0));
        let (bar, _) = ui.allocate_exact_size(vec2(60.0, 6.0), Sense::hover());
        ui.painter().rect_filled(bar, 2.0, Color32::from_gray(0x55));
        let f = ((app.manip_size - 0.2) / 3.8).clamp(0.0, 1.0);
        ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height())), 2.0, Color32::from_rgb(0x5d, 0x9b, 0xd5));
        ui.label(RichText::new(format!("{:.2}", app.manip_size)).size(11.0));
        if ui.small_button("Reset").clicked() {
            app.manip_size = 1.0;
            app.manip_size_shown = 2.5;
        }
    });
}

/// Display > Polygons > Face Normals / Border Edges overlays.
fn mesh_overlays(app: &IncaApp, cv: &CamView, rect: Rect, p: &egui::Painter) {
    let line = |a: DVec3, b: DVec3, c: Color32| {
        if let (Some(qa), Some(qb)) = (cv.project(a, rect), cv.project(b, rect)) {
            p.line_segment([qa, qb], Stroke::new(1.0, c));
        }
    };
    for id in &app.overlay_normals {
        let Some(pm) = app.s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { continue };
        let wm = app.s.world_matrix(id);
        let b = pm.bbox();
        let len = (b.size().length() * 0.05).max(0.05);
        for f in 0..pm.f.len() {
            let c = pm.face_center(f);
            line(wm.transform_point3(c), wm.transform_point3(c + pm.face_normal(f) * len), Color32::from_rgb(0xff, 0xff, 0x40));
        }
    }
    for id in &app.overlay_border {
        let Some(pm) = app.s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { continue };
        let wm = app.s.world_matrix(id);
        let t = pm.topo();
        for (e, &(a, b)) in t.edges.iter().enumerate() {
            if t.edge_faces[e].len() == 1 {
                line(wm.transform_point3(pm.v[a as usize]), wm.transform_point3(pm.v[b as usize]), Color32::from_rgb(0xff, 0x70, 0x30));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;
    use inca_core::params::Params;
    use inca_core::scene::Scene;

    #[test]
    fn active_handles_are_remembered_per_mode() {
        let mut vp = VpState::default();
        vp.active_handles[mode_index(Tool::Move)] = Some(GizmoMode::TranslateX);
        vp.active_handles[mode_index(Tool::Rotate)] = Some(GizmoMode::RotateZ);
        vp.active_handles[mode_index(Tool::Scale)] = Some(GizmoMode::ScaleUniform);
        // the JS `Manip.activeAxis` reset only clears the current mode
        vp.reset_active_handle(Tool::Rotate);
        assert_eq!(vp.active_handle(Tool::Rotate), None);
        assert_eq!(vp.active_handle(Tool::Move), Some(GizmoMode::TranslateX));
        assert_eq!(vp.active_handle(Tool::Scale), Some(GizmoMode::ScaleUniform));
        assert!(is_center_handle(GizmoMode::TranslateView));
        assert!(is_center_handle(GizmoMode::RotateView));
        assert!(is_center_handle(GizmoMode::ScaleUniform));
        assert!(!is_center_handle(GizmoMode::TranslateX));
    }

    #[test]
    fn ctrl_axis_locks_to_the_other_plane() {
        // transform-gizmo names its plane handles after the corner they sit at: XY is the handle
        // between +Y/+Z (normal X, locks X), YZ is the one between +X/+Y (normal Z, locks Z).
        assert_eq!(plane_of(GizmoMode::TranslateX), GizmoMode::TranslateXY);
        assert_eq!(plane_of(GizmoMode::TranslateY), GizmoMode::TranslateXZ);
        assert_eq!(plane_of(GizmoMode::TranslateZ), GizmoMode::TranslateYZ);
        assert_eq!(plane_of(GizmoMode::ScaleX), GizmoMode::ScaleXY);
        assert_eq!(plane_of(GizmoMode::ScaleY), GizmoMode::ScaleXZ);
        assert_eq!(plane_of(GizmoMode::ScaleZ), GizmoMode::ScaleYZ);
        // planes stay planes, and the centre / rotate axes are untouched
        assert_eq!(plane_of(GizmoMode::TranslateXY), GizmoMode::TranslateXY);
        assert_eq!(plane_of(GizmoMode::TranslateXZ), GizmoMode::TranslateXZ);
        assert_eq!(plane_of(GizmoMode::TranslateYZ), GizmoMode::TranslateYZ);
        assert_eq!(plane_of(GizmoMode::TranslateView), GizmoMode::TranslateView);
        assert_eq!(plane_of(GizmoMode::RotateX), GizmoMode::RotateX);
        assert!(is_center_handle(GizmoMode::TranslateView));
        assert!(!is_center_handle(GizmoMode::TranslateX));
    }

    /// Force a plane mode (as a Ctrl+click does) and drag from the pivot; returns the translation.
    fn plane_drag(mode: GizmoMode) -> [f64; 3] {
        let (cv, rect) = {
            let mut s = Scene::new();
            let id = s.create_mesh("polyCube", &Params::new(), None);
            let mut vp = viewport::Viewport::new("persp");
            vp.rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
            let b = s.world_bbox(&id);
            viewport::frame_box(&mut s, "persp", &b, 800.0 / 600.0);
            (viewport::cam_view(&s, "persp", [800.0, 600.0]), vp.rect)
        };
        let q = cv.project(DVec3::ZERO, rect).unwrap();
        let tf = Transform::from_scale_rotation_translation(DVec3::ONE, DQuat::IDENTITY, DVec3::ZERO);
        let mut g = Gizmo::default();
        g.update_config(GizmoConfig {
            view_matrix: cv.view.into(),
            projection_matrix: cv.proj.into(),
            viewport: rect,
            modes: GizmoMode::all_translate(),
            mode_override: Some(mode),
            orientation: GizmoOrientation::Global,
            ..Default::default()
        });
        assert!(g.update(GizmoInteraction { cursor_pos: (q.x, q.y), hovered: true, drag_started: true, dragging: true }, std::slice::from_ref(&tf)).is_some());
        let (_, out) = g.update(GizmoInteraction { cursor_pos: (q.x + 40.0, q.y + 12.0), hovered: true, drag_started: false, dragging: true }, std::slice::from_ref(&tf)).unwrap();
        let t = out[0].translation;
        [t.x, t.y, t.z]
    }

    #[test]
    fn forced_plane_override_drags_the_plane_not_the_handle_under_the_cursor() {
        // Ctrl on X locks X: the forced drag must keep X fixed even though the press is elsewhere
        for (mode, keep) in [
            (plane_of(GizmoMode::TranslateX), 0),
            (plane_of(GizmoMode::TranslateY), 1),
            (plane_of(GizmoMode::TranslateZ), 2),
        ] {
            let t = plane_drag(mode);
            assert!(t[keep].abs() < 1e-9, "{mode:?} must keep axis {keep} fixed: {t:?}");
            assert!(t.iter().enumerate().any(|(i, v)| i != keep && v.abs() > 1e-6), "{mode:?} did not move: {t:?}");
        }
    }

    #[test]
    fn auto_axis_follows_the_drag_direction() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let mut vp = viewport::Viewport::new("persp");
        vp.rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let b = s.world_bbox(&id);
        viewport::frame_box(&mut s, "persp", &b, 800.0 / 600.0);
        let cv = viewport::cam_view(&s, "persp", [800.0, 600.0]);
        let pivot = DVec3::ZERO;
        let origin = cv.project(pivot, vp.rect).unwrap();
        for (axis, mode) in [(DVec3::X, GizmoMode::TranslateX), (DVec3::Y, GizmoMode::TranslateY), (DVec3::Z, GizmoMode::TranslateZ)] {
            let tip = cv.project(pivot + axis, vp.rect).unwrap();
            let drag = (tip - origin) * 4.0;
            let got = auto_axis(Tool::Move, pivot, DQuat::IDENTITY, &cv, vp.rect, origin, origin + drag);
            assert_eq!(got, mode, "drag along {axis:?}");
        }
        // the same drag with the scale tool picks the scale axis
        let tip = cv.project(pivot + DVec3::Y, vp.rect).unwrap();
        let drag = (tip - origin) * 4.0;
        assert_eq!(auto_axis(Tool::Scale, pivot, DQuat::IDENTITY, &cv, vp.rect, origin, origin + drag), GizmoMode::ScaleY);
    }
}
