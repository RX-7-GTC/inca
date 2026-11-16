//! Floating editors: Script Editor, Hypershade, Graph Editor, Render View / Settings, dialogs.

use crate::app::IncaApp;
use crate::theme;
use egui::{Color32, RichText};
use inca_core::materials::{MAT_TYPES, TEX_TYPES};
use inca_core::render::{RenderScene, RenderSettings};
use inca_core::scene::{MsgKind, Scene};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) enum RenameAction {
    Commit(String),
    Cancel,
}

/// Modal rename prompt: it stays above every window and blocks input to the rest of the UI,
/// like Maya's rename dialogs.
pub(crate) fn rename_prompt(ctx: &egui::Context, id: egui::Id, title: &str, name: &mut String, confirm: &str) -> Option<RenameAction> {
    let mut action = None;
    egui::Modal::new(id).backdrop_color(Color32::from_black_alpha(40)).show(ctx, |ui| {
        ui.set_width(240.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Close").clicked() {
                    action = Some(RenameAction::Cancel);
                }
            });
        });
        let field_id = id.with("name");
        if !ctx.memory(|m| m.has_focus(field_id)) {
            // Start with the caret at the end, even when an earlier rename left a stored caret.
            if let Some(mut st) = egui::TextEdit::load_state(ctx, field_id) {
                st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(name.chars().count()))));
                st.store(ctx, field_id);
            }
        }
        let r = ui.add_sized([240.0, 20.0], egui::TextEdit::singleline(name).id(field_id));
        r.request_focus();
        ui.horizontal(|ui| {
            if ui.button(confirm).clicked() || ((r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                action = Some(RenameAction::Commit(name.trim().to_string()));
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(RenameAction::Cancel);
            }
        });
    });
    action
}

pub struct RenderJob {
    pub cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<(Vec<u8>, u32)>>>,
    alpha: Arc<Mutex<Option<Vec<u8>>>>,
    pub size: [u32; 2],
    pub passes: u32,
    pub total: u32,
    pub started: std::time::Instant,
    /// render time once finished
    pub elapsed: Option<f32>,
    pub tris: usize,
    pub camera: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyClipKind {
    #[default]
    None,
    Graph,
    Timeline,
}

/// Axis locked by the first movement of a Graph Editor MMB scale drag.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ScaleAxis {
    #[default]
    Free,
    Time,
    Value,
}

#[derive(Default)]
pub struct WinState {
    /// dope sheet key drag: (node, time grabbed, current offset)
    pub ds_drag: Option<(Option<String>, f64, f64)>,
    pub job: Option<RenderJob>,
    pub image: Option<egui::TextureHandle>,
    pub pixels: Option<(Vec<u8>, [u32; 2])>,
    pub hs_selected: Option<String>,
    pub ge_curves: std::collections::HashSet<String>,
    /// Selected keys: curve id and key index.
    pub ge_keys: Vec<(String, usize)>,
    pub ge_drag: bool,
    /// Keys captured at drag start: curve, index, original time and value.
    pub ge_drag_keys: Vec<(String, usize, f64, f64)>,
    pub ge_drag_anchor: Option<(f64, f64)>,
    /// Marquee origin in screen coordinates while box-selecting.
    pub ge_box: Option<egui::Pos2>,
    /// Scale-keys tool and drag state.
    pub ge_scale: bool,
    pub ge_scaling: bool,
    pub ge_scale_pivot: Option<(f64, f64)>,
    pub ge_scale_factors: (f64, f64),
    pub ge_scale_axis: ScaleAxis,
    /// Tangent handle being dragged: -1 = in, +1 = out.
    pub ge_tan_drag: Option<i8>,
    /// Copied Graph Editor keys: source curve, original time, key data.
    pub ge_clipboard: Vec<(String, f64, inca_core::anim::Key)>,
    /// Last copy/paste status shown in the Graph Editor.
    pub ge_status: Option<String>,
    /// Set after copying keys so the next frame can put a marker on the OS clipboard.
    pub key_clipboard_marker: bool,
    /// Which key clipboard was copied last.
    pub key_clip_kind: KeyClipKind,
    /// Screen position of the first visible key, used by the in-app UI tests.
    pub ge_first_key_screen: Option<egui::Pos2>,
    /// Pending Graph Editor frame request: 1 = selected, 2 = all.
    pub ge_frame: Option<u8>,
    /// Optional Dope Sheet time view, set by frame actions.
    pub ds_view: Option<(f64, f64)>,
    /// Last drawn Graph Editor window rect, used to activate its hotkey scope.
    pub ge_window_rect: Option<egui::Rect>,
    /// Last drawn Dope Sheet window rect, used to activate its hotkey scope.
    pub ds_window_rect: Option<egui::Rect>,
    /// Docked Time Slider / Range Slider rect, used to activate the Timeline scope.
    pub timeline_rect: Option<egui::Rect>,
    /// Windows already seen open last frame, windows seen this frame, and those needing a recenter.
    pub window_seen: std::collections::HashSet<egui::Id>,
    pub window_seen_now: std::collections::HashSet<egui::Id>,
    pub window_recenter: std::collections::HashSet<egui::Id>,
    /// Last rect each floating window had while open, keyed by egui id value so it can be saved.
    pub window_last_rect: std::collections::HashMap<u64, egui::Rect>,
    pub rv: crate::render_view::RvState,
    pub hs: crate::hypershade::HsState,
}

impl WinState {
    pub fn start_render(&mut self, s: &Scene, camera: &str, _frame: u32, smooth_divisions: u32) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
        let mut settings = RenderSettings::from_scene(s);
        settings.smooth_divisions = smooth_divisions.clamp(1, 4);
        let size = [settings.width.max(1), settings.height.max(1)];
        let total = settings.samples.max(1);
        let rs = Arc::new(RenderScene::build(s, camera, settings));
        let tris = rs.triangle_count();
        let cancel = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(None));
        let alpha = Arc::new(Mutex::new(None));
        let (c2, l2, a2) = (cancel.clone(), latest.clone(), alpha.clone());
        std::thread::spawn(move || {
            *a2.lock().unwrap() = Some(rs.coverage());
            let mut acc = vec![0f32; (size[0] * size[1] * 3) as usize];
            for pass in 0..total {
                if c2.load(Ordering::Relaxed) {
                    return;
                }
                rs.render_pass(&mut acc, pass);
                let px = rs.resolve(&acc, pass + 1);
                *l2.lock().unwrap() = Some((px, pass + 1));
            }
        });
        self.rv.alpha = None;
        self.job = Some(RenderJob { cancel, latest, alpha, size, passes: 0, total, started: std::time::Instant::now(), elapsed: None, tris, camera: camera.into() });
    }

    /// Show a finished image that did not come from the path tracer (the hardware renderer).
    pub fn set_image(&mut self, px: Vec<u8>, alpha: Vec<u8>, size: [u32; 2], camera: &str, tris: usize) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
        self.pixels = Some((px, size));
        self.rv.alpha = Some(alpha);
        self.rv.dirty = true;
        self.job = Some(RenderJob { cancel: Arc::new(AtomicBool::new(true)), latest: Arc::new(Mutex::new(None)), alpha: Arc::new(Mutex::new(None)), size, passes: 1, total: 1, started: std::time::Instant::now(), elapsed: Some(0.0), tris, camera: camera.into() });
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(j) = &mut self.job else { return };
        if let Some(a) = j.alpha.lock().unwrap().take() {
            self.rv.alpha = Some(a);
            self.rv.dirty = true;
        }
        let got = j.latest.lock().unwrap().take();
        if let Some((px, n)) = got {
            j.passes = n;
            if n >= j.total {
                j.elapsed = Some(j.started.elapsed().as_secs_f32());
            }
            self.pixels = Some((px, j.size));
            self.rv.dirty = true;
        }
        if j.passes < j.total {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

/// Help > Getting Started / Mouse & Navigation (the JS HELP_HTML); *bold* runs are keys.
const HELP: &[(&str, &[&str])] = &[
    ("Navigating the viewport", &["*Alt + Left drag* tumble · *Alt + Middle drag* track (pan) · *Alt + Right drag* or *wheel* dolly (zoom)", "*F* frame selection · *A* frame all · *Space* (tap) toggles single/four view · *Space* (hold) shows the hotbox"]),
    ("Selecting & transforming", &["*Q* select · *W* move · *E* rotate · *R* scale · *Y* last tool · *D*/*Insert* edit pivot · *+/-* manipulator size", "Click to select, *Shift* toggle, *Ctrl* deselect, *Ctrl+Shift* add, drag for marquee. Middle-drag moves along the last used axis.", "Hold *X* to snap to grid, *V* to snap to points, *J* for step rotation."]),
    ("Components", &["*Right-click* an object for its marking menu (Vertex / Edge / Face / Object Mode). *F8* object/component, *F9* vertex, *F10* edge, *F11* face. *Shift+Right-click* gives modeling tools. Double-click an edge to select its loop.", "*Ctrl+E* extrude · *Ctrl+B* bevel · *Ctrl+D* duplicate · *Ctrl+G* group · *P* parent · *B* soft select · *1/2/3* smooth preview"]),
    ("Display", &["*4* wireframe · *5* shaded · *6* textured · *7* use all lights · *Alt+A* X-ray · *Ctrl+1* isolate select · *Ctrl+H* hide"]),
    ("Animation", &["*S* set key · *Shift+W/E/R* key translate/rotate/scale · *Alt+V* play · *Esc* stop · *,*/*.* previous/next key · *Alt+,*/*Alt+.* previous/next frame"]),
    ("Files", &["Set a project first (File > Project Window) so scenes, textures and renders are organised in the standard folders."]),
];

fn help_line(ui: &mut egui::Ui, l: &str) {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(13.0);
    for (i, part) in l.split('*').enumerate() {
        let color = if i % 2 == 1 { Color32::WHITE } else { theme::TEXT_DIM };
        job.append(part, 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
    }
    job.wrap.max_width = ui.available_width();
    ui.label(job);
}

/// A window with less than 10% of its area visible, or with an unreachable title bar, is lost.
fn window_reachable(ctx: &egui::Context, rect: egui::Rect) -> bool {
    let content = ctx.content_rect();
    let inter = content.intersect(rect);
    let area = rect.width().max(0.0) * rect.height().max(0.0);
    let visible = inter.width().max(0.0) * inter.height().max(0.0);
    let title = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + 26.0));
    area > 0.0 && visible >= area * 0.10 && content.intersect(title).width() > 0.0
}

fn centered_window_pos(ctx: &egui::Context, size: egui::Vec2) -> egui::Pos2 {
    let content = ctx.content_rect();
    let p = content.center() - size * 0.5;
    egui::pos2(p.x.max(content.left()), p.y.max(content.top()))
}

/// Serialize the remembered window rects for the app storage.
pub(crate) fn encode_window_rects(map: &std::collections::HashMap<u64, egui::Rect>) -> Vec<(u64, [f32; 4])> {
    map.iter().map(|(k, r)| (*k, [r.min.x, r.min.y, r.width(), r.height()])).collect()
}

/// Restore the remembered window rects from the app storage.
pub(crate) fn decode_window_rects(list: Vec<(u64, [f32; 4])>) -> std::collections::HashMap<u64, egui::Rect> {
    list.into_iter().map(|(k, r)| (k, egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3])))).collect()
}

/// Show a floating window and repair an off-screen saved/auto position. A window that opens
/// outside the viewport is recentered on the next frame; reopening a lost window recenters it
/// immediately.
pub(crate) fn place_window<R>(
    app: &mut IncaApp,
    ctx: &egui::Context,
    id: egui::Id,
    open: bool,
    show: impl FnOnce(&mut IncaApp, Option<egui::Pos2>) -> Option<egui::InnerResponse<Option<R>>>,
) -> Option<egui::InnerResponse<Option<R>>> {
    let first = open && !app.windows_state.window_seen.contains(&id);
    if open {
        app.windows_state.window_seen_now.insert(id);
    }
    let key = id.value();
    let persisted = ctx.memory(|m| m.area_rect(id));
    let saved = persisted.filter(|r| r.width() > 1.0 && r.height() > 1.0);
    let last = app.windows_state.window_last_rect.get(&key).copied().or(saved);
    let mut fix = None;
    if first {
        if let Some(rect) = last {
            if window_reachable(ctx, rect) {
                fix = Some(rect.min);
            } else {
                fix = Some(centered_window_pos(ctx, rect.size()));
            }
        }
    }
    // A legacy egui position has no size. Center it once on first open instead of placing its
    // top-left at the old, possibly off-screen, position.
    let stale_pos = first && last.is_none() && persisted.is_some();
    if app.windows_state.window_recenter.remove(&id) {
        let size = last.map_or(egui::vec2(480.0, 360.0), |r| r.size());
        fix = Some(centered_window_pos(ctx, size));
    }
    let resp = show(app, fix);
    if open {
        if let Some(r) = &resp {
            let rect = r.response.rect;
            app.windows_state.window_last_rect.insert(key, rect);
            if first && (stale_pos || !window_reachable(ctx, rect)) {
                app.windows_state.window_recenter.insert(id);
                ctx.request_repaint();
            }
        }
    }
    resp
}

pub fn show(app: &mut IncaApp, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    if app.playblast.running.is_some() {
        // floating windows would end up in the captured frames
        return;
    }
    crate::rig_tools::sdk_window(app, ctx);
    crate::anim_extras::window(app, ctx);
    let mut open = app.win.help;
    let is_open = open;
    let _ = place_window(app, ctx, egui::Id::new("Getting Started with Inca"), is_open, |_app, fix| {
        let mut window = egui::Window::new("Getting Started with Inca").open(&mut open).default_size([600.0, 520.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (title, lines) in HELP {
                    theme::section_header(ui, title);
                    for l in *lines {
                        help_line(ui, l);
                    }
                    ui.add_space(4.0);
                }
            });
        })
    });
    app.win.help = open;
    let mut open = app.win.assign_material;
    let is_open = open;
    let mut pick: Option<&str> = None;
    let _ = place_window(app, ctx, egui::Id::new("Assign New Material"), is_open, |_app, fix| {
        let mut window = egui::Window::new("Assign New Material").open(&mut open).default_size([260.0, 300.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
            ui.label(RichText::new("Assign a new material to the selection:").color(theme::TEXT_DIM));
            for t in MAT_TYPES {
                if ui.add(egui::Button::new(t.label).frame(false)).on_hover_text(format!("Assign a new {} to the selection", t.label)).clicked() {
                    pick = Some(t.ty);
                }
            }
        })
    });
    if let Some(ty) = pick {
        let _ = app.run_mel("assign new material", &format!("string $m = `shadingNode -asShader {ty}`; hyperShade -assign $m;"));
        open = false;
    }
    app.win.assign_material = open;
    let mut open = app.win.script_editor;
    let is_open = open;
    let _ = place_window(app, ctx, egui::Id::new("Script Editor"), is_open, |app, fix| {
        let mut window = egui::Window::new("Script Editor").open(&mut open).default_size([640.0, 460.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| script_editor(app, ui))
    });
    app.win.script_editor = open;
    let mut open = app.win.hypershade;
    let is_open = open;
    // A distinct id discards the old stored area state that had grown to the screen width
    // (the layout feedback bug); the window now starts at its default size again.
    let _ = place_window(app, ctx, egui::Id::new("HypershadeWindow"), is_open, |app, fix| {
        let mut window = egui::Window::new("Hypershade").id(egui::Id::new("HypershadeWindow")).open(&mut open).default_size([980.0, 560.0]).min_size([600.0, 420.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| crate::hypershade::window(app, ui))
    });
    // File > Close sets the flag from inside the window; the X button clears `open`.
    app.win.hypershade = open && app.win.hypershade;
    if app.win.graph_editor {
        let mut open = true;
        let is_open = open;
        let resp = place_window(app, ctx, egui::Id::new("Graph Editor"), is_open, |app, fix| {
            let mut window = egui::Window::new("Graph Editor").open(&mut open).default_pos(egui::pos2(60.0, 80.0)).default_size([760.0, 420.0]).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| graph_editor(app, ui))
        });
        app.windows_state.ge_window_rect = resp.map(|r| r.response.rect);
        app.win.graph_editor = open;
    } else {
        app.windows_state.ge_window_rect = None;
        app.windows_state.ge_frame = None;
        app.windows_state.ge_status = None;
    }
    if app.win.dope_sheet {
        let mut open = true;
        let is_open = open;
        let resp = place_window(app, ctx, egui::Id::new("Dope Sheet"), is_open, |app, fix| {
            let mut window = egui::Window::new("Dope Sheet").open(&mut open).default_pos(egui::pos2(820.0, 80.0)).default_size([760.0, 260.0]).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| dope_sheet(app, ui))
        });
        app.windows_state.ds_window_rect = resp.map(|r| r.response.rect);
        app.win.dope_sheet = open;
    } else {
        app.windows_state.ds_window_rect = None;
    }
    if app.win.channel_control {
        let mut open = true;
        let _ = place_window(app, ctx, egui::Id::new("Channel Control"), true, |app, fix| {
            let mut window = egui::Window::new("Channel Control").open(&mut open).default_size([420.0, 420.0]).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| crate::panels::channel_control_window(app, ui))
        });
        app.win.channel_control = open && app.win.channel_control;
    }
    if app.win.delete_attributes {
        let mut open = true;
        let _ = place_window(app, ctx, egui::Id::new("Delete Attributes"), true, |app, fix| {
            let mut window = egui::Window::new("Delete Attributes").open(&mut open).default_size([300.0, 360.0]).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| crate::panels::delete_attributes_window(app, ui))
        });
        app.win.delete_attributes = open && app.win.delete_attributes;
    }
    if app.win.edit_attributes {
        let mut open = true;
        let _ = place_window(app, ctx, egui::Id::new("Edit Attribute"), true, |app, fix| {
            let mut window = egui::Window::new("Edit Attribute").open(&mut open).default_size([320.0, 220.0]).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| crate::panels::edit_attributes_window(app, ui))
        });
        app.win.edit_attributes = open && app.win.edit_attributes;
    }
    if app.win.expression_editor {
        let mut open = true;
        let _ = place_window(app, ctx, egui::Id::new("Expression Editor"), true, |app, fix| {
            let mut window = egui::Window::new("Expression Editor").open(&mut open).default_size([420.0, 260.0]).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| crate::panels::expression_editor_window(app, ui))
        });
        app.win.expression_editor = open && app.win.expression_editor;
    }
    let mut open = app.win.render_view;
    let is_open = open;
    let _ = place_window(app, ctx, egui::Id::new("Render View"), is_open, |app, fix| {
        let mut window = egui::Window::new("Render View").open(&mut open).default_size([700.0, 560.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| crate::render_view::render_view(app, ui))
    });
    app.win.render_view = open;
    let mut open = app.win.render_settings;
    let is_open = open;
    let _ = place_window(app, ctx, egui::Id::new("Render Settings"), is_open, |app, fix| {
        let mut window = egui::Window::new("Render Settings").open(&mut open).default_size([340.0, 260.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| render_settings(app, ui))
    });
    app.win.render_settings = open;
    crate::prefs::window(app, ctx);
    if let Some(msg) = app.win.backend_notice.clone() {
        let mut close = false;
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(12));
        egui::Modal::new(egui::Id::new("backend-notice")).frame(frame).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new("Rendering backend").strong());
            ui.add_space(6.0);
            ui.label(msg);
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
        });
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.win.backend_notice = None;
        }
    }
    if app.panels.quit_confirm {
        let mut decided = None;
        egui::Window::new("Quit Inca").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("Save changes to the scene before quitting?");
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    decided = Some(1);
                }
                if ui.button("Don't Save").clicked() {
                    decided = Some(2);
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(0);
                }
            });
        });
        match decided {
            Some(1) => {
                app.panels.quit_confirm = false;
                app.action("saveScene");
                app.panels.quit = !app.s.modified;
            }
            Some(2) => {
                app.panels.quit_confirm = false;
                app.panels.quit = true;
            }
            Some(_) => app.panels.quit_confirm = false,
            None => {}
        }
    }
    crate::hotkey_editor::window(app, ctx);
    crate::component_editor::window(app, ctx);
    crate::toolbar::shelf_editor(app, ctx);
    crate::hypergraph::window(app, ctx);
    crate::hypergraph::connection_editor(app, ctx);
    crate::hik_ui::window(app, ctx);
    let mut open = app.win.command_list;
    let is_open = open;
    let _ = place_window(app, ctx, egui::Id::new("MEL Command Reference"), is_open, |_app, fix| {
        let mut window = egui::Window::new("MEL Command Reference").open(&mut open).default_size([380.0, 480.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
            ui.label("Commands available to MEL scripts, the command line and the Script Editor:");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for g in inca_core::primitives::GEN_INFO {
                    let flags: Vec<String> = g.short.iter().map(|(s, l)| format!("-{s} ({l})")).collect();
                    ui.label(RichText::new(g.ty).monospace().strong()).on_hover_text(flags.join("\n"));
                }
                for c in inca_core::commands::CORE_COMMANDS {
                    ui.label(RichText::new(*c).monospace());
                }
                ui.separator();
                ui.label("Language: int/float/string/vector variables and arrays, procs, if/else, for, for-in, while, do-while, switch, backtick command capture, print, size, sin/cos/..., tokenize, substring, gmatch ...");
            });
        })
    });
    app.win.command_list = open;
    let mut open = app.win.about;
    let is_open = open;
    let _ = place_window(app, ctx, egui::Id::new("About Inca"), is_open, |_app, fix| {
        let mut window = egui::Window::new("About Inca").open(&mut open).collapsible(false).resizable(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
            ui.heading("Inca");
            ui.label(format!("version {}", env!("CARGO_PKG_VERSION")));
            ui.label("A free, open-source 3D modeling, animation and rendering application.");
            ui.label("Built with egui/eframe, wgpu, glam, transform-gizmo, boolmesh, tobj, gltf, stl_io, rayon and rhai.");
            ui.hyperlink("https://incacad.com");
        })
    });
    app.win.about = open;
    // rename dialog
    if let Some(mut name) = app.win.rename.take() {
        match rename_prompt(ctx, egui::Id::new("win_rename"), "Rename", &mut name, "Rename") {
            Some(RenameAction::Commit(new)) => {
                let _ = app.run_mel("rename", &format!("rename \"{}\";", new));
            }
            Some(RenameAction::Cancel) => {}
            None => app.win.rename = Some(name),
        }
    }
    floating(app, ctx);
}

/// Main panels undocked from the frame, shown as floating windows. Closing the window or
/// clicking Dock returns the panel to the main layout.
fn floating(app: &mut IncaApp, ctx: &egui::Context) {
    const PANELS: &[(&str, &str, [f32; 2], [f32; 2])] = &[
        ("shelf", "Shelf", [300.0, 120.0], [720.0, 96.0]),
        ("toolbox", "Tool Box", [300.0, 240.0], [64.0, 420.0]),
        ("outliner", "Outliner", [380.0, 240.0], [300.0, 420.0]),
        ("rightpanel", "Attribute Editor / Channel Box", [800.0, 180.0], [380.0, 620.0]),
        ("timeslider", "Time Slider", [300.0, 700.0], [720.0, 110.0]),
        ("rangeslider", "Range Slider", [300.0, 590.0], [900.0, 120.0]),
        ("cmdline", "Command Line", [300.0, 810.0], [720.0, 110.0]),
    ];
    let mut dock = None;
    for &(id, title, pos, size) in PANELS {
        let visible = match id {
            "outliner" => app.show_outliner,
            "rightpanel" => app.show_right,
            _ => app.shown(id),
        };
        if !visible {
            app.detached.remove(id);
            continue;
        }
        if !app.detached.contains(id) {
            continue;
        }
        let mut open = true;
        let gen = app.detach_gen.get(id).copied().unwrap_or(0);
        let pos = app.detach_pos.get(id).copied().unwrap_or(egui::pos2(pos[0], pos[1]));
        let mut window = egui::Window::new(title)
            // v3: discard any size persisted before the range-bar width feedback fix.
            .id(egui::Id::new(("floating_panel_v3", id, gen)))
            .default_pos(pos)
            .default_size(size)
            .resizable(true)
            .constrain(false)
            .open(&mut open);
        if id == "rightpanel" {
            window = window.min_width(crate::app::right_panel_min_width(app.right_tab));
        }
        if app.drag_panel == Some(id) {
            if let Some(p) = ctx.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos())) {
                window = window.fixed_pos(p);
            }
            if !ctx.input(|i| i.pointer.primary_down()) {
                app.drag_panel = None;
            }
        }
        window.show(ctx, |ui| {
            if ui.button("Dock").clicked() {
                dock = Some(id);
            }
            ui.separator();
            match id {
                "shelf" => crate::toolbar::shelf(app, ui),
                "toolbox" => crate::toolbar::tool_box(app, ui),
                "outliner" => crate::panels::outliner(app, ui),
                "rightpanel" => crate::panels::right_panel(app, ui),
                "timeslider" => crate::timeline::time_slider(app, ui),
                "rangeslider" => crate::timeline::range_slider(app, ui),
                "cmdline" => crate::timeline::command_line(app, ui),
                _ => {}
            }
        });
        if !open {
            dock = Some(id);
        }
    }
    if let Some(id) = dock {
        app.detached.remove(id);
    }
}

// ------------------------------------------------------------------ script editor

fn script_editor(app: &mut IncaApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if ui.selectable_label(!app.script_lang_rhai, "MEL").clicked() {
            app.script_lang_rhai = false;
        }
        if ui.selectable_label(app.script_lang_rhai, "Rhai").on_hover_text("Rhai scripting: mel(\"polyCube\"), cmd(\"move\", [1, 2, 3]), getAttr(\"pCube1.tx\"), ls()").clicked() {
            app.script_lang_rhai = true;
        }
        ui.separator();
        if ui.button("▶ Execute").on_hover_text("Ctrl+Enter").clicked() {
            run_script(app);
        }
        if ui.button("Clear History").clicked() {
            app.script_log.clear();
        }
        if !app.script_lang_rhai && ui.button("Save to Shelf").on_hover_text("Add the script as a button on the Custom shelf").clicked() {
            let src = app.script_text.trim().to_string();
            if !src.is_empty() {
                let l: String = src.lines().find(|l| !l.trim_start().starts_with("//")).unwrap_or("script").split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("script").chars().take(8).collect();
                if app.shelves.is_empty() {
                    app.shelves = crate::toolbar::default_shelves(&[]);
                }
                let i = app.shelves.iter().position(|s| s.name == "Custom").unwrap_or(app.shelves.len() - 1);
                app.shelves[i].items.push(crate::toolbar::ShelfEntry { mel: Some(src), label: Some(l), ..Default::default() });
            }
        }
        if ui.button("Load...").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("Scripts", &["mel", "rhai", "txt"]).pick_file() {
                if let Ok(t) = std::fs::read_to_string(&p) {
                    app.script_lang_rhai = p.extension().is_some_and(|e| e == "rhai");
                    app.script_text = t;
                }
            }
        }
        if ui.button("Save...").clicked() {
            if let Some(p) = rfd::FileDialog::new().set_file_name(if app.script_lang_rhai { "script.rhai" } else { "script.mel" }).save_file() {
                let _ = std::fs::write(p, &app.script_text);
            }
        }
    });
    let h = ui.available_height();
    egui::Frame::NONE.fill(theme::BG_DARK).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("selog").max_height(h * 0.55).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            for (k, l) in &app.script_log {
                let c = match k {
                    MsgKind::Error => Color32::from_rgb(240, 110, 110),
                    MsgKind::Warning => Color32::from_rgb(240, 200, 90),
                    MsgKind::Result => Color32::from_rgb(160, 200, 255),
                    MsgKind::Info => Color32::from_gray(230),
                    MsgKind::Echo => theme::TEXT,
                };
                ui.label(RichText::new(l).monospace().color(c));
            }
        });
    });
    ui.separator();
    let r = egui::ScrollArea::vertical()
        .id_salt("seinput")
        .show(ui, |ui| ui.add(egui::TextEdit::multiline(&mut app.script_text).code_editor().desired_rows(10).desired_width(f32::INFINITY)))
        .inner;
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && (i.modifiers.ctrl || i.modifiers.command)) {
        run_script(app);
    }
}

fn run_script(app: &mut IncaApp) {
    let src = app.script_text.clone();
    if app.script_lang_rhai {
        app.s.checkpoint("rhai script");
        match crate::scripting::run_rhai(app, &src) {
            Ok(r) => {
                if !r.is_empty() {
                    app.script_log.push((MsgKind::Result, format!("// Result: {r}")));
                }
            }
            Err(e) => app.script_log.push((MsgKind::Error, format!("// Error: {e}"))),
        }
    } else {
        let _ = app.run_mel("script", &src);
    }
}

// ------------------------------------------------------------------ hypershade

pub fn material_editor(app: &mut IncaApp, ui: &mut egui::Ui, mid: &str, show_name: bool) {
    let Some(m) = app.s.mats.get(mid).cloned() else { return };
    let read_only = crate::hypershade::read_only(app, mid);
    if show_name {
    ui.horizontal(|ui| {
        let p = m.pbr();
        let (r, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 13.0, Color32::from(egui::Rgba::from_rgb(p.base[0], p.base[1], p.base[2])));
        let mut name = m.name.clone();
        let resp = ui.text_edit_singleline(&mut name);
        let committed = resp.lost_focus() || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if committed && name != m.name && !name.trim().is_empty() {
            if read_only {
                app.error("line 1: Cannot rename a read only node.");
            } else {
                app.s.checkpoint("rename material");
                let old = m.name.clone();
                app.s.release_name(&old);
                let nn = app.s.unique_name(name.trim());
                app.s.mats.get_mut(mid).unwrap().name = nn;
                app.s.touch();
            }
        }
        ui.label(RichText::new(inca_core::materials::mat_type(&m.ty).label).color(theme::TEXT_DIM));
    });
    }
    crate::panels::material_fields_editor(app, ui, mid, &m.ty);
    theme::section_header(ui, "Texture Maps");
    for slot in inca_core::materials::mat_type(&m.ty).mappable {
        ui.horizontal(|ui| {
            ui.add_sized([108.0, 18.0], egui::Label::new(inca_core::ops::nice_name(slot)).truncate());
            let cur = m.maps.get(*slot).and_then(|v| v.as_str()).map(String::from);
            let cur_name = cur.as_ref().and_then(|t| app.s.texs.get(t)).map(|t| t.name.clone()).unwrap_or("None".into());
            egui::ComboBox::from_id_salt(("map", mid, *slot)).width(120.0).selected_text(cur_name).show_ui(ui, |ui| {
                if ui.selectable_label(cur.is_none(), "None").clicked() {
                    app.s.checkpoint("disconnect texture");
                    app.s.mats.get_mut(mid).unwrap().clear_map(slot);
                    app.s.touch();
                }
                for t in TEX_TYPES {
                    if ui.button(format!("New {}", t.label)).clicked() {
                        app.s.checkpoint("connect texture");
                        let tid = app.s.create_texture(t.ty, None);
                        crate::hypershade::place_new_node(app, &tid, None);
                        app.s.mats.get_mut(mid).unwrap().set_map(slot, &tid, "outColor");
                        app.s.touch();
                    }
                }
                let texs: Vec<(String, String)> = app.s.texs.values().map(|t| (t.id.clone(), t.name.clone())).collect();
                for (tid, tn) in texs {
                    if ui.selectable_label(cur.as_deref() == Some(tid.as_str()), &tn).clicked() {
                        app.s.checkpoint("connect texture");
                        app.s.mats.get_mut(mid).unwrap().set_map(slot, &tid, "outColor");
                        app.s.touch();
                    }
                }
            });
        });
    }
}

// ------------------------------------------------------------------ graph editor

/// Delete all keys selected in the Graph Editor.
pub(crate) fn delete_selected_keys(app: &mut IncaApp) {
    let selected = std::mem::take(&mut app.windows_state.ge_keys);
    if selected.is_empty() {
        return;
    }
    app.s.checkpoint("delete keys");
    let mut by_curve: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (ck, ki) in selected {
        by_curve.entry(ck).or_default().push(ki);
    }
    for (ck, mut indices) in by_curve {
        indices.sort_unstable();
        indices.dedup();
        if let Some(c) = app.s.anim.curves.get_mut(&ck) {
            for &i in indices.iter().rev() {
                if i < c.keys.len() {
                    c.keys.remove(i);
                }
            }
        }
    }
    app.s.anim.curves.retain(|_, c| !c.keys.is_empty());
    app.s.apply_anim();
    app.s.touch();
    app.windows_state.ge_drag = false;
    app.windows_state.ge_drag_keys.clear();
    app.windows_state.ge_box = None;
}

/// Copy the selected Graph Editor keys.
pub(crate) fn copy_selected_keys(app: &mut IncaApp) {
    let selected = app.windows_state.ge_keys.clone();
    let mut clip = Vec::new();
    for (ck, ki) in selected {
        if let Some(key) = app.s.anim.curves.get(&ck).and_then(|c| c.keys.get(ki)) {
            clip.push((ck, key.t, key.clone()));
        }
    }
    if clip.is_empty() {
        return;
    }
    let n = clip.len();
    app.windows_state.ge_clipboard = clip;
    app.windows_state.key_clipboard_marker = true;
    app.windows_state.key_clip_kind = KeyClipKind::Graph;
    let msg = format!("Copied {n} key{}", if n == 1 { "" } else { "s" });
    app.windows_state.ge_status = Some(msg.clone());
    app.help = msg;
}

/// Paste copied Graph Editor keys at the current time, preserving their relative spacing.
pub(crate) fn paste_keys(app: &mut IncaApp) {
    let clip = app.windows_state.ge_clipboard.clone();
    if clip.is_empty() {
        return;
    }
    let valid: Vec<(String, f64, inca_core::anim::Key)> = clip.into_iter().filter(|(ck, _, _)| app.s.anim.curves.contains_key(ck)).collect();
    if valid.is_empty() {
        return;
    }
    let first = valid.iter().map(|(_, t, _)| *t).fold(f64::INFINITY, f64::min);
    let offset = app.s.time.current - first;
    app.s.checkpoint("paste keys");
    let mut pasted: Vec<(String, f64)> = Vec::new();
    for (ck, t, key) in &valid {
        let new_t = *t + offset;
        if let Some(c) = app.s.anim.curves.get_mut(ck) {
            let mut k = key.clone();
            k.t = new_t;
            if let Some(i) = c.find(new_t) {
                c.keys[i] = k;
            } else {
                c.keys.push(k);
                c.sort();
            }
            pasted.push((ck.clone(), new_t));
        }
    }
    let mut selected = Vec::new();
    for (ck, t) in &pasted {
        if let Some(c) = app.s.anim.curves.get(ck) {
            if let Some(i) = c.find(*t) {
                selected.push((ck.clone(), i));
            }
        }
    }
    app.windows_state.ge_keys = selected;
    app.s.apply_anim();
    app.s.touch();
    let n = pasted.len();
    let msg = format!("Pasted {n} key{}", if n == 1 { "" } else { "s" });
    app.windows_state.ge_status = Some(msg.clone());
    app.help = msg;
}

/// Delete keys at the current time for the selected objects (Dope Sheet).
pub(crate) fn delete_keys_at_current_time(app: &mut IncaApp) {
    let names: Vec<String> = app.s.sel.iter().map(|n| app.s.name_of(n)).collect();
    if names.is_empty() {
        return;
    }
    let t = app.s.time.current;
    let _ = app.run_mel("cut keys", &format!("cutKey -t {t} {};", names.join(" ")));
}

/// Frame the Dope Sheet: selected nodes' keys (F) or every key in the scene (A).
pub(crate) fn dope_frame(app: &mut IncaApp, selected_only: bool) {
    let mut times: Vec<f64> = if selected_only {
        app.s.anim.key_times(&app.s.sel)
    } else {
        app.s.anim.curves.values().flat_map(|c| c.keys.iter().map(|k| k.t)).collect()
    };
    times.retain(|t| t.is_finite());
    if times.is_empty() {
        app.windows_state.ds_view = None;
        return;
    }
    let lo = times.iter().copied().fold(f64::INFINITY, f64::min) - 1.0;
    let hi = times.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 1.0;
    app.windows_state.ds_view = Some((lo, hi.max(lo + 1.0)));
}

fn ge_selected(app: &IncaApp, ck: &str, ki: usize) -> bool {
    app.windows_state.ge_keys.iter().any(|(k, i)| k == ck && *i == ki)
}

fn ge_select_only(app: &mut IncaApp, ck: &str, ki: usize) {
    app.windows_state.ge_keys.clear();
    app.windows_state.ge_keys.push((ck.to_string(), ki));
}

fn ge_toggle(app: &mut IncaApp, ck: &str, ki: usize) {
    if let Some(pos) = app.windows_state.ge_keys.iter().position(|(k, i)| k == ck && *i == ki) {
        app.windows_state.ge_keys.remove(pos);
    } else {
        app.windows_state.ge_keys.push((ck.to_string(), ki));
    }
}

fn nearest_graph_key(app: &IncaApp, shown: &[String], tf: &egui_plot::PlotTransform, pp: egui::Pos2) -> Option<(String, usize)> {
    let mut best: Option<(f32, String, usize)> = None;
    for k in shown {
        for (i, key) in app.s.anim.curves[k].keys.iter().enumerate() {
            let sp = tf.position_from_point(&egui_plot::PlotPoint::new(key.t, key.v));
            let d = sp.distance(pp);
            if d < 10.0 && best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, k.clone(), i));
            }
        }
    }
    best.map(|b| (b.1, b.2))
}

/// Screen positions of the selected key and its in/out tangent handles.
fn graph_tangent_handles(app: &IncaApp, tf: &egui_plot::PlotTransform, selected: &[(String, usize)]) -> Option<(egui::Pos2, egui::Pos2, egui::Pos2)> {
    if selected.len() != 1 {
        return None;
    }
    let (ck, ki) = &selected[0];
    let c = app.s.anim.curves.get(ck)?;
    let key = c.keys.get(*ki)?;
    let kp = tf.position_from_point(&egui_plot::PlotPoint::new(key.t, key.v));
    let left = tf.value_from_position(kp - egui::vec2(36.0, 0.0));
    let right = tf.value_from_position(kp + egui::vec2(36.0, 0.0));
    let in_dx = (key.t - left.x).abs().max(1e-6);
    let out_dx = (right.x - key.t).abs().max(1e-6);
    let in_slope = if key.it == "fixed" { key.is } else { (key.v - c.eval(key.t - 0.01)) / 0.01 };
    let out_slope = if key.ot == "fixed" { key.os } else { (c.eval(key.t + 0.01) - key.v) / 0.01 };
    let ip = tf.position_from_point(&egui_plot::PlotPoint::new(key.t - in_dx, key.v - in_slope * in_dx));
    let op = tf.position_from_point(&egui_plot::PlotPoint::new(key.t + out_dx, key.v + out_slope * out_dx));
    Some((kp, ip, op))
}

/// Drag distance, in points, that takes the scale factor from 1 to 0 (and on to -1).
const GRAPH_SCALE_DRAG: f64 = 120.0;
/// Movement needed before the scale drag locks to the dominant axis.
const GRAPH_SCALE_AXIS_LOCK: f64 = 4.0;

/// Ends an interactive scale: sort the curves and remap the selection to the new key indices.
pub(crate) fn finish_graph_scale(app: &mut IncaApp) {
    if !app.windows_state.ge_scaling {
        return;
    }
    let (sx, sy) = app.windows_state.ge_scale_factors;
    let pivot = app.windows_state.ge_scale_pivot;
    let keys = app.windows_state.ge_drag_keys.clone();
    let affected: std::collections::HashSet<String> = keys.iter().map(|(ck, _, _, _)| ck.clone()).collect();
    for ck in &affected {
        if let Some(c) = app.s.anim.curves.get_mut(ck) {
            c.sort();
        }
    }
    let mut selected = Vec::new();
    if let Some((pt, pv)) = pivot {
        for (ck, _, t0, v0) in &keys {
            let nt = pt + (t0 - pt) * sx;
            let nv = pv + (v0 - pv) * sy;
            if let Some(c) = app.s.anim.curves.get(ck) {
                // Match time and value: keys can share a time after collapsing onto the pivot.
                if let Some(i) = c.keys.iter().position(|k| (k.t - nt).abs() < 1e-4 && (k.v - nv).abs() < 1e-4) {
                    selected.push((ck.clone(), i));
                }
            }
        }
    }
    selected.sort();
    selected.dedup();
    app.windows_state.ge_keys = selected;
    app.s.apply_anim();
    app.s.touch();
    app.windows_state.ge_scaling = false;
    app.windows_state.ge_scale_pivot = None;
    app.windows_state.ge_scale_factors = (1.0, 1.0);
    app.windows_state.ge_scale_axis = ScaleAxis::Free;
    app.windows_state.ge_drag_anchor = None;
    app.windows_state.ge_drag_keys.clear();
}

fn graph_editor(app: &mut IncaApp, ui: &mut egui::Ui) {
    use egui_plot::{Line, Plot, PlotPoints, Points};
    if (ui.input(|i| i.modifiers.ctrl || i.modifiers.command)) && ui.rect_contains_pointer(ui.max_rect()) {
        if ui.input(|i| i.key_pressed(egui::Key::C)) {
            copy_selected_keys(app);
        }
        if ui.input(|i| i.key_pressed(egui::Key::V)) {
            paste_keys(app);
        }
    }
    let mut curves: Vec<(String, String, String)> = app.s.anim.curves.iter().filter(|(_, c)| app.s.sel.contains(&c.node) && !c.keys.is_empty()).map(|(k, c)| (k.clone(), app.s.name_of(&c.node), c.attr.clone())).collect();
    if app.panels.cb_sync_graph && !app.panels.cb_sel.is_empty() {
        curves.retain(|(_, _, attr)| app.panels.cb_sel.contains(attr));
    }
    let status = app.windows_state.ge_status.clone();
    ui.horizontal(|ui| {
        if let Some(status) = &status {
            ui.label(RichText::new(status).color(theme::TEXT_DIM));
            ui.separator();
        }
        if ui.selectable_label(app.windows_state.ge_scale, "Scale").on_hover_text("Scale selected keys (R), then middle-drag in the graph; the press point is the pivot").clicked() {
            finish_graph_scale(app);
            app.windows_state.ge_scale = !app.windows_state.ge_scale;
        }
        ui.separator();
        let selected_count = app.windows_state.ge_keys.len();
        if let Some((ck, ki)) = app.windows_state.ge_keys.first().cloned() {
            if selected_count > 1 {
                ui.label(format!("{selected_count} keys selected"));
            } else if let Some(c) = app.s.anim.curves.get(&ck).cloned() {
                if let Some(k) = c.keys.get(ki).cloned() {
                    ui.label(format!("Key  t={}  v={}", inca_core::math::fmt_num(k.t), inca_core::math::fmt_num(k.v)));
                    let mut tan = k.ot.clone();
                    egui::ComboBox::from_id_salt("getan").selected_text(&tan).show_ui(ui, |ui| {
                        for t in inca_core::anim::TANGENTS {
                            ui.selectable_value(&mut tan, t.to_string(), *t);
                        }
                    });
                    if tan != k.ot {
                        app.s.checkpoint("key tangents");
                        let key = &mut app.s.anim.curves.get_mut(&ck).unwrap().keys[ki];
                        key.it = tan.clone();
                        key.ot = tan;
                        app.s.touch();
                    }
                }
                let (mut pre, mut post) = (c.pre.clone(), c.post.clone());
                ui.separator();
                egui::ComboBox::from_id_salt("gepre").selected_text(format!("pre: {pre}")).show_ui(ui, |ui| {
                    for t in inca_core::anim::INFINITY {
                        ui.selectable_value(&mut pre, t.to_string(), *t);
                    }
                });
                egui::ComboBox::from_id_salt("gepost").selected_text(format!("post: {post}")).show_ui(ui, |ui| {
                    for t in inca_core::anim::INFINITY {
                        ui.selectable_value(&mut post, t.to_string(), *t);
                    }
                });
                let c = app.s.anim.curves.get_mut(&ck).unwrap();
                if pre != c.pre || post != c.post {
                    c.pre = pre;
                    c.post = post;
                    app.s.touch();
                }
            }
        } else {
            let hint = if app.windows_state.ge_scale {
                "Scale Keys: select keys, then middle-drag in the graph; the drag locks to the first axis you move"
            } else {
                "Select an animated object; click a key to edit it, drag to move it"
            };
            ui.label(RichText::new(hint).color(theme::TEXT_DIM));
        }
    });
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(160.0);
            for (k, n, a) in &curves {
                let mut on = !app.windows_state.ge_curves.contains(k);
                if ui.checkbox(&mut on, format!("{n}.{a}")).changed() {
                    if on {
                        app.windows_state.ge_curves.remove(k);
                    } else {
                        app.windows_state.ge_curves.insert(k.clone());
                        app.windows_state.ge_keys.retain(|(sk, _)| sk != k);
                    }
                }
            }
        });
        let colors = [Color32::from_rgb(230, 80, 80), Color32::from_rgb(90, 210, 90), Color32::from_rgb(90, 140, 255), Color32::from_rgb(230, 200, 80), Color32::from_rgb(200, 120, 230)];
        let t0 = app.s.time.start;
        let t1 = app.s.time.end;
        let cur = app.s.time.current;
        let shown: Vec<String> = curves.iter().filter(|c| !app.windows_state.ge_curves.contains(&c.0)).map(|c| c.0.clone()).collect();
        app.windows_state.ge_keys.retain(|(ck, _)| shown.contains(ck));
        let selected_keys = app.windows_state.ge_keys.clone();
        // A single key or a flat curve gives egui_plot a zero-height y range, which makes it show
        // raw values like 217.13609496869865 at the axis limits. Pad the visible range.
        let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
        for k in &shown {
            if let Some(c) = app.s.anim.curves.get(k) {
                for key in &c.keys {
                    ymin = ymin.min(key.v);
                    ymax = ymax.max(key.v);
                }
            }
        }
        if !ymin.is_finite() || !ymax.is_finite() || ymax < ymin {
            ymin = -1.5;
            ymax = 1.5;
        }
        let empty = shown.is_empty();
        let span = ymax - ymin;
        let pad = if empty { 0.0 } else if span < 1e-6 { ymax.abs().max(1.0) * 0.01 } else { span * 0.1 };
        let (y0, y1) = (ymin - pad, ymax + pad);
        let (mut kx0, mut kx1) = (f64::INFINITY, f64::NEG_INFINITY);
        for k in &shown {
            if let Some(c) = app.s.anim.curves.get(k) {
                if let Some(first) = c.keys.first() {
                    kx0 = kx0.min(first.t);
                }
                if let Some(last) = c.keys.last() {
                    kx1 = kx1.max(last.t);
                }
            }
        }
        let (x0, x1) = if kx0.is_finite() && kx1.is_finite() {
            let pad = ((kx1 - kx0) * 0.05).max(2.0);
            (kx0 - pad, kx1 + pad)
        } else {
            (t0, t1)
        };
        // F frames the selected key, or all visible curves; with nothing selected it returns to
        // Maya's default graph view.
        let frame_req = app.windows_state.ge_frame.take();
        let mut key_bounds = None;
        if !selected_keys.is_empty() {
            let (mut t0k, mut t1k, mut v0k, mut v1k) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for (ck, ki) in &selected_keys {
                if let Some(k) = app.s.anim.curves.get(ck).and_then(|c| c.keys.get(*ki)) {
                    t0k = t0k.min(k.t);
                    t1k = t1k.max(k.t);
                    v0k = v0k.min(k.v);
                    v1k = v1k.max(k.v);
                }
            }
            if t0k.is_finite() && t1k.is_finite() && v0k.is_finite() && v1k.is_finite() {
                let xpad = ((t1k - t0k) * 0.05).max(2.0);
                let ypad = ((v1k - v0k) * 0.1).max(1.0);
                key_bounds = Some(egui_plot::PlotBounds::from_min_max([t0k - xpad, v0k - ypad], [t1k + xpad, v1k + ypad]));
            }
        }
        let frame_bounds = match frame_req {
            Some(1) => key_bounds.or_else(|| {
                if shown.is_empty() {
                    Some(egui_plot::PlotBounds::from_min_max([t0, -1.5], [t1, 1.5]))
                } else {
                    Some(egui_plot::PlotBounds::from_min_max([x0, y0], [x1, y1]))
                }
            }),
            Some(2) => {
                if shown.is_empty() {
                    Some(egui_plot::PlotBounds::from_min_max([t0, -1.5], [t1, 1.5]))
                } else {
                    Some(egui_plot::PlotBounds::from_min_max([x0, y0], [x1, y1]))
                }
            }
            _ => None,
        };
        // While an MMB scale drag is active, the graph view is locked: no wheel zoom, pan or
        // double-click reset until the scale finishes.
        let scaling = app.windows_state.ge_scaling;
        let mut plot = Plot::new("graph").grid_color(egui::Color32::from_gray(0x50)).y_axis_formatter(|m, _| format!("{:.3}", m.value)).allow_drag(false).allow_axis_zoom_drag(false).allow_boxed_zoom(false).allow_zoom(!scaling).allow_scroll(!scaling).allow_double_click_reset(!scaling);
        // Use fixed default bounds instead of following the data: otherwise dragging a key past
        // the edge makes egui_plot auto-frame the view every frame.
        if empty {
            plot = plot.default_x_bounds(t0, t1.max(t0 + 1.0)).default_y_bounds(-1.5, 1.5);
        } else {
            plot = plot.default_x_bounds(x0, x1.max(x0 + 1.0)).default_y_bounds(y0, y1.max(y0 + 1e-6));
        }
        plot = plot.auto_bounds(false);
        let resp = plot.show(ui, |pu| {
            if let Some(bounds) = frame_bounds {
                pu.set_plot_bounds(bounds);
            }
            // Maya-style navigation: Alt+left/middle pans, Alt+right zooms.
            let alt = pu.ctx().input(|i| i.modifiers.alt);
            let over = pu.response().hovered() || pu.response().dragged();
            if alt && over && !scaling {
                let (delta, primary, middle, secondary) = pu.ctx().input(|i| (i.pointer.delta(), i.pointer.primary_down(), i.pointer.middle_down(), i.pointer.secondary_down()));
                if primary || middle {
                    let b = pu.plot_bounds();
                    let (xr, yr) = (b.range_x(), b.range_y());
                    let rect = pu.response().rect;
                    if rect.width() > 0.0 && rect.height() > 0.0 {
                        let dx = -(delta.x as f64) * (*xr.end() - *xr.start()) / rect.width() as f64;
                        let dy = (delta.y as f64) * (*yr.end() - *yr.start()) / rect.height() as f64;
                        pu.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                            [*xr.start() + dx, *yr.start() + dy],
                            [*xr.end() + dx, *yr.end() + dy],
                        ));
                    }
                } else if secondary && delta.x != 0.0 {
                    let bounds = pu.plot_bounds();
                    let (xr, yr) = (bounds.range_x(), bounds.range_y());
                    let anchor = pu.pointer_coordinate().unwrap_or_else(|| egui_plot::PlotPoint::new((*xr.start() + *xr.end()) * 0.5, (*yr.start() + *yr.end()) * 0.5));
                    let factor = ((delta.x * 0.01).exp() as f64).clamp(0.5, 2.0);
                    pu.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                        [anchor.x + (*xr.start() - anchor.x) / factor, anchor.y + (*yr.start() - anchor.y) / factor],
                        [anchor.x + (*xr.end() - anchor.x) / factor, anchor.y + (*yr.end() - anchor.y) / factor],
                    ));
                }
            }
            for (i, k) in shown.iter().enumerate() {
                let c = &app.s.anim.curves[k];
                let col = match c.attr.chars().last() {
                    Some('X') => colors[0],
                    Some('Y') => colors[1],
                    Some('Z') => colors[2],
                    _ => colors[3 + i % 2],
                };
                let (a, b) = (c.keys[0].t.min(t0) - 10.0, c.keys.last().unwrap().t.max(t1) + 10.0);
                let pts: PlotPoints = (0..=400).map(|s| {
                    let t = a + (b - a) * s as f64 / 400.0;
                    [t, c.eval(t)]
                }).collect();
                pu.line(Line::new(c.attr.clone(), pts).color(col).width(1.5));
                let kp: PlotPoints = c.keys.iter().map(|k| [k.t, k.v]).collect();
                pu.points(Points::new(format!("{} keys", c.attr), kp).radius(3.5).color(Color32::BLACK));
                let mut selected_pts: Vec<[f64; 2]> = vec![];
                for (sk, si) in &selected_keys {
                    if sk == k {
                        if let Some(key) = c.keys.get(*si) {
                            selected_pts.push([key.t, key.v]);
                        }
                    }
                }
                if !selected_pts.is_empty() {
                    pu.points(Points::new(format!("selected {}", c.attr), selected_pts).radius(5.0).color(Color32::from_rgb(255, 230, 0)));
                }
            }
            pu.vline(egui_plot::VLine::new("time", cur).color(Color32::from_rgb(200, 60, 60)));
            pu.pointer_coordinate()
        });
        let pointer = resp.inner;
        let r = resp.response;
        let tf = resp.transform;
        app.windows_state.ge_first_key_screen = shown.iter().find_map(|k| {
            app.s.anim.curves.get(k).and_then(|c| c.keys.first()).map(|key| tf.position_from_point(&egui_plot::PlotPoint::new(key.t, key.v)))
        });
        let alt = ui.input(|i| i.modifiers.alt);
        let additive = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command);
        let mid_down = ui.input(|i| i.pointer.middle_down());
        let mid_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle));
        let mid_released = ui.input(|i| i.pointer.button_released(egui::PointerButton::Middle));
        let mid_pos = ui.input(|i| i.pointer.interact_pos());
        if r.clicked() || r.drag_started() {
            app.hk.capturing = false;
            app.hk.pending = None;
            if let Some(id) = ui.ctx().memory(|m| m.focused()) {
                ui.ctx().memory_mut(|m| m.surrender_focus(id));
            }
        }
        // Maya's Scale Keys tool scales with the middle mouse button, anywhere over the graph
        // view. The press point itself is the pivot, not the selection center.
        if app.windows_state.ge_scale && !alt && mid_pressed && !app.windows_state.ge_scaling {
            if let Some(p) = mid_pos {
                if r.rect.contains(p) {
                    if app.windows_state.ge_keys.is_empty() {
                        app.help = "Scale Keys: select keys first".into();
                    } else {
                        let pivot = tf.value_from_position(p);
                        app.s.checkpoint("scale keys");
                        app.windows_state.ge_scaling = true;
                        app.windows_state.ge_scale_pivot = Some((pivot.x, pivot.y));
                        app.windows_state.ge_scale_factors = (1.0, 1.0);
                        app.windows_state.ge_scale_axis = ScaleAxis::Free;
                        app.windows_state.ge_drag_anchor = Some((p.x as f64, p.y as f64));
                        let keys = app.windows_state.ge_keys.clone();
                        app.windows_state.ge_drag_keys = keys
                            .into_iter()
                            .filter_map(|(k, i)| app.s.anim.curves.get(&k).and_then(|c| c.keys.get(i)).map(|key| (k, i, key.t, key.v)))
                            .collect();
                        app.windows_state.ge_drag = false;
                        app.windows_state.ge_box = None;
                        app.windows_state.ge_tan_drag = None;
                    }
                }
            }
        }
        if !alt && !app.windows_state.ge_scaling {
        if r.drag_started() {
            if let Some(pp) = r.interact_pointer_pos() {
                if app.windows_state.ge_scale {
                    // With the Scale Keys tool active, the left button only selects keys.
                    if let Some((ck, ki)) = nearest_graph_key(app, &shown, &tf, pp) {
                        if additive {
                            ge_toggle(app, &ck, ki);
                        } else if !ge_selected(app, &ck, ki) {
                            ge_select_only(app, &ck, ki);
                        }
                    } else {
                        app.windows_state.ge_box = Some(pp);
                    }
                    app.windows_state.ge_drag = false;
                    app.windows_state.ge_tan_drag = None;
                    app.windows_state.ge_drag_keys.clear();
                } else {
                    let handle_side = graph_tangent_handles(app, &tf, &app.windows_state.ge_keys)
                        .and_then(|(_, ip, op)| if ip.distance(pp) < 7.0 { Some(-1) } else if op.distance(pp) < 7.0 { Some(1) } else { None });
                    if let Some(side) = handle_side {
                        app.s.checkpoint("key tangents");
                        app.windows_state.ge_tan_drag = Some(side);
                        app.windows_state.ge_drag = false;
                        app.windows_state.ge_box = None;
                    } else if let Some((ck, ki)) = nearest_graph_key(app, &shown, &tf, pp) {
                        if additive {
                            ge_toggle(app, &ck, ki);
                        } else if !ge_selected(app, &ck, ki) {
                            ge_select_only(app, &ck, ki);
                        }
                        if ge_selected(app, &ck, ki) {
                            app.s.checkpoint("move keys");
                            app.windows_state.ge_drag = true;
                            app.windows_state.ge_drag_anchor = pointer.map(|p| (p.x, p.y));
                            let keys = app.windows_state.ge_keys.clone();
                            app.windows_state.ge_drag_keys = keys
                                .into_iter()
                                .filter_map(|(k, i)| app.s.anim.curves.get(&k).and_then(|c| c.keys.get(i)).map(|key| (k, i, key.t, key.v)))
                                .collect();
                            app.windows_state.ge_box = None;
                            app.windows_state.ge_tan_drag = None;
                        }
                    } else {
                        app.windows_state.ge_box = Some(pp);
                        app.windows_state.ge_drag = false;
                        app.windows_state.ge_tan_drag = None;
                        app.windows_state.ge_drag_keys.clear();
                    }
                }
            }
        } else if r.clicked() {
            if let Some(pp) = r.interact_pointer_pos() {
                if let Some((ck, ki)) = nearest_graph_key(app, &shown, &tf, pp) {
                    if additive {
                        ge_toggle(app, &ck, ki);
                    } else {
                        ge_select_only(app, &ck, ki);
                    }
                } else if !additive {
                    app.windows_state.ge_keys.clear();
                }
            }
        }
        if r.dragged() {
            if let Some(side) = app.windows_state.ge_tan_drag {
                if let Some(p) = pointer {
                    if let Some((ck, ki)) = app.windows_state.ge_keys.first().cloned() {
                        if let Some(c) = app.s.anim.curves.get_mut(&ck) {
                            if let Some(key) = c.keys.get_mut(ki) {
                                let dx = if side < 0 { key.t - p.x } else { p.x - key.t };
                                if dx.abs() > 1e-6 {
                                    let slope = if side < 0 { (key.v - p.y) / dx } else { (p.y - key.v) / dx };
                                    if side < 0 {
                                        key.it = "fixed".into();
                                        key.is = slope;
                                    } else {
                                        key.ot = "fixed".into();
                                        key.os = slope;
                                    }
                                }
                            }
                        }
                        app.s.apply_anim();
                        app.s.touch();
                    }
                }
            } else if app.windows_state.ge_drag {
                if let (Some((ax, ay)), Some(p)) = (app.windows_state.ge_drag_anchor, pointer) {
                    let raw_dt = (p.x - ax).round();
                    let dv = p.y - ay;
                    let mut dt_min = f64::NEG_INFINITY;
                    let mut dt_max = f64::INFINITY;
                    for (ck, ki, t0k, _) in &app.windows_state.ge_drag_keys {
                        if let Some(c) = app.s.anim.curves.get(ck) {
                            let selected: std::collections::HashSet<usize> = app.windows_state.ge_drag_keys.iter().filter(|(k, _, _, _)| k == ck).map(|(_, i, _, _)| *i).collect();
                            if let Some(prev) = (0..*ki).rev().find(|i| !selected.contains(i)) {
                                dt_min = dt_min.max(c.keys[prev].t - *t0k + 1.0);
                            }
                            if let Some(next) = (*ki + 1..c.keys.len()).find(|i| !selected.contains(i)) {
                                dt_max = dt_max.min(c.keys[next].t - *t0k - 1.0);
                            }
                        }
                    }
                    let dt = if dt_min <= dt_max { raw_dt.clamp(dt_min, dt_max) } else { 0.0 };
                    let keys = app.windows_state.ge_drag_keys.clone();
                    for (ck, ki, t0k, v0k) in keys {
                        if let Some(c) = app.s.anim.curves.get_mut(&ck) {
                            if let Some(key) = c.keys.get_mut(ki) {
                                key.t = t0k + dt;
                                key.v = v0k + dv;
                            }
                        }
                    }
                    app.s.apply_anim();
                    app.s.touch();
                }
            }
        }
        if let Some(start) = app.windows_state.ge_box {
            if let Some(end) = r.interact_pointer_pos() {
                theme::marquee(&ui.painter_at(r.rect), egui::Rect::from_two_pos(start, end));
            }
        }
        }
        // Middle-drag scaling continues even if the pointer leaves the graph view. Alt is reserved
        // for view navigation, so it pauses scaling instead of mixing the two.
        if app.windows_state.ge_scaling && !alt && (mid_down || mid_released) {
            if let (Some((ax, ay)), Some(p)) = (app.windows_state.ge_drag_anchor, mid_pos) {
                if let Some((pt, pv)) = app.windows_state.ge_scale_pivot {
                    let (px, py) = (p.x as f64, p.y as f64);
                    let (dx, dy) = (px - ax, py - ay);
                    // Maya locks the drag to the axis you first move in, and keeps it locked
                    // until the middle button is released.
                    if app.windows_state.ge_scale_axis == ScaleAxis::Free && dx.abs().max(dy.abs()) >= GRAPH_SCALE_AXIS_LOCK {
                        app.windows_state.ge_scale_axis = if dx.abs() >= dy.abs() { ScaleAxis::Time } else { ScaleAxis::Value };
                    }
                    let axis = app.windows_state.ge_scale_axis;
                    // Linear scale so the factor passes through zero: the keys collapse onto the
                    // pivot and then cross over it instead of stopping at the pivot.
                    let (sx, sy) = match axis {
                        ScaleAxis::Free => (1.0, 1.0),
                        ScaleAxis::Time => ((1.0 + dx / GRAPH_SCALE_DRAG).clamp(-100.0, 100.0), 1.0),
                        ScaleAxis::Value => (1.0, (1.0 + dy / GRAPH_SCALE_DRAG).clamp(-100.0, 100.0)),
                    };
                    app.windows_state.ge_scale_factors = (sx, sy);
                    if axis != ScaleAxis::Free {
                        // Keep each drag key's original time/value (so the factor is applied once,
                        // not compounded every frame) but remap its index after sorting. Sorting live
                        // keeps the curve evaluation correct while keys cross over the pivot.
                        let original = app.windows_state.ge_drag_keys.clone();
                        let mut targets = Vec::with_capacity(original.len());
                        for (ck, ki, t0, v0) in &original {
                            let nt = pt + (t0 - pt) * sx;
                            let nv = pv + (v0 - pv) * sy;
                            targets.push((ck.clone(), nt, nv));
                            if let Some(c) = app.s.anim.curves.get_mut(ck) {
                                if let Some(key) = c.keys.get_mut(*ki) {
                                    key.t = nt;
                                    key.v = nv;
                                }
                            }
                        }
                        let affected: std::collections::HashSet<String> = targets.iter().map(|(ck, _, _)| ck.clone()).collect();
                        for ck in &affected {
                            if let Some(c) = app.s.anim.curves.get_mut(ck) {
                                c.sort();
                            }
                        }
                        let mut remapped = Vec::with_capacity(original.len());
                        for ((ck, _, t0, v0), (_, nt, nv)) in original.iter().zip(targets.iter()) {
                            if let Some(c) = app.s.anim.curves.get(ck) {
                                if let Some(i) = c.keys.iter().position(|k| k.t == *nt && k.v == *nv) {
                                    remapped.push((ck.clone(), i, *t0, *v0));
                                }
                            }
                        }
                        app.windows_state.ge_keys = remapped.iter().map(|(ck, i, _, _)| (ck.clone(), *i)).collect();
                        app.windows_state.ge_drag_keys = remapped;
                        app.s.apply_anim();
                        app.s.touch();
                    }
                }
            }
        }
        if app.windows_state.ge_scaling && !mid_down {
            finish_graph_scale(app);
        }
        if app.windows_state.ge_scaling {
            let axis = app.windows_state.ge_scale_axis;
            match axis {
                ScaleAxis::Time => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                ScaleAxis::Value => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical),
                ScaleAxis::Free => {}
            }
            if let Some((pt, pv)) = app.windows_state.ge_scale_pivot {
                let sp = tf.position_from_point(&egui_plot::PlotPoint::new(pt, pv));
                let p = ui.painter_at(r.rect);
                let col = theme::ACCENT;
                let (hx, vy) = match axis {
                    ScaleAxis::Time => (14.0, 4.0),
                    ScaleAxis::Value => (4.0, 14.0),
                    ScaleAxis::Free => (7.0, 7.0),
                };
                p.line_segment([sp - egui::vec2(hx, 0.0), sp + egui::vec2(hx, 0.0)], egui::Stroke::new(1.0, col));
                p.line_segment([sp - egui::vec2(0.0, vy), sp + egui::vec2(0.0, vy)], egui::Stroke::new(1.0, col));
                p.circle_stroke(sp, 5.0, egui::Stroke::new(1.0, col));
            }
        }
        if !app.windows_state.ge_scale {
            if let Some((kp, ip, op)) = graph_tangent_handles(app, &tf, &selected_keys) {
                let p = ui.painter_at(r.rect);
                let col = Color32::from_rgb(220, 180, 80);
                p.line_segment([kp, ip], egui::Stroke::new(1.0, col));
                p.line_segment([kp, op], egui::Stroke::new(1.0, col));
                p.circle_filled(ip, 3.0, col);
                p.circle_filled(op, 3.0, col);
            }
        }
        if r.drag_stopped() {
            if let Some(start) = app.windows_state.ge_box.take() {
                if let Some(end) = ui.input(|i| i.pointer.latest_pos()) {
                    let rect = egui::Rect::from_two_pos(start, end);
                    if rect.width() > 3.0 || rect.height() > 3.0 {
                        if !additive {
                            app.windows_state.ge_keys.clear();
                        }
                        for k in &shown {
                            for (i, key) in app.s.anim.curves[k].keys.iter().enumerate() {
                                let sp = tf.position_from_point(&egui_plot::PlotPoint::new(key.t, key.v));
                                if rect.contains(sp) && !ge_selected(app, k, i) {
                                    app.windows_state.ge_keys.push((k.clone(), i));
                                }
                            }
                        }
                    }
                }
            }
            if !app.windows_state.ge_scaling {
                app.windows_state.ge_tan_drag = None;
                app.windows_state.ge_drag = false;
                app.windows_state.ge_drag_anchor = None;
                app.windows_state.ge_drag_keys.clear();
            }
        }
    });
}

// ------------------------------------------------------------------ dope sheet

fn dope_sheet(app: &mut IncaApp, ui: &mut egui::Ui) {
    use egui::{pos2, vec2, Rect, Sense, Stroke};
    let nodes: Vec<String> = app.s.sel.clone();
    let (t0, t1) = app.windows_state.ds_view.unwrap_or((app.s.time.start, app.s.time.end.max(app.s.time.start + 1.0)));
    ui.label(RichText::new("Drag keys to retime them (the summary row moves every key). Delete removes the keys at the current time.").color(theme::TEXT_DIM));
    let rows: Vec<(Option<String>, String, Vec<f64>)> = std::iter::once((None, "Dope Sheet Summary".to_string(), app.s.anim.key_times(&nodes)))
        .chain(nodes.iter().map(|n| (Some(n.clone()), app.s.name_of(n), app.s.anim.key_times(std::slice::from_ref(n)))))
        .collect();
    let label_w = 170.0;
    let row_h = 20.0;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h * rows.len().max(1) as f32 + 18.0), Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() {
        if let Some(id) = ui.ctx().memory(|m| m.focused()) {
            ui.ctx().memory_mut(|m| m.surrender_focus(id));
        }
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, theme::BG_DARK);
    let track = Rect::from_min_max(pos2(rect.min.x + label_w, rect.min.y + 18.0), rect.max);
    let x_of = |t: f64| track.min.x + 6.0 + ((t - t0) / (t1 - t0)) as f32 * (track.width() - 12.0);
    let t_of = |x: f32| t0 + ((x - track.min.x - 6.0) / (track.width() - 12.0)) as f64 * (t1 - t0);
    // frame ruler
    let span = t1 - t0;
    let step = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0].into_iter().find(|s| (span / s) * 36.0 < track.width() as f64).unwrap_or(100.0);
    let mut t = (t0 / step).ceil() * step;
    while t <= t1 {
        let x = x_of(t);
        p.line_segment([pos2(x, track.min.y), pos2(x, rect.max.y)], Stroke::new(1.0, theme::BG2));
        p.text(pos2(x + 2.0, rect.min.y + 2.0), egui::Align2::LEFT_TOP, inca_core::math::fmt_num_long(t), egui::FontId::proportional(10.0), theme::TEXT_DIM);
        t += step;
    }
    let drag = app.windows_state.ds_drag.clone();
    for (i, (node, name, times)) in rows.iter().enumerate() {
        let y = track.min.y + i as f32 * row_h;
        p.text(pos2(rect.min.x + 6.0, y + row_h / 2.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(12.0), if node.is_none() { theme::TEXT_DIM } else { theme::TEXT });
        p.line_segment([pos2(track.min.x, y + row_h), pos2(track.max.x, y + row_h)], Stroke::new(1.0, theme::BG2));
        for &k in times {
            let moving = drag.as_ref().is_some_and(|(n, grab, _)| (n.is_none() || n == node) && (k - grab).abs() < 1e-6);
            let off = if moving { drag.as_ref().map(|d| d.2).unwrap_or(0.0) } else { 0.0 };
            let x = x_of(k + off);
            let c = if moving { Color32::from_rgb(255, 230, 0) } else if node.is_none() { Color32::from_rgb(170, 170, 170) } else { theme::KEY };
            p.rect_filled(Rect::from_center_size(pos2(x, y + row_h / 2.0), vec2(6.0, 12.0)), 1.0, c);
        }
    }
    let cx = x_of(app.s.time.current);
    p.line_segment([pos2(cx, rect.min.y), pos2(cx, rect.max.y)], Stroke::new(1.5, Color32::from_rgb(200, 60, 60)));
    if resp.drag_started() {
        if let Some(pp) = resp.interact_pointer_pos() {
            let row = ((pp.y - track.min.y) / row_h).floor() as isize;
            if row >= 0 && (row as usize) < rows.len() {
                let (node, _, times) = &rows[row as usize];
                if let Some(&k) = times.iter().min_by(|a, b| (x_of(**a) - pp.x).abs().partial_cmp(&(x_of(**b) - pp.x).abs()).unwrap()) {
                    if (x_of(k) - pp.x).abs() < 8.0 {
                        app.windows_state.ds_drag = Some((node.clone(), k, 0.0));
                    }
                }
            }
        }
    }
    if let (Some(d), Some(pp)) = (app.windows_state.ds_drag.as_mut(), resp.interact_pointer_pos()) {
        if resp.dragged() {
            d.2 = (t_of(pp.x) - d.1).round();
        }
    }
    if resp.drag_stopped() {
        if let Some((node, grab, off)) = app.windows_state.ds_drag.take() {
            if off != 0.0 {
                app.s.checkpoint("move keys");
                let ids: Vec<String> = match &node {
                    Some(n) => vec![n.clone()],
                    None => nodes.clone(),
                };
                for c in app.s.anim.curves.values_mut().filter(|c| ids.contains(&c.node)) {
                    if c.keys.iter().any(|k| (k.t - (grab + off)).abs() < 1e-6) {
                        continue; // don't stack two keys on one frame
                    }
                    for k in c.keys.iter_mut() {
                        if (k.t - grab).abs() < 1e-6 {
                            k.t = grab + off;
                        }
                    }
                    c.sort();
                }
                app.s.touch();
                app.s.apply_anim();
            }
        }
    }
    if resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            if track.contains(pp) {
                let t = t_of(pp.x).round().clamp(t0, t1);
                app.s.set_time(t);
            }
        }
    }
}

// ------------------------------------------------------------------ rendering

fn render_settings(app: &mut IncaApp, ui: &mut egui::Ui) {
    let d = inca_core::render::RenderSettings::default();
    let r = &mut app.s.render;
    let get = |k: &str, d: f64| r.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let gets = |k: &str, d: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string();
    let getb = |k: &str, d: bool| r.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
    let (mut w, mut h, mut spp, mut b, mut ex) = (get("width", d.width as f64), get("height", d.height as f64), get("samples", d.samples as f64), get("bounces", d.bounces as f64), get("exposure", d.exposure as f64));
    let mut pixel_aspect = get("pixelAspect", d.pixel_aspect as f64);
    let mut tone = gets("toneMapping", "aces");
    let mut hw_tone = gets("hwToneMapping", &tone);
    let mut bg: [f32; 3] = r.get("background").and_then(|v| v.as_array()).map(|a| [a.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32, a.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32, a.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32]).unwrap_or(d.background);
    let mut transparent = getb("transparentBg", d.transparent);
    let mut hw_aa = gets("hwAA", "high");
    let mut hw_shadow = get("hwShadowMap", d.hw_shadow_map as f64) as u32;
    let mut all_shadows = getb("hwAllShadows", d.hw_all_shadows);
    let mut format = gets("format", "png");
    let mut jpg_quality = get("jpgQuality", 0.92);
    let cams: Vec<String> = app.s.nodes.values().filter(|n| matches!(n.shape, inca_core::scene::Shape::Camera(_))).map(|n| n.name.clone()).collect();
    let r = &mut app.s.render;
    let mut renderer = r.get("renderer").and_then(|v| v.as_str()).unwrap_or("path").to_string();
    let mut camera = r.get("camera").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut fname = r.get("filename").and_then(|v| v.as_str()).unwrap_or("image").to_string();
    let (mut fs, mut fe, mut fb) = (r.get("start").and_then(|v| v.as_f64()).unwrap_or(1.0), r.get("end").and_then(|v| v.as_f64()).unwrap_or(10.0), r.get("by").and_then(|v| v.as_f64()).unwrap_or(1.0));
    egui::Grid::new("rs0").num_columns(2).show(ui, |ui| {
        ui.label("Render using");
        egui::ComboBox::from_id_salt("rsRenderer").selected_text(crate::render_view::RENDERERS.iter().find(|x| x.0 == renderer).map(|x| x.1).unwrap_or("")).show_ui(ui, |ui| {
            for (k, l) in crate::render_view::RENDERERS {
                ui.selectable_value(&mut renderer, k.to_string(), l);
            }
        });
        ui.end_row();
        ui.label("Renderable camera");
        egui::ComboBox::from_id_salt("rsCam").selected_text(if camera.is_empty() { "(active viewport)" } else { &camera }).show_ui(ui, |ui| {
            ui.selectable_value(&mut camera, String::new(), "(active viewport)");
            for c in &cams {
                ui.selectable_value(&mut camera, c.clone(), c);
            }
        });
        ui.end_row();
        ui.label("File name prefix");
        ui.text_edit_singleline(&mut fname);
        ui.end_row();
        ui.label("Start / End / By frame");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut fs));
            ui.add(egui::DragValue::new(&mut fe));
            ui.add(egui::DragValue::new(&mut fb).range(1.0..=1000.0));
        });
        ui.end_row();
    });
    r.insert("renderer".into(), json!(renderer));
    r.insert("camera".into(), json!(camera));
    r.insert("filename".into(), json!(fname.trim()));
    for (k, v) in [("start", fs), ("end", fe), ("by", fb)] {
        r.insert(k.into(), json!(v));
    }
    ui.separator();
    const TONES: [(&str, &str); 5] = [("aces", "ACES Filmic"), ("agx", "AgX"), ("neutral", "Khronos Neutral"), ("linear", "Linear"), ("none", "None")];
    const AA: [(&str, &str); 5] = [("off", "Off"), ("low", "Low"), ("medium", "Medium"), ("high", "High"), ("ultra", "Ultra")];
    egui::Grid::new("rs").num_columns(2).show(ui, |ui| {
        ui.label("Presets");
        ui.horizontal(|ui| {
            for (l, pw, ph) in [("HD 540", 960.0, 540.0), ("HD 720", 1280.0, 720.0), ("HD 1080", 1920.0, 1080.0), ("640x480", 640.0, 480.0), ("2K Sq", 2048.0, 2048.0), ("4K UHD", 3840.0, 2160.0)] {
                if ui.small_button(l).clicked() {
                    w = pw;
                    h = ph;
                }
            }
        });
        ui.end_row();
        ui.label("Width");
        ui.add(egui::DragValue::new(&mut w).range(16.0..=8192.0));
        ui.end_row();
        ui.label("Height");
        ui.add(egui::DragValue::new(&mut h).range(16.0..=8192.0));
        ui.end_row();
        ui.label("Pixel aspect");
        ui.add(egui::DragValue::new(&mut pixel_aspect).speed(0.01).range(0.1..=10.0));
        ui.end_row();
        ui.label("Samples");
        ui.add(egui::DragValue::new(&mut spp).range(1.0..=10000.0));
        ui.end_row();
        ui.label("Bounces");
        ui.add(egui::DragValue::new(&mut b).range(0.0..=32.0));
        ui.end_row();
        ui.label("Exposure");
        ui.add(egui::DragValue::new(&mut ex).speed(0.05).range(-10.0..=10.0));
        ui.end_row();
        ui.label("Tone mapping");
        egui::ComboBox::from_id_salt("rsTone").selected_text(TONES.iter().find(|x| x.0 == tone).map(|x| x.1).unwrap_or("ACES Filmic")).show_ui(ui, |ui| {
            for (k, l) in TONES {
                ui.selectable_value(&mut tone, k.to_string(), l);
            }
        });
        ui.end_row();
        ui.label("Hardware tone");
        egui::ComboBox::from_id_salt("rsHwTone").selected_text(TONES.iter().find(|x| x.0 == hw_tone).map(|x| x.1).unwrap_or("ACES Filmic")).show_ui(ui, |ui| {
            for (k, l) in TONES {
                ui.selectable_value(&mut hw_tone, k.to_string(), l);
            }
        });
        ui.end_row();
        ui.label("Background");
        ui.color_edit_button_rgb(&mut bg);
        ui.end_row();
        ui.label("Transparent background");
        ui.checkbox(&mut transparent, "");
        ui.end_row();
        ui.label("Hardware anti-aliasing");
        egui::ComboBox::from_id_salt("rsAA").selected_text(AA.iter().find(|x| x.0 == hw_aa).map(|x| x.1).unwrap_or("High")).show_ui(ui, |ui| {
            for (k, l) in AA {
                ui.selectable_value(&mut hw_aa, k.to_string(), l);
            }
        });
        ui.end_row();
        ui.label("Shadow map size");
        egui::ComboBox::from_id_salt("rsShadowSize").selected_text(hw_shadow.to_string()).show_ui(ui, |ui| {
            for s in [512, 1024, 2048, 4096, 8192] {
                ui.selectable_value(&mut hw_shadow, s, s.to_string());
            }
        });
        ui.end_row();
        ui.label("All lights cast shadows");
        ui.checkbox(&mut all_shadows, "");
        ui.end_row();
        ui.label("Image format");
        egui::ComboBox::from_id_salt("rsFmt").selected_text(if format == "jpg" { "JPEG" } else { "PNG" }).show_ui(ui, |ui| {
            ui.selectable_value(&mut format, "png".to_string(), "PNG");
            ui.selectable_value(&mut format, "jpg".to_string(), "JPEG");
        });
        ui.end_row();
        ui.label("JPEG quality");
        ui.add(egui::DragValue::new(&mut jpg_quality).speed(0.01).range(0.01..=1.0));
        ui.end_row();
    });
    for (k, v) in [("width", w), ("height", h), ("samples", spp), ("bounces", b), ("exposure", ex), ("pixelAspect", pixel_aspect), ("jpgQuality", jpg_quality)] {
        r.insert(k.into(), json!(v));
    }
    r.insert("toneMapping".into(), json!(tone));
    r.insert("hwToneMapping".into(), json!(hw_tone));
    r.insert("background".into(), json!(bg));
    r.insert("transparentBg".into(), json!(transparent));
    r.insert("hwAA".into(), json!(hw_aa));
    r.insert("hwShadowMap".into(), json!(hw_shadow));
    r.insert("hwAllShadows".into(), json!(all_shadows));
    r.insert("format".into(), json!(format));
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_delete_key_removes_only_the_selected_key() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut c = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        c.set_key(1.0, 0.0, "linear");
        c.set_key(10.0, 5.0, "linear");
        app.s.anim.curves.insert(ck.clone(), c);
        app.windows_state.ge_keys = vec![(ck.clone(), 0)];
        delete_selected_keys(&mut app);
        let c = app.s.anim.curves.get(&ck).expect("curve with a remaining key should stay");
        assert_eq!(c.keys.len(), 1);
        assert_eq!(c.keys[0].t, 10.0);
        app.windows_state.ge_keys = vec![(ck.clone(), 0)];
        delete_selected_keys(&mut app);
        assert!(!app.s.anim.curves.contains_key(&ck), "empty curve should be removed");
    }

    #[test]
    fn offscreen_window_is_recentered() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = egui::Id::new("offscreen_recenter_test");
        let raw = || {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw
        };
        // Seed an off-screen position.
        let mut out = ctx.run_ui(raw(), |ui| {
            let ctx = ui.ctx().clone();
            egui::Window::new("Offscreen Recenter Test").id(id).fixed_pos(egui::pos2(5000.0, 5000.0)).constrain(false).show(&ctx, |ui| {
                ui.label("x");
            });
        });
        out.textures_delta.clear();
        assert!(!window_reachable(&ctx, ctx.memory(|m| m.area_rect(id)).expect("seeded window")));
        // Reopening through the placement helper must bring it back.
        let mut open = true;
        let is_open = open;
        let mut out = ctx.run_ui(raw(), |ui| {
            let ctx = ui.ctx().clone();
            let _ = place_window(&mut app, &ctx, id, is_open, |_app, fix| {
                let mut window = egui::Window::new("Offscreen Recenter Test").id(id).open(&mut open).constrain(false);
                if let Some(p) = fix {
                    window = window.fixed_pos(p);
                }
                window.show(&ctx, |ui| {
                    ui.label("x");
                })
            });
        });
        out.textures_delta.clear();
        let rect = ctx.memory(|m| m.area_rect(id)).expect("reopened window");
        assert!(window_reachable(&ctx, rect), "window is still off screen: {rect:?}");
    }

    #[test]
    fn placed_window_can_be_dragged() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = egui::Id::new("drag_window_test");
        let mut open = true;
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            let is_open = open;
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| {
                let ctx = ui.ctx().clone();
                let _ = place_window(app, &ctx, id, is_open, |_app, fix| {
                    let mut window = egui::Window::new("Drag Test").id(id).open(&mut open).constrain(false);
                    if let Some(p) = fix {
                        window = window.fixed_pos(p);
                    }
                    window.show(&ctx, |ui| {
                        ui.label("x");
                    })
                });
            });
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        run(&mut app, vec![], 0.0);
        let before = ctx.memory(|m| m.area_rect(id)).expect("window shown");
        let start = egui::pos2(before.left() + 40.0, before.top() + 10.0);
        let end = start + egui::vec2(60.0, 40.0);
        run(&mut app, vec![egui::Event::PointerMoved(start)], 0.1);
        run(&mut app, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.2);
        run(&mut app, vec![egui::Event::PointerMoved(end)], 0.3);
        run(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.4);
        let after = ctx.memory(|m| m.area_rect(id)).expect("window still shown");
        assert!(after.left() > before.left() + 20.0, "window did not move: {before:?} -> {after:?}");
    }

    #[test]
    fn window_rects_round_trip() {
        let map = std::collections::HashMap::from([(42u64, egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 200.0)))]);
        assert_eq!(decode_window_rects(encode_window_rects(&map)), map);
    }

    #[derive(Default)]
    struct TestStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for TestStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn window_rects_persist_through_storage() {
        let map = std::collections::HashMap::from([(42u64, egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 200.0)))]);
        let mut storage = TestStorage::default();
        eframe::set_value(&mut storage, "windowRects", &encode_window_rects(&map));
        let loaded: Vec<(u64, [f32; 4])> = eframe::get_value(&storage, "windowRects").expect("stored rects");
        assert_eq!(decode_window_rects(loaded), map);
    }

    #[test]
    fn reopened_window_keeps_its_position() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = egui::Id::new("reopen_position_test");
        let open = std::cell::Cell::new(true);
        let run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            let is_open = open.get();
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| {
                let ctx = ui.ctx().clone();
                let mut o = is_open;
                let _ = place_window(app, &ctx, id, is_open, |_app, fix| {
                    let mut window = egui::Window::new("Reopen Test").id(id).open(&mut o).constrain(false);
                    if let Some(p) = fix {
                        window = window.fixed_pos(p);
                    }
                    window.show(&ctx, |ui| {
                        ui.label("x");
                    })
                });
                open.set(o);
            });
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        run(&mut app, vec![], 0.0);
        let before = ctx.memory(|m| m.area_rect(id)).expect("window shown");
        let start = egui::pos2(before.left() + 40.0, before.top() + 10.0);
        let end = start + egui::vec2(60.0, 40.0);
        run(&mut app, vec![egui::Event::PointerMoved(start)], 0.1);
        run(&mut app, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.2);
        run(&mut app, vec![egui::Event::PointerMoved(end)], 0.3);
        run(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.4);
        let moved = ctx.memory(|m| m.area_rect(id)).expect("window moved");
        open.set(false);
        for i in 0..4 {
            run(&mut app, vec![], 0.5 + i as f64 * 0.1);
        }
        // Corrupt egui's saved area position while the window is closed: the app's last-rect
        // memory must still win on reopen.
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
        raw.time = Some(1.3);
        let mut out = ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            egui::Window::new("Reopen Test").id(id).fixed_pos(egui::pos2(1400.0, 900.0)).constrain(false).show(&ctx, |ui| {
                ui.label("x");
            });
        });
        out.textures_delta.clear();
        open.set(true);
        run(&mut app, vec![], 1.4);
        let reopened = ctx.memory(|m| m.area_rect(id)).expect("window reopened");
        assert!((reopened.left() - moved.left()).abs() < 2.0 && (reopened.top() - moved.top()).abs() < 2.0, "position changed on reopen: {moved:?} -> {reopened:?}");
    }

    #[test]
    fn reopened_hypershade_keeps_its_position() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        let run = |app: &mut IncaApp, frame: &mut eframe::Frame, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| show(app, ui.ctx(), frame));
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        app.win.hypershade = true;
        for i in 0..4 {
            run(&mut app, &mut frame, vec![], i as f64 * 0.1);
        }
        let id = egui::Id::new("HypershadeWindow");
        let before = ctx.memory(|m| m.area_rect(id)).expect("Hypershade shown");
        let start = egui::pos2(before.left() + 80.0, before.top() + 12.0);
        let end = start + egui::vec2(60.0, 40.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(start)], 0.5);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.6);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(end)], 0.7);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.8);
        let moved = ctx.memory(|m| m.area_rect(id)).expect("Hypershade moved");
        app.win.hypershade = false;
        for i in 0..4 {
            run(&mut app, &mut frame, vec![], 0.9 + i as f64 * 0.1);
        }
        app.win.hypershade = true;
        run(&mut app, &mut frame, vec![], 1.4);
        let reopened = ctx.memory(|m| m.area_rect(id)).expect("Hypershade reopened");
        assert!((reopened.left() - moved.left()).abs() < 2.0 && (reopened.top() - moved.top()).abs() < 2.0, "Hypershade position changed on reopen: {moved:?} -> {reopened:?}");
    }

    #[test]
    fn graph_delete_removes_all_selected_keys() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut c = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        for (i, t) in [1.0, 5.0, 10.0, 15.0].iter().enumerate() {
            c.set_key(*t, i as f64, "linear");
        }
        app.s.anim.curves.insert(ck.clone(), c);
        app.windows_state.ge_keys = vec![(ck.clone(), 1), (ck.clone(), 3)];
        delete_selected_keys(&mut app);
        let c = app.s.anim.curves.get(&ck).expect("curve should remain");
        assert_eq!(c.keys.iter().map(|k| k.t).collect::<Vec<_>>(), vec![1.0, 10.0]);
    }

    #[test]
    fn graph_copy_paste_preserves_relative_spacing() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut c = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        for (i, t) in [1.0, 5.0, 10.0].iter().enumerate() {
            c.set_key(*t, i as f64 * 10.0, "linear");
        }
        app.s.anim.curves.insert(ck.clone(), c);
        app.windows_state.ge_keys = vec![(ck.clone(), 0), (ck.clone(), 2)];
        copy_selected_keys(&mut app);
        app.s.time.current = 20.0;
        paste_keys(&mut app);
        let times: Vec<f64> = app.s.anim.curves[&ck].keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![1.0, 5.0, 10.0, 20.0, 29.0]);
        let mut selected: Vec<f64> = app.windows_state.ge_keys.iter().filter(|(k, _)| k == &ck).map(|(_, i)| app.s.anim.curves[&ck].keys[*i].t).collect();
        selected.sort_by(|a, b| a.total_cmp(b));
        assert_eq!(selected, vec![20.0, 29.0]);
    }

    #[test]
    fn graph_scale_mode_toggles() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        app.action("graphScaleKeys");
        assert!(app.windows_state.ge_scale);
        app.action("graphSelectKeys");
        assert!(!app.windows_state.ge_scale);
    }

    #[test]
    fn graph_scale_uses_middle_drag_not_left_drag() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut c = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        for (i, t) in [1.0, 5.0, 10.0].iter().enumerate() {
            c.set_key(*t, i as f64 * 10.0, "linear");
        }
        app.s.anim.curves.insert(ck.clone(), c);
        app.s.sel = vec!["pCube1".into()];
        app.win.graph_editor = true;
        app.windows_state.ge_scale = true;
        app.windows_state.ge_keys = vec![(ck.clone(), 0), (ck.clone(), 2)];
        let run = |app: &mut IncaApp, frame: &mut eframe::Frame, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| show(app, ui.ctx(), frame));
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        for i in 0..5 {
            run(&mut app, &mut frame, vec![], i as f64 * 0.1);
        }
        let key_pos = app.windows_state.ge_first_key_screen.expect("key on screen");
        // Left-drag on a selected key must not scale or move it in Scale Keys mode.
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(key_pos)], 0.6);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: key_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.7);
        let left_end = key_pos + egui::vec2(80.0, 80.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(left_end)], 0.8);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: left_end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.9);
        let times: Vec<f64> = app.s.anim.curves[&ck].keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![1.0, 5.0, 10.0], "left drag moved or scaled keys");
        // Middle-drag anywhere in the graph (not on a key) scales the selected keys, with the
        // press point as the pivot.
        let before: Vec<(f64, f64)> = app.s.anim.curves[&ck].keys.iter().map(|k| (k.t, k.v)).collect();
        let start = key_pos + egui::vec2(50.0, -40.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(start)], 1.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 1.1);
        assert!(app.windows_state.ge_scaling, "middle press did not start scaling (start {start:?}, window {:?}, key {key_pos:?})", app.windows_state.ge_window_rect);
        assert_eq!(app.windows_state.ge_drag_keys.len(), 2, "scale drag keys after press: {:?}", app.windows_state.ge_drag_keys);
        let pivot = app.windows_state.ge_scale_pivot.expect("scale pivot missing");
        assert!((pivot.0 - 5.5).abs() > 1.0, "pivot must be the MMB press point, not the selection center: {pivot:?}");
        // The view is locked while the middle button is down: the wheel must not zoom. The pointer
        // is still over the graph here.
        run(&mut app, &mut frame, vec![], 1.15);
        let before_wheel = app.windows_state.ge_first_key_screen.expect("key on screen");
        run(&mut app, &mut frame, vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, 5.0), phase: egui::TouchPhase::Move, modifiers: Default::default() }], 1.2);
        run(&mut app, &mut frame, vec![], 1.25);
        let after_wheel = app.windows_state.ge_first_key_screen.expect("key on screen");
        assert!(app.windows_state.ge_scaling, "scaling ended during wheel");
        assert!(after_wheel.distance(before_wheel) < 0.01, "wheel changed the locked view: {before_wheel:?} -> {after_wheel:?}");
        // The first movement is horizontal, so the drag locks to time scaling; the vertical part
        // of the motion must not change values.
        let end = start + egui::vec2(120.0, 120.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(end)], 1.3);
        assert!(app.windows_state.ge_scaling, "middle move ended scaling");
        assert_eq!(app.windows_state.ge_scale_axis, ScaleAxis::Time, "drag should lock to the time axis");
        assert!((app.windows_state.ge_scale_factors.0 - 2.0).abs() < 1e-3, "time factor after middle move: {:?}", app.windows_state.ge_scale_factors);
        assert!((app.windows_state.ge_scale_factors.1 - 1.0).abs() < 1e-9, "value factor must stay locked: {:?}", app.windows_state.ge_scale_factors);
        assert_eq!(app.windows_state.ge_drag_keys.len(), 2, "scale drag keys after move: {:?}", app.windows_state.ge_drag_keys);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() }], 1.4);
        let mut expected: Vec<(f64, f64)> = before
            .iter()
            .enumerate()
            .map(|(i, (t, v))| if i == 1 { (*t, *v) } else { (pivot.0 + (t - pivot.0) * 2.0, *v) })
            .collect();
        expected.sort_by(|a, b| a.0.total_cmp(&b.0));
        let keys = &app.s.anim.curves[&ck].keys;
        assert_eq!(keys.len(), expected.len());
        for (i, key) in keys.iter().enumerate() {
            assert!((key.t - expected[i].0).abs() < 1e-3, "key {i} time: {:?} expected {expected:?}", keys.iter().map(|k| k.t).collect::<Vec<_>>());
            assert!((key.v - expected[i].1).abs() < 1e-3, "key {i} value: {:?} expected {expected:?}", keys.iter().map(|k| k.v).collect::<Vec<_>>());
        }
        assert_eq!(app.windows_state.ge_keys.len(), 2, "scaled keys should stay selected");
        assert!(!app.windows_state.ge_scaling, "scale drag did not finish");
        assert_eq!(app.windows_state.ge_scale_axis, ScaleAxis::Free, "axis lock should reset");
    }

    #[test]
    fn graph_scale_crosses_the_middle_pivot() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut c = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        c.set_key(1.0, 0.0, "linear");
        c.set_key(9.0, 10.0, "linear");
        app.s.anim.curves.insert(ck.clone(), c);
        app.s.sel = vec!["pCube1".into()];
        app.win.graph_editor = true;
        app.windows_state.ge_scale = true;
        app.windows_state.ge_keys = vec![(ck.clone(), 0), (ck.clone(), 1)];
        let run = |app: &mut IncaApp, frame: &mut eframe::Frame, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| show(app, ui.ctx(), frame));
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        for i in 0..5 {
            run(&mut app, &mut frame, vec![], i as f64 * 0.1);
        }
        let key_pos = app.windows_state.ge_first_key_screen.expect("key on screen");
        // Press between the keys, then drag far enough left that the factor becomes negative.
        let start = key_pos + egui::vec2(60.0, -30.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(start)], 0.6);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 0.7);
        let pivot = app.windows_state.ge_scale_pivot.expect("scale pivot");
        assert!(pivot.0 > 1.0 && pivot.0 < 9.0, "press point should be between the keys: {pivot:?}");
        let drag = app.windows_state.ge_drag_keys.clone();
        let end = start + egui::vec2(-240.0, 0.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(end)], 0.8);
        assert_eq!(app.windows_state.ge_scale_axis, ScaleAxis::Time, "horizontal drag should lock to time");
        assert!((app.windows_state.ge_scale_factors.0 + 1.0).abs() < 1e-3, "factor should cross zero: {:?}", app.windows_state.ge_scale_factors);
        assert!((app.windows_state.ge_scale_factors.1 - 1.0).abs() < 1e-9, "value factor must stay locked: {:?}", app.windows_state.ge_scale_factors);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() }], 0.9);
        let mut expected: Vec<(f64, f64)> = drag.iter().map(|(_, _, t0, v0)| (pivot.0 + (t0 - pivot.0) * -1.0, *v0)).collect();
        expected.sort_by(|a, b| a.0.total_cmp(&b.0));
        let keys = &app.s.anim.curves[&ck].keys;
        assert_eq!(keys.len(), expected.len());
        for (i, key) in keys.iter().enumerate() {
            assert!((key.t - expected[i].0).abs() < 1e-3, "key {i} time: {:?} expected {expected:?}", keys.iter().map(|k| k.t).collect::<Vec<_>>());
            assert!((key.v - expected[i].1).abs() < 1e-3, "key {i} value: {:?} expected {expected:?}", keys.iter().map(|k| k.v).collect::<Vec<_>>());
        }
        // The left key is now right of the right key, so the values swapped ends.
        assert_eq!(expected.first().map(|k| k.1), Some(10.0), "keys stopped at the pivot instead of crossing");
        assert_eq!(expected.last().map(|k| k.1), Some(0.0));
        assert_eq!(app.windows_state.ge_keys.len(), 2, "crossed keys should stay selected");
    }

    #[test]
    fn graph_scale_axis_stays_locked_until_release() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut c = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        c.set_key(1.0, 0.0, "linear");
        c.set_key(9.0, 10.0, "linear");
        app.s.anim.curves.insert(ck.clone(), c);
        app.s.sel = vec!["pCube1".into()];
        app.win.graph_editor = true;
        app.windows_state.ge_scale = true;
        app.windows_state.ge_keys = vec![(ck.clone(), 0), (ck.clone(), 1)];
        let run = |app: &mut IncaApp, frame: &mut eframe::Frame, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| show(app, ui.ctx(), frame));
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        for i in 0..5 {
            run(&mut app, &mut frame, vec![], i as f64 * 0.1);
        }
        let key_pos = app.windows_state.ge_first_key_screen.expect("key on screen");
        let start = key_pos + egui::vec2(80.0, -30.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(start)], 0.6);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: start, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 0.7);
        let pivot = app.windows_state.ge_scale_pivot.expect("scale pivot");
        // Move mostly downward first: this locks the drag to value scaling.
        let first = start + egui::vec2(20.0, 120.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(first)], 0.8);
        assert_eq!(app.windows_state.ge_scale_axis, ScaleAxis::Value, "vertical drag should lock to value");
        assert!((app.windows_state.ge_scale_factors.0 - 1.0).abs() < 1e-12, "time factor must stay locked: {:?}", app.windows_state.ge_scale_factors);
        assert!((app.windows_state.ge_scale_factors.1 - 2.0).abs() < 1e-3, "value factor after first move: {:?}", app.windows_state.ge_scale_factors);
        // Moving far sideways must not switch axes or change times.
        let sideways = start + egui::vec2(240.0, 140.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(sideways)], 0.9);
        assert_eq!(app.windows_state.ge_scale_axis, ScaleAxis::Value, "axis switched while the middle button was held");
        assert!((app.windows_state.ge_scale_factors.0 - 1.0).abs() < 1e-12, "time factor changed after sideways move: {:?}", app.windows_state.ge_scale_factors);
        let sy = 1.0 + 140.0 / GRAPH_SCALE_DRAG;
        assert!((app.windows_state.ge_scale_factors.1 - sy).abs() < 1e-3, "value factor after sideways move: {:?}", app.windows_state.ge_scale_factors);
        run(&mut app, &mut frame, vec![egui::Event::PointerButton { pos: sideways, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() }], 1.0);
        let keys = &app.s.anim.curves[&ck].keys;
        assert_eq!(keys.iter().map(|k| k.t).collect::<Vec<_>>(), vec![1.0, 9.0], "times changed during a value-locked drag");
        for (key, v0) in keys.iter().zip([0.0, 10.0]) {
            let expected = pivot.1 + (v0 - pivot.1) * sy;
            assert!((key.v - expected).abs() < 1e-3, "value should scale around the press point: {} != {expected}", key.v);
        }
        assert_eq!(app.windows_state.ge_scale_axis, ScaleAxis::Free, "axis lock should reset");
    }

    #[test]
    fn graph_and_dope_windows_open_on_screen() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        let run = |app: &mut IncaApp, frame: &mut eframe::Frame, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| show(app, ui.ctx(), frame));
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        // Closed windows run their fade-out once; opening the editors after that is when the
        // old auto-placement could push them below the screen.
        for i in 0..3 {
            run(&mut app, &mut frame, i as f64 * 0.1);
        }
        app.win.graph_editor = true;
        app.win.dope_sheet = true;
        for i in 3..6 {
            run(&mut app, &mut frame, i as f64 * 0.1);
        }
        let content = ctx.content_rect();
        for (name, rect) in [("Graph Editor", app.windows_state.ge_window_rect), ("Dope Sheet", app.windows_state.ds_window_rect)] {
            let rect = rect.unwrap_or_else(|| panic!("{name} was not shown"));
            assert!(content.contains_rect(rect), "{name} is off screen: {rect:?} outside {content:?}");
        }
    }
}
