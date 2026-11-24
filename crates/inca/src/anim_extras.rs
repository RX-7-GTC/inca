//! Animation > Visualize (Ghost Selected, Motion Trail) and Playblast.

use crate::app::IncaApp;
use crate::viewport::gpu::{DrawCmd, DrawUniform, Geo, LineVertex, MeshVertex, Pass};
use crate::viewport::CamView;
use egui::{vec2, Color32, Rect};
use glam::{DVec3, Mat4};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

/// A ghost: world-space triangles of a mesh at another frame.
pub struct Ghost {
    tris: Vec<MeshVertex>,
    alpha: f32,
}

/// How layer Ghost buttons choose the objects to ghost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayerGhostMode {
    /// Selected objects on the ghosted layer.
    #[default]
    AutoSelected,
    /// Every object belonging to the ghosted layer.
    AutoLayer,
    /// Objects manually ghosted with Visualize > Ghost Selected.
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhostType {
    #[default]
    GlobalPreferences,
    CustomFrames,
    CustomFrameSteps,
    CustomKeySteps,
    Keyframes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhostRange {
    #[default]
    TimeSlider,
    StartEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GhostOptions {
    pub ghost_type: GhostType,
    pub frames: String,
    pub steps_before: i32,
    pub steps_after: i32,
    pub step_size: i32,
    pub range: GhostRange,
    pub start: f64,
    pub end: f64,
    pub hierarchy: bool,
    pub use_ghost_driver: bool,
    pub geometry: bool,
    pub locator: bool,
    pub joint: bool,
    pub other: bool,
}

impl Default for GhostOptions {
    fn default() -> Self {
        Self {
            ghost_type: GhostType::GlobalPreferences,
            frames: "1,10,20".into(),
            steps_before: 3,
            steps_after: 3,
            step_size: 1,
            range: GhostRange::TimeSlider,
            start: 1.0,
            end: 100.0,
            hierarchy: true,
            use_ghost_driver: false,
            geometry: true,
            locator: true,
            joint: true,
            other: false,
        }
    }
}

#[derive(Default)]
pub struct AnimViz {
    pub ghosts: Vec<Ghost>,
    /// Object roots explicitly ghosted with Visualize > Ghost Selected.
    pub ghost_objs: Vec<String>,
    /// The objects selected by Ghost Options (hierarchy and object types applied).
    ghost_targets: Vec<String>,
    /// Hide every ghost without clearing the ghost sets.
    pub ghosts_hidden: bool,
    /// Layers evaluated up to a ghosted layer, rebuilt when the scene or time changes.
    pub layer_ghosts: Vec<Ghost>,
    /// world positions per frame, one trail per object
    pub trails: Vec<Vec<DVec3>>,
    ghost_rev: u64,
    ghost_time: f64,
    /// Selection used by the last BaseAnimation ghost build.
    ghost_sel: Vec<String>,
    ghost_targets_sel: Vec<String>,
    ghost_mode: Option<LayerGhostMode>,
    /// Bumped when the ghost geometry changes, so the viewport texture is redrawn.
    pub ghost_gen: u64,
}

/// World-space triangles for the objects at their current evaluated transforms.
fn capture(app: &IncaApp, objs: &[String], col: [f32; 3], alpha: f32) -> Vec<Ghost> {
    let mut out = vec![];
    for o in objs {
        let wm = app.s.world_matrix(o);
        let mut tris = vec![];
        if let Some(pm) = app.s.node(o).and_then(|n| n.mesh()).map(|m| &m.mesh) {
            for fi in 0..pm.f.len() {
                for tr in pm.triangulate_face(fi) {
                    for &c in &tr {
                        let p = wm.transform_point3(pm.v[pm.f[fi][c] as usize]);
                        tris.push(MeshVertex { pos: [p.x as f32, p.y as f32, p.z as f32], normal: [0.0, 1.0, 0.0], color: col, ..Default::default() });
                    }
                }
            }
        } else {
            // Joints, locators and other non-mesh nodes ghost as a small pivot diamond.
            let p = wm.transform_point3(DVec3::ZERO);
            let r = 0.08;
            let v = [p + DVec3::X * r, p - DVec3::X * r, p + DVec3::Y * r, p - DVec3::Y * r, p + DVec3::Z * r, p - DVec3::Z * r];
            for f in [[0, 2, 4], [2, 1, 4], [1, 3, 4], [3, 0, 4], [2, 0, 5], [1, 2, 5], [3, 1, 5], [0, 3, 5]] {
                for idx in f {
                    let q = v[idx];
                    tris.push(MeshVertex { pos: [q.x as f32, q.y as f32, q.z as f32], normal: [0.0, 1.0, 0.0], color: col, ..Default::default() });
                }
            }
        }
        out.push(Ghost { tris, alpha });
    }
    out
}

/// Re-evaluate the scene at `t` (keys, constraints, rigs and the meshes that follow them).
fn goto(app: &mut IncaApp, t: f64) {
    app.s.set_time(t);
    let moved = app.s.eval_constraints();
    app.s.refresh_many(&moved);
}

fn selection_roots(app: &IncaApp) -> Vec<String> {
    let mut roots = app.s.sel.clone();
    roots.sort();
    roots.dedup();
    roots
}

fn ghost_targets_from_roots(app: &IncaApp, roots: &[String], opts: &GhostOptions) -> Vec<String> {
    let mut out = vec![];
    for root in roots {
        let ids = if opts.hierarchy { app.s.subtree(root) } else { vec![root.clone()] };
        for id in ids {
            let Some(n) = app.s.node(&id) else { continue };
            let keep = if n.mesh().is_some() {
                opts.geometry
            } else if n.kind == "joint" {
                opts.joint
            } else if n.kind == "locator" {
                opts.locator
            } else {
                opts.other
            };
            if keep {
                out.push(id);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn parse_ghost_frames(text: &str) -> Vec<f64> {
    text.split(',').filter_map(|s| s.trim().parse::<f64>().ok()).collect()
}

fn ghost_times(app: &IncaApp, targets: &[String], opts: &GhostOptions, t0: f64) -> Vec<f64> {
    let mut times = vec![];
    let (range_start, range_end) = match opts.range {
        GhostRange::TimeSlider => (app.s.time.start, app.s.time.end),
        GhostRange::StartEnd => (opts.start.min(opts.end), opts.start.max(opts.end)),
    };
    let in_range = |t: f64| t >= range_start - 1e-6 && t <= range_end + 1e-6;
    match opts.ghost_type {
        GhostType::GlobalPreferences => {
            for i in 1..=3 {
                times.push(t0 - i as f64);
                times.push(t0 + i as f64);
            }
        }
        GhostType::CustomFrameSteps => {
            let step = opts.step_size.max(1) as f64;
            for i in 1..=opts.steps_before.max(0) {
                times.push(t0 - i as f64 * step);
            }
            for i in 1..=opts.steps_after.max(0) {
                times.push(t0 + i as f64 * step);
            }
        }
        GhostType::CustomKeySteps | GhostType::Keyframes => {
            let keyed: Vec<String> = if opts.use_ghost_driver { targets.first().cloned().into_iter().collect() } else { targets.to_vec() };
            let mut ks = app.s.anim_key_times(&keyed);
            ks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            ks.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            if opts.ghost_type == GhostType::Keyframes {
                times.extend(ks.into_iter().filter(|t| in_range(*t)));
            } else {
                let step = opts.step_size.max(1) as usize;
                let before: Vec<f64> = ks.iter().copied().filter(|t| *t < t0).collect();
                let after: Vec<f64> = ks.iter().copied().filter(|t| *t > t0).collect();
                times.extend(before.iter().rev().step_by(step).take(opts.steps_before.max(0) as usize));
                times.extend(after.iter().step_by(step).take(opts.steps_after.max(0) as usize));
            }
        }
        GhostType::CustomFrames => times.extend(parse_ghost_frames(&opts.frames)),
    }
    times.retain(|t| in_range(*t) && (*t - t0).abs() > 1e-6);
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    times
}

fn rebuild_selected_ghosts(app: &mut IncaApp) {
    let t0 = app.s.time.current;
    app.viz.ghost_objs.retain(|i| app.s.node(i).is_some());
    app.viz.ghost_objs.sort();
    app.viz.ghost_objs.dedup();
    let opts = app.panels.ghost_options.clone();
    let targets = ghost_targets_from_roots(app, &app.viz.ghost_objs, &opts);
    app.viz.ghost_targets = targets.clone();
    if targets.is_empty() {
        app.viz.ghosts.clear();
        return;
    }
    let times = ghost_times(app, &targets, &opts, t0);
    app.viz.ghosts.clear();
    for t in times {
        goto(app, t);
        let col = if t < t0 { [0.33, 0.47, 1.0] } else { [1.0, 0.47, 0.33] };
        let alpha = 0.18 + 0.1 / (1.0 + (t - t0).abs() as f32 / 10.0);
        app.viz.ghosts.extend(capture(app, &targets, col, alpha));
    }
    goto(app, t0);
}

/// Ghost Selected: add the selected object roots to the persistent ghost set and rebuild.
pub fn ghost(app: &mut IncaApp) {
    let roots = selection_roots(app);
    if roots.is_empty() {
        app.last_result = "// Warning: Select animated objects to ghost".into();
        return;
    }
    let opts = app.panels.ghost_options.clone();
    if ghost_targets_from_roots(app, &roots, &opts).is_empty() {
        app.last_result = "// Warning: No selected objects match the Ghost Options".into();
        return;
    }
    app.viz.ghost_objs.extend(roots);
    rebuild_selected_ghosts(app);
    app.echo("ghosting;");
}

/// Unghost Selected: remove the selected object roots from the persistent ghost set.
pub fn unghost_selected(app: &mut IncaApp) {
    let roots = selection_roots(app);
    if roots.is_empty() {
        return;
    }
    app.viz.ghost_objs.retain(|o| !roots.contains(o));
    rebuild_selected_ghosts(app);
    app.echo("unghostSelected;");
}

pub fn unghost(app: &mut IncaApp) {
    app.viz.ghost_objs.clear();
    app.viz.ghost_targets.clear();
    app.viz.ghosts.clear();
    app.echo("unghostAll;");
}

/// Rebuild ghosts after changing Ghost Options without adding a new selection.
pub fn rebuild_ghosts(app: &mut IncaApp) {
    rebuild_selected_ghosts(app);
}

fn ghost_type_label(t: GhostType) -> &'static str {
    match t {
        GhostType::GlobalPreferences => "Global preferences",
        GhostType::CustomFrames => "Custom frames",
        GhostType::CustomFrameSteps => "Custom frame steps",
        GhostType::CustomKeySteps => "Custom key steps",
        GhostType::Keyframes => "Keyframes",
    }
}

/// Visualize > Ghost Selected > options box.
pub fn options_window(app: &mut IncaApp, ctx: &egui::Context) {
    if !app.panels.ghost_options_open {
        return;
    }
    let mut open = true;
    let is_open = open;
    let mut opts = app.panels.ghost_options.clone();
    let mut ghost_now = false;
    let mut apply = false;
    let mut close = false;
    let _ = crate::windows::place_window(app, ctx, egui::Id::new("GhostSelectedOptions"), is_open, |_app, fix| {
        let mut window = egui::Window::new("Ghost Options").id(egui::Id::new("GhostSelectedOptions")).open(&mut open).collapsible(false).resizable(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
            egui::Grid::new("ghost_options_grid").num_columns(2).spacing([10.0, 5.0]).show(ui, |ui| {
                ui.label("Type of ghosting");
                egui::ComboBox::from_id_salt("ghost_type").selected_text(ghost_type_label(opts.ghost_type)).show_ui(ui, |ui| {
                    for t in [GhostType::GlobalPreferences, GhostType::CustomFrames, GhostType::CustomFrameSteps, GhostType::CustomKeySteps, GhostType::Keyframes] {
                        ui.selectable_value(&mut opts.ghost_type, t, ghost_type_label(t));
                    }
                });
                ui.end_row();
                ui.label("Frames to display");
                ui.add_enabled(opts.ghost_type == GhostType::CustomFrames, egui::TextEdit::singleline(&mut opts.frames).desired_width(180.0));
                ui.end_row();
                let steps = matches!(opts.ghost_type, GhostType::CustomFrameSteps | GhostType::CustomKeySteps);
                ui.label("Steps before current frame");
                ui.add_enabled(steps, egui::DragValue::new(&mut opts.steps_before).range(0..=100));
                ui.end_row();
                ui.label("Steps after current frame");
                ui.add_enabled(steps, egui::DragValue::new(&mut opts.steps_after).range(0..=100));
                ui.end_row();
                ui.label("Step size");
                ui.add_enabled(steps, egui::DragValue::new(&mut opts.step_size).range(1..=100));
                ui.end_row();
                let keyed = opts.ghost_type == GhostType::Keyframes;
                ui.label("Frame range");
                ui.add_enabled_ui(keyed, |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut opts.range, GhostRange::TimeSlider, "Time Slider");
                        ui.radio_value(&mut opts.range, GhostRange::StartEnd, "Start/End");
                    });
                });
                ui.end_row();
                let custom_range = keyed && opts.range == GhostRange::StartEnd;
                ui.label("Start frame");
                ui.add_enabled(custom_range, egui::DragValue::new(&mut opts.start).speed(1.0));
                ui.end_row();
                ui.label("End frame");
                ui.add_enabled(custom_range, egui::DragValue::new(&mut opts.end).speed(1.0));
                ui.end_row();
                ui.label("Hierarchy");
                ui.checkbox(&mut opts.hierarchy, "");
                ui.end_row();
                ui.label("Use ghost driver");
                ui.add_enabled(matches!(opts.ghost_type, GhostType::CustomKeySteps | GhostType::Keyframes), egui::Checkbox::new(&mut opts.use_ghost_driver, ""));
                ui.end_row();
                ui.label("Object type");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut opts.geometry, "Geometry");
                    ui.checkbox(&mut opts.locator, "Locator");
                    ui.checkbox(&mut opts.joint, "Joint");
                    ui.checkbox(&mut opts.other, "All other types");
                });
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Ghost").clicked() {
                    ghost_now = true;
                }
                if ui.button("Apply").clicked() {
                    apply = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        })
    });
    app.panels.ghost_options = opts;
    if ghost_now {
        if app.s.sel.is_empty() {
            app.help = "Select objects to ghost".into();
        } else {
            ghost(app);
            app.panels.ghost_options_open = false;
        }
    } else if apply || close || !open {
        rebuild_ghosts(app);
    }
    if close || !open {
        app.panels.ghost_options_open = false;
    }
}

/// Show/Hide ghosts: globally show or hide every ghost without forgetting the ghost sets.
pub fn toggle_ghosts(app: &mut IncaApp) {
    app.viz.ghosts_hidden = !app.viz.ghosts_hidden;
    app.viz.ghost_gen += 1;
}

pub fn ghosts_visible(viz: &AnimViz) -> bool {
    !viz.ghosts_hidden && (!viz.ghosts.is_empty() || !viz.layer_ghosts.is_empty())
}

/// Rebuild the ghosts for the layers (and BaseAnimation) with their Ghost button on.
/// Each ghost is the result up to and including that layer at the current frame.
pub fn layer_ghost_tick(app: &mut IncaApp) {
    let any = app.s.anim_base_ghost || app.s.anim_layers.iter().any(|l| app.s.anim_layer_ghost(&l.id));
    let t = app.s.time.current;
    let sel = app.s.sel.clone();
    let mode = app.panels.layer_ghost_mode;
    let opts = app.panels.ghost_options.clone();
    let manual = app.viz.ghost_targets.clone();
    if !any {
        if !app.viz.layer_ghosts.is_empty() {
            app.viz.layer_ghosts.clear();
            app.viz.ghost_gen += 1;
        }
        app.viz.ghost_rev = app.s.rev;
        app.viz.ghost_time = t;
        app.viz.ghost_sel = sel;
        app.viz.ghost_targets_sel = manual;
        app.viz.ghost_mode = Some(mode);
        return;
    }
    if app.viz.ghost_rev == app.s.rev && app.viz.ghost_time.to_bits() == t.to_bits() && app.viz.ghost_sel == sel && app.viz.ghost_targets_sel == manual && app.viz.ghost_mode == Some(mode) {
        return;
    }
    let is_mesh = |i: &String| app.s.node(i).is_some_and(|n| n.mesh().is_some());
    let selected_targets = ghost_targets_from_roots(app, &app.s.sel, &opts);
    let base_objs: Vec<String> = if app.s.anim_base_ghost { selected_targets.clone() } else { vec![] };
    // Layer ghosts follow the live selection in every mode; Manual differs only in how normal
    // Ghost Selected objects are collected.
    let allowed: Option<&Vec<String>> = match mode {
        LayerGhostMode::AutoLayer => None,
        LayerGhostMode::AutoSelected | LayerGhostMode::Manual => Some(&selected_targets),
    };
    let layers: Vec<(usize, [f32; 3], Vec<String>)> = app
        .s
        .anim_layers
        .iter()
        .enumerate()
        .filter(|(_, l)| app.s.anim_layer_ghost(&l.id))
        .filter_map(|(i, l)| {
            let mut nodes = l.members.clone();
            nodes.extend(l.curves.values().map(|c| c.node.clone()));
            nodes.sort();
            nodes.dedup();
            let mut objs = vec![];
            for n in &nodes {
                objs.extend(app.s.subtree(n).into_iter().filter(is_mesh));
            }
            objs.sort();
            objs.dedup();
            if let Some(allowed) = allowed {
                objs.retain(|o| allowed.contains(o));
            }
            (!objs.is_empty()).then_some((i, app.s.anim_layer_ghost_color(&l.id), objs))
        })
        .collect();
    if base_objs.is_empty() && layers.is_empty() {
        if !app.viz.layer_ghosts.is_empty() {
            app.viz.layer_ghosts.clear();
            app.viz.ghost_gen += 1;
        }
    } else {
        // Ghost capture evaluates the scene at intermediate layer states. Save the live pose so
        // a manual transform made since the last time change (a gizmo drag, a typed value) is
        // not overwritten when the scene returns to the current frame.
        let live: Vec<(String, inca_core::scene::Xform, bool)> = app.s.nodes.iter().map(|(id, n)| (id.clone(), n.xf.clone(), n.visible)).collect();
        let mut out = vec![];
        if app.s.anim_base_ghost && !base_objs.is_empty() {
            app.s.apply_anim_upto(None);
            out.extend(capture(app, &base_objs, app.s.anim_base_ghost_color, 0.35));
        }
        for (i, color, objs) in layers {
            app.s.apply_anim_upto(Some(i));
            out.extend(capture(app, &objs, color, 0.35));
        }
        app.s.set_time(t);
        for (id, xf, visible) in live {
            if let Some(n) = app.s.nodes.get_mut(&id) {
                n.xf = xf;
                n.visible = visible;
            }
        }
        app.viz.layer_ghosts = out;
        app.viz.ghost_gen += 1;
    }
    app.viz.ghost_rev = app.s.rev;
    app.viz.ghost_time = t;
    app.viz.ghost_sel = sel;
    app.viz.ghost_targets_sel = manual;
    app.viz.ghost_mode = Some(mode);
}

/// Create Motion Trail: the selected objects' pivots over the playback range.
pub fn motion_trail(app: &mut IncaApp) {
    app.viz.trails.clear();
    let objs = app.s.sel.clone();
    if objs.is_empty() {
        app.last_result = "// Warning: Select animated objects".into();
        return;
    }
    let (t0, a, b) = (app.s.time.current, app.s.time.start, app.s.time.end);
    let mut pts: Vec<Vec<DVec3>> = vec![vec![]; objs.len()];
    let mut t = a;
    while t <= b {
        goto(app, t);
        for (i, o) in objs.iter().enumerate() {
            pts[i].push(app.s.world_pos(o));
        }
        t += 1.0;
    }
    goto(app, t0);
    app.viz.trails = pts;
    app.echo("snapshot -motionTrail 1 -increment 1;");
}

/// Ghost and trail geometry for a viewport.
pub fn draws(viz: &AnimViz) -> Vec<DrawCmd> {
    let mut d = vec![];
    if !viz.ghosts_hidden {
        for g in viz.ghosts.iter().chain(viz.layer_ghosts.iter()) {
            d.push(DrawCmd { pass: Pass::MeshBlend, geo: Geo::TransientTris(g.tris.clone()), u: DrawUniform::new(Mat4::IDENTITY).unlit().alpha(g.alpha) });
        }
    }
    let col = [0.55, 0.85, 1.0, 1.0];
    for tr in &viz.trails {
        let mut l = vec![];
        for w in tr.windows(2) {
            l.push(LineVertex { pos: w[0].as_vec3().to_array(), color: col, ..Default::default() });
            l.push(LineVertex { pos: w[1].as_vec3().to_array(), color: col, ..Default::default() });
        }
        d.push(DrawCmd { pass: Pass::Lines, geo: Geo::TransientLines(l), u: DrawUniform::new(Mat4::IDENTITY) });
    }
    d
}

/// Frame dots along the motion trails.
pub fn overlay(app: &IncaApp, cv: &CamView, rect: Rect, painter: &egui::Painter) {
    for tr in &app.viz.trails {
        for p in tr {
            if let Some(q) = cv.project(*p, rect) {
                painter.rect_filled(Rect::from_center_size(q, vec2(3.0, 3.0)), 0.0, Color32::from_rgb(140, 215, 255));
            }
        }
    }
}

// ---------------------------------------------------------------- Playblast

#[derive(Clone, PartialEq)]
pub struct PlayblastOpts {
    pub slider: bool,
    pub start: i64,
    pub end: i64,
    pub scale: u32,
    /// `webm` (the JS default) or `png`
    pub format: String,
    pub path: String,
}

impl Default for PlayblastOpts {
    fn default() -> Self {
        PlayblastOpts { slider: true, start: 1, end: 120, scale: 100, format: "webm".into(), path: String::new() }
    }
}

/// Raw frames piped to ffmpeg, which encodes the WebM in the background.
struct Encoder {
    child: Option<Child>,
    sink: Option<ChildStdin>,
}

/// A running playblast: one screenshot per frame, cropped to the active viewport.
pub struct Playblast {
    frame: i64,
    end: i64,
    restore: f64,
    out: PathBuf,
    scale: u32,
    waiting: bool,
    saved: usize,
    video: bool,
    fps: f64,
    encoder: Option<Encoder>,
    error: Option<String>,
}

fn ffmpeg_exe() -> String {
    std::env::var("INCA_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

/// The JS falls back with a warning when MediaRecorder is unavailable; here WebM needs ffmpeg.
fn ffmpeg_available() -> bool {
    Command::new(ffmpeg_exe())
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Open the encoder for `size`, piping raw RGBA frames on stdin (JS MediaRecorder equivalent).
fn start_encoder(out: &Path, size: [u32; 2], fps: f64) -> std::io::Result<Encoder> {
    let bitrate = ((size[0] * size[1]) as f64 * fps * 0.25).max(2_000_000.0) as u64;
    let mut child = Command::new(ffmpeg_exe())
        .args([
            "-y", "-loglevel", "error", "-f", "rawvideo", "-pixel_format", "rgba", "-video_size", &format!("{}x{}", size[0], size[1]), "-framerate", &format!("{fps}"), "-i", "-",
            "-an", "-c:v", "libvpx-vp9", "-pix_fmt", "yuv420p", "-b:v", &bitrate.to_string(),
        ])
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let sink = child.stdin.take();
    Ok(Encoder { child: Some(child), sink })
}

/// Close stdin and wait for the encoder off the UI thread.
fn finish_encoder(enc: Encoder) {
    let Encoder { child, sink } = enc;
    drop(sink);
    if let Some(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Marks screenshots taken for a playblast (the test hook ignores them).
pub struct PlayblastShot;

#[derive(Default)]
pub struct PlayblastUi {
    pub open: bool,
    pub opts: PlayblastOpts,
    pub running: Option<Playblast>,
    pub status: String,
}

fn frame_path(out: &std::path::Path, f: i64) -> PathBuf {
    let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("playblast".into());
    let stem = stem.trim_end_matches(".####").to_string();
    out.with_file_name(format!("{stem}.{f:04}.png"))
}

/// The JS appends `.webm` when a video target has any other extension.
fn video_path(out: PathBuf) -> PathBuf {
    if out.to_string_lossy().to_lowercase().ends_with(".webm") {
        out
    } else {
        PathBuf::from(format!("{}.webm", out.display()))
    }
}

pub fn start(app: &mut IncaApp) {
    let o = app.playblast.opts.clone();
    let video = o.format == "webm";
    if video && !ffmpeg_available() {
        app.warn("Video recording is not supported here (ffmpeg was not found); use a PNG image sequence.");
        return;
    }
    let (a, b) = if o.slider { (app.s.time.start as i64, app.s.time.end as i64) } else { (o.start.min(o.end), o.start.max(o.end)) };
    let out = if o.path.trim().is_empty() {
        crate::project::default_dir(app, "movies").join(if video { "playblast.webm" } else { "playblast.png" })
    } else {
        let p = PathBuf::from(o.path.trim());
        if p.is_absolute() { p } else { crate::project::default_dir(app, "movies").join(p) }
    };
    let out = if video { video_path(out) } else { out };
    if let Some(d) = out.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let fps = app.s.time.fps.max(1.0);
    app.echo(format!("playblast -startTime {a} -endTime {b} -format {} -percent {} -filename \"{}\";", if video { "webm" } else { "image" }, o.scale, out.display()));
    app.playblast.running = Some(Playblast { frame: a, end: b, restore: app.s.time.current, out, scale: o.scale, waiting: false, saved: 0, video, fps, encoder: None, error: None });
}

/// Advance a running playblast (called once per frame, before the viewports draw).
pub fn tick(app: &mut IncaApp, ctx: &egui::Context) {
    layer_ghost_tick(app);
    if app.playblast.running.is_none() {
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel(app);
        return;
    }
    let pb = app.playblast.running.as_mut().unwrap();
    if pb.waiting {
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, user_data, .. } if user_data.data.as_ref().is_some_and(|d| d.is::<PlayblastShot>()) => Some(image.clone()),
                _ => None,
            })
        });
        let Some(img) = shot else {
            ctx.request_repaint();
            return;
        };
        let rect = app.vps[app.active_vp].rect;
        let ppp = ctx.pixels_per_point();
        let [w, h] = img.size;
        let x0 = ((rect.min.x * ppp) as usize).min(w);
        let y0 = ((rect.min.y * ppp) as usize).min(h);
        let x1 = ((rect.max.x * ppp) as usize).min(w);
        let y1 = ((rect.max.y * ppp) as usize).min(h);
        let pb = app.playblast.running.as_mut().unwrap();
        if x1 > x0 && y1 > y0 {
            let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
            for y in y0..y1 {
                for x in x0..x1 {
                    px.extend_from_slice(&img.pixels[y * w + x].to_array());
                }
            }
            let mut buf = image::RgbaImage::from_raw((x1 - x0) as u32, (y1 - y0) as u32, px).unwrap();
            if pb.video {
                // MediaRecorder dimensions are even; round like the JS capture canvas.
                let k = pb.scale as f32 / 100.0;
                let nw = (((x1 - x0) as f32 * k / 2.0).round() as u32 * 2).max(2);
                let nh = (((y1 - y0) as f32 * k / 2.0).round() as u32 * 2).max(2);
                if buf.width() != nw || buf.height() != nh {
                    buf = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Triangle);
                }
                let size = [buf.width(), buf.height()];
                if pb.encoder.is_none() {
                    match start_encoder(&pb.out, size, pb.fps) {
                        Ok(enc) => pb.encoder = Some(enc),
                        Err(e) => pb.error = Some(format!("Playblast failed: could not start ffmpeg ({e})")),
                    }
                }
                if let Some(enc) = pb.encoder.as_mut() {
                    let written = enc.sink.as_mut().is_some_and(|sink| sink.write_all(buf.as_raw()).and_then(|_| sink.flush()).is_ok());
                    if written {
                        pb.saved += 1;
                    } else {
                        pb.error = Some("Playblast failed: ffmpeg stopped accepting frames".into());
                    }
                }
            } else {
                if pb.scale < 100 {
                    let k = pb.scale as f32 / 100.0;
                    let (nw, nh) = (((x1 - x0) as f32 * k).max(2.0) as u32, ((y1 - y0) as f32 * k).max(2.0) as u32);
                    buf = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Triangle);
                }
                if buf.save(frame_path(&pb.out, pb.frame)).is_ok() {
                    pb.saved += 1;
                }
            }
        }
        pb.waiting = false;
        pb.frame += 1;
    }
    if let Some(err) = app.playblast.running.as_ref().and_then(|p| p.error.clone()) {
        if let Some(pb) = app.playblast.running.take() {
            if let Some(enc) = pb.encoder {
                finish_encoder(enc);
            }
            goto(app, pb.restore);
        }
        app.warn(err);
        return;
    }
    let pb = app.playblast.running.as_mut().unwrap();
    if pb.frame > pb.end {
        let (restore, saved, out, video, enc) = (pb.restore, pb.saved, pb.out.clone(), pb.video, pb.encoder.take());
        app.playblast.running = None;
        if let Some(enc) = enc {
            finish_encoder(enc);
        }
        goto(app, restore);
        app.playblast.status = format!("Saved {saved} frames");
        app.last_result = if video { format!("// Result: {}", out.display()) } else { format!("// Result: playblast wrote {saved} frames to {}", frame_path(&out, 0).with_file_name("").display()) };
        return;
    }
    let (f, end) = (pb.frame, pb.end);
    pb.waiting = true;
    app.playblast.status = format!("Frame {f}");
    app.help = format!("Playblast: frame {f} of {end} (Esc cancels)");
    goto(app, f as f64);
    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(PlayblastShot)));
    ctx.request_repaint();
}

pub fn cancel(app: &mut IncaApp) {
    if let Some(pb) = app.playblast.running.take() {
        if let Some(enc) = pb.encoder {
            finish_encoder(enc);
        }
        goto(app, pb.restore);
        app.playblast.status = format!("Cancelled after {} frames", pb.saved);
    }
}

pub fn window(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.playblast.open;
    if !open {
        return;
    }
    let (ts, te) = (app.s.time.start as i64, app.s.time.end as i64);
    let mut go = false;
    let mut stop = false;
    let _ = crate::windows::place_window(app, ctx, egui::Id::new("Playblast Options"), open, |app, fix| {
        let mut window = egui::Window::new("Playblast Options").open(&mut open).resizable(false).constrain(false);
        if let Some(p) = fix {
            window = window.fixed_pos(p);
        }
        window.show(ctx, |ui| {
        let o = &mut app.playblast.opts;
        egui::Grid::new("pb").num_columns(2).show(ui, |ui| {
            ui.label("Time range");
            ui.horizontal(|ui| {
                ui.radio_value(&mut o.slider, true, "Time Slider");
                ui.radio_value(&mut o.slider, false, "Start/End");
            });
            ui.end_row();
            if o.slider {
                o.start = ts;
                o.end = te;
            }
            ui.label("Start time");
            ui.add_enabled(!o.slider, egui::DragValue::new(&mut o.start));
            ui.end_row();
            ui.label("End time");
            ui.add_enabled(!o.slider, egui::DragValue::new(&mut o.end));
            ui.end_row();
            ui.label("Format");
            let mut format = o.format.clone();
            egui::ComboBox::from_id_salt("pbformat").selected_text(if format == "webm" { "WebM video (.webm)" } else { "PNG image sequence" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut format, "webm".into(), "WebM video (.webm)");
                ui.selectable_value(&mut format, "png".into(), "PNG image sequence");
            });
            if format != o.format {
                o.format = format;
                if !o.path.trim().is_empty() {
                    let stem = Path::new(o.path.trim()).with_extension("");
                    o.path = format!("{}.{}", stem.display(), if o.format == "webm" { "webm" } else { "png" });
                }
            }
            ui.end_row();
            ui.label("Scale");
            egui::ComboBox::from_id_salt("pbscale").selected_text(format!("{}%", o.scale)).show_ui(ui, |ui| {
                for s in [25, 50, 75, 100] {
                    ui.selectable_value(&mut o.scale, s, format!("{s}%"));
                }
            });
            ui.end_row();
            let video = o.format == "webm";
            ui.label("File");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut o.path).hint_text(if video { "<project>/movies/playblast.webm" } else { "<project>/movies/playblast.####.png" }).desired_width(220.0));
                if ui.button("Browse…").clicked() {
                    let dialog = if video {
                        rfd::FileDialog::new().add_filter("WebM Video", &["webm"]).set_file_name("playblast.webm")
                    } else {
                        rfd::FileDialog::new().add_filter("PNG Image", &["png"]).set_file_name("playblast.png")
                    };
                    if let Some(p) = dialog.save_file() {
                        o.path = p.display().to_string();
                    }
                }
            });
            ui.end_row();
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&app.playblast.status).color(crate::theme::TEXT_DIM));
            if app.playblast.running.is_some() {
                stop = ui.button("Cancel").clicked();
            } else {
                go = ui.button("Playblast").clicked();
            }
        });
    })
    });
    if go {
        start(app);
    }
    if stop || !open {
        cancel(app);
    }
    app.playblast.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_targets_get_a_webm_extension() {
        assert_eq!(video_path(PathBuf::from("clip.webm")), PathBuf::from("clip.webm"));
        assert_eq!(video_path(PathBuf::from("clip")), PathBuf::from("clip.webm"));
        assert_eq!(video_path(PathBuf::from("clip.png")), PathBuf::from("clip.png.webm"));
    }

    #[test]
    fn layer_ghosts_capture_the_selected_mesh() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let nid = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(std::slice::from_ref(&nid), inca_core::scene::SelMode::Replace);
        app.s.anim_layers.push(inca_core::anim::AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![nid.clone()],
            attrs: vec![],
            parent: None,
            curves: Default::default(),
        });
        app.s.set_anim_layer_bool("al1", "ghost", true);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty(), "layer ghost was not built");
        app.s.set_anim_layer_bool("al1", "ghost", false);
        app.s.anim_base_ghost = true;
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty(), "BaseAnimation ghost was not built");
        app.s.anim_base_ghost = false;
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(app.viz.layer_ghosts.is_empty(), "ghosts stayed on");
    }

    #[test]
    fn ghost_tick_does_not_reset_a_manual_move() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let nid = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(std::slice::from_ref(&nid), inca_core::scene::SelMode::Replace);
        app.s.set_key(&nid, "translateX", Some(0.0), Some(0.0));
        app.s.set_key(&nid, "translateX", Some(10.0), Some(10.0));
        app.s.set_time(0.0);
        app.s.anim_base_ghost = true;
        app.s.touch();
        layer_ghost_tick(&mut app);
        app.s.set_attr_f(&nid, "translateX", 4.0);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!((app.s.get_attr_f(&nid, "translateX") - 4.0).abs() < 1e-9, "ghost tick reset a manual move");
    }

    #[test]
    fn ghost_selected_accumulates_and_unghosts_selected() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        let b = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        for id in [&a, &b] {
            app.s.set_key(id, "translateX", Some(0.0), Some(0.0));
            app.s.set_key(id, "translateX", Some(10.0), Some(5.0));
        }
        app.s.set_time(5.0);
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        ghost(&mut app);
        assert_eq!(app.viz.ghost_objs, vec![a.clone()]);
        assert!(!app.viz.ghosts.is_empty());
        app.s.select(std::slice::from_ref(&b), inca_core::scene::SelMode::Replace);
        ghost(&mut app);
        assert_eq!(app.viz.ghost_objs, vec![a.clone(), b.clone()], "ghosting another object must add it");
        unghost_selected(&mut app);
        assert_eq!(app.viz.ghost_objs, vec![a.clone()], "Unghost Selected removed the wrong object");
        assert!(!app.viz.ghosts.is_empty(), "remaining ghost was lost");
        unghost(&mut app);
        assert!(app.viz.ghost_objs.is_empty() && app.viz.ghosts.is_empty());
    }

    #[test]
    fn show_hide_ghosts_keeps_the_ghost_set() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        app.s.set_key(&a, "translateX", Some(10.0), Some(5.0));
        app.s.set_time(5.0);
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        ghost(&mut app);
        assert!(ghosts_visible(&app.viz));
        assert!(!draws(&app.viz).is_empty());
        toggle_ghosts(&mut app);
        assert!(!ghosts_visible(&app.viz));
        assert!(draws(&app.viz).is_empty(), "hidden ghosts still draw");
        assert_eq!(app.viz.ghost_objs, vec![a], "Show/Hide must not forget the ghost");
        toggle_ghosts(&mut app);
        assert!(!draws(&app.viz).is_empty(), "shown ghosts did not return");
    }

    #[test]
    fn manual_layer_ghost_uses_ghost_selected_objects() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        app.s.anim_layers.push(inca_core::anim::AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![],
            parent: None,
            curves: Default::default(),
        });
        app.s.set_anim_layer_bool("al1", "ghost", true);
        app.panels.layer_ghost_mode = LayerGhostMode::Manual;
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty(), "Manual mode should fall back to the selection");        ghost(&mut app);
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty(), "Ghost Selected did not enable the manual layer ghost");
        assert!(app.viz.ghost_targets.contains(&a));
    }

    #[test]
    fn switching_to_manual_keeps_existing_auto_ghosts() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        app.s.anim_layers.push(inca_core::anim::AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![],
            parent: None,
            curves: Default::default(),
        });
        app.s.set_anim_layer_bool("al1", "ghost", true);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty());
        app.panels.layer_ghost_mode = LayerGhostMode::Manual;
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty(), "switching to Manual hid the current ghost");
        assert!(app.viz.ghost_objs.is_empty(), "switching to Manual must not freeze the selection");
    }

    #[test]
    fn ghost_options_filter_hierarchy_and_object_types() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let mesh = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        let group = app.s.group(std::slice::from_ref(&mesh), "ghostGroup");
        let locator = app.s.create_locator("ghostLocator");
        app.panels.ghost_options.geometry = true;
        app.panels.ghost_options.locator = false;
        app.panels.ghost_options.joint = false;
        app.panels.ghost_options.other = false;
        app.panels.ghost_options.hierarchy = false;
        app.s.select(std::slice::from_ref(&group), inca_core::scene::SelMode::Replace);
        ghost(&mut app);
        assert!(app.viz.ghost_objs.is_empty(), "hierarchy off should not ghost a group's children");
        app.panels.ghost_options.hierarchy = true;
        ghost(&mut app);
        assert_eq!(app.viz.ghost_targets, vec![mesh.clone()], "hierarchy should ghost the child mesh");
        unghost(&mut app);
        app.s.select(std::slice::from_ref(&locator), inca_core::scene::SelMode::Replace);
        app.panels.ghost_options.locator = true;
        ghost(&mut app);
        assert_eq!(app.viz.ghost_targets, vec![locator.clone()], "locator should be ghosted");
        assert!(!app.viz.ghosts.is_empty(), "locator ghost has no pivot marker");
        unghost(&mut app);
        app.panels.ghost_options.locator = false;
        ghost(&mut app);
        assert!(app.viz.ghost_objs.is_empty(), "disabled locator should not be ghosted");
    }

    #[test]
    fn manual_layer_ghost_falls_back_to_selection() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        app.s.anim_layers.push(inca_core::anim::AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![],
            parent: None,
            curves: Default::default(),
        });
        app.s.set_anim_layer_bool("al1", "ghost", true);
        app.panels.layer_ghost_mode = LayerGhostMode::Manual;
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(!app.viz.layer_ghosts.is_empty(), "Manual mode hid the selected layer object");
    }

    #[test]
    fn manual_layer_ghost_tracks_selection_across_layers() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        let b = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        for (id, member, color) in [("al1", a.clone(), [1.0, 0.0, 0.0]), ("al2", b.clone(), [0.0, 0.0, 1.0])] {
            app.s.anim_layers.push(inca_core::anim::AnimLayer {
                id: id.into(),
                name: id.into(),
                weight_curve: Default::default(),
                members: vec![member],
                attrs: vec![],
                parent: None,
                curves: Default::default(),
            });
            app.s.set_anim_layer_bool(id, "ghost", true);
            app.s.set_anim_layer_ghost_color(id, color);
        }
        app.panels.layer_ghost_mode = LayerGhostMode::Manual;
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert_eq!(app.viz.layer_ghosts.len(), 1, "only the selected layer object should ghost");
        assert_eq!(app.viz.layer_ghosts[0].tris.first().map(|v| v.color), Some([1.0, 0.0, 0.0]));
        app.s.select(std::slice::from_ref(&b), inca_core::scene::SelMode::Replace);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert_eq!(app.viz.layer_ghosts.len(), 1, "the old selection stayed ghosted");
        assert_eq!(app.viz.layer_ghosts[0].tris.first().map(|v| v.color), Some([0.0, 0.0, 1.0]));
        app.s.sel.clear();
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(app.viz.layer_ghosts.is_empty(), "deselecting left the ghost behind");
    }

    #[test]
    fn layer_ghost_ignores_objects_not_in_the_layer() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let a = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        let b = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(&[a.clone(), b.clone()], inca_core::scene::SelMode::Replace);
        app.s.anim_layers.push(inca_core::anim::AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![],
            parent: None,
            curves: Default::default(),
        });
        app.s.set_anim_layer_bool("al1", "ghost", true);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert_eq!(app.viz.layer_ghosts.len(), 1, "ghosted layer captured a selected object outside the layer");
        app.s.select(std::slice::from_ref(&b), inca_core::scene::SelMode::Replace);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(app.viz.layer_ghosts.is_empty(), "selected object outside the layer should have no layer ghost");
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert_eq!(app.viz.layer_ghosts.len(), 1, "layer object should ghost while selected");
        app.s.sel.clear();
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert!(app.viz.layer_ghosts.is_empty(), "layer objects should not ghost when nothing is selected");
        app.panels.layer_ghost_mode = LayerGhostMode::AutoLayer;
        layer_ghost_tick(&mut app);
        assert_eq!(app.viz.layer_ghosts.len(), 1, "Auto Ghost Objects In Layer should ghost unselected layer objects");
        app.panels.layer_ghost_mode = LayerGhostMode::Manual;
        layer_ghost_tick(&mut app);
        assert!(app.viz.layer_ghosts.is_empty(), "Manual mode ghosted an empty selection");
        app.s.select(std::slice::from_ref(&a), inca_core::scene::SelMode::Replace);
        app.s.touch();
        layer_ghost_tick(&mut app);
        assert_eq!(app.viz.layer_ghosts.len(), 1, "Manual mode did not follow the selection");
    }
}
