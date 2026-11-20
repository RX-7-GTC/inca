//! Docked panels: Outliner, Channel Box / Layer Editor, Attribute Editor, Tool Settings.

use crate::app::{IncaApp, RightTab, Tool};
use crate::theme;
use egui::{pos2, vec2, Color32, Rect, RichText};
use inca_core::math::fmt_num;
use inca_core::ops;
use inca_core::primitives::ChanKind;
use inca_core::scene::{Limit, NodeRef, SelMode, Shape, CHANNELS};
use serde_json::{json, Value};

const IDENTITY_OPM: [[f64; 4]; 4] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

#[derive(Clone)]
pub struct CbPick {
    pub targets: Vec<NodeRef>,
    pub attr: String,
    pub int: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// One drawn Channel Box row. `channel_box` collects these while drawing so a drag can select
/// every row between the press row and the row under the pointer.
struct CbRow {
    /// Unique row id: node id (or history node id) and attribute.
    key: (String, String),
    pick: CbPick,
    label: Rect,
    field: Option<Rect>,
    numeric: bool,
    locked: bool,
}

fn cb_is_picked(fields: &[CbPick], first: &NodeRef, attr: &str) -> bool {
    fields.iter().any(|p| p.attr == attr && p.targets.first() == Some(first))
}

fn cb_short_name(attr: &str) -> &str {
    match attr {
        "translateX" => "tx",
        "translateY" => "ty",
        "translateZ" => "tz",
        "rotateX" => "rx",
        "rotateY" => "ry",
        "rotateZ" => "rz",
        "scaleX" => "sx",
        "scaleY" => "sy",
        "scaleZ" => "sz",
        "visibility" => "v",
        "rotateOrder" => "ro",
        "rotatePivot" => "rp",
        "scalePivot" => "sp",
        a => a,
    }
}

fn cb_label(app: &IncaApp, attr: &str) -> String {
    match app.panels.cb_names {
        1 => attr.to_string(),
        2 => cb_short_name(attr).to_string(),
        _ => ops::nice_name(attr),
    }
}

fn cb_clear_names(app: &mut IncaApp) {
    app.panels.cb_pick.clear();
    app.panels.cb_sel.clear();
    app.panels.cb_last = None;
    app.panels.cb_armed = false;
}

fn cb_manip_mode(app: &IncaApp) -> u8 {
    app.panels.cb_manips.unwrap_or(2).min(2)
}

/// Arm the virtual slider unless No Manips is selected.
fn cb_arm(app: &mut IncaApp) {
    app.panels.cb_armed = cb_manip_mode(app) != 0;
}

fn shown_node_name(app: &IncaApp, name: &str) -> String {
    if app.panels.cb_hide_namespace {
        name.rsplit(':').next().unwrap_or(name).to_string()
    } else {
        name.to_string()
    }
}

fn cb_filter(app: &IncaApp) -> [bool; 4] {
    app.panels.cb_show.unwrap_or([true; 4])
}

/// Value change per drag point for the MMB virtual slider. 0 = Medium, 1 = Slow, 2 = Fast.
fn cb_adjust_speed(app: &IncaApp) -> f64 {
    [0.01, 0.005, 0.05][app.panels.cb_speed.min(2) as usize]
}

fn cb_shown(filter: [bool; 4], attr: &str) -> bool {
    if attr.starts_with("translate") {
        filter[0]
    } else if attr.starts_with("rotate") {
        filter[1]
    } else if attr.starts_with("scale") {
        filter[2]
    } else if attr == "visibility" {
        filter[3]
    } else {
        true
    }
}

fn cb_custom_shown(app: &IncaApp, attr: &str) -> bool {
    match app.panels.cb_custom_active.and_then(|i| app.panels.cb_custom_entries.get(i)) {
        Some((_, members)) => members.iter().any(|m| m == attr),
        None => true,
    }
}

fn create_custom_show_entry(app: &mut IncaApp) {
    let Some(lead) = app.s.lead().cloned() else {
        app.help = "Select an object first".into();
        return;
    };
    let mut members: Vec<String> = app.panels.cb_pick.iter().map(|p| p.attr.clone()).collect();
    members.sort();
    members.dedup();
    if members.is_empty() {
        members = cb_order(&app.s, &NodeRef::Dag(lead));
    }
    let mut n = 1;
    while app.panels.cb_custom_entries.iter().any(|(name, _)| name == &format!("Custom {n}")) {
        n += 1;
    }
    let name = format!("Custom {n}");
    app.panels.cb_custom_entries.push((name, members));
    app.panels.cb_custom_active = Some(app.panels.cb_custom_entries.len() - 1);
    app.help = "Created a custom attribute filter from the selected channels".into();
}

fn cb_text(kind: ChanKind, x: f64, decimals: Option<usize>) -> String {
    if kind == ChanKind::Int {
        format!("{}", x.round() as i64)
    } else if let Some(d) = decimals {
        let x = if x == 0.0 { 0.0 } else { x };
        format!("{x:.d$}")
    } else {
        fmt_num(x)
    }
}

fn cb_locked(app: &IncaApp, first: &NodeRef, attr: &str) -> bool {
    match first {
        NodeRef::Dag(i) => app.s.node(i).is_some_and(|n| n.is_locked(attr)),
        _ => false,
    }
}

fn cb_fill(app: &IncaApp, first: &NodeRef, attr: &str) -> Color32 {
    let id = app.s.ref_id(first);
    let keyed = app.s.anim_is_keyed(&id, attr);
    let muted = app.s.anim_curve(&id, attr).is_some_and(|c| c.mute);
    let on_key = app.s.anim_on_key(&id, attr, app.s.time.current);
    let driven = app.s.conns.iter().any(|c| c.dst == id && c.dst_attr == attr);
    let expressed = app.s.node(&id).is_some_and(|n| n.expressions.contains_key(attr));
    if cb_locked(app, first, attr) {
        theme::LOCKED
    } else if muted {
        Color32::from_gray(0x4c)
    } else if driven || expressed {
        theme::DRIVEN.gamma_multiply(0.6)
    } else if on_key {
        theme::KEY
    } else if keyed {
        theme::KEYED.gamma_multiply(0.7)
    } else {
        theme::FIELD
    }
}

/// Commit the active field editor if its text changed, then close it. Used when the user clicks
/// away from the value fields.
pub(crate) fn commit_cb_edit(app: &mut IncaApp) {
    if app.panels.cb_editing.is_none() {
        return;
    }
    let fields = app.panels.cb_fields.clone();
    if !fields.is_empty() {
        let text = app.panels.cb_edit_text.trim().to_string();
        let relative = matches!(text.as_bytes(), [b'+' | b'-' | b'*' | b'/', b'=', ..]);
        let changed = if relative {
            true
        } else if let Some(v) = eval_expr(text.strip_prefix('=').unwrap_or(&text)) {
            fields.iter().any(|p| {
                p.targets.iter().any(|t| {
                    let cur = app.s.get_attr(t, &p.attr).and_then(|x| x.as_f64()).unwrap_or(f64::NAN);
                    (cur - v).abs() > 1e-9
                })
            })
        } else {
            false
        };
        if changed {
            cb_apply_text(app, &fields, &text);
        }
    }
    app.panels.cb_editing = None;
    app.panels.cb_edit_new = false;
}

/// Apply typed Channel Box text (absolute or `+=` style) to every selected field.
fn cb_apply_text(app: &mut IncaApp, fields: &[CbPick], text: &str) {
    let text = text.trim();
    let mut rel = None;
    let mut abs = None;
    for op in ['+', '-', '*', '/'] {
        if let Some(rest) = text.strip_prefix(op).and_then(|r| r.strip_prefix('=')) {
            rel = eval_expr(rest).map(|a| (op, a));
            break;
        }
    }
    if rel.is_none() {
        abs = eval_expr(text.strip_prefix('=').unwrap_or(text));
    }
    if rel.is_none() && abs.is_none() {
        return;
    }
    app.s.checkpoint("setAttr channel");
    for p in fields {
        for t in &p.targets {
            let mut nv = if let Some((op, a)) = rel {
                apply_rel(op, app.s.get_attr(t, &p.attr).and_then(|v| v.as_f64()).unwrap_or(0.0), a)
            } else {
                abs.unwrap_or(0.0)
            };
            if let Some(a) = p.min {
                nv = nv.max(a);
            }
            if let Some(b) = p.max {
                nv = nv.min(b);
            }
            app.s.set_attr(t, &p.attr, if p.int { json!(nv.round() as i64) } else { json!(nv) });
            if app.s.time.auto_key {
                if let NodeRef::Dag(id) = t {
                    if app.s.anim_is_keyed(id, &p.attr) {
                        app.s.set_key(id, &p.attr, None, None);
                    }
                }
            }
        }
    }
    app.echo(format!("// set {} selected field(s): {text}", fields.len()));
}

/// Picks between two row indices, keeping draw order. Fields only include unlocked numeric rows.
fn cb_rows_range(rows: &[CbRow], anchor: &(String, String), current: usize, numeric_only: bool) -> (Vec<CbPick>, Option<(String, String)>) {
    let Some(a) = rows.iter().position(|r| &r.key == anchor) else {
        return (Vec::new(), None);
    };
    let (lo, hi) = (a.min(current), a.max(current));
    let picks = rows[lo..=hi].iter().filter(|r| !numeric_only || (r.numeric && !r.locked)).map(|r| r.pick.clone()).collect();
    let last = rows[lo..=hi].iter().rev().find(|r| !numeric_only || (r.numeric && !r.locked)).map(|r| r.key.clone());
    (picks, last)
}

/// Picks whose name/field rect intersects a marquee box.
fn cb_rows_in_box(rows: &[CbRow], rect: Rect, numeric_only: bool) -> (Vec<CbPick>, Option<(String, String)>) {
    let hit = |r: &CbRow| {
        if numeric_only && !(r.numeric && !r.locked) {
            return false;
        }
        let target = if numeric_only { r.field.unwrap_or(r.label) } else { r.label };
        rect.intersects(target)
    };
    let picks = rows.iter().filter(|r| hit(r)).map(|r| r.pick.clone()).collect();
    let last = rows.iter().rev().find(|r| hit(r)).map(|r| r.key.clone());
    (picks, last)
}

/// One editable numeric channel field: display, drag-select, inline editing and the wheel step.
/// Used by the Channel Box rows and by the Attribute Editor's 3-column transform rows.
fn cb_numeric_cell(
    app: &mut IncaApp,
    ui: &mut egui::Ui,
    width: f32,
    targets: &[NodeRef],
    attr: &str,
    kind: ChanKind,
    min: Option<f64>,
    max: Option<f64>,
    decimals: Option<usize>,
    rows: &mut Vec<CbRow>,
    label_rect: Rect,
) -> egui::Response {
    let Some(first) = targets.first() else {
        return ui.allocate_response(egui::vec2(width, 18.0), egui::Sense::hover());
    };
    let Some(cur) = app.s.get_attr(first, attr) else {
        return ui.allocate_response(egui::vec2(width, 18.0), egui::Sense::hover());
    };
    let x = cur.as_f64().unwrap_or(0.0);
    let id = app.s.ref_id(first);
    let key = (id.clone(), attr.to_string());
    let int = matches!(kind, ChanKind::Int);
    let me = CbPick { targets: targets.to_vec(), attr: attr.to_string(), int, min, max };
    let field_selected = cb_is_picked(&app.panels.cb_fields, first, attr);
    let locked = cb_locked(app, first, attr);
    let editing = app.panels.cb_editing.as_ref().is_some_and(|(i, a)| i == &key.0 && a == &key.1);
    let r = if editing {
        let edit_id = ui.make_persistent_id(("cb_edit", &key.0, &key.1));
        let mut text = app.panels.cb_edit_text.clone();
        let r = ui.add_sized(
            [width, 18.0],
            egui::TextEdit::singleline(&mut text).id(edit_id).horizontal_align(egui::Align::RIGHT).margin(egui::Margin::symmetric(4, 2)),
        );
        app.panels.cb_edit_text = text;
        let still_editing = app.panels.cb_editing.as_ref() == Some(&key);
        if app.panels.cb_edit_new && still_editing {
            ui.memory_mut(|m| m.request_focus(edit_id));
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit_id) {
                let n = app.panels.cb_edit_text.chars().count();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::default(), egui::text::CCursor::new(n))));
                state.store(ui.ctx(), edit_id);
            }
            app.panels.cb_edit_new = false;
        } else if still_editing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.panels.cb_editing = None;
        } else if still_editing && (r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            let fields = if field_selected && app.panels.cb_fields.len() > 1 { app.panels.cb_fields.clone() } else { vec![me.clone()] };
            cb_apply_text(app, &fields, &app.panels.cb_edit_text.clone());
            app.panels.cb_editing = None;
        }
        r
    } else {
        let text = cb_text(kind, x, decimals);
        let (rrect, r) = ui.allocate_exact_size(egui::vec2(width, 18.0), egui::Sense::click_and_drag());
        let painter = ui.painter();
        painter.rect_filled(rrect, 2.0, if field_selected { theme::HI } else { cb_fill(app, first, attr) });
        painter.text(
            rrect.right_center() - egui::vec2(5.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            text,
            egui::FontId::proportional(12.0),
            if locked { theme::TEXT_DIM } else { theme::TEXT },
        );
        let border = if field_selected { Color32::from_rgb(0x9a, 0xc8, 0xe0) } else { theme::LINE };
        painter.rect_stroke(rrect, 2.0, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
        if r.hovered() && !locked {
            painter.rect_stroke(rrect, 2.0, egui::Stroke::new(1.0, theme::TEXT_DIM), egui::StrokeKind::Inside);
        }
        if r.clicked() && !locked {
            // Commit the previous field before switching editors, whichever row it is in.
            if app.panels.cb_editing.as_ref().is_some_and(|(i, a)| i != &key.0 || a != &key.1) {
                commit_cb_edit(app);
            }
            // Clicking a text box drops the channel-name selection (and the MMB slider arm).
            cb_clear_names(app);
            let m = ui.input(|i| i.modifiers);
            if m.shift {
                app.panels.cb_field_click = Some((key.clone(), true));
            } else if m.ctrl || m.command {
                if cb_is_picked(&app.panels.cb_fields, first, attr) {
                    app.panels.cb_fields.retain(|q| !(q.attr == attr && q.targets.first() == Some(first)));
                } else {
                    app.panels.cb_fields.push(me.clone());
                }
                app.panels.cb_field_last = Some(key.clone());
            } else {
                // A plain click always collapses a multi-field selection to the clicked field.
                app.panels.cb_fields = vec![me.clone()];
                app.panels.cb_field_last = Some(key.clone());
            }
            app.panels.cb_edit_text = cb_text(kind, x, decimals);
            app.panels.cb_editing = Some(key.clone());
            app.panels.cb_edit_new = true;
        }
        if r.drag_started_by(egui::PointerButton::Primary) && !locked {
            if app.panels.cb_editing.as_ref().is_some_and(|(i, a)| i != &key.0 || a != &key.1) {
                commit_cb_edit(app);
            }
            cb_clear_names(app);
            app.panels.cb_field_drag = Some(key.clone());
            app.panels.cb_drag_origin = ui.input(|i| i.pointer.press_origin());
            // Like a plain click, the start of a field drag collapses the selection to that field;
            // the marquee then extends it if the pointer leaves the pressed field.
            app.panels.cb_fields = vec![me.clone()];
            app.panels.cb_field_last = Some(key.clone());
            app.panels.cb_editing = None;
            app.panels.cb_edit_new = false;
            if let Some(fid) = ui.memory(|m| m.focused()) {
                ui.memory_mut(|m| m.surrender_focus(fid));
            }
        }
        if kind == ChanKind::Int && r.hovered() && !locked {
            let sc = wheel_notch(ui);
            if sc != 0.0 {
                let mut nv = x + sc.signum() as f64;
                if let Some(a) = min {
                    nv = nv.max(a);
                }
                if let Some(b) = max {
                    nv = nv.min(b);
                }
                app.s.checkpoint(&format!("setAttr {attr}"));
                for t in targets {
                    app.s.set_attr(t, attr, json!(nv.round() as i64));
                }
            }
        }
        r.on_hover_cursor(egui::CursorIcon::Text)
    };
    rows.push(CbRow { key, pick: me, label: label_rect, field: Some(r.rect), numeric: true, locked });
    r
}

/// Draft of the Edit Layer dialog: applied as one undo step on Save.
#[derive(Clone)]
pub struct LayerEditDraft {
    pub id: String,
    pub name: String,
    pub mode: String,
    pub visible: bool,
    pub playback: bool,
    pub color: Option<[f32; 3]>,
    /// `false` = Index palette, `true` = RGB picker.
    pub rgb: bool,
    pub focus_name: bool,
}

#[derive(Default)]
pub struct PanelState {
    pub outliner_filter: String,
    pub renaming: Option<(String, String)>,
    pub expanded: std::collections::HashSet<String>,
    pub input_line: String,
    pub input_mode: usize,
    pub quit: bool,
    pub quit_confirm: bool,
    /// a channel drag is in progress (one undo step per drag)
    pub editing: bool,
    /// Outliner Color picker drag: one undo step from press to release.
    outliner_color_editing: bool,
    /// Channel Box channel names picked for the MMB virtual slider (and whether it is armed)
    pub cb_sel: Vec<String>,
    /// the picked channels themselves: rows' targets, attribute, whole-number?, range
    pub cb_pick: Vec<CbPick>,
    pub cb_armed: bool,
    cb_last: Option<String>,
    /// virtual slider drag: (target, attribute, start value, int?)
    pub cb_drag: Option<Vec<(NodeRef, String, f64, bool, Option<f64>, Option<f64>)>>,
    cb_drag_x: f32,
    /// Channel Box value fields picked by dragging/Shift/Ctrl. Typing into one of them edits all.
    pub cb_fields: Vec<CbPick>,
    cb_field_last: Option<(String, String)>,
    /// Pending Shift-click field range: field key and whether Shift was held.
    cb_field_click: Option<((String, String), bool)>,
    /// Active Channel Box drag-select anchors: channel names and value fields.
    cb_name_drag: Option<(String, String)>,
    cb_field_drag: Option<(String, String)>,
    /// Screen position where the current drag-select marquee started.
    cb_drag_origin: Option<egui::Pos2>,
    /// Inline Channel Box value editor. `cb_edit_new` requests focus and select-all.
    cb_editing: Option<(String, String)>,
    cb_edit_text: String,
    cb_edit_new: bool,
    /// Channel Box object name field: shown name and edit buffer.
    cb_name: (String, String),
    pub cb_name_rect: Option<Rect>,
    /// Rects of the numeric value fields drawn last frame, used to drop field selection on
    /// a click anywhere else.
    pub cb_numeric_rects: Vec<Rect>,
    /// Channel Box menu settings: slider speed, name style, attribute type filters and sync.
    pub cb_speed: u8,
    pub cb_names: u8,
    pub cb_show: Option<[bool; 4]>,
    /// Decimal places for Channel Box numeric values; `None` keeps Maya's trimmed display.
    pub cb_precision: Option<usize>,
    /// Show Assets display mode: 0 = all published attributes, 1 = selected object's attributes.
    pub cb_asset_mode: u8,
    pub cb_asset_top: bool,
    /// Custom Show > Attributes entries: name and member attribute names.
    pub cb_custom_entries: Vec<(String, Vec<String>)>,
    pub cb_custom_active: Option<usize>,
    /// Attributes checked in the Delete Attributes window.
    pub delete_attr_checked: std::collections::BTreeSet<String>,
    /// Custom attribute being edited and its new long name.
    pub edit_attr_name: String,
    pub edit_attr_new: String,
    /// Expression Editor target channel and source text.
    pub expression_attr: String,
    pub expression_text: String,
    pub cb_sync_graph: bool,
    pub cb_sync_timeline: bool,
    /// Channel Box manipulator mode: 0 = No Manips, 1 = Invisible, 2 = Standard. `None` = Standard.
    pub cb_manips: Option<u8>,
    /// Hyperbolic attribute-adjustment mapping for the virtual slider.
    pub cb_hyperbolic: bool,
    /// Hide namespace prefixes in displayed object names.
    pub cb_hide_namespace: bool,
    /// Last drawn Channel Box rows for the in-app UI tests: key, name rect, field rect.
    pub cb_row_rects: Vec<(String, String, Rect, Rect)>,
    pub ae_tab: usize,
    pub ae_tab_scroll: f32,
    /// Attribute Editor name style: 0 = nice, 1 = long, 2 = short.
    pub ae_names: u8,
    ae_opm_editing: bool,
    ae_limit_editing: bool,
    pub ol: OutlinerOpts,
    /// Attribute Editor locked to a node (List > Auto Load off, or the lock button)
    pub ae_locked: Option<String>,
    /// (shown name, edited text) of the AE node field
    pub ae_name: (String, String),
    /// Name typed into the Attribute Editor Presets menu.
    pub ae_preset_name: String,
    pub ae_copies: Vec<(String, usize)>,
    /// outliner rows in display order (last frame), for Shift range selection and arrow keys
    pub ol_rows: Vec<String>,
    pub ol_anchor: Option<String>,
    /// the outliner was clicked last, so arrow keys walk it
    pub ol_active: bool,
    pub ol_scroll: bool,
    /// Layer Editor tab: 0 = Display, 1 = Anim.
    pub layer_tab: u8,
    /// Display layers highlighted in the Layer Editor.
    pub layer_sel: Vec<String>,
    /// Anchor for Shift-click ranges in the Layer Editor.
    pub layer_anchor: Option<String>,
    /// Inline layer-name edit: (layer id, edit buffer).
    pub layer_rename: Option<(String, String)>,
    /// The inline layer-name edit has not received focus yet.
    pub layer_rename_new: bool,
    /// Inline animation-layer name edit: (layer id, edit buffer).
    pub anim_layer_rename: Option<(String, String)>,
    /// The inline animation-layer edit has not received focus yet.
    pub anim_layer_rename_new: bool,
    /// Hide the BaseAnimation row in the animation layer editor.
    pub anim_hide_base: bool,
    /// Layer whose colour palette popup is open.
    pub layer_color_popup: Option<String>,
    /// Custom RGB chosen in the layer colour popup (0..1 per channel).
    pub layer_custom_rgb: [f32; 3],
    /// Layer whose full colour picker window is open.
    pub layer_picker: Option<String>,
    /// Edit Layer dialog draft, when open.
    pub layer_edit: Option<LayerEditDraft>,
    /// Display layer shown in the Attribute Editor (until the scene selection changes).
    pub ae_layer: Option<String>,
    /// Scene selection captured when the layer was loaded into the AE.
    pub ae_layer_sel: Vec<String>,
    /// Layer whose Membership window is open.
    pub layer_membership: Option<String>,
    /// Objects selected in the Membership editor's object pane.
    pub membership_sel: Vec<String>,
    /// Search filter for the Membership editor's object pane.
    pub membership_filter: String,
    /// Expanded display layers in the Membership editor's Layers pane.
    pub membership_layers_open: std::collections::HashSet<String>,
    /// Options > Make New Layers Current.
    pub layer_make_current: bool,
    /// Options > Add New Objects to Current Layer.
    pub layer_add_new: bool,
}

/// Outliner icon for a node (the JS KIND_ICON, lights by type).
fn kind_icon(n: &inca_core::scene::Node) -> &'static str {
    match (&n.shape, n.kind.as_str()) {
        (Shape::Light(l), _) => match l.ty.as_str() {
            "ambientLight" => "ambientLight",
            "directionalLight" => "directionalLight",
            "spotLight" => "spotLight",
            "areaLight" => "areaLight",
            _ => "pointLight",
        },
        (_, "joint") => "joint",
        (_, "constraint") => "parent",
        (_, "lattice") => "nurbsCube",
        (Shape::Mesh(_), _) => "olMesh",
        (Shape::Group, _) => "olGroup",
        (Shape::Camera(_), _) => "olCamera",
        (Shape::Curve(_), _) => "olCurve",
        (Shape::Locator, _) => "olLocator",
    }
}

/// Outliner display options (JS prefs.opt.outliner).
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutlinerOpts {
    pub shapes: bool,
    pub sets: bool,
    pub dag_only: bool,
    pub sort_alpha: bool,
    pub materials: bool,
    #[serde(default)]
    pub ignore_outliner_color: bool,
}

impl Default for OutlinerOpts {
    fn default() -> Self {
        OutlinerOpts { shapes: false, sets: true, dag_only: false, sort_alpha: false, materials: false, ignore_outliner_color: false }
    }
}

// ------------------------------------------------------------------ outliner

pub fn outliner(app: &mut IncaApp, ui: &mut egui::Ui) {
    use crate::menus::{activated, menu_check, menu_item, panel_bar, MenuSource};
    panel_bar(app, ui, MenuSource::Outliner, &["Edit", "Display", "Show", "Help"], false, |app, ui, title| match title {
        "Edit" => {
            for (l, a) in [("Rename", "renameSel"), ("Select Hierarchy", "selectHierarchy")] {
                outliner_cmd(app, ui, l, a);
            }
            ui.separator();
            for (l, a) in [("Group", "group"), ("Ungroup", "ungroup"), ("Delete", "deleteSel")] {
                outliner_cmd(app, ui, l, a);
            }
            ui.separator();
            let r = ui.button("Expand All");
            if activated(app, ui, &r) {
                app.panels.expanded = app.s.all_dag(true).into_iter().collect();
            }
            let r = ui.button("Collapse All");
            if activated(app, ui, &r) {
                app.panels.expanded.clear();
            }
        }
        "Display" => {
            let mut o = app.panels.ol;
            for (value, label) in [
                (&mut o.dag_only, "DAG Objects Only"),
                (&mut o.shapes, "Shapes"),
                (&mut o.sets, "Sets"),
                (&mut o.materials, "Materials"),
            ] {
                if menu_check(app, ui, *value, label) {
                    *value = !*value;
                }
            }
            ui.separator();
            if menu_check(app, ui, o.sort_alpha, "Sort Alphabetical") {
                o.sort_alpha = !o.sort_alpha;
            }
            if menu_check(app, ui, !app.panels.cb_hide_namespace, "Show Namespace") {
                app.panels.cb_hide_namespace = !app.panels.cb_hide_namespace;
            }
            if menu_check(app, ui, o.ignore_outliner_color, "Ignore 'Use Outliner Color'") {
                o.ignore_outliner_color = !o.ignore_outliner_color;
            }
            app.panels.ol = o;
        }
        "Show" => {
            if menu_item(app, ui, "Show Selected") {
                for id in app.s.sel.clone() {
                    let mut p = app.s.node(&id).and_then(|n| n.parent.clone());
                    while let Some(x) = p {
                        p = app.s.node(&x).and_then(|n| n.parent.clone());
                        app.panels.expanded.insert(x);
                    }
                }
                app.panels.ol_scroll = true;
            }
            outliner_cmd(app, ui, "Frame Selection", "frameSelected");
        }
        _ => {
            if menu_item(app, ui, "Help on Outliner") {
                app.help = "Click to select, Ctrl to toggle, Shift for range. Drag onto another item to parent; drop on empty space to unparent. Double-click to rename.".into();
            }
        }
    });
    ui.add(egui::TextEdit::singleline(&mut app.panels.outliner_filter).hint_text("Search...").desired_width(f32::INFINITY));
    let filter = app.panels.outliner_filter.trim().to_lowercase().replace('*', "");
    let hovered = ui.rect_contains_pointer(ui.max_rect());
    let mut rows = vec![];
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let order = ["persp", "top", "front", "side"];
        let (mut cams, mut rest): (Vec<_>, Vec<_>) = app.s.roots.clone().into_iter().partition(|r| app.s.node(r).is_some_and(|n| n.startup()));
        cams.sort_by_key(|c| order.iter().position(|o| *o == app.s.name_of(c)).unwrap_or(9));
        if app.panels.ol.sort_alpha {
            rest.sort_by_key(|r| app.s.name_of(r).to_lowercase());
        }
        for r in cams.iter().chain(rest.iter()) {
            outliner_row(app, ui, r, 0, &filter, &mut rows);
        }
        let o = app.panels.ol;
        if !o.dag_only && o.sets {
            for n in ["defaultLightSet", "defaultObjectSet"] {
                plain_row(ui, 0, "olSet", n, Color32::from_gray(0xd0));
            }
        }
        if o.materials {
            let mats: Vec<String> = app.s.mats.values().map(|m| m.name.clone()).collect();
            for m in mats {
                if plain_row(ui, 0, "olMaterial", &m, theme::TEXT).clicked() {
                    let _ = app.run_mel("select material", &format!("select -r {m};"));
                }
            }
        }
        // empty space: click deselects, drop unparents
        let (_, zone) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ui.available_height().max(40.0)), egui::Sense::click());
        if zone.clicked() && !app.s.sel.is_empty() {
            app.s.checkpoint_selection("select");
            app.select_names(&[], SelMode::Replace);
        }
        if let Some(ids) = zone.dnd_release_payload::<Vec<String>>() {
            let names: Vec<String> = ids.iter().map(|i| app.s.name_of(i)).collect();
            let _ = app.run_mel("unparent", &format!("parent -w {};", names.join(" ")));
        }
    });
    app.panels.ol_scroll = false;
    // arrow keys walk the list (Shift adds), left / right collapse / expand
    if (hovered || app.panels.ol_active) && !ui.ctx().egui_wants_keyboard_input() {
        let (up, down, left, right, shift) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.modifiers.shift));
        if up || down {
            let cur = app.s.lead().and_then(|l| rows.iter().position(|r| r == l));
            let i = match cur {
                Some(i) if up => i.saturating_sub(1),
                Some(i) => (i + 1).min(rows.len().saturating_sub(1)),
                None => 0,
            };
            if let Some(r) = rows.get(i).cloned() {
                app.select_names(&[r], if shift { SelMode::Add } else { SelMode::Replace });
                app.panels.ol_scroll = true;
            }
        }
        if left || right {
            for id in app.s.sel.clone() {
                if right {
                    app.panels.expanded.insert(id);
                } else {
                    app.panels.expanded.remove(&id);
                }
            }
        }
    }
    app.panels.ol_rows = rows;
}

fn outliner_cmd(app: &mut IncaApp, ui: &mut egui::Ui, label: &str, action: &str) {
    let r = ui.button(label);
    if crate::menus::activated(app, ui, &r) {
        app.action(action);
    }
}

fn matches_filter(app: &IncaApp, id: &str, f: &str) -> bool {
    f.is_empty() || app.s.subtree(id).iter().any(|i| app.s.name_of(i).to_lowercase().contains(f))
}

const ROW_H: f32 = 19.0;

/// A non-node row (shape, set, material).
fn plain_row(ui: &mut egui::Ui, depth: usize, icon: &str, text: &str, col: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, Color32::from_gray(0x4d));
    }
    let x = r.min.x + 4.0 + depth as f32 * 16.0 + 12.0;
    crate::icons::paint(ui, egui::Rect::from_min_size(egui::pos2(x, r.center().y - 7.5), egui::vec2(15.0, 15.0)), icon, Color32::WHITE);
    ui.painter().text(egui::pos2(x + 20.0, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(12.0), col);
    resp
}

/// Move `moving` next to `target` among its siblings (drag between Outliner rows).
/// Returns false when nothing could move.
fn outliner_reorder(app: &mut IncaApp, moving: &[String], target: &str, after: bool) -> bool {
    let Some(t) = app.s.node(target) else { return false };
    let parent = t.parent.clone();
    let mut ids: Vec<String> = moving
        .iter()
        .filter(|i| i.as_str() != target && app.s.node(i).is_some())
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    // Drop children of other moved nodes, and ancestors of the target (invalid parents).
    let kept: Vec<String> = ids
        .iter()
        .filter(|i| !ids.iter().any(|j| j != *i && app.s.is_ancestor(j, i)))
        .filter(|i| !app.s.is_ancestor(i, target))
        .cloned()
        .collect();
    if kept.is_empty() {
        return false;
    }
    app.s.checkpoint("reorder in outliner");
    for mid in &kept {
        app.s.reparent(mid, parent.as_deref(), None);
    }
    let siblings = app.s.children(parent.as_deref()).to_vec();
    let Some(pos) = siblings.iter().position(|s| s == target) else { return false };
    let mut at = if after { pos + 1 } else { pos };
    for mid in &kept {
        app.s.reparent(mid, parent.as_deref(), Some(at));
        at += 1;
    }
    true
}

/// Text for the Outliner drag preview: packs names up to `DRAG_LINE` characters, breaking only
/// between names. An individual name longer than the line is ellipsized, never cut mid-name.
/// Only the first few names are shown so a large selection cannot grow the tooltip forever.
fn outliner_drag_text(names: &[String]) -> String {
    const DRAG_LINE: usize = 32;
    const MAX_NAMES: usize = 10;
    let shown = &names[..names.len().min(MAX_NAMES)];
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for name in shown {
        let name = if name.chars().count() > DRAG_LINE {
            format!("{}…", name.chars().take(DRAG_LINE - 1).collect::<String>())
        } else {
            name.clone()
        };
        if line.is_empty() {
            line = name;
        } else if line.chars().count() + 2 + name.chars().count() <= DRAG_LINE {
            line.push_str(", ");
            line.push_str(&name);
        } else {
            lines.push(std::mem::take(&mut line));
            line = name;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if names.len() > MAX_NAMES {
        lines.push(format!("… +{} more", names.len() - MAX_NAMES));
    }
    lines.join("\n")
}

fn outliner_row(app: &mut IncaApp, ui: &mut egui::Ui, id: &str, depth: usize, filter: &str, rows: &mut Vec<String>) {
    if !matches_filter(app, id, filter) {
        return;
    }
    let Some(n) = app.s.node(id) else { return };
    let full_name = n.name.clone();
    let name = shown_node_name(app, &full_name);
    let icon = kind_icon(n);
    let startup = n.startup();
    let template = app.s.is_templated_display(id);
    let custom_color = if n.use_outliner_color && !app.panels.ol.ignore_outliner_color {
        let c = n.outliner_color;
        Some(Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8))
    } else {
        None
    };
    let shape_name = n.shape_name.clone().filter(|_| app.panels.ol.shapes);
    let mut kids = n.children.clone();
    if app.panels.ol.sort_alpha {
        kids.sort_by_key(|k| app.s.name_of(k).to_lowercase());
    }
    let has_kids = !kids.is_empty() || shape_name.is_some();
    let open = has_kids && (app.panels.expanded.contains(id) || !filter.is_empty());
    let selected = app.s.sel.iter().any(|s| s == id) || app.s.hilite().iter().any(|s| s == id);
    let child_sel = !selected && app.s.sel.iter().any(|s| app.s.is_ancestor(id, s));
    // hidden here or above
    let mut hidden = false;
    let mut p = Some(id.to_string());
    while let Some(x) = p {
        let Some(nn) = app.s.node(&x) else { break };
        hidden |= !nn.visible;
        p = nn.parent.clone();
    }
    rows.push(id.to_string());
    let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click_and_drag());
    if app.panels.ol_scroll && app.s.lead().is_some_and(|l| l == id) {
        ui.scroll_to_rect(r, None);
    }
    let x0 = r.min.x + 4.0 + depth as f32 * 16.0;
    let exp_rect = egui::Rect::from_min_size(egui::pos2(x0, r.min.y), egui::vec2(12.0, ROW_H));
    // Drop zones: top/bottom quarter reorders siblings, the middle reparents.
    let drop_payload = resp.dnd_hover_payload::<Vec<String>>();
    let drop_ok = drop_payload.as_ref().is_some_and(|p| !p.iter().any(|x| x == id));
    let drop_zone = drop_ok.then(|| {
        let rel = ui.ctx().pointer_latest_pos().map(|p| (p.y - r.top()) / r.height()).unwrap_or(0.5);
        if rel < 0.25 { 0 } else if rel > 0.75 { 2 } else { 1 }
    });
    let bg = if selected {
        Some(theme::HI)
    } else if resp.hovered() {
        Some(Color32::from_gray(0x4d))
    } else {
        None
    };
    if let Some(c) = bg {
        ui.painter().rect_filled(r, 0.0, c);
    }
    let drop_color = Color32::from_rgb(0x8f, 0xd0, 0xff);
    match drop_zone {
        Some(0) => {
            ui.painter().line_segment([r.left_top(), r.right_top()], egui::Stroke::new(2.0, drop_color));
        }
        Some(2) => {
            ui.painter().line_segment([r.left_bottom(), r.right_bottom()], egui::Stroke::new(2.0, drop_color));
        }
        Some(1) => {
            ui.painter().rect_stroke(r, 0.0, egui::Stroke::new(1.0, drop_color), egui::StrokeKind::Inside);
        }
        _ => {}
    }
    let dim = if hidden { 110 } else { 255 };
    if has_kids {
        let c = exp_rect.center();
        let pts = if open {
            vec![c + egui::vec2(-3.5, -2.0), c + egui::vec2(3.5, -2.0), c + egui::vec2(0.0, 2.5)]
        } else {
            vec![c + egui::vec2(-2.0, -3.5), c + egui::vec2(2.5, 0.0), c + egui::vec2(-2.0, 3.5)]
        };
        ui.painter().add(egui::Shape::convex_polygon(pts, theme::TEXT, egui::Stroke::NONE));
    }
    crate::icons::paint(ui, egui::Rect::from_min_size(egui::pos2(x0 + 12.0, r.center().y - 7.5), egui::vec2(15.0, 15.0)), icon, Color32::from_white_alpha(dim));
    let col = if selected {
        Color32::WHITE
    } else if let Some(c) = custom_color {
        c
    } else if template {
        Color32::from_rgb(224, 160, 64)
    } else if child_sel {
        Color32::from_rgb(0x9f, 0xd7, 0xff)
    } else if startup {
        Color32::from_gray(0x8a)
    } else {
        theme::TEXT
    };
    let col = if hidden && !selected { Color32::from_gray(0x7a) } else { col };
    let name_pos = egui::pos2(x0 + 32.0, r.center().y);
    let renaming = app.panels.renaming.as_ref().is_some_and(|(rid, _)| rid == id);
    if renaming {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(egui::pos2(name_pos.x, r.min.y), r.max)));
        let (_, text) = app.panels.renaming.as_mut().unwrap();
        let te = child.text_edit_singleline(text);
        if te.lost_focus() {
            let t = text.trim().to_string();
            let cancel = child.input(|i| i.key_pressed(egui::Key::Escape));
            app.panels.renaming = None;
            if !cancel && !t.is_empty() && t != name {
                let _ = app.run_mel("rename", &format!("rename {name} \"{t}\";"));
            }
        } else {
            te.request_focus();
        }
    } else {
        ui.painter().text(name_pos, egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(12.0), col);
    }
    let resp = if template { resp.on_hover_text(format!("{name} (template)")) } else { resp };
    let m = ui.input(|i| i.modifiers);
    let on_exp = resp.interact_pointer_pos().is_some_and(|p| exp_rect.contains(p));
    if resp.clicked() && on_exp && has_kids {
        // Shift expands / collapses the whole branch
        let set = !open;
        let ids = if m.shift { app.s.subtree(id) } else { vec![id.to_string()] };
        for i in ids {
            if set {
                app.panels.expanded.insert(i);
            } else {
                app.panels.expanded.remove(&i);
            }
        }
    } else if resp.clicked() {
        app.panels.ol_active = true;
        app.s.checkpoint_selection("select");
        let anchor = app.panels.ol_anchor.clone().filter(|a| app.panels.ol_rows.contains(a));
        if m.shift && !startup && anchor.is_some() {
            let a = app.panels.ol_rows.iter().position(|x| Some(x) == anchor.as_ref()).unwrap();
            let b = app.panels.ol_rows.iter().position(|x| x == id).unwrap_or(a);
            let range: Vec<String> = app.panels.ol_rows[a.min(b)..=a.max(b)].to_vec();
            app.select_names(&range, if m.ctrl || m.command { SelMode::Add } else { SelMode::Replace });
        } else if m.ctrl || m.command {
            app.select_names(&[id.to_string()], SelMode::Toggle);
        } else {
            app.select_names(&[id.to_string()], SelMode::Replace);
            app.panels.ol_anchor = Some(id.to_string());
        }
    }
    if resp.double_clicked() && !startup && !on_exp {
        app.panels.renaming = Some((id.to_string(), full_name.clone()));
    }
    if resp.drag_started() && !startup {
        let moving: Vec<String> = if app.s.sel.iter().any(|s| s == id) { app.s.sel.clone() } else { vec![id.to_string()] };
        resp.dnd_set_drag_payload(moving);
    }
    if resp.dragged() {
        if let (Some(p), Some(ids)) = (ui.ctx().pointer_latest_pos(), egui::DragAndDrop::payload::<Vec<String>>(ui.ctx())) {
            let names: Vec<String> = ids.iter().map(|i| app.s.name_of(i)).collect();
            egui::Area::new(egui::Id::new("ol-drag")).fixed_pos(p + egui::vec2(12.0, 6.0)).order(egui::Order::Tooltip).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.add(egui::Label::new(outliner_drag_text(&names)).wrap_mode(egui::TextWrapMode::Extend));
                });
            });
        }
    }
    if let Some(ids) = resp.dnd_release_payload::<Vec<String>>() {
        if !startup {
            let rel = ui.ctx().pointer_latest_pos().map(|p| (p.y - r.top()) / r.height()).unwrap_or(0.5);
            let moving: Vec<String> = ids
                .iter()
                .filter(|i| i.as_str() != id && app.s.node(i).is_some_and(|n| !n.startup()))
                .cloned()
                .collect();
            if !moving.is_empty() {
                if (0.25..=0.75).contains(&rel) {
                    app.s.checkpoint("parent in outliner");
                    for mid in &moving {
                        app.s.reparent(mid, Some(id), None);
                    }
                    app.panels.expanded.insert(id.to_string());
                } else {
                    outliner_reorder(app, &moving, id, rel > 0.75);
                }
            }
        }
    }
    resp.context_menu(|ui| {
        if !app.s.sel.iter().any(|s| s == id) {
            app.select_names(&[id.to_string()], SelMode::Replace);
        }
        if ui.button("Rename").clicked() {
            if !startup {
                app.panels.renaming = Some((id.to_string(), full_name.clone()));
            }
            ui.close();
        }
        outliner_cmd(app, ui, "Select Hierarchy", "selectHierarchy");
        ui.separator();
        for (l, a) in [("Group", "group"), ("Ungroup", "ungroup"), ("Duplicate", "duplicate"), ("Unparent", "unparent"), ("Delete", "deleteSel")] {
            outliner_cmd(app, ui, l, a);
        }
        ui.separator();
        for (l, a) in [("Hide", "hideSelection"), ("Show", "showSelection"), ("Isolate Select", "isolateSelect")] {
            outliner_cmd(app, ui, l, a);
        }
        ui.separator();
        for (l, a) in [("Frame Selection", "frameSelected"), ("Attribute Editor", "attributeEditor")] {
            outliner_cmd(app, ui, l, a);
        }
        ui.separator();
        if ui.button("Expand All").clicked() {
            for s in app.s.sel.clone() {
                app.panels.expanded.extend(app.s.subtree(&s));
            }
            ui.close();
        }
        if ui.button("Collapse All").clicked() {
            app.panels.expanded.clear();
            ui.close();
        }
    });
    if open {
        if let Some(sn) = shape_name {
            plain_row(ui, depth + 1, icon, &sn, Color32::from_gray(0xaa));
        }
        for k in kids {
            outliner_row(app, ui, &k, depth + 1, filter, rows);
        }
    }
}

// ------------------------------------------------------------------ right panel

/// Right dock panels as the JS side tabs list them.
const SIDE_TABS: &[(Option<RightTab>, &str)] = &[
    (Some(RightTab::ChannelBox), "Channel Box / Layer Editor"),
    (Some(RightTab::AttributeEditor), "Attribute Editor"),
    (Some(RightTab::ToolSettings), "Tool Settings"),
    (Some(RightTab::ModelingToolkit), "Modeling Toolkit"),
];

/// The vertical tab strip at the right edge: click shows a panel, clicking the shown one hides the dock.
pub fn side_tabs(app: &mut IncaApp, ui: &mut egui::Ui) {
    let mut y = ui.max_rect().min.y + 6.0;
    let x = ui.max_rect().center().x;
    let font = egui::FontId::proportional(10.5);
    for (tab, label) in SIDE_TABS {
        let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE));
        let len = galley.size().x + 16.0;
        let r = egui::Rect::from_min_size(egui::pos2(x - 9.0, y), egui::vec2(18.0, len));
        let resp = ui.interact(r, egui::Id::new(("sidetab", *label)), egui::Sense::click()).on_hover_text(*label);
        let active = app.show_right && tab.is_some_and(|t| t == app.right_tab);
        if active {
            ui.painter().rect_filled(r, 3.0, Color32::from_gray(0x55));
        }
        let col = if active || resp.hovered() { Color32::WHITE } else { Color32::from_gray(0x9d) };
        // vertical-rl: text reads top to bottom
        let pos = egui::pos2(r.max.x - (18.0 - galley.size().y) / 2.0, r.min.y + 8.0);
        ui.painter().add(egui::epaint::TextShape::new(pos, galley, col).with_override_text_color(col).with_angle(std::f32::consts::FRAC_PI_2));
        if resp.clicked() {
            match tab {
                Some(_) if active => app.show_right = false,
                Some(t) => {
                    app.right_tab = *t;
                    app.show_right = true;
                }
                None => {}
            }
        }
        y += len + 2.0;
    }
}

pub fn right_panel(app: &mut IncaApp, ui: &mut egui::Ui) {
    ui.set_min_width(crate::app::right_panel_min_width(app.right_tab));
    if app.shown("sidetabs") {
        // the side tabs switch panels; show the panel's title like the JS dock
        let title = SIDE_TABS.iter().find(|t| t.0 == Some(app.right_tab)).map(|t| t.1).unwrap_or("");
        ui.label(RichText::new(title).strong());
    } else {
        ui.horizontal(|ui| {
            for (t, l) in [(RightTab::ChannelBox, "Channel Box"), (RightTab::AttributeEditor, "Attribute Editor"), (RightTab::ToolSettings, "Tool Settings"), (RightTab::ModelingToolkit, "Modeling Toolkit")] {
                if ui.selectable_label(app.right_tab == t, l).clicked() {
                    app.right_tab = t;
                }
            }
        });
    }
    ui.separator();
    match app.right_tab {
        RightTab::ChannelBox => {
            channel_box_menu(app, ui);
            let h = ui.available_height();
            egui::ScrollArea::vertical().id_salt("cbscroll").max_height(h * 0.68).auto_shrink([false, false]).show(ui, |ui| channel_box(app, ui));
            ui.separator();
            layer_editor(app, ui);
        }
        RightTab::AttributeEditor => attribute_editor(app, ui),
        RightTab::ToolSettings => tool_settings(app, ui),
        RightTab::ModelingToolkit => modeling_toolkit(app, ui),
    }
}

/// Modeling Toolkit: selection modes, symmetry and the common mesh commands (right-click: options).
fn modeling_toolkit(app: &mut IncaApp, ui: &mut egui::Ui) {
    use inca_core::scene::CompMode;
    ui.horizontal(|ui| {
        let cm = app.s.comp_mode;
        for (id, icon, tip, on) in [("objectMode", "selObject", "Object selection", cm.is_none()), ("vertexMode", "compVertex", "Vertex selection (F9)", cm == Some(CompMode::Vertex) && !app.uv_mode), ("edgeMode", "compEdge", "Edge selection (F10)", cm == Some(CompMode::Edge)), ("faceMode", "compFace", "Face selection (F11)", cm == Some(CompMode::Face) && !app.uv_mode), ("uvMode", "compUV", "UV selection (F12)", app.uv_mode)] {
            if crate::icons::button(ui, icon, tip, on, 24.0).clicked() {
                if id == "objectMode" {
                    if cm.is_some() {
                        app.action("objectMode");
                    }
                } else {
                    app.action(id);
                }
            }
        }
        ui.separator();
        if crate::icons::button(ui, "softSelect", "Soft Select (B)", app.soft.on, 24.0).clicked() {
            app.action("softSelect");
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Symmetry:").color(theme::TEXT_DIM));
        let mut sym = app.symmetry;
        egui::ComboBox::from_id_salt("mtksym").selected_text(["Off", "Object X", "World X", "World Y", "World Z"][sym.min(4)]).show_ui(ui, |ui| {
            for (i, l) in ["Off", "Object X", "World X", "World Y", "World Z"].iter().enumerate() {
                ui.selectable_value(&mut sym, i, *l);
            }
        });
        app.symmetry = sym;
    });
    ui.separator();
    const SECTIONS: &[(&str, &[&str])] = &[
        ("Mesh", &["combine", "separate", "smooth", "mirror", "fillHole", "triangulate", "quadrangulate", "cleanup"]),
        ("Components", &["extrude", "bevel", "bridge", "addDivisions", "poke", "chamferVertex", "merge", "mergeToCenter", "collapse", "deleteEdgeVertex", "extract", "duplicateFaces", "connect", "reverseNormals"]),
        ("Tools", &["multiCutTool", "targetWeldTool", "insertEdgeLoopTool", "createPolygonTool", "quadDrawTool"]),
        ("Normals & UVs", &["softenEdge", "hardenEdge", "planarMap", "automaticMap", "uvEditor"]),
    ];
    egui::ScrollArea::vertical().id_salt("mtk").show(ui, |ui| {
        for (title, ids) in SECTIONS {
            if !theme::fold(ui, title) {
                continue;
            }
            egui::Grid::new(("mtkg", *title)).num_columns(2).spacing([4.0, 2.0]).show(ui, |ui| {
                for (i, id) in ids.iter().enumerate() {
                    let r = ui.horizontal(|ui| {
                        let (ir, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        crate::icons::paint(ui, ir, crate::icons::for_action(id), Color32::WHITE);
                        ui.add(egui::Button::new(crate::actions::label(id)).frame(false))
                    }).inner;
                    let has_box = crate::options::has_box(id);
                    let r = r.on_hover_text(if has_box { "Right-click for options" } else { "" });
                    if r.clicked() {
                        app.action(id);
                    }
                    if r.secondary_clicked() && has_box {
                        crate::options::open(app, id);
                    }
                    if r.hovered() {
                        app.help = crate::actions::label(id);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        }
    });
}

/// Apply a channel edit to every selected object (Maya edits all selected channels).
/// One wheel notch over a field: +1 up, -1 down, 0 none.
fn wheel_notch(ui: &egui::Ui) -> f32 {
    ui.input(|i| i.events.iter().filter_map(|e| match e {
        egui::Event::MouseWheel { delta, .. } => Some(delta.y),
        _ => None,
    }).sum::<f32>().signum() * if i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. })) { 1.0 } else { 0.0 })
}

fn apply_rel(op: char, c: f64, a: f64) -> f64 {
    match op {
        '+' => c + a,
        '-' => c - a,
        '*' => c * a,
        _ if a != 0.0 => c / a,
        _ => c,
    }
}

/// Arithmetic typed into a channel field: numbers, + - * /, parentheses.
pub fn eval_expr(s: &str) -> Option<f64> {
    fn num(t: &[u8], i: &mut usize) -> Option<f64> {
        while *i < t.len() && t[*i] == b' ' {
            *i += 1;
        }
        if *i < t.len() && t[*i] == b'(' {
            *i += 1;
            let v = sum(t, i)?;
            while *i < t.len() && t[*i] == b' ' {
                *i += 1;
            }
            if *i < t.len() && t[*i] == b')' {
                *i += 1;
            }
            return Some(v);
        }
        if *i < t.len() && (t[*i] == b'-' || t[*i] == b'+') {
            let neg = t[*i] == b'-';
            *i += 1;
            return num(t, i).map(|v| if neg { -v } else { v });
        }
        let st = *i;
        while *i < t.len() && (t[*i].is_ascii_digit() || t[*i] == b'.' || ((t[*i] == b'e' || t[*i] == b'E') && *i > st) || ((t[*i] == b'-' || t[*i] == b'+') && *i > st && (t[*i - 1] == b'e' || t[*i - 1] == b'E'))) {
            *i += 1;
        }
        std::str::from_utf8(&t[st..*i]).ok()?.parse().ok()
    }
    fn prod(t: &[u8], i: &mut usize) -> Option<f64> {
        let mut v = num(t, i)?;
        loop {
            while *i < t.len() && t[*i] == b' ' {
                *i += 1;
            }
            match t.get(*i) {
                Some(b'*') => {
                    *i += 1;
                    v *= num(t, i)?;
                }
                Some(b'/') => {
                    *i += 1;
                    v /= num(t, i)?;
                }
                _ => return Some(v),
            }
        }
    }
    fn sum(t: &[u8], i: &mut usize) -> Option<f64> {
        let mut v = prod(t, i)?;
        loop {
            while *i < t.len() && t[*i] == b' ' {
                *i += 1;
            }
            match t.get(*i) {
                Some(b'+') => {
                    *i += 1;
                    v += prod(t, i)?;
                }
                Some(b'-') => {
                    *i += 1;
                    v -= prod(t, i)?;
                }
                _ => return Some(v),
            }
        }
    }
    let t = s.trim().as_bytes();
    let mut i = 0;
    let v = sum(t, &mut i)?;
    (i == t.len() && v.is_finite()).then_some(v)
}

/// Channel names in Channel Box order (for Shift range picks).
fn cb_order(s: &inca_core::scene::Scene, first: &NodeRef) -> Vec<String> {
    let mut v: Vec<String> = CHANNELS.iter().map(|c| c.to_string()).collect();
    if let NodeRef::Dag(id) = first {
        if let Some(e) = s.node(id).and_then(|n| n.extra.as_object()) {
            v.extend(e.iter().filter(|(_, x)| x.is_number() || x.is_boolean()).map(|(k, _)| k.clone()));
        }
        v.extend(s.shape_channels(id));
    }
    v
}

/// MMB drag in a viewport with channels picked in the Channel Box: the virtual slider.
pub fn virtual_slider(app: &mut IncaApp, resp: &egui::Response) -> bool {
    // Start on the MMB press itself. Waiting for egui's drag threshold lets the forced-axis
    // manipulator run a frame first, which jumps the object to the press point and makes the
    // slider start from there instead of from the object's real position.
    let pressed = resp.hovered() && resp.ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle));
    if (pressed || resp.drag_started_by(egui::PointerButton::Middle)) && app.panels.cb_armed && !app.panels.cb_pick.is_empty() {
        // every picked channel, on every object its row edits (history inputs included); whole-number
        // channels step in whole numbers within their range
        let mut list = vec![];
        for p in app.panels.cb_pick.clone() {
            for r in &p.targets {
                if let Some(v) = app.s.get_attr(r, &p.attr) {
                    list.push((r.clone(), p.attr.clone(), v.as_f64().unwrap_or(v.as_bool().map(|b| b as i32 as f64).unwrap_or(0.0)), p.int, p.min, p.max));
                }
            }
        }
        if !list.is_empty() {
            app.s.checkpoint("channel drag");
            app.panels.cb_drag = Some(list);
            app.panels.cb_drag_x = 0.0;
        }
    }
    let Some(list) = app.panels.cb_drag.clone() else { return false };
    if resp.dragged_by(egui::PointerButton::Middle) {
        let m = resp.ctx.input(|i| i.modifiers);
        app.panels.cb_drag_x += resp.drag_delta().x;
        // 0.05 used to be Slow; it is now Fast, with much finer medium and slow settings.
        let speed = cb_adjust_speed(app);
        let raw = app.panels.cb_drag_x as f64;
        let mapped = if app.panels.cb_hyperbolic {
            let sign = if raw < 0.0 { -1.0 } else { 1.0 };
            sign * raw.abs() * (1.0 + raw.abs() / 300.0)
        } else {
            raw
        };
        let d = mapped * speed * if m.ctrl || m.command { 0.1 } else if m.shift { 10.0 } else { 1.0 };
        for (r, a, start, int, lo, hi) in &list {
            let mut x = start + d;
            if let Some(lo) = lo {
                x = x.max(*lo);
            }
            if let Some(hi) = hi {
                x = x.min(*hi);
            }
            let v = if *int { json!(x.round() as i64) } else { json!(x) };
            app.s.set_attr(r, a, v);
        }
    }
    if resp.drag_stopped() || !resp.ctx.input(|i| i.pointer.middle_down()) {
        app.panels.cb_drag = None;
        let names: Vec<String> = list.iter().map(|(r, a, ..)| format!("{}.{a}", app.s.ref_name(r))).collect();
        app.echo(format!("// virtual slider: {}", names.join(" ")));
    }
    true
}

fn edit_channel(app: &mut IncaApp, targets: &[NodeRef], attr: &str, v: Value, resp: &egui::Response) {
    if (resp.drag_started() || (resp.changed() && !resp.dragged())) && !app.panels.editing {
        app.s.checkpoint(&format!("setAttr {attr}"));
        app.panels.editing = true;
    }
    for r in targets {
        app.s.set_attr(r, attr, v.clone());
        if app.s.time.auto_key {
            if let NodeRef::Dag(id) = r {
                if app.s.anim_is_keyed(id, attr) {
                    app.s.set_key(id, attr, None, None);
                }
            }
        }
    }
    let _ = resp;
}

fn finish_edit(app: &mut IncaApp, targets: &[NodeRef], attr: &str, resp: &egui::Response) {
    if (resp.drag_stopped() || resp.lost_focus() || (resp.changed() && !resp.dragged())) && app.panels.editing {
        app.panels.editing = false;
        for r in targets {
            let name = app.s.ref_name(r);
            if let Some(v) = app.s.get_attr(r, attr) {
                let vs = match &v {
                    Value::Number(n) => fmt_num(n.as_f64().unwrap_or(0.0)),
                    Value::Bool(b) => (*b as i32).to_string(),
                    Value::Array(a) => a.iter().map(|x| fmt_num(x.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join(" "),
                    Value::String(s) => format!("-type \"string\" \"{s}\""),
                    _ => String::new(),
                };
                app.echo(format!("setAttr \"{name}.{attr}\" {vs};"));
            }
        }
    }
}

fn channel_row(app: &mut IncaApp, ui: &mut egui::Ui, targets: &[NodeRef], attr: &str, label: &str, kind: ChanKind, min: Option<f64>, max: Option<f64>) {
    channel_row_impl(app, ui, targets, attr, label, kind, min, max, None);
}

/// Attribute Editor limit row: Min/Max XYZ plus the enable checkbox.
fn limit_row(ui: &mut egui::Ui, label: &str, lim: &mut Limit, changed: &mut bool) {
    egui::Grid::new(("limit", label)).num_columns(6).spacing([3.0, 3.0]).show(ui, |ui| {
        ui.label(label);
        ui.label("Min");
        for i in 0..3 {
            *changed |= ui.add_sized([42.0, 18.0], egui::DragValue::new(&mut lim.min[i]).speed(0.1)).changed();
        }
        *changed |= ui.checkbox(&mut lim.on, "Limit").changed();
        ui.end_row();
        ui.label("");
        ui.label("Max");
        for i in 0..3 {
            *changed |= ui.add_sized([42.0, 18.0], egui::DragValue::new(&mut lim.max[i]).speed(0.1)).changed();
        }
        ui.label("");
        ui.end_row();
    });
}

/// Attribute Editor vector row: one label followed by three numeric fields.
fn channel_vec_row(app: &mut IncaApp, ui: &mut egui::Ui, rows: &mut Vec<CbRow>, targets: &[NodeRef], attrs: [&str; 3], label: &str, decimals: Option<usize>) {
    ui.horizontal(|ui| {
        let (lr, _) = ui.allocate_exact_size(egui::vec2(90.0, 18.0), egui::Sense::hover());
        ui.painter().text(lr.right_center() - egui::vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER, label, egui::FontId::proportional(12.0), theme::TEXT);
        let gap = ui.spacing().item_spacing.x;
        let w = ((ui.available_width() - gap * 2.0) / 3.0).max(24.0);
        for (i, attr) in attrs.iter().enumerate() {
            let w = if i == 2 { ui.available_width().max(24.0) } else { w };
            cb_numeric_cell(app, ui, w, targets, attr, ChanKind::Float, None, None, decimals, rows, lr);
        }
    });
}

/// Channel Box row: also records its rects so drags across rows can select names or fields.
fn channel_row_cb(app: &mut IncaApp, ui: &mut egui::Ui, rows: &mut Vec<CbRow>, targets: &[NodeRef], attr: &str, label: &str, kind: ChanKind, min: Option<f64>, max: Option<f64>) {
    channel_row_impl(app, ui, targets, attr, label, kind, min, max, Some(rows));
}

fn channel_row_impl(app: &mut IncaApp, ui: &mut egui::Ui, targets: &[NodeRef], attr: &str, label: &str, kind: ChanKind, min: Option<f64>, max: Option<f64>, mut rows: Option<&mut Vec<CbRow>>) {
    let selectable = rows.is_some();
    let decimals = if rows.is_some() { app.panels.cb_precision } else { None };
    let Some(first) = targets.first() else { return };
    let Some(cur) = app.s.get_attr(first, attr) else { return };
    let id = app.s.ref_id(first);
    let keyed = app.s.anim_is_keyed(&id, attr);
    let on_key = app.s.anim_on_key(&id, attr, app.s.time.current);
    let driven = app.s.conns.iter().any(|c| c.dst == id && c.dst_attr == attr);
    let locked = match first {
        NodeRef::Dag(i) => app.s.node(i).is_some_and(|n| n.is_locked(attr)),
        _ => false,
    };
    let keyable = match first {
        NodeRef::Dag(i) => app.s.node(i).is_none_or(|n| n.is_keyable(attr)),
        _ => true,
    };
    let label_color = if keyable { theme::TEXT } else { theme::TEXT_DIM };
    ui.horizontal(|ui| {
        let lw = if attr == "rotateOrder" { 90.0 } else { 120.0 };
        let sense = if selectable { egui::Sense::click_and_drag() } else { egui::Sense::click() };
        let (rect, lresp) = ui.allocate_exact_size(egui::vec2(lw, 18.0), sense);
        let picked = app.panels.cb_pick.iter().any(|p| p.attr == attr && p.targets.first() == Some(first));
        let field_selected = selectable && cb_is_picked(&app.panels.cb_fields, first, attr);
        let field_key = (id.clone(), attr.to_string());
        let int = matches!(kind, ChanKind::Int | ChanKind::Bool | ChanKind::Enum(_));
        let me = CbPick { targets: targets.to_vec(), attr: attr.to_string(), int, min, max };
        if picked {
            ui.painter().rect_filled(rect, 2.0, theme::HI.gamma_multiply(0.6));
        }
        ui.painter().text(rect.right_center() - egui::vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, label, egui::FontId::proportional(12.0), label_color);
        if lresp.clicked() {
            // pick channels for the MMB virtual slider: click, Ctrl toggles, Shift adds a range
            let m = ui.input(|i| i.modifiers);
            let p = &mut app.panels;
            if m.ctrl || m.command {
                if picked {
                    p.cb_sel.retain(|c| c != attr);
                    p.cb_pick.retain(|q| !(q.attr == attr && q.targets.first() == Some(first)));
                } else {
                    p.cb_sel.push(attr.to_string());
                    p.cb_pick.push(me.clone());
                }
            } else if m.shift && p.cb_last.is_some() {
                let all = cb_order(&app.s, first);
                let (a, b) = (all.iter().position(|c| Some(c) == app.panels.cb_last.as_ref()), all.iter().position(|c| c == attr));
                if let (Some(a), Some(b)) = (a, b) {
                    for c in &all[a.min(b)..=a.max(b)] {
                        if !app.panels.cb_sel.contains(c) {
                            app.panels.cb_sel.push(c.clone());
                            app.panels.cb_pick.push(CbPick { targets: targets.to_vec(), attr: c.clone(), int: c == "visibility", min: None, max: None });
                        }
                    }
                }
            } else {
                p.cb_sel = vec![attr.to_string()];
                p.cb_pick = vec![me.clone()];
            }
            app.panels.cb_last = Some(attr.to_string());
            cb_arm(app);
            app.help = "Middle-drag in a viewport to change the selected channels (virtual slider; Ctrl slower, Shift faster)".into();
        }
        if selectable && lresp.drag_started_by(egui::PointerButton::Primary) {
            app.panels.cb_name_drag = Some(field_key.clone());
            app.panels.cb_drag_origin = ui.input(|i| i.pointer.press_origin());
            app.panels.cb_sel = vec![attr.to_string()];
            app.panels.cb_pick = vec![me.clone()];
            app.panels.cb_last = Some(attr.to_string());
            cb_arm(app);
            app.help = "Middle-drag in a viewport to change the selected channels (virtual slider; Ctrl slower, Shift faster)".into();
            if let Some(fid) = ui.memory(|m| m.focused()) {
                ui.memory_mut(|m| m.surrender_focus(fid));
            }
        }
        let fill = if field_selected {
            theme::HI
        } else if locked {
            theme::LOCKED
        } else if driven {
            theme::DRIVEN.gamma_multiply(0.6)
        } else if on_key {
            theme::KEY
        } else if keyed {
            theme::KEYED.gamma_multiply(0.7)
        } else {
            theme::FIELD
        };
        let w = if attr == "rotateOrder" { 70.0 } else { ui.available_width() };
        let frame = egui::Frame::NONE.fill(fill).show(ui, |ui| {
            ui.set_width(w);
            ui.add_enabled_ui(!locked, |ui| match kind {
                ChanKind::Bool => {
                    let mut b = cur.as_bool().unwrap_or(cur.as_f64().unwrap_or(0.0) != 0.0);
                    let lbl = if b { "on" } else { "off" };
                    let r = ui.checkbox(&mut b, lbl);
                    if r.clicked() {
                        cb_clear_names(app);
                    }
                    if r.changed() {
                        let v = if attr == "visibility" { json!(b as i32) } else { json!(b) };
                        edit_channel(app, targets, attr, v, &r);
                        finish_edit(app, targets, attr, &r);
                    }
                    r
                }
                ChanKind::Enum(opts) => {
                    let mut i = cur.as_f64().map(|x| x as usize).or_else(|| cur.as_str().and_then(|s| opts.iter().position(|o| *o == s))).unwrap_or(0);
                    let before = i;
                    let r = egui::ComboBox::from_id_salt(("enum", attr, id.clone())).width(w - 8.0).selected_text(opts.get(i).copied().unwrap_or("?")).show_index(ui, &mut i, opts.len(), |k| opts[k].to_string());
                    if r.clicked() {
                        cb_clear_names(app);
                    }
                    // the wheel steps through the options
                    if r.hovered() {
                        let sc = wheel_notch(ui);
                        if sc > 0.0 && i > 0 {
                            i -= 1;
                        } else if sc < 0.0 && i + 1 < opts.len() {
                            i += 1;
                        }
                    }
                    if i != before {
                        cb_clear_names(app);
                        let v = if cur.is_string() { json!(opts[i]) } else { json!(i) };
                        app.s.checkpoint(&format!("setAttr {attr}"));
                        for t in targets {
                            app.s.set_attr(t, attr, v.clone());
                        }
                        app.echo(format!("setAttr \"{}.{attr}\" {i};", app.s.ref_name(first)));
                    }
                    r
                }
                _ => {
                    let x = cur.as_f64().unwrap_or(0.0);
                    if selectable {
                        let rows = rows.as_deref_mut().expect("selectable channel rows");
                        cb_numeric_cell(app, ui, w - 4.0, targets, attr, kind, min, max, decimals, rows, rect)
                    } else {
                        let mut x = x;
                        // typed expressions: "2*3", or relative "+=2" "-=1" "*=2" "/=2" applied to each selected object's own value
                        let rel: std::cell::Cell<Option<(char, f64)>> = std::cell::Cell::new(None);
                        let x0 = x;
                        let mut dv = egui::DragValue::new(&mut x).speed(if kind == ChanKind::Int { 0.1 } else { 0.02 }).max_decimals(3).custom_parser(|t| {
                            let t = t.trim();
                            for op in ['+', '-', '*', '/'] {
                                if let Some(rest) = t.strip_prefix(op).and_then(|r| r.strip_prefix('=')) {
                                    let a = eval_expr(rest)?;
                                    rel.set(Some((op, a)));
                                    return Some(apply_rel(op, x0, a));
                                }
                            }
                            eval_expr(t.strip_prefix('=').unwrap_or(t))
                        });
                        if kind == ChanKind::Int {
                            dv = dv.fixed_decimals(0);
                        }
                        if let (Some(a), Some(b)) = (min, max) {
                            dv = dv.range(a..=b);
                        } else if let Some(a) = min {
                            dv = dv.range(a..=f64::MAX);
                        }
                        let r = ui.add_sized([w - 4.0, 18.0], dv);
                        // the wheel over an int channel steps it by one
                        let mut stepped = false;
                        if kind == ChanKind::Int && r.hovered() && !locked {
                            let sc = wheel_notch(ui);
                            if sc != 0.0 {
                                x += sc.signum() as f64;
                                if let Some(a) = min {
                                    x = x.max(a);
                                }
                                if let Some(b) = max {
                                    x = x.min(b);
                                }
                                stepped = true;
                            }
                        }
                        if let (true, Some((op, a))) = (r.changed(), rel.get()) {
                            app.s.checkpoint(&format!("setAttr {attr}"));
                            for t in targets {
                                let c = app.s.get_attr(t, attr).and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let nv = apply_rel(op, c, a);
                                app.s.set_attr(t, attr, if kind == ChanKind::Int { json!(nv.round() as i64) } else { json!(nv) });
                            }
                            app.echo(format!("setAttr -r ... {attr} {op}= {}", fmt_num(a)));
                        } else if r.changed() && !r.dragged() && picked && app.panels.cb_pick.len() > 1 {
                            // a value typed into one of several picked channels goes into all of them
                            app.s.checkpoint(&format!("setAttr {attr}"));
                            for p in app.panels.cb_pick.clone() {
                                for t in &p.targets {
                                    app.s.set_attr(t, &p.attr, if p.int { json!(x.round() as i64) } else { json!(x) });
                                }
                            }
                            app.echo(format!("// set {} picked channels to {}", app.panels.cb_pick.len(), fmt_num(x)));
                        } else if r.changed() || stepped {
                            let v = if kind == ChanKind::Int { json!(x.round() as i64) } else { json!(x) };
                            if stepped {
                                app.s.checkpoint(&format!("setAttr {attr}"));
                                for t in targets {
                                    app.s.set_attr(t, attr, v.clone());
                                }
                            } else {
                                edit_channel(app, targets, attr, v, &r);
                            }
                        }
                        finish_edit(app, targets, attr, &r);
                        r
                    }
                }
            })
            .inner
        });
        if let Some(rows) = rows {
            let numeric = matches!(kind, ChanKind::Float | ChanKind::Int);
            if !(selectable && numeric) {
                rows.push(CbRow {
                    key: field_key.clone(),
                    pick: me.clone(),
                    label: rect,
                    field: Some(frame.response.rect),
                    numeric,
                    locked,
                });
            }
        }
        let channel_menu = app.right_tab == RightTab::ChannelBox;
        if channel_menu && lresp.secondary_clicked() {
            if !cb_is_picked(&app.panels.cb_pick, first, attr) {
                app.panels.cb_sel = vec![attr.to_string()];
                app.panels.cb_pick = vec![me.clone()];
            }
            app.panels.cb_last = Some(attr.to_string());
            cb_arm(app);
        }
        if channel_menu {
            lresp.context_menu(|ui| {
                ui.label(RichText::new("Channels").strong().color(theme::TEXT_DIM));
                ui.separator();
                channels_menu(app, ui);
            });
        }
    });
}

/// Resolve drag selections after the whole Channel Box has been drawn, using this frame's rows.
fn finish_cb_rows(app: &mut IncaApp, ui: &egui::Ui, rows: &[CbRow]) {
    if rows.is_empty() {
        app.panels.cb_fields.clear();
        app.panels.cb_field_last = None;
        app.panels.cb_field_click = None;
        app.panels.cb_name_drag = None;
        app.panels.cb_field_drag = None;
        app.panels.cb_editing = None;
        app.panels.cb_edit_new = false;
        app.panels.cb_row_rects.clear();
        return;
    }
    let (pos, primary_down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
    // Shift-click extends the field selection from the last selected field.
    if let Some((key, shift)) = app.panels.cb_field_click.take() {
        if shift {
            if let Some(cur) = rows.iter().position(|r| r.key == key) {
                let anchor = app.panels.cb_field_last.clone();
                let (picks, last) = match anchor.as_ref().and_then(|a| rows.iter().position(|r| &r.key == a)) {
                    Some(a) => cb_rows_range(rows, &rows[a].key, cur, true),
                    None => (Vec::new(), None),
                };
                let (picks, last) = if picks.is_empty() {
                    let only = rows.get(cur).filter(|r| r.numeric && !r.locked).map(|r| (vec![r.pick.clone()], Some(r.key.clone())));
                    only.unwrap_or((Vec::new(), None))
                } else {
                    (picks, last)
                };
                if !picks.is_empty() {
                    app.panels.cb_fields = picks;
                    app.panels.cb_field_last = last;
                }
            }
        }
    }
    // Drag over channel names selects every name inside the marquee box.
    if let Some(origin) = app.panels.cb_drag_origin {
        if let (Some(pos), true) = (pos, app.panels.cb_name_drag.is_some()) {
            let rect = Rect::from_two_pos(origin, pos);
            let (picks, last) = cb_rows_in_box(rows, rect, false);
            if !picks.is_empty() {
                let mut sel: Vec<String> = Vec::new();
                for p in &picks {
                    if !sel.contains(&p.attr) {
                        sel.push(p.attr.clone());
                    }
                }
                app.panels.cb_sel = sel;
                app.panels.cb_pick = picks;
                app.panels.cb_last = last.map(|(_, a)| a);
                cb_arm(app);
            }
            theme::marquee(&ui.painter(), rect);
        }
    }
    // Drag over value fields selects every field inside the marquee box. While the pointer is
    // still on the pressed field, keep the existing selection (a click with a little movement
    // is not a marquee).
    if let Some(origin) = app.panels.cb_drag_origin {
        if let (Some(pos), Some(anchor)) = (pos, app.panels.cb_field_drag.clone()) {
            let rect = Rect::from_two_pos(origin, pos);
            let inside_anchor = rows.iter().find(|r| r.key == anchor).and_then(|r| r.field).is_some_and(|f| f.contains(pos));
            if !inside_anchor {
                let (picks, last) = cb_rows_in_box(rows, rect, true);
                if !picks.is_empty() {
                    app.panels.cb_fields = picks;
                    app.panels.cb_field_last = last;
                }
                theme::marquee(&ui.painter(), rect);
            }
        }
    }
    if app.panels.cb_name_drag.is_some() && !primary_down {
        app.panels.cb_name_drag = None;
    }
    if app.panels.cb_field_drag.is_some() && !primary_down {
        app.panels.cb_field_drag = None;
        if let Some(last) = app.panels.cb_field_last.clone() {
            if let Some(row) = rows.iter().find(|r| r.key == last && r.numeric && !r.locked) {
                let old = row.pick.targets.first().and_then(|t| app.s.get_attr(t, &row.pick.attr)).and_then(|v| v.as_f64()).unwrap_or(0.0);
                app.panels.cb_edit_text = if row.pick.int { format!("{}", old.round() as i64) } else { fmt_num(old) };
                app.panels.cb_editing = Some(last);
                app.panels.cb_edit_new = true;
            }
        }
    }
    if !primary_down {
        app.panels.cb_drag_origin = None;
    }
    // Drop selections/editors whose rows are no longer displayed or editable.
    app.panels.cb_fields.retain(|p| rows.iter().any(|r| r.numeric && !r.locked && r.pick.attr == p.attr && r.pick.targets.first() == p.targets.first()));
    if let Some(key) = app.panels.cb_editing.clone() {
        if !rows.iter().any(|r| r.key == key && r.numeric && !r.locked) {
            app.panels.cb_editing = None;
        }
    }
    app.panels.cb_numeric_rects = rows.iter().filter(|r| r.numeric && !r.locked).filter_map(|r| r.field).collect();
    app.panels.cb_row_rects = rows.iter().map(|r| (r.key.0.clone(), r.key.1.clone(), r.label, r.field.unwrap_or(r.label))).collect();
}

pub fn channel_box(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(lead) = app.s.lead().cloned() else {
        ui.label(RichText::new("Nothing selected").color(theme::TEXT_DIM));
        app.panels.cb_fields.clear();
        app.panels.cb_editing = None;
        app.panels.cb_name_drag = None;
        app.panels.cb_field_drag = None;
        app.panels.cb_row_rects.clear();
        return;
    };
    let Some(n) = app.s.node(&lead) else { return };
    let full_name = n.name.clone();
    let name = shown_node_name(app, &full_name);
    let shape = n.shape_name.clone();
    let mut cb_rows: Vec<CbRow> = Vec::new();
    // object name (editable; keep the edit buffer between frames so typing is not reset)
    if app.panels.cb_name.0 != name {
        app.panels.cb_name = (name.clone(), name.clone());
    }
    let r = ui.add(egui::TextEdit::singleline(&mut app.panels.cb_name.1).font(egui::TextStyle::Heading).desired_width(f32::INFINITY));
    app.panels.cb_name_rect = Some(r.rect);
    if r.lost_focus() {
        let new = app.panels.cb_name.1.trim().to_string();
        if !new.is_empty() && new != name {
            let _ = app.run_mel("rename", &format!("rename {full_name} \"{new}\";"));
            return;
        }
    }
    // the lead object comes first: its values are the ones shown
    let mut targets: Vec<NodeRef> = vec![NodeRef::Dag(lead.clone())];
    targets.extend(app.s.sel.iter().filter(|i| **i != lead).map(|i| NodeRef::Dag(i.clone())));
    let filter = cb_filter(app);
    // Published attributes of the enclosing asset, shown per the Show Assets options.
    let asset = app.s.container_of(&lead);
    let asset_rows: Vec<(String, ChanKind)> = asset.as_ref().and_then(|aid| app.s.node(aid).map(|n| {
        n.published
            .iter()
            .filter(|(_, p)| app.panels.cb_asset_mode == 0 || app.s.sel.contains(&p.node))
            .filter_map(|(name, _)| match app.s.get_attr(&NodeRef::Dag(aid.clone()), name) {
                Some(Value::Bool(_)) => Some((name.clone(), ChanKind::Bool)),
                Some(Value::Number(x)) if x.is_f64() => Some((name.clone(), ChanKind::Float)),
                Some(Value::Number(_)) => Some((name.clone(), ChanKind::Int)),
                _ => None,
            })
            .collect()
    })).unwrap_or_default();
    if app.panels.cb_asset_top {
        if let (Some(aid), false) = (&asset, asset_rows.is_empty()) {
            ui.add_space(4.0);
            theme::section_header(ui, "ASSET");
            for (k, kind) in &asset_rows {
                if app.s.node(aid).is_some_and(|n| n.channel_hidden(k)) || !cb_custom_shown(app, k) {
                    continue;
                }
                channel_row_cb(app, ui, &mut cb_rows, &[NodeRef::Dag(aid.clone())], k, k, *kind, None, None);
            }
        }
    }
    for c in CHANNELS {
        if !cb_shown(filter, c) || !cb_custom_shown(app, c) || app.s.node(&lead).is_some_and(|n| n.channel_hidden(c)) {
            continue;
        }
        let kind = if c == "visibility" { ChanKind::Bool } else { ChanKind::Float };
        let label = cb_label(app, c);
        channel_row_cb(app, ui, &mut cb_rows, &targets, c, &label, kind, None, None);
    }
    // added (dynamic) attributes, unless the SHAPES section lists them
    let sh_channels = app.s.shape_channels(&lead);
    let extra: Vec<(String, ChanKind)> = app.s.node(&lead).and_then(|n| n.extra.as_object()).map(|e| {
        e.iter()
            .filter(|(k, _)| !sh_channels.contains(k))
            .filter_map(|(k, v)| match v {
                Value::Bool(_) => Some((k.clone(), ChanKind::Bool)),
                Value::Number(n) if n.is_f64() => Some((k.clone(), ChanKind::Float)),
                Value::Number(_) => Some((k.clone(), ChanKind::Int)),
                _ => None,
            })
            .collect()
    }).unwrap_or_default();
    for (k, kind) in extra {
        if app.s.node(&lead).is_some_and(|n| n.channel_hidden(&k)) || !cb_custom_shown(app, &k) {
            continue;
        }
        let label = cb_label(app, &k);
        channel_row_cb(app, ui, &mut cb_rows, &targets, &k, &label, kind, None, None);
    }
    // PUBLISHED (container interface attributes)
    let published: Vec<(String, ChanKind)> = app.s.node(&lead).map(|n| {
        n.published
            .keys()
            .filter_map(|name| match app.s.get_attr(&NodeRef::Dag(lead.clone()), name) {
                Some(Value::Bool(_)) => Some((name.clone(), ChanKind::Bool)),
                Some(Value::Number(x)) if x.is_f64() => Some((name.clone(), ChanKind::Float)),
                Some(Value::Number(_)) => Some((name.clone(), ChanKind::Int)),
                _ => None,
            })
            .collect()
    }).unwrap_or_default();
    if !published.is_empty() {
        ui.add_space(4.0);
        theme::section_header(ui, "PUBLISHED");
        for (k, kind) in published {
            if app.s.node(&lead).is_some_and(|n| n.channel_hidden(&k)) || !cb_custom_shown(app, &k) {
                continue;
            }
            channel_row_cb(app, ui, &mut cb_rows, &[NodeRef::Dag(lead.clone())], &k, &k, kind, None, None);
        }
    }
    if !app.panels.cb_asset_top {
        if let (Some(aid), false) = (&asset, asset_rows.is_empty()) {
            ui.add_space(4.0);
            theme::section_header(ui, "ASSET");
            for (k, kind) in &asset_rows {
                if app.s.node(aid).is_some_and(|n| n.channel_hidden(k)) || !cb_custom_shown(app, k) {
                    continue;
                }
                channel_row_cb(app, ui, &mut cb_rows, &[NodeRef::Dag(aid.clone())], k, k, *kind, None, None);
            }
        }
    }
    // SHAPES
    let shape = shape.or_else(|| (!sh_channels.is_empty()).then(|| app.s.node(&lead).map(|n| n.kind.clone()).unwrap_or_default()));
    if let Some(sn) = shape {
        ui.add_space(4.0);
        theme::section_header(ui, "SHAPES");
        ui.label(RichText::new(sn).color(theme::TEXT));
        for c in sh_channels {
            if app.s.node(&lead).is_some_and(|n| n.channel_hidden(&c)) || !cb_custom_shown(app, &c) {
                continue;
            }
            let v = app.s.get_attr(&NodeRef::Dag(lead.clone()), &c);
            let kind = match v {
                Some(Value::Bool(_)) => ChanKind::Bool,
                _ if c == "smoothLevel" => ChanKind::Int,
                _ => ChanKind::Float,
            };
            let (mn, mx) = if c == "smoothLevel" { (Some(0.0), Some(3.0)) } else { (None, None) };
            let label = cb_label(app, &c);
            channel_row_cb(app, ui, &mut cb_rows, &[NodeRef::Dag(lead.clone())], &c, &label, kind, mn, mx);
        }
    }
    // INPUTS (construction history, newest first); a deformer handle shows its deformer
    let mut hist: Vec<(String, usize, String, String)> = app.s.node(&lead).and_then(|n| n.mesh()).map(|m| m.history.iter().enumerate().filter(|(_, h)| !h.name.contains('#')).map(|(i, h)| (lead.clone(), i, h.name.clone(), h.ty.clone())).collect()).unwrap_or_default();
    hist.reverse();
    if app.s.node(&lead).is_some_and(|n| n.kind == "deformer") {
        for n in app.s.nodes.values() {
            if let Some(m) = n.mesh() {
                for (i, h) in m.history.iter().enumerate() {
                    if h.params.get("handle").and_then(|v| v.as_str()) == Some(lead.as_str()) {
                        hist.push((n.id.clone(), i, h.name.clone(), h.ty.clone()));
                    }
                }
            }
        }
    }
    if !hist.is_empty() {
        ui.add_space(4.0);
        theme::section_header(ui, "INPUTS");
        for (owner, i, hname, ty) in hist {
            let lead = owner;
            let open_id = format!("{lead}:{hname}");
            let open = app.panels.expanded.contains(&open_id);
            let err = app.s.node(&lead).and_then(|n| n.mesh()).and_then(|m| m.history[i].error.clone());
            let t = RichText::new(format!("{} {hname}", if open { "-" } else { "+" })).color(if err.is_some() { Color32::from_rgb(240, 120, 120) } else { theme::TEXT });
            let r = ui.add(egui::Button::new(t).frame(false));
            if let Some(e) = err {
                r.clone().on_hover_text(e);
            }
            if r.clicked() {
                if open {
                    app.panels.expanded.remove(&open_id);
                } else {
                    app.panels.expanded.insert(open_id);
                }
            }
            if open {
                if let Some((_, _, chans)) = ops::info(&ty) {
                    let r = NodeRef::History(lead.clone(), i);
                    for c in chans {
                        if !cb_custom_shown(app, c.key) {
                            continue;
                        }
                        let label = match app.panels.cb_names {
                            1 => c.key.to_string(),
                            2 => cb_short_name(c.key).to_string(),
                            _ => c.label.map(String::from).unwrap_or_else(|| ops::nice_name(c.key)),
                        };
                        channel_row_cb(app, ui, &mut cb_rows, std::slice::from_ref(&r), c.key, &label, c.kind, c.min, c.max);
                    }
                }
            }
        }
    }
    finish_cb_rows(app, ui, &cb_rows);
}

/// Channels menu: keying and Graph Editor/Time Slider sync.
/// The Channels menu, shared by the menu bar and the channel-name context menu.
fn channels_menu(app: &mut IncaApp, ui: &mut egui::Ui) {
    use crate::menus::{menu_check, menu_item};
    if menu_item(app, ui, "Key Selected") {
        if !key_selected_channels(app) {
            app.action("setKey");
        }
    }
    if menu_item(app, ui, "Key All Keyable") {
        let _ = app.run_mel("Key All Keyable", "setKeyframe;");
    }
    ui.separator();
    if menu_item(app, ui, "Breakdown Selected") {
        breakdown_channels(app, false);
    }
    if menu_item(app, ui, "Breakdown All") {
        breakdown_channels(app, true);
    }
    ui.separator();
    if menu_item(app, ui, "Mute Selected") {
        mute_channels(app, true, false);
    }
    if menu_item(app, ui, "Mute All") {
        mute_channels(app, true, true);
    }
    if menu_item(app, ui, "Unmute Selected") {
        mute_channels(app, false, false);
    }
    if menu_item(app, ui, "Unmute All") {
        mute_channels(app, false, true);
    }
    ui.separator();
    if menu_check(app, ui, app.panels.cb_sync_graph, "Sync Selection in Graph Editor") {
        app.panels.cb_sync_graph = !app.panels.cb_sync_graph;
    }
    if menu_check(app, ui, app.panels.cb_sync_timeline, "Sync Timeline Display") {
        app.panels.cb_sync_timeline = !app.panels.cb_sync_timeline;
    }
    ui.separator();
    if menu_item(app, ui, "Cut Selected") {
        if copy_channel_keys(app) {
            delete_channel_keys(app);
        }
    }
    if menu_item(app, ui, "Copy Selected") {
        copy_channel_keys(app);
    }
    if menu_item(app, ui, "Paste Selected") {
        if !app.key_clip.is_empty() {
            let n: usize = app.key_clip.iter().map(|c| c.2.len()).sum();
            crate::timeline::paste_keys(app);
            app.help = format!("Pasted {n} timeline key{}", if n == 1 { "" } else { "s" });
        }
    }
    if menu_item(app, ui, "Delete Selected") {
        delete_channel_keys(app);
    }
    ui.separator();
    if menu_item(app, ui, "Duplicate Values") {
        duplicate_channel_values(app);
    }
    if menu_item(app, ui, "Freeze") {
        app.action("freezeTransformations");
    }
    ui.separator();
    if menu_item(app, ui, "Break Connections") {
        break_channel_connections(app);
    }
    if menu_item(app, ui, "Select Connection") {
        select_channel_connection(app);
    }
    ui.separator();
    if menu_item(app, ui, "Lock Selected") {
        lock_channels(app, true);
    }
    if menu_item(app, ui, "Unlock Selected") {
        lock_channels(app, false);
    }
    ui.separator();
    if menu_item(app, ui, "Hide Selected") {
        hide_channels(app, false);
    }
    if menu_item(app, ui, "Lock and Hide Selected") {
        hide_channels(app, true);
    }
    ui.separator();
    if menu_item(app, ui, "Make Selected Nonkeyable") {
        set_channel_keyable(app, false);
    }
    if menu_item(app, ui, "Make Selected Keyable") {
        set_channel_keyable(app, true);
    }
    ui.separator();
    if menu_item(app, ui, "Edit Expressions...") {
        if let Some(lead) = app.s.lead().cloned() {
            let attr = app.panels.cb_pick.first().map(|p| p.attr.clone()).unwrap_or_else(|| "translateX".into());
            app.panels.expression_text = app.s.node(&lead).and_then(|n| n.expressions.get(&attr).cloned()).unwrap_or_default();
            app.panels.expression_attr = attr;
            app.win.expression_editor = true;
        }
    }
    ui.separator();
    if menu_item(app, ui, "Add to Selected Layers") {
        layer_members(app, true);
    }
    if menu_item(app, ui, "Remove From Selected Layers") {
        layer_members(app, false);
    }
}

fn channel_box_menu(app: &mut IncaApp, ui: &mut egui::Ui) {
    use crate::menus::{menu_check, menu_item, menu_radio, panel_bar, MenuSource};
    panel_bar(app, ui, MenuSource::ChannelBox, &["Channels", "Edit", "Object", "Show"], false, |app, ui, title| match title {
        "Channels" => channels_menu(app, ui),
        "Edit" => {
            if menu_item(app, ui, "Set Driven Key...") {
                app.action("setDrivenKeyWindow");
            }
            if menu_item(app, ui, "Graph Editor") {
                app.win.graph_editor = true;
            }
            if menu_item(app, ui, "Channel Control...") {
                app.win.channel_control = true;
            }
            if menu_item(app, ui, "Connection Editor") {
                app.action("connectionEditor");
            }
            if menu_item(app, ui, "Attribute Editor") {
                app.action("attributeEditor");
            }
            if menu_item(app, ui, "Material Attributes...") {
                app.action("materialAttributes");
            }
            ui.separator();
            if menu_item(app, ui, "Add Attribute...") {
                app.action("addAttribute");
            }
            if menu_item(app, ui, "Edit Attribute...") {
                if let Some(lead) = app.s.lead().cloned() {
                    let attr = app.panels.cb_pick.iter().find_map(|p| {
                        let id = app.s.ref_id(p.targets.first()?);
                        app.s.node(&id).and_then(|n| n.extra.as_object()).is_some_and(|e| e.contains_key(&p.attr)).then(|| p.attr.clone())
                    }).or_else(|| app.s.node(&lead).and_then(|n| n.extra.as_object()).and_then(|e| e.keys().next().cloned()));
                    if let Some(attr) = attr {
                        app.panels.edit_attr_name = attr.clone();
                        app.panels.edit_attr_new = attr;
                        app.win.edit_attributes = true;
                    } else {
                        app.help = "The selected node has no custom attributes".into();
                    }
                }
            }
            if menu_item(app, ui, "Delete Attributes...") {
                for p in app.panels.cb_pick.clone() {
                    if let Some(t) = p.targets.first() {
                        let id = app.s.ref_id(t);
                        if app.s.node(&id).and_then(|n| n.extra.as_object()).is_some_and(|e| e.contains_key(&p.attr)) {
                            app.panels.delete_attr_checked.insert(p.attr.clone());
                        }
                    }
                }
                app.win.delete_attributes = true;
            }
            if menu_item(app, ui, "Publish to Asset") {
                app.action("publishAttributes");
            }
            if menu_item(app, ui, "Unpublish from Asset") {
                unpublish_channels(app);
            }
            if menu_item(app, ui, "Lock Unpublished Attributes") {
                lock_unpublished_attributes(app);
            }
            ui.separator();
            if menu_item(app, ui, "Select Node") {
                if let Some(id) = app.s.lead().cloned() {
                    app.s.select(&[id], SelMode::Replace);
                }
            }
            if menu_item(app, ui, "Delete Node") {
                app.action("delete");
            }
            if menu_item(app, ui, "Delete History") {
                app.action("deleteHistory");
            }
            ui.menu_button("Settings", |ui| {
                ui.menu_button("Attribute Adjustment Speed", |ui| {
                    for (value, label) in [(1u8, "Slow"), (0u8, "Medium"), (2u8, "Fast")] {
                        if menu_check(app, ui, app.panels.cb_speed == value, label) {
                            app.panels.cb_speed = value;
                        }
                    }
                });
                if menu_check(app, ui, app.panels.cb_hyperbolic, "Hyperbolic") {
                    app.panels.cb_hyperbolic = !app.panels.cb_hyperbolic;
                }
                if menu_check(app, ui, !app.panels.cb_hide_namespace, "Show Namespace") {
                    app.panels.cb_hide_namespace = !app.panels.cb_hide_namespace;
                }
                ui.separator();
                let mode = cb_manip_mode(app);
                if menu_radio(app, ui, mode == 0, "No Manips") {
                    app.panels.cb_manips = Some(0);
                    app.panels.cb_armed = false;
                }
                if menu_radio(app, ui, mode == 1, "Invisible Manips") {
                    app.panels.cb_manips = Some(1);
                    cb_arm(app);
                }
                if menu_radio(app, ui, mode == 2, "Standard Manips") {
                    app.panels.cb_manips = None;
                    cb_arm(app);
                }
                ui.menu_button("Change Precision", |ui| {
                    if menu_check(app, ui, app.panels.cb_precision.is_none(), "Default") {
                        app.panels.cb_precision = None;
                    }
                    for d in 0..=6 {
                        if menu_check(app, ui, app.panels.cb_precision == Some(d), &format!("{d} decimal{}", if d == 1 { "" } else { "s" })) {
                            app.panels.cb_precision = Some(d);
                        }
                    }
                });
                if menu_item(app, ui, "Reset to Default") {
                    app.panels.cb_speed = 0;
                    app.panels.cb_names = 0;
                    app.panels.cb_show = None;
                    app.panels.cb_custom_active = None;
                    app.panels.cb_precision = None;
                    app.panels.cb_manips = None;
                    app.panels.cb_hyperbolic = false;
                    app.panels.cb_hide_namespace = false;
                    cb_arm(app);
                }
            });
            ui.menu_button("Channel Names", |ui| {
                for (value, label) in [(0u8, "Nice"), (1u8, "Long"), (2u8, "Short")] {
                    if menu_check(app, ui, app.panels.cb_names == value, label) {
                        app.panels.cb_names = value;
                    }
                }
            });
        }
        "Object" => {
            let lead = app.s.lead().cloned();
            for id in app.s.sel.clone() {
                let name = app.s.name_of(&id);
                if menu_check(app, ui, lead.as_deref() == Some(id.as_str()), &name) {
                    app.s.sel.retain(|x| x != &id);
                    app.s.sel.push(id);
                }
            }
        }
        "Show" => {
            ui.menu_button("Attributes", |ui| {
                let mut f = cb_filter(app);
                for (i, name) in ["Translate", "Rotate", "Scale", "Visibility"].iter().enumerate() {
                    if menu_check(app, ui, f[i], name) {
                        f[i] = !f[i];
                    }
                }
                app.panels.cb_show = Some(f);
            });
            if menu_item(app, ui, "Show All") {
                app.panels.cb_show = None;
                app.panels.cb_custom_active = None;
            }
            if menu_item(app, ui, "Invert Shown") {
                app.panels.cb_show = Some(cb_filter(app).map(|b| !b));
            }
            if menu_item(app, ui, "Clear Below") {
                app.panels.cb_show = None;
                app.panels.cb_custom_active = None;
            }
            if menu_item(app, ui, "Show Selected Type(s)") {
                show_selected_channel_types(app);
            }
            ui.separator();
            if menu_item(app, ui, "Create Entry...") {
                create_custom_show_entry(app);
            }
            let entries = app.panels.cb_custom_entries.clone();
            if !entries.is_empty() {
                ui.menu_button("Custom Entries", |ui| {
                    if menu_check(app, ui, app.panels.cb_custom_active.is_none(), "Off") {
                        app.panels.cb_custom_active = None;
                    }
                    for (i, (name, _)) in entries.iter().enumerate() {
                        if menu_check(app, ui, app.panels.cb_custom_active == Some(i), name) {
                            app.panels.cb_custom_active = Some(i);
                        }
                    }
                });
                ui.menu_button("Delete Entry...", |ui| {
                    for (i, (name, _)) in entries.iter().enumerate() {
                        if menu_item(app, ui, name) {
                            app.panels.cb_custom_entries.remove(i);
                            app.panels.cb_custom_active = None;
                            break;
                        }
                    }
                });
            }
            ui.separator();
            ui.menu_button("Show Assets", |ui| {
                if menu_check(app, ui, app.panels.cb_asset_mode == 0, "All Attributes") {
                    app.panels.cb_asset_mode = 0;
                }
                if menu_check(app, ui, app.panels.cb_asset_mode == 1, "Selected Object Attributes") {
                    app.panels.cb_asset_mode = 1;
                }
                ui.separator();
                if menu_check(app, ui, app.panels.cb_asset_top, "Show at Top") {
                    app.panels.cb_asset_top = !app.panels.cb_asset_top;
                }
            });
        }
        _ => {}
    });
}

/// Channel Control: keyable / display / lock state for every channel of the lead object.
pub(crate) fn channel_control_window(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(lead) = app.s.lead().cloned() else {
        ui.label(RichText::new("Select an object to edit its channels.").color(theme::TEXT_DIM));
        return;
    };
    let Some(node) = app.s.node(&lead) else { return };
    let name = node.name.clone();
    let channels = cb_order(&app.s, &NodeRef::Dag(lead.clone()));
    ui.label(RichText::new(format!("{name}:")).color(theme::TEXT));
    ui.add_space(2.0);
    let mut changes: Vec<(String, bool, bool, bool)> = vec![];
    egui::Grid::new(("channel_control", lead.clone())).num_columns(4).spacing([10.0, 2.0]).striped(true).show(ui, |ui| {
        ui.label(RichText::new("Channel").color(theme::TEXT_DIM));
        ui.label(RichText::new("Keyable").color(theme::TEXT_DIM));
        ui.label(RichText::new("Display").color(theme::TEXT_DIM));
        ui.label(RichText::new("Locked").color(theme::TEXT_DIM));
        ui.end_row();
        for attr in &channels {
            let mut keyable = node.is_keyable(attr);
            let mut shown = !node.channel_hidden(attr);
            let mut locked = node.is_locked(attr);
            let before = (keyable, shown, locked);
            ui.label(RichText::new(cb_label(app, attr)).color(if keyable { theme::TEXT } else { theme::TEXT_DIM }));
            ui.checkbox(&mut keyable, "");
            ui.checkbox(&mut shown, "");
            ui.checkbox(&mut locked, "");
            ui.end_row();
            if before != (keyable, shown, locked) {
                changes.push((attr.clone(), keyable, shown, locked));
            }
        }
    });
    if let Some(n) = app.s.nodes.get_mut(&lead) {
        for (attr, keyable, shown, locked) in changes {
            if keyable {
                n.nonkeyable.remove(&attr);
            } else {
                n.nonkeyable.insert(attr.clone());
            }
            if shown {
                n.hidden_channels.remove(&attr);
            } else {
                n.hidden_channels.insert(attr.clone());
            }
            n.locks.insert(attr, json!(locked));
        }
        app.s.touch();
    }
}

/// Expression Editor: one arithmetic expression per channel, evaluated during animation updates.
pub(crate) fn expression_editor_window(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(lead) = app.s.lead().cloned() else {
        ui.label(RichText::new("Select an object with channels.").color(theme::TEXT_DIM));
        return;
    };
    let Some(node) = app.s.node(&lead) else { return };
    let name = node.name.clone();
    let channels: Vec<String> = cb_order(&app.s, &NodeRef::Dag(lead.clone()));
    if app.panels.expression_attr.is_empty() || !channels.contains(&app.panels.expression_attr) {
        app.panels.expression_attr = channels.first().cloned().unwrap_or_else(|| "translateX".into());
        app.panels.expression_text = node.expressions.get(&app.panels.expression_attr).cloned().unwrap_or_default();
    }
    ui.label(RichText::new(format!("Expression for {name}:")).color(theme::TEXT));
    ui.add_space(2.0);
    let old_attr = app.panels.expression_attr.clone();
    egui::ComboBox::from_id_salt("expr_attr").selected_text(&app.panels.expression_attr).show_ui(ui, |ui| {
        for attr in &channels {
            if ui.selectable_label(*attr == app.panels.expression_attr, attr).clicked() {
                app.panels.expression_attr = attr.clone();
                app.panels.expression_text = app.s.node(&lead).and_then(|n| n.expressions.get(attr).cloned()).unwrap_or_default();
            }
        }
    });
    if old_attr != app.panels.expression_attr && app.panels.expression_text.is_empty() {
        app.panels.expression_text = app.s.node(&lead).and_then(|n| n.expressions.get(&app.panels.expression_attr).cloned()).unwrap_or_default();
    }
    ui.add(egui::TextEdit::multiline(&mut app.panels.expression_text).desired_rows(3).desired_width(f32::INFINITY).hint_text("e.g. sin(time * 2) + translateY"));
    ui.label(RichText::new("time/frame, channel names, + - * / % ^, sin, cos, abs, min, max, pow").color(theme::TEXT_DIM).size(10.5));
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Apply").clicked() {
            let attr = app.panels.expression_attr.clone();
            let src = app.panels.expression_text.trim().to_string();
            app.s.checkpoint("expression");
            if let Some(n) = app.s.nodes.get_mut(&lead) {
                if src.is_empty() {
                    n.expressions.shift_remove(&attr);
                } else {
                    n.expressions.insert(attr.clone(), src);
                }
                app.s.touch();
            }
            app.s.apply_anim();
            app.help = format!("Applied expression to {name}.{attr}");
        }
        if ui.button("Remove").clicked() {
            let attr = app.panels.expression_attr.clone();
            app.s.checkpoint("remove expression");
            if let Some(n) = app.s.nodes.get_mut(&lead) {
                n.expressions.shift_remove(&attr);
                app.s.touch();
            }
            app.panels.expression_text.clear();
            app.help = format!("Removed expression from {name}.{attr}");
        }
        if ui.button("Close").clicked() {
            app.win.expression_editor = false;
        }
    });
}

/// Delete Attributes window: remove dynamic attributes from the lead node.
pub(crate) fn delete_attributes_window(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(lead) = app.s.lead().cloned() else {
        ui.label(RichText::new("Select an object with custom attributes.").color(theme::TEXT_DIM));
        return;
    };
    let Some(node) = app.s.node(&lead) else { return };
    let name = node.name.clone();
    let attrs: Vec<String> = node.extra.as_object().map(|e| e.keys().cloned().collect()).unwrap_or_default();
    if attrs.is_empty() {
        ui.label(RichText::new(format!("{name} has no custom attributes.")).color(theme::TEXT_DIM));
        return;
    }
    app.panels.delete_attr_checked.retain(|a| attrs.contains(a));
    ui.label(RichText::new(format!("Delete attributes from {name}:")).color(theme::TEXT));
    ui.add_space(2.0);
    for attr in &attrs {
        let mut checked = app.panels.delete_attr_checked.contains(attr);
        if ui.checkbox(&mut checked, attr).changed() {
            if checked {
                app.panels.delete_attr_checked.insert(attr.clone());
            } else {
                app.panels.delete_attr_checked.remove(attr);
            }
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        let count = app.panels.delete_attr_checked.len();
        if ui.add_enabled(count > 0, egui::Button::new(format!("Delete {count}"))).clicked() {
            let mut script = String::new();
            for attr in &app.panels.delete_attr_checked {
                script.push_str(&format!("deleteAttr -at {name}.{attr};\n"));
            }
            let _ = app.run_mel("delete attributes", &script);
            if let Some(lead) = app.s.lead().cloned() {
                if let Some(n) = app.s.nodes.get_mut(&lead) {
                    for attr in &app.panels.delete_attr_checked {
                        n.expressions.shift_remove(attr);
                        n.nonkeyable.remove(attr);
                        n.hidden_channels.remove(attr);
                        n.locks.remove(attr);
                    }
                }
            }
            app.panels.delete_attr_checked.clear();
            app.win.delete_attributes = false;
        }
        if ui.button("Close").clicked() {
            app.win.delete_attributes = false;
        }
    });
}

/// Edit Attribute window: rename a dynamic attribute and change its keyable flag.
pub(crate) fn edit_attributes_window(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(lead) = app.s.lead().cloned() else {
        ui.label(RichText::new("Select an object with custom attributes.").color(theme::TEXT_DIM));
        return;
    };
    let Some(name) = app.s.node(&lead).map(|n| n.name.clone()) else { return };
    let attrs: Vec<String> = app.s.node(&lead).and_then(|n| n.extra.as_object()).map(|e| e.keys().cloned().collect()).unwrap_or_default();
    if attrs.is_empty() {
        ui.label(RichText::new(format!("{name} has no custom attributes.")).color(theme::TEXT_DIM));
        return;
    }
    if app.panels.edit_attr_name.is_empty() || !attrs.contains(&app.panels.edit_attr_name) {
        app.panels.edit_attr_name = attrs[0].clone();
        app.panels.edit_attr_new = attrs[0].clone();
    }
    ui.label(RichText::new(format!("Edit attribute on {name}:")).color(theme::TEXT));
    ui.add_space(2.0);
    egui::ComboBox::from_id_salt("edit_attr_pick").selected_text(&app.panels.edit_attr_name).show_ui(ui, |ui| {
        for attr in &attrs {
            if ui.selectable_label(*attr == app.panels.edit_attr_name, attr).clicked() {
                app.panels.edit_attr_name = attr.clone();
                app.panels.edit_attr_new = attr.clone();
            }
        }
    });
    egui::Grid::new("edit_attr_fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Long Name");
        ui.text_edit_singleline(&mut app.panels.edit_attr_new);
        ui.end_row();
        ui.label("Keyable");
        let mut keyable = app.s.node(&lead).is_none_or(|n| n.is_keyable(&app.panels.edit_attr_name));
        if ui.checkbox(&mut keyable, "").changed() {
            if let Some(n) = app.s.nodes.get_mut(&lead) {
                if keyable {
                    n.nonkeyable.remove(&app.panels.edit_attr_name);
                } else {
                    n.nonkeyable.insert(app.panels.edit_attr_name.clone());
                }
                app.s.touch();
            }
        }
        ui.end_row();
    });
    ui.separator();
    ui.horizontal(|ui| {
        let new = app.panels.edit_attr_new.trim().to_string();
        let valid = !new.is_empty() && !new.contains('.') && !CHANNELS.contains(&new.as_str()) && (new == app.panels.edit_attr_name || !attrs.contains(&new));
        if ui.add_enabled(valid, egui::Button::new("Apply")).clicked() {
            let old = app.panels.edit_attr_name.clone();
            rename_custom_attr(app, &lead, &old, &new);
            app.panels.edit_attr_name = new;
            app.panels.edit_attr_new = app.panels.edit_attr_name.clone();
            app.win.edit_attributes = false;
        }
        if ui.button("Close").clicked() {
            app.win.edit_attributes = false;
        }
    });
}

fn rename_custom_attr(app: &mut IncaApp, id: &str, old: &str, new: &str) {
    if old == new {
        return;
    }
    app.s.checkpoint("edit attribute");
    {
        let Some(n) = app.s.nodes.get_mut(id) else { return };
        let Some(extra) = n.extra.as_object_mut() else { return };
        let Some(v) = extra.remove(old) else { return };
        extra.insert(new.to_string(), v);
        if let Some(v) = n.locks.remove(old) {
            n.locks.insert(new.to_string(), v);
        }
        if n.hidden_channels.remove(old) {
            n.hidden_channels.insert(new.to_string());
        }
        if n.nonkeyable.remove(old) {
            n.nonkeyable.insert(new.to_string());
        }
        if let Some(src) = n.expressions.shift_remove(old) {
            n.expressions.insert(new.to_string(), src);
        }
    }
    for node in app.s.nodes.values_mut() {
        for p in node.published.values_mut() {
            if p.node == id && p.attr == old {
                p.attr = new.to_string();
            }
        }
    }
    let old_key = inca_core::anim::Anim::key(id, old);
    if let Some(mut c) = app.s.anim_curves_mut().shift_remove(&old_key) {
        c.attr = new.to_string();
        app.s.anim_curves_mut().insert(inca_core::anim::Anim::key(id, new), c);
    }
    for c in &mut app.s.conns {
        if c.src == id && c.src_attr == old {
            c.src_attr = new.to_string();
        }
        if c.dst == id && c.dst_attr == old {
            c.dst_attr = new.to_string();
        }
    }
    app.s.touch();
}

/// Key the highlighted Channel Box channels at the current time. Returns false when none are picked.
pub(crate) fn key_selected_channels(app: &mut IncaApp) -> bool {
    let picks = app.panels.cb_pick.clone();
    if picks.is_empty() {
        return false;
    }
    app.s.checkpoint("key selected channels");
    let mut n = 0;
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                if app.s.node(id).is_some_and(|n| !n.is_keyable(&p.attr)) {
                    continue;
                }
                app.s.set_key(id, &p.attr, None, None);
                n += 1;
            }
        }
    }
    app.s.apply_anim();
    app.s.touch();
    app.help = format!("Keyed {n} selected channel(s)");
    n > 0
}

fn copy_channel_keys(app: &mut IncaApp) -> bool {
    let picks = app.panels.cb_pick.clone();
    if picks.is_empty() {
        app.help = "Select channel names to copy their animation".into();
        return false;
    }
    let mut clip: Vec<(String, String, Vec<inca_core::anim::Key>)> = Vec::new();
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                let key = inca_core::anim::Anim::key(id, &p.attr);
                if let Some(c) = app.s.anim_curves().get(&key) {
                    if let Some(first) = c.keys.first() {
                        let t0 = first.t;
                        let keys = c.keys.iter().map(|k| inca_core::anim::Key { t: k.t - t0, ..k.clone() }).collect();
                        clip.push((id.clone(), p.attr.clone(), keys));
                    }
                }
            }
        }
    }
    if clip.is_empty() {
        app.help = "No animation on the selected channels".into();
        return false;
    }
    app.key_clip = clip;
    app.windows_state.key_clip_kind = crate::windows::KeyClipKind::Timeline;
    app.windows_state.key_clipboard_marker = true;
    app.help = format!("Copied {} channel curve(s)", app.key_clip.len());
    true
}

fn delete_channel_keys(app: &mut IncaApp) -> usize {
    let picks = app.panels.cb_pick.clone();
    if picks.is_empty() {
        app.help = "Select channel names to delete their animation".into();
        return 0;
    }
    app.s.checkpoint("delete channel animation");
    let mut n = 0;
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                let key = inca_core::anim::Anim::key(id, &p.attr);
                if app.s.anim_curves_mut().shift_remove(&key).is_some() {
                    n += 1;
                }
            }
        }
    }
    if n > 0 {
        app.s.apply_anim();
        app.s.touch();
    }
    app.help = format!("Deleted {n} channel curve(s)");
    n
}

fn duplicate_channel_values(app: &mut IncaApp) {
    let picks = app.panels.cb_pick.clone();
    if picks.is_empty() {
        app.help = "Select channel names to duplicate their values".into();
        return;
    }
    app.s.checkpoint("duplicate channel values");
    let mut n = 0;
    for p in picks {
        let Some(first) = p.targets.first() else { continue };
        let Some(value) = app.s.get_attr(first, &p.attr) else { continue };
        for t in &p.targets[1..] {
            if app.s.set_attr(t, &p.attr, value.clone()) {
                n += 1;
            }
        }
    }
    app.s.touch();
    app.help = format!("Copied {n} channel value(s)");
}

fn break_channel_connections(app: &mut IncaApp) {
    let picks = app.panels.cb_pick.clone();
    app.s.checkpoint("break channel connections");
    let mut keys = 0;
    for p in picks {
        for t in &p.targets {
            let id = app.s.ref_id(t);
            app.s.conns.retain(|c| !(c.dst == id.as_str() && c.dst_attr == p.attr));
            let key = inca_core::anim::Anim::key(&id, &p.attr);
            if app.s.anim_curves_mut().shift_remove(&key).is_some_and(|c| !c.keys.is_empty()) {
                keys += 1;
            }
        }
    }
    app.s.touch();
    app.help = if keys > 0 {
        format!("Broke connections and removed animation from {keys} channel(s)")
    } else {
        "Broke connections on the selected channels".into()
    };
}

fn select_channel_connection(app: &mut IncaApp) {
    let mut ids = Vec::new();
    for p in &app.panels.cb_pick {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                for c in &app.s.conns {
                    if c.dst == id.as_str() && c.dst_attr == p.attr {
                        ids.push(c.src.clone());
                    }
                }
            }
        }
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        app.help = "No incoming connection on the selected channel(s)".into();
    } else {
        app.s.select(&ids, SelMode::Replace);
        app.help = format!("Selected {} connection node(s)", ids.len());
    }
}

fn lock_channels(app: &mut IncaApp, lock: bool) {
    let picks = app.panels.cb_pick.clone();
    app.s.checkpoint(if lock { "lock channels" } else { "unlock channels" });
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                if let Some(n) = app.s.nodes.get_mut(id) {
                    n.locks.insert(p.attr.clone(), json!(lock));
                }
            }
        }
    }
    app.s.touch();
    app.help = if lock { "Locked selected channels".into() } else { "Unlocked selected channels".into() };
}

fn layer_members(app: &mut IncaApp, add: bool) {
    let lids = layer_targets(app);
    if lids.is_empty() {
        app.help = "Select a display layer in the Layer Editor first".into();
        return;
    }
    layer_members_to(app, &lids, add);
}

/// Selected layers, or the current layer when nothing is highlighted.
fn layer_targets(app: &IncaApp) -> Vec<String> {
    let mut lids: Vec<String> = app.panels.layer_sel.iter().filter(|id| app.s.layers.iter().any(|l| &l.id == *id)).cloned().collect();
    if lids.is_empty() {
        if let Some(c) = &app.s.current_layer {
            if app.s.layers.iter().any(|l| &l.id == c) {
                lids.push(c.clone());
            }
        }
    }
    lids
}

fn layer_members_to(app: &mut IncaApp, lids: &[String], add: bool) {
    let Some(lid) = lids.first() else { return };
    let label = app.s.layers.iter().find(|l| &l.id == lid).map(|l| l.name.clone()).unwrap_or_default();
    app.s.checkpoint(if add { "add to display layer" } else { "remove from display layer" });
    let ids = app.s.sel.clone();
    for id in ids {
        if let Some(n) = app.s.nodes.get_mut(&id) {
            if add {
                n.layer = Some(lid.clone());
            } else if n.layer.as_ref().is_some_and(|x| lids.contains(x)) {
                n.layer = None;
            }
        }
    }
    app.s.touch();
    app.help = if add {
        format!("Added selected objects to layer {label}")
    } else {
        format!("Removed selected objects from layer {label}")
    };
}

/// Picked channels, or every keyable channel of the selected objects when `all` is set.
fn cb_action_picks(app: &IncaApp, all: bool, keyable_only: bool) -> Vec<CbPick> {
    if !all {
        return app
            .panels
            .cb_pick
            .iter()
            .filter(|p| !keyable_only || p.targets.iter().all(|t| match t {
                NodeRef::Dag(id) => app.s.node(id).is_none_or(|n| n.is_keyable(&p.attr)),
                _ => true,
            }))
            .cloned()
            .collect();
    }
    let mut out = vec![];
    for id in &app.s.sel {
        let Some(n) = app.s.node(id) else { continue };
        for attr in cb_order(&app.s, &NodeRef::Dag(id.clone())) {
            if keyable_only && !n.is_keyable(&attr) {
                continue;
            }
            out.push(CbPick { targets: vec![NodeRef::Dag(id.clone())], attr: attr.clone(), int: attr == "visibility", min: None, max: None });
        }
    }
    out
}

fn breakdown_channels(app: &mut IncaApp, all: bool) {
    let picks = cb_action_picks(app, all, true);
    if picks.is_empty() {
        app.help = "Select channel names to create breakdowns".into();
        return;
    }
    app.s.checkpoint("breakdown keys");
    let mut n = 0;
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                if app.s.node(id).is_none_or(|n| n.is_keyable(&p.attr)) {
                    app.s.set_key_ex(id, &p.attr, None, None, true);
                    n += 1;
                }
            }
        }
    }
    app.s.touch();
    app.help = format!("Set {n} breakdown key(s) at the current frame");
}

fn mute_channels(app: &mut IncaApp, mute: bool, all: bool) {
    app.s.checkpoint(if mute { "mute channels" } else { "unmute channels" });
    let mut n = 0;
    if all {
        let sel = app.s.sel.clone();
        for c in app.s.anim_curves_mut().values_mut() {
            if sel.contains(&c.node) && !c.keys.is_empty() && c.mute != mute {
                c.mute = mute;
                n += 1;
            }
        }
    } else {
        for p in app.panels.cb_pick.clone() {
            for t in &p.targets {
                if let NodeRef::Dag(id) = t {
                    let key = inca_core::anim::Anim::key(id, &p.attr);
                    if let Some(c) = app.s.anim_curves_mut().get_mut(&key) {
                        if !c.keys.is_empty() && c.mute != mute {
                            c.mute = mute;
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    app.s.apply_anim();
    app.s.touch();
    app.help = match (mute, n) {
        (true, 0) => "No selected animated channels to mute".into(),
        (false, 0) => "No selected animated channels to unmute".into(),
        (true, _) => format!("Muted {n} channel(s)"),
        (false, _) => format!("Unmuted {n} channel(s)"),
    };
}

fn hide_channels(app: &mut IncaApp, lock: bool) {
    let picks = app.panels.cb_pick.clone();
    commit_cb_edit(app);
    app.s.checkpoint(if lock { "lock and hide channels" } else { "hide channels" });
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                if let Some(n) = app.s.nodes.get_mut(id) {
                    n.hidden_channels.insert(p.attr.clone());
                    if lock {
                        n.locks.insert(p.attr.clone(), json!(true));
                    }
                }
            }
        }
    }
    cb_clear_names(app);
    app.panels.cb_fields.clear();
    app.s.touch();
    app.help = if lock { "Locked and hid the selected channel(s)".into() } else { "Hid the selected channel(s)".into() };
}

fn set_channel_keyable(app: &mut IncaApp, keyable: bool) {
    let picks = app.panels.cb_pick.clone();
    app.s.checkpoint(if keyable { "make channels keyable" } else { "make channels nonkeyable" });
    for p in picks {
        for t in &p.targets {
            if let NodeRef::Dag(id) = t {
                if let Some(n) = app.s.nodes.get_mut(id) {
                    if keyable {
                        n.nonkeyable.remove(&p.attr);
                        n.hidden_channels.remove(&p.attr);
                    } else {
                        n.nonkeyable.insert(p.attr.clone());
                    }
                }
            }
        }
    }
    app.s.touch();
    app.help = if keyable { "Made the selected channel(s) keyable".into() } else { "Made the selected channel(s) nonkeyable".into() };
}

fn unpublish_channels(app: &mut IncaApp) {
    let picks = app.panels.cb_pick.clone();
    let mut n = 0;
    for p in picks {
        for t in &p.targets {
            let NodeRef::Dag(id) = t else { continue };
            // A published row belongs directly to the asset; other rows resolve through the
            // containing asset.
            if app.s.nodes.get(id).is_some_and(|n| n.kind == "container" && n.published.contains_key(&p.attr)) {
                if let Some(c) = app.s.nodes.get_mut(id) {
                    c.published.shift_remove(&p.attr);
                    n += 1;
                }
                continue;
            }
            let Some(cid) = app.s.container_of(id) else { continue };
            let names: Vec<String> = app.s.nodes.get(&cid).map(|c| {
                c.published.iter().filter_map(|(name, pa)| {
                    (pa.node == *id && pa.attr == p.attr && app.s.published_ref(pa).as_ref() == Some(t)).then(|| name.clone())
                }).collect()
            }).unwrap_or_default();
            if let Some(c) = app.s.nodes.get_mut(&cid) {
                for name in names {
                    c.published.shift_remove(&name);
                    n += 1;
                }
            }
        }
    }
    app.s.touch();
    app.help = if n > 0 { format!("Unpublished {n} attribute(s) from the asset") } else { "The selected channel(s) are not published".into() };
}

fn lock_unpublished_attributes(app: &mut IncaApp) {
    let ids = app.s.sel.clone();
    let mut n = 0;
    for id in ids {
        let container = app.s.container_of(&id);
        let attrs = cb_order(&app.s, &NodeRef::Dag(id.clone()));
        let unpublished: Vec<String> = attrs
            .into_iter()
            .filter(|attr| container.as_ref().is_none_or(|c| app.s.published_name_for(c, &NodeRef::Dag(id.clone()), attr).is_none()))
            .collect();
        let Some(n_node) = app.s.nodes.get_mut(&id) else { continue };
        for attr in unpublished {
            n_node.locks.insert(attr, json!(true));
            n += 1;
        }
    }
    app.s.touch();
    app.help = if n > 0 { format!("Locked {n} unpublished attribute(s)") } else { "No unpublished attributes to lock".into() };
}

fn show_selected_channel_types(app: &mut IncaApp) {
    let mut f = [false; 4];
    let mut any = false;
    for p in &app.panels.cb_pick {
        if p.attr.starts_with("translate") {
            f[0] = true;
            any = true;
        } else if p.attr.starts_with("rotate") {
            f[1] = true;
            any = true;
        } else if p.attr.starts_with("scale") {
            f[2] = true;
            any = true;
        } else if p.attr == "visibility" {
            f[3] = true;
            any = true;
        }
    }
    app.panels.cb_show = if any { Some(f) } else { None };
    app.panels.cb_custom_active = None;
}

fn layer_editor(app: &mut IncaApp, ui: &mut egui::Ui) {
    use crate::icons;
    use crate::menus::{menu_check, menu_item, panel_bar, MenuSource};
    app.panels.layer_sel.retain(|id| app.s.layers.iter().any(|l| &l.id == id));
    if app.panels.layer_rename.as_ref().is_some_and(|(id, _)| !app.s.layers.iter().any(|l| &l.id == id)) {
        app.panels.layer_rename = None;
    }
    if app.panels.layer_color_popup.as_ref().is_some_and(|id| !app.s.layers.iter().any(|l| &l.id == id)) {
        app.panels.layer_color_popup = None;
    }
    if app.s.current_layer.as_ref().is_some_and(|id| !app.s.layers.iter().any(|l| &l.id == id)) {
        app.s.current_layer = None;
    }
    layer_edit_dialog(app, ui.ctx());
    layer_membership_dialog(app, ui.ctx());
    egui::Frame::NONE.fill(theme::BG2).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add_space(3.0);
            // Match the shelf tabs: zero-width strokes in every state so hover never changes
            // the tab width, transparent inactive fill, active tab filled with the panel BG.
            let w = &mut ui.visuals_mut().widgets;
            w.inactive.bg_stroke = egui::Stroke::NONE;
            w.hovered.bg_stroke = egui::Stroke::NONE;
            w.active.bg_stroke = egui::Stroke::NONE;
            w.open.bg_stroke = egui::Stroke::NONE;
            ui.spacing_mut().item_spacing.x = 0.0;
            for (i, name) in ["Display", "Anim"].iter().enumerate() {
                let on = app.panels.layer_tab as usize == i;
                let id = ui.next_auto_id();
                let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
                let color = if on || hovered { Color32::WHITE } else { Color32::from_gray(0xbd) };
                let t = RichText::new(*name).size(11.0).color(color);
                let b = egui::Button::new(t)
                    .fill(if on { theme::BG } else { Color32::TRANSPARENT })
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 });
                if ui.add(b).clicked() {
                    app.panels.layer_tab = i as u8;
                }
            }
        });
    });
    if app.panels.layer_tab == 1 {
        app.panels.layer_color_popup = None;
        anim_layer_editor(app, ui);
        return;
    }
    let has_layers = !app.panels.layer_sel.is_empty();
    // Maya: menus on their own row, the four layer buttons on the row below.
    panel_bar(app, ui, MenuSource::LayerEditor, &["Layers", "Options", "Help"], true, |app, ui, title| match title {
            "Layers" => {
                if menu_item(app, ui, "Create Empty Layer") {
                    layer_create(app, false);
                }
                if menu_item(app, ui, "Create Layer from Selected") {
                    layer_create(app, true);
                }
                ui.separator();
                if menu_item(app, ui, "Select Objects in Selected Layers") {
                    layer_select_objects(app);
                }
                if menu_item(app, ui, "Add Selected Objects to Selected Layers") {
                    layer_members(app, true);
                }
                if menu_item(app, ui, "Remove Selected Objects from Selected Layers") {
                    layer_members(app, false);
                }
                ui.separator();
                if menu_item(app, ui, "Membership...") {
                    let id = app.panels.layer_sel.first().cloned().or_else(|| app.s.current_layer.clone());
                    if let Some(id) = id {
                        layer_open_membership(app, &id);
                    }
                }
                if menu_item(app, ui, "Attributes...") {
                    let id = app.panels.layer_sel.first().cloned().or_else(|| app.s.current_layer.clone());
                    if let Some(id) = id {
                        layer_open_attributes(app, &id);
                    }
                }
                ui.separator();
                if menu_item(app, ui, "Delete Selected Layers") {
                    let lids = app.panels.layer_sel.clone();
                    layer_delete(app, &lids);
                }
                if menu_item(app, ui, "Delete Unused Layers") {
                    layer_delete_unused(app);
                }
                ui.separator();
                ui.menu_button("Set All Layers", |ui| layer_set_entries(app, ui, LayerScope::All));
                ui.menu_button("Set Selected Layers", |ui| layer_set_entries(app, ui, LayerScope::Selected));
                ui.menu_button("Set Only Selected Layers", |ui| layer_set_entries(app, ui, LayerScope::OnlySelected));
                ui.separator();
                if menu_item(app, ui, "Sort Layers Alphabetically") {
                    layer_sort(app, true);
                }
                if menu_item(app, ui, "Sort Layers Chronologically") {
                    layer_sort(app, false);
                }
            }
            "Options" => {
                if menu_check(app, ui, app.panels.layer_make_current, "Make New Layers Current") {
                    app.panels.layer_make_current = !app.panels.layer_make_current;
                }
                if menu_check(app, ui, app.panels.layer_add_new, "Add New Objects to Current Layer") {
                    app.panels.layer_add_new = !app.panels.layer_add_new;
                    app.s.add_new_to_current = app.panels.layer_add_new;
                }
                if menu_check(app, ui, !app.panels.cb_hide_namespace, "Show Namespace") {
                    app.panels.cb_hide_namespace = !app.panels.cb_hide_namespace;
                }
            }
            "Help" => {
                if menu_item(app, ui, "Layer Editor Help") {
                    app.help = "V hides the layer, P hides it during playback, and the T column cycles normal, template and reference. Double-click a layer to edit its name, type, visibility and colour. Use the toolbar to create, reorder, populate, select from and delete layers.".into();
                }
            }
            _ => {}
        });
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icons::button_ex(ui, "layerFromSel", "Create a layer from selected objects", false, 24.0, 20.0, true).clicked() {
                layer_create(app, true);
            }
            if icons::button_ex(ui, "layerNew", "Create an empty layer", false, 24.0, 20.0, true).clicked() {
                layer_create(app, false);
            }
            if icons::button_ex(ui, "layerDown", "Move selected layers down", false, 24.0, 20.0, has_layers).clicked() {
                layer_move(app, false);
            }
            if icons::button_ex(ui, "layerUp", "Move selected layers up", false, 24.0, 20.0, has_layers).clicked() {
                layer_move(app, true);
            }
        });
    });
    let layers = app.s.layers.clone();
    let well_h = (ui.available_height() - 4.0).max(ROW_H);
    egui::Frame::NONE.fill(theme::BG_DARK).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
        ui.set_min_height(well_h);
        egui::ScrollArea::vertical().id_salt("layers").max_height(well_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for l in &layers {
                layer_row(app, ui, l);
            }
            if layers.is_empty() {
                ui.label(RichText::new("No layers").color(theme::TEXT_DIM));
            }
        });
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayerScope {
    All,
    Selected,
    OnlySelected,
}

/// Maya-style display layer colour presets (the `Color` column palette).
const LAYER_PALETTE: [[u8; 3]; 24] = [
    [0, 0, 0],
    [64, 64, 64],
    [128, 128, 128],
    [192, 192, 192],
    [255, 255, 255],
    [128, 0, 0],
    [255, 0, 0],
    [255, 128, 128],
    [255, 128, 0],
    [255, 255, 0],
    [128, 128, 0],
    [0, 128, 0],
    [0, 255, 0],
    [128, 255, 128],
    [0, 128, 128],
    [0, 255, 255],
    [0, 0, 128],
    [0, 0, 255],
    [128, 128, 255],
    [64, 0, 128],
    [128, 0, 255],
    [255, 0, 255],
    [128, 64, 0],
    [160, 120, 80],
];

/// True when the layer has an explicit colour (otherwise the swatch shows the empty slash).
fn layer_has_color(color: &Option<Value>) -> bool {
    matches!(color, Some(Value::Array(a)) if a.len() >= 3 && a.iter().take(3).all(|v| v.is_number()))
}

/// Maya's empty layer colour is drawn as an empty box with a diagonal slash, not a grey fill.
fn layer_swatch_color(color: &Option<Value>) -> Color32 {
    if !layer_has_color(color) {
        return Color32::TRANSPARENT;
    }
    let Some(Value::Array(a)) = color else { return Color32::TRANSPARENT };
    let mut c = [0.0f32; 3];
    for (i, v) in a.iter().take(3).enumerate() {
        c[i] = v.as_f64().unwrap_or(0.0) as f32;
    }
    if c.iter().any(|x| *x > 1.0) {
        for x in &mut c {
            *x /= 255.0;
        }
    }
    Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8)
}

fn layer_set_color(app: &mut IncaApp, id: &str, rgb: [f32; 3]) {
    app.s.checkpoint("layer color");
    if let Some(l) = app.s.layers.iter_mut().find(|l| l.id == id) {
        l.color = Some(json!([rgb[0] as f64, rgb[1] as f64, rgb[2] as f64]));
    }
    app.s.touch();
}

fn layer_clear_color(app: &mut IncaApp, id: &str) {
    app.s.checkpoint("layer color");
    if let Some(l) = app.s.layers.iter_mut().find(|l| l.id == id) {
        l.color = None;
    }
    app.s.touch();
}

fn color32_of(color: &[f32; 3]) -> Color32 {
    Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

/// Full colour-picker body: wheel/sliders plus a pasteable HEX field. Mutates `color` live and
/// returns whether it changed this frame.
pub(crate) fn color_picker_body(ui: &mut egui::Ui, id: egui::Id, color: &mut [f32; 3]) -> bool {
    ui.spacing_mut().slider_width = 320.0;
    let row_top = ui.cursor().min;
    let mut c = color32_of(color);
    let mut changed = egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque);
    if changed {
        *color = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
    }

    // HEX goes on the picker's own numeric row, to the right of the R/G/B fields.
    let hex_id = id.with("hex");
    let mut buf = ui.data(|d| d.get_temp::<String>(hex_id)).unwrap_or_default();
    let row_h = ui.spacing().interact_size.y;
    let hex_rect = egui::Rect::from_min_size(
        egui::pos2(row_top.x + 250.0, row_top.y),
        egui::vec2(68.0, row_h),
    );
    let resp = ui.put(hex_rect, egui::TextEdit::singleline(&mut buf).hint_text("#RRGGBB"));
    if resp.changed() {
        if let Some(rgb) = parse_hex(&buf) {
            *color = rgb;
            changed = true;
        }
    }
    if !resp.has_focus() {
        buf = format!(
            "#{:02X}{:02X}{:02X}",
            (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[2].clamp(0.0, 1.0) * 255.0).round() as u8
        );
    }
    ui.data_mut(|d| d.insert_temp(hex_id, buf));
    changed
}

/// Colour swatch that opens the shared picker window (wheel + HEX field). One picker at a time.
pub(crate) fn color_edit(ui: &mut egui::Ui, id: egui::Id, color: &mut [f32; 3]) -> bool {
    let open_id = egui::Id::new("inca_color_picker_open");
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::click());
    ui.painter().rect_filled(rect, 2.0, color32_of(color));
    ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, Color32::from_gray(0x22)), egui::StrokeKind::Inside);
    let resp = resp.on_hover_text("Color picker");
    let mut open = ui.data(|d| d.get_temp::<Option<egui::Id>>(open_id).flatten()) == Some(id);
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| {
            if open {
                d.insert_temp(open_id, Some(id));
            } else {
                d.remove_temp::<Option<egui::Id>>(open_id);
            }
        });
    }
    let mut changed = false;
    if open {
        let mut keep = true;
        let frame = egui::Frame::window(ui.style()).shadow(egui::epaint::Shadow::NONE);
        let screen = ui.ctx().content_rect();
        let x = (rect.left() - 360.0).clamp(screen.left() + 20.0, (screen.right() - 360.0).max(screen.left() + 20.0));
        egui::Window::new("Color")
            .frame(frame)
            .id(id.with("color_picker"))
            .collapsible(false)
            .resizable(false)
            .min_width(320.0)
            .default_pos(egui::pos2(x, rect.top()))
            .open(&mut keep)
            .show(ui.ctx(), |ui| {
                changed = color_picker_body(ui, id, color);
            });
        if !keep {
            ui.data_mut(|d| d.remove_temp::<Option<egui::Id>>(open_id));
        }
    }
    changed
}

fn layer_create(app: &mut IncaApp, from_selection: bool) {
    app.s.checkpoint("create display layer");
    let id = app.s.new_id("l");
    let name = app.s.unique_name("layer1");
    let created = app.s.id_counter;
    app.s.layers.push(inca_core::scene::Layer {
        id: id.clone(),
        name,
        visible: true,
        mode: "normal".into(),
        color: None,
        playback: true,
        enabled: true,
        shading: true,
        texturing: true,
        lod: "full".into(),
        anim_playback: true,
        created,
        extra: Default::default(),
    });
    if from_selection {
        for oid in app.s.sel.clone() {
            if let Some(n) = app.s.nodes.get_mut(&oid) {
                n.layer = Some(id.clone());
            }
        }
    }
    if app.panels.layer_make_current || !app.s.current_layer.as_ref().is_some_and(|id| app.s.layers.iter().any(|l| &l.id == id)) {
        app.s.current_layer = Some(id.clone());
    }
    app.panels.layer_sel = vec![id.clone()];
    app.panels.layer_anchor = Some(id.clone());
    app.panels.layer_rename = None;
    app.s.touch();
    let lname = app.s.layers.iter().find(|l| l.id == id).map(|l| l.name.clone()).unwrap_or_default();
    app.help = if from_selection {
        format!("Created layer {lname} from the selected objects")
    } else {
        format!("Created layer {lname}")
    };
}

fn layer_select_objects(app: &mut IncaApp) {
    let lids = layer_targets(app);
    if lids.is_empty() {
        app.help = "Select a display layer in the Layer Editor first".into();
        return;
    }
    let ids: Vec<String> = app.s.nodes.values().filter(|n| n.layer.as_ref().is_some_and(|l| lids.contains(l))).map(|n| n.id.clone()).collect();
    app.select_names(&ids, SelMode::Replace);
    app.help = format!("Selected {} object(s) in the selected layer(s)", ids.len());
}

fn layer_empty(app: &mut IncaApp, lid: &str) {
    app.s.checkpoint("empty display layer");
    for n in app.s.nodes.values_mut() {
        if n.layer.as_deref() == Some(lid) {
            n.layer = None;
        }
    }
    app.s.touch();
    app.help = "Emptied the display layer".into();
}

fn layer_delete(app: &mut IncaApp, lids: &[String]) {
    if lids.is_empty() {
        app.help = "Select a display layer in the Layer Editor first".into();
        return;
    }
    app.s.checkpoint("delete display layers");
    let removed: Vec<String> = app.s.layers.iter().filter(|l| lids.contains(&l.id)).map(|l| l.name.clone()).collect();
    app.s.layers.retain(|l| !lids.contains(&l.id));
    for name in &removed {
        if !app.s.nodes.values().any(|n| &n.name == name) {
            app.s.names.remove(name);
        }
    }
    for n in app.s.nodes.values_mut() {
        if n.layer.as_ref().is_some_and(|l| lids.contains(l)) {
            n.layer = None;
        }
    }
    if app.s.current_layer.as_ref().is_some_and(|l| lids.contains(l)) {
        app.s.current_layer = None;
    }
    app.panels.layer_sel.retain(|l| !lids.contains(l));
    app.panels.layer_anchor = None;
    app.s.touch();
    app.help = format!("Deleted {} display layer(s); their objects were kept", removed.len());
}

fn layer_delete_unused(app: &mut IncaApp) {
    let used: std::collections::HashSet<String> = app.s.nodes.values().filter_map(|n| n.layer.clone()).collect();
    let lids: Vec<String> = app.s.layers.iter().filter(|l| !used.contains(&l.id)).map(|l| l.id.clone()).collect();
    if lids.is_empty() {
        app.help = "No unused display layers".into();
        return;
    }
    layer_delete(app, &lids);
    app.help = format!("Deleted {} unused display layer(s)", lids.len());
}

fn layer_sort(app: &mut IncaApp, alphabetical: bool) {
    app.s.checkpoint("sort display layers");
    if alphabetical {
        app.s.layers.sort_by_key(|l| l.name.to_lowercase());
    } else {
        app.s.layers.sort_by_key(|l| l.created);
    }
    app.s.touch();
    app.help = if alphabetical { "Sorted layers alphabetically".into() } else { "Sorted layers chronologically".into() };
}

/// Move the highlighted layers one step up or down in the displayed list.
fn layer_move(app: &mut IncaApp, up: bool) {
    let targets = layer_targets(app);
    if targets.is_empty() {
        app.help = "Select a display layer in the Layer Editor first".into();
        return;
    }
    app.s.checkpoint(if up { "move display layers up" } else { "move display layers down" });
    let n = app.s.layers.len();
    if up {
        for i in 1..n {
            if targets.contains(&app.s.layers[i].id) && !targets.contains(&app.s.layers[i - 1].id) {
                app.s.layers.swap(i - 1, i);
            }
        }
    } else {
        for i in (0..n.saturating_sub(1)).rev() {
            if targets.contains(&app.s.layers[i].id) && !targets.contains(&app.s.layers[i + 1].id) {
                app.s.layers.swap(i, i + 1);
            }
        }
    }
    app.s.touch();
    app.help = if up { "Moved the selected layer(s) up".into() } else { "Moved the selected layer(s) down".into() };
}

fn layer_update(app: &mut IncaApp, id: &str, label: &str, f: impl FnOnce(&mut inca_core::scene::Layer)) {
    app.s.checkpoint(label);
    if let Some(l) = app.s.layers.iter_mut().find(|l| l.id == id) {
        f(l);
    }
    app.s.touch();
}

fn layer_apply(app: &mut IncaApp, scope: LayerScope, label: &str, on: impl Fn(&mut inca_core::scene::Layer), off: impl Fn(&mut inca_core::scene::Layer)) {
    let targets = layer_targets(app);
    if scope != LayerScope::All && targets.is_empty() {
        app.help = "Select a display layer in the Layer Editor first".into();
        return;
    }
    app.s.checkpoint(label);
    for l in &mut app.s.layers {
        if scope == LayerScope::All || targets.contains(&l.id) {
            on(l);
        } else if scope == LayerScope::OnlySelected {
            off(l);
        }
    }
    app.s.touch();
    app.help = label.into();
}

fn layer_set_entries(app: &mut IncaApp, ui: &mut egui::Ui, scope: LayerScope) {
    use crate::menus::menu_item;
    let item = |app: &mut IncaApp, ui: &mut egui::Ui, label: &str, set: fn(&mut inca_core::scene::Layer), inverse: fn(&mut inca_core::scene::Layer)| {
        if menu_item(app, ui, label) {
            layer_apply(app, scope, label, set, inverse);
        }
    };
    item(app, ui, "Visible", |l| l.visible = true, |l| l.visible = false);
    item(app, ui, "Invisible", |l| l.visible = false, |l| l.visible = true);
    ui.separator();
    item(app, ui, "Normal", |l| l.mode = "normal".into(), |l| l.mode = "normal".into());
    item(app, ui, "Template", |l| l.mode = "template".into(), |l| l.mode = "normal".into());
    item(app, ui, "Reference", |l| l.mode = "reference".into(), |l| l.mode = "normal".into());
    ui.separator();
    item(app, ui, "Visible Playback", |l| l.playback = true, |l| l.playback = false);
    item(app, ui, "Invisible Playback", |l| l.playback = false, |l| l.playback = true);
    ui.separator();
    item(app, ui, "Full Detail", |l| l.lod = "full".into(), |l| l.lod = "full".into());
    item(app, ui, "Bounding Box", |l| l.lod = "bbox".into(), |l| l.lod = "full".into());
    ui.separator();
    item(app, ui, "Shaded", |l| l.shading = true, |l| l.shading = true);
    item(app, ui, "Unshaded", |l| l.shading = false, |l| l.shading = true);
    ui.separator();
    item(app, ui, "Textured", |l| l.texturing = true, |l| l.texturing = true);
    item(app, ui, "Untextured", |l| l.texturing = false, |l| l.texturing = true);
    ui.separator();
    item(app, ui, "Playback On", |l| l.anim_playback = true, |l| l.anim_playback = true);
    item(app, ui, "Playback Off", |l| l.anim_playback = false, |l| l.anim_playback = true);
}

fn layer_context_menu(app: &mut IncaApp, ui: &mut egui::Ui, lid: &str) {
    use crate::menus::menu_item;
    let id = lid.to_string();
    if menu_item(app, ui, "Add Selected Objects") {
        layer_members_to(app, std::slice::from_ref(&id), true);
    }
    if menu_item(app, ui, "Remove Selected Objects") {
        layer_members_to(app, std::slice::from_ref(&id), false);
    }
    if menu_item(app, ui, "Select Objects") {
        let ids: Vec<String> = app.s.nodes.values().filter(|n| n.layer.as_deref() == Some(lid)).map(|n| n.id.clone()).collect();
        app.select_names(&ids, SelMode::Replace);
        app.help = format!("Selected {} object(s) in the layer", ids.len());
    }
    if menu_item(app, ui, "Edit Layer...") {
        layer_open_edit(app, lid);
    }
    if menu_item(app, ui, "Rename...") {
        if let Some(l) = app.s.layers.iter().find(|l| l.id == lid) {
            app.panels.layer_rename = Some((lid.to_string(), l.name.clone()));
            app.panels.layer_rename_new = true;
        }
    }
    ui.separator();
    ui.menu_button("Set All Layers", |ui| layer_set_entries(app, ui, LayerScope::All));
    ui.menu_button("Set Selected Layers", |ui| layer_set_entries(app, ui, LayerScope::Selected));
    ui.menu_button("Set Only Selected Layers", |ui| layer_set_entries(app, ui, LayerScope::OnlySelected));
    ui.separator();
    if menu_item(app, ui, "Empty the Layer") {
        layer_empty(app, lid);
    }
    if menu_item(app, ui, "Delete Layer") {
        layer_delete(app, std::slice::from_ref(&id));
    }
    ui.separator();
    if menu_item(app, ui, "Membership...") {
        layer_open_membership(app, lid);
    }
    if menu_item(app, ui, "Attributes...") {
        layer_open_attributes(app, lid);
    }
}

fn layer_rename_commit(app: &mut IncaApp, id: &str, new: &str) {
    let Some(old) = app.s.layers.iter().find(|l| l.id == id).map(|l| l.name.clone()) else { return };
    let want = new.trim();
    if want.is_empty() || want == old {
        return;
    }
    app.s.checkpoint("rename display layer");
    layer_apply_rename(app, id, want);
    app.s.touch();
}

fn layer_apply_rename(app: &mut IncaApp, id: &str, want: &str) {
    let Some(old) = app.s.layers.iter().find(|l| l.id == id).map(|l| l.name.clone()) else { return };
    if !app.s.nodes.values().any(|n| n.name == old) {
        app.s.names.remove(&old);
    }
    let name = app.s.unique_name(want);
    if let Some(l) = app.s.layers.iter_mut().find(|l| l.id == id) {
        l.name = name;
    }
}

/// Open the Edit Layer dialog for a display layer.
fn layer_open_edit(app: &mut IncaApp, id: &str) {
    let Some(l) = app.s.layers.iter().find(|l| l.id == id) else { return };
    let color = if layer_has_color(&l.color) {
        let c = layer_swatch_color(&l.color);
        Some([c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0])
    } else {
        None
    };
    app.panels.layer_edit = Some(LayerEditDraft {
        id: l.id.clone(),
        name: l.name.clone(),
        mode: l.mode.clone(),
        visible: l.visible,
        playback: l.playback,
        color,
        rgb: false,
        focus_name: true,
    });
}

/// Apply the Edit Layer dialog as a single undo step.
fn layer_apply_edit(app: &mut IncaApp, d: &LayerEditDraft) {
    let Some(l) = app.s.layers.iter().find(|l| l.id == d.id) else { return };
    let old_name = l.name.clone();
    let new_name = d.name.trim();
    let name_changed = !new_name.is_empty() && new_name != old_name;
    let color_now = if layer_has_color(&l.color) {
        let c = layer_swatch_color(&l.color);
        Some([c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0])
    } else {
        None
    };
    let color_changed = match (color_now, d.color) {
        (None, None) => false,
        (Some(a), Some(b)) => (0..3).any(|i| (a[i] - b[i]).abs() > 1.0 / 512.0),
        _ => true,
    };
    if !(name_changed || color_changed || l.mode != d.mode || l.visible != d.visible || l.playback != d.playback) {
        return;
    }
    app.s.checkpoint("edit display layer");
    if name_changed {
        layer_apply_rename(app, &d.id, new_name);
    }
    if let Some(l) = app.s.layers.iter_mut().find(|l| l.id == d.id) {
        l.mode = d.mode.clone();
        l.visible = d.visible;
        l.playback = d.playback;
        l.color = d.color.map(|c| json!([c[0] as f64, c[1] as f64, c[2] as f64]));
    }
    app.s.touch();
}

/// Maya's Edit Layer window: name, display type, visible/playback and Index/RGB colour.
fn layer_edit_dialog(app: &mut IncaApp, ctx: &egui::Context) {
    let Some(mut draft) = app.panels.layer_edit.clone() else { return };
    if !app.s.layers.iter().any(|l| l.id == draft.id) {
        app.panels.layer_edit = None;
        return;
    }
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    let center = ctx.content_rect().center();
    let mut window = egui::Window::new("Edit Layer")
        .id(egui::Id::new("layer_edit_window"))
        .collapsible(false)
        .resizable(false);
    // Centre on every open; once open the user can drag it and that position holds for the session.
    if draft.focus_name {
        window = window.current_pos(egui::pos2(center.x - 180.0, center.y - 140.0));
    }
    window
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("layer_edit_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Name");
                let resp = ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(170.0));
                if draft.focus_name {
                    resp.request_focus();
                    draft.focus_name = false;
                }
                ui.end_row();
                ui.label("Display Type");
                egui::ComboBox::from_id_salt("layer_edit_type")
                    .selected_text(match draft.mode.as_str() {
                        "template" => "Template",
                        "reference" => "Reference",
                        _ => "Normal",
                    })
                    .show_ui(ui, |ui| {
                        for (k, label) in [("normal", "Normal"), ("template", "Template"), ("reference", "Reference")] {
                            ui.selectable_value(&mut draft.mode, k.into(), label);
                        }
                    });
                ui.end_row();
                ui.label("Visible");
                ui.checkbox(&mut draft.visible, "");
                ui.end_row();
                ui.label("Visible During Playback");
                ui.checkbox(&mut draft.playback, "");
                ui.end_row();
                ui.label("Color");
                egui::ComboBox::from_id_salt("layer_edit_color_mode")
                    .selected_text(if draft.rgb { "RGB" } else { "Index" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut draft.rgb, false, "Index");
                        ui.selectable_value(&mut draft.rgb, true, "RGB");
                    });
                ui.end_row();
            });
            if draft.rgb {
                ui.horizontal(|ui| {
                    let mut c = draft.color.unwrap_or([1.0; 3]);
                    if color_edit(ui, egui::Id::new("layer_edit_rgb"), &mut c) {
                        draft.color = Some(c);
                    }
                    if ui.button("Default").clicked() {
                        draft.color = None;
                    }
                });
            } else {
                ui.add_space(2.0);
                for row in LAYER_PALETTE.chunks(8) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        for c in row {
                            let (r, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                            let color = Color32::from_rgb(c[0], c[1], c[2]);
                            ui.painter().rect_filled(r, 2.0, color);
                            let picked = draft.color.is_some_and(|x| {
                                (x[0] - c[0] as f32 / 255.0).abs() < 0.002
                                    && (x[1] - c[1] as f32 / 255.0).abs() < 0.002
                                    && (x[2] - c[2] as f32 / 255.0).abs() < 0.002
                            });
                            ui.painter().rect_stroke(
                                r,
                                2.0,
                                egui::Stroke::new(if picked { 2.0 } else { 1.0 }, if picked { Color32::WHITE } else { Color32::from_gray(0x22) }),
                                egui::StrokeKind::Inside,
                            );
                            if resp.clicked() {
                                draft.color = Some([c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]);
                            }
                        }
                    });
                }
                if ui.button("Default").clicked() {
                    draft.color = None;
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if save {
        layer_apply_edit(app, &draft);
        app.panels.layer_edit = None;
    } else if cancel || !open {
        app.panels.layer_edit = None;
    } else {
        app.panels.layer_edit = Some(draft);
    }
}

/// Load a display layer into the Attribute Editor and switch the right dock to it.
fn layer_open_attributes(app: &mut IncaApp, id: &str) {
    if app.s.layers.iter().any(|l| l.id == id) {
        app.panels.ae_layer = Some(id.to_string());
        app.panels.ae_layer_sel = app.s.sel.clone();
        app.panels.ae_locked = None;
        app.right_tab = crate::app::RightTab::AttributeEditor;
        app.show_right = true;
        app.panels.ae_tab_scroll = 0.0;
    }
}

fn layer_open_membership(app: &mut IncaApp, id: &str) {
    if app.s.layers.iter().any(|l| l.id == id) {
        app.panels.layer_membership = Some(id.to_string());
        app.panels.membership_sel.clear();
        app.panels.membership_filter.clear();
        app.panels.membership_layers_open.insert(id.to_string());
    }
}

/// Connect or disconnect several objects in one undo step. Objects already on another layer are
/// moved when adding; removing only touches objects that belong to this layer.
fn layer_set_members(app: &mut IncaApp, lid: &str, ids: &[String], member: bool) {
    let ids: Vec<String> = ids
        .iter()
        .filter(|id| {
            app.s.node(id).is_some_and(|n| {
                let is_member = n.layer.as_deref() == Some(lid);
                if member { !is_member } else { is_member }
            })
        })
        .cloned()
        .collect();
    if ids.is_empty() {
        return;
    }
    app.s.checkpoint(if member { "add objects to display layer" } else { "remove objects from display layer" });
    for id in ids {
        if let Some(n) = app.s.nodes.get_mut(&id) {
            n.layer = if member { Some(lid.to_string()) } else { None };
        }
    }
    app.s.touch();
}

/// Display layer attributes inside the Attribute Editor panel.
fn layer_attribute_editor(app: &mut IncaApp, ui: &mut egui::Ui, lid: &str) {
    let Some(name) = app.s.layers.iter().find(|l| l.id == lid).map(|l| l.name.clone()) else { return };
    if app.panels.ae_name.0 != name {
        app.panels.ae_name = (name.clone(), name.clone());
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("displayLayer:").color(theme::TEXT_DIM));
        let w = (ui.available_width() - 130.0).max(40.0);
        let te = ui.add(egui::TextEdit::singleline(&mut app.panels.ae_name.1).desired_width(w));
        if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let new = app.panels.ae_name.1.trim().to_string();
            if !new.is_empty() && new != name {
                app.s.checkpoint("rename display layer");
                layer_apply_rename(app, lid, &new);
                app.s.touch();
            }
        }
        if ui.button("Focus").on_hover_text("Show the layer in the Layer Editor").clicked() {
            app.panels.layer_tab = 0;
            app.panels.layer_sel = vec![lid.to_string()];
        }
    });
    let h = ui.available_height() - 26.0;
    egui::ScrollArea::vertical().id_salt("layer_aescroll").max_height(h.max(40.0)).auto_shrink([false, false]).show(ui, |ui| {
        layer_ae_body(app, ui, lid);
    });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.add_sized([88.0, 20.0], egui::Button::new("Select")).on_hover_text("Select the objects in this layer").clicked() {
            let ids: Vec<String> = app.s.nodes.values().filter(|n| n.layer.as_deref() == Some(lid)).map(|n| n.id.clone()).collect();
            app.select_names(&ids, SelMode::Replace);
        }
        if ui.add_sized([72.0, 20.0], egui::Button::new("Close")).clicked() {
            app.show_right = false;
            app.panels.ae_layer = None;
        }
    });
}

fn layer_ae_body(app: &mut IncaApp, ui: &mut egui::Ui, lid: &str) {
    let Some((number, mut enabled, mut shading, mut texturing, mut anim_playback, mut visible, mut lod, mut mode, mut color)) = app
        .s
        .layers
        .iter()
        .enumerate()
        .find(|(_, l)| l.id == lid)
        .map(|(i, l)| {
            let color = if layer_has_color(&l.color) {
                let c = layer_swatch_color(&l.color);
                Some([c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0])
            } else {
                None
            };
            (i + 1, l.enabled, l.shading, l.texturing, l.anim_playback, l.visible, l.lod.clone(), l.mode.clone(), color)
        })
    else {
        return;
    };
    let mut changed = false;
    theme::section_header(ui, "Display Layer Attributes");
    egui::Grid::new("layer_ae_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Enable Overrides");
        changed |= ui.checkbox(&mut enabled, "").on_hover_text("Turn off to make the layer behave as if unassigned").changed();
        ui.end_row();
        ui.label("Display Type");
        egui::ComboBox::from_id_salt("layer_ae_type")
            .selected_text(match mode.as_str() {
                "template" => "Template",
                "reference" => "Reference",
                _ => "Normal",
            })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut mode, "normal".into(), "Normal").changed();
                changed |= ui.selectable_value(&mut mode, "template".into(), "Template").changed();
                changed |= ui.selectable_value(&mut mode, "reference".into(), "Reference").changed();
            });
        ui.end_row();
        ui.label("Level of Detail");
        egui::ComboBox::from_id_salt("layer_ae_lod")
            .selected_text(if lod == "bbox" { "Bounding Box" } else { "Full" })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut lod, "full".into(), "Full").changed();
                changed |= ui.selectable_value(&mut lod, "bbox".into(), "Bounding Box").changed();
            });
        ui.end_row();
        ui.label("Shading");
        changed |= ui.checkbox(&mut shading, "").changed();
        ui.end_row();
        ui.label("Texturing");
        changed |= ui.checkbox(&mut texturing, "").changed();
        ui.end_row();
        ui.label("Playback");
        changed |= ui.checkbox(&mut anim_playback, "").on_hover_text("Animate the layer's objects during playback").changed();
        ui.end_row();
        ui.label("Visible");
        changed |= ui.checkbox(&mut visible, "").changed();
        ui.end_row();
        ui.label("Color");
        ui.horizontal(|ui| {
            let mut c = color.unwrap_or([1.0; 3]);
            if color_edit(ui, egui::Id::new("layer_ae_color"), &mut c) {
                color = Some(c);
                changed = true;
            }
            if ui.button("Default").clicked() {
                color = None;
                changed = true;
            }
        });
        ui.end_row();
        ui.label("Number");
        ui.label(number.to_string());
        ui.end_row();
    });
    if changed {
        layer_update(app, lid, "layer attributes", |l| {
            l.enabled = enabled;
            l.shading = shading;
            l.texturing = texturing;
            l.anim_playback = anim_playback;
            l.visible = visible;
            l.lod = lod;
            l.mode = mode;
            l.color = color.map(|c| json!([c[0] as f64, c[1] as f64, c[2] as f64]));
        });
    }
}

/// Relationship Editor: Layers on the left, scene objects on the right, with + / - to connect
/// the selected objects to the selected display layer.
fn layer_membership_dialog(app: &mut IncaApp, ctx: &egui::Context) {
    let Some(lid) = app.panels.layer_membership.clone() else { return };
    let Some(layer_name) = app.s.layers.iter().find(|l| l.id == lid).map(|l| l.name.clone()) else {
        app.panels.layer_membership = None;
        return;
    };
    let layers: Vec<(String, String, Option<serde_json::Value>)> = app.s.layers.iter().map(|l| (l.id.clone(), l.name.clone(), l.color.clone())).collect();
    let mut open = true;
    let mut add: Option<Vec<String>> = None;
    let mut remove: Option<Vec<String>> = None;
    let center = ctx.content_rect().center();
    let selected_count = app.panels.membership_sel.len();
    egui::Window::new("Relationship Editor")
        .id(egui::Id::new("layer_membership_editor"))
        .default_size([640.0, 470.0])
        .resizable(true)
        .default_pos(egui::pos2(center.x - 320.0, center.y - 235.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Display layers").strong());
                let can = selected_count > 0;
                if ui.add_enabled(can, egui::Button::new("+")).on_hover_text("Add selected objects to the layer").clicked() {
                    add = Some(app.panels.membership_sel.clone());
                }
                if ui.add_enabled(can, egui::Button::new("-")).on_hover_text("Remove selected objects from the layer").clicked() {
                    remove = Some(app.panels.membership_sel.clone());
                }
                ui.separator();
                ui.label(RichText::new(format!("{layer_name} — {selected_count} selected")).color(theme::TEXT_DIM));
            });
            ui.separator();
            let pane_h = 340.0;
            ui.columns(2, |cols| {
                cols[0].vertical(|ui| {
                    ui.vertical_centered(|ui| ui.label(RichText::new("Layers").strong()));
                    egui::ScrollArea::vertical().id_salt("membership_layers").max_height(pane_h).auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        for (id, lname, color) in &layers {
                            let selected = app.panels.layer_membership.as_deref() == Some(id.as_str());
                            let expanded = app.panels.membership_layers_open.contains(id);
                            let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
                            if selected {
                                ui.painter().rect_filled(r, 0.0, theme::HI);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(r, 0.0, Color32::from_gray(0x4d));
                            }
                            let exp = egui::Rect::from_min_size(egui::pos2(r.min.x + 4.0, r.center().y - 6.0), egui::vec2(12.0, 12.0));
                            let exp_hot = resp.hovered() && ui.ctx().pointer_hover_pos().is_some_and(|p| exp.contains(p));
                            ui.painter().rect_filled(exp, 1.0, if exp_hot { Color32::from_gray(0x55) } else { Color32::from_gray(0x30) });
                            ui.painter().rect_stroke(exp, 1.0, egui::Stroke::new(1.0, Color32::from_gray(0x88)), egui::StrokeKind::Inside);
                            let ec = exp.center();
                            ui.painter().line_segment([egui::pos2(ec.x - 3.0, ec.y), egui::pos2(ec.x + 3.0, ec.y)], egui::Stroke::new(1.0, theme::TEXT));
                            if !expanded {
                                ui.painter().line_segment([egui::pos2(ec.x, ec.y - 3.0), egui::pos2(ec.x, ec.y + 3.0)], egui::Stroke::new(1.0, theme::TEXT));
                            }
                            let sw = egui::Rect::from_center_size(egui::pos2(r.min.x + 26.0, r.center().y), egui::vec2(12.0, 12.0));
                            let c = layer_swatch_color(color);
                            if c == Color32::TRANSPARENT {
                                ui.painter().rect_stroke(sw, 1.0, egui::Stroke::new(1.0, Color32::from_gray(0x99)), egui::StrokeKind::Inside);
                            } else {
                                ui.painter().rect_filled(sw, 1.0, c);
                            }
                            ui.painter().text(egui::pos2(r.min.x + 38.0, r.center().y), egui::Align2::LEFT_CENTER, lname, egui::FontId::proportional(12.0), if selected { Color32::WHITE } else { theme::TEXT });
                            let on_exp = resp.interact_pointer_pos().is_some_and(|p| exp.contains(p));
                            if resp.clicked() {
                                if on_exp {
                                    if expanded {
                                        app.panels.membership_layers_open.remove(id);
                                    } else {
                                        app.panels.membership_layers_open.insert(id.clone());
                                    }
                                } else {
                                    app.panels.layer_membership = Some(id.clone());
                                    app.panels.membership_sel.clear();
                                }
                            }
                            if expanded {
                                let mut members: Vec<(String, String)> = app
                                    .s
                                    .nodes
                                    .values()
                                    .filter(|n| n.layer.as_deref() == Some(id.as_str()))
                                    .map(|n| (n.id.clone(), n.name.clone()))
                                    .collect();
                                members.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
                                for (mid, mname) in members {
                                    let (cr, cresp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
                                    let sel = app.panels.membership_sel.iter().any(|s| s == &mid);
                                    if sel {
                                        ui.painter().rect_filled(cr, 0.0, theme::HI);
                                    } else if cresp.hovered() {
                                        ui.painter().rect_filled(cr, 0.0, Color32::from_gray(0x4d));
                                    }
                                    let lx = r.min.x + 10.0;
                                    let cy = cr.center().y;
                                    let line = egui::Stroke::new(1.0, Color32::from_gray(0x77));
                                    ui.painter().line_segment([egui::pos2(lx, r.bottom()), egui::pos2(lx, cy)], line);
                                    ui.painter().line_segment([egui::pos2(lx, cy), egui::pos2(lx + 8.0, cy)], line);
                                    ui.painter().circle_filled(egui::pos2(lx + 9.0, cy), 2.0, Color32::from_gray(0xa0));
                                    if let Some(n) = app.s.node(&mid) {
                                        let icon = kind_icon(n);
                                        crate::icons::paint(ui, egui::Rect::from_min_size(egui::pos2(r.min.x + 22.0, cy - 7.5), egui::vec2(15.0, 15.0)), icon, Color32::from_white_alpha(255));
                                    }
                                    let display = shown_node_name(app, &mname);
                                    ui.painter().text(egui::pos2(r.min.x + 40.0, cy), egui::Align2::LEFT_CENTER, display, egui::FontId::proportional(12.0), if sel { Color32::WHITE } else { theme::TEXT });
                                    if cresp.clicked() {
                                        app.panels.layer_membership = Some(id.clone());
                                        app.panels.membership_sel = vec![mid];
                                    }
                                }
                            }
                        }
                    });
                });
                cols[1].vertical(|ui| {
                    ui.vertical_centered(|ui| ui.label(RichText::new("Objects").strong()));
                    ui.add(egui::TextEdit::singleline(&mut app.panels.membership_filter).hint_text("Search...").desired_width(f32::INFINITY));
                    let filter = app.panels.membership_filter.trim().to_lowercase().replace('*', "");
                    egui::ScrollArea::vertical().id_salt("membership_objects").max_height(pane_h).auto_shrink([false, false]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_min_width(ui.available_width());
                        for root in app.s.roots.clone() {
                            membership_obj_row(app, ui, &root, 0, &filter, &lid);
                        }
                    });
                });
            });
        });
    if !open {
        app.panels.layer_membership = None;
        app.panels.membership_sel.clear();
        app.panels.membership_filter.clear();
    } else {
        if let Some(ids) = add {
            layer_set_members(app, &lid, &ids, true);
        }
        if let Some(ids) = remove {
            layer_set_members(app, &lid, &ids, false);
        }
    }
}

/// One object row in the Membership editor's tree. Selection is local to the editor; a dot marks
/// objects already connected to the layer.
fn membership_obj_row(app: &mut IncaApp, ui: &mut egui::Ui, id: &str, depth: usize, filter: &str, lid: &str) {
    if !matches_filter(app, id, filter) {
        return;
    }
    let Some(n) = app.s.node(id) else { return };
    let full_name = n.name.clone();
    let name = shown_node_name(app, &full_name);
    let icon = kind_icon(n);
    let member = n.layer.as_deref() == Some(lid);
    let mut kids = n.children.clone();
    if app.panels.ol.sort_alpha {
        kids.sort_by_key(|k| app.s.name_of(k).to_lowercase());
    }
    let has_kids = !kids.is_empty();
    let open = has_kids && (app.panels.expanded.contains(id) || !filter.is_empty());
    let selected = app.panels.membership_sel.iter().any(|s| s == id);
    let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(r, 0.0, theme::HI);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, Color32::from_gray(0x4d));
    }
    let x0 = r.min.x + 4.0 + depth as f32 * 14.0;
    let exp_rect = egui::Rect::from_min_size(egui::pos2(x0, r.min.y), egui::vec2(12.0, ROW_H));
    if has_kids {
        let c = exp_rect.center();
        let pts = if open {
            vec![c + egui::vec2(-3.5, -2.0), c + egui::vec2(3.5, -2.0), c + egui::vec2(0.0, 2.5)]
        } else {
            vec![c + egui::vec2(-2.0, -3.5), c + egui::vec2(2.5, 0.0), c + egui::vec2(-2.0, 3.5)]
        };
        ui.painter().add(egui::Shape::convex_polygon(pts, theme::TEXT, egui::Stroke::NONE));
    }
    crate::icons::paint(ui, egui::Rect::from_min_size(egui::pos2(x0 + 12.0, r.center().y - 7.5), egui::vec2(15.0, 15.0)), icon, Color32::from_white_alpha(255));
    ui.painter().text(egui::pos2(x0 + 32.0, r.center().y), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(12.0), if selected { Color32::WHITE } else { theme::TEXT });
    if member {
        ui.painter().circle_filled(egui::pos2(r.right() - 8.0, r.center().y), 3.0, Color32::from_rgb(0x7f, 0xd0, 0x9a));
    }
    let on_exp = resp.interact_pointer_pos().is_some_and(|p| exp_rect.contains(p));
    if resp.clicked() {
        if on_exp && has_kids {
            if open {
                app.panels.expanded.remove(id);
            } else {
                app.panels.expanded.insert(id.to_string());
            }
        } else {
            let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
            if ctrl {
                if selected {
                    app.panels.membership_sel.retain(|s| s != id);
                } else {
                    app.panels.membership_sel.push(id.to_string());
                }
            } else {
                app.panels.membership_sel = vec![id.to_string()];
            }
        }
    }
    if open {
        for k in kids {
            membership_obj_row(app, ui, &k, depth + 1, filter, lid);
        }
    }
}

/// A small fixed-size V/P/T toggle. Painted by hand so hovering changes only the fill and
/// stroke colours, never the button's shape. The letter shows the state; an unselected layer
/// keeps the neutral boxes, while the highlighted layer gets the blue selected fill.
fn layer_toggle_shell(ui: &mut egui::Ui, selected: bool) -> (egui::Rect, egui::Response) {
    layer_toggle_shell_ex(ui, selected, true)
}

fn layer_toggle_shell_ex(ui: &mut egui::Ui, selected: bool, enabled: bool) -> (egui::Rect, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 16.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let hovered = enabled && resp.hovered();
    let fill = if !enabled {
        Color32::from_gray(0x4a)
    } else if selected {
        if hovered { Color32::from_rgb(0x73, 0x92, 0xab) } else { Color32::from_rgb(0x5d, 0x7a, 0x8f) }
    } else if hovered {
        Color32::from_gray(0x48)
    } else {
        Color32::from_gray(0x2e)
    };
    let edge = if !enabled {
        Color32::from_gray(0x68)
    } else if selected {
        Color32::from_rgb(0x81, 0xa9, 0xc4)
    } else if hovered {
        Color32::from_gray(0x93)
    } else {
        Color32::from_gray(0x55)
    };
    ui.painter().rect_filled(rect, 2.0, fill);
    ui.painter().rect_stroke(rect.shrink(0.5), 2.0, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    (rect, resp)
}

fn layer_toggle(ui: &mut egui::Ui, text: &str, selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = layer_toggle_shell(ui, selected);
    let text = text.trim();
    if !text.is_empty() {
        let color = if selected { Color32::from_gray(0x16) } else { Color32::from_gray(0xef) };
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(11.0), color);
    }
    resp.on_hover_text(tip)
}

/// Our lock glyph: gold padlock, shackle open when unlocked and closed when locked.
fn layer_lock_toggle(ui: &mut egui::Ui, locked: bool, selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = layer_toggle_shell(ui, selected);
    let c = rect.center();
    let gold = Color32::from_rgb(0xef, 0xc8, 0x3a);
    let dark = Color32::from_rgb(0x2b, 0x22, 0x10);
    let body = egui::Rect::from_center_size(egui::pos2(c.x, c.y + 2.0), egui::vec2(12.0, 8.0));
    let r = 3.8;
    let sweep = if locked { 1.0 } else { 0.62 };
    let mut pts = vec![];
    for i in 0..=12 {
        let a = std::f32::consts::PI * (1.0 + sweep * i as f32 / 12.0);
        pts.push(egui::pos2(c.x + r * a.cos(), body.min.y + r * a.sin()));
    }
    ui.painter().add(egui::Shape::line(pts.clone(), egui::Stroke::new(2.8, dark)));
    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.5, gold)));
    ui.painter().rect_filled(body, 2.0, gold);
    ui.painter().rect_stroke(body, 2.0, egui::Stroke::new(1.0, dark), egui::StrokeKind::Inside);
    ui.painter().circle_filled(egui::pos2(c.x, body.center().y - 0.5), 1.2, dark);
    ui.painter().line_segment([egui::pos2(c.x, body.center().y + 0.4), egui::pos2(c.x, body.center().y + 2.4)], egui::Stroke::new(1.1, dark));
    resp.on_hover_text(tip)
}

/// The barred circle: grey when the layer plays, red when it is muted.
/// A layer muted by another layer's Solo shows the red icon but cannot be unmuted here.
fn layer_mute_toggle(ui: &mut egui::Ui, muted: bool, solo_muted: bool, selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = layer_toggle_shell_ex(ui, selected, !solo_muted);
    let c = rect.center();
    let color = if solo_muted {
        Color32::from_rgb(0x9a, 0x45, 0x45)
    } else if muted {
        Color32::from_rgb(0xe3, 0x4b, 0x4b)
    } else {
        Color32::from_gray(0x9a)
    };
    ui.painter().circle_stroke(c, 5.4, egui::Stroke::new(1.6, color));
    let d = 5.4 / std::f32::consts::SQRT_2;
    ui.painter().line_segment([egui::pos2(c.x - d, c.y - d), egui::pos2(c.x + d, c.y + d)], egui::Stroke::new(1.6, color));
    resp.on_hover_text(tip)
}

const GHOST_COLORS: [[f32; 3]; 8] = [
    [0.85, 0.10, 0.10],
    [0.85, 0.45, 0.10],
    [0.85, 0.78, 0.15],
    [0.25, 0.65, 0.25],
    [0.15, 0.60, 0.65],
    [0.25, 0.35, 0.85],
    [0.60, 0.30, 0.80],
    [0.80, 0.80, 0.80],
];

fn ghost_swatch(ui: &mut egui::Ui, color: [f32; 3]) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    ui.painter().rect_filled(rect, 2.0, color32_of(&color));
    ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, Color32::from_gray(0x22)), egui::StrokeKind::Inside);
    resp.clicked()
}

/// A coloured dot: the layer's ghost, dimmed when off.
fn layer_ghost_toggle(ui: &mut egui::Ui, on: bool, color: [f32; 3], selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = layer_toggle_shell(ui, selected);
    let c = rect.center();
    let dot = if on {
        color32_of(&color)
    } else {
        Color32::from_rgb((color[0].clamp(0.0, 1.0) * 102.0) as u8, (color[1].clamp(0.0, 1.0) * 102.0) as u8, (color[2].clamp(0.0, 1.0) * 102.0) as u8)
    };
    ui.painter().circle_filled(c, 4.3, dot);
    ui.painter().circle_stroke(c, 4.3, egui::Stroke::new(1.0, Color32::from_gray(0x22)));
    resp.on_hover_text(tip)
}

// ------------------------------------------------------------ animation layers
fn anim_layer_unique_name(app: &IncaApp) -> String {
    let mut i = 1;
    loop {
        let name = format!("AnimLayer{i}");
        if !app.s.anim_layers.iter().any(|l| l.name == name) {
            return name;
        }
        i += 1;
    }
}

fn anim_layer_new(app: &mut IncaApp, override_mode: bool) {
    app.s.checkpoint(if override_mode { "create override animation layer" } else { "create animation layer" });
    let sel = app.s.sel.clone();
    app.s.remember_anim_static(&sel);
    let id = app.s.new_id("al");
    let name = anim_layer_unique_name(app);
    app.s.anim_layers.insert(
        0,
        inca_core::anim::AnimLayer {
            id: id.clone(),
            name,
            weight: 1.0,
            mute: false,
            solo: false,
            lock: false,
            override_mode,
            members: vec![],
            ghost: false,
            ghost_color: inca_core::anim::ghost_red(),
            curves: Default::default(),
        },
    );
    app.s.anim_layer_current = Some(id);
    app.s.touch();
}

fn anim_layer_new_from_selected(app: &mut IncaApp, override_mode: bool) {
    if app.s.sel.is_empty() {
        return;
    }
    let sel = app.s.sel.clone();
    anim_layer_new(app, override_mode);
    if let Some(id) = app.s.anim_layer_current.clone() {
        if let Some(l) = app.s.anim_layers.iter_mut().find(|l| l.id == id) {
            l.members = sel;
        }
        app.s.touch();
    }
}

/// Add or remove the selected objects from an animation layer.
fn anim_layer_add_remove_selected(app: &mut IncaApp, id: &str, add: bool) {
    if app.s.sel.is_empty() {
        return;
    }
    let sel = app.s.sel.clone();
    app.s.checkpoint(if add { "add objects to animation layer" } else { "remove objects from animation layer" });
    if add {
        app.s.remember_anim_static(&sel);
    }
    if let Some(l) = app.s.anim_layers.iter_mut().find(|l| l.id == id) {
        if add {
            for n in sel {
                if !l.members.contains(&n) {
                    l.members.push(n);
                }
            }
        } else {
            l.members.retain(|m| !sel.contains(m));
        }
    }
    app.s.touch();
}

fn anim_layer_select_objects(app: &mut IncaApp, id: &str) {
    let ids = app.s.anim_layers.iter().find(|l| l.id == id).map(|l| l.members.clone()).unwrap_or_default();
    app.select_names(&ids, SelMode::Replace);
}

fn anim_layer_delete(app: &mut IncaApp, id: &str) {
    app.s.checkpoint("delete animation layer");
    if app.s.anim_layers.iter().any(|l| l.id == id && l.solo) {
        anim_layer_unsolo(app);
    }
    let keys: Vec<String> = app.s.anim_layers.iter().filter(|l| l.id == id).flat_map(|l| l.curves.keys().cloned()).collect();
    app.s.anim_layers.retain(|l| l.id != id);
    if app.s.anim_layer_current.as_deref() == Some(id) {
        app.s.anim_layer_current = None;
    }
    let orphan: Vec<String> = keys.into_iter().filter(|k| !app.s.anim_layers.iter().any(|l| l.curves.contains_key(k))).collect();
    app.s.restore_anim_static(&orphan);
    app.s.set_time(app.s.time.current);
    app.s.touch();
}

fn anim_layer_move(app: &mut IncaApp, id: &str, up: bool) {
    let Some(i) = app.s.anim_layers.iter().position(|l| l.id == id) else { return };
    let j = if up {
        i.checked_sub(1)
    } else if i + 1 < app.s.anim_layers.len() {
        Some(i + 1)
    } else {
        None
    };
    let Some(j) = j else { return };
    app.s.checkpoint("reorder animation layers");
    app.s.anim_layers.swap(i, j);
    app.s.set_time(app.s.time.current);
    app.s.touch();
}

fn anim_layer_update(app: &mut IncaApp, id: &str, label: &str, f: impl FnOnce(&mut inca_core::anim::AnimLayer)) {
    app.s.checkpoint(label);
    if let Some(l) = app.s.anim_layers.iter_mut().find(|l| l.id == id) {
        f(l);
    }
    app.s.set_time(app.s.time.current);
    app.s.touch();
}

fn anim_layer_mute(app: &mut IncaApp, id: &str) {
    anim_layer_update(app, id, "mute animation layer", |l| {
        l.mute = !l.mute;
        l.lock = l.mute;
    });
}

fn anim_layer_lock(app: &mut IncaApp, id: &str) {
    anim_layer_update(app, id, "lock animation layer", |l| l.lock = !l.lock);
}

fn anim_layer_mode(app: &mut IncaApp, id: &str, override_mode: bool) {
    app.s.checkpoint("animation layer mode");
    app.s.set_anim_layer_override(id, override_mode);
    app.s.set_time(app.s.time.current);
    app.s.touch();
}

fn anim_layer_unsolo(app: &mut IncaApp) {
    let saved = std::mem::take(&mut app.s.anim_solo_saved);
    for l in &mut app.s.anim_layers {
        l.solo = false;
        if let Some((_, mute, lock)) = saved.iter().find(|(sid, _, _)| sid == &l.id) {
            l.mute = *mute;
            l.lock = *lock;
        }
    }
}

fn anim_layer_solo(app: &mut IncaApp, id: &str) {
    let Some(on) = app.s.anim_layers.iter().find(|l| l.id == id).map(|l| !l.solo) else { return };
    app.s.checkpoint("solo animation layer");
    if !app.s.anim_solo_saved.is_empty() {
        anim_layer_unsolo(app);
    }
    if on {
        app.s.anim_solo_saved = app.s.anim_layers.iter().map(|l| (l.id.clone(), l.mute, l.lock)).collect();
        for l in &mut app.s.anim_layers {
            if l.id == id {
                l.solo = true;
                l.mute = false;
                l.lock = false;
            } else {
                l.solo = false;
                l.mute = true;
                l.lock = true;
            }
        }
    }
    app.s.set_time(app.s.time.current);
    app.s.touch();
}

fn anim_layer_rename_commit(app: &mut IncaApp, id: &str, new: &str) {
    let want = new.trim();
    if want.is_empty() {
        return;
    }
    if app.s.anim_layers.iter().any(|l| l.id != id && l.name == want) {
        return;
    }
    app.s.checkpoint("rename animation layer");
    if let Some(l) = app.s.anim_layers.iter_mut().find(|l| l.id == id) {
        l.name = want.to_string();
    }
    app.s.touch();
}

fn anim_layer_editor(app: &mut IncaApp, ui: &mut egui::Ui) {
    use crate::icons;
    use crate::menus::{menu_check, menu_item, panel_bar, MenuSource};
    // Maya: menus on their own row, the four layer buttons on the row below.
    panel_bar(app, ui, MenuSource::LayerEditor, &["Layers", "Options", "Show", "Help"], true, |app, ui, title| match title {
            "Layers" => {
                if menu_item(app, ui, "Create Empty Layer") {
                    anim_layer_new(app, false);
                }
                if menu_item(app, ui, "Create Override Layer") {
                    anim_layer_new(app, true);
                }
                ui.separator();
                ui.add_enabled_ui(!app.s.sel.is_empty(), |ui| {
                    if menu_item(app, ui, "Create Layer from Selected") {
                        anim_layer_new_from_selected(app, false);
                    }
                    if menu_item(app, ui, "Create Override Layer from Selected") {
                        anim_layer_new_from_selected(app, true);
                    }
                });
                ui.separator();
                ui.add_enabled_ui(app.s.anim_layer_current.is_some() && !app.s.sel.is_empty(), |ui| {
                    if menu_item(app, ui, "Add Selected Objects") {
                        if let Some(id) = app.s.anim_layer_current.clone() {
                            anim_layer_add_remove_selected(app, &id, true);
                        }
                    }
                    if menu_item(app, ui, "Remove Selected Objects") {
                        if let Some(id) = app.s.anim_layer_current.clone() {
                            anim_layer_add_remove_selected(app, &id, false);
                        }
                    }
                });
                ui.add_enabled_ui(app.s.anim_layer_current.is_some(), |ui| {
                    if menu_item(app, ui, "Select Objects") {
                        if let Some(id) = app.s.anim_layer_current.clone() {
                            anim_layer_select_objects(app, &id);
                        }
                    }
                });
                ui.separator();
                if menu_item(app, ui, "Delete Current Layer") {
                    if let Some(id) = app.s.anim_layer_current.clone() {
                        anim_layer_delete(app, &id);
                    }
                }
            }
            "Options" => {
                if let Some(id) = app.s.anim_layer_current.clone() {
                    let (mute, solo, lock) = app.s.anim_layers.iter().find(|l| l.id == id).map(|l| (l.mute, l.solo, l.lock)).unwrap_or((false, false, false));
                    if menu_check(app, ui, mute, "Mute Current Layer") {
                        anim_layer_mute(app, &id);
                    }
                    if menu_check(app, ui, solo, "Solo Current Layer") {
                        anim_layer_solo(app, &id);
                    }
                    if menu_check(app, ui, lock, "Lock Current Layer") {
                        anim_layer_lock(app, &id);
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("No layer selected"));
                }
            }
            "Show" => {
                if menu_check(app, ui, !app.panels.anim_hide_base, "Show Base Animation") {
                    app.panels.anim_hide_base = !app.panels.anim_hide_base;
                }
            }
            "Help" => {
                if menu_item(app, ui, "Animation Layer Editor Help") {
                    app.help = "Select a layer to receive new keys. Weight blends the layer, the barred circle mutes, S solos, the padlock locks, O/A switches override/additive. Keys on additive layers are stored as deltas.".into();
                }
            }
            _ => {}
        });
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let current = app.s.anim_layer_current.clone();
            let has = current.is_some();
            if icons::button_ex(ui, "layerFromSel", "Create an animation layer from the selected objects", false, 24.0, 20.0, !app.s.sel.is_empty()).clicked() {
                anim_layer_new_from_selected(app, false);
            }
            if icons::button_ex(ui, "layerNew", "Create an empty animation layer", false, 24.0, 20.0, true).clicked() {
                anim_layer_new(app, false);
            }
            if icons::button_ex(ui, "layerDown", "Move the current layer down", false, 24.0, 20.0, has).clicked() {
                if let Some(id) = current.as_deref() {
                    anim_layer_move(app, id, false);
                }
            }
            if icons::button_ex(ui, "layerUp", "Move the current layer up", false, 24.0, 20.0, has).clicked() {
                if let Some(id) = current.as_deref() {
                    anim_layer_move(app, id, true);
                }
            }
        });
    });
    let layers = app.s.anim_layers.clone();
    let current = app.s.anim_layer_current.clone();
    let solo_active = layers.iter().any(|l| l.solo);
    let well_h = (ui.available_height() - 4.0).max(ROW_H);
    egui::Frame::NONE
        .fill(theme::BG_DARK)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.set_min_height(well_h);
            egui::ScrollArea::vertical().id_salt("anim_layers").max_height(well_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for l in &layers {
            let selected = current.as_deref() == Some(l.id.as_str());
            let solo_muted = solo_active && !l.solo;
            egui::Frame::NONE.fill(if selected { theme::HI } else { Color32::TRANSPARENT }).inner_margin(egui::Margin::symmetric(1, 1)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let fixed = 46.0 + 5.0 * 20.0 + 6.0 * 3.0;
                    let name_w = (ui.available_width() - fixed).max(60.0);
                    let (name_rect, name_resp) = ui.allocate_exact_size(egui::vec2(name_w, ROW_H), egui::Sense::click());
                    let renaming = app.panels.anim_layer_rename.as_ref().is_some_and(|(id, _)| id == &l.id);
                    if renaming {
                        let mut edit = app.panels.anim_layer_rename.take().unwrap();
                        let resp = ui.put(name_rect, egui::TextEdit::singleline(&mut edit.1));
                        if app.panels.anim_layer_rename_new {
                            resp.request_focus();
                            app.panels.anim_layer_rename_new = false;
                        }
                        if resp.lost_focus() {
                            let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if !cancel {
                                anim_layer_rename_commit(app, &edit.0, &edit.1);
                            }
                        } else {
                            app.panels.anim_layer_rename = Some(edit);
                        }
                    } else {
                        ui.painter().text(
                            egui::pos2(name_rect.min.x + 4.0, name_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &l.name,
                            egui::FontId::proportional(12.0),
                            if l.mute || solo_muted { theme::TEXT_DIM } else if selected { Color32::WHITE } else { theme::TEXT },
                        );
                        if name_resp.clicked() {
                            app.s.anim_layer_current = Some(l.id.clone());
                        }
                        if name_resp.double_clicked() {
                            app.panels.anim_layer_rename = Some((l.id.clone(), l.name.clone()));
                            app.panels.anim_layer_rename_new = true;
                        }
                        name_resp.context_menu(|ui| {
                            if ui.button("Add Selected Objects").clicked() {
                                anim_layer_add_remove_selected(app, &l.id, true);
                                ui.close();
                            }
                            if ui.button("Remove Selected Objects").clicked() {
                                anim_layer_add_remove_selected(app, &l.id, false);
                                ui.close();
                            }
                            if ui.button("Select Objects").clicked() {
                                anim_layer_select_objects(app, &l.id);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Rename").clicked() {
                                app.panels.anim_layer_rename = Some((l.id.clone(), l.name.clone()));
                                app.panels.anim_layer_rename_new = true;
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                anim_layer_delete(app, &l.id);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Move Up").clicked() {
                                anim_layer_move(app, &l.id, true);
                                ui.close();
                            }
                            if ui.button("Move Down").clicked() {
                                anim_layer_move(app, &l.id, false);
                                ui.close();
                            }
                        });
                    }
                    let mut w = l.weight;
                    if ui.add_sized([46.0, ROW_H], egui::DragValue::new(&mut w).speed(0.01).range(0.0..=1.0).max_decimals(2)).changed() {
                        anim_layer_update(app, &l.id, "animation layer weight", |l| l.weight = w);
                    }
                    if layer_mute_toggle(ui, l.mute, solo_muted, selected, "Mute").clicked() {
                        anim_layer_mute(app, &l.id);
                    }
                    if layer_toggle(ui, if l.solo { "S" } else { " " }, selected, "Solo").clicked() {
                        anim_layer_solo(app, &l.id);
                    }
                    if layer_lock_toggle(ui, l.lock, selected, "Lock").clicked() {
                        anim_layer_lock(app, &l.id);
                    }
                    if layer_toggle(ui, if l.override_mode { "O" } else { "A" }, selected, "Override (O) / Additive (A)").clicked() {
                        anim_layer_mode(app, &l.id, !l.override_mode);
                    }
                    let ghost_resp = layer_ghost_toggle(ui, l.ghost, l.ghost_color, selected, "Ghost layer (right-click for color)");
                    if ghost_resp.clicked() {
                        anim_layer_update(app, &l.id, "ghost animation layer", |l| l.ghost = !l.ghost);
                        ui.ctx().request_repaint();
                    }
                    ghost_resp.context_menu(|ui| {
                        ui.label("Ghost color");
                        let mut picked = None;
                        ui.horizontal_wrapped(|ui| {
                            for c in GHOST_COLORS {
                                if ghost_swatch(ui, c) {
                                    picked = Some(c);
                                }
                            }
                        });
                        if let Some(c) = picked {
                            anim_layer_update(app, &l.id, "animation layer ghost color", |l| l.ghost_color = c);
                            ui.ctx().request_repaint();
                            ui.close();
                        }
                    });
                });
            });
        }
        if !app.panels.anim_hide_base {
            anim_base_row(app, ui);
        }
        });
    });
    if app.panels.anim_layer_rename.as_ref().is_some_and(|(id, _)| !app.s.anim_layers.iter().any(|l| &l.id == id)) {
        app.panels.anim_layer_rename = None;
    }
}

fn anim_base_row(app: &mut IncaApp, ui: &mut egui::Ui) {
    let selected = app.s.anim_layer_current.is_none();
    egui::Frame::NONE.fill(if selected { theme::HI } else { Color32::TRANSPARENT }).inner_margin(egui::Margin::symmetric(1, 1)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let fixed = 46.0 + 5.0 * 20.0 + 6.0 * 3.0;
            let name_w = (ui.available_width() - fixed).max(60.0);
            let (name_rect, name_resp) = ui.allocate_exact_size(egui::vec2(name_w, ROW_H), egui::Sense::click());
            ui.painter().text(
                egui::pos2(name_rect.min.x + 4.0, name_rect.center().y),
                egui::Align2::LEFT_CENTER,
                "BaseAnimation",
                egui::FontId::proportional(12.0),
                if selected { Color32::WHITE } else { theme::TEXT },
            );
            if name_resp.clicked() {
                app.s.anim_layer_current = None;
            }
            // keep the Lock in the same column as the layer rows; BaseAnimation has no
            // weight, mute, solo or mode
            for w in [46.0, 20.0, 20.0] {
                ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::hover());
            }
            if layer_lock_toggle(ui, app.s.anim_base_lock, selected, "Lock").clicked() {
                app.s.checkpoint("lock base animation");
                app.s.anim_base_lock = !app.s.anim_base_lock;
                app.s.touch();
            }
            ui.allocate_exact_size(egui::vec2(20.0, ROW_H), egui::Sense::hover());
            let ghost_resp = layer_ghost_toggle(ui, app.s.anim_base_ghost, app.s.anim_base_ghost_color, selected, "Ghost BaseAnimation (right-click for color)");
            if ghost_resp.clicked() {
                app.s.checkpoint("ghost base animation");
                app.s.anim_base_ghost = !app.s.anim_base_ghost;
                app.s.touch();
                ui.ctx().request_repaint();
            }
            ghost_resp.context_menu(|ui| {
                ui.label("Ghost color");
                let mut picked = None;
                ui.horizontal_wrapped(|ui| {
                    for c in GHOST_COLORS {
                        if ghost_swatch(ui, c) {
                            picked = Some(c);
                        }
                    }
                });
                if let Some(c) = picked {
                    app.s.checkpoint("base animation ghost color");
                    app.s.anim_base_ghost_color = c;
                    app.s.touch();
                    ui.ctx().request_repaint();
                    ui.close();
                }
            });
        });
    });
}

fn layer_row(app: &mut IncaApp, ui: &mut egui::Ui, l: &inca_core::scene::Layer) {
    let selected = app.panels.layer_sel.iter().any(|x| x == &l.id);
    let current = app.s.current_layer.as_deref() == Some(l.id.as_str());
    let renaming = app.panels.layer_rename.as_ref().is_some_and(|(id, _)| id == &l.id);
    let fill = if selected { Color32::from_rgb(0x3b, 0x55, 0x72) } else { Color32::TRANSPARENT };
    egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(1, 1)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let vis = layer_toggle(ui, if l.visible { "V" } else { " " }, selected, "Visible");
            if vis.clicked() {
                let id = l.id.clone();
                layer_update(app, &id, "layer visibility", |l| l.visible = !l.visible);
            }
            let play = layer_toggle(ui, if l.playback { "P" } else { " " }, selected, "Visible during playback");
            if play.clicked() {
                let id = l.id.clone();
                layer_update(app, &id, "layer playback visibility", |l| l.playback = !l.playback);
            }
            let mode_tag = match l.mode.as_str() {
                "template" => "T",
                "reference" => "R",
                _ => " ",
            };
            let mode = layer_toggle(ui, mode_tag, selected, "Normal / Template / Reference");
            if mode.clicked() {
                let id = l.id.clone();
                layer_update(app, &id, "layer display type", |l| {
                    l.mode = match l.mode.as_str() {
                        "normal" => "template",
                        "template" => "reference",
                        _ => "normal",
                    }
                    .into();
                });
            }
            let (swatch, swatch_resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
            // The empty swatch keeps its own dark background even on a highlighted row; it must
            // not pick up the blue layer-selection colour.
            let swatch_fill = if layer_has_color(&l.color) { layer_swatch_color(&l.color) } else { theme::BG_DARK };
            ui.painter().rect_filled(swatch, 2.0, swatch_fill);
            ui.painter().rect_stroke(swatch, 2.0, egui::Stroke::new(1.0, Color32::from_gray(0x99)), egui::StrokeKind::Inside);
            if !layer_has_color(&l.color) {
                ui.painter().line_segment(
                    [egui::pos2(swatch.left() + 2.0, swatch.bottom() - 2.0), egui::pos2(swatch.right() - 2.0, swatch.top() + 2.0)],
                    egui::Stroke::new(1.0, Color32::from_gray(0xb4)),
                );
            }
            let swatch_resp = swatch_resp.on_hover_text("Layer color presets");
            if swatch_resp.clicked() {
                let opening = app.panels.layer_color_popup.as_deref() != Some(l.id.as_str());
                app.panels.layer_color_popup = if opening { Some(l.id.clone()) } else { None };
                if opening {
                    let c = layer_swatch_color(&l.color);
                    app.panels.layer_custom_rgb = if layer_has_color(&l.color) {
                        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
                    } else {
                        [1.0; 3]
                    };
                    app.panels.layer_picker = None;
                }
            }
            let mut open = app.panels.layer_color_popup.as_deref() == Some(l.id.as_str());
            if open {
                let mut picked: Option<[f32; 3]> = None;
                let mut custom: Option<[f32; 3]> = None;
                let mut use_default = false;
                let picker_here = app.panels.layer_picker.as_deref() == Some(l.id.as_str());
                egui::Popup::from_response(&swatch_resp)
                    .open_bool(&mut open)
                    .close_behavior(if picker_here {
                        egui::PopupCloseBehavior::IgnoreClicks
                    } else {
                        egui::PopupCloseBehavior::CloseOnClickOutside
                    })
                    .gap(2.0)
                    .show(|ui| {
                        ui.label(RichText::new("Layer Color").strong());
                        for row in LAYER_PALETTE.chunks(8) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                for c in row {
                                    let (r, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                                    let color = Color32::from_rgb(c[0], c[1], c[2]);
                                    let current_color = layer_swatch_color(&l.color);
                                    ui.painter().rect_filled(r, 2.0, color);
                                    let selected = current_color == color;
                                    ui.painter().rect_stroke(
                                        r,
                                        2.0,
                                        egui::Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { Color32::WHITE } else { Color32::from_gray(0x22) }),
                                        egui::StrokeKind::Inside,
                                    );
                                    if resp.clicked() {
                                        picked = Some([c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]);
                                    }
                                }
                            });
                        }
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Custom:").strong());
                            let c = app.panels.layer_custom_rgb;
                            let (sw, resp) = ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::click());
                            let rgb = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
                            ui.painter().rect_filled(sw, 2.0, rgb);
                            ui.painter().rect_stroke(sw, 2.0, egui::Stroke::new(1.0, Color32::from_gray(0x22)), egui::StrokeKind::Inside);
                            if resp.on_hover_text("Open the color picker").clicked() {
                                app.panels.layer_picker = if picker_here { None } else { Some(l.id.clone()) };
                            }
                            if ui.button("Set").clicked() {
                                custom = Some(c);
                            }
                        });
                        if ui.button("Default").clicked() {
                            use_default = true;
                        }
                    });
                if app.panels.layer_picker.as_deref() == Some(l.id.as_str()) {
                    let mut keep = true;
                    let frame = egui::Frame::window(ui.style()).shadow(egui::epaint::Shadow::NONE);
                    let screen = ui.ctx().content_rect();
                    let x = (swatch.left() - 380.0).clamp(screen.left() + 20.0, (screen.right() - 360.0).max(screen.left() + 20.0));
                    egui::Window::new("Layer Color")
                        .frame(frame)
                        .id(egui::Id::new("layer_color_picker"))
                        .collapsible(false)
                        .resizable(false)
                        .min_width(320.0)
                        .default_pos(egui::pos2(x, swatch.top() - 30.0))
                        .open(&mut keep)
                        .show(ui.ctx(), |ui| {
                            color_picker_body(ui, egui::Id::new("layer_color_picker"), &mut app.panels.layer_custom_rgb);
                        });
                    if !keep {
                        app.panels.layer_picker = None;
                    }
                }
                if let Some(c) = picked {
                    layer_set_color(app, &l.id, c);
                    app.panels.layer_color_popup = None;
                    app.panels.layer_picker = None;
                } else if let Some(c) = custom {
                    layer_set_color(app, &l.id, c);
                    app.panels.layer_color_popup = None;
                    app.panels.layer_picker = None;
                } else if use_default {
                    layer_clear_color(app, &l.id);
                    app.panels.layer_color_popup = None;
                    app.panels.layer_picker = None;
                } else if !open {
                    app.panels.layer_color_popup = None;
                    app.panels.layer_picker = None;
                }
            }
            if renaming {
                let mut edit = app.panels.layer_rename.take().unwrap();
                let resp = ui.add(egui::TextEdit::singleline(&mut edit.1).desired_width(ui.available_width()));
                if app.panels.layer_rename_new {
                    resp.request_focus();
                    app.panels.layer_rename_new = false;
                }
                if resp.lost_focus() {
                    if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        let id = edit.0.clone();
                        layer_rename_commit(app, &id, &edit.1);
                    }
                } else {
                    app.panels.layer_rename = Some(edit);
                }
                return;
            }
            let display = if app.panels.cb_hide_namespace {
                l.name.rsplit(':').next().unwrap_or(&l.name).to_string()
            } else {
                l.name.clone()
            };
            // The name field spans the rest of the row, so the empty area to its right also
            // selects the layer (Maya selects anywhere in the row). Paint the text ourselves
            // so it stays left-aligned inside that full-width hit area.
            let (name_rect, name) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 16.0), egui::Sense::click());
            let color = if current { Color32::WHITE } else { Color32::from_gray(0xcf) };
            ui.painter().text(
                egui::pos2(name_rect.left() + 2.0, name_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &display,
                egui::FontId::proportional(12.0),
                color,
            );
            if name.clicked() {
                let mods = ui.input(|i| i.modifiers);
                if mods.shift {
                    let anchor = app.panels.layer_anchor.clone().unwrap_or_else(|| l.id.clone());
                    let (a, b) = (app.s.layers.iter().position(|x| x.id == anchor), app.s.layers.iter().position(|x| x.id == l.id));
                    if let (Some(a), Some(b)) = (a, b) {
                        app.panels.layer_sel = app.s.layers[a.min(b)..=a.max(b)].iter().map(|x| x.id.clone()).collect();
                    }
                } else if mods.ctrl || mods.command {
                    if selected {
                        app.panels.layer_sel.retain(|x| x != &l.id);
                    } else {
                        app.panels.layer_sel.push(l.id.clone());
                    }
                } else {
                    app.panels.layer_sel = vec![l.id.clone()];
                }
                app.panels.layer_anchor = Some(l.id.clone());
                app.s.current_layer = Some(l.id.clone());
            }
            if name.double_clicked() {
                layer_open_edit(app, &l.id);
            }
            if name.secondary_clicked() && !selected {
                app.panels.layer_sel = vec![l.id.clone()];
                app.panels.layer_anchor = Some(l.id.clone());
                app.s.current_layer = Some(l.id.clone());
            }
            name.context_menu(|ui| layer_context_menu(app, ui, &l.id));
        });
    });
}

// ------------------------------------------------------------------ attribute editor

#[derive(Clone, Copy)]
enum FieldKind {
    Float,
    Color,
    Bool,
    Text,
    Enum(&'static [&'static str]),
}

#[derive(Clone, Copy)]
struct FieldSpec {
    key: &'static str,
    label: &'static str,
    kind: FieldKind,
    min: f32,
    max: f32,
}

fn fspec(key: &'static str, label: &'static str, min: f32, max: f32) -> FieldSpec {
    FieldSpec { key, label, kind: FieldKind::Float, min, max }
}
fn cspec(key: &'static str, label: &'static str) -> FieldSpec {
    FieldSpec { key, label, kind: FieldKind::Color, min: 0.0, max: 1.0 }
}
fn bspec(key: &'static str, label: &'static str) -> FieldSpec {
    FieldSpec { key, label, kind: FieldKind::Bool, min: 0.0, max: 1.0 }
}
fn tspec(key: &'static str, label: &'static str) -> FieldSpec {
    FieldSpec { key, label, kind: FieldKind::Text, min: 0.0, max: 1.0 }
}
fn espec(key: &'static str, label: &'static str, options: &'static [&'static str]) -> FieldSpec {
    FieldSpec { key, label, kind: FieldKind::Enum(options), min: 0.0, max: 1.0 }
}

fn material_layout(ty: &str) -> Vec<(&'static str, Vec<FieldSpec>)> {
    match ty {
        "blinn" => vec![
            ("Common Material Attributes", vec![cspec("color", "Color"), cspec("transparency", "Transparency"), cspec("ambientColor", "Ambient Color"), cspec("incandescence", "Incandescence"), fspec("bump", "Bump", 0.0, 2.0), fspec("diffuse", "Diffuse", 0.0, 1.0)]),
            ("Specular Shading", vec![fspec("eccentricity", "Eccentricity", 0.0, 1.0), fspec("specularRollOff", "Specular Roll Off", 0.0, 1.0), cspec("specularColor", "Specular Color"), fspec("reflectivity", "Reflectivity", 0.0, 1.0)]),
        ],
        "phong" => vec![
            ("Common Material Attributes", vec![cspec("color", "Color"), cspec("transparency", "Transparency"), cspec("ambientColor", "Ambient Color"), cspec("incandescence", "Incandescence"), fspec("bump", "Bump", 0.0, 2.0), fspec("diffuse", "Diffuse", 0.0, 1.0)]),
            ("Specular Shading", vec![fspec("cosinePower", "Cosine Power", 2.0, 100.0), cspec("specularColor", "Specular Color"), fspec("reflectivity", "Reflectivity", 0.0, 1.0)]),
        ],
        "standardSurface" => vec![
            ("Base", vec![fspec("base", "Base", 0.0, 1.0), cspec("baseColor", "Base Color"), fspec("diffuseRoughness", "Diffuse Roughness", 0.0, 1.0), fspec("metalness", "Metalness", 0.0, 1.0)]),
            ("Specular", vec![fspec("specular", "Specular", 0.0, 1.0), cspec("specularColor", "Specular Color"), fspec("specularRoughness", "Specular Roughness", 0.0, 1.0), fspec("specularIOR", "Specular IOR", 1.0, 3.0)]),
            ("Transmission", vec![fspec("transmission", "Transmission", 0.0, 1.0), cspec("transmissionColor", "Transmission Color")]),
            ("Coat", vec![fspec("coat", "Coat", 0.0, 1.0), fspec("coatRoughness", "Coat Roughness", 0.0, 1.0)]),
            ("Sheen", vec![fspec("sheen", "Sheen", 0.0, 1.0), cspec("sheenColor", "Sheen Color")]),
            ("Emission", vec![fspec("emission", "Emission", 0.0, 10.0), cspec("emissionColor", "Emission Color")]),
            ("Geometry", vec![cspec("opacity", "Opacity"), bspec("thinWalled", "Thin Walled"), fspec("bump", "Bump", 0.0, 2.0)]),
        ],
        "surfaceShader" => vec![("Surface Shader Attributes", vec![cspec("outColor", "Out Color"), cspec("outTransparency", "Out Transparency")])],
        _ => vec![("Common Material Attributes", vec![cspec("color", "Color"), cspec("transparency", "Transparency"), cspec("ambientColor", "Ambient Color"), cspec("incandescence", "Incandescence"), fspec("bump", "Bump", 0.0, 2.0), fspec("diffuse", "Diffuse", 0.0, 1.0)])],
    }
}

fn field_editor(app: &mut IncaApp, ui: &mut egui::Ui, r: &NodeRef, spec: FieldSpec) {
    let Some(v) = app.s.get_attr(r, spec.key) else { return };
    match spec.kind {
        FieldKind::Float => {
            let mut x = v.as_f64().unwrap_or(0.0) as f32;
            let resp = ui.add(egui::Slider::new(&mut x, spec.min..=spec.max).max_decimals(3));
            if resp.changed() {
                edit_channel(app, std::slice::from_ref(r), spec.key, json!(x), &resp);
            }
            finish_edit(app, std::slice::from_ref(r), spec.key, &resp);
        }
        FieldKind::Color => {
            let a = v.as_array();
            let mut c = [0.0, 0.0, 0.0];
            if let Some(a) = a {
                for i in 0..3 {
                    c[i] = a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                }
            }
            let color_id = egui::Id::new(("attr_color", spec.key, format!("{r:?}")));
            if color_edit(ui, color_id, &mut c) {
                if !app.panels.editing {
                    app.s.checkpoint(&format!("set {}", spec.key));
                }
                app.s.set_attr(r, spec.key, json!([c[0], c[1], c[2]]));
            }
        }
        FieldKind::Bool => {
            let mut b = v.as_bool().unwrap_or(false);
            if ui.checkbox(&mut b, "").changed() {
                app.s.checkpoint(&format!("set {}", spec.key));
                app.s.set_attr(r, spec.key, json!(b));
            }
        }
        FieldKind::Text => {
            let mut s = v.as_str().unwrap_or("").to_string();
            if ui.text_edit_singleline(&mut s).changed() {
                app.s.checkpoint(&format!("set {}", spec.key));
                app.s.set_attr(r, spec.key, json!(s));
            }
        }
        FieldKind::Enum(options) => {
            let mut cur = v.as_str().unwrap_or(options[0]).to_string();
            egui::ComboBox::from_id_salt(("field", app.s.ref_id(r), spec.key)).selected_text(&cur).show_ui(ui, |ui| {
                for o in options {
                    ui.selectable_value(&mut cur, o.to_string(), *o);
                }
            });
            if cur != v.as_str().unwrap_or(options[0]) {
                app.s.checkpoint(&format!("set {}", spec.key));
                app.s.set_attr(r, spec.key, json!(cur));
            }
        }
    }
}

fn field_grid(app: &mut IncaApp, ui: &mut egui::Ui, r: &NodeRef, specs: &[FieldSpec]) {
    egui::Grid::new(("fields", app.s.ref_id(r), specs.first().map(|s| s.key).unwrap_or(""))).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for spec in specs {
            if app.s.get_attr(r, spec.key).is_none() {
                continue;
            }
            ui.add_sized([108.0, 18.0], egui::Label::new(spec.label).truncate());
            field_editor(app, ui, r, *spec);
            ui.end_row();
        }
    });
}

fn ramp_entries(v: &Value) -> Vec<(f64, [f32; 3], String)> {
    v.as_array().map(|a| a.iter().filter_map(|e| {
        let p = e.as_array()?;
        let pos = p.first()?.as_f64()?;
        let c = p.get(1)?.as_array()?;
        let interp = p.get(2).and_then(|x| x.as_str()).unwrap_or("Linear").to_string();
        Some((pos, [c.first()?.as_f64()? as f32, c.get(1)?.as_f64()? as f32, c.get(2)?.as_f64()? as f32], interp))
    }).collect()).unwrap_or_default()
}

const RAMP_INTERP: [&str; 7] = ["None", "Linear", "Exponential Up", "Exponential Down", "Smooth", "Bump", "Spike"];

fn ramp_factor(name: &str, t: f32) -> f32 {
    match name {
        "None" => 0.0,
        "Exponential Up" => t * t,
        "Exponential Down" => 1.0 - (1.0 - t) * (1.0 - t),
        "Smooth" => t * t * (3.0 - 2.0 * t),
        "Bump" => (t * std::f32::consts::PI).sin(),
        "Spike" => 1.0 - (2.0 * t - 1.0).abs(),
        _ => t,
    }
}

/// Samples a ramp in linear RGB, matching the stored values and the color picker.
fn ramp_color(entries: &[(f64, [f32; 3], String)], t: f64) -> [f32; 3] {
    if entries.is_empty() {
        return [0.09, 0.09, 0.09];
    }
    if t <= entries[0].0 {
        return entries[0].1;
    }
    for w in entries.windows(2) {
        let (p0, c0, interp) = (w[0].0, w[0].1, &w[0].2);
        let (p1, c1) = (w[1].0, w[1].1);
        if t <= p1 {
            let raw = if p1 > p0 { ((t - p0) / (p1 - p0)) as f32 } else { 0.0 };
            let f = ramp_factor(interp, raw);
            return [0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * f);
        }
    }
    entries[entries.len() - 1].1
}

/// Linear RGB to a screen color, same conversion egui's color button uses.
fn lin_color(c: [f32; 3]) -> Color32 {
    Color32::from(egui::Rgba::from_rgb(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)))
}

fn ramp_editor(app: &mut IncaApp, ui: &mut egui::Ui, r: &NodeRef) {
    let Some(v) = app.s.get_attr(r, "colors") else { return };
    // Keep stored order stable: sorting here would swap the index identities of the
    // dot widgets mid-drag, making the dragged dot hand off to its neighbour.
    let mut entries = ramp_entries(&v);
    let sorted = |e: &[(f64, [f32; 3], String)]| {
        let mut s = e.to_vec();
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
        s
    };
    let state_id = ui.make_persistent_id(("ramp_selected", app.s.ref_id(r)));
    let mut selected = ui.data_mut(|d| d.get_temp::<usize>(state_id).unwrap_or(0));
    selected = selected.min(entries.len().saturating_sub(1));
    theme::section_header(ui, "Ramp");
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    let bar = Rect::from_min_max(pos2(rect.left() + 6.0, rect.top() + 14.0), pos2(rect.right() - 6.0, rect.top() + 38.0));
    let steps = bar.width().max(1.0).ceil() as i32;
    let display = sorted(&entries);
    for x in 0..steps {
        let t = x as f64 / steps.max(1) as f64;
        painter.line_segment([pos2(bar.left() + x as f32, bar.top()), pos2(bar.left() + x as f32, bar.bottom())], egui::Stroke::new(1.0, lin_color(ramp_color(&display, t))));
    }
    painter.rect_stroke(bar, 2.0, egui::Stroke::new(1.0, Color32::from_gray(0x22)), egui::StrokeKind::Inside);
    let mut changed = false;
    let bar_resp = ui.interact(bar, ui.make_persistent_id(("ramp_bar", app.s.ref_id(r))), egui::Sense::click());
    if bar_resp.clicked() {
        if let Some(q) = bar_resp.interact_pointer_pos() {
            let pos = (((q.x - bar.left()) / bar.width()) as f64).clamp(0.0, 1.0);
            entries.push((pos, ramp_color(&display, pos), "Linear".into()));
            selected = entries.len() - 1;
            changed = true;
        }
    }
    let mut remove = None;
    for i in 0..entries.len() {
        let hx = bar.left() + entries[i].0 as f32 * bar.width();
        let hy = rect.top() + 7.0;
        let cr = Rect::from_center_size(pos2(hx, hy), vec2(16.0, 16.0));
        let cr = ui.interact(cr, ui.make_persistent_id(("ramp_handle", app.s.ref_id(r), i)), egui::Sense::click_and_drag());
        if cr.dragged() {
            if let Some(p) = cr.interact_pointer_pos() {
                entries[i].0 = (((p.x - bar.left()) / bar.width()) as f64).clamp(0.0, 1.0);
                changed = true;
            }
        }
        if cr.clicked() {
            selected = i;
        }
        if cr.secondary_clicked() && entries.len() > 1 {
            remove = Some(i);
        }
        painter.circle_filled(pos2(hx, hy), 5.0, lin_color(entries[i].1));
        painter.circle_stroke(pos2(hx, hy), 6.0, egui::Stroke::new(if selected == i { 2.0 } else { 1.0 }, if selected == i { Color32::WHITE } else { Color32::from_gray(0x33) }));
    }
    if let Some(i) = remove {
        entries.remove(i);
        selected = selected.min(entries.len().saturating_sub(1));
        changed = true;
    }
    if let Some((pos, rgb, interp)) = entries.get_mut(selected) {
        ui.horizontal(|ui| {
            ui.label("Selected Color");
            changed |= color_edit(ui, egui::Id::new(("ramp_color", app.s.ref_id(r))), rgb);
            ui.label("Position");
            changed |= ui.add(egui::DragValue::new(pos).range(0.0..=1.0).speed(0.01).max_decimals(3)).changed();
        });
        ui.horizontal(|ui| {
            ui.label("Interpolation");
            let mut cur = interp.clone();
            egui::ComboBox::from_id_salt(("ramp_interp", app.s.ref_id(r))).selected_text(&cur).show_ui(ui, |ui| {
                for o in RAMP_INTERP {
                    ui.selectable_value(&mut cur, o.to_string(), o);
                }
            });
            if cur != *interp {
                *interp = cur;
                changed = true;
            }
        });
    }
    ui.data_mut(|d| d.insert_temp(state_id, selected));
    if changed {
        app.s.checkpoint("edit ramp");
        app.s.set_attr(r, "colors", json!(entries.iter().map(|(p, c, i)| json!([p, c, i])).collect::<Vec<_>>()));
    }
}

/// Generic editor for a JSON attribute map (materials, lights, textures, history params).
pub fn attr_map_editor(app: &mut IncaApp, ui: &mut egui::Ui, r: &NodeRef, keys: &[String]) {
    egui::Grid::new(("ae", app.s.ref_id(r))).num_columns(2).spacing([8.0, 4.0]).striped(false).show(ui, |ui| {
        for k in keys {
            let Some(v) = app.s.get_attr(r, k) else { continue };
            // Fixed label column: keeps every node's attributes aligned instead of
            // re-flowing the panel width as the selected node changes.
            ui.add_sized([108.0, 18.0], egui::Label::new(ops::nice_name(k)).truncate());
            match v {
                Value::Array(a) if a.len() == 3 && a.iter().all(|x| x.is_number()) => {
                    let mut c = [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32];
                    let is_color = k.to_lowercase().contains("color") || k.contains("transparency") || k.contains("incandescence") || k == "opacity";
                    let changed = if is_color {
                        ui.allocate_ui_with_layout(egui::vec2(152.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            color_edit(ui, egui::Id::new(("ae_color", format!("{r:?}"), k)), &mut c)
                        }).inner
                    } else {
                        ui.allocate_ui_with_layout(egui::vec2(152.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let mut ch = false;
                            for x in c.iter_mut() {
                                ch |= ui.add_sized([48.0, 18.0], egui::DragValue::new(x).speed(0.02)).changed();
                            }
                            ch
                        }).inner
                    };
                    if changed {
                        if !app.panels.editing {
                            app.s.checkpoint(&format!("setAttr {k}"));
                        }
                        app.s.set_attr(r, k, json!([c[0], c[1], c[2]]));
                    }
                }
                Value::Number(n) => {
                    let mut x = n.as_f64().unwrap_or(0.0);
                    let resp = ui.allocate_ui_with_layout(egui::vec2(152.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add_sized([64.0, 18.0], egui::DragValue::new(&mut x).speed(0.01).max_decimals(3))
                    }).inner;
                    if resp.changed() {
                        edit_channel(app, std::slice::from_ref(r), k, json!(x), &resp);
                    }
                    finish_edit(app, std::slice::from_ref(r), k, &resp);
                }
                Value::Bool(mut b) => {
                    let changed = ui.allocate_ui_with_layout(egui::vec2(152.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.checkbox(&mut b, "").changed()
                    }).inner;
                    if changed {
                        app.s.checkpoint(&format!("setAttr {k}"));
                        app.s.set_attr(r, k, json!(b));
                    }
                }
                Value::String(mut s) => {
                    // JS file fields (`textureFile`, `fileTextureName`) get a browse button.
                    let file_field = matches!(k.as_str(), "textureFile" | "fileTextureName");
                    let edited = ui.allocate_ui_with_layout(egui::vec2(152.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let edited = ui.text_edit_singleline(&mut s).changed();
                        if file_field && ui.small_button("...").on_hover_text("Browse").clicked() {
                            let filters: &[&str] = if k == "textureFile" {
                                &["hdr", "exr", "png", "jpg", "jpeg"]
                            } else {
                                &["png", "jpg", "jpeg", "bmp", "tga", "tif", "tiff", "exr", "hdr", "webp", "gif"]
                            };
                            let mut dlg = rfd::FileDialog::new().add_filter("HDR / Images", filters);
                            if let Some(proj) = app.s.project.as_deref() {
                                // the JS file field starts in <project>/sourceimages
                                let src = std::path::Path::new(proj).join("sourceimages");
                                dlg = dlg.set_directory(if src.is_dir() { src } else { std::path::PathBuf::from(proj) });
                            }
                            if let Some(p) = dlg.pick_file() {
                                let mut v = p.to_string_lossy().replace('\\', "/");
                                if let Some(proj) = app.s.project.as_deref() {
                                    let pr = proj.replace('\\', "/");
                                    if let Some(rest) = v.strip_prefix(&(pr + "/")) {
                                        v = rest.to_string();
                                    }
                                }
                                app.s.checkpoint(&format!("set {k}"));
                                app.s.set_attr(r, k, json!(v.clone()));
                                s = v;
                            }
                        }
                        edited
                    }).inner;
                    if edited {
                        app.s.set_attr(r, k, json!(s));
                    }
                }
                other => {
                    ui.allocate_ui_with_layout(egui::vec2(152.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add_sized([152.0, 18.0], egui::Label::new(RichText::new(other.to_string()).color(theme::TEXT_DIM)).truncate());
                    });
                }
            }
            ui.end_row();
        }
    });
}

pub fn material_fields_editor(app: &mut IncaApp, ui: &mut egui::Ui, mid: &str, ty: &str) {
    let r = NodeRef::Material(mid.to_string());
    for (group, specs) in material_layout(ty) {
        theme::section_header(ui, group);
        field_grid(app, ui, &r, &specs);
    }
}

pub fn texture_fields_editor(app: &mut IncaApp, ui: &mut egui::Ui, id: &str, ty: &str) {
    let r = NodeRef::Texture(id.to_string());
    let Some(t) = app.s.texs.get(id).cloned() else { return };
    let place = t.place.clone();
    let mut specs = vec![];
    for k in t.attrs.keys() {
        let spec = match k.as_str() {
            "fileTextureName" => tspec("fileTextureName", "Image Name"),
            "colorSpace" => espec("colorSpace", "Color Space", &["sRGB", "Raw"]),
            "type" if ty == "ramp" => espec("type", "Type", &["V Ramp", "U Ramp", "Circular Ramp"]),
            "colorMode" if ty == "ocean" => espec("colorMode", "Color Mode", &["Wave Height", "Foam", "Wave Height and Foam"]),
            "color1" => cspec("color1", "Color 1"),
            "color2" => cspec("color2", "Color 2"),
            "lineColor" => cspec("lineColor", "Line Color"),
            "fillerColor" => cspec("fillerColor", "Filler Color"),
            "uColor" => cspec("uColor", "U Color"),
            "vColor" => cspec("vColor", "V Color"),
            "gapColor" => cspec("gapColor", "Gap Color"),
            "snowColor" => cspec("snowColor", "Snow Color"),
            "rockColor" => cspec("rockColor", "Rock Color"),
            "contrast" => fspec("contrast", "Contrast", 0.0, 2.0),
            "frequency" => fspec("frequency", "Frequency", 0.0, 32.0),
            "amplitude" => fspec("amplitude", "Amplitude", 0.0, 2.0),
            "uWidth" => fspec("uWidth", "U Width", 0.0, 1.0),
            "vWidth" => fspec("vWidth", "V Width", 0.0, 1.0),
            "uWave" => fspec("uWave", "U Wave", 0.0, 0.5),
            "vWave" => fspec("vWave", "V Wave", 0.0, 0.5),
            "randomness" => fspec("randomness", "Randomness", 0.0, 1.0),
            "widthSpread" => fspec("widthSpread", "Width Spread", 0.0, 1.0),
            "brightSpread" => fspec("brightSpread", "Bright Spread", 0.0, 1.0),
            "snowRoughness" => fspec("snowRoughness", "Snow Roughness", 0.0, 1.0),
            "rockRoughness" => fspec("rockRoughness", "Rock Roughness", 0.0, 1.0),
            "boundary" => fspec("boundary", "Boundary", 0.0, 1.0),
            "snowAltitude" => fspec("snowAltitude", "Snow Altitude", 0.0, 1.0),
            "snowDropoff" => fspec("snowDropoff", "Snow Dropoff", 0.0, 5.0),
            "snowSlope" => fspec("snowSlope", "Snow Slope", 0.0, 3.0),
            "depthMax" => fspec("depthMax", "Depth Max", 0.0, 40.0),
            "ratio" => fspec("ratio", "Ratio", 0.0, 1.0),
            "threshold" => fspec("threshold", "Threshold", 0.0, 1.0),
            "levelMax" => fspec("levelMax", "Level Max", 0.0, 12.0),
            "frequencyRatio" => fspec("frequencyRatio", "Frequency Ratio", 1.0, 4.0),
            "bias" => fspec("bias", "Bias", -1.0, 1.0),
            "waveAmplitude" => fspec("waveAmplitude", "Wave Amplitude", 0.0, 1.0),
            "waveFrequency" => fspec("waveFrequency", "Wave Frequency", 0.0, 20.0),
            "subWaveFrequency" => fspec("subWaveFrequency", "Sub Wave Frequency", 0.0, 2.0),
            "smoothness" => fspec("smoothness", "Smoothness", 0.0, 10.0),
            "numberOfWaves" => fspec("numberOfWaves", "Number of Waves", 0.0, 12.0),
            "windU" => fspec("windU", "Wind U", -1.0, 1.0),
            "windV" => fspec("windV", "Wind V", -1.0, 1.0),
            "waveTime" => fspec("waveTime", "Wave Time", -10.0, 10.0),
            "waveVelocity" => fspec("waveVelocity", "Wave Velocity", 0.0, 5.0),
            "numFrequencies" => fspec("numFrequencies", "Num Frequencies", 1.0, 12.0),
            "waveLengthMin" => fspec("waveLengthMin", "Wave Length Min", 0.01, 10.0),
            "waveLengthMax" => fspec("waveLengthMax", "Wave Length Max", 0.01, 20.0),
            "waveHeight" => fspec("waveHeight", "Wave Height", 0.0, 2.0),
            "waveTurbulence" => fspec("waveTurbulence", "Wave Turbulence", 0.0, 1.0),
            "wavePeaking" => fspec("wavePeaking", "Wave Peaking", 0.0, 1.0),
            "waveDirSpread" => fspec("waveDirSpread", "Wave Dir Spread", 0.0, 1.0),
            "scale" => fspec("scale", "Scale", 0.1, 50.0),
            "time" => fspec("time", "Time", -10.0, 10.0),
            "observerSpeed" => fspec("observerSpeed", "Observer Speed", 0.0, 2.0),
            "foamThreshold" => fspec("foamThreshold", "Foam Threshold", 0.0, 1.0),
            "foamEmission" => fspec("foamEmission", "Foam Emission", 0.0, 1.0),
            _ => continue,
        };
        specs.push(spec);
    }
    theme::section_header(ui, "Attributes");
    field_grid(app, ui, &r, &specs);
    if let Some(pid) = place {
        place_fields_editor(app, ui, &pid);
    }
    if ty == "ramp" {
        ramp_editor(app, ui, &r);
    }
}

/// place2dTexture controls, shown both from its texture and when the node is selected.
pub fn place_fields_editor(app: &mut IncaApp, ui: &mut egui::Ui, id: &str) {
    let placement = vec![
        fspec("coverageU", "Coverage U", 0.01, 1.0),
        fspec("coverageV", "Coverage V", 0.01, 1.0),
        fspec("translateFrameU", "Translate Frame U", -1.0, 1.0),
        fspec("translateFrameV", "Translate Frame V", -1.0, 1.0),
        fspec("rotateFrame", "Rotate Frame", 0.0, 360.0),
        bspec("mirrorU", "Mirror U"),
        bspec("mirrorV", "Mirror V"),
        espec("wrapU", "Wrap U", &["Repeat", "Clamp", "Mirror"]),
        espec("wrapV", "Wrap V", &["Repeat", "Clamp", "Mirror"]),
        fspec("repeatU", "Repeat U", 0.0, 16.0),
        fspec("repeatV", "Repeat V", 0.0, 16.0),
        fspec("offsetU", "Offset U", -1.0, 1.0),
        fspec("offsetV", "Offset V", -1.0, 1.0),
        fspec("rotateUV", "Rotate UV", 0.0, 360.0),
        bspec("stagger", "Stagger"),
        fspec("noiseU", "Noise U", 0.0, 1.0),
        fspec("noiseV", "Noise V", 0.0, 1.0),
    ];
    theme::section_header(ui, "2D Texture Placement");
    if let Some(p) = app.s.places.get(id) {
        ui.label(RichText::new(&p.name).color(theme::TEXT_DIM));
    }
    field_grid(app, ui, &NodeRef::Place2D(id.to_string()), &placement);
}

/// Attribute Editor tabs for a node: transform, shape, history nodes, material.
/// Rendering > Material Attributes: the Attribute Editor on the selection's material tab.
pub fn show_material_attributes(app: &mut IncaApp) {
    let Some(id) = app.s.lead().cloned() else {
        app.last_result = "// Warning: Select an object to see its material.".into();
        return;
    };
    match ae_tabs(app, &id).iter().position(|t| matches!(t.1, NodeRef::Material(_))) {
        Some(i) => {
            app.show_right = true;
            app.right_tab = RightTab::AttributeEditor;
            app.panels.ae_tab = i;
        }
        None => app.last_result = "// Warning: The selection has no material.".into(),
    }
}

fn ae_tabs(app: &IncaApp, id: &str) -> Vec<(String, NodeRef)> {
    let Some(n) = app.s.node(id) else { return vec![] };
    let mut tabs: Vec<(String, NodeRef)> = vec![(n.name.clone(), NodeRef::Dag(id.to_string()))];
    if let Some(sn) = &n.shape_name {
        tabs.push((sn.clone(), NodeRef::Dag(id.to_string())));
    } else if n.extra.as_object().is_some_and(|e| !e.is_empty()) {
        tabs.push((format!("{} attributes", n.kind), NodeRef::Dag(id.to_string())));
    }
    if let Some(m) = n.mesh() {
        for (i, h) in m.history.iter().enumerate().rev() {
            if !h.name.contains('#') {
                tabs.push((h.name.clone(), NodeRef::History(id.to_string(), i)));
            }
        }
        if app.s.mats.contains_key(&m.material) {
            tabs.push((app.s.mats[&m.material].name.clone(), NodeRef::Material(m.material.clone())));
        }
    }
    tabs
}

/// The node the Attribute Editor shows: the locked one, or the lead (hilited object in component mode).
fn ae_target(app: &mut IncaApp) -> Option<String> {
    if let Some(l) = &app.panels.ae_locked {
        if app.s.node(l).is_some() {
            return Some(l.clone());
        }
        app.panels.ae_locked = None;
    }
    if app.s.comp_mode.is_some() {
        if let Some(h) = app.s.hilite().last() {
            return Some(h.clone());
        }
    }
    app.s.lead().cloned()
}

/// Attribute Editor view for a material or texture picked in the Hypershade/Hypergraph.
fn shading_attribute_editor(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(id) = app.snode_sel.clone() else { return };
    let mut tabs: Vec<(String, NodeRef)> = vec![];
    if let Some(m) = app.s.mats.get(&id) {
        tabs.push((m.name.clone(), NodeRef::Material(id.clone())));
        let mut seen: Vec<String> = vec![];
        for tid in m.maps.values().filter_map(|v| v.as_str()).map(String::from) {
            if !seen.contains(&tid) {
                if let Some(t) = app.s.texs.get(&tid) {
                    tabs.push((t.name.clone(), NodeRef::Texture(tid.clone())));
                }
                seen.push(tid);
            }
        }
    } else if let Some(t) = app.s.texs.get(&id) {
        tabs.push((t.name.clone(), NodeRef::Texture(id.clone())));
    } else {
        app.snode_sel = None;
        return;
    }
    let mut tab = app.panels.ae_tab.min(tabs.len().saturating_sub(1));
    ui.horizontal_wrapped(|ui| {
        for (i, (t, _)) in tabs.iter().enumerate() {
            if ui.selectable_label(tab == i, t).clicked() {
                tab = i;
            }
        }
    });
    app.panels.ae_tab = tab;
    ui.horizontal(|ui| {
        ui.label(RichText::new("node:").color(theme::TEXT_DIM));
        ui.label(RichText::new(&tabs[tab].0).strong());
    });
    let h = ui.available_height() - 26.0;
    egui::ScrollArea::vertical().id_salt("aescroll_snode").max_height(h.max(40.0)).auto_shrink([false, false]).show(ui, |ui| ae_body(app, ui, &tabs, tab));
}

fn ae_preset_kind(app: &IncaApp, node: &NodeRef) -> Option<String> {
    match node {
        NodeRef::Dag(id) => app.s.node(id).map(|n| n.kind.clone()),
        _ => None,
    }
}

fn ae_capture_preset(app: &IncaApp, node: &NodeRef) -> Vec<(String, Value)> {
    let NodeRef::Dag(id) = node else { return vec![] };
    let mut attrs: Vec<String> = CHANNELS.iter().map(|c| c.to_string()).collect();
    if let Some(n) = app.s.node(id) {
        if let Some(e) = n.extra.as_object() {
            attrs.extend(e.iter().filter(|(_, v)| v.is_number() || v.is_boolean()).map(|(k, _)| k.clone()));
        }
    }
    attrs.extend(app.s.shape_channels(id));
    attrs.sort();
    attrs.dedup();
    attrs
        .into_iter()
        .filter(|attr| !app.s.conns.iter().any(|c| c.dst == *id && c.dst_attr == *attr))
        .filter_map(|attr| app.s.get_attr(node, &attr).map(|v| (attr, v)))
        .collect()
}

fn ae_blend_value(cur: &Value, new: &Value, f: f64) -> Value {
    match (cur, new) {
        (Value::Number(a), Value::Number(b)) => {
            match (a.as_f64(), b.as_f64()) {
                (Some(a), Some(b)) => json!(a + (b - a) * f),
                _ => new.clone(),
            }
        }
        (Value::Bool(a), Value::Bool(b)) => json!(if f >= 0.5 { *b } else { *a }),
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            json!(a.iter().zip(b).map(|(a, b)| ae_blend_value(a, b, f)).collect::<Vec<_>>())
        }
        _ => new.clone(),
    }
}

fn ae_apply_preset(app: &mut IncaApp, node: &NodeRef, preset: &crate::prefs::AttributePreset, blend: Option<f64>) {
    let NodeRef::Dag(_) = node else { return };
    app.s.checkpoint("apply attribute preset");
    for (attr, v) in &preset.values {
        let value = match blend {
            Some(f) => app.s.get_attr(node, attr).map(|cur| ae_blend_value(&cur, v, f)).unwrap_or_else(|| v.clone()),
            None => v.clone(),
        };
        app.s.set_attr(node, attr, value);
    }
    app.s.touch();
    app.help = format!("Applied preset \"{}\"", preset.name);
}

fn ae_save_preset(app: &mut IncaApp, node: &NodeRef, kind: &str, name: &str) {
    let values = ae_capture_preset(app, node);
    if values.is_empty() {
        app.help = "No preset-able attributes on this node".into();
        return;
    }
    let name = if name.trim().is_empty() { kind.to_string() } else { name.trim().to_string() };
    if let Some(p) = app.prefs.attr_presets.iter_mut().find(|p| p.kind == kind && p.name == name) {
        p.values = values;
    } else {
        app.prefs.attr_presets.push(crate::prefs::AttributePreset { name: name.clone(), kind: kind.to_string(), values });
    }
    app.help = format!("Saved {kind} preset \"{name}\"");
}

pub fn attribute_editor(app: &mut IncaApp, ui: &mut egui::Ui) {
    use crate::menus::{menu_check, menu_item, panel_bar, MenuSource};
    let target = ae_target(app);
    let titles = ["List", "Selected", "Focus", "Attributes", "Display", "Show", "Help"];
    panel_bar(app, ui, MenuSource::AttrEditor, &titles, false, |app, ui, title| match title {
        "List" => {
            let mut auto = app.panels.ae_locked.is_none();
            if menu_check(app, ui, auto, "Auto Load Selected Attributes") {
                auto = !auto;
                app.panels.ae_locked = if auto { None } else { target.clone() };
            }
            if menu_item(app, ui, "Load Selected Attributes") {
                app.panels.ae_locked = None;
                app.panels.ae_layer = None;
            }
        }
        "Selected" => {
            if menu_item(app, ui, "Select Node") {
                if let Some(t) = &target {
                    app.select_names(std::slice::from_ref(t), SelMode::Replace);
                }
            }
        }
        "Focus" => {
            for id in app.s.sel.clone() {
                let name = app.s.name_of(&id);
                if menu_item(app, ui, &name) {
                    app.panels.ae_locked = Some(id);
                }
            }
        }
        "Attributes" => {
            ui.add_enabled(false, egui::Button::new("Add Attribute..."));
        }
        "Display" => {
            let mut mode = app.panels.ae_names;
            if menu_check(app, ui, mode == 0, "Nice Attribute Names") {
                mode = 0;
            }
            if menu_check(app, ui, mode == 1, "Long Attribute Names") {
                mode = 1;
            }
            if menu_check(app, ui, mode == 2, "Short Attribute Names") {
                mode = 2;
            }
            app.panels.ae_names = mode;
        }
        "Show" => {
            if menu_item(app, ui, "Show Notes") {
                app.help = "Notes are under Extra Attributes on the transform tab".into();
            }
        }
        _ => {
            if menu_item(app, ui, "Help on Attribute Editor") {
                app.help = "The Attribute Editor shows every attribute of the selected node; tabs list connected nodes (shape, history, material).".into();
            }
        }
    });
    if let Some(lid) = app.panels.ae_layer.clone() {
        if !app.s.layers.iter().any(|l| l.id == lid) || app.s.sel != app.panels.ae_layer_sel {
            app.panels.ae_layer = None;
        } else {
            layer_attribute_editor(app, ui, &lid);
            return;
        }
    }
    if app.snode_sel.is_some() {
        shading_attribute_editor(app, ui);
        return;
    }
    let Some(id) = target else {
        ui.label(RichText::new("Select an object to see its attributes.").color(theme::TEXT_DIM));
        return;
    };
    let tabs = ae_tabs(app, &id);
    if tabs.is_empty() {
        return;
    }
    let mut tab = app.panels.ae_tab.min(tabs.len() - 1);
    ui.horizontal(|ui| {
        let scroll_w = (ui.available_width() - 46.0).max(80.0);
        let out = egui::ScrollArea::horizontal()
            .id_salt("aetabs")
            .max_width(scroll_w)
            .scroll_bar_visibility(egui::containers::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .horizontal_scroll_offset(app.panels.ae_tab_scroll)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, (t, _)) in tabs.iter().enumerate() {
                        if ui.selectable_label(tab == i, t).clicked() {
                            tab = i;
                        }
                    }
                });
            });
        app.panels.ae_tab_scroll = out.state.offset.x;
        if ui.small_button("◀").on_hover_text("Scroll tabs left").clicked() {
            app.panels.ae_tab_scroll = (app.panels.ae_tab_scroll - 80.0).max(0.0);
        }
        if ui.small_button("▶").on_hover_text("Scroll tabs right").clicked() {
            app.panels.ae_tab_scroll += 80.0;
        }
    });
    app.panels.ae_tab = tab;
    // node row: transform/shape label, Focus, Presets, lock; then Show/Hide
    ui.horizontal(|ui| {
        let kind = match &tabs[tab].1 {
            NodeRef::Dag(_) if tab == 0 => "transform:",
            NodeRef::Dag(_) => "shape:",
            NodeRef::History(..) => "history:",
            NodeRef::Material(_) => "material:",
            NodeRef::Texture(_) => "texture:",
            NodeRef::Place2D(_) => "placement:",
        };
        ui.label(RichText::new(kind).color(theme::TEXT_DIM));
        let title = tabs[tab].0.clone();
        if app.panels.ae_name.0 != title {
            app.panels.ae_name = (title.clone(), title.clone());
        }
        let w = (ui.available_width() - 130.0).max(40.0);
        let te = ui.add(egui::TextEdit::singleline(&mut app.panels.ae_name.1).desired_width(w));
        if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let new = app.panels.ae_name.1.trim().to_string();
            if !new.is_empty() && new != title {
                let _ = app.run_mel("rename", &format!("rename {title} \"{new}\";"));
            }
        }
        if ui.button("Focus").on_hover_text("Show the selected node").clicked() {
            app.panels.ae_locked = None;
        }
        let preset_node = tabs[tab].1.clone();
        let preset_kind = ae_preset_kind(app, &preset_node);
        ui.add_enabled_ui(preset_kind.is_some(), |ui| {
            let button = ui.menu_button("Presets", |ui| {
                let Some(kind) = preset_kind.clone() else { return };
                ui.menu_button(format!("Save {kind} Preset..."), |ui| {
                    ui.label("Preset name");
                    ui.text_edit_singleline(&mut app.panels.ae_preset_name);
                    if ui.button("Save Attribute Preset").clicked() {
                        let name = app.panels.ae_preset_name.clone();
                        ae_save_preset(app, &preset_node, &kind, &name);
                        ui.close();
                    }
                });
                ui.separator();
                let presets: Vec<crate::prefs::AttributePreset> = app.prefs.attr_presets.iter().filter(|p| p.kind == kind).cloned().collect();
                if presets.is_empty() {
                    ui.label(RichText::new("No presets").color(theme::TEXT_DIM));
                } else {
                    for p in presets {
                        ui.menu_button(p.name.clone(), |ui| {
                            if ui.button("Replace").clicked() {
                                ae_apply_preset(app, &preset_node, &p, None);
                                ui.close();
                            }
                            if ui.button("Blend 50%").clicked() {
                                ae_apply_preset(app, &preset_node, &p, Some(0.5));
                                ui.close();
                            }
                        });
                    }
                }
            });
            if preset_kind.is_none() {
                button.response.on_disabled_hover_text("Presets are available for scene nodes");
            }
        });
        let locked = app.panels.ae_locked.is_some();
        if lock_button(ui, locked).on_hover_text("Lock the Attribute Editor to the current node").clicked() {
            app.panels.ae_locked = if locked { None } else { Some(id.clone()) };
        }
    });
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Hide").on_hover_text("Hide the selected object(s)").clicked() {
                app.action("hideSelection");
            }
            if ui.button("Show").on_hover_text("Show the selected object(s)").clicked() {
                app.action("showSelection");
            }
        });
    });
    let h = ui.available_height() - 26.0;
    egui::ScrollArea::vertical().id_salt("aescroll").max_height(h.max(40.0)).auto_shrink([false, false]).show(ui, |ui| ae_body(app, ui, &tabs, tab));
    ui.separator();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.spacing_mut().button_padding.x = 2.0;
        // Load Attributes gets the extra room
        let w = (ui.available_width() - 9.0) / 4.4;
        let b = |ui: &mut egui::Ui, t: &str, w: f32| ui.add_sized([w, 20.0], egui::Button::new(t)).clicked();
        if b(ui, "Select", w) {
            app.select_names(std::slice::from_ref(&id), SelMode::Replace);
        }
        if b(ui, "Load Attributes", w * 1.4) {
            app.panels.ae_locked = None;
            app.panels.ae_layer = None;
        }
        if b(ui, "Copy Tab", w) {
            app.panels.ae_copies.push((id.clone(), tab));
        }
        if b(ui, "Close", w) {
            app.show_right = false;
        }
    });
}

/// A painted padlock toggle (the icon set has none).
fn lock_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    let bg = if on { Color32::from_rgb(0x5d, 0x7a, 0x8f) } else if resp.hovered() { Color32::from_gray(0x6f) } else { theme::BTN };
    ui.painter().rect_filled(r, 3.0, bg);
    let c = r.center();
    let body = egui::Rect::from_center_size(c + egui::vec2(0.0, 2.5), egui::vec2(10.0, 7.0));
    let col = Color32::from_gray(0xe8);
    ui.painter().rect_filled(body, 1.0, col);
    // the shackle: open when unlocked
    let lift = if on { 0.0 } else { 2.0 };
    let pts: Vec<egui::Pos2> = (0..=12).map(|i| {
        let a = std::f32::consts::PI * i as f32 / 12.0;
        egui::pos2(c.x - 3.0 * a.cos(), body.min.y - lift - 4.0 * a.sin())
    }).collect();
    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.5, col)));
    resp
}

/// Copy Tab: floating Attribute Editor windows pinned to a node and tab.
pub fn ae_copies(app: &mut IncaApp, ctx: &egui::Context) {
    let copies = std::mem::take(&mut app.panels.ae_copies);
    let mut keep = vec![];
    for (i, (id, tab)) in copies.into_iter().enumerate() {
        let tabs = ae_tabs(app, &id);
        let Some((title, _)) = tabs.get(tab).cloned() else { continue };
        let mut open = true;
        let wid = egui::Id::new(("aecopy", i, &id, tab));
        let _ = crate::windows::place_window(app, ctx, wid, open, |app, fix| {
            let mut window = egui::Window::new(format!("Attribute Editor: {title}")).id(wid).open(&mut open).default_size([380.0, 520.0]).min_width(crate::app::RIGHT_PANEL_MIN_W_AE).constrain(false);
            if let Some(p) = fix {
                window = window.fixed_pos(p);
            }
            window.show(ctx, |ui| {
                ui.push_id(("aecopy", i), |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ae_body(app, ui, &tabs, tab));
                });
            })
        });
        if open {
            keep.push((id, tab));
        }
    }
    keep.extend(std::mem::take(&mut app.panels.ae_copies));
    app.panels.ae_copies = keep;
}

/// The channels of one history node (Attribute Editor tab, In-View Editor).
pub fn history_channels(app: &mut IncaApp, ui: &mut egui::Ui, id: &str, i: usize) {
    let r = NodeRef::History(id.to_string(), i);
    let ty = app.s.node(id).and_then(|n| n.mesh()).and_then(|m| m.history.get(i)).map(|h| h.ty.clone()).unwrap_or_default();
    if let Some((_, _, chans)) = ops::info(&ty) {
        for c in chans {
            let label = c.label.map(String::from).unwrap_or_else(|| ops::nice_name(c.key));
            channel_row(app, ui, std::slice::from_ref(&r), c.key, &label, c.kind, c.min, c.max);
        }
    }
}

fn ae_body(app: &mut IncaApp, ui: &mut egui::Ui, tabs: &[(String, NodeRef)], tab: usize) {
    let (_, r) = tabs[tab].clone();
    let mut cb_rows: Vec<CbRow> = Vec::new();
    match &r {
        NodeRef::Dag(id) if tab == 0 => {
            let targets = vec![r.clone()];
            let published: Vec<(String, ChanKind)> = app.s.node(id).map(|n| {
                n.published
                    .keys()
                    .filter_map(|name| match app.s.get_attr(&r, name) {
                        Some(Value::Bool(_)) => Some((name.clone(), ChanKind::Bool)),
                        Some(Value::Number(x)) if x.is_f64() => Some((name.clone(), ChanKind::Float)),
                        Some(Value::Number(_)) => Some((name.clone(), ChanKind::Int)),
                        _ => None,
                    })
                    .collect()
            }).unwrap_or_default();
            if !published.is_empty() && theme::fold_ae_closed(ui, "Published Attributes") {
                for (k, kind) in published {
                    channel_row_cb(app, ui, &mut cb_rows, &targets, &k, &k, kind, None, None);
                }
            }
            if theme::fold_ae(ui, "Transform Attributes") {
                channel_vec_row(app, ui, &mut cb_rows, &targets, ["translateX", "translateY", "translateZ"], "Translate", Some(3));
                channel_vec_row(app, ui, &mut cb_rows, &targets, ["rotateX", "rotateY", "rotateZ"], "Rotate", Some(3));
                channel_vec_row(app, ui, &mut cb_rows, &targets, ["scaleX", "scaleY", "scaleZ"], "Scale", Some(3));
                channel_vec_row(app, ui, &mut cb_rows, &targets, ["shearXY", "shearXZ", "shearYZ"], "Shear", Some(3));
                channel_row_cb(app, ui, &mut cb_rows, &targets, "rotateOrder", "Rotate Order", ChanKind::Enum(&inca_core::math::ROTATE_ORDERS), None, None);
                channel_vec_row(app, ui, &mut cb_rows, &targets, ["rotateAxisX", "rotateAxisY", "rotateAxisZ"], "Rotate Axis", Some(3));
                ui.horizontal(|ui| {
                    ui.add_space(90.0 + ui.spacing().item_spacing.x);
                    let mut inherits = app.s.get_attr(&r, "inheritsTransform").and_then(|v| v.as_bool().or_else(|| v.as_f64().map(|x| x != 0.0))).unwrap_or(true);
                    if ui.checkbox(&mut inherits, "Inherits Transform").changed() {
                        app.s.checkpoint("inheritsTransform");
                        app.s.set_attr(&r, "inheritsTransform", json!(inherits));
                    }
                });
            }
            if theme::fold_ae_closed(ui, "Transform Offset Parent Matrix") {
                let mut opm = app.s.node(id).map(|n| n.xf.opm).unwrap_or(IDENTITY_OPM);
                let mut changed = false;
                egui::Grid::new(("opm", id)).num_columns(4).spacing([3.0, 3.0]).show(ui, |ui| {
                    for row in 0..4 {
                        for col in 0..4 {
                            changed |= ui.add_sized([56.0, 18.0], egui::DragValue::new(&mut opm[col][row]).speed(0.01).max_decimals(4)).changed();
                        }
                        ui.end_row();
                    }
                });
                if changed {
                    if !app.panels.ae_opm_editing {
                        app.s.checkpoint("offset parent matrix");
                        app.panels.ae_opm_editing = true;
                    }
                    if let Some(n) = app.s.nodes.get_mut(id) {
                        n.xf.opm = opm;
                    }
                    app.s.touch();
                }
                if app.panels.ae_opm_editing && !ui.input(|i| i.pointer.primary_down()) {
                    app.panels.ae_opm_editing = false;
                }
                ui.horizontal(|ui| {
                    if ui.small_button("Identity").clicked() {
                        app.s.checkpoint("offset parent matrix");
                        if let Some(n) = app.s.nodes.get_mut(id) {
                            n.xf.opm = IDENTITY_OPM;
                        }
                        app.s.touch();
                    }
                    if ui.small_button("Invert").clicked() {
                        if let Some(m) = app.s.node(id).map(|n| inca_core::math::DMat4::from_cols_array_2d(&n.xf.opm).inverse().to_cols_array_2d()) {
                            app.s.checkpoint("offset parent matrix");
                            if let Some(n) = app.s.nodes.get_mut(id) {
                                n.xf.opm = m;
                            }
                            app.s.touch();
                        }
                    }
                });
            }
            if theme::fold_ae_closed(ui, "Pivots") {
                let p = app.s.world_pivot(id);
                ui.label(format!("World pivot  {:.3} {:.3} {:.3}", p.x, p.y, p.z));
                if ui.button("Center Pivot").clicked() {
                    let _ = app.run_mel("center pivot", &format!("xform -cp {};", app.s.name_of(id)));
                }
            }
            if theme::fold_ae_closed(ui, "Limit Information") {
                let mut lim = app.s.node(id).map(|n| n.xf.lim.clone()).unwrap_or_default();
                let mut changed = false;
                limit_row(ui, "Translate", &mut lim.translate, &mut changed);
                limit_row(ui, "Rotate", &mut lim.rotate, &mut changed);
                limit_row(ui, "Scale", &mut lim.scale, &mut changed);
                if changed {
                    if !app.panels.ae_limit_editing {
                        app.s.checkpoint("limits");
                        app.panels.ae_limit_editing = true;
                    }
                    if let Some(n) = app.s.nodes.get_mut(id) {
                        n.xf.lim = lim;
                    }
                    app.s.touch();
                }
                if app.panels.ae_limit_editing && !ui.input(|i| i.pointer.primary_down()) {
                    app.panels.ae_limit_editing = false;
                }
            }
            if theme::fold_ae_closed(ui, "Display") {
                channel_row_cb(app, ui, &mut cb_rows, &targets, "visibility", "Visibility", ChanKind::Bool, None, None);
                channel_row_cb(app, ui, &mut cb_rows, &targets, "template", "Template", ChanKind::Bool, None, None);
                let mut use_outliner_color = app.s.node(id).map(|n| n.use_outliner_color).unwrap_or(false);
                if ui.checkbox(&mut use_outliner_color, "Use Outliner Color").changed() {
                    app.s.checkpoint("outliner color");
                    if let Some(n) = app.s.nodes.get_mut(id) {
                        n.use_outliner_color = use_outliner_color;
                    }
                    app.s.touch();
                }
                let mut outliner_color = app.s.node(id).map(|n| n.outliner_color).unwrap_or([1.0; 3]);
                ui.horizontal(|ui| {
                    ui.label("Outliner Color");
                    let changed = color_edit(ui, egui::Id::new(("outliner_color", id)), &mut outliner_color);
                    if changed {
                        if !app.panels.outliner_color_editing {
                            app.s.checkpoint("outliner color");
                            app.panels.outliner_color_editing = true;
                        }
                        if let Some(n) = app.s.nodes.get_mut(id) {
                            n.outliner_color = outliner_color;
                        }
                        app.s.touch();
                    }
                    if app.panels.outliner_color_editing && !ui.input(|i| i.pointer.primary_down()) {
                        app.panels.outliner_color_editing = false;
                    }
                });
                let layer = app.s.node(id).and_then(|n| n.layer.clone()).and_then(|l| app.s.layers.iter().find(|x| x.id == l).map(|x| x.name.clone())).unwrap_or_else(|| "defaultLayer".into());
                ui.horizontal(|ui| {
                    ui.label("Display Layer");
                    ui.label(RichText::new(layer).color(theme::TEXT_DIM));
                });
            }
            if theme::fold_ae_closed(ui, "Node Behavior") {
                let mut on = app.s.node(id).map(|n| n.selectable).unwrap_or(true);
                if ui.checkbox(&mut on, "Selectable").changed() {
                    app.s.checkpoint("selectable");
                    if let Some(n) = app.s.nodes.get_mut(id) {
                        n.selectable = on;
                    }
                }
            }
            if theme::fold_ae_closed(ui, "UUID") {
                ui.label(RichText::new(id).color(theme::TEXT_DIM));
            }
            if theme::fold_ae_closed(ui, "Extra Attributes") {
                let keys: Vec<String> = app.s.node(id).and_then(|n| n.extra.as_object()).map(|e| e.keys().filter(|k| k.as_str() != "notes").cloned().collect()).unwrap_or_default();
                if keys.is_empty() {
                    ui.label(RichText::new("No extra attributes").color(theme::TEXT_DIM));
                } else {
                    for k in keys {
                        ui.label(k);
                    }
                }
            }
            ui.separator();
            ui.label(RichText::new("Notes:").color(theme::TEXT_DIM));
            let mut notes = app.s.node(id).and_then(|n| n.extra.get("notes")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if ui.add(egui::TextEdit::multiline(&mut notes).desired_rows(3).desired_width(f32::INFINITY)).changed() {
                if let Some(n) = app.s.nodes.get_mut(id) {
                    if !n.extra.is_object() {
                        n.extra = serde_json::json!({});
                    }
                    n.extra.as_object_mut().unwrap().insert("notes".into(), serde_json::json!(notes));
                    app.s.modified = true;
                }
            }
        }
        NodeRef::Dag(id) => shape_tab(app, ui, id),
        NodeRef::History(id, i) => {
            let ty = app.s.node(id).and_then(|n| n.mesh()).and_then(|m| m.history.get(*i)).map(|h| h.ty.clone()).unwrap_or_default();
            if theme::fold(ui, &format!("{ty} History")) {
                if let Some((_, _, chans)) = ops::info(&ty) {
                    for c in chans {
                        let label = c.label.map(String::from).unwrap_or_else(|| ops::nice_name(c.key));
                        channel_row_cb(app, ui, &mut cb_rows, std::slice::from_ref(&r), c.key, &label, c.kind, c.min, c.max);
                    }
                }
            }
        }
        NodeRef::Place2D(pid) => place_fields_editor(app, ui, pid),
        NodeRef::Material(mid) => crate::windows::material_editor(app, ui, mid, true),
        NodeRef::Texture(tid) => {
            let ty = app.s.texs.get(tid).map(|t| t.ty.clone()).unwrap_or_default();
            texture_fields_editor(app, ui, tid, &ty);
            if ty == "file" && ui.button("Browse image...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "hdr"]).pick_file() {
                    app.s.set_attr(&NodeRef::Texture(tid.clone()), "fileTextureName", json!(p.to_string_lossy()));
                }
            }
        }
    }
    finish_cb_rows(app, ui, &cb_rows);
}

fn shape_tab(app: &mut IncaApp, ui: &mut egui::Ui, id: &str) {
    let Some(n) = app.s.node(id) else { return };
    let r = NodeRef::Dag(id.to_string());
    let (kind, extra) = (n.kind.clone(), n.extra.clone());
    if let Some(e) = extra.as_object() {
        let keys: Vec<String> = e.keys().filter(|k| !matches!(k.as_str(), "source" | "affects" | "initial" | "notes")).cloned().collect();
        if !keys.is_empty() {
            let title = match kind.as_str() {
                "emitter" => "Emitter Attributes",
                "field" => "Field Attributes",
                "deformer" => "Deformer Handle",
                _ if e.contains_key("rigidBody") => "Rigid Body Attributes",
                _ => "Extra Attributes",
            };
            if theme::fold(ui, title) {
                attr_map_editor(app, ui, &r, &keys);
            }
        }
    }
    if kind == "field" && theme::fold(ui, "Connections") {
        // per-field connections: an empty list means the field affects everything
        let aff: Vec<String> = extra.get("affects").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        if aff.is_empty() {
            ui.label(RichText::new("Affects all particles and rigid bodies").color(theme::TEXT_DIM));
        }
        let fname = app.s.name_of(id);
        for o in aff {
            ui.horizontal(|ui| {
                ui.label(app.s.name_of(&o));
                if ui.small_button("Disconnect").clicked() {
                    let _ = app.run_mel("disconnect field", &format!("connectDynamic -d -f {fname} {};", app.s.name_of(&o)));
                }
            });
        }
        ui.label(RichText::new("Select objects and the field, then Fields > Affect Selected Object(s)").color(theme::TEXT_DIM));
    }
    let Some(n) = app.s.node(id) else { return };
    let xray = n.xray;
    match &n.shape {
        Shape::Mesh(m) => {
            let (v, f, t) = (m.mesh.v.len(), m.mesh.f.len(), m.mesh.tri_count());
            let uvs = m.mesh.uv.as_ref().map(|u| u.iter().flatten().map(|x| x.len()).sum::<usize>()).unwrap_or(0);
            let mat = m.material.clone();
            if theme::fold(ui, "Mesh Info") {
                ui.label(format!("{v} vertices, {f} faces, {t} triangles, {uvs} UVs"));
            }
            if theme::fold(ui, "Smooth Mesh") {
                channel_row(app, ui, std::slice::from_ref(&r), "smoothLevel", "Display", ChanKind::Int, Some(0.0), Some(3.0));
                // JS `smoothDivisions`: the assignment is global and cached previews are rebuilt
                ui.horizontal(|ui| {
                    ui.label("Preview Division Levels");
                    let mut d = app.prefs.smooth_divisions.clamp(1, 4);
                    if ui.add(egui::DragValue::new(&mut d).range(1..=4)).changed() {
                        app.prefs.smooth_divisions = d;
                        app.smooth_cache.clear();
                    }
                });
            }
            if theme::fold(ui, "Normals Display") {
                channel_row(app, ui, std::slice::from_ref(&r), "softAngle", "Soft Edge Angle", ChanKind::Float, Some(0.0), Some(180.0));
            }
            if theme::fold(ui, "Render Stats") {
                // stored like the JS `inc.extra` flags; meshes cast and receive by default
                for (key, label) in [("castShadows", "Casts Shadows"), ("receiveShadows", "Receive Shadows")] {
                    let mut on = extra.get(key).and_then(|v| v.as_bool()).unwrap_or(true);
                    if ui.checkbox(&mut on, label).changed() {
                        app.s.checkpoint("render stats");
                        if let Some(n) = app.s.nodes.get_mut(id) {
                            if !n.extra.is_object() {
                                n.extra = serde_json::json!({});
                            }
                            n.extra.as_object_mut().unwrap().insert(key.into(), serde_json::json!(on));
                        }
                    }
                }
                let mut on = xray;
                if ui.checkbox(&mut on, "X-Ray (object)").changed() {
                    app.s.checkpoint("object x-ray");
                    if let Some(n) = app.s.nodes.get_mut(id) {
                        n.xray = on;
                    }
                }
            }
            if !theme::fold(ui, "Shading") {
                return;
            }
            let mut sel = mat.clone();
            egui::ComboBox::from_id_salt("aemat").selected_text(app.s.mats.get(&mat).map(|m| m.name.clone()).unwrap_or_default()).show_ui(ui, |ui| {
                for m in app.s.mats.values() {
                    ui.selectable_value(&mut sel, m.id.clone(), &m.name);
                }
            });
            if sel != mat {
                let mn = app.s.mats[&sel].name.clone();
                let _ = app.run_mel("assign material", &format!("select -r {}; hyperShade -assign {mn};", app.s.name_of(id)));
            }
        }
        Shape::Light(l) => {
            // shadows live in their own section like the JS Attribute Editor
            let ty = l.ty.clone();
            let keys: Vec<String> = l.attrs.keys().filter(|k| !matches!(k.as_str(), "shadows" | "shadowRadius")).cloned().collect();
            let mut shadows = l.attrs.get("shadows").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut radius = l.attrs.get("shadowRadius").and_then(|v| v.as_f64()).unwrap_or(2.0) as f32;
            if theme::fold(ui, &format!("{} Attributes", crate::actions::label(&ty))) {
                attr_map_editor(app, ui, &r, &keys);
            }
            if ty != "ambientLight" && ty != "skyDomeLight" && theme::fold(ui, "Shadows") {
                if ui.checkbox(&mut shadows, "Use Depth Map / Ray Trace Shadows").changed() {
                    app.s.checkpoint("light shadows");
                    app.s.set_attr(&r, "shadows", json!(shadows));
                }
                ui.horizontal(|ui| {
                    ui.label("Shadow Softness");
                    if ui.add(egui::DragValue::new(&mut radius).range(0.0..=10.0).speed(0.01)).changed() {
                        app.s.set_attr(&r, "shadowRadius", json!(radius));
                    }
                });
                ui.label(RichText::new("Tip: press 7 in a viewport to see lights; turn on Lighting > Shadows for shadows.").color(theme::TEXT_DIM));
            }
        }
        Shape::Camera(_) => {
            if theme::fold(ui, "Camera Attributes") {
                let keys: Vec<String> = ["focalLength", "near", "far", "ortho", "orthoWidth", "coi", "horizontalFilmAperture", "verticalFilmAperture"].iter().map(|s| s.to_string()).collect();
                attr_map_editor(app, ui, &r, &keys);
            }
        }
        Shape::Curve(c) => {
            if theme::fold(ui, "NURBS Curve") {
                ui.label(format!("degree {}, {} CVs, {} spans, {}", c.degree, c.cvs.len(), c.spans(), c.form));
            }
        }
        _ => {}
    }
}

fn tool_settings(app: &mut IncaApp, ui: &mut egui::Ui) {
    theme::section_header(ui, match app.tool {
        Tool::Select => "Select Tool",
        Tool::Lasso => "Lasso Tool",
        Tool::Paint => "Paint Selection Tool",
        Tool::Move => "Move Tool",
        Tool::Rotate => "Rotate Tool",
        Tool::Scale => "Scale Tool",
        Tool::CvCurve => "CV Curve Tool",
        Tool::EpCurve => "EP Curve Tool",
        Tool::Arc => "Three Point Circular Arc",
        Tool::Polygon => "Create Polygon Tool",
        Tool::Joint => "Joint Tool",
        Tool::EdgeLoop => "Insert Edge Loop Tool",
        Tool::MultiCut => "Multi-Cut Tool",
        Tool::TargetWeld => "Target Weld Tool",
        Tool::Sculpt => "Sculpt Tool",
        Tool::QuadDraw => "Quad Draw Tool",
        Tool::Pencil => "Pencil Curve Tool",
        Tool::IkHandle => "IK Handle Tool",
        Tool::Distance => "Distance Tool",
        Tool::LatticePoint => "Lattice Point Tool",
        Tool::PaintWeights => "Paint Skin Weights Tool",
    });
    crate::mesh_tools::settings(app, ui);
    crate::rig_tools::settings(app, ui);
    if app.tool.is_creation() {
        ui.label("Click in a viewport to place points (on the ground grid in perspective, on the view plane in orthographic views). Enter completes, Backspace removes the last point, Esc cancels.");
        ui.checkbox(&mut app.snap_grid, "Snap to grid");
    }
    if app.tool.is_select() {
        ui.checkbox(&mut app.prefs.preselect, "Pre-selection highlight");
    }
    if app.tool == Tool::Paint {
        ui.horizontal(|ui| {
            ui.label("Brush radius");
            ui.add(egui::Slider::new(&mut app.paint_radius, 2.0..=200.0));
        });
    }
    if app.tool.is_manip() {
        ui.horizontal(|ui| {
            ui.label("Axis Orientation");
            let mut world = app.space_world(app.tool);
            ui.radio_value(&mut world, true, "World");
            ui.radio_value(&mut world, false, "Object");
            if world != app.space_world(app.tool) {
                app.set_space_world(app.tool, world);
            }
        });
        // JS Tool Settings: each tool has its own discrete toggle and step size
        let mi = crate::vp_ui::mode_index(app.tool);
        let label = match app.tool {
            Tool::Rotate => "Discrete rotate",
            Tool::Scale => "Discrete scale",
            _ => "Discrete move",
        };
        ui.checkbox(&mut app.vp_state.discrete[mi], label);
        ui.horizontal(|ui| {
            ui.label(if app.tool == Tool::Rotate { "Step size (deg)" } else { "Step size" });
            let (var, speed, max): (&mut f64, f64, f64) = match app.tool {
                Tool::Rotate => (&mut app.vp_state.snap_angle, 0.5, 90.0),
                Tool::Scale => (&mut app.vp_state.snap_scale, 0.01, 5.0),
                _ => (&mut app.vp_state.snap_step, 0.05, 5.0),
            };
            ui.add(egui::DragValue::new(var).speed(speed).range(0.001..=max));
        });
        ui.horizontal(|ui| {
            ui.label("Manipulator size");
            ui.add(egui::Slider::new(&mut app.manip_size, 0.2..=5.0));
        });
    }
    if app.tool.is_manip() && theme::fold(ui, "Soft Selection") {
        ui.checkbox(&mut app.soft.on, "Soft Select (B)");
        ui.horizontal(|ui| {
            ui.label("Falloff radius");
            ui.add(egui::DragValue::new(&mut app.soft.radius).speed(0.02).range(0.001..=1000.0));
        });
        ui.horizontal(|ui| {
            ui.label("Falloff curve");
            ui.radio_value(&mut app.soft.linear, false, "Smooth");
            ui.radio_value(&mut app.soft.linear, true, "Linear");
        });
        ui.label(RichText::new("B + drag in a viewport changes the radius").color(theme::TEXT_DIM));
    }
    ui.separator();
    ui.checkbox(&mut app.history_on, "Construction history");
}

#[cfg(test)]
mod tests {
    use super::*;
    use inca_core::params::Params;
    use inca_core::scene::SelMode;

    fn raw(events: Vec<egui::Event>, t: f64) -> egui::RawInput {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
        raw.time = Some(t);
        raw.events = events;
        raw
    }

    #[test]
    fn channel_box_drag_selects_names_and_fields() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let rows = app.panels.cb_row_rects.clone();
        let field = |attr: &str| rows.iter().find(|(_, a, _, _)| a == attr).map(|(_, _, _, f)| *f).expect("field rect");
        let name = |attr: &str| rows.iter().find(|(_, a, _, _)| a == attr).map(|(_, _, r, _)| *r).expect("name rect");
        let (fx, fy, fz) = (field("translateX"), field("translateY"), field("translateZ"));
        let before: Vec<f64> = ["translateX", "translateY", "translateZ"].iter().map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0)).collect();
        // Drag across the three value boxes.
        run(&mut app, vec![egui::Event::PointerMoved(fx.center())], 0.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: fx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.7);
        run(&mut app, vec![egui::Event::PointerMoved(fy.center())], 0.8);
        run(&mut app, vec![egui::Event::PointerMoved(fz.center())], 0.9);
        run(&mut app, vec![egui::Event::PointerButton { pos: fz.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.0);
        run(&mut app, vec![], 1.1);
        let fields: Vec<String> = app.panels.cb_fields.iter().map(|p| p.attr.clone()).collect();
        assert_eq!(fields, vec!["translateX", "translateY", "translateZ"], "field drag selected {fields:?}");
        let during: Vec<f64> = ["translateX", "translateY", "translateZ"].iter().map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0)).collect();
        assert_eq!(during, before, "dragging value fields changed the scene");
        // The last selected field is focused, so typing goes into all three.
        run(&mut app, vec![egui::Event::Text("7".into())], 1.2);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![], 1.4);
        let after: Vec<f64> = ["translateX", "translateY", "translateZ"].iter().map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0)).collect();
        assert_eq!(after, vec![7.0, 7.0, 7.0], "typing into the last field did not update all selected fields");
        // Drag across the channel names for the MMB virtual slider.
        let (nx, nz) = (name("translateX"), name("translateZ"));
        run(&mut app, vec![egui::Event::PointerMoved(nx.center())], 1.5);
        run(&mut app, vec![egui::Event::PointerButton { pos: nx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 1.6);
        run(&mut app, vec![egui::Event::PointerMoved(nz.center())], 1.7);
        run(&mut app, vec![egui::Event::PointerButton { pos: nz.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.8);
        run(&mut app, vec![], 1.9);
        let picks: Vec<String> = app.panels.cb_pick.iter().map(|p| p.attr.clone()).collect();
        assert_eq!(picks, vec!["translateX", "translateY", "translateZ"], "name drag selected {picks:?}");
        assert!(app.panels.cb_armed, "name drag did not arm the virtual slider");
        // Clicking a different field edits only that field.
        let rx = field("rotateX");
        run(&mut app, vec![egui::Event::PointerMoved(rx.center())], 2.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: rx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 2.1);
        run(&mut app, vec![egui::Event::PointerButton { pos: rx.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 2.2);
        run(&mut app, vec![], 2.3);
        run(&mut app, vec![egui::Event::Text("3".into())], 2.4);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], 2.5);
        run(&mut app, vec![], 2.6);
        assert_eq!(app.panels.cb_fields.len(), 1, "click should collapse the field selection");
        assert_eq!(app.panels.cb_fields[0].attr, "rotateX");
        let rotate_x = app.s.get_attr(&NodeRef::Dag(id.clone()), "rotateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let translate_x = app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        assert_eq!(rotate_x, 3.0, "single-field edit did not apply");
        assert_eq!(translate_x, 7.0, "single-field edit changed another field");
        // Relative expressions still apply to every selected field's own value.
        run(&mut app, vec![egui::Event::PointerMoved(fx.center())], 3.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: fx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 3.1);
        run(&mut app, vec![egui::Event::PointerMoved(fy.center())], 3.2);
        run(&mut app, vec![egui::Event::PointerMoved(fz.center())], 3.3);
        run(&mut app, vec![egui::Event::PointerButton { pos: fz.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 3.4);
        run(&mut app, vec![], 3.5);
        run(&mut app, vec![egui::Event::Text("+=1".into())], 3.6);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], 3.7);
        run(&mut app, vec![], 3.8);
        let relative: Vec<f64> = ["translateX", "translateY", "translateZ"].iter().map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0)).collect();
        assert_eq!(relative, vec![8.0, 8.0, 8.0], "relative multi-field edit did not apply: {relative:?}");
        // Shift-click selects a range of fields, then typing sets them all.
        let rz = field("rotateZ");
        run(&mut app, vec![egui::Event::PointerMoved(rx.center())], 4.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: rx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 4.1);
        run(&mut app, vec![egui::Event::PointerButton { pos: rx.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 4.2);
        run(&mut app, vec![], 4.3);
        run(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers { shift: true, ..Default::default() })], 4.4);
        run(&mut app, vec![egui::Event::PointerMoved(rz.center())], 4.5);
        run(&mut app, vec![egui::Event::PointerButton { pos: rz.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers { shift: true, ..Default::default() } }], 4.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: rz.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers { shift: true, ..Default::default() } }], 4.7);
        run(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers::default())], 4.8);
        run(&mut app, vec![], 4.9);
        run(&mut app, vec![egui::Event::Text("5".into())], 5.0);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], 5.1);
        run(&mut app, vec![], 5.2);
        let rotated: Vec<f64> = ["rotateX", "rotateY", "rotateZ"].iter().map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0)).collect();
        assert_eq!(rotated, vec![5.0, 5.0, 5.0], "Shift-click range did not apply to all fields: {rotated:?}");
    }

    #[test]
    fn channel_box_menu_actions() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let pick = CbPick { targets: vec![NodeRef::Dag(id.clone())], attr: "translateX".into(), int: false, min: None, max: None };
        app.panels.cb_pick = vec![pick];
        app.panels.cb_sel = vec!["translateX".into()];
        app.s.time.current = 5.0;
        assert!(key_selected_channels(&mut app));
        let key = inca_core::anim::Anim::key(&id, "translateX");
        assert!(app.s.anim_curves().get(&key).is_some_and(|c| c.keys.iter().any(|k| (k.t - 5.0).abs() < 1e-9)));
        assert!(copy_channel_keys(&mut app));
        assert!(!app.key_clip.is_empty());
        assert_eq!(delete_channel_keys(&mut app), 1);
        assert!(app.s.anim_curves().get(&key).is_none());
        app.s.time.current = 20.0;
        crate::timeline::paste_keys(&mut app);
        assert!(app.s.anim_curves().get(&key).is_some_and(|c| c.keys.iter().any(|k| (k.t - 20.0).abs() < 1e-9)));
        lock_channels(&mut app, true);
        assert!(app.s.node(&id).unwrap().is_locked("translateX"));
        lock_channels(&mut app, false);
        assert!(!app.s.node(&id).unwrap().is_locked("translateX"));
        app.panels.cb_sync_timeline = true;
        app.s.time.current = 7.0;
        app.action("setKey");
        assert!(app.s.anim_curves().get(&key).is_some_and(|c| c.keys.iter().any(|k| (k.t - 7.0).abs() < 1e-9)));
        app.panels.cb_names = 2;
        assert_eq!(cb_label(&app, "translateX"), "tx");
        app.panels.cb_speed = 1;
        assert!((cb_adjust_speed(&app) - 0.005).abs() < 1e-9);
        app.panels.cb_speed = 0;
        assert!((cb_adjust_speed(&app) - 0.01).abs() < 1e-9);
        app.panels.cb_speed = 2;
        assert!((cb_adjust_speed(&app) - 0.05).abs() < 1e-9);
        app.panels.cb_show = Some([true, false, false, true]);
        assert!(cb_shown(cb_filter(&app), "translateX"));
        assert!(!cb_shown(cb_filter(&app), "rotateX"));
        assert!(cb_shown(cb_filter(&app), "visibility"));
    }

    #[test]
    fn channel_box_key_selected_uses_all_picked_channels() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let pick = |attr: &str| CbPick { targets: vec![NodeRef::Dag(id.clone())], attr: attr.into(), int: false, min: None, max: None };
        app.panels.cb_pick = vec![pick("translateX"), pick("translateY")];
        app.panels.cb_sel = vec!["translateX".into(), "translateY".into()];
        app.s.time.current = 5.0;
        assert!(key_selected_channels(&mut app));
        for attr in ["translateX", "translateY"] {
            let key = inca_core::anim::Anim::key(&id, attr);
            assert!(app.s.anim_curves().get(&key).is_some_and(|c| c.keys.iter().any(|k| (k.t - 5.0).abs() < 1e-9)), "{attr} was not keyed");
        }
    }

    #[test]
    fn channel_box_click_away_clears_field_selection() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let rows = app.panels.cb_row_rects.clone();
        let label = rows.iter().find(|(_, a, _, _)| a == "translateX").map(|(_, _, r, _)| *r).expect("translate X label");
        app.panels.cb_fields = vec![CbPick { targets: vec![NodeRef::Dag(id.clone())], attr: "translateX".into(), int: false, min: None, max: None }];
        app.panels.cb_editing = Some((id.clone(), "translateX".into()));
        app.panels.cb_edit_text = "0".into();
        run(&mut app, vec![egui::Event::PointerMoved(label.center()), egui::Event::PointerButton { pos: label.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: label.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.7);
        run(&mut app, vec![], 0.8);
        assert!(app.panels.cb_fields.is_empty(), "clicking the channel name left the value field selected");
        assert!(app.panels.cb_editing.is_none(), "clicking the channel name left the value editor active");
        assert!(app.panels.cb_pick.iter().any(|p| p.attr == "translateX"), "clicking the channel name did not select it");
        // The reverse: clicking the value field drops the channel-name selection.
        let field = rows.iter().find(|(_, a, _, _)| a == "translateX").map(|(_, _, _, f)| *f).expect("translate X field");
        run(&mut app, vec![egui::Event::PointerMoved(field.center()), egui::Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.9);
        run(&mut app, vec![egui::Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.0);
        run(&mut app, vec![], 1.1);
        assert!(app.panels.cb_pick.is_empty(), "clicking the value field left the channel name selected");
        assert!(app.panels.cb_fields.iter().any(|p| p.attr == "translateX"), "clicking the value field did not select it");
    }

    #[test]
    fn channel_box_clicking_a_selected_field_collapses_selection() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let rows = app.panels.cb_row_rects.clone();
        let field = |attr: &str| rows.iter().find(|(_, a, _, _)| a == attr).map(|(_, _, _, f)| *f).expect("field");
        let (fx, fy, fz) = (field("translateX"), field("translateY"), field("translateZ"));
        let drag_select_all = |app: &mut IncaApp, run: &mut dyn FnMut(&mut IncaApp, Vec<egui::Event>, f64), t: f64| {
            run(app, vec![egui::Event::PointerMoved(fx.center())], t);
            run(app, vec![egui::Event::PointerButton { pos: fx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], t + 0.1);
            run(app, vec![egui::Event::PointerMoved(fy.center())], t + 0.2);
            run(app, vec![egui::Event::PointerMoved(fz.center())], t + 0.3);
            run(app, vec![egui::Event::PointerButton { pos: fz.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], t + 0.4);
            run(app, vec![], t + 0.5);
        };
        drag_select_all(&mut app, &mut run, 0.6);
        assert_eq!(app.panels.cb_fields.len(), 3, "drag selection did not select three fields");
        // Clicking one of the selected fields collapses the selection to that field.
        run(&mut app, vec![egui::Event::PointerMoved(fy.center())], 1.2);
        run(&mut app, vec![egui::Event::PointerButton { pos: fy.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![egui::Event::PointerButton { pos: fy.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.4);
        run(&mut app, vec![], 1.5);
        let attrs: Vec<String> = app.panels.cb_fields.iter().map(|p| p.attr.clone()).collect();
        assert_eq!(attrs, vec!["translateY"], "clicking a selected field did not collapse the selection: {attrs:?}");
        // A click with a few pixels of movement also collapses to the pressed field.
        drag_select_all(&mut app, &mut run, 1.6);
        assert_eq!(app.panels.cb_fields.len(), 3);
        let jitter = fy.center() + egui::vec2(8.0, 0.0);
        run(&mut app, vec![egui::Event::PointerMoved(fy.center())], 2.2);
        run(&mut app, vec![egui::Event::PointerButton { pos: fy.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 2.3);
        run(&mut app, vec![egui::Event::PointerMoved(jitter)], 2.4);
        run(&mut app, vec![egui::Event::PointerButton { pos: jitter, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 2.5);
        run(&mut app, vec![], 2.6);
        let attrs: Vec<String> = app.panels.cb_fields.iter().map(|p| p.attr.clone()).collect();
        assert_eq!(attrs, vec!["translateY"], "small drag on a selected field did not collapse the selection: {attrs:?}");
    }

    #[test]
    fn channel_box_renames_from_name_field() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let rect = app.panels.cb_name_rect.expect("Channel Box name field");
        run(&mut app, vec![egui::Event::PointerMoved(rect.center())], 0.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: rect.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.7);
        run(&mut app, vec![egui::Event::PointerButton { pos: rect.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.8);
        run(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers { ctrl: true, command: true, ..Default::default() })], 0.85);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers { ctrl: true, command: true, ..Default::default() } }], 0.9);
        run(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers::default())], 0.95);
        run(&mut app, vec![egui::Event::Text("renamedCube".into())], 1.0);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], 1.1);
        run(&mut app, vec![], 1.2);
        assert_eq!(app.s.name_of(&id), "renamedCube", "Channel Box name field did not rename the object");
    }

    #[test]
    fn display_layer_toolbar_actions() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        layer_create(&mut app, true);
        assert_eq!(app.s.layers.len(), 1);
        let lid = app.s.layers[0].id.clone();
        assert_eq!(app.s.node(&id).unwrap().layer, Some(lid.clone()));
        assert_eq!(app.s.current_layer, Some(lid.clone()));
        assert_eq!(app.panels.layer_sel, vec![lid.clone()]);
        app.s.select(&[], SelMode::Replace);
        layer_select_objects(&mut app);
        assert_eq!(app.s.sel, vec![id.clone()], "Select Objects in Selected Layers did not select the layer contents");
        layer_create(&mut app, false);
        assert_eq!(app.s.layers.len(), 2);
        layer_delete_unused(&mut app);
        assert_eq!(app.s.layers.len(), 1, "Delete Unused Layers kept the empty layer");
        layer_empty(&mut app, &lid);
        assert!(app.s.node(&id).unwrap().layer.is_none());
        app.panels.layer_sel = vec![lid.clone()];
        let lids = app.panels.layer_sel.clone();
        layer_delete(&mut app, &lids);
        assert!(app.s.layers.is_empty());
        assert_eq!(app.s.node(&id).unwrap().name, "pCube1");
    }

    #[test]
    fn layer_set_only_selected_and_add_new_objects() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        layer_create(&mut app, false);
        let l1 = app.s.layers[0].id.clone();
        layer_create(&mut app, false);
        let l2 = app.s.layers[1].id.clone();
        app.panels.layer_sel = vec![l1.clone()];
        layer_apply(&mut app, LayerScope::OnlySelected, "Visible", |l| l.visible = true, |l| l.visible = false);
        assert!(app.s.layers[0].visible);
        assert!(!app.s.layers[1].visible, "Set Only Selected Layers did not hide the others");
        layer_apply(&mut app, LayerScope::Selected, "Bounding Box", |l| l.lod = "bbox".into(), |l| l.lod = "full".into());
        assert_eq!(app.s.layers[0].lod, "bbox");
        assert_eq!(app.s.layers[1].lod, "full");
        app.panels.layer_add_new = true;
        app.s.add_new_to_current = true;
        app.s.current_layer = Some(l1.clone());
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        assert_eq!(app.s.node(&id).unwrap().layer, Some(l1));
        assert_eq!(app.s.node(&id).unwrap().layer, app.s.current_layer);
        let _ = l2;
    }

    #[test]
    fn layer_rename_keeps_names_unique() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        layer_create(&mut app, false);
        layer_create(&mut app, false);
        let lid = app.s.layers[0].id.clone();
        let taken = app.s.layers[1].name.clone();
        layer_rename_commit(&mut app, &lid, &taken);
        assert_ne!(app.s.layers[0].name, taken, "rename reused another layer's name");
        layer_rename_commit(&mut app, &lid, "Characters");
        assert_eq!(app.s.layers[0].name, "Characters");
    }

    #[test]
    fn animation_layer_members_follow_the_selection() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let nid = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&nid), SelMode::Replace);
        anim_layer_new(&mut app, false);
        let lid = app.s.anim_layer_current.clone().expect("current layer");
        assert!(app.s.anim_layers[0].members.is_empty());
        anim_layer_add_remove_selected(&mut app, &lid, true);
        assert_eq!(app.s.anim_layers[0].members, vec![nid.clone()]);
        anim_layer_add_remove_selected(&mut app, &lid, false);
        assert!(app.s.anim_layers[0].members.is_empty());
        anim_layer_new_from_selected(&mut app, false);
        assert!(app.s.anim_layers[0].members.contains(&nid), "from-selected assigns the object");
    }

    #[test]
    fn additive_layer_keys_and_mode_toggle_keep_the_object() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let nid = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&nid), SelMode::Replace);
        anim_layer_new(&mut app, false);
        let lid = app.s.anim_layer_current.clone().expect("current layer");
        // S keys every channel at the rest pose, then a second rotation pose.
        app.s.set_time(1.0);
        for c in CHANNELS {
            app.s.set_key(&nid, c, None, None);
        }
        app.s.set_time(20.0);
        app.s.set_attr_f(&nid, "rotateY", 90.0);
        app.s.set_key(&nid, "rotateY", None, None);
        app.s.set_time(10.0);
        let mid = app.s.get_attr_f(&nid, "rotateY");
        assert!(mid > 1.0 && mid < 89.0, "layer key does not animate: {mid}");
        anim_layer_mode(&mut app, &lid, true);
        assert!((app.s.get_attr_f(&nid, "scaleX") - 1.0).abs() < 1e-9, "switching to override collapsed the scale");
        anim_layer_mode(&mut app, &lid, false);
        assert!(app.s.nodes[&nid].visible, "mode toggle hid the object");
        assert!((app.s.get_attr_f(&nid, "rotateY") - mid).abs() < 1e-9, "mode toggle changed the pose");
        assert!((app.s.get_attr_f(&nid, "scaleX") - 1.0).abs() < 1e-9, "mode toggle changed the scale");
        anim_layer_update(&mut app, &lid, "layer weight", |l| l.weight = 0.5);
        assert!((app.s.get_attr_f(&nid, "rotateY") - mid / 2.0).abs() < 1e-9, "weight does not blend the pose");
        assert!((app.s.get_attr_f(&nid, "scaleX") - 1.0).abs() < 1e-9, "weight changed the scale");
        anim_layer_update(&mut app, &lid, "layer weight", |l| l.weight = 0.0);
        assert!((app.s.get_attr_f(&nid, "rotateY")).abs() < 1e-9, "weight 0 did not return to the static pose");
        assert!((app.s.get_attr_f(&nid, "scaleX") - 1.0).abs() < 1e-9);
        anim_layer_update(&mut app, &lid, "layer weight", |l| l.weight = 1.0);
        anim_layer_update(&mut app, &lid, "layer mute", |l| l.mute = true);
        assert!((app.s.get_attr_f(&nid, "rotateY")).abs() < 1e-9, "mute did not return to the static pose");
        assert!((app.s.get_attr_f(&nid, "scaleX") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn muting_an_animation_layer_also_locks_it() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        anim_layer_new(&mut app, false);
        let lid = app.s.anim_layer_current.clone().expect("current layer");
        anim_layer_mute(&mut app, &lid);
        assert!(app.s.anim_layers[0].mute && app.s.anim_layers[0].lock);
        anim_layer_mute(&mut app, &lid);
        assert!(!app.s.anim_layers[0].mute && !app.s.anim_layers[0].lock);
        // unmute clears the lock even when it was set manually before muting
        anim_layer_lock(&mut app, &lid);
        anim_layer_mute(&mut app, &lid);
        assert!(app.s.anim_layers[0].lock);
        anim_layer_mute(&mut app, &lid);
        assert!(!app.s.anim_layers[0].mute && !app.s.anim_layers[0].lock);
        // unlocking while muted works; re-locking then unmuting still clears it
        anim_layer_mute(&mut app, &lid);
        anim_layer_lock(&mut app, &lid);
        assert!(app.s.anim_layers[0].mute && !app.s.anim_layers[0].lock);
        anim_layer_lock(&mut app, &lid);
        assert!(app.s.anim_layers[0].lock);
        anim_layer_mute(&mut app, &lid);
        assert!(!app.s.anim_layers[0].mute && !app.s.anim_layers[0].lock);
    }

    #[test]
    fn solo_mutes_and_locks_the_other_layers() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        for _ in 0..3 {
            anim_layer_new(&mut app, false);
        }
        let ids: Vec<String> = app.s.anim_layers.iter().map(|l| l.id.clone()).collect();
        let solo_id = ids[1].clone();
        let bottom = ids[2].clone();
        if let Some(l) = app.s.anim_layers.iter_mut().find(|l| l.id == bottom) {
            l.mute = true;
            l.lock = true;
        }
        anim_layer_solo(&mut app, &solo_id);
        for l in &app.s.anim_layers {
            if l.id == solo_id {
                assert!(l.solo && !l.mute && !l.lock);
            } else {
                assert!(!l.solo && l.mute && l.lock, "solo did not mute+lock {}", l.name);
            }
        }
        assert_eq!(app.s.anim_solo_saved.len(), 3);
        // switching solo restores the previous mute/lock states, then applies the new solo
        let other = ids[0].clone();
        anim_layer_solo(&mut app, &other);
        for l in &app.s.anim_layers {
            if l.id == other {
                assert!(l.solo && !l.mute && !l.lock);
            } else {
                assert!(l.mute && l.lock);
            }
        }
        anim_layer_solo(&mut app, &other);
        for l in &app.s.anim_layers {
            assert!(!l.solo);
            if l.id == bottom {
                assert!(l.mute && l.lock, "unsolo lost a manual mute+lock");
            } else {
                assert!(!l.mute && !l.lock, "unsolo did not restore {}", l.name);
            }
        }
        assert!(app.s.anim_solo_saved.is_empty());
    }

    #[test]
    fn layer_attributes_load_into_the_attribute_editor() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        layer_create(&mut app, false);
        let lid = app.s.layers[0].id.clone();
        app.right_tab = crate::app::RightTab::ChannelBox;
        layer_open_attributes(&mut app, &lid);
        assert_eq!(app.panels.ae_layer.as_deref(), Some(lid.as_str()));
        assert_eq!(app.right_tab, crate::app::RightTab::AttributeEditor);
    }

    #[test]
    fn layer_edit_applies_name_type_visibility_and_color() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        layer_create(&mut app, false);
        let lid = app.s.layers[0].id.clone();
        let draft = LayerEditDraft {
            id: lid.clone(),
            name: "Props".into(),
            mode: "template".into(),
            visible: false,
            playback: false,
            color: Some([1.0, 0.5, 0.25]),
            rgb: true,
            focus_name: false,
        };
        layer_apply_edit(&mut app, &draft);
        let l = app.s.layers.iter().find(|l| l.id == lid).unwrap();
        assert_eq!(l.name, "Props");
        assert_eq!(l.mode, "template");
        assert!(!l.visible);
        assert!(!l.playback);
        assert!(layer_has_color(&l.color));
        assert!(l.shading, "unrelated layer fields must be preserved");
    }

    #[test]
    fn outliner_reorder_moves_siblings() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let p = app.s.create_group("grp", None);
        let a = app.s.create_group("a", Some(p.as_str()));
        let b = app.s.create_group("b", Some(p.as_str()));
        let c = app.s.create_group("c", Some(p.as_str()));
        let names = |app: &IncaApp| -> Vec<String> { app.s.children(Some(p.as_str())).iter().map(|i| app.s.name_of(i)).collect() };
        outliner_reorder(&mut app, std::slice::from_ref(&c), &a, false);
        assert_eq!(names(&app), vec!["c", "a", "b"]);
        outliner_reorder(&mut app, std::slice::from_ref(&c), &b, true);
        assert_eq!(names(&app), vec!["a", "b", "c"]);
    }

    #[test]
    fn outliner_drag_preview_keeps_names_intact() {
        let names = vec![
            "pCube1".to_string(),
            "pCylinder1".to_string(),
            "a_very_long_object_name_that_will_not_fit".to_string(),
        ];
        let text = outliner_drag_text(&names);
        for line in text.lines() {
            assert!(line.chars().count() <= 32, "line too long: {line}");
            assert!(!line.starts_with('…') && !line.ends_with("…,…"), "mid-name break: {line}");
        }
        assert!(text.contains("pCube1, pCylinder1"), "{text}");
        assert!(text.ends_with('…'), "long name must be ellipsized: {text}");
        assert!(!text.contains('\n') || !text.lines().last().unwrap().contains(','), "the long name should be alone on its line");

        let many: Vec<String> = (0..20).map(|i| format!("pCube{i}")).collect();
        let text = outliner_drag_text(&many);
        assert!(text.contains("+10 more"), "{text}");
        assert!(text.lines().count() <= 8, "the preview must stay bounded: {text}");
    }

    #[test]
    fn layer_membership_adds_and_removes_an_object() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let nid = app.s.create_mesh("polyCube", &Params::new(), None);
        layer_create(&mut app, false);
        let lid = app.s.layers[0].id.clone();
        layer_set_members(&mut app, &lid, std::slice::from_ref(&nid), true);
        assert_eq!(app.s.node(&nid).unwrap().layer.as_deref(), Some(lid.as_str()));
        layer_set_members(&mut app, &lid, std::slice::from_ref(&nid), false);
        assert!(app.s.node(&nid).unwrap().layer.is_none());
        // removing from a different layer must not clear this object's membership
        layer_create(&mut app, false);
        let other = app.s.layers[1].id.clone();
        layer_set_members(&mut app, &lid, std::slice::from_ref(&nid), true);
        layer_set_members(&mut app, &other, std::slice::from_ref(&nid), false);
        assert_eq!(app.s.node(&nid).unwrap().layer.as_deref(), Some(lid.as_str()));
    }

    #[test]
    fn layer_move_reorders_the_list() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        layer_create(&mut app, false);
        layer_create(&mut app, false);
        layer_create(&mut app, false);
        let last = app.s.layers[2].id.clone();
        app.panels.layer_sel = vec![last.clone()];
        layer_move(&mut app, true);
        assert_eq!(app.s.layers[1].id, last);
        layer_move(&mut app, true);
        assert_eq!(app.s.layers[0].id, last);
        layer_move(&mut app, true);
        assert_eq!(app.s.layers[0].id, last, "the top layer moved past the start");
        layer_move(&mut app, false);
        assert_eq!(app.s.layers[1].id, last);
    }

    #[test]
    fn layer_color_and_default_round_trip() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        layer_create(&mut app, false);
        let lid = app.s.layers[0].id.clone();
        layer_set_color(&mut app, &lid, [1.0, 0.0, 0.0]);
        assert_eq!(app.s.layers[0].color, Some(json!([1.0, 0.0, 0.0])));
        assert_eq!(layer_swatch_color(&app.s.layers[0].color), Color32::from_rgb(255, 0, 0));
        layer_clear_color(&mut app, &lid);
        assert!(app.s.layers[0].color.is_none());
        assert!(!layer_has_color(&app.s.layers[0].color));
        assert_eq!(layer_swatch_color(&app.s.layers[0].color), Color32::TRANSPARENT);
    }

    #[test]
    fn channel_box_add_and_remove_selected_layer() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let lid = app.s.new_id("l");
        app.s.layers.push(inca_core::scene::Layer {
            id: lid.clone(),
            name: "Layer 1".into(),
            visible: true,
            mode: "normal".into(),
            color: None,
            playback: true,
            enabled: true,
            shading: true,
            texturing: true,
            lod: "full".into(),
            anim_playback: true,
            created: 1,
            extra: Default::default(),
        });
        app.panels.layer_sel = vec![lid.clone()];
        app.s.current_layer = Some(lid.clone());
        layer_members(&mut app, true);
        assert_eq!(app.s.node(&id).unwrap().layer.as_deref(), Some(lid.as_str()));
        layer_members(&mut app, false);
        assert_eq!(app.s.node(&id).unwrap().layer, None);
    }

    #[test]
    fn channel_box_settings_manips_and_namespace() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        app.panels.cb_manips = Some(0);
        cb_arm(&mut app);
        assert!(!app.panels.cb_armed, "No Manips armed the virtual slider");
        app.panels.cb_manips = Some(1);
        cb_arm(&mut app);
        assert!(app.panels.cb_armed, "Invisible Manips did not arm the virtual slider");
        app.panels.cb_manips = None;
        cb_arm(&mut app);
        assert!(app.panels.cb_armed);
        app.panels.cb_hide_namespace = true;
        assert_eq!(shown_node_name(&app, "asset:pCube1"), "pCube1");
        app.panels.cb_hide_namespace = false;
        assert_eq!(shown_node_name(&app, "asset:pCube1"), "asset:pCube1");
    }

    #[test]
    fn shading_modes_turn_off_use_all_lights() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        app.active_vp = 0;
        app.vps[0].shade = crate::viewport::ShadeMode::Textured;
        app.vps[0].lights = crate::viewport::LightMode::All;
        app.action("shadeSmooth");
        assert_eq!(app.vps[0].shade, crate::viewport::ShadeMode::Shaded);
        assert_eq!(app.vps[0].lights, crate::viewport::LightMode::Default);
        app.vps[0].lights = crate::viewport::LightMode::All;
        app.action("shadeTextured");
        assert_eq!(app.vps[0].shade, crate::viewport::ShadeMode::Textured);
        assert_eq!(app.vps[0].lights, crate::viewport::LightMode::Default);
        app.vps[0].lights = crate::viewport::LightMode::All;
        app.action("shadeWire");
        assert_eq!(app.vps[0].shade, crate::viewport::ShadeMode::Wire);
        assert_eq!(app.vps[0].lights, crate::viewport::LightMode::Default);
        app.action("shadeAllLights");
        assert_eq!(app.vps[0].lights, crate::viewport::LightMode::All);
        app.action("shadeAllLights");
        assert_eq!(app.vps[0].lights, crate::viewport::LightMode::All, "the 7 hotkey should not toggle all-lights off");
    }

    #[test]
    fn right_panel_widths_are_remembered_per_tab() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, t: f64| {
            let mut out = ctx.run_ui(raw(vec![], t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        app.right_tab = crate::app::RightTab::ChannelBox;
        for i in 0..3 { run(&mut app, i as f64 * 0.1); }
        let cb_id = egui::Id::new(("rightpanel", crate::app::RightTab::ChannelBox));
        let cb_width = egui::containers::panel::PanelState::load(&ctx, cb_id).expect("Channel Box panel state").size().x;
        app.right_tab = crate::app::RightTab::AttributeEditor;
        for i in 0..3 { run(&mut app, 1.0 + i as f64 * 0.1); }
        let ae_id = egui::Id::new(("rightpanel", crate::app::RightTab::AttributeEditor));
        let ae_width = egui::containers::panel::PanelState::load(&ctx, ae_id).expect("Attribute Editor panel state").size().x;
        app.right_tab = crate::app::RightTab::ChannelBox;
        for i in 0..3 { run(&mut app, 2.0 + i as f64 * 0.1); }
        let cb_again = egui::containers::panel::PanelState::load(&ctx, cb_id).expect("Channel Box panel state").size().x;
        assert!(ae_width > cb_width, "tab min widths should differ: CB {cb_width}, AE {ae_width}");
        assert!((cb_again - cb_width).abs() < 1.0, "Channel Box width changed after visiting the AE: {cb_width} -> {cb_again}");
    }

    #[test]
    fn attribute_editor_presets_save_and_blend() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        let node = NodeRef::Dag(id.clone());
        app.s.set_attr(&node, "translateX", json!(2.0));
        let kind = ae_preset_kind(&app, &node).expect("node kind");
        ae_save_preset(&mut app, &node, &kind, "Preset A");
        assert!(app.prefs.attr_presets.iter().any(|p| p.name == "Preset A" && p.kind == kind));
        app.s.set_attr(&node, "translateX", json!(5.0));
        let preset = app.prefs.attr_presets.iter().find(|p| p.name == "Preset A").unwrap().clone();
        ae_apply_preset(&mut app, &node, &preset, None);
        assert_eq!(app.s.get_attr(&node, "translateX").and_then(|v| v.as_f64()), Some(2.0));
        app.s.set_attr(&node, "translateX", json!(4.0));
        ae_apply_preset(&mut app, &node, &preset, Some(0.5));
        assert_eq!(app.s.get_attr(&node, "translateX").and_then(|v| v.as_f64()), Some(3.0));
    }

    #[test]
    fn attribute_editor_drag_selects_transform_fields() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::AttributeEditor;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let rows = app.panels.cb_row_rects.clone();
        let field = |attr: &str| rows.iter().find(|(_, a, _, _)| a == attr).map(|(_, _, _, f)| *f).expect("AE field rect");
        let (fx, fy, fz) = (field("translateX"), field("translateY"), field("translateZ"));
        assert!((fx.center().y - fy.center().y).abs() < 1.0 && (fy.center().y - fz.center().y).abs() < 1.0, "translate fields are not a 3-column row");
        run(&mut app, vec![egui::Event::PointerMoved(fx.center())], 0.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: fx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.7);
        run(&mut app, vec![egui::Event::PointerMoved(fy.center())], 0.8);
        run(&mut app, vec![egui::Event::PointerMoved(fz.center())], 0.9);
        run(&mut app, vec![egui::Event::PointerButton { pos: fz.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.0);
        run(&mut app, vec![], 1.1);
        let selected: Vec<String> = app.panels.cb_fields.iter().map(|p| p.attr.clone()).collect();
        assert_eq!(selected, vec!["translateX", "translateY", "translateZ"], "AE drag selected {selected:?}");
        run(&mut app, vec![egui::Event::Text("6".into())], 1.2);
        run(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![], 1.4);
        let values: Vec<f64> = ["translateX", "translateY", "translateZ"].iter().map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0)).collect();
        assert_eq!(values, vec![6.0, 6.0, 6.0], "AE multi-field edit did not apply: {values:?}");
    }

    #[test]
    fn channel_box_virtual_slider_starts_at_the_press() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.s.set_attr(&NodeRef::Dag(id.clone()), "translateX", serde_json::json!(2.0));
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let name = |app: &IncaApp, attr: &str| app.panels.cb_row_rects.iter().find(|(_, a, _, _)| a == attr).map(|(_, _, r, _)| *r).expect("name rect");
        let nx = name(&app, "translateX");
        run(&mut app, vec![egui::Event::PointerMoved(nx.center())], 0.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: nx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.7);
        run(&mut app, vec![egui::Event::PointerButton { pos: nx.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.8);
        run(&mut app, vec![], 0.9);
        assert!(app.panels.cb_armed && !app.panels.cb_pick.is_empty(), "name click did not arm the virtual slider");
        let before = app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vp = app.vps[app.active_vp].rect;
        let press = vp.center() + egui::vec2(140.0, 120.0);
        let moved = press + egui::vec2(20.0, 0.0);
        run(&mut app, vec![egui::Event::PointerMoved(press)], 1.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: press, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 1.1);
        run(&mut app, vec![egui::Event::PointerMoved(moved)], 1.2);
        run(&mut app, vec![egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![], 1.4);
        let after = app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        assert!((after - before).abs() < 1.0, "MMB virtual slider jumped the object: {before} -> {after}");
        assert!((after - before).abs() > 0.01, "MMB virtual slider did not move the object");
        for attr in ["translateY", "translateZ"] {
            let v = app.s.get_attr(&NodeRef::Dag(id.clone()), attr).and_then(|v| v.as_f64()).unwrap_or(0.0);
            assert_eq!(v, 0.0, "MMB virtual slider moved {attr}");
        }
    }

    #[test]
    fn mmb_to_lmb_switch_uses_the_new_handle() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.tool = Tool::Move;
        app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)] = Some(transform_gizmo_egui::prelude::GizmoMode::TranslateX);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let vp = app.vps[app.active_vp].rect;
        let press = vp.center() + egui::vec2(140.0, 120.0);
        run(&mut app, vec![egui::Event::PointerMoved(press)], 1.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: press, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 1.1);
        run(&mut app, vec![egui::Event::PointerMoved(press + egui::vec2(20.0, 0.0))], 1.2);
        // Switch to LMB on the Y arrow in the same frame as the MMB release.
        let x_before = app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cam = app.vps[app.active_vp].camera.clone();
        let rect = app.vps[app.active_vp].rect;
        let cv = crate::viewport::cam_view(&app.s, &cam, [rect.width(), rect.height()]);
        let p = cv.project(app.s.world_pivot(&id), rect).expect("pivot on screen");
        let py = cv.project(app.s.world_pivot(&id) + glam::DVec3::Y, rect).expect("Y axis on screen");
        let u = (py - p).normalized();
        let y = p + u * 60.0;
        run(&mut app, vec![
            egui::Event::PointerButton { pos: y, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() },
            egui::Event::PointerMoved(y),
            egui::Event::PointerButton { pos: y, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        ], 1.3);
        run(&mut app, vec![egui::Event::PointerMoved(y + u * 20.0)], 1.4);
        run(&mut app, vec![egui::Event::PointerButton { pos: y + u * 20.0, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.5);
        run(&mut app, vec![], 1.6);
        let x = app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let yv = app.s.get_attr(&NodeRef::Dag(id.clone()), "translateY").and_then(|v| v.as_f64()).unwrap_or(0.0);
        assert!((x - x_before).abs() < 1e-6, "LMB after MMB kept moving the old X handle: {x_before} -> {x}");
        assert!(yv.abs() > 0.001, "LMB after MMB did not drag the Y handle: y={yv}");
        assert_eq!(app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)], Some(transform_gizmo_egui::prelude::GizmoMode::TranslateY), "LMB after MMB did not select the Y handle");
    }

    #[test]
    fn ctrl_click_activates_the_plane_handle() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.tool = Tool::Move;
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let y = app.vps[app.active_vp].rect.center() + egui::vec2(0.0, -60.0);
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        run(&mut app, vec![egui::Event::ModifiersChanged(ctrl)], 1.0);
        run(&mut app, vec![egui::Event::PointerMoved(y)], 1.05);
        run(&mut app, vec![egui::Event::PointerButton { pos: y, button: egui::PointerButton::Primary, pressed: true, modifiers: ctrl }], 1.1);
        run(&mut app, vec![egui::Event::PointerButton { pos: y, button: egui::PointerButton::Primary, pressed: false, modifiers: ctrl }], 1.2);
        run(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers::default())], 1.3);
        run(&mut app, vec![], 1.4);
        assert_eq!(app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)], Some(transform_gizmo_egui::prelude::GizmoMode::TranslateXZ), "Ctrl+click did not lock the Y axis");
        assert!(app.vp_state.force_handle.is_none(), "Ctrl+click left the plane drag armed");
    }

    #[test]
    fn mmb_drag_does_not_lock_the_active_handle() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.tool = Tool::Move;
        app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)] = Some(transform_gizmo_egui::prelude::GizmoMode::TranslateX);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let vp = app.vps[app.active_vp].rect;
        let press = vp.center() + egui::vec2(140.0, 120.0);
        let moved = press + egui::vec2(20.0, 0.0);
        run(&mut app, vec![egui::Event::PointerMoved(press)], 1.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: press, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 1.1);
        run(&mut app, vec![egui::Event::PointerMoved(moved)], 1.2);
        run(&mut app, vec![egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![], 1.4);
        assert!(app.vp_state.mmb.is_none(), "MMB drag left the middle-button drag armed");
        assert!(app.vp_state.force_handle.is_none(), "MMB drag left a forced handle armed");
        assert!(app.vp_state.drag.is_none(), "MMB drag left the gizmo drag active");
        assert!(app.vp_state.gizmos[app.active_vp].config().mode_override.is_none(), "MMB drag left the gizmo locked to one handle");
        // put the cube back under the gizmo centre, then click the Y arrow
        app.s.set_attr(&NodeRef::Dag(id.clone()), "translateX", serde_json::json!(0.0));
        run(&mut app, vec![], 1.5);
        let y = vp.center() + egui::vec2(0.0, -60.0);
        run(&mut app, vec![egui::Event::PointerMoved(y)], 1.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: y, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 1.7);
        run(&mut app, vec![egui::Event::PointerButton { pos: y, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.8);
        run(&mut app, vec![], 1.9);
        assert_eq!(app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)], Some(transform_gizmo_egui::prelude::GizmoMode::TranslateY), "LMB could not change the active handle after an MMB drag");
    }

    #[test]
    fn rotate_grey_area_free_rotates() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.tool = Tool::Rotate;
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let rotate = |app: &IncaApp| -> [f64; 3] {
            ["rotateX", "rotateY", "rotateZ"].map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0))
        };
        let before = rotate(&app);
        let c = app.vps[app.active_vp].rect.center();
        let end = c + egui::vec2(40.0, 20.0);
        run(&mut app, vec![egui::Event::PointerMoved(c)], 0.6);
        run(&mut app, vec![egui::Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.7);
        run(&mut app, vec![egui::Event::PointerMoved(end)], 0.8);
        run(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.9);
        run(&mut app, vec![], 1.0);
        assert_eq!(app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Rotate)], Some(transform_gizmo_egui::prelude::GizmoMode::Arcball), "the grey area did not become the free-rotate handle");
        let after = rotate(&app);
        assert!(before.iter().zip(after).any(|(a, b)| (a - b).abs() > 0.01), "dragging the grey area did not rotate: {before:?} -> {after:?}");
    }

    #[test]
    fn shift_mmb_auto_axis_starts_at_the_object() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.tool = Tool::Move;
        app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)] = None;
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let xyz = |app: &IncaApp| -> [f64; 3] {
            ["translateX", "translateY", "translateZ"].map(|a| app.s.get_attr(&NodeRef::Dag(id.clone()), a).and_then(|v| v.as_f64()).unwrap_or(0.0))
        };
        let before = xyz(&app);
        let vp = app.vps[app.active_vp].rect;
        let press = vp.center() + egui::vec2(140.0, 120.0);
        let moved = press + egui::vec2(20.0, 0.0);
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        run(&mut app, vec![egui::Event::ModifiersChanged(shift)], 1.0);
        run(&mut app, vec![egui::Event::PointerMoved(press)], 1.05);
        run(&mut app, vec![egui::Event::PointerButton { pos: press, button: egui::PointerButton::Middle, pressed: true, modifiers: shift }], 1.1);
        run(&mut app, vec![], 1.15);
        assert_eq!(xyz(&app), before, "Shift+MMB press jumped the object");
        run(&mut app, vec![egui::Event::PointerMoved(moved)], 1.2);
        let solo = app.vp_state.gizmo_solo[crate::vp_ui::mode_index(Tool::Move)];
        assert_eq!(solo, app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)], "Shift+MMB did not hide the other axes");
        assert!(solo.is_some(), "Shift+MMB selected no axis");
        run(&mut app, vec![egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Middle, pressed: false, modifiers: shift }], 1.3);
        run(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers::default())], 1.35);
        run(&mut app, vec![], 1.4);
        assert_eq!(app.vp_state.gizmo_solo[crate::vp_ui::mode_index(Tool::Move)], None, "Shift+MMB left only one axis showing");
        let after = xyz(&app);
        let moved_by: f64 = before.iter().zip(after).map(|(a, b)| (b - a).abs()).sum();
        assert!(moved_by < 1.0, "Shift+MMB auto-axis jumped: {before:?} -> {after:?}");
        assert!(moved_by > 0.001, "Shift+MMB auto-axis did not move: {before:?} -> {after:?}");
    }

    #[test]
    fn lock_selection_lmb_drags_the_active_handle() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.s.set_attr(&NodeRef::Dag(id.clone()), "translateX", serde_json::json!(2.0));
        app.tool = Tool::Move;
        app.lock_selection = true;
        app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)] = Some(transform_gizmo_egui::prelude::GizmoMode::TranslateX);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let x = |app: &IncaApp| app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vp = app.vps[app.active_vp].rect;
        let press = vp.center() + egui::vec2(140.0, 120.0);
        let moved = press + egui::vec2(20.0, 0.0);
        run(&mut app, vec![egui::Event::PointerMoved(press)], 1.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: press, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 1.1);
        run(&mut app, vec![], 1.15);
        assert!((x(&app) - 2.0).abs() < 1e-6, "locked LMB press jumped the object: {}", x(&app));
        run(&mut app, vec![egui::Event::PointerMoved(moved)], 1.2);
        run(&mut app, vec![egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![], 1.4);
        let dx = x(&app) - 2.0;
        assert!(dx.abs() < 1.0, "locked LMB drag jumped the object: 2 -> {}", x(&app));
        assert!(dx.abs() > 0.001, "locked LMB drag did not move the object");
        assert_eq!(app.s.sel.len(), 1, "locked LMB drag changed the selection");
    }

    #[test]
    fn manipulator_mmb_drag_starts_at_the_object() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.s.set_attr(&NodeRef::Dag(id.clone()), "translateX", serde_json::json!(2.0));
        app.tool = Tool::Move;
        app.vp_state.active_handles[crate::vp_ui::mode_index(Tool::Move)] = Some(transform_gizmo_egui::prelude::GizmoMode::TranslateX);
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut out = ctx.run_ui(raw(events, t), |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
        };
        for i in 0..5 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        let x = |app: &IncaApp| app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vp = app.vps[app.active_vp].rect;
        let press = vp.center() + egui::vec2(140.0, 120.0);
        let moved = press + egui::vec2(20.0, 0.0);
        run(&mut app, vec![egui::Event::PointerMoved(press)], 1.0);
        run(&mut app, vec![egui::Event::PointerButton { pos: press, button: egui::PointerButton::Middle, pressed: true, modifiers: Default::default() }], 1.1);
        assert_eq!(app.vp_state.gizmo_solo[crate::vp_ui::mode_index(Tool::Move)], Some(transform_gizmo_egui::prelude::GizmoMode::TranslateX), "MMB did not hide the other axes");
        run(&mut app, vec![], 1.15);
        assert!((x(&app) - 2.0).abs() < 1e-6, "MMB press jumped the object to the cursor: {}", x(&app));
        run(&mut app, vec![egui::Event::PointerMoved(moved)], 1.2);
        run(&mut app, vec![egui::Event::PointerButton { pos: moved, button: egui::PointerButton::Middle, pressed: false, modifiers: Default::default() }], 1.3);
        run(&mut app, vec![], 1.4);
        assert_eq!(app.vp_state.gizmo_solo[crate::vp_ui::mode_index(Tool::Move)], None, "MMB drag left only one axis showing");
        let dx = x(&app) - 2.0;
        assert!(dx.abs() < 1.0, "MMB axis drag jumped the object: 2 -> {}", x(&app));
        assert!(dx.abs() > 0.001, "MMB axis drag did not move the object");
        for attr in ["translateY", "translateZ"] {
            let v = app.s.get_attr(&NodeRef::Dag(id.clone()), attr).and_then(|v| v.as_f64()).unwrap_or(0.0);
            assert_eq!(v, 0.0, "MMB axis drag moved {attr}");
        }
    }

    #[test]
    fn channel_box_unpublish_and_lock_unpublished() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let _ = app.run_mel("setup asset", "polyCube -n box; select -r box; container -n asset1; select -r box; container -e -publishName pubTx asset1; container -e -ba box.translateX pubTx asset1;");
        let asset = app.s.find("asset1").expect("asset");
        let cube = app.s.find("box").expect("box");
        app.s.select(std::slice::from_ref(&cube), SelMode::Replace);
        app.panels.cb_pick = vec![CbPick { targets: vec![NodeRef::Dag(asset.clone())], attr: "pubTx".into(), int: false, min: None, max: None }];
        unpublish_channels(&mut app);
        assert!(!app.s.node(&asset).unwrap().published.contains_key("pubTx"), "published attribute was not removed");
        let _ = app.run_mel("republish", "select -r box; container -e -publishName pubTx asset1; container -e -ba box.translateX pubTx asset1;");
        lock_unpublished_attributes(&mut app);
        let n = app.s.node(&cube).unwrap();
        assert!(!n.is_locked("translateX"), "published attribute was locked");
        assert!(n.is_locked("translateY"), "unpublished attribute was not locked");
    }

    #[test]
    fn channel_box_shows_asset_attributes_at_top() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let _ = app.run_mel("setup asset", "polyCube -n box; select -r box; container -n asset1; select -r box; container -e -publishName pubTx asset1; container -e -ba box.translateX pubTx asset1;");
        let cube = app.s.find("box").expect("box");
        app.s.select(std::slice::from_ref(&cube), SelMode::Replace);
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..3 {
            let mut out = ctx.run_ui(raw(vec![], 1.0 + i as f64 * 0.1), |ui| eframe::App::ui(&mut app, ui, &mut frame));
            out.textures_delta.clear();
        }
        assert!(app.panels.cb_row_rects.iter().any(|(_, a, _, _)| a == "pubTx"), "asset attribute is missing from the Channel Box");
        app.panels.cb_asset_top = true;
        let mut out = ctx.run_ui(raw(vec![], 1.4), |ui| eframe::App::ui(&mut app, ui, &mut frame));
        out.textures_delta.clear();
        assert_eq!(app.panels.cb_row_rects.first().map(|(_, a, _, _)| a.as_str()), Some("pubTx"), "asset attributes are not at the top");
    }

    #[test]
    fn channel_box_renames_custom_attribute_references() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.nodes.get_mut(&id).unwrap().extra = json!({ "foo": 2.0 });
        app.s.set_key(&id, "foo", Some(4.0), Some(2.0));
        app.s.nodes.get_mut(&id).unwrap().locks.insert("foo".into(), json!(true));
        app.s.nodes.get_mut(&id).unwrap().hidden_channels.insert("foo".into());
        app.s.nodes.get_mut(&id).unwrap().nonkeyable.insert("foo".into());
        rename_custom_attr(&mut app, &id, "foo", "bar");
        let n = app.s.node(&id).unwrap();
        assert!(n.extra.as_object().is_some_and(|e| e.contains_key("bar") && !e.contains_key("foo")));
        assert!(n.locks.contains_key("bar") && !n.locks.contains_key("foo"));
        assert!(n.hidden_channels.contains("bar") && n.nonkeyable.contains("bar"));
        assert!(app.s.anim_curve(&id, "bar").is_some_and(|c| c.find(4.0).is_some()), "animation curve lost its new name");
    }

    #[test]
    fn channel_box_precision_controls_value_text() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.s.set_attr(&NodeRef::Dag(id.clone()), "translateX", json!(1.23456));
        app.panels.cb_precision = Some(2);
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..3 {
            let mut out = ctx.run_ui(raw(vec![], 1.0 + i as f64 * 0.1), |ui| eframe::App::ui(&mut app, ui, &mut frame));
            out.textures_delta.clear();
        }
        let field = app.panels.cb_row_rects.iter().find(|(_, a, _, _)| a == "translateX").map(|(_, _, _, f)| *f).expect("translate field");
        for (t, ev) in [
            (1.4, vec![egui::Event::PointerMoved(field.center())]),
            (1.5, vec![egui::Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }]),
            (1.6, vec![egui::Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]),
            (1.7, vec![]),
        ] {
            let mut out = ctx.run_ui(raw(ev, t), |ui| eframe::App::ui(&mut app, ui, &mut frame));
            out.textures_delta.clear();
        }
        assert_eq!(app.panels.cb_edit_text, "1.23", "precision was not applied to the field");
    }

    #[test]
    fn channel_box_custom_show_entry_filters_rows() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.panels.cb_pick = vec![CbPick { targets: vec![NodeRef::Dag(id.clone())], attr: "translateX".into(), int: false, min: None, max: None }];
        create_custom_show_entry(&mut app);
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..3 {
            let mut out = ctx.run_ui(raw(vec![], 1.0 + i as f64 * 0.1), |ui| eframe::App::ui(&mut app, ui, &mut frame));
            out.textures_delta.clear();
        }
        let rows: Vec<String> = app.panels.cb_row_rects.iter().map(|(_, a, _, _)| a.clone()).collect();
        assert_eq!(rows, vec!["translateX"], "custom filter did not restrict the Channel Box");
    }

    #[test]
    fn channel_box_breakdown_mute_keyable_and_hide() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.right_tab = crate::app::RightTab::ChannelBox;
        let id = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.s.time.current = 5.0;
        let pick = |attr: &str| CbPick { targets: vec![NodeRef::Dag(id.clone())], attr: attr.into(), int: attr == "visibility", min: None, max: None };
        app.panels.cb_pick = vec![pick("translateX")];
        app.panels.cb_sel = vec!["translateX".into()];
        breakdown_channels(&mut app, false);
        let key = inca_core::anim::Anim::key(&id, "translateX");
        assert!(app.s.anim_curves().get(&key).is_some_and(|c| c.find(5.0).is_some_and(|i| c.keys[i].breakdown)), "breakdown key was not marked");
        mute_channels(&mut app, true, false);
        assert!(app.s.anim_curves().get(&key).is_some_and(|c| c.mute), "selected channel was not muted");
        app.s.set_attr(&NodeRef::Dag(id.clone()), "translateX", json!(3.0));
        app.s.apply_anim();
        assert_eq!(app.s.get_attr(&NodeRef::Dag(id.clone()), "translateX").and_then(|v| v.as_f64()), Some(3.0), "muted curve drove the channel");
        mute_channels(&mut app, false, false);
        assert!(app.s.anim_curves().get(&key).is_some_and(|c| !c.mute), "selected channel was not unmuted");
        app.panels.cb_pick = vec![pick("translateY")];
        app.panels.cb_sel = vec!["translateY".into()];
        set_channel_keyable(&mut app, false);
        assert!(!key_selected_channels(&mut app), "nonkeyable channel was keyed");
        assert!(!app.s.anim_is_keyed(&id, "translateY"), "nonkeyable channel created a curve");
        set_channel_keyable(&mut app, true);
        assert!(key_selected_channels(&mut app), "keyable channel was not keyed");
        break_channel_connections(&mut app);
        assert!(!app.s.anim_is_keyed(&id, "translateY"), "Break Connections left the keyframe behind");
        app.panels.cb_pick = vec![pick("translateX")];
        app.panels.cb_sel = vec!["translateX".into()];
        hide_channels(&mut app, false);
        let mut frame = eframe::Frame::_new_kittest();
        for i in 0..3 {
            let mut out = ctx.run_ui(raw(vec![], 1.0 + i as f64 * 0.1), |ui| eframe::App::ui(&mut app, ui, &mut frame));
            out.textures_delta.clear();
        }
        assert!(!app.panels.cb_row_rects.iter().any(|(_, a, _, _)| a == "translateX"), "hidden channel still has a row");
    }
}
