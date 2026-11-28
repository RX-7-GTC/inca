//! Preferences window: categories on the left, settings on the right.
//! Settings that live elsewhere in the app (background, HUD, manipulator size, time range, undo,
//! autosave, UI elements) are edited in place; the rest are kept here and saved with the app state.

use crate::app::{IncaApp, MenuSet};
use crate::theme;
use egui::RichText;
use std::path::PathBuf;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AttributePreset {
    pub name: String,
    pub kind: String,
    pub values: Vec<(String, serde_json::Value)>,
}

/// Current manipulator defaults revision. 1: negative axis handles off by default, like Maya.
pub const MANIP_DEFAULTS_REV: u32 = 1;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub ui_scale: f32,
    pub follow_os_scale: bool,
    pub menu_set: usize,
    pub tooltips: bool,
    pub confirm_quit: bool,
    pub ortho_tumble: bool,
    pub grid_size: f64,
    pub grid_spacing: f64,
    /// visual grid line count over the grid length (JS `grid.divisions`)
    pub grid_divisions: i32,
    pub grid_major: i32,
    pub tangent_in: String,
    pub tangent_out: String,
    /// 0 = play every frame, 1 = real-time, 2 = half, 3 = twice
    pub playback_speed: usize,
    pub linear_unit: usize,
    pub angular_unit: usize,
    pub recent_count: usize,
    pub recent_projects_count: usize,
    pub autosave_limit: usize,
    pub incremental_save: bool,
    /// smooth mesh preview subdivision levels (JS `smoothDivisions`)
    pub smooth_divisions: u32,
    /// red pre-selection highlight of the component under the cursor
    pub preselect: bool,
    /// faint guide line along the axis being dragged
    pub manip_guides: bool,
    /// size of the previous-state feedback square points
    pub manip_prev_size: f32,
    /// show the manipulator only in the active viewport (JS `manipActiveOnly`)
    pub manip_active_only: bool,
    /// show the move / scale plane handles (JS `manipPlaneHandles`)
    pub manip_plane_handles: bool,
    /// show the handles on the negative axes (JS `manipNegativeHandles`; off by default, as in Maya)
    pub manip_negative_handles: bool,
    /// Revision of the manipulator defaults these preferences have been migrated to (see
    /// `migrate`). Missing in older saved preferences, so it reads as 0 there.
    #[serde(default)]
    pub manip_defaults_rev: u32,
    /// hide the manipulator while Ctrl is held
    pub hide_manip_ctrl: bool,
    /// hide the manipulator while Shift is held
    pub hide_manip_shift: bool,
    /// hide the manipulator while Ctrl+Shift is held
    pub hide_manip_ctrl_shift: bool,
    /// viewport anti-aliasing samples: 1 (off) or 4
    pub msaa: u32,
    /// subtle menu/popup drop shadow
    pub menu_shadow: bool,
    /// "auto", "dx12", "vulkan", "metal" or "gl"; applied on restart
    pub backend: String,
    /// Saved Attribute Editor attribute presets.
    pub attr_presets: Vec<AttributePreset>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { ui_scale: 1.0, follow_os_scale: false, menu_set: 0, tooltips: true, confirm_quit: true, ortho_tumble: false, grid_size: 12.0, grid_spacing: 1.0, grid_divisions: 12, grid_major: 5, tangent_in: "auto".into(), tangent_out: "auto".into(), playback_speed: 1, linear_unit: 1, angular_unit: 0, recent_count: 10, recent_projects_count: 5, autosave_limit: 10, incremental_save: false, smooth_divisions: 2, preselect: true, manip_guides: true, manip_prev_size: 4.0, manip_active_only: false, manip_plane_handles: true, manip_negative_handles: false, manip_defaults_rev: MANIP_DEFAULTS_REV, hide_manip_ctrl: false, hide_manip_shift: false, hide_manip_ctrl_shift: false, msaa: 4, menu_shadow: true, backend: "auto".into(), attr_presets: vec![] }
    }
}

pub const CATEGORIES: &[&str] = &["Interface", "UI Elements", "Display", "GPU", "Kinematics / Manipulators", "Animation", "Time Slider", "Modeling", "Settings", "Undo", "Files/Projects", "Save"];
const MENU_SETS: [(&str, MenuSet); 5] = [("Modeling", MenuSet::Modeling), ("Rigging", MenuSet::Rigging), ("Animation", MenuSet::Animation), ("FX", MenuSet::Fx), ("Rendering", MenuSet::Rendering)];
const UNITS: [&str; 6] = ["millimeter", "centimeter", "meter", "inch", "foot", "yard"];
const SPEEDS: [(&str, f64); 4] = [("Play every frame", 0.0), ("Real-time [fps]", 1.0), ("Half", 0.5), ("Twice", 2.0)];
const FPS: [(&str, f64); 6] = [("24 fps (Film)", 24.0), ("25 fps (PAL)", 25.0), ("30 fps (NTSC)", 30.0), ("48 fps (Show)", 48.0), ("50 fps (PAL Field)", 50.0), ("60 fps (NTSC Field)", 60.0)];

impl Prefs {
    /// Real-time playback multiplier (None = play every frame).
    pub fn speed(&self) -> Option<f64> {
        let s = SPEEDS[self.playback_speed.min(3)].1;
        (s > 0.0).then_some(s)
    }

    /// Bring saved preferences up to the current manipulator defaults, once. Revision 1 turns
    /// the negative axis handles off, as Maya shows none; turning them back on afterwards sticks.
    pub fn migrate(&mut self) {
        if self.manip_defaults_rev < 1 {
            self.manip_negative_handles = false;
        }
        self.manip_defaults_rev = self.manip_defaults_rev.max(MANIP_DEFAULTS_REV);
    }
}

#[cfg(test)]
mod manip_defaults_tests {
    use super::*;

    #[test]
    fn negative_handles_default_off_and_migrate_once() {
        assert!(!Prefs::default().manip_negative_handles);
        // preferences saved before the revision existed: the old default (on) is switched off
        let mut old: Prefs = serde_json::from_str(r#"{"manip_negative_handles": true, "grid_size": 20.0}"#).unwrap();
        assert_eq!(old.manip_defaults_rev, 0);
        old.migrate();
        assert!(!old.manip_negative_handles);
        assert_eq!(old.manip_defaults_rev, MANIP_DEFAULTS_REV);
        assert_eq!(old.grid_size, 20.0, "other saved settings are kept");
        // turned back on after the migration: kept
        let mut on: Prefs = serde_json::from_str(r#"{"manip_negative_handles": true, "manip_defaults_rev": 1}"#).unwrap();
        on.migrate();
        assert!(on.manip_negative_handles);
    }
}

fn backend_path() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Inca")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Inca")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
            .join("inca")
    };
    Some(dir.join("backend.txt"))
}

pub fn read_backend() -> String {
    let Some(p) = backend_path() else { return "auto".into() };
    let Ok(s) = std::fs::read_to_string(p) else { return "auto".into() };
    match s.trim().to_ascii_lowercase().as_str() {
        "dx12" | "vulkan" | "metal" | "gl" => s.trim().to_ascii_lowercase(),
        _ => "auto".into(),
    }
}

pub fn write_backend(v: &str) {
    let Some(p) = backend_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, v);
}

/// Must run before eframe creates the wgpu instance.
pub fn apply_backend_env() {
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    let b = read_backend();
    if b != "auto" {
        std::env::set_var("WGPU_BACKEND", b);
    }
}

/// Apply the settings that act every frame (UI scale, tooltips, grid, list sizes, time).
pub fn apply(app: &mut IncaApp, ctx: &egui::Context) {
    // like the JS, the menu set in use is the one the next session starts with
    app.prefs.menu_set = MENU_SETS.iter().position(|m| m.1 == app.menu_set).unwrap_or(0);
    app.prefs.msaa = match app.prefs.msaa {
        1 | 4 => app.prefs.msaa,
        _ => 4,
    };
    if app.gpu.as_ref().is_some_and(|g| g.msaa() != app.prefs.msaa) {
        if let Some(rs) = app.gpu_rs.clone() {
            app.gpu = Some(crate::viewport::gpu::Gpu::new(&rs, app.prefs.msaa));
            app.vp_state.render_keys = [None; 4];
        }
    }
    let p = &app.prefs;
    // Smaller blur/spread than egui's default shadows; off removes the overdraw.
    let shadow = if p.menu_shadow {
        egui::epaint::Shadow { offset: [2, 3], blur: 6, spread: 1, color: egui::Color32::from_black_alpha(90) }
    } else {
        egui::epaint::Shadow::NONE
    };
    let style = ctx.global_style();
    if style.visuals.popup_shadow != shadow || style.visuals.window_shadow != shadow {
        ctx.global_style_mut(|s| {
            s.visuals.popup_shadow = shadow;
            s.visuals.window_shadow = shadow;
        });
    }
    let native = ctx.native_pixels_per_point().unwrap_or(1.0);
    let target = if p.follow_os_scale { p.ui_scale } else { p.ui_scale / native };
    if (ctx.zoom_factor() - target).abs() > 1e-4 {
        ctx.set_zoom_factor(target.clamp(0.25, 2.0));
    }
    let delay = if p.tooltips { 0.5 } else { f32::INFINITY };
    if ctx.global_style().interaction.tooltip_delay != delay {
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = delay);
    }
    for v in app.vps.iter_mut() {
        // the JS grid divides the length into `divisions` lines; `spacing` only feeds snapping
        let step = (p.grid_size / p.grid_divisions.max(1) as f64).max(0.001);
        v.grid_cfg = (p.grid_size.max(0.01), step, p.grid_major.max(1));
    }
    app.recent.truncate(p.recent_count.max(1));
    app.recent_projects.truncate(p.recent_projects_count.max(1));
    app.s.time.play_every = p.playback_speed == 0;
    app.s.key_tangents = (p.tangent_in.clone(), p.tangent_out.clone());
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    add(ui);
    ui.end_row();
}

/// Boolean preference row: the checkbox carries its own label, Maya-style.
fn bool_row(ui: &mut egui::Ui, label: &str, value: &mut bool) {
    ui.checkbox(value, label);
    ui.end_row();
}

fn combo(ui: &mut egui::Ui, id: &str, cur: &mut usize, opts: &[&str]) {
    egui::ComboBox::from_id_salt(id).selected_text(opts[(*cur).min(opts.len() - 1)]).show_ui(ui, |ui| {
        for (i, o) in opts.iter().enumerate() {
            ui.selectable_value(cur, i, *o);
        }
    });
}

fn tangent(ui: &mut egui::Ui, id: &str, cur: &mut String) {
    egui::ComboBox::from_id_salt(id).selected_text(cur.as_str()).show_ui(ui, |ui| {
        for t in inca_core::anim::TANGENTS {
            ui.selectable_value(cur, t.to_string(), *t);
        }
    });
}

pub fn window(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.win.prefs;
    if !open {
        return;
    }
    let _ = crate::windows::place_window(app, ctx, egui::Id::new("Preferences"), open, |app, fix| {
        let mut window = egui::Window::new("Preferences").open(&mut open).default_size([640.0, 440.0]).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    ui.label(RichText::new("Categories").strong());
                    for (i, c) in CATEGORIES.iter().enumerate() {
                        ui.selectable_value(&mut app.prefs_cat, i, *c);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| category(app, ui));
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Restore Default Settings").clicked() {
                            app.prefs = Prefs::default();
                        }
                    });
                });
            });
        })
    });
    app.win.prefs = open;
}

fn category(app: &mut IncaApp, ui: &mut egui::Ui) {
    let cat = CATEGORIES[app.prefs_cat.min(CATEGORIES.len() - 1)];
    egui::Grid::new(("prefs", cat)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| match cat {
        "Interface" => {
            theme::section_header(ui, "Interface");
            ui.end_row();
            row(ui, "Interface scaling", |ui| {
                ui.add(egui::Slider::new(&mut app.prefs.ui_scale, 0.8..=1.5).step_by(0.05));
            });
            bool_row(ui, "Follow OS display scaling", &mut app.prefs.follow_os_scale);
            row(ui, "Default menu set", |ui| {
                let names: Vec<&str> = MENU_SETS.iter().map(|m| m.0).collect();
                combo(ui, "pms", &mut app.prefs.menu_set, &names);
                app.menu_set = MENU_SETS[app.prefs.menu_set.min(4)].1;
            });
            bool_row(ui, "Show tooltips", &mut app.prefs.tooltips);
            bool_row(ui, "Drop shadow", &mut app.prefs.menu_shadow);
            bool_row(ui, "Ask before quitting with unsaved changes", &mut app.prefs.confirm_quit);
        }
        "UI Elements" => {
            theme::section_header(ui, "Visible UI Elements");
            ui.end_row();
            for (k, l) in crate::workspace::ELEMENTS {
                let mut on = app.shown(k);
                if ui.checkbox(&mut on, *l).changed() {
                    app.set_shown(k, on);
                }
                ui.end_row();
            }
        }
        "Display" => {
            theme::section_header(ui, "Viewport");
            ui.end_row();
            row(ui, "Background color", |ui| {
                let names: Vec<&str> = crate::viewport::BG_MODES.to_vec();
                combo(ui, "pbg", &mut app.bg_mode, &names);
            });
            bool_row(ui, "Allow tumbling orthographic views", &mut app.prefs.ortho_tumble);
            theme::section_header(ui, "Grid");
            ui.end_row();
            row(ui, "Length and width", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.grid_size).speed(0.5).range(0.01..=10000.0).suffix(" units"));
            });
            row(ui, "Grid lines every", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.grid_spacing).speed(0.1).range(0.001..=1000.0).suffix(" units (snapping)"));
            });
            row(ui, "Subdivisions", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.grid_divisions).range(1..=200));
            });
            row(ui, "Major line every", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.grid_major).range(1..=100).suffix(" subdivisions"));
            });
            theme::section_header(ui, "Heads Up Display");
            ui.end_row();
            let h = &mut app.hud;
            for (l, v) in [("Poly count", &mut h.poly_count), ("Frame rate", &mut h.fps), ("Current frame", &mut h.frame), ("Camera names", &mut h.cam_names), ("View axis", &mut h.view_axis)] {
                ui.checkbox(v, l);
                ui.end_row();
            }
        }
        "GPU" => {
            theme::section_header(ui, "Graphics Device");
            ui.end_row();
            if let Some(rs) = &app.gpu_rs {
                let i = rs.adapter.get_info();
                row(ui, "Rendering engine", |ui| {
                    ui.label(format!("{:?}", i.backend));
                });
                row(ui, "Adapter", |ui| {
                    ui.label(&i.name);
                });
                row(ui, "Device type", |ui| {
                    ui.label(format!("{:?}", i.device_type));
                });
                row(ui, "Driver", |ui| {
                    ui.label(if i.driver_info.is_empty() { i.driver.clone() } else { i.driver_info.clone() });
                });
            } else {
                row(ui, "Renderer", |ui| {
                    ui.label("wgpu unavailable");
                });
            }
            let (names, values): (&[&str], &[&str]) = if cfg!(windows) {
                (&["Automatic", "DirectX 12", "Vulkan"], &["auto", "dx12", "vulkan"])
            } else if cfg!(target_os = "macos") {
                (&["Automatic", "Metal"], &["auto", "metal"])
            } else {
                (&["Automatic", "Vulkan", "OpenGL"], &["auto", "vulkan", "gl"])
            };
            let mut backend_idx = values.iter().position(|v| *v == app.prefs.backend).unwrap_or(0);
            row(ui, "Backend preference", |ui| {
                combo(ui, "pbackend", &mut backend_idx, names);
            });
            if values[backend_idx] != app.prefs.backend {
                app.prefs.backend = values[backend_idx].to_string();
                crate::prefs::write_backend(values[backend_idx]);
                let msg = if values[backend_idx] == "auto" {
                    "Inca will choose the viewport rendering backend automatically the next time it starts.".to_string()
                } else {
                    format!("Inca will use {} for the viewport rendering backend after it is restarted.", names[backend_idx])
                };
                app.win.backend_notice = Some(msg);
            }
            row(ui, "Viewport anti-aliasing", |ui| {
                let mut aa = if app.prefs.msaa == 1 { 0 } else { 1 };
                combo(ui, "pmsaa", &mut aa, &["Off", "4x"]);
                app.prefs.msaa = [1, 4][aa.min(1)];
            });
            ui.label("Backend preference applies after restart; WGPU_BACKEND overrides it.");
            ui.end_row();
        }
        "Kinematics / Manipulators" => {
            theme::section_header(ui, "Manipulators");
            ui.end_row();
            row(ui, "Global scale", |ui| {
                ui.add(egui::Slider::new(&mut app.manip_size, 0.2..=4.0));
            });
            bool_row(ui, "Show manipulator guides", &mut app.prefs.manip_guides);
            row(ui, "Previous state size", |ui| {
                ui.add(egui::Slider::new(&mut app.prefs.manip_prev_size, 0.0..=16.0).max_decimals(1));
            });
            bool_row(ui, "Show manipulator in active view only", &mut app.prefs.manip_active_only);
            bool_row(ui, "Show plane handles", &mut app.prefs.manip_plane_handles);
            bool_row(ui, "Show negative axis handles", &mut app.prefs.manip_negative_handles);
            theme::section_header(ui, "Manipulator Visibility");
            ui.end_row();
            bool_row(ui, "Hide Manipulator: When Ctrl is held", &mut app.prefs.hide_manip_ctrl);
            bool_row(ui, "When Shift is held", &mut app.prefs.hide_manip_shift);
            bool_row(ui, "When Ctrl+Shift is held", &mut app.prefs.hide_manip_ctrl_shift);
        }
        "Animation" => {
            theme::section_header(ui, "Tangents");
            ui.end_row();
            row(ui, "Default in tangent", |ui| tangent(ui, "pti", &mut app.prefs.tangent_in));
            row(ui, "Default out tangent", |ui| tangent(ui, "pto", &mut app.prefs.tangent_out));
        }
        "Time Slider" => {
            theme::section_header(ui, "Playback");
            ui.end_row();
            row(ui, "Playback speed", |ui| {
                let names: Vec<&str> = SPEEDS.iter().map(|s| s.0).collect();
                combo(ui, "pps", &mut app.prefs.playback_speed, &names);
            });
            row(ui, "Looping", |ui| {
                let opts = ["once", "oscillate", "continuous"];
                let mut i = opts.iter().position(|o| *o == app.s.time.r#loop).unwrap_or(2);
                combo(ui, "plp", &mut i, &["Once", "Oscillate", "Continuous"]);
                app.s.time.r#loop = opts[i].into();
            });
            theme::section_header(ui, "Time Range");
            ui.end_row();
            let t = &mut app.s.time;
            for (l, v) in [("Playback start", &mut t.start), ("Playback end", &mut t.end), ("Animation start", &mut t.anim_start), ("Animation end", &mut t.anim_end)] {
                ui.label(l);
                ui.add(egui::DragValue::new(v));
                ui.end_row();
            }
        }
        "Modeling" => {
            theme::section_header(ui, "Smooth Mesh Preview");
            ui.end_row();
            row(ui, "Subdivision levels", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.smooth_divisions).range(1..=4));
            });
            theme::section_header(ui, "Construction History");
            ui.end_row();
            bool_row(ui, "Construction history on by default", &mut app.history_on);
        }
        "Settings" => {
            theme::section_header(ui, "Working Units");
            ui.end_row();
            row(ui, "Linear", |ui| combo(ui, "plu", &mut app.prefs.linear_unit, &UNITS));
            row(ui, "Angular", |ui| combo(ui, "pau", &mut app.prefs.angular_unit, &["degrees", "radians"]));
            row(ui, "Time", |ui| {
                let names: Vec<&str> = FPS.iter().map(|f| f.0).collect();
                let mut i = FPS.iter().position(|f| f.1 == app.s.time.fps).unwrap_or(0);
                combo(ui, "pfps", &mut i, &names);
                app.s.time.fps = FPS[i].1;
            });
        }
        "Undo" => {
            theme::section_header(ui, "Undo");
            ui.end_row();
            row(ui, "Queue size (levels)", |ui| {
                ui.add(egui::DragValue::new(&mut app.s.undo_limit).range(1..=1000));
            });
        }
        "Files/Projects" => {
            theme::section_header(ui, "Recent Files");
            ui.end_row();
            row(ui, "Recent files list size", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.recent_count).range(1..=30));
            });
            row(ui, "Recent projects list size", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.recent_projects_count).range(1..=30));
            });
        }
        "Save" => {
            theme::section_header(ui, "Auto Save");
            ui.end_row();
            bool_row(ui, "Enable auto save", &mut app.autosave.0);
            row(ui, "Interval", |ui| {
                ui.add(egui::DragValue::new(&mut app.autosave.1).speed(0.2).range(1.0..=600.0).suffix(" minutes"));
            });
            row(ui, "Limit auto saves", |ui| {
                ui.add(egui::DragValue::new(&mut app.prefs.autosave_limit).range(1..=100));
            });
            theme::section_header(ui, "Save");
            ui.end_row();
            bool_row(ui, "Incremental save", &mut app.prefs.incremental_save);
        }
        _ => {}
    });
}

/// The menu set chosen as the default in Preferences.
pub fn default_menu_set(p: &Prefs) -> MenuSet {
    MENU_SETS[p.menu_set.min(4)].1
}
