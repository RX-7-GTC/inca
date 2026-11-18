//! Viewport panes: layout, panel menus, mouse interaction (navigation, selection, manipulators), HUD.

use crate::app::{IncaApp, Layout, Tool};
use crate::theme;
use crate::viewport::{self, CamView, Comp, LightMode, ShadeMode};
use egui::{pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke};
use glam::{DMat4, DQuat, DVec3};
use inca_core::math::quat_from_euler_deg;
use inca_core::mesh::epair;
use inca_core::scene::{CompMode, SelMode, Shape};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use transform_gizmo_egui::math::Transform;
use transform_gizmo_egui::prelude::*;

/// Base on-screen manipulator size at settings scale 1.0. Maya's default reads a little larger
/// than transform-gizmo's 75 px, while the user-facing size value stays 1.0.
const MANIP_SIZE: f32 = 115.0;
/// Hover colour for manipulator handles: a pale yellow that stays distinct from the selected one.
const MANIP_HOVER: Color32 = Color32::from_rgb(255, 245, 170);
/// Colour of the remembered / active handle.
const MANIP_SELECTED: Color32 = Color32::from_rgb(240, 225, 80);

/// Camera navigation mode of an Alt+drag in progress (JS `onDown`).
#[derive(Clone, Copy, PartialEq)]
pub enum NavMode {
    Tumble,
    Track,
    Dolly,
}

#[derive(Clone, Copy, PartialEq)]
enum SelectCursor {
    Neutral,
    Add,
    Remove,
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
    pub snap_scale: f64,
    /// Discrete move / rotate / scale, as JS `Manip.discrete` (separate from the grid toggle)
    pub discrete: [bool; 3],
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
    /// Inputs of the last rendered 3D image per viewport, so UI-only repaints can reuse it.
    pub render_keys: [Option<u64>; 4],
    /// Transform used to draw the manipulator last frame. The viewport image is rendered before
    /// interaction, so during a drag the object on screen is one frame behind the current drag
    /// result; drawing the gizmo from the previous result keeps it attached to the rendered object.
    pub gizmo_draw_tf: [Option<Transform>; 4],
    /// While an MMB / Shift+MMB drag is active, only this handle is drawn.
    pub gizmo_solo: [Option<GizmoMode>; 4],
    /// Tab+drag select in progress: the components it has already toggled, and the last point
    pub tab_drag: Option<(HashSet<String>, Option<Pos2>)>,
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
            snap_scale: 1.0,
            discrete: [false; 3],
            tool_pts: vec![],
            active_handles: [None; 3],
            mmb: None,
            force_handle: None,
            auto_drag: None,
            press_handle: None,
            arc_drag: None,
            arc_anchor: None,
            render_keys: [None; 4],
            gizmo_draw_tf: [None; 4],
            gizmo_solo: [None; 4],
            tab_drag: None,
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
    /// object local scales at drag start, for signed scale edits without matrix decomposition
    pub local_scales: Vec<DVec3>,
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
    /// press position of a JS-style drag (MMB / Shift auto-axis), where rotate and scale use
    /// `middleDrag`'s dx maths rather than the TransformControls handle geometry
    pub js_drag: Option<Pos2>,
    /// where the drag was pressed; the JS grid snap slides the start point's screen position by
    /// the mouse delta from here (`_dragPx0`), preserving the grab offset on the centre handle
    pub press_px: Pos2,
    /// Forced-handle (MMB / Shift auto-axis) drags: transform-gizmo clamps the picked axis start
    /// point to the arrow, then measures from the infinite axis line. The first result therefore
    /// contains a jump from the object to the cursor; subtracting this one-shot bias keeps the
    /// object at its real position. Later results are already incremental.
    pub rebase: Option<DVec3>,
}

impl VpState {
    pub fn live_meshes(&self) -> HashSet<String> {
        self.drag.as_ref().map(|d| d.comps.iter().map(|c| c.0.clone()).collect()).unwrap_or_default()
    }
}

fn tf_matrix(t: &Transform) -> DMat4 {
    DMat4::from_scale_rotation_translation(DVec3::from(t.scale), DQuat::from(t.rotation), DVec3::from(t.translation))
}

/// Everything the 3D pass depends on. If it is unchanged, a repaint caused by menus,
/// toolbar hover, panels etc. can reuse the previous viewport texture.
fn vp_render_key(app: &IncaApp, idx: usize, px: [u32; 2], cv: &CamView) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.s.rev.hash(&mut h);
    app.s.time.current.to_bits().hash(&mut h);
    app.s.project.hash(&mut h);
    px.hash(&mut h);
    app.bg_mode.hash(&mut h);
    app.prefs.smooth_divisions.hash(&mut h);
    format!("{:?}", app.s.comp_mode).hash(&mut h);
    app.s.sel.hash(&mut h);
    app.s.hilite().hash(&mut h);
    app.s.lead().hash(&mut h);
    format!("{:?}", app.vps[idx]).hash(&mut h);
    app.soft.on.hash(&mut h);
    app.soft.radius.to_bits().hash(&mut h);
    app.soft.linear.hash(&mut h);
    let mut live: Vec<String> = app.vp_state.live_meshes().into_iter().collect();
    live.sort();
    live.hash(&mut h);
    for x in cv.view_proj().to_cols_array() {
        x.to_bits().hash(&mut h);
    }
    h.finish()
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
    // Maya's active panel follows the mouse in a multi-pane layout, so Space maximizes the
    // viewport under the cursor instead of the last clicked one.
    if app.layout != Layout::Single && !ui.ctx().input(|i| i.pointer.any_down()) && app.marking.is_none() {
        if let Some(p) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            if let Some(&(idx, _)) = panes.iter().find(|(_, r)| r.contains(p)) {
                app.active_vp = idx;
            }
        }
    }
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
    let render_key = vp_render_key(app, idx, px, &cv);
    let force_render = !app.viz.ghosts.is_empty() || !app.viz.trails.is_empty();
    // ---- render
    if let (Some(rs), Some(gpu)) = (frame.wgpu_render_state(), app.gpu.as_mut()) {
        let tex = {
            let mut r = rs.renderer.write();
            gpu.target(&mut r, idx, px)
        };
        if force_render || app.vp_state.render_keys[idx] != Some(render_key) {
            let live = app.vp_state.live_meshes();
            let soft = app.soft.on.then_some((app.soft.radius, app.soft.linear));
            let mut draws = viewport::build_draws(&app.s, &app.vps[idx], &cv, gpu, &mut app.smooth_cache, &live, &mut app.mesh_cache, soft, app.prefs.smooth_divisions);
            draws.extend(crate::anim_extras::draws(&app.viz));
            let mut fu = viewport::frame_uniform(&app.s, &app.vps[idx], &cv, app.bg_mode);
            let mut sky_warning = None;
            if fu.sky[0] > 1.5 {
                match viewport::sky_texture_file(&app.s, &app.vps[idx]) {
                    Some(file) => match gpu.prepare_sky(app.s.project.as_deref(), &file) {
                        viewport::gpu::SkyLoad::Ready => {}
                        viewport::gpu::SkyLoad::Failed(e) => {
                            fu.sky[0] = 1.0;
                            sky_warning = Some(format!("Sky dome texture could not be loaded: {file} ({e})"));
                        }
                        viewport::gpu::SkyLoad::Unavailable => fu.sky[0] = 1.0,
                    },
                    None => fu.sky[0] = 1.0,
                }
            }
            gpu.render(idx, &fu, &draws);
            if let Some(w) = sky_warning {
                app.warn(w);
            }
            app.vp_state.render_keys[idx] = Some(render_key);
        }
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
    let hover_selected = hover_comp.as_ref().is_some_and(|(id, c)| comp_is_selected(app, id, c));
    viewport::draw_component_overlay(&app.s, &app.vps[idx], &cv, &painter, hover_comp.as_ref().map(|(id, c)| (id.as_str(), c, hover_selected)));
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
    use crate::menus::{activated, menu_check, menu_item, menu_radio, menu_selectable, panel_bar, MenuSource};
    ui.spacing_mut().button_padding = vec2(4.0, 0.0);
    let cam = app.vps[idx].camera.clone();
    let titles = ["View", "Shading", "Lighting", "Show", "Renderer", "Panels"];
    panel_bar(app, ui, MenuSource::Panel(idx), &titles, true, |app, ui, title| match title {
        "View" => {
        if menu_item(app, ui, "Select Camera") {
            select_camera(app, idx);
        }
        let r = ui.add_enabled(app.can_undo_view(idx), egui::Button::new("Undo View Change").shortcut_text("Alt+Z"));
        if activated(app, ui, &r) {
            app.active_vp = idx;
            app.undo_view(idx);
        }
        let r = ui.add_enabled(app.can_redo_view(idx), egui::Button::new("Redo View Change").shortcut_text("Alt+Y"));
        if activated(app, ui, &r) {
            app.active_vp = idx;
            app.redo_view(idx);
        }
        let r = ui.add(egui::Button::new("Default View").shortcut_text("Alt+Home"));
        if activated(app, ui, &r) {
            app.active_vp = idx;
            app.push_view_change(idx);
            viewport::default_view(&mut app.s, &cam);
        }
        if menu_item(app, ui, "Look at Selection") {
            // JS `lookAtSelection` unions the selected objects' bounding boxes, not components
            let b = viewport::selected_objects_bbox(&app.s);
            if !b.is_empty() {
                app.active_vp = idx;
                app.push_view_change(idx);
                viewport::look_at(&mut app.s, &cam, b.center());
            }
        }
        let r = ui.add(egui::Button::new("Frame All").shortcut_text("A"));
        if activated(app, ui, &r) {
            app.active_vp = idx;
            app.action("frameAll");
        }
        let r = ui.add(egui::Button::new("Frame Selection").shortcut_text("F"));
        if activated(app, ui, &r) {
            app.active_vp = idx;
            app.action("frameSelected");
        }
        ui.separator();
        ui.menu_button("Camera Settings", |ui| {
            if menu_check(app, ui, app.vps[idx].res_gate, "Resolution Gate") {
                app.vps[idx].res_gate = !app.vps[idx].res_gate;
                app.vps[idx].film_gate = false;
            }
            if menu_check(app, ui, app.vps[idx].film_gate, "Film Gate") {
                app.vps[idx].film_gate = !app.vps[idx].film_gate;
                app.vps[idx].res_gate = false;
            }
            if menu_check(app, ui, app.prefs.ortho_tumble, "Orthographic Tumble") {
                app.prefs.ortho_tumble = !app.prefs.ortho_tumble;
            }
        });
        if menu_item(app, ui, "Camera Attribute Editor...") {
            select_camera(app, idx);
            app.action("attributeEditor");
        }
        ui.separator();
        ui.menu_button("Predefined Bookmarks", |ui| {
            for (l, t, r) in BOOKMARKS {
                if menu_item(app, ui, l) {
                    let ortho = viewport::camera_id(&app.s, &cam).and_then(|id| app.s.node(&id)).is_some_and(|n| matches!(&n.shape, Shape::Camera(c) if c.ortho));
                    if !ortho {
                        viewport::set_view(&mut app.s, &cam, *t, *r, Some((t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt()));
                    }
                }
            }
        });
        }
        "Shading" => {
        let mut backface = app.backface;
        if menu_radio(app, ui, app.vps[idx].shade == ShadeMode::Wire, "Wireframe  4") {
            app.vps[idx].shade = ShadeMode::Wire;
            if app.vps[idx].lights == LightMode::All {
                app.vps[idx].lights = LightMode::Default;
            }
        }
        if menu_radio(app, ui, app.vps[idx].shade == ShadeMode::Shaded, "Smooth Shade All  5") {
            app.vps[idx].shade = ShadeMode::Shaded;
            if app.vps[idx].lights == LightMode::All {
                app.vps[idx].lights = LightMode::Default;
            }
        }
        // JS "Bounding Box" also switches to wireframe in this release
        if menu_selectable(app, ui, false, "Bounding Box") {
            app.vps[idx].shade = ShadeMode::Wire;
            if app.vps[idx].lights == LightMode::All {
                app.vps[idx].lights = LightMode::Default;
            }
        }
        ui.separator();
        if menu_check(app, ui, app.vps[idx].wire_on_shaded, "Wireframe on Shaded") {
            app.vps[idx].wire_on_shaded = !app.vps[idx].wire_on_shaded;
        }
        if menu_check(app, ui, app.vps[idx].xray, "X-Ray  Alt+A") {
            app.vps[idx].xray = !app.vps[idx].xray;
        }
        if menu_check(app, ui, backface, "Backface Culling") {
            backface = !backface;
        }
        ui.separator();
        let tex = app.vps[idx].shade == ShadeMode::Textured;
        if menu_check(app, ui, tex, "Hardware Texturing  6") {
            app.vps[idx].shade = if !tex { ShadeMode::Textured } else { ShadeMode::Shaded };
            if app.vps[idx].lights == LightMode::All {
                app.vps[idx].lights = LightMode::Default;
            }
        }
        if backface != app.backface {
            app.set_backface(backface);
        }
        }
        "Lighting" => {
        if menu_radio(app, ui, app.vps[idx].lights == LightMode::Default, "Use Default Lighting") {
            app.vps[idx].lights = LightMode::Default;
        }
        if menu_radio(app, ui, app.vps[idx].lights == LightMode::All, "Use All Lights  7") {
            app.vps[idx].lights = LightMode::All;
        }
        if menu_radio(app, ui, app.vps[idx].lights == LightMode::Flat, "Use Flat Lighting") {
            app.vps[idx].lights = LightMode::Flat;
        }
        if menu_radio(app, ui, app.vps[idx].lights == LightMode::None, "Use No Lights") {
            app.vps[idx].lights = LightMode::None;
        }
        ui.separator();
        if menu_check(app, ui, app.vps[idx].shadows, "Shadows") {
            app.vps[idx].shadows = !app.vps[idx].shadows;
        }
        if app.vps[idx].shadows && app.vps[idx].lights != LightMode::All {
            ui.label(egui::RichText::new("Shadows need \"Use All Lights\" (7)").color(theme::TEXT_DIM));
        }
        }
        "Show" => {
        ui.menu_button("Isolate Select", |ui| {
            if menu_check(app, ui, app.vps[idx].isolate.is_some(), "View Selected") {
                app.active_vp = idx;
                app.action("isolateSelect");
            }
            if menu_item(app, ui, "Add Selected Objects") {
                let sel = app.s.sel.clone();
                let iso = app.vps[idx].isolate.get_or_insert_with(Vec::new);
                for id in sel {
                    if !iso.contains(&id) {
                        iso.push(id);
                    }
                }
            }
            if menu_item(app, ui, "Remove Selected Objects") {
                let sel = app.s.sel.clone();
                if let Some(iso) = app.vps[idx].isolate.as_mut() {
                    iso.retain(|i| !sel.contains(i));
                }
            }
        });
        ui.separator();
        if menu_item(app, ui, "All") {
            let v = &mut app.vps[idx];
            v.show_all(true);
            v.hud = true;
            v.show_manips = true;
        }
        if menu_item(app, ui, "None") {
            app.vps[idx].show_all(false);
        }
        ui.separator();
        if menu_check(app, ui, app.vps[idx].show_curves, "NURBS Curves") {
            app.vps[idx].show_curves = !app.vps[idx].show_curves;
        }
        if menu_check(app, ui, app.vps[idx].show_nurbs, "NURBS Surfaces") {
            app.vps[idx].show_nurbs = !app.vps[idx].show_nurbs;
        }
        if menu_check(app, ui, app.vps[idx].show_meshes, "Polygons") {
            app.vps[idx].show_meshes = !app.vps[idx].show_meshes;
        }
        if menu_check(app, ui, app.vps[idx].show_lights, "Lights") {
            app.vps[idx].show_lights = !app.vps[idx].show_lights;
        }
        if menu_check(app, ui, app.vps[idx].show_cameras, "Cameras") {
            app.vps[idx].show_cameras = !app.vps[idx].show_cameras;
        }
        if menu_check(app, ui, app.vps[idx].show_locators, "Locators") {
            app.vps[idx].show_locators = !app.vps[idx].show_locators;
        }
        ui.separator();
        if menu_check(app, ui, app.vps[idx].grid, "Grid") {
            app.vps[idx].grid = !app.vps[idx].grid;
        }
        if menu_check(app, ui, app.vps[idx].show_manips, "Manipulators") {
            app.vps[idx].show_manips = !app.vps[idx].show_manips;
        }
        if menu_check(app, ui, app.vps[idx].show_sel_highlight, "Selection Highlighting") {
            app.vps[idx].show_sel_highlight = !app.vps[idx].show_sel_highlight;
        }
        if menu_check(app, ui, app.vps[idx].hud, "HUD") {
            app.vps[idx].hud = !app.vps[idx].hud;
        }
        }
        "Renderer" => {
        let _ = menu_radio(app, ui, true, "Viewport 2.0 (Inca wgpu)");
        if menu_item(app, ui, "   Inca Path Tracer (Render View)") {
            app.action("renderView");
        }
        }
        "Panels" => {
        let cams: Vec<(String, bool)> = app.s.all_dag(true).into_iter().filter_map(|id| app.s.node(&id)).filter_map(|n| match &n.shape {
            Shape::Camera(c) => Some((n.name.clone(), c.ortho)),
            _ => None,
        }).collect();
        ui.menu_button("Perspective", |ui| {
            for (c, _) in cams.iter().filter(|c| !c.1) {
                if menu_check(app, ui, app.vps[idx].camera == *c, c) {
                    app.push_view_change(idx);
                    app.vps[idx].camera = c.clone();
                }
            }
            ui.separator();
            if menu_item(app, ui, "New") {
                let before: HashSet<String> = app.s.nodes.keys().cloned().collect();
                app.action("createCamera");
                if let Some(name) = app.s.nodes.values().find(|n| !before.contains(&n.id) && matches!(n.shape, Shape::Camera(_))).map(|n| n.name.clone()) {
                    app.push_view_change(idx);
                    app.vps[idx].camera = name;
                }
            }
        });
        ui.menu_button("Orthographic", |ui| {
            for (c, _) in cams.iter().filter(|c| c.1) {
                if menu_check(app, ui, app.vps[idx].camera == *c, c) {
                    app.push_view_change(idx);
                    app.vps[idx].camera = c.clone();
                }
            }
        });
        if menu_item(app, ui, "Look Through Selected") {
            match app.s.sel.iter().find(|i| matches!(app.s.node(i).map(|n| &n.shape), Some(Shape::Camera(_)))).cloned() {
                Some(c) => {
                    app.push_view_change(idx);
                    app.vps[idx].camera = app.s.name_of(&c);
                }
                None => app.last_result = "Warning: Select a camera".into(),
            }
        }
        ui.separator();
        ui.menu_button("Panel", |ui| {
            for (l, a) in [("Outliner", "outlinerWindow"), ("Graph Editor", "graphEditor"), ("Dope Sheet", "dopeSheet"), ("Hypershade", "hypershade"), ("Hypergraph Hierarchy", "hypergraph"), ("Render View", "renderView")] {
                if menu_item(app, ui, l) {
                    app.action(a);
                }
            }
        });
        ui.menu_button("Layouts", |ui| {
            for (l, lay) in [("Single Pane", Layout::Single), ("Two Panes Side by Side", Layout::TwoSide), ("Two Panes Stacked", Layout::TwoStack), ("Three Panes Split Left", Layout::Three), ("Four Panes", Layout::Four)] {
                if menu_check(app, ui, app.layout == lay, l) {
                    app.active_vp = idx;
                    app.layout = lay;
                    if lay != Layout::Single {
                        app.layout_before = lay;
                    }
                }
            }
        });
        }
        _ => {}
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
        app.active_vp = idx;
        app.push_view_change(idx);
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
        if v.lights == LightMode::All {
            v.lights = LightMode::Default;
        }
    }
    if ib(ui, "smoothShade", "Smooth shade all (5)", v.shade != ShadeMode::Wire) {
        v.shade = ShadeMode::Shaded;
        if v.lights == LightMode::All {
            v.lights = LightMode::Default;
        }
    }
    if ib(ui, "wireOnShaded", "Wireframe on shaded", v.wire_on_shaded) {
        v.wire_on_shaded = !v.wire_on_shaded;
    }
    if ib(ui, "textured", "Textured (6)", v.shade == ShadeMode::Textured) {
        v.shade = if v.shade == ShadeMode::Textured { ShadeMode::Shaded } else { ShadeMode::Textured };
        if v.lights == LightMode::All {
            v.lights = LightMode::Default;
        }
    }
    if ib(ui, "useLights", "Use all lights (7)", v.lights == LightMode::All) {
        v.lights = if v.lights == LightMode::All { LightMode::Default } else { LightMode::All };
    }
    if ib(ui, "shadows", "Shadows", v.shadows) {
        v.shadows = !v.shadows;
        // the JS toolbar also switches to Use All Lights, unlike the menu item
        if v.shadows && v.lights != LightMode::All {
            v.lights = LightMode::All;
        }
    }
    separator(ui);
    let iso = v.isolate.is_some();
    if ib(ui, "isolate", "Isolate select", iso) {
        app.active_vp = idx;
        app.action("isolateSelect");
    }
    if ib(ui, "xray", "X-Ray (Alt+A)", app.vps[idx].xray) {
        app.vps[idx].xray = !app.vps[idx].xray;
    }
    if ib(ui, "backface", "Backface culling", app.backface) {
        app.set_backface(!app.backface);
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
    // A wheel tick during an Alt+MMB pan is part of the same gesture, not a dolly.
    if resp.hovered() && scroll != 0.0 && !pointer.button_down(egui::PointerButton::Middle) {
        // JS wheel: dolly(-deltaY * 0.35); dolly uses exp(-d * 0.006)
        let amount = scroll as f64 * 0.0021;
        app.active_vp = idx;
        app.push_view_change(idx);
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
        app.active_vp = idx;
        app.push_view_change(idx);
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
            gizmo(app, ui, idx, resp, cv, painter, false);
            return;
        }
    }
    if mods.alt {
        gizmo(app, ui, idx, resp, cv, painter, false);
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
    let gizmo_used = gizmo(app, ui, idx, resp, cv, painter, true);
    if gizmo_used {
        return;
    }
    // Tab over a viewport is for drag select, not keyboard focus traversal
    if resp.hovered() && ui.input(|i| i.key_pressed(egui::Key::Tab)) {
        if let Some(f) = ui.memory(|m| m.focused()) {
            ui.memory_mut(|m| m.surrender_focus(f));
        }
    }
    // ---- Tab: a toggle-select tool. Hold Tab and hover/drag; the cursor shows + or - for the
    // item under it, and each item is toggled once without clearing the existing selection.
    let tab_down = ui.input(|i| i.key_down(egui::Key::Tab));
    if tab_down && (resp.hovered() || app.vp_state.tab_drag.is_some()) && !app.lock_selection {
        let mode = pointer
            .interact_pos()
            .filter(|p| app.vps[idx].rect.contains(*p))
            .and_then(|p| pick_name(app, idx, cv, p))
            .map(|n| if item_selected(app, &n) { SelectCursor::Remove } else { SelectCursor::Add })
            .unwrap_or(SelectCursor::Neutral);
        ui.ctx().set_cursor_image(Some(select_cursor(mode)));
    }
    if resp.drag_started_by(egui::PointerButton::Primary) && tab_down && !app.lock_selection {
        app.s.checkpoint_selection("select");
        app.vp_state.tab_drag = Some((HashSet::new(), resp.interact_pointer_pos().map(|p| p - resp.drag_delta())));
    }
    if let Some((mut seen, last)) = app.vp_state.tab_drag.clone() {
        if let Some(p) = pointer.interact_pos() {
            // everything under the path since the last frame, not just the current point
            let from = last.unwrap_or(p);
            let steps = (from.distance(p) / 4.0).ceil().max(1.0) as usize;
            let (mut add, mut remove) = (vec![], vec![]);
            for k in 0..=steps {
                if let Some(n) = pick_name(app, idx, cv, from.lerp(p, k as f32 / steps as f32)).filter(|n| !seen.contains(n)) {
                    seen.insert(n.clone());
                    if item_selected(app, &n) {
                        remove.push(n);
                    } else {
                        add.push(n);
                    }
                }
            }
            if !add.is_empty() {
                let _ = app.exec_mel(&format!("select -add {};", add.join(" ")), false);
            }
            if !remove.is_empty() {
                let _ = app.exec_mel(&format!("select -d {};", remove.join(" ")), false);
            }
            app.vp_state.tab_drag = Some((seen.clone(), Some(p)));
        }
        if resp.drag_stopped() || !pointer.primary_down() {
            if seen.is_empty() {
                app.s.undo_stack.pop();
            }
            app.vp_state.tab_drag = None;
        }
        return;
    }
    if tab_down && resp.clicked_by(egui::PointerButton::Primary) && !app.lock_selection {
        // a Tab click toggles the item under the pointer, keeping the rest of the selection
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(n) = pick_name(app, idx, cv, p) {
                app.s.checkpoint_selection("select");
                let verb = if item_selected(app, &n) { "-d" } else { "-add" };
                let _ = app.exec_mel(&format!("select {verb} {n};"), false);
            }
        }
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
                        crate::theme::marquee(&painter, Rect::from_two_pos(start, cur));
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
    // double-click: edge/face loops in component mode, Tool Settings on an object (JS onDbl).
    // Shift+double-click a face beside a selected one takes the face loop through both and
    // skips the normal edge-ring behaviour.
    if resp.double_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let mut handled = false;
            if mods.shift && app.s.comp_mode == Some(inca_core::scene::CompMode::Face) && !app.lock_selection {
                if let Some((id, f)) = pick_face(app, idx, cv, p) {
                    app.s.checkpoint_selection("select");
                    if inca_core::selection::add_face_loop(&mut app.s, &id, f) {
                        let faces: Vec<String> = app.s.comp_sel(&id).f.iter().map(|f| format!("{}.f[{f}]", app.s.name_of(&id))).collect();
                        app.echo(format!("select -r {};", faces.join(" ")));
                        handled = true;
                    } else {
                        app.s.undo_stack.pop();
                    }
                }
            }
            if !handled {
                double_click(app, idx, cv, p, mods.shift);
            }
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
                app.s.checkpoint_selection("select");
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
                // clicking another object in component mode hilites it (Maya behaviour) and
                // appends it to the selection, but never drops the previous selection
                if let Some(o) = pick_masked(app, &vp, cv, p) {
                    if !hil.contains(&o) {
                        app.s.checkpoint_selection("select");
                        if mode == SelMode::Replace {
                            app.s.set_hilite(std::slice::from_ref(&o));
                        } else {
                            app.s.add_hilite(&o);
                        }
                        if !app.s.sel.contains(&o) {
                            app.s.sel.push(o);
                        }
                        app.s.rev += 1;
                        return;
                    }
                }
                if mode == SelMode::Replace {
                    let any = hil.iter().any(|id| !app.s.comp_sel_active(id).is_empty());
                    if any {
                        app.s.checkpoint_selection("select");
                        for id in hil {
                            app.s.clear_active_comp(&id);
                        }
                        app.s.rev += 1;
                    }
                }
            }
        }
        return;
    }
    let hit = pick_masked(app, &vp, cv, p).and_then(|h| filter_picks(app, vec![h]).pop());
    match hit {
        Some(id) => {
            app.s.checkpoint_selection("select");
            app.select_names(&[id], mode);
        }
        None => {
            if mode == SelMode::Replace && !app.s.sel.is_empty() {
                app.s.checkpoint_selection("select");
                app.select_names(&[], SelMode::Replace);
            }
        }
    }
}

/// Object pick honouring the status-line selection mask, as the JS `visibleNodes`.
fn pick_masked(app: &mut IncaApp, vp: &viewport::Viewport, cv: &CamView, p: Pos2) -> Option<String> {
    let s = &app.s;
    let off = &app.sel_mask_off;
    let allow_templates = !off.contains("template");
    viewport::pick_object_if(s, vp, cv, p, &mut app.mesh_cache, &|id| (allow_templates || !s.is_template(id)) && crate::toolbar::mask_allows_scene(s, off, id))
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

fn cursor_ring(x: f32, y: f32, cx: f32, cy: f32, r: f32, w: f32) -> bool {
    (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() <= w * 0.5
}

fn cursor_disc(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> bool {
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn cursor_segment(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, w: f32) -> bool {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let t = (((x - x0) * dx + (y - y0) * dy) / (dx * dx + dy * dy).max(1e-6)).clamp(0.0, 1.0);
    let (px, py) = (x0 + dx * t, y0 + dy * t);
    (x - px).powi(2) + (y - py).powi(2) <= (w * 0.5).powi(2)
}

/// Maya's Tab+drag selection cursor: a bullseye plus a plus (add) or minus (subtract).
fn cursor_shape(x: f32, y: f32, mode: SelectCursor, outline: bool) -> bool {
    let (cx, cy) = (11.0, 16.0);
    let (bx, by) = (25.0, 9.0);
    let (arm, tw) = (3.2, if outline { 3.2 } else { 1.8 });
    let (outer, inner, dot) = if outline { (3.6, 3.0, 2.2) } else { (2.0, 1.6, 1.3) };
    let badge = mode == SelectCursor::Add || mode == SelectCursor::Remove;
    let vertical = mode == SelectCursor::Add;
    cursor_ring(x, y, cx, cy, 7.2, outer)
        || cursor_ring(x, y, cx, cy, 4.0, inner)
        || cursor_disc(x, y, cx, cy, dot)
        || (badge && cursor_segment(x, y, bx - arm, by, bx + arm, by, tw))
        || (vertical && cursor_segment(x, y, bx, by - arm, bx, by + arm, tw))
}

fn build_select_cursor(mode: SelectCursor) -> egui::CustomCursorImage {
    const N: usize = 32;
    const SS: usize = 4;
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    let (cr, cg, cb, ca) = if cursor_shape(px, py, mode, false) {
                        (0.96, 0.96, 0.96, 1.0)
                    } else if cursor_shape(px, py, mode, true) {
                        (0.08, 0.08, 0.08, 1.0)
                    } else {
                        (0.0, 0.0, 0.0, 0.0)
                    };
                    r += cr * ca;
                    g += cg * ca;
                    b += cb * ca;
                    a += ca;
                }
            }
            let i = (y * N + x) * 4;
            if a > 0.0 {
                rgba[i] = (r / a * 255.0) as u8;
                rgba[i + 1] = (g / a * 255.0) as u8;
                rgba[i + 2] = (b / a * 255.0) as u8;
                rgba[i + 3] = (a / (SS * SS) as f32 * 255.0) as u8;
            }
        }
    }
    egui::CustomCursorImage { rgba: rgba.into(), size: [N as u16, N as u16], hotspot: [11, 16] }
}

fn select_cursor(mode: SelectCursor) -> egui::CustomCursorImage {
    static CURSORS: std::sync::OnceLock<[egui::CustomCursorImage; 3]> = std::sync::OnceLock::new();
    let cursors = CURSORS.get_or_init(|| [build_select_cursor(SelectCursor::Neutral), build_select_cursor(SelectCursor::Add), build_select_cursor(SelectCursor::Remove)]);
    cursors[match mode {
        SelectCursor::Neutral => 0,
        SelectCursor::Add => 1,
        SelectCursor::Remove => 2,
    }]
    .clone()
}

/// Face under the cursor, considering hilited and selected meshes (never curves).
fn pick_face(app: &mut IncaApp, idx: usize, cv: &CamView, p: Pos2) -> Option<(String, usize)> {
    let vp = app.vps[idx].clone();
    let mut ids: Vec<String> = app.s.hilite();
    ids.retain(|id| app.s.node(id).is_some_and(|n| n.mesh().is_some()));
    for id in &app.s.sel {
        if !ids.contains(id) && app.s.node(id).is_some_and(|n| n.mesh().is_some()) {
            ids.push(id.clone());
        }
    }
    ids.into_iter().find_map(|id| {
        match viewport::pick_component_hit(&app.s, &vp, cv, &id, CompMode::Face, p, &mut app.mesh_cache).map(|h| h.comp) {
            Some(Comp::F(f)) => Some((id, f)),
            _ => None,
        }
    })
}

/// JS `onDbl`: double-clicking an edge selects its loop, double-clicking a face selects all
/// of the object's faces, and an object opens Tool Settings.
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
    if mode == CompMode::Face {
        // Maya: double-clicking a face selects the whole object's faces; face loops are Shift+dbl.
        if let Some((id, _)) = pick_face(app, idx, cv, p) {
            let has_selected_face = !app.s.comp_sel(&id).f.is_empty();
            if !shift || !has_selected_face {
                let name = app.s.name_of(&id);
                app.s.checkpoint_selection("select");
                let _ = app.exec_mel(&format!("select -r {name}.f[*];"), true);
            }
        }
        return;
    }
    let ids = app.s.hilite();
    let Some((id, Comp::E(a, b))) = viewport::pick_best_component(&app.s, &vp, cv, &ids, CompMode::Edge, p, &mut app.mesh_cache) else { return };
    let topo = app.mesh_cache.topo(&app.s, &id);
    let Some(pm) = app.s.node(&id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { return };
    let Some(topo) = topo else { return };
    let name = app.s.name_of(&id);
    let clicked = inca_core::mesh::epair(a, b);
    let selected: Vec<(u32, u32)> = app.s.comp_sel(&id).e.iter().copied().filter(|&e| e != clicked).collect();
    let mut path: Vec<(u32, u32)> = vec![];
    if shift {
        // Shift+double-click another edge: the shortest partial loop/ring between the two edges.
        let mut best: Option<Vec<(u32, u32)>> = None;
        for &se in &selected {
            for p in [inca_core::mesh::edge_loop_path(&topo, clicked, se), inca_core::mesh::edge_ring_path(pm, &topo, clicked, se)] {
                if let Some(p) = p {
                    if best.as_ref().is_none_or(|b| p.len() < b.len()) {
                        best = Some(p);
                    }
                }
            }
        }
        if let Some(p) = best {
            path = p;
        }
    }
    if path.is_empty() && !(shift && !selected.is_empty()) {
        path = inca_core::mesh::edge_loop(pm, &topo, a, b).into_iter().map(|(x, y)| inca_core::mesh::epair(x, y)).collect();
    }
    // one argument per component: the MEL parser does not accept `name.e[0,1,2]`
    let parts: Vec<String> = path.iter().filter_map(|&(x, y)| topo.edge(x, y)).map(|i| format!("{name}.e[{i}]")).collect();
    if parts.is_empty() {
        return;
    }
    app.s.checkpoint_selection("select");
    let _ = app.exec_mel(&format!("select {} {};", if shift { "-add" } else { "-r" }, parts.join(" ")), true);
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

/// The component (in component mode) or object under `p`, as a MEL name.
fn pick_name(app: &mut IncaApp, idx: usize, cv: &CamView, p: Pos2) -> Option<String> {
    let vp = app.vps[idx].clone();
    match app.s.comp_mode {
        Some(cm) => app.s.hilite().into_iter().find_map(|id| {
            let c = viewport::pick_component_hit(&app.s, &vp, cv, &id, cm, p, &mut app.mesh_cache)?.comp;
            let name = app.s.name_of(&id);
            let is_curve = app.s.node(&id).is_some_and(|n| n.curve().is_some());
            Some(match c {
                Comp::V(v) => format!("{name}.{}[{v}]", if is_curve { "cv" } else { "vtx" }),
                Comp::E(a, b) => format!("{name}.e[{}]", app.s.node(&id)?.mesh()?.mesh.topo().edge(a, b)?),
                Comp::F(f) => format!("{name}.f[{f}]"),
            })
        }),
        None => pick_masked(app, &vp, cv, p).and_then(|h| filter_picks(app, vec![h]).pop()).map(|id| app.s.name_of(&id)),
    }
}

fn comp_is_selected(app: &IncaApp, id: &str, c: &Comp) -> bool {
    let sel = app.s.comp_sel(id);
    match c {
        Comp::V(v) => sel.v.contains(v),
        Comp::E(a, b) => sel.e.contains(&epair(*a, *b)),
        Comp::F(f) => sel.f.contains(f),
    }
}

/// Is the MEL component/object name currently selected?
fn item_selected(app: &IncaApp, name: &str) -> bool {
    let Some((obj, rest)) = name.split_once('.') else {
        return app.s.find(name).is_some_and(|id| app.s.sel.contains(&id));
    };
    let Some(id) = app.s.find(obj) else { return false };
    let Some((kind, index)) = rest.split_once('[') else { return false };
    let Ok(index) = index.trim_end_matches(']').parse::<usize>() else { return false };
    let sel = app.s.comp_sel(&id);
    match kind {
        "f" => sel.f.contains(&index),
        "vtx" | "cv" => sel.v.contains(&(index as u32)),
        "e" => app
            .s
            .node(&id)
            .and_then(|n| n.mesh())
            .and_then(|m| m.mesh.topo().edges.get(index).copied())
            .is_some_and(|(a, b)| sel.e.contains(&epair(a, b))),
        _ => false,
    }
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
            if mode == SelMode::Replace && app.s.hilite().iter().any(|id| !app.s.comp_sel_active(id).is_empty()) {
                app.s.checkpoint_selection("select");
                for id in app.s.hilite() {
                    app.s.clear_active_comp(&id);
                }
                app.s.rev += 1;
            }
            return;
        }
        // keep the echo short for big selections
        let mel = format!("select {flag} {};", parts.join(" "));
        app.s.checkpoint_selection("select");
        let _ = app.exec_mel(&mel, parts.len() < 200);
        return;
    }
    let allow_templates = !app.sel_mask_off.contains("template");
    let ids = {
        let s = &app.s;
        viewport::region_objects(s, &vp, cv, region, center, &mut app.mesh_cache, allow_templates)
    };
    let ids = filter_picks(app, ids);
    if !ids.is_empty() || (mode == SelMode::Replace && !app.s.sel.is_empty()) {
        app.s.checkpoint_selection("select");
    }
    app.select_names(&ids, if mode == SelMode::Toggle && ids.is_empty() { SelMode::Add } else { mode });
}


/// Move / rotate / scale manipulator. Returns true while it owns the mouse.
/// Transform channels picked in the Channel Box (all translate, all rotate or all scale): the
/// manipulator shows just those axes, as in Maya.
fn channel_modes(app: &IncaApp) -> Option<(Tool, EnumSet<GizmoMode>)> {
    if !app.panels.cb_armed || app.panels.cb_manips.unwrap_or(2).min(2) < 2 || app.panels.cb_sel.is_empty() || app.s.comp_mode.is_some() {
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

/// The manipulator handle under the cursor (probing each handle on its own). The temporary
/// gizmo is updated with the current target transform first: `GizmoConfig` alone carries no
/// translation/rotation, so picking against it directly would test a gizmo at the world origin.
fn handle_at(config: &GizmoConfig, cursor: Pos2, tf: &Transform) -> Option<GizmoMode> {
    // planes / centre last: an axis drawn over a plane wins
    let mut order: Vec<GizmoMode> = config.modes.iter().collect();
    order.sort_by_key(|m| (m.axes().len(), matches!(m, GizmoMode::TranslateView | GizmoMode::RotateView | GizmoMode::ScaleUniform)));
    order.into_iter().find(|m| {
        let mut c = *config;
        c.modes = EnumSet::only(*m);
        c.mode_override = None;
        let mut g = Gizmo::new(c);
        g.update(GizmoInteraction { cursor_pos: (cursor.x, cursor.y), hovered: true, drag_started: false, dragging: false }, std::slice::from_ref(tf));
        g.pick_preview((cursor.x, cursor.y))
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

/// Which local scale components a scale handle drives. The plane modes are named after the
/// corner they sit at (the axis they keep fixed), while the JS names them after the axes they
/// scale (`XY` scales X and Y), hence the swap.
fn scale_axes(m: GizmoMode) -> [bool; 3] {
    match m {
        GizmoMode::ScaleX => [true, false, false],
        GizmoMode::ScaleY => [false, true, false],
        GizmoMode::ScaleZ => [false, false, true],
        GizmoMode::ScaleXY => [false, true, true],
        GizmoMode::ScaleXZ => [true, false, true],
        GizmoMode::ScaleYZ => [true, true, false],
        _ => [true, true, true],
    }
}

/// JS `middleDrag` rotation: dx * 0.5 degrees about the active axis, applied in world space
/// (`quaternion.copy(st.quat).premultiply(q)`), with the view handle turned the other way.
fn js_rotate(start: DQuat, handle: GizmoMode, dx: f64, snap_deg: Option<f64>, view_fwd: DVec3) -> DQuat {
    let mut deg = dx * 0.5;
    if let Some(s) = snap_deg {
        deg = (deg / s).round() * s;
    }
    let axis = match handle {
        GizmoMode::RotateX => start * DVec3::X,
        GizmoMode::RotateY => start * DVec3::Y,
        GizmoMode::RotateZ => start * DVec3::Z,
        _ => -view_fwd,
    };
    let ang = if handle == GizmoMode::RotateView { -deg } else { deg };
    DQuat::from_axis_angle(axis, ang.to_radians()) * start
}

/// JS `middleDrag` scale: exp(dx * 0.01) on the handle's axes (`Math.max(s, snap(f, s))`).
fn js_scale(handle: GizmoMode, dx: f64, snap: Option<f64>) -> DVec3 {
    let mut f = (dx * 0.01).exp();
    if let Some(s) = snap {
        f = ((f / s).round() * s).max(s);
    }
    let ax = scale_axes(handle);
    DVec3::new(if ax[0] { f } else { 1.0 }, if ax[1] { f } else { 1.0 }, if ax[2] { f } else { 1.0 })
}

/// The pivot and the screen-space unit directions of the manipulator's three axes.
fn screen_axes(pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect) -> Option<(Pos2, [Option<egui::Vec2>; 3])> {
    let p = cv.project(pivot, rect)?;
    let mut dirs = [None; 3];
    for (i, ax) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate() {
        if let Some(q) = cv.project(pivot + rot * ax, rect) {
            let d = q - p;
            let l = d.length();
            if l > 1e-3 {
                dirs[i] = Some(d / l);
            }
        }
    }
    Some((p, dirs))
}

/// Distance from `p` to the segment `a..b`.
fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    let t = if l2 <= 1e-12 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
    (p - (a + ab * t)).length()
}

/// The handle a click on a negative-axis arrow maps to, if the cursor is over one. The JS
/// TransformControls pickers include those arrows; transform-gizmo only draws positive ones.
fn negative_handle_at(tool: Tool, axes: &[bool; 3], pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, cursor: Pos2, size: f32) -> Option<GizmoMode> {
    let (p, dirs) = screen_axes(pivot, rot, cv, rect)?;
    for i in 0..3 {
        if !axes[i] {
            continue;
        }
        let Some(u) = dirs[i] else { continue };
        let a = p - u * (size * 0.2);
        let b = p - u * size;
        if dist_to_segment(cursor, a, b) < 7.0 {
            return Some(if tool == Tool::Scale { [GizmoMode::ScaleX, GizmoMode::ScaleY, GizmoMode::ScaleZ][i] } else { [GizmoMode::TranslateX, GizmoMode::TranslateY, GizmoMode::TranslateZ][i] });
        }
    }
    None
}

/// Draw the negative-axis arrows (JS `manipNegativeHandles`, on by default).
fn draw_negative_handles(painter: &egui::Painter, tool: Tool, axes: &[bool; 3], pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, cursor: Option<Pos2>, size: f32) {
    let Some((p, dirs)) = screen_axes(pivot, rot, cv, rect) else { return };
    let colors = [Color32::from_rgb(230, 57, 62), Color32::from_rgb(92, 214, 92), Color32::from_rgb(74, 132, 255)];
    for i in 0..3 {
        if !axes[i] {
            continue;
        }
        let Some(u) = dirs[i] else { continue };
        let a = p - u * (size * 0.2);
        let b = p - u * size;
        if (b - a).length() < 5.0 {
            continue;
        }
        let hot = cursor.is_some_and(|c| dist_to_segment(c, a, b) < 7.0);
        let mut color = colors[i];
        if hot {
            color = MANIP_HOVER;
        }
        painter.line_segment([a, b], Stroke::new(if hot { 4.0 } else { 2.5 }, color));
        if tool == Tool::Scale {
            painter.rect_filled(Rect::from_center_size(b, vec2(7.0, 7.0)), 0.0, color);
        } else {
            // arrow head pointing along -u
            let base = b + u * 8.0;
            let perp = vec2(-u.y, u.x) * 4.0;
            painter.add(egui::Shape::convex_polygon(vec![b, base + perp, base - perp], color, Stroke::NONE));
        }
    }
}

/// Maya's rotate feedback: two grey sticks from the pivot (the fixed start angle and the rotating
/// current angle) with the swept pie filled between them.
fn draw_rotate_pie(painter: &egui::Painter, d: &GizmoDrag, cv: &CamView, rect: Rect, size: f32, point: f32) {
    let Some(handle) = d.handle else { return };
    let axis = match handle {
        GizmoMode::RotateX => DQuat::from(d.start.rotation) * DVec3::X,
        GizmoMode::RotateY => DQuat::from(d.start.rotation) * DVec3::Y,
        GizmoMode::RotateZ => DQuat::from(d.start.rotation) * DVec3::Z,
        GizmoMode::RotateView => cv.fwd,
        _ => return,
    }
    .normalize();
    let pivot = DVec3::from(d.start.translation);
    let Some(p) = cv.project(pivot, rect) else { return };
    let helper = if axis.dot(DVec3::Y).abs() < 0.9 { DVec3::Y } else { DVec3::X };
    let u = axis.cross(helper).normalize();
    let v = axis.cross(u).normalize();
    // Match the vendor ring radius exactly instead of estimating it from the screen direction.
    let scale_factor = {
        let model = DMat4::from_scale_rotation_translation(DVec3::from(d.start.scale), DQuat::from(d.start.rotation), pivot);
        let mvp = cv.view_proj() * model;
        (mvp.w_axis.w as f32) / (cv.proj.x_axis.x as f32) / rect.width() * 2.0
    };
    let radius = (scale_factor * (size + if handle == GizmoMode::RotateView { 8.0 } else { 0.0 })) as f64;
    // The grab point fixes the start stick. Intersect the press ray with the ring plane for
    // sub-pixel angle precision at every zoom level (sampling the ring was visibly coarse).
    let start = {
        let ray = cv.ray(d.press_px, rect);
        let denom = ray.dir.dot(axis);
        if denom.abs() < 1e-9 {
            0.0
        } else {
            let t = (pivot - ray.origin).dot(axis) / denom;
            let w = ray.origin + ray.dir * t - pivot;
            w.dot(v).atan2(w.dot(u)) as f32
        }
    };
    let delta = DQuat::from(d.cur.rotation) * DQuat::from(d.start.rotation).inverse();
    let mut sweep = 2.0 * DVec3::new(delta.x, delta.y, delta.z).dot(axis).atan2(delta.w);
    if sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    } else if sweep < -std::f64::consts::PI {
        sweep += std::f64::consts::TAU;
    }
    let sweep = sweep as f32;
    let color = Color32::from_rgba_unmultiplied(190, 190, 190, 170);
    let ring = |a: f32| cv.project(pivot + (u * a.cos() as f64 + v * a.sin() as f64) * radius, rect);
    let (Some(start_pos), Some(end_pos)) = (ring(start), ring(start + sweep)) else { return };
    painter.line_segment([p, start_pos], Stroke::new(1.5, color));
    painter.line_segment([p, end_pos], Stroke::new(1.5, color));
    let r = (point * 0.5).max(1.0);
    painter.rect_filled(Rect::from_center_size(p, vec2(r * 2.0, r * 2.0)), 0.0, color);
    painter.rect_filled(Rect::from_center_size(start_pos, vec2(r * 2.0, r * 2.0)), 0.0, color);
    painter.rect_filled(Rect::from_center_size(end_pos, vec2(r * 2.0, r * 2.0)), 0.0, color);
    if sweep.abs() > 0.02 && start_pos.distance(end_pos) > 1.0 {
        let steps = ((sweep.abs() / 0.08) as usize).clamp(2, 48);
        let mut pts = Vec::with_capacity(steps + 2);
        pts.push(p);
        for i in 0..=steps {
            if let Some(q) = ring(start + sweep * i as f32 / steps as f32) {
                pts.push(q);
            }
        }
        if pts.len() > 2 {
            painter.add(egui::Shape::convex_polygon(pts, Color32::from_rgba_unmultiplied(240, 225, 80, 46), Stroke::NONE));
        }
    }
}

fn draw_cube_at(painter: &egui::Painter, center: DVec3, rot: DQuat, cv: &CamView, rect: Rect, half_px: f32, color: Color32) {
    let scale_factor = {
        let model = DMat4::from_scale_rotation_translation(DVec3::ONE, rot, center);
        let mvp = cv.view_proj() * model;
        (mvp.w_axis.w as f32) / (cv.proj.x_axis.x as f32) / rect.width() * 2.0
    };
    let half = (scale_factor * half_px) as f64;
    let corners = [
        DVec3::new(-1.0, -1.0, -1.0),
        DVec3::new(1.0, -1.0, -1.0),
        DVec3::new(1.0, 1.0, -1.0),
        DVec3::new(-1.0, 1.0, -1.0),
        DVec3::new(-1.0, -1.0, 1.0),
        DVec3::new(1.0, -1.0, 1.0),
        DVec3::new(1.0, 1.0, 1.0),
        DVec3::new(-1.0, 1.0, 1.0),
    ];
    let mut pts = [None; 8];
    for (i, c) in corners.iter().enumerate() {
        pts[i] = cv.project(center + rot * (*c * half), rect);
    }
    if pts.iter().any(Option::is_none) {
        return;
    }
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    for p in pts.iter().flatten() {
        min = (min.0.min(p.x), min.1.min(p.y));
        max = (max.0.max(p.x), max.1.max(p.y));
    }
    if max.0 - min.0 > 80.0 || max.1 - min.1 > 80.0 {
        return;
    }
    let light = (cv.up + cv.right * 0.25 - cv.fwd * 0.75).normalize();
    let shade = |f: f64| {
        let k = (0.82 + 0.18 * f.max(0.0)) as f32;
        Color32::from_rgb((color.r() as f32 * k) as u8, (color.g() as f32 * k) as u8, (color.b() as f32 * k) as u8)
    };
    let faces = [
        (DVec3::Z, [4, 5, 6, 7]),
        (DVec3::NEG_Z, [1, 0, 3, 2]),
        (DVec3::X, [1, 2, 6, 5]),
        (DVec3::NEG_X, [0, 4, 7, 3]),
        (DVec3::Y, [3, 7, 6, 2]),
        (DVec3::NEG_Y, [0, 1, 5, 4]),
    ];
    for (normal, idx) in faces {
        let n = rot * normal;
        let face_center = center + n * half;
        if n.dot(cv.eye - face_center) <= 0.0 {
            continue;
        }
        let poly: Vec<Pos2> = idx.iter().filter_map(|&i| pts[i]).collect();
        if poly.len() == 4 {
            painter.add(egui::Shape::convex_polygon(poly, shade(n.dot(light)), Stroke::NONE));
        }
    }
}

fn draw_scale_center_cube(painter: &egui::Painter, pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, scale: f32, color: Color32) {
    draw_cube_at(painter, pivot, rot, cv, rect, MANIP_SIZE * 0.045 * scale, color);
}

/// Maya-style scale axes: a coloured shaft ending in a cube.
fn draw_scale_axis_cubes(painter: &egui::Painter, pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, size: f32, scale: f32, active: Option<GizmoMode>, axes: [bool; 3], dragged_axes: [bool; 3], factors: [f64; 3]) {
    let Some(p) = cv.project(pivot, rect) else { return };
    let scale_factor = {
        let model = DMat4::from_scale_rotation_translation(DVec3::ONE, rot, pivot);
        let mvp = cv.view_proj() * model;
        (mvp.w_axis.w as f32) / (cv.proj.x_axis.x as f32) / rect.width() * 2.0
    };
    let start_len = (scale_factor * (size * 0.2 + 1.5)) as f64;
    let base_len = (scale_factor * (size - 6.0)) as f64;
    let colors = [Color32::from_rgb(230, 57, 62), Color32::from_rgb(92, 214, 92), Color32::from_rgb(74, 132, 255)];
    let modes = [GizmoMode::ScaleX, GizmoMode::ScaleY, GizmoMode::ScaleZ];
    let half = MANIP_SIZE * 0.045 * scale;
    for i in 0..3 {
        if !axes[i] {
            continue;
        }
        let world_axis = rot * [DVec3::X, DVec3::Y, DVec3::Z][i];
        let Some(q) = cv.project(pivot + world_axis, rect) else { continue };
        if q.distance(p) < 2.0 {
            continue;
        }
        let factor = factors[i];
        let center = pivot + world_axis * (base_len * factor);
        let Some(e) = cv.project(center, rect) else { continue };
        let color = if active == Some(modes[i]) { MANIP_SELECTED } else { colors[i] };
        if dragged_axes[i] {
            if factor > 1.0 + 1e-3 {
                // Only the part beyond the old grey handle is shown in yellow.
                if let (Some(a), Some(b)) = (cv.project(pivot + world_axis * base_len, rect), cv.project(center, rect)) {
                    painter.line_segment([a, b], Stroke::new(2.0, color));
                }
            } else if factor < 0.0 {
                if let (Some(a), Some(b)) = (cv.project(pivot + world_axis * (-start_len), rect), cv.project(center, rect)) {
                    painter.line_segment([a, b], Stroke::new(2.0, color));
                }
            }
        } else {
            if let Some(a) = cv.project(pivot + world_axis * (if factor < 0.0 { -start_len } else { start_len }), rect) {
                painter.line_segment([a, e], Stroke::new(2.0, color));
            }
        }
        draw_cube_at(painter, center, rot, cv, rect, half, color);
    }
}

fn scale_center_color(active: Option<GizmoMode>) -> Color32 {
    if active == Some(GizmoMode::ScaleUniform) {
        MANIP_SELECTED
    } else {
        Color32::from_rgb(0, 200, 200)
    }
}

/// The old scale axis while its cube is being dragged: a grey stick from the pivot to the
/// original handle position, with a square head there.
/// Draw the old scale axes as grey sticks with square heads.
fn draw_scale_old_axes(painter: &egui::Painter, pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, size: f32, point: f32, axes: &[usize]) {
    let Some(p) = cv.project(pivot, rect) else { return };
    let scale_factor = {
        let model = DMat4::from_scale_rotation_translation(DVec3::ONE, rot, pivot);
        let mvp = cv.view_proj() * model;
        (mvp.w_axis.w as f32) / (cv.proj.x_axis.x as f32) / rect.width() * 2.0
    };
    let start_len = (scale_factor * (size * 0.2 + 1.5)) as f64;
    let base_len = (scale_factor * (size - 6.0)) as f64;
    let gray = Color32::from_rgba_unmultiplied(190, 190, 190, 170);
    let r = (point * 0.5).max(1.0);
    for &i in axes {
        let world_axis = rot * [DVec3::X, DVec3::Y, DVec3::Z][i];
        let Some(q) = cv.project(pivot + world_axis, rect) else { continue };
        if q.distance(p) < 2.0 {
            continue;
        }
        let (Some(a), Some(e)) = (cv.project(pivot + world_axis * start_len, rect), cv.project(pivot + world_axis * base_len, rect)) else { continue };
        painter.line_segment([a, e], Stroke::new(1.5, gray));
        painter.rect_filled(Rect::from_center_size(e, vec2(r * 2.0, r * 2.0)), 0.0, gray);
    }
}

/// The old scale axes while a drag is active.
fn draw_scale_previous(painter: &egui::Painter, d: &GizmoDrag, cv: &CamView, rect: Rect, size: f32, point: f32) {
    let pivot = DVec3::from(d.start.translation);
    let rot = DQuat::from(d.start.rotation);
    draw_scale_old_axes(painter, pivot, rot, cv, rect, size, point, &previous_state_axes(d.handle));
}


/// that axis; plane handles show the two free axes; view/centre handles show all three.
fn previous_state_axes(handle: Option<GizmoMode>) -> Vec<usize> {
    match handle {
        Some(GizmoMode::TranslateX | GizmoMode::ScaleX | GizmoMode::RotateX) => vec![0],
        Some(GizmoMode::TranslateY | GizmoMode::ScaleY | GizmoMode::RotateY) => vec![1],
        Some(GizmoMode::TranslateZ | GizmoMode::ScaleZ | GizmoMode::RotateZ) => vec![2],
        Some(GizmoMode::TranslateXY | GizmoMode::ScaleXY) => vec![1, 2],
        Some(GizmoMode::TranslateXZ | GizmoMode::ScaleXZ) => vec![0, 2],
        Some(GizmoMode::TranslateYZ | GizmoMode::ScaleYZ) => vec![0, 1],
        _ => vec![0, 1, 2],
    }
}

/// Previous-state feedback while dragging: the start transform's axes with square endpoints.
fn draw_previous_state(painter: &egui::Painter, d: &GizmoDrag, cv: &CamView, rect: Rect, size: f32, point: f32) {
    if point <= 0.0 {
        return;
    }
    let pivot = DVec3::from(d.start.translation);
    let rot = DQuat::from(d.start.rotation);
    let Some((p, dirs)) = screen_axes(pivot, rot, cv, rect) else { return };
    let color = Color32::from_rgba_unmultiplied(190, 190, 190, 170);
    let len = size * 0.65;
    let r = (point * 0.5).max(1.0);
    for i in previous_state_axes(d.handle) {
        let Some(u) = dirs[i] else { continue };
        let e = p + u * len;
        painter.line_segment([p, e], Stroke::new(1.5, color));
        painter.rect_filled(Rect::from_center_size(e, vec2(r * 2.0, r * 2.0)), 0.0, color);
    }
    // the previous pivot itself
    painter.rect_filled(Rect::from_center_size(p, vec2(r * 2.0, r * 2.0)), 0.0, color);
}

/// Maya's Manipulator Visibility preference: hiding applies to the idle manipulator,
/// not to a drag already in progress.
fn manipulator_hidden(prefs: &crate::prefs::Prefs, ctrl: bool, shift: bool, dragging: bool) -> bool {
    !dragging && ((ctrl && prefs.hide_manip_ctrl) || (shift && prefs.hide_manip_shift) || (ctrl && shift && prefs.hide_manip_ctrl_shift))
}

/// The axis locked by a plane handle. `TranslateXY` keeps X fixed (the YZ plane), etc.
fn plane_lock_axis(handle: GizmoMode) -> Option<usize> {
    match handle {
        GizmoMode::TranslateXY | GizmoMode::ScaleXY => Some(0),
        GizmoMode::TranslateXZ | GizmoMode::ScaleXZ => Some(1),
        GizmoMode::TranslateYZ | GizmoMode::ScaleYZ => Some(2),
        _ => None,
    }
}

/// Maya's Move centre handle: a yellow square, or the square lying in the plane left free by a
/// Ctrl+LMB axis lock. Snapping uses the native circle instead. The square is cyan while an axis
/// is the selected handle.
fn center_square_color(active: Option<GizmoMode>) -> Color32 {
    if matches!(active, Some(GizmoMode::TranslateX | GizmoMode::TranslateY | GizmoMode::TranslateZ)) {
        Color32::from_rgb(0, 200, 200)
    } else {
        Color32::from_rgb(240, 225, 80)
    }
}

fn draw_center_square(painter: &egui::Painter, pivot: DVec3, rot: DQuat, cv: &CamView, rect: Rect, scale: f32, locked: Option<usize>, color: Color32) {
    if let Some(axis) = locked {
        let (a, b) = match axis {
            0 => (DVec3::Y, DVec3::Z),
            1 => (DVec3::X, DVec3::Z),
            _ => (DVec3::X, DVec3::Y),
        };
        let upp = if cv.ortho {
            cv.units_per_px()
        } else {
            cv.units_per_px() * (cv.depth(pivot) / cv.coi).max(0.05)
        };
        let half = upp * (MANIP_SIZE as f64 * 0.107) * scale as f64;
        let u = rot * a * half;
        let v = rot * b * half;
        let pts: Vec<Pos2> = [pivot + u + v, pivot - u + v, pivot - u - v, pivot + u - v].into_iter().filter_map(|p| cv.project(p, rect)).collect();
        if pts.len() == 4 {
            painter.add(egui::Shape::closed_line(pts, Stroke::new(2.0, color)));
        }
    } else if let Some(p) = cv.project(pivot, rect) {
        let half = MANIP_SIZE * 0.107 * scale;
        painter.rect_stroke(Rect::from_center_size(p, vec2(half * 2.0, half * 2.0)), 0.0, Stroke::new(2.0, color), egui::StrokeKind::Inside);
        painter.circle_filled(p, 1.5, color);
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

fn paint_gizmo(painter: &egui::Painter, draw: GizmoDrawData) {
    painter.add(egui::Mesh {
        indices: draw.indices,
        vertices: draw.vertices.into_iter().zip(draw.colors).map(|(p, [r, gg, b, a])| egui::epaint::Vertex { pos: p.into(), uv: Pos2::ZERO, color: egui::Rgba::from_rgba_premultiplied(r, gg, b, a).into() }).collect(),
        ..Default::default()
    });
}

/// Orientation from the rotation channels, ignoring negative scale signs.
fn rotation_without_scale(app: &IncaApp, id: &str) -> DQuat {
    let Some(n) = app.s.node(id) else { return DQuat::IDENTITY };
    let ra = quat_from_euler_deg(n.xf.ra, "xyz");
    let local = n.xf.jo_quat() * ra * quat_from_euler_deg(n.xf.r, &n.xf.ro) * ra.inverse();
    app.s.parent_world(id).to_scale_rotation_translation().1 * local
}

fn gizmo(app: &mut IncaApp, ui: &mut egui::Ui, idx: usize, resp: &egui::Response, cv: &CamView, painter: &egui::Painter, interact: bool) -> bool {
    app.vp_state.gizmo_solo[idx] = None;
    let chan = channel_modes(app);
    if (!app.tool.is_manip() && chan.is_none()) || !app.vps[idx].show_manips {
        return false;
    }
    // JS `manipActiveOnly`: other panes keep drawing the scene but hide their manipulators
    if app.prefs.manip_active_only && app.active_vp != idx {
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
        let rot = if tool == Tool::Scale {
            rotation_without_scale(app, lead)
        } else if app.space_world(tool) {
            DQuat::IDENTITY
        } else {
            app.s.world_matrix(lead).to_scale_rotation_translation().1
        };
        (app.s.world_pivot(lead), rot)
    };
    let mut modes = match tool {
        _ if pivot_mode => GizmoMode::all_translate(),
        _ if chan.is_some() => chan.unwrap().1,
        Tool::Move => GizmoMode::all_translate(),
        Tool::Rotate => {
            // Maya's free rotate: dragging the grey area inside the rings is a trackball.
            let mut m = GizmoMode::all_rotate();
            m.insert(GizmoMode::Arcball);
            m
        }
        _ => GizmoMode::all_scale(),
    };
    // JS `manipPlaneHandles`; Edit Pivot also drops the plane handles (and the centre)
    if pivot_mode || !app.prefs.manip_plane_handles {
        for m in [GizmoMode::TranslateXY, GizmoMode::TranslateXZ, GizmoMode::TranslateYZ, GizmoMode::ScaleXY, GizmoMode::ScaleXZ, GizmoMode::ScaleYZ] {
            modes.remove(m);
        }
    }
    // the active handle must be one of the shown handles (JS remembers one per mode)
    let mode_tool = if pivot_mode { Tool::Move } else { tool };
    let mi = mode_index(mode_tool);
    let gizmo_size = MANIP_SIZE * app.manip_size;
    // negative-axis arrows (`manipNegativeHandles`): translate and scale only, never in pivot mode
    let neg_axes: [bool; 3] = if app.prefs.manip_negative_handles && !pivot_mode {
        match mode_tool {
            // Maya's Scale tool has positive-axis cubes only.
            Tool::Scale => [false; 3],
            Tool::Move => [modes.contains(GizmoMode::TranslateX), modes.contains(GizmoMode::TranslateY), modes.contains(GizmoMode::TranslateZ)],
            _ => [false; 3],
        }
    } else {
        [false; 3]
    };
    let default_handle = match mode_tool {
        Tool::Rotate => GizmoMode::RotateView,
        Tool::Scale => GizmoMode::ScaleUniform,
        _ => GizmoMode::TranslateView,
    };
    let mut active = app.vp_state.active_handles[mi].filter(|m| modes.contains(*m)).or_else(|| modes.contains(default_handle).then_some(default_handle)).or_else(|| modes.iter().next());
    let ppp = ui.ctx().pixels_per_point();
    let space_world = app.space_world(tool);
    let pointer = ui.input(|i| i.pointer.clone());
    let mods = ui.input(|i| i.modifiers);
    let ctrl = mods.ctrl || mods.command;
    let cursor = pointer.hover_pos().unwrap_or_default();
    // Maya's Hide Manipulator preference hides the idle manipulator so clicks pass through
    // to component/object selection. Do not abort a drag that is already underway.
    if manipulator_hidden(&app.prefs, ctrl, mods.shift, app.vp_state.drag.is_some()) {
        return false;
    }
    let lmb_pressed = interact && resp.hovered() && pointer.button_pressed(egui::PointerButton::Primary);
    let mmb_pressed = interact && resp.hovered() && pointer.button_pressed(egui::PointerButton::Middle) && !mods.alt;
    let locked_pressed = interact && app.lock_selection && lmb_pressed;
    // A new press during an old drag (for example LMB right after an MMB drag) must finish the
    // old drag and drop the gizmo's internal active subgizmo. Otherwise the new press keeps
    // driving the previous handle and the manipulator looks stuck.
    if (lmb_pressed && (app.vp_state.drag.is_some() || app.vp_state.mmb.is_some())) || (mmb_pressed && app.vp_state.drag.is_some()) {
        if let Some(d) = app.vp_state.drag.take() {
            finish_drag(app, d);
        }
        app.vp_state.mmb = None;
        app.vp_state.force_handle = None;
        app.vp_state.auto_drag = None;
        app.vp_state.press_handle = None;
        app.vp_state.gizmos[idx] = Gizmo::default();
    }
    // a forced / MMB drag ends with its button
    if app.vp_state.force_handle.is_some_and(|(v, _, b)| v == idx && !pointer.button_down(b)) {
        app.vp_state.force_handle = None;
    }
    if app.vp_state.mmb == Some(idx) && !pointer.button_down(egui::PointerButton::Middle) {
        app.vp_state.mmb = None;
    }
    let g = &mut app.vp_state.gizmos[idx];
    let mut visuals = GizmoVisuals { gizmo_size: MANIP_SIZE * app.manip_size, stroke_width: 3.0, ..Default::default() };
    visuals.x_color = Color32::from_rgb(230, 57, 62);
    visuals.y_color = Color32::from_rgb(92, 214, 92);
    visuals.z_color = Color32::from_rgb(74, 132, 255);
    visuals.s_color = match mode_tool {
        Tool::Rotate => Color32::from_rgb(120, 190, 255),
        Tool::Move => Color32::from_rgb(240, 225, 80),
        _ => Color32::from_rgb(220, 220, 220),
    };
    // The interaction gizmo (drawn while rotating) keeps the strong yellow; the drawing gizmo
    // below overrides this to pale yellow so hover and selected stay distinct.
    visuals.highlight_color = Some(MANIP_SELECTED);
    // JS `dragStep`: grid snap (X) snaps translation to the grid spacing; discrete move / rotate /
    // scale each have their own step; holding J is a 15 degree rotate snap
    let key_x = mode_tool == Tool::Move && ui.input(|i| i.key_down(egui::Key::X));
    let key_j = mode_tool == Tool::Rotate && ui.input(|i| i.key_down(egui::Key::J));
    let trans_step = if app.snap_grid || key_x {
        Some(app.prefs.grid_spacing)
    } else if app.vp_state.discrete[0] {
        Some(app.vp_state.snap_step)
    } else {
        None
    };
    let rot_step = if key_j {
        Some(15.0)
    } else if app.vp_state.discrete[1] {
        Some(app.vp_state.snap_angle)
    } else {
        None
    };
    let scl_step = app.vp_state.discrete[2].then_some(app.vp_state.snap_scale);
    let (snapping, snap_distance, snap_angle, snap_scale) = match mode_tool {
        Tool::Move => (trans_step.is_some(), trans_step.unwrap_or(1.0) as f32, 0.0, 0.0),
        Tool::Rotate => (rot_step.is_some(), 0.0, (rot_step.unwrap_or(15.0) as f32).to_radians(), 0.0),
        _ => (scl_step.is_some(), 0.0, 0.0, scl_step.unwrap_or(1.0) as f32),
    };
    let mut base = GizmoConfig {
        view_matrix: cv.view.into(),
        projection_matrix: cv.proj.into(),
        viewport: resp.rect,
        modes,
        orientation: if space_world && tool != Tool::Scale && app.s.comp_mode.is_none() { GizmoOrientation::Global } else { GizmoOrientation::Local },
        // MMB: drag the active handle from anywhere in the view; Ctrl+LMB / auto-axis force a handle
        mode_override: app.vp_state.force_handle.filter(|f| f.0 == idx).map(|f| f.1).or(if app.vp_state.mmb == Some(idx) { active } else { None }),
        snapping,
        snap_distance,
        snap_angle,
        snap_scale,
        visuals,
        pixels_per_point: ppp,
        ..Default::default()
    };
    g.update_config(base);
    let tf = Transform::from_scale_rotation_translation(DVec3::ONE, rot, pivot);
    // A click / MMB press on a handle makes it the active one. Ctrl turns move/scale axes into
    // their drag plane and starts that drag at once; Shift on the centre (or Shift+MMB away from
    // a handle) picks the axis closest to the drag after 6 px, as the JS `middleDrag` does.
    let mut consume_primary = false;
    let mut start_mmb = false;
    let mut start_auto = false;
    let mut start_force: Option<GizmoMode> = None;
    let mut press = None;
    let mut mmb_press = None;
    let hover = if lmb_pressed || mmb_pressed {
        handle_at(g.config(), cursor, &tf).or_else(|| negative_handle_at(mode_tool, &neg_axes, pivot, rot, cv, app.vps[idx].rect, cursor, gizmo_size))
    } else {
        None
    };
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
    } else if locked_pressed {
        // Lock Current Selection: LMB drags the active handle from anywhere, like MMB normally.
        start_force = active;
        press = active;
    }
    if let Some(h) = start_force {
        app.vp_state.force_handle = Some((idx, h, egui::PointerButton::Primary));
    }
    if start_mmb {
        app.vp_state.mmb = Some(idx);
        mmb_press = Some(cursor);
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
    let mut auto_origin = None;
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
            auto_origin = Some(a.origin);
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
        hovered: interact && !auto_pending && (resp.hovered() || app.vp_state.drag.is_some() || app.vp_state.mmb == Some(idx) || app.vp_state.force_handle.is_some_and(|f| f.0 == idx)),
        drag_started: interact && !auto_pending && (lmb_pressed || (app.vp_state.mmb == Some(idx) && pointer.button_pressed(egui::PointerButton::Middle)) || auto_started),
        dragging: interact && !auto_pending && (pointer.button_down(egui::PointerButton::Primary) || app.vp_state.mmb == Some(idx)),
    };
    let focused_before = g.is_focused();
    let mut result = g.update(interaction, &[tf]);
    if auto_started {
        // apply the movement already dragged past the 6 px threshold in this same frame
        result = g.update(GizmoInteraction { cursor_pos: (cursor.x, cursor.y), ..interaction }, &[tf]);
    }
    // The interaction gizmo can have its internal transform moved by the raw result (the
    // off-handle axis pick starts with a jump), which would make its handles follow the cursor.
    // Draw from a separate gizmo fed the object's real transform instead.
    let js_drag = auto_origin.or(mmb_press);
    let forced = !auto_started && (js_drag.is_some() || app.vp_state.force_handle.is_some_and(|f| f.0 == idx));
    let rebase = if app.vp_state.drag.is_none() && forced && drag_handle.is_some_and(|m| m.is_translate()) {
        result.as_ref().and_then(|(_, t)| t.first()).map(|n| DVec3::from(n.translation) - DVec3::from(tf.translation))
    } else {
        None
    };
    let mut draw_tf = result.as_ref().and_then(|(_, t)| t.first()).copied().unwrap_or(tf);
    if let Some(bias) = rebase {
        draw_tf.translation = (DVec3::from(draw_tf.translation) - bias).into();
    }
    // MMB / Shift+MMB drags show only the active handle, as the JS version did.
    let solo = app.vp_state.drag.as_ref().filter(|d| d.vp == idx && d.js_drag.is_some()).and_then(|d| d.handle).or_else(|| {
        if js_drag.is_some() && interaction.drag_started {
            drag_handle
        } else {
            None
        }
    });
    app.vp_state.gizmo_solo[idx] = solo;
    // Handle under the cursor for the idle hover tint. During a press/drag the pressed-state
    // drawing takes over instead.
    let hover_handle = if interact && resp.hovered() && app.vp_state.drag.is_none() && !interaction.dragging {
        handle_at(g.config(), cursor, &tf)
    } else {
        None
    };
    let active_draw = app.vp_state.drag.as_ref().filter(|d| d.vp == idx).and_then(|d| d.handle).or_else(|| app.vp_state.active_handles[mi].filter(|m| modes.contains(*m)));
    let plane_lock = if mode_tool == Tool::Move { active_draw.and_then(plane_lock_axis) } else { None };
    let snap = app.snap_grid || app.snap_point || app.snap_curve || ui.input(|i| i.key_down(egui::Key::X) || i.key_down(egui::Key::V) || i.key_down(egui::Key::C));
    let move_center = mode_tool == Tool::Move
        && solo.map_or(true, |h| h == GizmoMode::TranslateView)
        && (plane_lock.is_some() || (modes.contains(GizmoMode::TranslateView) && !snap));
    let scale_center = mode_tool == Tool::Scale && solo.map_or(true, |h| h == GizmoMode::ScaleUniform);
    let center_square = move_center || scale_center;
    let center_color = if scale_center {
        if hover_handle == Some(GizmoMode::ScaleUniform) {
            MANIP_HOVER
        } else {
            scale_center_color(active_draw.or(active))
        }
    } else if hover_handle == Some(GizmoMode::TranslateView) {
        MANIP_HOVER
    } else {
        center_square_color(active_draw)
    };
    // The image was rendered before interaction, so during a drag it shows the previous drag
    // result. Draw the manipulator from that same transform instead of the new one.
    let visual_tf = if app.vp_state.drag.is_some() {
        app.vp_state.gizmo_draw_tf[idx].unwrap_or(draw_tf)
    } else {
        draw_tf
    };
    let draw_pivot = DVec3::from(visual_tf.translation);
    let draw_rot = DQuat::from(visual_tf.rotation);
    let scale_drag_mode = if mode_tool == Tool::Scale {
        app.vp_state.drag.as_ref().filter(|d| d.vp == idx).and_then(|d| d.handle).filter(|m| matches!(m, GizmoMode::ScaleX | GizmoMode::ScaleY | GizmoMode::ScaleZ))
    } else {
        None
    };
    let scale_pressed = if mode_tool == Tool::Scale && scale_drag_mode.is_none() && pointer.button_down(egui::PointerButton::Primary) {
        app.vp_state.press_handle.filter(|m| matches!(m, GizmoMode::ScaleX | GizmoMode::ScaleY | GizmoMode::ScaleZ))
    } else {
        None
    };
    let scale_custom_axis = scale_drag_mode.or(scale_pressed);
    let scale_uniform_drag = if mode_tool == Tool::Scale {
        app.vp_state.drag.as_ref().filter(|d| d.vp == idx).and_then(|d| d.handle).filter(|m| *m == GizmoMode::ScaleUniform)
    } else {
        None
    };
    let scale_plane_drag = if mode_tool == Tool::Scale {
        app.vp_state.drag.as_ref().filter(|d| d.vp == idx).and_then(|d| d.handle).filter(|m| matches!(m, GizmoMode::ScaleXY | GizmoMode::ScaleXZ | GizmoMode::ScaleYZ))
    } else {
        None
    };
    let mut draw_base = base;
    draw_base.mode_override = None;
    draw_base.visuals.highlight_color = Some(MANIP_HOVER);
    if let Some(h) = solo {
        // Keep the full mode set so the arrow geometry (inner radius / length) matches the full
        // gizmo; the override builds and draws only the active subgizmo.
        if mode_tool != Tool::Scale {
            draw_base.mode_override = Some(h);
        }
    }
    if let Some(h) = scale_custom_axis {
        draw_base.modes.remove(h);
    }
    if scale_uniform_drag.is_some() {
        for m in [GizmoMode::ScaleX, GizmoMode::ScaleY, GizmoMode::ScaleZ] {
            draw_base.modes.remove(m);
        }
    }
    if let Some(h) = scale_plane_drag {
        for i in previous_state_axes(Some(h)) {
            draw_base.modes.remove([GizmoMode::ScaleX, GizmoMode::ScaleY, GizmoMode::ScaleZ][i]);
        }
    }
    if center_square {
        if solo == Some(GizmoMode::TranslateView) || solo == Some(GizmoMode::ScaleUniform) {
            draw_base.modes = EnumSet::new();
            draw_base.mode_override = None;
        } else {
            draw_base.modes.remove(GizmoMode::TranslateView);
            draw_base.modes.remove(GizmoMode::ScaleUniform);
        }
    }
    if mode_tool == Tool::Move {
        draw_base.visuals.s_color = center_color;
    }
    let mut draw_gizmo = Gizmo::new(draw_base);
    draw_gizmo.update(GizmoInteraction { cursor_pos: (cursor.x, cursor.y), hovered: solo.is_some() || resp.hovered(), drag_started: false, dragging: false }, &[visual_tf]);
    let draw = if mode_tool == Tool::Rotate && app.vp_state.drag.as_ref().is_some_and(|d| d.vp == idx) {
        // The interaction gizmo holds the active swept-arc state; the fresh drawing gizmo would
        // only show the idle rings.
        g.draw()
    } else {
        draw_gizmo.draw()
    };
    // manipulator guide: a faint line along the axis being dragged (Preferences can hide it)
    if app.prefs.manip_guides && mode_tool != Tool::Rotate {
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
    if let Some(d) = app.vp_state.drag.as_ref().filter(|d| d.vp == idx && d.tool == Tool::Move) {
        draw_previous_state(painter, d, cv, app.vps[idx].rect, gizmo_size, app.prefs.manip_prev_size * app.manip_size);
    }
    // Edit Pivot: a blue ring around the pivot, kept while dragging; an inner ring while point snap is on
    if pivot_mode {
        if let Some(c) = cv.project(pivot, app.vps[idx].rect) {
            let blue = Color32::from_rgb(95, 211, 255);
            let r = MANIP_SIZE * 0.187 * app.manip_size;
            painter.circle_stroke(c, r, Stroke::new(2.0, blue));
            if app.snap_point || ui.input(|i| i.key_down(egui::Key::V)) {
                painter.circle_stroke(c, r * 0.55, Stroke::new(2.0, blue));
            }
        }
    }
    paint_gizmo(painter, draw);
    if mode_tool == Tool::Rotate {
        if let Some(d) = app.vp_state.drag.as_ref().filter(|d| d.vp == idx) {
            draw_rotate_pie(painter, d, cv, app.vps[idx].rect, gizmo_size, app.prefs.manip_prev_size * app.manip_size);
        }
    }
    if mode_tool == Tool::Scale {
        let drag = app.vp_state.drag.as_ref().filter(|d| d.vp == idx && d.tool == Tool::Scale);
        let factors = drag.map_or([1.0; 3], |d| {
            let s = DVec3::from(d.start.scale);
            let c = DVec3::from(d.cur.scale);
            let f = |c: f64, s: f64| {
                if s.abs() < 1e-9 {
                    1.0
                } else {
                    let f = c / s;
                    if f.abs() < 0.02 { 0.02 * if f < 0.0 { -1.0 } else { 1.0 } } else { f }
                }
            };
            [f(c.x, s.x), f(c.y, s.y), f(c.z, s.z)]
        });
        if let Some(d) = drag {
            draw_scale_previous(painter, d, cv, app.vps[idx].rect, gizmo_size, app.prefs.manip_prev_size * app.manip_size);
        } else if let Some(h) = scale_pressed {
            draw_scale_old_axes(painter, draw_pivot, draw_rot, cv, app.vps[idx].rect, gizmo_size, app.prefs.manip_prev_size * app.manip_size, &previous_state_axes(Some(h)));
        }
        if let Some(h) = scale_custom_axis {
            let dragged = [
                h == GizmoMode::ScaleX,
                h == GizmoMode::ScaleY,
                h == GizmoMode::ScaleZ,
            ];
            draw_scale_axis_cubes(painter, draw_pivot, draw_rot, cv, app.vps[idx].rect, gizmo_size, app.manip_size, Some(h), dragged, dragged, factors);
        }
        if scale_uniform_drag.is_some() {
            // Uniform scaling moves all three axis cubes in or out together.
            draw_scale_axis_cubes(painter, draw_pivot, draw_rot, cv, app.vps[idx].rect, gizmo_size, app.manip_size, Some(GizmoMode::ScaleUniform), [true; 3], [true; 3], factors);
        }
        if let Some(h) = scale_plane_drag {
            // A plane drag moves the two free axes' cubes like a single-axis drag.
            let mut free = [false; 3];
            for i in previous_state_axes(Some(h)) {
                free[i] = true;
            }
            draw_scale_axis_cubes(painter, draw_pivot, draw_rot, cv, app.vps[idx].rect, gizmo_size, app.manip_size, Some(h), free, free, factors);
        }
    }
    // Highlight a remembered/active handle by drawing it again in yellow above the full gizmo.
    // Using the gizmo's own geometry keeps the selected shape identical to the hovered one.
    // Skip it while the cursor is on that handle so the pale hover tint stays visible.
    if solo.is_none() && scale_custom_axis.is_none() && !(mode_tool == Tool::Rotate && app.vp_state.drag.as_ref().is_some_and(|d| d.vp == idx)) {
        if let Some(h) = active_draw
            .filter(|m| !matches!(m, GizmoMode::TranslateView | GizmoMode::ScaleUniform | GizmoMode::Arcball))
            .filter(|m| hover_handle != Some(*m))
        {
            let mut hl_base = base;
            hl_base.mode_override = Some(h);
            hl_base.visuals.highlight_color = Some(MANIP_SELECTED);
            let mut hl_gizmo = Gizmo::new(hl_base);
            hl_gizmo.update(GizmoInteraction { cursor_pos: (cursor.x, cursor.y), hovered: true, drag_started: false, dragging: false }, &[visual_tf]);
            paint_gizmo(painter, hl_gizmo.draw());
        }
    }
    // the negative-axis arrows transform-gizmo doesn't draw
    let rect = app.vps[idx].rect;
    if neg_axes.iter().any(|b| *b) && app.vp_state.drag.is_none() {
        draw_negative_handles(painter, mode_tool, &neg_axes, draw_pivot, draw_rot, cv, rect, resp.hovered().then_some(cursor), gizmo_size);
    }
    if scale_center {
        draw_scale_center_cube(painter, draw_pivot, draw_rot, cv, rect, app.manip_size, center_color);
    } else if center_square {
        draw_center_square(painter, draw_pivot, draw_rot, cv, rect, app.manip_size, plane_lock, center_color);
    }
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
                let local_scales: Vec<DVec3> = objs.iter().map(|(o, _)| app.s.node(o).map(|n| DVec3::from(n.xf.s)).unwrap_or(DVec3::ONE)).collect();
                app.vp_state.drag = Some(GizmoDrag { vp: idx, start: tf, cur: tf, objects: objs, local_scales, comps, tool: if pivot_mode { Tool::Move } else { app.tool }, pivots, weights, mirror, extrude, handle: drag_handle, js_drag, press_px: auto_origin.or(mmb_press).unwrap_or(cursor), rebase });
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            if let Some(n) = new_tfs.first() {
                d.cur = *n;
                if let Some(bias) = d.rebase.take() {
                    d.cur.translation = (DVec3::from(d.cur.translation) - bias).into();
                }
            }
            // JS `middleDrag` maths for MMB / Shift auto-axis drags: rotate by dx * 0.5 degrees
            // around the active axis and scale by exp(dx * 0.01). Normal LMB handle drags keep
            // TransformControls' tangential / radial behaviour.
            if let (Some(origin), Some(handle)) = (d.js_drag, d.handle) {
                let dx = (cursor.x - origin.x) as f64;
                // JS `dragStep`: holding J is a 15 degree rotation snap, otherwise the mode's
                // discrete step (if enabled)
                let key_j = ui.input(|i| i.key_down(egui::Key::J));
                match handle {
                    GizmoMode::RotateX | GizmoMode::RotateY | GizmoMode::RotateZ | GizmoMode::RotateView => {
                        let snap = if key_j {
                            Some(15.0)
                        } else if app.vp_state.discrete[1] {
                            Some(app.vp_state.snap_angle)
                        } else {
                            None
                        };
                        d.cur.rotation = js_rotate(DQuat::from(d.start.rotation), handle, dx, snap, cv.fwd).into();
                        d.cur.translation = d.start.translation;
                        d.cur.scale = d.start.scale;
                    }
                    GizmoMode::ScaleX | GizmoMode::ScaleY | GizmoMode::ScaleZ | GizmoMode::ScaleXY | GizmoMode::ScaleXZ | GizmoMode::ScaleYZ | GizmoMode::ScaleUniform => {
                        let snap = app.vp_state.discrete[2].then_some(app.vp_state.snap_scale);
                        let m = js_scale(handle, dx, snap);
                        d.cur.scale = (m * DVec3::from(d.start.scale)).into();
                        d.cur.rotation = d.start.rotation;
                        d.cur.translation = d.start.translation;
                    }
                    // Free rotate: keep the gizmo's trackball result instead of the dx maths.
                    GizmoMode::Arcball => {}
                    _ => {}
                }
            }
            // hold C (or the status line toggle): the moved pivot snaps to the nearest curve point
            // (a Rust extension; the point and grid snaps below take priority as in the JS)
            if d.handle.is_some_and(|m| m.is_translate()) && (app.snap_curve || ui.input(|i| i.key_down(egui::Key::C))) {
                let moving: HashSet<String> = d.objects.iter().map(|o| o.0.clone()).collect();
                if let Some(w) = snap_curve_point(&app.s, &app.vps[idx], cv, cursor, &moving) {
                    d.cur.translation = w.into();
                }
            }
            // JS `snapDuringDrag`: the point snap runs first and wins over the grid; the centre
            // handle with grid snap slides on the ground grid (persp) or the view plane's grid (ortho)
            if d.handle.is_some_and(|m| m.is_translate()) {
                let point_on = app.snap_point || ui.input(|i| i.key_down(egui::Key::V));
                let comp = app.s.comp_mode.is_some();
                let pivot = d.pivots.is_some();
                if point_on {
                    // object mode never snaps to a selected object (moved or not); a pivot may snap
                    // to its own object, and moved components never snap onto themselves
                    let excl: HashSet<String> = if comp || pivot { HashSet::new() } else { app.s.sel.iter().cloned().collect() };
                    let skip: std::collections::HashMap<String, HashSet<u32>> = if comp {
                        d.comps.iter().map(|(id, v, _)| (id.clone(), v.iter().copied().collect())).collect()
                    } else {
                        Default::default()
                    };
                    if let Some(sp) = snap_point_near(&app.s, &app.vps[idx], &app.sel_mask_off, cv, cursor, &excl, &skip, f32::INFINITY) {
                        let st = DVec3::from(d.start.translation);
                        let handle = d.handle.unwrap_or(GizmoMode::TranslateView);
                        d.cur.translation = project_snap_point(sp, st, DQuat::from(d.start.rotation), handle).into();
                        d.cur.rotation = d.start.rotation;
                        d.cur.scale = d.start.scale;
                    }
                }
            }
            // The app can fail to name the centre handle (handle=None) while the gizmo still
            // starts the drag; the centre grid snap must not depend on the handle being named.
            if matches!(d.handle, Some(GizmoMode::TranslateView) | None) && (app.snap_grid || ui.input(|i| i.key_down(egui::Key::X))) {
                let st = DVec3::from(d.start.translation);
                if let Some(w) = grid_snap_center(cv, app.vps[idx].rect, st, cursor, d.press_px, app.prefs.grid_spacing.max(1e-6)) {
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
                let final_tf = app.vp_state.drag.as_ref().map(|d| d.cur).unwrap_or(tf);
                if !interaction.dragging {
                    if let Some(d) = app.vp_state.drag.take() {
                        finish_drag(app, d);
                    }
                }
                app.vp_state.gizmo_draw_tf[idx] = Some(final_tf);
                return true;
            }
            let d = app.vp_state.drag.as_mut().unwrap();
            let delta = tf_matrix(&d.cur) * tf_matrix(&d.start).inverse();
            let objs = d.objects.clone();
            let local_scales = d.local_scales.clone();
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
                    if mode_tool == Tool::Scale && comps.is_empty() {
                        // Apply signed scale directly to the local channels. Going through the
                        // world matrix forces the negative sign onto X during decomposition.
                        let factor = DVec3::from(d.cur.scale) / DVec3::from(d.start.scale);
                        for ((id, _), base) in objs.iter().zip(&local_scales) {
                            if let Some(n) = app.s.nodes.get_mut(id) {
                                n.xf.s = [base.x * factor.x, base.y * factor.y, base.z * factor.z];
                                n.rev += 1;
                            }
                            app.s.refresh_dependents(id);
                        }
                    } else {
                        for (id, start) in objs {
                            app.s.set_world_matrix(&id, delta * start);
                        }
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
            // The gizmo can return a final result on the release frame. Finish the drag here too,
            // otherwise the next press reuses this drag's start point and press position.
            let final_tf = app.vp_state.drag.as_ref().map(|d| d.cur).unwrap_or(tf);
            if !interaction.dragging {
                if let Some(d) = app.vp_state.drag.take() {
                    finish_drag(app, d);
                }
            }
            app.vp_state.gizmo_draw_tf[idx] = Some(final_tf);
            true
        }
        None => {
            if auto_pending || consume_primary {
                app.vp_state.gizmo_draw_tf[idx] = Some(tf);
                return true;
            }
            if let Some(d) = app.vp_state.drag.take() {
                let final_tf = d.cur;
                finish_drag(app, d);
                app.vp_state.gizmo_draw_tf[idx] = Some(final_tf);
                return true;
            }
            // a press on a handle belongs to the gizmo
            app.vp_state.gizmo_draw_tf[idx] = Some(tf);
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
    let snap = app.snap_grid.then_some(app.prefs.grid_spacing);
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
    let snap = app.snap_grid.then_some(app.prefs.grid_spacing);
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

/// The JS `snapPointNear`: the mesh vertex or curve CV nearest the cursor among visible nodes.
///
/// `exclude` drops whole objects (the selected objects in object mode), `skip` drops individual
/// vertices of moved components, and `radius` caps the screen distance (`Infinity` during a drag:
/// the nearest point always wins, so the manipulator stays on it until another is closer).
#[allow(clippy::too_many_arguments)]
fn snap_point_near(
    s: &inca_core::scene::Scene,
    vp: &viewport::Viewport,
    off: &HashSet<String>,
    cv: &CamView,
    cursor: Pos2,
    exclude: &HashSet<String>,
    skip: &std::collections::HashMap<String, HashSet<u32>>,
    radius: f32,
) -> Option<DVec3> {
    let mut best: Option<(f32, DVec3)> = None;
    for id in s.all_dag(false) {
        if exclude.contains(&id) || !s.is_visible(&id) || s.is_template(&id) || !crate::toolbar::mask_allows_scene(s, off, &id) {
            continue;
        }
        if let Some(iso) = &vp.isolate {
            if !iso.iter().any(|r| r == &id || s.is_ancestor(r, &id)) {
                continue;
            }
        }
        let Some(node) = s.node(&id) else { continue };
        let wm = s.world_matrix(&id);
        let sk = skip.get(&id);
        let mut test = |p: DVec3| {
            let w = wm.transform_point3(p);
            if let Some(q) = cv.project(w, vp.rect) {
                let d = q.distance(cursor);
                if d < radius && best.is_none_or(|b| d < b.0) {
                    best = Some((d, w));
                }
            }
        };
        match &node.shape {
            Shape::Mesh(m) => {
                for (i, p) in m.mesh.v.iter().enumerate() {
                    if sk.is_some_and(|s| s.contains(&(i as u32))) {
                        continue;
                    }
                    test(*p);
                }
            }
            Shape::Curve(c) => {
                for (i, p) in c.cvs.iter().enumerate() {
                    if sk.is_some_and(|s| s.contains(&(i as u32))) {
                        continue;
                    }
                    test(*p);
                }
            }
            _ => {}
        }
    }
    best.map(|b| b.1)
}

/// JS `snapDuringDrag`: project a snapped point onto the dragged axis, or onto the dragged
/// plane (dropping the component along the plane's normal). Centre handles take the point as is.
///
/// transform-gizmo names its plane handles after the corner they sit at, so `TranslateXY` keeps
/// X fixed (normal X) and `TranslateYZ` keeps Z fixed (normal Z).
fn project_snap_point(sp: DVec3, st: DVec3, rot: DQuat, handle: GizmoMode) -> DVec3 {
    let d = sp - st;
    let axis = |a: DVec3| st + a * d.dot(a);
    let plane = |n: DVec3| sp - n * d.dot(n);
    match handle {
        GizmoMode::TranslateX => axis(rot * DVec3::X),
        GizmoMode::TranslateY => axis(rot * DVec3::Y),
        GizmoMode::TranslateZ => axis(rot * DVec3::Z),
        GizmoMode::TranslateXY => plane(rot * DVec3::X),
        GizmoMode::TranslateXZ => plane(rot * DVec3::Y),
        GizmoMode::TranslateYZ => plane(rot * DVec3::Z),
        _ => sp,
    }
}

/// JS `snapDuringDrag` grid branch for the centre handle: the start point's screen position
/// plus the mouse delta picks a point on the ground plane (persp, y = 0) or the view plane
/// through the start (ortho, keeping the depth along the dominant view axis), then X/Z snap
/// to the grid spacing.
fn grid_snap_center(cv: &CamView, rect: Rect, st: DVec3, cursor: Pos2, press: Pos2, spacing: f64) -> Option<DVec3> {
    let a = cv.project(st, rect)?;
    let ray = cv.ray(a + (cursor - press), rect);
    let snap = |x: f64| (x / spacing).round() * spacing;
    if cv.ortho {
        let n = cv.fwd;
        let denom = ray.dir.dot(n);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = (st - ray.origin).dot(n) / denom;
        let p = ray.at(t);
        let back = [n.x.abs(), n.y.abs(), n.z.abs()];
        let depth = if back[0] >= back[1] && back[0] >= back[2] { 0 } else if back[1] >= back[2] { 1 } else { 2 };
        let mut out = [snap(p.x), snap(p.y), snap(p.z)];
        out[depth] = [st.x, st.y, st.z][depth];
        Some(DVec3::from(out))
    } else {
        if ray.dir.y.abs() < 1e-9 {
            return None;
        }
        let t = (0.0 - ray.origin.y) / ray.dir.y;
        let p = ray.at(t);
        Some(DVec3::new(snap(p.x), 0.0, snap(p.z)))
    }
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
            let cs = app.s.comp_sel_active(id);
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
        // JS `drawHud` uses `fmt(App.time.current, 2)` (two decimals, trailing zeros trimmed)
        right += &format!("Current Frame: {}\n", inca_core::math::fmt_num(inca_core::math::round_to(app.s.time.current, 2)));
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
    fn active_plane_is_redrawn_yellow() {
        let tf = Transform::from_scale_rotation_translation(DVec3::ONE, DQuat::IDENTITY, DVec3::ZERO);
        let view = DMat4::look_at_rh(DVec3::new(0.0, 0.0, 10.0), DVec3::ZERO, DVec3::Y);
        let proj = DMat4::perspective_rh(45.0_f64.to_radians(), 1.0, 0.1, 100.0);
        let mut visuals = GizmoVisuals::default();
        visuals.highlight_color = Some(Color32::from_rgb(240, 225, 80));
        let mut g = Gizmo::new(GizmoConfig {
            view_matrix: view.into(),
            projection_matrix: proj.into(),
            viewport: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)),
            modes: GizmoMode::all_translate(),
            mode_override: Some(GizmoMode::TranslateYZ),
            pixels_per_point: 1.0,
            visuals,
            ..Default::default()
        });
        g.update(GizmoInteraction { cursor_pos: (400.0, 300.0), hovered: true, drag_started: false, dragging: false }, &[tf]);
        let draw = g.draw();
        assert!(!draw.vertices.is_empty(), "plane highlight drew nothing");
        for c in &draw.colors {
            if c[3] < 0.1 {
                continue;
            }
            assert!(c[0] > 0.5 && c[1] > 0.4 && c[2] < 0.4, "plane highlight was not yellow: {c:?}");
        }
    }

    #[test]
    fn center_square_is_cyan_while_an_axis_is_selected() {
        let cyan = Color32::from_rgb(0, 200, 200);
        let yellow = Color32::from_rgb(240, 225, 80);
        for axis in [GizmoMode::TranslateX, GizmoMode::TranslateY, GizmoMode::TranslateZ] {
            assert_eq!(center_square_color(Some(axis)), cyan, "{axis:?} should deselect the centre");
        }
        for other in [None, Some(GizmoMode::TranslateView), Some(GizmoMode::TranslateXY)] {
            assert_eq!(center_square_color(other), yellow, "{other:?} should keep the centre yellow");
        }
    }

    #[test]
    fn plane_handles_report_the_locked_axis() {
        assert_eq!(plane_lock_axis(GizmoMode::TranslateXY), Some(0));
        assert_eq!(plane_lock_axis(GizmoMode::TranslateXZ), Some(1));
        assert_eq!(plane_lock_axis(GizmoMode::TranslateYZ), Some(2));
        assert_eq!(plane_lock_axis(GizmoMode::ScaleXY), Some(0));
        assert_eq!(plane_lock_axis(GizmoMode::TranslateX), None);
    }

    #[test]
    fn solo_gizmo_draws_only_the_active_arrow() {
        let tf = Transform::from_scale_rotation_translation(DVec3::ONE, DQuat::IDENTITY, DVec3::ZERO);
        let view = DMat4::look_at_rh(DVec3::new(0.0, 0.0, 10.0), DVec3::ZERO, DVec3::Y);
        let proj = DMat4::perspective_rh(45.0_f64.to_radians(), 1.0, 0.1, 100.0);
        let mut visuals = GizmoVisuals::default();
        visuals.highlight_color = Some(Color32::from_rgb(240, 225, 80));
        let mut g = Gizmo::new(GizmoConfig {
            view_matrix: view.into(),
            projection_matrix: proj.into(),
            viewport: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)),
            modes: GizmoMode::all_translate(),
            mode_override: Some(GizmoMode::TranslateX),
            pixels_per_point: 1.0,
            visuals,
            ..Default::default()
        });
        g.update(GizmoInteraction { cursor_pos: (400.0, 300.0), hovered: true, drag_started: false, dragging: false }, &[tf]);
        let draw = g.draw();
        assert!(!draw.vertices.is_empty(), "solo gizmo drew nothing");
        for c in &draw.colors {
            if c[3] < 0.1 {
                continue;
            }
            assert!(c[0] > 0.5 && c[1] > 0.4 && c[2] < 0.4, "solo gizmo drew a non-active handle: {c:?}");
        }
    }

    #[test]
    fn previous_state_shows_only_the_dragged_axis() {
        assert_eq!(previous_state_axes(Some(GizmoMode::TranslateX)), vec![0]);
        assert_eq!(previous_state_axes(Some(GizmoMode::TranslateY)), vec![1]);
        assert_eq!(previous_state_axes(Some(GizmoMode::TranslateZ)), vec![2]);
        assert_eq!(previous_state_axes(Some(GizmoMode::TranslateXY)), vec![1, 2]);
        assert_eq!(previous_state_axes(Some(GizmoMode::TranslateYZ)), vec![0, 1]);
        assert_eq!(previous_state_axes(Some(GizmoMode::TranslateView)), vec![0, 1, 2]);
        assert_eq!(previous_state_axes(None), vec![0, 1, 2]);
    }

    #[test]
    fn manipulator_visibility_modifiers() {
        let mut p = crate::prefs::Prefs::default();
        assert!(!manipulator_hidden(&p, true, false, false));
        p.hide_manip_ctrl = true;
        assert!(manipulator_hidden(&p, true, false, false));
        assert!(!manipulator_hidden(&p, false, true, false));
        p.hide_manip_shift = true;
        assert!(manipulator_hidden(&p, false, true, false));
        p.hide_manip_ctrl = false;
        p.hide_manip_shift = false;
        p.hide_manip_ctrl_shift = true;
        assert!(manipulator_hidden(&p, true, true, false));
        assert!(!manipulator_hidden(&p, true, false, false));
        assert!(!manipulator_hidden(&p, true, true, true));
    }

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
    fn negative_axis_arrows_are_pickable() {
        let (cv, rect) = {
            let mut s = Scene::new();
            let id = s.create_mesh("polyCube", &Params::new(), None);
            let mut vp = viewport::Viewport::new("persp");
            vp.rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
            let b = s.world_bbox(&id);
            viewport::frame_box(&mut s, "persp", &b, 800.0 / 600.0);
            (viewport::cam_view(&s, "persp", [800.0, 600.0]), vp.rect)
        };
        let pivot = DVec3::ZERO;
        let (p, dirs) = screen_axes(pivot, DQuat::IDENTITY, &cv, rect).unwrap();
        let size = 75.0;
        let mid = |i: usize| {
            let u = dirs[i].unwrap();
            p - u * (size * 0.6)
        };
        assert_eq!(negative_handle_at(Tool::Move, &[true, false, false], pivot, DQuat::IDENTITY, &cv, rect, mid(0), size), Some(GizmoMode::TranslateX));
        assert_eq!(negative_handle_at(Tool::Scale, &[false, true, false], pivot, DQuat::IDENTITY, &cv, rect, mid(1), size), Some(GizmoMode::ScaleY));
        // hidden handles are not pickable
        assert_eq!(negative_handle_at(Tool::Move, &[false, false, false], pivot, DQuat::IDENTITY, &cv, rect, mid(0), size), None);
        // nor is empty space on the positive side
        let pos = p + dirs[0].unwrap() * (size * 0.6);
        assert_eq!(negative_handle_at(Tool::Move, &[true, true, true], pivot, DQuat::IDENTITY, &cv, rect, pos, size), None);
    }

    #[test]
    fn mmb_rotate_and_scale_use_middle_drag_math() {
        let s2 = 0.5f64.sqrt();
        // rotation: 90 px -> 45 degrees, premultiplied in world space (`quaternion.copy(st.quat).premultiply(q)`)
        let q = js_rotate(DQuat::IDENTITY, GizmoMode::RotateY, 90.0, None, -DVec3::Z);
        assert!((q * DVec3::X - DVec3::new(s2, 0.0, -s2)).length() < 1e-9);
        // discrete 15 degree steps
        let q = js_rotate(DQuat::IDENTITY, GizmoMode::RotateY, 20.0, Some(15.0), -DVec3::Z);
        let a = 15f64.to_radians();
        assert!((q * DVec3::X - DVec3::new(a.cos(), 0.0, -a.sin())).length() < 1e-9);
        // the view handle turns the other way around the camera axis
        let q = js_rotate(DQuat::IDENTITY, GizmoMode::RotateView, 90.0, None, -DVec3::Z);
        assert!((q * DVec3::X - DVec3::new(s2, -s2, 0.0)).length() < 1e-9);
        // scale: exp(dx * 0.01) on the freed axes (transform-gizmo's corner names are swapped)
        let s = js_scale(GizmoMode::ScaleX, 100.0, None);
        assert!((s - DVec3::new(std::f64::consts::E, 1.0, 1.0)).length() < 1e-9, "{s:?}");
        let s = js_scale(GizmoMode::ScaleXY, 100.0, None);
        assert!((s - DVec3::new(1.0, std::f64::consts::E, std::f64::consts::E)).length() < 1e-9, "{s:?}");
        let s = js_scale(GizmoMode::ScaleYZ, 100.0, None);
        assert!((s - DVec3::new(std::f64::consts::E, std::f64::consts::E, 1.0)).length() < 1e-9, "{s:?}");
        // the discrete scale step (JS default 1) rounds the factor and never goes below it
        let dx = 100.0 * 1.24f64.ln();
        assert_eq!(js_scale(GizmoMode::ScaleUniform, dx, Some(1.0)), DVec3::splat(1.0));
        let dx = 100.0 * 1.6f64.ln();
        assert_eq!(js_scale(GizmoMode::ScaleUniform, dx, Some(1.0)), DVec3::splat(2.0));
        assert_eq!(js_scale(GizmoMode::ScaleUniform, -10_000.0, Some(1.0)), DVec3::splat(1.0));
    }

    #[test]
    fn point_snap_projects_onto_the_dragged_handle() {
        let st = DVec3::ZERO;
        let sp = DVec3::new(3.0, 4.0, 5.0);
        let id = DQuat::IDENTITY;
        // single axes: slide the start pivot along that axis
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateX), DVec3::new(3.0, 0.0, 0.0));
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateY), DVec3::new(0.0, 4.0, 0.0));
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateZ), DVec3::new(0.0, 0.0, 5.0));
        // transform-gizmo's corner names: XY keeps X, XZ keeps Y, YZ keeps Z
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateXY), DVec3::new(0.0, 4.0, 5.0));
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateXZ), DVec3::new(3.0, 0.0, 5.0));
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateYZ), DVec3::new(3.0, 4.0, 0.0));
        // the centre handle takes the point itself
        assert_eq!(project_snap_point(sp, st, id, GizmoMode::TranslateView), sp);
        // object-space axes are rotated by the manipulator's start orientation
        let rot = DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2);
        let a = rot * DVec3::Y;
        let p = project_snap_point(sp, st, rot, GizmoMode::TranslateY);
        assert!((p - a * sp.dot(a)).length() < 1e-9);
    }

    #[test]
    fn grid_snap_center_lands_on_the_ground_or_view_plane() {
        let s = Scene::new();
        let cv = viewport::cam_view(&s, "persp", [800.0, 600.0]);
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let p = pos2(400.0, 300.0);
        let st = DVec3::new(1.3, 5.0, -2.6);
        let w = grid_snap_center(&cv, rect, st, p, p, 1.0).unwrap();
        // perspective: the centre handle slides on the ground grid (y = 0)
        assert!(w.y.abs() < 1e-9, "{w:?}");
        assert!((w.x - w.x.round()).abs() < 1e-9 && (w.z - w.z.round()).abs() < 1e-9, "{w:?}");
        // orthographic: the view plane through the start keeps the depth, snaps the other two
        let cv = viewport::cam_view(&s, "front", [800.0, 600.0]);
        assert!(cv.ortho);
        let w = grid_snap_center(&cv, rect, st, p, p, 1.0).unwrap();
        assert!((w.x - 1.0).abs() < 1e-9 && (w.y - 5.0).abs() < 1e-9 && (w.z - st.z).abs() < 1e-9, "{w:?}");
        // moving the mouse keeps the grab offset from where the start projects
        let w2 = grid_snap_center(&cv, rect, st, p + vec2(17.0, 0.0), p, 1.0).unwrap();
        assert_ne!(w2, w);
    }

    #[test]
    fn snap_point_near_has_no_radius_cap_and_includes_curves() {
        use inca_core::curves::Curve;
        let mut s = Scene::new();
        let mesh = s.create_mesh("polyCube", &Params::new(), None);
        let mut vp = viewport::Viewport::new("persp");
        vp.rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let cv = viewport::cam_view(&s, "persp", [800.0, 600.0]);
        let off = pos2(5.0, 5.0);
        let none = HashSet::new();
        let empty = std::collections::HashMap::new();
        // the cube sits near the middle of the view: with a 40 px cap the corner finds nothing,
        // during a drag (Infinity) the nearest vertex always wins
        assert!(snap_point_near(&s, &vp, &none, &cv, off, &none, &empty, 40.0).is_none());
        assert!(snap_point_near(&s, &vp, &none, &cv, off, &none, &empty, f32::INFINITY).is_some());
        // selected objects are excluded in object mode
        let mut excl = HashSet::new();
        excl.insert(mesh.clone());
        assert!(snap_point_near(&s, &vp, &none, &cv, off, &excl, &empty, f32::INFINITY).is_none());
        // every vertex can be skipped (moved components never snap to themselves)
        let verts: HashSet<u32> = (0..s.node(&mesh).unwrap().mesh().unwrap().mesh.v.len() as u32).collect();
        let mut skip = std::collections::HashMap::new();
        skip.insert(mesh, verts);
        assert!(snap_point_near(&s, &vp, &none, &cv, off, &none, &skip, f32::INFINITY).is_none());
        // curve CVs count as snap points too
        let curve = s.create_curve(Curve::new(vec![DVec3::ZERO], 1, "open", "cv"), "curve1");
        let q = cv.project(DVec3::ZERO, vp.rect).unwrap();
        let hit = snap_point_near(&s, &vp, &none, &cv, q, &none, &empty, f32::INFINITY).unwrap();
        assert!(hit.length() < 1e-9, "{hit:?}");
        assert!(s.node(&curve).is_some());
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

    #[test]
    fn uniform_scale_follows_horizontal_drag() {
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
        let drag = |dx: f32| {
            let mut g = Gizmo::default();
            g.update_config(GizmoConfig {
                view_matrix: cv.view.into(),
                projection_matrix: cv.proj.into(),
                viewport: rect,
                modes: GizmoMode::all_scale(),
                mode_override: Some(GizmoMode::ScaleUniform),
                orientation: GizmoOrientation::Local,
                ..Default::default()
            });
            assert!(g.update(GizmoInteraction { cursor_pos: (q.x, q.y), hovered: true, drag_started: true, dragging: true }, std::slice::from_ref(&tf)).is_some());
            let (_, out) = g.update(GizmoInteraction { cursor_pos: (q.x + dx, q.y), hovered: true, drag_started: false, dragging: true }, std::slice::from_ref(&tf)).unwrap();
            out[0].scale
        };
        let right = drag(90.0);
        assert!(right.x > 1.0 && right.x < 2.0, "dragging right must grow gently: {right:?}");
        assert!(
            (right.x - right.y).abs() < 1e-9 && (right.x - right.z).abs() < 1e-9,
            "all three axes must scale together: {right:?}"
        );
        let left = drag(-90.0);
        assert!(left.x > 0.0 && left.x < 1.0, "dragging left must shrink: {left:?}");
        // past the pivot the factor goes negative and the object flips
        let negative = drag(-240.0);
        assert!(negative.x < 0.0, "dragging past zero must go negative: {negative:?}");
        assert!(
            (negative.x - negative.y).abs() < 1e-9 && (negative.x - negative.z).abs() < 1e-9,
            "all three axes must stay uniform: {negative:?}"
        );
    }

    #[test]
    fn plane_scale_crosses_zero() {
        let (cv, rect) = {
            let mut s = Scene::new();
            let id = s.create_mesh("polyCube", &Params::new(), None);
            let mut vp = viewport::Viewport::new("persp");
            vp.rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
            let b = s.world_bbox(&id);
            viewport::frame_box(&mut s, "persp", &b, 800.0 / 600.0);
            (viewport::cam_view(&s, "persp", [800.0, 600.0]), vp.rect)
        };
        let o = cv.project(DVec3::ZERO, rect).unwrap();
        // ScaleXY sits at the X normal (the Y/Z corner); its drag axis is that diagonal.
        let d = (cv.project(DVec3::Y + DVec3::Z, rect).unwrap() - o).normalized();
        let tf = Transform::from_scale_rotation_translation(DVec3::ONE, DQuat::IDENTITY, DVec3::ZERO);
        let mut g = Gizmo::default();
        g.update_config(GizmoConfig {
            view_matrix: cv.view.into(),
            projection_matrix: cv.proj.into(),
            viewport: rect,
            modes: GizmoMode::all_scale(),
            mode_override: Some(GizmoMode::ScaleXY),
            orientation: GizmoOrientation::Local,
            ..Default::default()
        });
        let press = o + d * 30.0;
        assert!(g.update(GizmoInteraction { cursor_pos: (press.x, press.y), hovered: true, drag_started: true, dragging: true }, std::slice::from_ref(&tf)).is_some());
        let drag = o - d * 60.0;
        let (_, out) = g.update(GizmoInteraction { cursor_pos: (drag.x, drag.y), hovered: true, drag_started: false, dragging: true }, std::slice::from_ref(&tf)).unwrap();
        let s = out[0].scale;
        assert!((s.x - 1.0).abs() < 1e-6, "fixed axis must not scale: {s:?}");
        assert!(s.y < 0.0, "free axes must go negative: {s:?}");
        assert!((s.y - s.z).abs() < 1e-6, "both free axes take the same ratio: {s:?}");
    }
}
