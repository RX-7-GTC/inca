//! Hotkey Editor: Maya-style keyboard picture plus a searchable command list.

use crate::app::IncaApp;
use crate::theme;
use egui::{pos2, vec2, Align, Align2, Color32, FontId, Key, LayerId, Modifiers, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// chord ("Ctrl+Shift+K") -> action id; "unbind:<action>" entries switch an action's default hotkey off.
pub type Keymap = BTreeMap<String, String>;

/// A named custom hotkey set. Inca Default is implicit and read-only.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct KeymapSet {
    pub name: String,
    pub map: Keymap,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NameMode {
    New,
    Duplicate,
    Rename,
}

/// Source of truth for the explicitly saved hotkey sets.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct KeymapFile {
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub sets: Vec<KeymapSet>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum KeymapImport {
    One(KeymapSet),
    Many(KeymapFile),
}

pub fn load_keymaps_file() -> Option<KeymapFile> {
    let path = eframe::storage_dir("Inca")?.join("keymaps.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_keymaps_file(app: &mut IncaApp) -> Result<(), String> {
    if app.test.is_some() {
        return Ok(());
    }
    let dir = eframe::storage_dir("Inca").ok_or_else(|| "no settings folder available".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    sync_active_set(app);
    let data = KeymapFile { active: app.active_keymap, sets: app.keymaps.clone() };
    let json = serde_json::to_string_pretty(&data).map_err(|e| format!("could not encode keymaps: {e}"))?;
    std::fs::write(dir.join("keymaps.json"), json).map_err(|e| format!("could not write keymaps: {e}"))
}

#[derive(Clone)]
pub struct Pending {
    pub chord: String,
    pub action: String,
    pub existing: String,
}

#[derive(Default)]
pub struct HkState {
    pub filter: String,
    pub category: String,
    pub selected: Option<String>,
    /// waiting for the next key press to bind to `selected`
    pub capturing: bool,
    pub layer_ctrl: bool,
    pub layer_alt: bool,
    pub layer_shift: bool,
    pub status: String,
    pub pending: Option<Pending>,
    pub restore_confirm: bool,
    /// one-shot: scroll the selected command into view
    pub reveal: bool,
    /// layers to restore when a command selected by auto-switch is deselected
    pub restore_layers: Option<(bool, bool, bool)>,
    pub naming: Option<NameMode>,
    pub name_buf: String,
    pub name_focus: bool,
    pub delete_confirm: bool,
}

pub fn chord(key: Key, m: Modifiers) -> String {
    let mut s = String::new();
    if m.ctrl || m.command {
        s += "Ctrl+";
    }
    if m.shift {
        s += "Shift+";
    }
    if m.alt {
        s += "Alt+";
    }
    s + key.name()
}

fn layer_chord(key: &str, ctrl: bool, shift: bool, alt: bool) -> String {
    let mut s = String::new();
    if ctrl {
        s += "Ctrl+";
    }
    if shift {
        s += "Shift+";
    }
    if alt {
        s += "Alt+";
    }
    s + key
}

fn chord_layer(c: &str) -> (bool, bool, bool) {
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    if let Some((prefix, _)) = c.rsplit_once('+') {
        for p in prefix.split('+') {
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "meta" => ctrl = true,
                "shift" => shift = true,
                "alt" | "option" => alt = true,
                _ => {}
            }
        }
    }
    (ctrl, shift, alt)
}

/// Simpler chords first, then Ctrl before Shift before Alt.
fn chord_pref(c: &str) -> (usize, bool, bool, bool) {
    let (ctrl, shift, alt) = chord_layer(c);
    let count = ctrl as usize + shift as usize + alt as usize;
    (count, !ctrl, !shift, !alt)
}

/// The action for a key press: a user binding first, then the Maya default unless it was removed.
pub fn resolve(app: &IncaApp, key: Key, m: Modifiers) -> Option<String> {
    if let Some(a) = app.keymap.get(&chord(key, m)) {
        return Some(a.clone());
    }
    let a = crate::actions::hotkey(key, m, app)?;
    (!app.keymap.contains_key(&format!("unbind:{a}"))).then(|| a.to_string())
}

fn bound_action(app: &IncaApp, c: &str) -> Option<String> {
    if let Some(a) = app.keymap.get(c) {
        return Some(a.clone());
    }
    crate::actions::ACTIONS.iter().find(|(id, _, hk)| *hk == c && !app.keymap.contains_key(&format!("unbind:{id}"))).map(|(id, _, _)| id.to_string())
}

/// The chords that run an action: user bindings plus its default (unless that chord was rebound).
pub fn chords_for(app: &IncaApp, id: &str) -> Vec<String> {
    let mut out: Vec<String> = app.keymap.iter().filter(|(c, a)| *a == id && !c.starts_with("unbind:")).map(|(c, _)| c.clone()).collect();
    let def = crate::actions::hotkey_label(id);
    if !def.is_empty() && !app.keymap.contains_key(&format!("unbind:{id}")) && app.keymap.get(def).map_or(true, |a| a == id) {
        out.push(def.to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn binding_map(app: &IncaApp) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (id, _, hk) in crate::actions::ACTIONS {
        if !hk.is_empty() && !app.keymap.contains_key(&format!("unbind:{id}")) {
            map.insert((*hk).to_string(), (*id).to_string());
        }
    }
    for (c, a) in &app.keymap {
        if !c.starts_with("unbind:") {
            map.insert(c.clone(), a.clone());
        }
    }
    map
}

fn category_of(id: &str) -> &'static str {
    let l = id.to_ascii_lowercase();
    let starts = |s: &str| l.starts_with(s);
    if l.ends_with("window") || l.ends_with("editor") || matches!(l.as_str(), "hypershade" | "grapheditor" | "dopesheet" | "renderview" | "uveditor" | "preferences" | "hotkeylist" | "about") {
        "Windows"
    } else if starts("new") || starts("open") || starts("save") || starts("import") || starts("export") || starts("reference") || starts("project") || starts("quit") || starts("revert") {
        "File"
    } else if starts("undo") || starts("redo") || starts("repeat") || starts("delete") || starts("duplicate") || starts("group") || starts("ungroup") || starts("parent") || starts("unparent") || starts("copy") || starts("cut") || starts("paste") || starts("rename") || starts("searchreplace") || starts("prefix") {
        "Edit"
    } else if starts("poly") || starts("nurbs") || starts("create") || starts("cvcurve") || starts("epcurve") || starts("camera") || l.ends_with("light") {
        "Create"
    } else if starts("select") || starts("deselect") || starts("invert") || starts("grow") || starts("shrink") || starts("tovertices") || starts("toedges") || starts("tofaces") || starts("objectmode") || starts("vertexmode") || starts("edgemode") || starts("facemode") || starts("uvmode") {
        "Select"
    } else if l.ends_with("tool") || starts("pivot") || starts("soft") || starts("manip") {
        "Tools"
    } else if starts("reset") || starts("freeze") || starts("center") || starts("match") || starts("snap") || starts("convert") {
        "Modify"
    } else if starts("hide") || starts("show") || starts("template") || starts("untemplate") || starts("smooth") || starts("shade") || starts("toggle") || starts("isolate") || starts("frame") || starts("hud") || starts("layout") {
        "Display"
    } else if starts("setkey") || starts("play") || starts("stop") || starts("next") || starts("prev") || starts("goto") || starts("key") || starts("bake") || starts("tan") {
        "Animation"
    } else if starts("render") || starts("ipr") || starts("assign") || starts("material") {
        "Rendering"
    } else if starts("revolve") || starts("loft") || starts("extrudecurve") || starts("planar") || starts("rebuildcurve") || starts("reversecurve") || starts("openclosecurve") {
        "Curves / Surfaces"
    } else if starts("extrude") || starts("bevel") || starts("chamfer") || starts("bridge") || starts("adddivisions") || starts("merge") || starts("collapse") || starts("poke") || starts("triangulate") || starts("quadrangulate") || starts("fillhole") || starts("mirror") || starts("combine") || starts("separate") || starts("extract") || starts("cleanup") || starts("connect") || starts("soften") || starts("harden") || starts("planarmap") || starts("automap") {
        "Mesh"
    } else {
        "Other"
    }
}

/// Shown when the pointer is outside the keyboard, not in the small gaps between keys.
const KEY_HELP: &str = "Hover over a key to see its command. Toggle Ctrl / Alt / Shift to view those layers.";

/// Main-menu order first, then the modeling menu-set groups, then animation/rendering.
const CATEGORY_ORDER: &[&str] = &["File", "Edit", "Create", "Select", "Modify", "Display", "Windows", "Mesh", "Tools", "Curves / Surfaces", "Animation", "Rendering", "Other"];

fn category_rank(c: &str) -> usize {
    CATEGORY_ORDER.iter().position(|x| *x == c).unwrap_or(CATEGORY_ORDER.len())
}

fn command_label(id: &str) -> String {
    if let Some(g) = inca_core::primitives::GEN_INFO.iter().find(|g| g.ty == id) {
        format!("{} {}", if g.nurbs { "NURBS" } else { "Polygon" }, g.label)
    } else {
        crate::actions::label(id)
    }
}

fn all_commands() -> Vec<(&'static str, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (id, label, _) in crate::actions::ACTIONS {
        seen.insert(*id);
        out.push((*id, label.to_string()));
    }
    for g in inca_core::primitives::GEN_INFO {
        if seen.insert(g.ty) {
            out.push((g.ty, command_label(g.ty)));
        }
    }
    for (id, _) in inca_core::scene::LIGHT_TYPES {
        if seen.insert(*id) {
            out.push((*id, command_label(id)));
        }
    }
    out
}

fn command_description(id: &str) -> Option<String> {
    if let Some(desc) = crate::hotkey_help::describe(id) {
        return Some(desc.to_string());
    }
    if let Some(g) = inca_core::primitives::GEN_INFO.iter().find(|g| g.ty == id) {
        return Some(format!("Creates a {} {}.", if g.nurbs { "NURBS" } else { "polygon" }, g.label.to_lowercase()));
    }
    inca_core::scene::LIGHT_TYPES.iter().find(|(ty, _)| *ty == id).map(|(_, label)| format!("Creates a {}.", label.to_lowercase()))
}

fn start_capture(app: &mut IncaApp) {
    if app.active_keymap == 0 {
        app.hk.status = "Inca Default is read-only. Create a new hotkey set to edit shortcuts.".into();
        return;
    }
    if app.hk.selected.is_some() && !app.hk.capturing {
        app.hk.capturing = true;
        app.hk.pending = None;
        app.hk.restore_confirm = false;
        app.hk.status = "Press a key combination for the selected command… (Esc cancels)".into();
    }
}

fn assign_key(app: &mut IncaApp, c: &str, id: &str) {
    app.keymap.insert(c.to_string(), id.to_string());
}

fn override_pending(app: &mut IncaApp) {
    let Some(p) = app.hk.pending.take() else { return };
    if app.keymap.get(&p.chord).is_some_and(|a| a == &p.existing) {
        app.keymap.remove(&p.chord);
    }
    if crate::actions::hotkey_label(&p.existing) == p.chord {
        app.keymap.insert(format!("unbind:{}", p.existing), p.existing.clone());
    }
    assign_key(app, &p.chord, &p.action);
    app.hk.status = format!("Assigned {} to {}.", p.chord, command_label(&p.action));
}

fn remove_hotkeys(app: &mut IncaApp) {
    if app.active_keymap == 0 {
        return;
    }
    let Some(id) = app.hk.selected.clone() else { return };
    app.keymap.retain(|_, a| a != &id);
    if !crate::actions::hotkey_label(&id).is_empty() {
        app.keymap.insert(format!("unbind:{id}"), id.clone());
    }
    app.hk.pending = None;
    app.hk.status = format!("Removed all hotkeys from {}.", command_label(&id));
}

fn restore_defaults(app: &mut IncaApp) {
    if app.active_keymap == 0 {
        return;
    }
    app.keymap.clear();
    app.hk.pending = None;
    app.hk.restore_confirm = false;
    app.hk.status = "Default hotkeys restored.".into();
}

fn sync_active_set(app: &mut IncaApp) {
    if app.active_keymap > 0 && app.active_keymap <= app.keymaps.len() {
        app.keymaps[app.active_keymap - 1].map = app.keymap.clone();
    }
}

fn reset_edit_state(app: &mut IncaApp) {
    app.hk.capturing = false;
    app.hk.pending = None;
    app.hk.restore_confirm = false;
    app.hk.restore_layers = None;
    app.hk.naming = None;
    app.hk.name_focus = false;
    app.hk.delete_confirm = false;
}

fn clear_modifier_layers(app: &mut IncaApp) {
    app.hk.layer_ctrl = false;
    app.hk.layer_shift = false;
    app.hk.layer_alt = false;
    app.hk.restore_layers = None;
}

fn focus_command(app: &mut IncaApp, id: &str) {
    let chords = chords_for(app, id);
    if chords.is_empty() {
        clear_modifier_layers(app);
        return;
    }
    let current = (app.hk.layer_ctrl, app.hk.layer_shift, app.hk.layer_alt);
    if chords.iter().any(|c| chord_layer(c) == current) {
        return;
    }
    if let Some(best) = chords.iter().min_by_key(|c| chord_pref(c.as_str())) {
        if app.hk.restore_layers.is_none() {
            app.hk.restore_layers = Some(current);
        }
        let (ctrl, shift, alt) = chord_layer(best);
        app.hk.layer_ctrl = ctrl;
        app.hk.layer_shift = shift;
        app.hk.layer_alt = alt;
    }
}

fn activate_set(app: &mut IncaApp, idx: usize) {
    let idx = idx.min(app.keymaps.len());
    if idx == app.active_keymap {
        return;
    }
    sync_active_set(app);
    app.active_keymap = idx;
    app.keymap = if idx == 0 { Keymap::new() } else { app.keymaps[idx - 1].map.clone() };
    reset_edit_state(app);
    app.hk.status.clear();
}

fn create_set(app: &mut IncaApp, name: String, map: Keymap) {
    sync_active_set(app);
    app.keymaps.push(KeymapSet { name: name.clone(), map: map.clone() });
    app.active_keymap = app.keymaps.len();
    app.keymap = map;
    reset_edit_state(app);
    app.hk.status = format!("Created hotkey set \"{name}\".");
}

fn rename_set(app: &mut IncaApp, name: String) {
    if app.active_keymap == 0 || app.active_keymap > app.keymaps.len() {
        return;
    }
    app.keymaps[app.active_keymap - 1].name = name.clone();
    app.hk.naming = None;
    app.hk.name_focus = false;
    app.hk.status = format!("Renamed hotkey set to \"{name}\".");
}

fn delete_set(app: &mut IncaApp) {
    if app.active_keymap == 0 || app.active_keymap > app.keymaps.len() {
        return;
    }
    let name = app.keymaps.remove(app.active_keymap - 1).name;
    app.active_keymap = 0;
    app.keymap = Keymap::new();
    reset_edit_state(app);
    app.hk.status = format!("Deleted hotkey set \"{name}\". Using Inca Default.");
}

fn submit_chord(app: &mut IncaApp, c: String, id: String) {
    if app.active_keymap == 0 {
        app.hk.capturing = false;
        app.hk.status = "Inca Default is read-only. Create a new hotkey set to edit shortcuts.".into();
        return;
    }
    app.hk.capturing = false;
    match bound_action(app, &c) {
        Some(existing) if existing == id => {
            app.hk.status = format!("{c} is already assigned to this command.");
        }
        Some(existing) => {
            app.hk.status = format!("{c} is assigned to {}.", command_label(&existing));
            app.hk.pending = Some(Pending { chord: c, action: id, existing });
        }
        None => {
            assign_key(app, &c, &id);
            app.hk.status = format!("Assigned {c} to {}.", command_label(&id));
        }
    }
}

fn capture(app: &mut IncaApp, ctx: &egui::Context) {
    if !app.hk.capturing {
        return;
    }
    if let Some(f) = ctx.memory(|m| m.focused()) {
        ctx.memory_mut(|m| m.surrender_focus(f));
        ctx.memory_mut(|m| m.stop_text_input());
    }
    let pressed = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } => Some((*key, *modifiers)),
            _ => None,
        })
    });
    let Some((key, m)) = pressed else { return };
    if key == Key::Escape && !m.ctrl && !m.command && !m.alt && !m.shift {
        app.hk.capturing = false;
        app.hk.status = "Assignment cancelled.".into();
        return;
    }
    let Some(id) = app.hk.selected.clone() else {
        app.hk.capturing = false;
        return;
    };
    submit_chord(app, chord(key, m), id);
}

#[derive(Clone)]
struct KeySpec {
    label: String,
    key: String,
    w: f32,
    modifier: Option<&'static str>,
    gap: bool,
}

impl KeySpec {
    fn k(label: impl Into<String>, key: impl Into<String>, w: f32) -> Self {
        KeySpec { label: label.into(), key: key.into(), w, modifier: None, gap: false }
    }
    fn m(label: &'static str, modifier: &'static str, w: f32) -> Self {
        KeySpec { label: label.into(), key: String::new(), w, modifier: Some(modifier), gap: false }
    }
    fn blank(label: impl Into<String>, w: f32) -> Self {
        KeySpec { label: label.into(), key: String::new(), w, modifier: None, gap: false }
    }
    fn gap(w: f32) -> Self {
        KeySpec { label: String::new(), key: String::new(), w, modifier: None, gap: true }
    }
}

fn keyboard_rows() -> (Vec<Vec<KeySpec>>, Vec<Vec<KeySpec>>) {
    let mut f = vec![KeySpec::k("Esc", "Escape", 1.0), KeySpec::gap(0.5)];
    for i in 1..=4 {
        let s = format!("F{i}");
        f.push(KeySpec::k(&s, &s, 1.0));
    }
    f.push(KeySpec::gap(0.25));
    for i in 5..=8 {
        let s = format!("F{i}");
        f.push(KeySpec::k(&s, &s, 1.0));
    }
    f.push(KeySpec::gap(0.25));
    for i in 9..=12 {
        let s = format!("F{i}");
        f.push(KeySpec::k(&s, &s, 1.0));
    }
    let digits = ["`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "="];
    let dkeys = ["Backtick", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "Minus", "Equals"];
    let mut r1: Vec<KeySpec> = digits.iter().zip(dkeys.iter()).map(|(l, k)| KeySpec::k(*l, *k, 1.0)).collect();
    r1.push(KeySpec::k("Bksp", "Backspace", 2.0));

    let letters = "QWERTYUIOP";
    let mut r2 = vec![KeySpec::k("Tab", "Tab", 1.5)];
    r2.extend(letters.chars().map(|c| { let s = c.to_string(); KeySpec::k(&s, &s, 1.0) }));
    r2.push(KeySpec::k("[", "OpenBracket", 1.0));
    r2.push(KeySpec::k("]", "CloseBracket", 1.0));
    r2.push(KeySpec::k("\\", "Backslash", 1.5));

    let mut r3 = vec![KeySpec::blank("Caps", 1.75)];
    r3.extend("ASDFGHJKL".chars().map(|c| { let s = c.to_string(); KeySpec::k(&s, &s, 1.0) }));
    r3.push(KeySpec::k(";", "Semicolon", 1.0));
    r3.push(KeySpec::k("'", "Quote", 1.0));
    r3.push(KeySpec::k("Enter", "Enter", 2.25));

    let mut r4 = vec![KeySpec::m("Shift", "Shift", 2.25)];
    r4.extend("ZXCVBNM".chars().map(|c| { let s = c.to_string(); KeySpec::k(&s, &s, 1.0) }));
    r4.push(KeySpec::k(",", "Comma", 1.0));
    r4.push(KeySpec::k(".", "Period", 1.0));
    r4.push(KeySpec::k("/", "Slash", 1.0));
    r4.push(KeySpec::m("Shift", "Shift", 2.75));

    let r5 = vec![
        KeySpec::m("Ctrl", "Ctrl", 1.5),
        KeySpec::blank("", 1.25),
        KeySpec::m("Alt", "Alt", 1.25),
        KeySpec::k("Space", "Space", 6.25),
        KeySpec::m("Alt", "Alt", 1.25),
        KeySpec::blank("", 1.25),
        KeySpec::m("Ctrl", "Ctrl", 1.5),
    ];

    let nav = vec![
        vec![KeySpec::blank("PrtSc", 1.0), KeySpec::blank("ScrLk", 1.0), KeySpec::blank("Pause", 1.0)],
        vec![KeySpec::k("Ins", "Insert", 1.0), KeySpec::k("Home", "Home", 1.0), KeySpec::k("PgUp", "PageUp", 1.0)],
        vec![KeySpec::k("Del", "Delete", 1.0), KeySpec::k("End", "End", 1.0), KeySpec::k("PgDn", "PageDown", 1.0)],
        vec![KeySpec::blank("", 1.0), KeySpec::blank("", 1.0), KeySpec::blank("", 1.0)],
        vec![KeySpec::blank("", 1.0), KeySpec::k("↑", "Up", 1.0), KeySpec::blank("", 1.0)],
        vec![KeySpec::k("←", "Left", 1.0), KeySpec::k("↓", "Down", 1.0), KeySpec::k("→", "Right", 1.0)],
    ];
    (vec![f, r1, r2, r3, r4, r5], nav)
}

fn shifted_label(label: &str, shift: bool) -> String {
    if !shift || label.chars().count() != 1 {
        return label.to_string();
    }
    match label {
        "`" => "~",
        "1" => "!",
        "2" => "@",
        "3" => "#",
        "4" => "$",
        "5" => "%",
        "6" => "^",
        "7" => "&",
        "8" => "*",
        "9" => "(",
        "0" => ")",
        "-" => "_",
        "=" => "+",
        "[" => "{",
        "]" => "}",
        "\\" => "|",
        ";" => ":",
        "'" => "\"",
        "," => "<",
        "." => ">",
        "/" => "?",
        other => other,
    }
    .to_string()
}

enum KeyClick {
    Modifier(&'static str),
    Action(String),
    Hint { action: String, chord: String },
}

#[allow(clippy::too_many_arguments)]
fn paint_key(
    painter: &egui::Painter,
    rect: Rect,
    spec: &KeySpec,
    unit: f32,
    bindings: &BTreeMap<String, String>,
    selected_chords: &HashSet<String>,
    layers: (bool, bool, bool),
    hover: Option<Pos2>,
    click: Option<Pos2>,
    info: &mut String,
    clicked: &mut Option<KeyClick>,
) {
    let combo = if spec.key.is_empty() { String::new() } else { layer_chord(&spec.key, layers.0, layers.1, layers.2) };
    let bound = bindings.get(&combo).cloned();
    let hint = if bound.is_none() && !spec.key.is_empty() {
        bindings.iter()
            .filter(|(c, _)| c.rsplit('+').next() == Some(spec.key.as_str()))
            .min_by_key(|(c, _)| chord_pref(c))
            .map(|(c, a)| (c.clone(), a.clone()))
    } else {
        None
    };
    let hint_blue = Color32::from_rgb(0x9e, 0xba, 0xc8);
    let (fill, text) = if let Some(m) = spec.modifier {
        let on = match m {
            "Ctrl" => layers.0,
            "Shift" => layers.1,
            "Alt" => layers.2,
            _ => false,
        };
        if on {
            (Color32::from_rgb(0xc5, 0x8b, 0x3a), Color32::WHITE)
        } else {
            (Color32::from_rgb(0x6a, 0x6a, 0x6a), Color32::WHITE)
        }
    } else if spec.key.is_empty() {
        (Color32::from_rgb(0x48, 0x48, 0x48), Color32::from_gray(0x77))
    } else if bound.is_some() {
        if selected_chords.contains(&combo) {
            (Color32::from_rgb(0xc5, 0x8b, 0x3a), Color32::WHITE)
        } else {
            (Color32::from_rgb(0x4f, 0x7d, 0x99), Color32::WHITE)
        }
    } else if hint.is_some() {
        (hint_blue, Color32::from_gray(0x1b))
    } else {
        (Color32::from_rgb(0x5a, 0x5a, 0x5a), Color32::from_gray(0xdd))
    };
    painter.rect_filled(rect, 3.0, fill);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_rgb(0x26, 0x26, 0x26)), StrokeKind::Inside);
    let label = shifted_label(&spec.label, layers.1);
    if matches!(label.as_str(), "↑" | "↓" | "←" | "→") {
        let c = rect.center();
        let s = unit * 0.22;
        let t = unit * 0.10;
        let points = match label.as_str() {
            "↑" => vec![pos2(c.x, c.y - s), pos2(c.x - s, c.y + t), pos2(c.x + s, c.y + t)],
            "↓" => vec![pos2(c.x, c.y + s), pos2(c.x - s, c.y - t), pos2(c.x + s, c.y - t)],
            "←" => vec![pos2(c.x - s, c.y), pos2(c.x + t, c.y - s), pos2(c.x + t, c.y + s)],
            _ => vec![pos2(c.x + s, c.y), pos2(c.x - t, c.y - s), pos2(c.x - t, c.y + s)],
        };
        painter.add(egui::Shape::convex_polygon(points, text, Stroke::NONE));
    } else if !label.is_empty() {
        let font_size = if label.chars().count() > 3 { (unit * 0.28).clamp(8.0, 12.0) } else { (unit * 0.34).clamp(8.0, 14.0) };
        painter.text(rect.left_top() + vec2(3.0, 2.0), Align2::LEFT_TOP, label, FontId::proportional(font_size), text);
    }
    if hover.is_some_and(|p| rect.contains(p)) {
        painter.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_rgb(0xe6, 0xc5, 0x4a)), StrokeKind::Outside);
        *info = if let Some(m) = spec.modifier {
            format!("{} modifier layer", m)
        } else if spec.key.is_empty() {
            format!("{}", spec.label)
        } else if let Some(a) = &bound {
            format!("{combo}  ->  {}", command_label(a))
        } else if let Some((c, a)) = &hint {
            format!("{c}  ->  {}  (other modifier layer)", command_label(a))
        } else {
            format!("{combo}  —  no command assigned")
        };
    }
    if click.is_some_and(|p| rect.contains(p)) {
        if let Some(m) = spec.modifier {
            *clicked = Some(KeyClick::Modifier(m));
        } else if let Some(a) = bound {
            *clicked = Some(KeyClick::Action(a));
        } else if let Some((c, a)) = &hint {
            *clicked = Some(KeyClick::Hint { action: a.clone(), chord: c.clone() });
        }
    }
}

fn keyboard(app: &mut IncaApp, ui: &mut egui::Ui) -> String {
    let mut bindings = binding_map(app);
    // The keyboard follows the category filter, like the command list.
    if !app.hk.category.is_empty() {
        bindings.retain(|_, action| category_of(action) == app.hk.category);
    }
    let selected_chords: HashSet<String> = app.hk.selected.as_ref()
        .filter(|id| app.hk.category.is_empty() || category_of(id) == app.hk.category)
        .map(|id| chords_for(app, id).into_iter().collect())
        .unwrap_or_default();
    let (main, nav) = keyboard_rows();
    let unit = ((ui.available_width() - 12.0) / 19.2).clamp(18.0, 46.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(unit * 19.2, unit * 6.85), Sense::click());
    let hover = ui.input(|i| i.pointer.hover_pos());
    let click = resp.interact_pointer_pos().filter(|_| resp.clicked());
    let mods = ui.input(|i| i.modifiers);
    // Don't preview layers while the user is typing in a text field.
    let text_focused = ui.ctx().memory(|m| m.focused()).is_some_and(|id| egui::text_edit::TextEditState::load(ui.ctx(), id).is_some());
    // Holding a modifier previews that layer; the checkboxes themselves stay unchanged.
    let layers = (
        app.hk.layer_ctrl || (!text_focused && (mods.ctrl || mods.command)),
        app.hk.layer_shift || (!text_focused && mods.shift),
        app.hk.layer_alt || (!text_focused && mods.alt),
    );
    let drop_payload = resp.dnd_hover_payload::<String>();
    let drop_pos = drop_payload.as_ref().and_then(|_| ui.input(|i| i.pointer.interact_pos()));
    let mut drop_key = None;
    let mut info = String::new();
    let mut clicked = None;
    let painter = ui.painter();
    let x0 = rect.min.x + 5.0;
    let y0 = rect.min.y + 5.0;
    for (row, keys) in main.iter().enumerate() {
        let y = y0 + (row as f32 + if row > 0 { 0.35 } else { 0.0 }) * unit;
        let mut x = x0;
        for spec in keys {
            if spec.gap {
                x += spec.w * unit;
                continue;
            }
            let kr = Rect::from_min_size(pos2(x, y), vec2((spec.w * unit - 2.0).max(4.0), unit - 3.0));
            paint_key(&painter, kr, spec, unit, &bindings, &selected_chords, layers, hover, click, &mut info, &mut clicked);
            if drop_pos.is_some_and(|p| kr.contains(p)) && spec.modifier.is_none() && !spec.key.is_empty() {
                painter.rect_stroke(kr, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
                drop_key = Some(spec.key.clone());
            }
            x += spec.w * unit;
        }
    }
    let nav_x = x0 + 15.45 * unit;
    for (row, keys) in nav.iter().enumerate() {
        let y = y0 + (row as f32 + if row > 0 { 0.35 } else { 0.0 }) * unit;
        for (col, spec) in keys.iter().enumerate() {
            if spec.label.is_empty() {
                continue;
            }
            let kr = Rect::from_min_size(pos2(nav_x + col as f32 * unit, y), vec2(unit - 2.0, unit - 3.0));
            paint_key(&painter, kr, spec, unit, &bindings, &selected_chords, layers, hover, click, &mut info, &mut clicked);
            if drop_pos.is_some_and(|p| kr.contains(p)) && spec.modifier.is_none() && !spec.key.is_empty() {
                painter.rect_stroke(kr, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
                drop_key = Some(spec.key.clone());
            }
        }
    }
    if let (Some(action), Some(key)) = (&drop_payload, &drop_key) {
        let c = layer_chord(key, layers.0 || mods.ctrl || mods.command, layers.1 || mods.shift, layers.2 || mods.alt);
        info = format!("Release to assign {} to {c}.", command_label(action.as_str()));
    }
    if info.is_empty() && !hover.is_some_and(|p| rect.contains(p)) {
        info = KEY_HELP.to_string();
    }
    if let Some(action) = resp.dnd_release_payload::<String>() {
        if let Some(key) = drop_key {
            let ctrl = layers.0 || mods.ctrl || mods.command;
            let shift = layers.1 || mods.shift;
            let alt = layers.2 || mods.alt;
            if app.hk.restore_layers.is_none() && ((ctrl && !app.hk.layer_ctrl) || (shift && !app.hk.layer_shift) || (alt && !app.hk.layer_alt)) {
                app.hk.restore_layers = Some((app.hk.layer_ctrl, app.hk.layer_shift, app.hk.layer_alt));
            }
            app.hk.layer_ctrl |= ctrl;
            app.hk.layer_shift |= shift;
            app.hk.layer_alt |= alt;
            app.hk.selected = Some((*action).clone());
            app.hk.pending = None;
            app.hk.status.clear();
            submit_chord(app, layer_chord(&key, ctrl, shift, alt), (*action).clone());
        }
    }
    if let Some(click) = clicked {
        match click {
            KeyClick::Modifier("Ctrl") => {
                app.hk.layer_ctrl = !app.hk.layer_ctrl;
                app.hk.restore_layers = None;
            }
            KeyClick::Modifier("Shift") => {
                app.hk.layer_shift = !app.hk.layer_shift;
                app.hk.restore_layers = None;
            }
            KeyClick::Modifier("Alt") => {
                app.hk.layer_alt = !app.hk.layer_alt;
                app.hk.restore_layers = None;
            }
            KeyClick::Modifier(_) => {}
            KeyClick::Action(id) => {
                // Clicking the same key again clears the selection.
                if app.hk.selected.as_deref() == Some(id.as_str()) {
                    app.hk.selected = None;
                    app.hk.reveal = false;
                    if let Some((ctrl, shift, alt)) = app.hk.restore_layers.take() {
                        app.hk.layer_ctrl = ctrl;
                        app.hk.layer_shift = shift;
                        app.hk.layer_alt = alt;
                    }
                } else {
                    app.hk.selected = Some(id);
                    app.hk.category.clear();
                    app.hk.filter.clear();
                    app.hk.reveal = true;
                }
                app.hk.capturing = false;
                app.hk.pending = None;
                app.hk.status.clear();
            }
            KeyClick::Hint { action, chord } => {
                let (ctrl, shift, alt) = chord_layer(&chord);
                if app.hk.restore_layers.is_none() {
                    app.hk.restore_layers = Some((app.hk.layer_ctrl, app.hk.layer_shift, app.hk.layer_alt));
                }
                app.hk.layer_ctrl = ctrl;
                app.hk.layer_shift = shift;
                app.hk.layer_alt = alt;
                app.hk.selected = Some(action);
                app.hk.filter.clear();
                app.hk.reveal = true;
                app.hk.capturing = false;
                app.hk.pending = None;
                app.hk.status.clear();
            }
        }
    }
    info
}

fn legend(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("■").color(Color32::from_rgb(0x4f, 0x7d, 0x99)));
        ui.label(RichText::new("Assigned").color(theme::TEXT_DIM));
        ui.label(RichText::new("■").color(Color32::from_rgb(0xc5, 0x8b, 0x3a)));
        ui.label(RichText::new("Selected command").color(theme::TEXT_DIM));
        ui.label(RichText::new("■").color(Color32::from_rgb(0x5a, 0x5a, 0x5a)));
        ui.label(RichText::new("Free").color(theme::TEXT_DIM));
        ui.label(RichText::new("■").color(Color32::from_rgb(0x9e, 0xba, 0xc8)));
        ui.label(RichText::new("Other modifier layer").color(theme::TEXT_DIM));
    });
}

fn command_details(app: &IncaApp, ui: &mut egui::Ui) {
    ui.label(RichText::new("Drag a command from the list onto a key to assign it.").color(theme::TEXT_DIM));
    let Some(id) = &app.hk.selected else { return };
    let keys = chords_for(app, id);
    ui.label(RichText::new(if keys.is_empty() { "No hotkey assigned".to_string() } else { format!("Hotkey: {}", keys.join(", ")) }).monospace().color(if keys.is_empty() { theme::TEXT_DIM } else { Color32::from_rgb(0x9f, 0xc6, 0xe0) }));
    ui.label(RichText::new(category_of(id)).color(theme::TEXT_DIM));
    if let Some(desc) = command_description(id) {
        ui.label(RichText::new(desc).color(theme::TEXT_DIM));
    }
    ui.label(RichText::new(command_label(id)).strong().size(15.0));
    ui.separator();
}

fn commit_name(app: &mut IncaApp, mode: NameMode) {
    let name = app.hk.name_buf.trim().to_string();
    if name.is_empty() {
        app.hk.status = "Enter a name for the hotkey set.".into();
        return;
    }
    if name.eq_ignore_ascii_case("Inca Default") {
        app.hk.status = "\"Inca Default\" is a reserved name.".into();
        return;
    }
    let skip = if mode == NameMode::Rename { app.active_keymap } else { 0 };
    if app.keymaps.iter().enumerate().any(|(i, s)| i + 1 != skip && s.name.eq_ignore_ascii_case(&name)) {
        app.hk.status = format!("A hotkey set named \"{name}\" already exists.");
        return;
    }
    match mode {
        NameMode::New => create_set(app, name, Keymap::new()),
        NameMode::Duplicate => {
            sync_active_set(app);
            let map = app.keymap.clone();
            create_set(app, name, map);
        }
        NameMode::Rename => rename_set(app, name),
    }
}

fn unique_set_name(app: &IncaApp, base: &str) -> String {
    let base = base.trim();
    let base = if base.is_empty() || base.eq_ignore_ascii_case("Inca Default") { "Imported" } else { base };
    if !app.keymaps.iter().any(|s| s.name.eq_ignore_ascii_case(base)) {
        return base.to_string();
    }
    let mut i = 2;
    loop {
        let candidate = format!("{base} ({i})");
        if !app.keymaps.iter().any(|s| s.name.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
        i += 1;
    }
}

fn save_keymaps_now(app: &mut IncaApp) {
    match save_keymaps_file(app) {
        Ok(()) => app.hk.status = "Hotkey sets saved.".into(),
        Err(e) => app.hk.status = format!("Save failed: {e}"),
    }
}

fn known_keymap(map: Keymap) -> (Keymap, usize) {
    let mut dropped = 0;
    let map = map
        .into_iter()
        .filter(|(_, action)| {
            let id = action.strip_prefix("unbind:").unwrap_or(action);
            let known = crate::actions::available(id);
            if !known {
                dropped += 1;
            }
            known
        })
        .collect();
    (map, dropped)
}

fn import_keymap_file(app: &mut IncaApp) {
    let Some(path) = rfd::FileDialog::new().add_filter("Inca hotkeys", &["json"]).pick_file() else { return };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            app.hk.status = format!("Import failed: {e}");
            return;
        }
    };
    let parsed: KeymapImport = match serde_json::from_str(&text) {
        Ok(parsed) => parsed,
        Err(e) => {
            app.hk.status = format!("Import failed: {e}");
            return;
        }
    };
    let sets = match parsed {
        KeymapImport::One(set) => vec![set],
        KeymapImport::Many(file) => file.sets,
    };
    if sets.is_empty() {
        app.hk.status = "That file has no hotkey sets.".into();
        return;
    }
    let count = sets.len();
    let mut last = String::new();
    let mut dropped = 0;
    for set in sets {
        let name = unique_set_name(app, &set.name);
        last = name.clone();
        let (map, lost) = known_keymap(set.map);
        dropped += lost;
        create_set(app, name, map);
    }
    app.hk.status = match (count, dropped) {
        (1, 0) => format!("Imported \"{last}\"."),
        (1, n) => format!("Imported \"{last}\"; skipped {n} unknown command(s)."),
        (n, 0) => format!("Imported {n} hotkey sets."),
        (n, dropped) => format!("Imported {n} hotkey sets; skipped {dropped} unknown command(s)."),
    };
}

fn export_keymap_file(app: &mut IncaApp) {
    if app.active_keymap == 0 || app.active_keymap > app.keymaps.len() {
        return;
    }
    sync_active_set(app);
    let set = app.keymaps[app.active_keymap - 1].clone();
    let mut file: String = set.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    file = file.trim().to_string();
    if file.is_empty() {
        file = "inca-hotkeys".into();
    }
    let Some(path) = rfd::FileDialog::new().add_filter("Inca hotkeys", &["json"]).set_file_name(format!("{file}.json")).save_file() else { return };
    let json = match serde_json::to_string_pretty(&set) {
        Ok(json) => json,
        Err(e) => {
            app.hk.status = format!("Export failed: {e}");
            return;
        }
    };
    match std::fs::write(&path, json) {
        Ok(()) => app.hk.status = format!("Exported \"{}\".", set.name),
        Err(e) => app.hk.status = format!("Export failed: {e}"),
    }
}

fn keymap_bar(app: &mut IncaApp, ui: &mut egui::Ui) {
    let editable = app.active_keymap > 0 && app.active_keymap <= app.keymaps.len();
    ui.horizontal_wrapped(|ui| {
        ui.label("Hotkey set:");
        let selected = if editable { app.keymaps[app.active_keymap - 1].name.clone() } else { "Inca Default".to_string() };
        let sets: Vec<(usize, String)> = app.keymaps.iter().enumerate().map(|(i, s)| (i + 1, s.name.clone())).collect();
        egui::ComboBox::from_id_salt("hk_set").width(190.0).selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_label(app.active_keymap == 0, "Inca Default").clicked() {
                activate_set(app, 0);
            }
            for (idx, name) in sets {
                if ui.selectable_label(app.active_keymap == idx, name).clicked() {
                    activate_set(app, idx);
                }
            }
        });
        if ui.button("New...").on_hover_text("Create a set from Inca Default shortcuts").clicked() {
            app.hk.naming = Some(NameMode::New);
            app.hk.name_buf = format!("Custom {}", app.keymaps.len() + 1);
            app.hk.name_focus = true;
            app.hk.delete_confirm = false;
        }
        if ui.button("Duplicate").on_hover_text("Copy the current hotkey set").clicked() {
            app.hk.naming = Some(NameMode::Duplicate);
            app.hk.name_buf = if editable { format!("{} Copy", app.keymaps[app.active_keymap - 1].name) } else { format!("Custom {}", app.keymaps.len() + 1) };
            app.hk.name_focus = true;
            app.hk.delete_confirm = false;
        }
        if ui.add_enabled(editable, egui::Button::new("Rename...")).clicked() {
            app.hk.naming = Some(NameMode::Rename);
            app.hk.name_buf = app.keymaps[app.active_keymap - 1].name.clone();
            app.hk.name_focus = true;
            app.hk.delete_confirm = false;
        }
        if ui.add_enabled(editable, egui::Button::new("Delete")).clicked() {
            app.hk.delete_confirm = true;
            app.hk.naming = None;
        }
        ui.add_space(8.0);
        if ui.button("Save").on_hover_text("Save all hotkey sets to the Inca settings folder").clicked() {
            save_keymaps_now(app);
        }
        if ui.button("Import...").on_hover_text("Import a hotkey set from a JSON file").clicked() {
            import_keymap_file(app);
        }
        if ui.add_enabled(editable, egui::Button::new("Export...")).on_hover_text("Export the current set to a JSON file").clicked() {
            export_keymap_file(app);
        }
    });
    let del_name = if app.hk.delete_confirm && app.active_keymap > 0 {
        app.keymaps.get(app.active_keymap - 1).map(|s| {
            let mut n: String = s.name.chars().take(32).collect();
            if s.name.chars().count() > 32 {
                n.push_str("...");
            }
            n
        })
    } else {
        None
    };
    // One fixed-height slot for the read-only hint, naming row, or delete confirmation.
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 30.0), egui::Layout::left_to_right(Align::Center), |ui| {
        ui.set_height(30.0);
        let mut commit = false;
        if let Some(mode) = app.hk.naming {
            let r = ui.add(egui::TextEdit::singleline(&mut app.hk.name_buf).desired_width(220.0).hint_text("Hotkey set name"));
            if app.hk.name_focus {
                r.request_focus();
                app.hk.name_focus = false;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let label = match mode {
                NameMode::New => "Create",
                NameMode::Duplicate => "Duplicate",
                NameMode::Rename => "Rename",
            };
            if ui.button(label).clicked() || enter {
                commit = true;
            }
            if ui.button("Cancel").clicked() {
                app.hk.naming = None;
                app.hk.name_focus = false;
            }
            if commit {
                commit_name(app, mode);
            }
        } else if app.hk.delete_confirm {
            if let Some(name) = del_name {
                ui.label(RichText::new(format!("Delete \"{name}\"?")).color(Color32::from_rgb(0xe6, 0xc5, 0x4a)));
                if ui.button("Delete").clicked() {
                    delete_set(app);
                }
                if ui.button("Cancel").clicked() {
                    app.hk.delete_confirm = false;
                }
            } else {
                app.hk.delete_confirm = false;
            }
        } else if !editable {
            ui.label(RichText::new("Inca Default is read-only — create a new set to edit shortcuts.").color(theme::TEXT_DIM));
        }
    });
    ui.separator();
}

fn left_panel(app: &mut IncaApp, ui: &mut egui::Ui) {
    keymap_bar(app, ui);
    ui.horizontal(|ui| {
        ui.label("Modifier layer:");
        let mut changed = false;
        changed |= ui.checkbox(&mut app.hk.layer_ctrl, "Ctrl").changed();
        changed |= ui.checkbox(&mut app.hk.layer_alt, "Alt").changed();
        changed |= ui.checkbox(&mut app.hk.layer_shift, "Shift").changed();
        if changed {
            app.hk.restore_layers = None;
        }
    });
    let info = keyboard(app, ui);
    ui.label(RichText::new(if info.is_empty() { " " } else { &info }).color(theme::TEXT_DIM));
    legend(ui);
    ui.with_layout(egui::Layout::bottom_up(Align::Min), |ui| {
        command_details(app, ui);
    });
}

fn command_row(ui: &mut egui::Ui, id: &str, label: &str, keys: &str, sel: bool, editable: bool) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let sense = if editable { Sense::click_and_drag() } else { Sense::click() };
    let resp = ui.interact(rect, ui.make_persistent_id(("hk_cmd", id)), sense);
    if editable {
        resp.dnd_set_drag_payload(id.to_string());
    }
    if resp.dragged() {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            let layer = LayerId::new(Order::Tooltip, egui::Id::new("hk_cmd_ghost"));
            let painter = ui.ctx().layer_painter(layer);
            let galley = painter.layout_no_wrap(label.to_string(), FontId::proportional(13.0), Color32::WHITE);
            let r = Rect::from_min_size(p + vec2(16.0, 12.0), galley.size() + vec2(12.0, 8.0));
            painter.rect_filled(r, 4.0, Color32::from_rgb(0x33, 0x3f, 0x4a));
            painter.rect_stroke(r, 4.0, Stroke::new(1.0, Color32::from_rgb(0x8f, 0xb8, 0xd8)), StrokeKind::Inside);
            painter.galley(r.min + vec2(6.0, 4.0), galley, Color32::WHITE);
        }
    }
    if sel || resp.hovered() {
        let bg = if sel { ui.visuals().selection.bg_fill } else { ui.visuals().widgets.hovered.weak_bg_fill };
        ui.painter().rect_filled(rect, 0.0, bg);
    }
    let text_color = if sel { Color32::WHITE } else { ui.visuals().text_color() };
    ui.painter().text(rect.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, label, egui::TextStyle::Body.resolve(ui.style()), text_color);
    if keys.is_empty() {
        ui.painter().text(rect.right_center() - vec2(6.0, 0.0), Align2::RIGHT_CENTER, "—", egui::TextStyle::Body.resolve(ui.style()), theme::TEXT_DIM);
    } else {
        let color = if sel { Color32::WHITE } else { Color32::from_rgb(0x9f, 0xc6, 0xe0) };
        ui.painter().text(rect.right_center() - vec2(6.0, 0.0), Align2::RIGHT_CENTER, keys, egui::TextStyle::Monospace.resolve(ui.style()), color);
    }
    resp
}

fn right_panel(app: &mut IncaApp, ui: &mut egui::Ui) {
    let editable = app.active_keymap > 0;
    let commands = all_commands();
    let cats: Vec<&'static str> = {
        let mut s = BTreeSet::new();
        for (id, _) in &commands {
            s.insert(category_of(id));
        }
        let mut cats: Vec<&'static str> = s.into_iter().collect();
        cats.sort_by(|a, b| category_rank(a).cmp(&category_rank(b)).then(a.cmp(b)));
        cats
    };
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 170.0).max(80.0);
        ui.add(egui::TextEdit::singleline(&mut app.hk.filter).hint_text("Search commands or keys…").desired_width(width));
        let clear = ui.add_visible(!app.hk.filter.is_empty(), egui::Button::new("×").small()).on_hover_text("Clear search");
        if clear.clicked() {
            app.hk.filter.clear();
        }
        egui::ComboBox::from_id_salt("hk_category").width(130.0).selected_text(if app.hk.category.is_empty() { "All categories" } else { app.hk.category.as_str() }).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.hk.category, String::new(), "All categories");
            for c in &cats {
                ui.selectable_value(&mut app.hk.category, (*c).to_string(), *c);
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Command").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Hotkey").color(theme::TEXT_DIM));
        });
    });
    ui.separator();
    let q = app.hk.filter.trim().to_lowercase();
    let cat_filter = app.hk.category.clone();
    let mut groups: BTreeMap<&'static str, Vec<(&'static str, String, String)>> = BTreeMap::new();
    for (id, label) in &commands {
        let cat = category_of(id);
        if !cat_filter.is_empty() && cat != cat_filter {
            continue;
        }
        let keys = chords_for(app, id);
        let hit = q.is_empty() || label.to_lowercase().contains(&q) || id.to_lowercase().contains(&q) || cat.to_lowercase().contains(&q) || keys.iter().any(|k| k.to_lowercase().contains(&q));
        if !hit {
            continue;
        }
        groups.entry(cat).or_default().push((id, label.clone(), keys.join(", ")));
    }
    let mut groups: Vec<(&'static str, Vec<(&'static str, String, String)>)> = groups.into_iter().collect();
    groups.sort_by(|(a, _), (b, _)| category_rank(a).cmp(&category_rank(b)).then(a.cmp(b)));
    let list_h = (ui.available_height() - 54.0).max(120.0);
    egui::ScrollArea::vertical().id_salt("hk_command_list").auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
        // Keep the floating scrollbar off the shortcut column.
        egui::Frame::NONE.inner_margin(egui::Margin { right: 20, ..Default::default() }).show(ui, |ui| {
            for (cat, mut items) in groups {
                items.sort_by(|a, b| a.1.cmp(&b.1));
                let reveal_here = app.hk.reveal && items.iter().any(|(id, _, _)| app.hk.selected.as_deref() == Some(*id));
                let header = format!("{} ({})", cat, items.len());
                egui::CollapsingHeader::new(RichText::new(header).strong())
                    .id_salt(("hk_cat", cat))
                    .default_open(true)
                    .open(reveal_here.then_some(true))
                    .show(ui, |ui| {
                        for (id, label, keys) in items {
                            let sel = app.hk.selected.as_deref() == Some(id);
                            let resp = command_row(ui, id, &label, &keys, sel, editable).on_hover_text(id);
                            if resp.double_clicked() {
                                app.hk.selected = Some(id.to_string());
                                app.hk.pending = None;
                                focus_command(app, id);
                                start_capture(app);
                            } else if resp.clicked() {
                                if app.hk.selected.as_deref() == Some(id) {
                                    app.hk.selected = None;
                                    app.hk.reveal = false;
                                    clear_modifier_layers(app);
                                } else {
                                    app.hk.selected = Some(id.to_string());
                                    app.hk.pending = None;
                                    focus_command(app, id);
                                }
                            }
                            if sel && app.hk.reveal {
                                resp.scroll_to_me(Some(Align::Center));
                            }
                        }
                    });
            }
        });
    });
    app.hk.reveal = false;
    ui.separator();
    let selected = app.hk.selected.clone();
    let has_sel = selected.is_some();
    let has_keys = selected.as_ref().is_some_and(|id| !chords_for(app, id).is_empty());
    ui.horizontal(|ui| {
        let txt = if app.hk.capturing { "Press a key… (Esc cancels)" } else { "Assign New Hotkey" };
        if ui.add_enabled(has_sel && editable, egui::Button::new(txt)).clicked() {
            if app.hk.capturing {
                app.hk.capturing = false;
                app.hk.status = "Assignment cancelled.".into();
            } else {
                start_capture(app);
            }
        }
        if ui.add_enabled(has_keys && !app.hk.capturing && editable, egui::Button::new("Remove Hotkeys")).clicked() {
            remove_hotkeys(app);
        }
        if ui.add_enabled(!app.hk.capturing && editable, egui::Button::new("Restore Defaults")).clicked() {
            app.hk.restore_confirm = true;
        }
    });
}

fn status_bar(app: &mut IncaApp, ui: &mut egui::Ui) {
    ui.set_min_height(56.0);
    ui.separator();
    if let Some(p) = app.hk.pending.clone() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} is assigned to {}.", p.chord, command_label(&p.existing))).color(Color32::from_rgb(0xe6, 0xc5, 0x4a)));
            if ui.button("Override").clicked() {
                override_pending(app);
            }
            if ui.button("Cancel").clicked() {
                app.hk.pending = None;
                app.hk.status = "Assignment cancelled.".into();
            }
        });
    }
    if app.hk.restore_confirm {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Restore all hotkeys to their defaults? Custom hotkeys will be lost.").color(Color32::from_rgb(0xe6, 0xc5, 0x4a)));
            if ui.button("Restore").clicked() {
                restore_defaults(app);
            }
            if ui.button("Cancel").clicked() {
                app.hk.restore_confirm = false;
                app.hk.status = "Restore cancelled.".into();
            }
        });
    }
    if !app.hk.status.is_empty() {
        ui.label(RichText::new(&app.hk.status).color(theme::TEXT_DIM));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_chords_use_the_same_order_as_capture() {
        let m = Modifiers { ctrl: true, shift: true, ..Default::default() };
        assert_eq!(chord(Key::K, m), layer_chord("K", true, true, false));
        assert_eq!(chord_layer("Ctrl+Shift+K"), (true, true, false));
        assert_eq!(chord_layer("S"), (false, false, false));
        assert!(chord_pref("Z") < chord_pref("Ctrl+Z"));
        assert!(chord_pref("Ctrl+Z") < chord_pref("Alt+Z"));
        assert!(chord_pref("Ctrl+Z") < chord_pref("Ctrl+Shift+Z"));
        assert_eq!(shifted_label("1", true), "!");
        assert_eq!(shifted_label("Q", true), "Q");
    }

    #[test]
    fn categories_cover_the_command_list() {
        assert_eq!(category_of("newScene"), "File");
        assert_eq!(category_of("undo"), "Edit");
        assert_eq!(category_of("selectAll"), "Select");
        assert_eq!(category_of("polyCube"), "Create");
        assert_eq!(category_of("extrude"), "Mesh");
        assert_eq!(category_of("renderCurrent"), "Rendering");
        assert_eq!(category_of("hudPolyCount"), "Display");
        assert!(category_rank("File") < category_rank("Edit"));
        assert!(category_rank("Edit") < category_rank("Create"));
        assert!(category_rank("Windows") < category_rank("Mesh"));
        assert!(category_rank("Mesh") < category_rank("Animation"));
        assert!(category_rank("Animation") < category_rank("Other"));
    }

    #[test]
    fn keymap_export_and_save_formats_import() {
        let set = KeymapSet {
            name: "Test".into(),
            map: [("Ctrl+K".to_string(), "insertEdgeLoopTool".to_string())].into_iter().collect(),
        };
        let one = serde_json::to_string(&set).unwrap();
        assert!(matches!(serde_json::from_str::<KeymapImport>(&one), Ok(KeymapImport::One(s)) if s.name == "Test"));

        let file = KeymapFile { active: 1, sets: vec![set] };
        let many = serde_json::to_string(&file).unwrap();
        assert!(matches!(serde_json::from_str::<KeymapImport>(&many), Ok(KeymapImport::Many(f)) if f.active == 1 && f.sets.len() == 1));
    }

    #[test]
    fn primitive_and_light_commands_have_labels_and_descriptions() {
        assert_eq!(command_label("polyCube"), "Polygon Cube");
        assert_eq!(command_label("nurbsCube"), "NURBS Cube");
        assert_eq!(command_label("pointLight"), "Point Light");
        assert_eq!(command_description("polyCube").as_deref(), Some("Creates a polygon cube."));
        assert_eq!(command_description("pointLight").as_deref(), Some("Creates a point light."));
    }

    #[test]
    fn import_drops_unknown_command_ids() {
        let mut map = Keymap::new();
        map.insert("Ctrl+C".into(), "copy".into());
        map.insert("Ctrl+X".into(), "notARealAction".into());
        map.insert("unbind:copy".into(), "copy".into());
        let (map, dropped) = known_keymap(map);
        assert_eq!(dropped, 1);
        assert!(map.contains_key("Ctrl+C"));
        assert!(map.contains_key("unbind:copy"));
        assert!(!map.contains_key("Ctrl+X"));
    }
}

pub fn window(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.win.hotkeys;
    if !open {
        return;
    }
    capture(app, ctx);
    // A distinct id discards window state stored by older builds. That state had ratcheted to
    // the full screen, so a default size alone could not shrink it again.
    egui::Window::new("Hotkey Editor").id(egui::Id::new("HotkeyEditorWindow2")).open(&mut open).default_size([1280.0, 720.0]).min_size([900.0, 620.0]).resizable([true, false]).show(ctx, |ui| {
        // A stable content height: deriving it from ui.available_height() inside an auto-sized
        // window feeds the window size back into itself and makes it grow every frame.
        let height = (ctx.content_rect().height() * 0.55).clamp(520.0, 620.0);
        ui.horizontal_top(|ui| {
            // Compute both widths from the window, not from how wide the left content
            // happened to lay out; that made the right list jump when the details appeared.
            let total_w = ui.available_width();
            let gap = ui.spacing().item_spacing.x;
            let left_w = ((total_w - gap) * 0.69).clamp(520.0, 880.0);
            let right_w = (total_w - left_w - gap).max(300.0);
            let left = ui.allocate_ui_with_layout(vec2(left_w, height), egui::Layout::top_down(Align::Min), |ui| {
                ui.set_min_width(ui.available_width());
                left_panel(app, ui);
            });
            // A separator widget would take the window's whole available height and make the
            // window grow every frame; paint a fixed line instead.
            let x = left.response.rect.right() + ui.spacing().item_spacing.x * 0.5;
            ui.painter().vline(x, left.response.rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
            ui.allocate_ui_with_layout(vec2(right_w, height), egui::Layout::top_down(Align::Min), |ui| {
                ui.set_min_width(ui.available_width());
                right_panel(app, ui);
            });
        });
        // Fixed status height keeps conflict/confirm rows from resizing the window.
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 56.0), egui::Layout::top_down(Align::Min), |ui| {
            status_bar(app, ui);
        });
    });
    if !open {
        app.hk.capturing = false;
        app.hk.restore_layers = None;
    }
    app.win.hotkeys = open;
}
