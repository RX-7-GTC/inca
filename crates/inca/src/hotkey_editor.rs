//! Hotkey Editor: Maya-style keyboard picture plus a searchable command list.

use crate::app::IncaApp;
use crate::theme;
use egui::{pos2, vec2, Align, Align2, Color32, FontId, Key, LayerId, Modifiers, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// chord ("Ctrl+Shift+K") -> action id, in creation order; "unbind:<action>" entries switch an
/// action's default hotkey off. Editor-scoped keys are prefixed, e.g. "viewport:Ctrl+Shift+K".
pub type Keymap = indexmap::IndexMap<String, String>;

/// Where a binding applies. Application bindings always apply; editor scopes only while the
/// pointer is over that editor (same rule as the UV Editor's own keys).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Scope {
    #[default]
    Application,
    Viewport,
    Hypershade,
    UvEditor,
}

impl Scope {
    pub const ALL: [Scope; 4] = [Scope::Application, Scope::Viewport, Scope::Hypershade, Scope::UvEditor];

    pub fn label(self) -> &'static str {
        match self {
            Scope::Application => "Application",
            Scope::Viewport => "Viewport",
            Scope::Hypershade => "Hypershade",
            Scope::UvEditor => "UV Editor",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Scope::Application => "",
            Scope::Viewport => "viewport",
            Scope::Hypershade => "hypershade",
            Scope::UvEditor => "uv",
        }
    }
}

fn scope_key(scope: Scope, chord: &str) -> String {
    if scope == Scope::Application { chord.to_string() } else { format!("{}:{chord}", scope.prefix()) }
}

fn parse_key(key: &str) -> (Scope, &str) {
    for scope in [Scope::Viewport, Scope::Hypershade, Scope::UvEditor] {
        if let Some(rest) = key.strip_prefix(scope.prefix()).and_then(|r| r.strip_prefix(':')) {
            return (scope, rest);
        }
    }
    (Scope::Application, key)
}

/// The editor under the pointer; viewport panels are checked after the floating editors and
/// the Hotkey Editor itself.
pub fn active_scope(app: &IncaApp, ctx: &egui::Context) -> Scope {
    let Some(p) = ctx.pointer_hover_pos() else { return Scope::Application };
    if app.hk.window_rect.is_some_and(|r| r.contains(p)) {
        return Scope::Application;
    }
    if app.uv_editor.contains_pointer(p) {
        return Scope::UvEditor;
    }
    if app.win.hypershade && app.windows_state.hs.window_rect.is_some_and(|r| r.contains(p)) {
        return Scope::Hypershade;
    }
    if app.vps.iter().any(|v| v.rect.contains(p)) {
        return Scope::Viewport;
    }
    Scope::Application
}

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
    pub existing_release: bool,
    pub scope: Scope,
}

#[derive(Default)]
pub struct HkState {
    pub filter: String,
    pub category: String,
    /// editor scope the command list is editing
    pub scope: Scope,
    /// the Hotkey Editor window rect, so viewport scoped keys don't fire while editing
    pub window_rect: Option<Rect>,
    /// filter/category as of the last frame, for resetting the command list scroll
    pub filter_seen: String,
    pub category_seen: String,
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
    s += key.name();
    canonical_chord(&s)
}

fn canonical_part(part: &str) -> &str {
    match part {
        "ControlLeft" | "ControlRight" => "Ctrl",
        "ShiftLeft" | "ShiftRight" => "Shift",
        "AltLeft" | "AltRight" => "Alt",
        _ => part,
    }
}

/// Both Ctrl keys produce "Ctrl", both Shift keys "Shift", etc. Duplicate modifiers collapse,
/// so "Ctrl+ControlLeft" becomes "Ctrl" and "Ctrl+Shift+ShiftLeft" becomes "Ctrl+Shift".
fn canonical_chord(c: &str) -> String {
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    let mut key: Option<&str> = None;
    for part in c.split('+') {
        match canonical_part(part) {
            "Ctrl" => ctrl = true,
            "Shift" => shift = true,
            "Alt" => alt = true,
            other => key = Some(other),
        }
    }
    let mut out = String::new();
    if ctrl {
        out += "Ctrl+";
    }
    if shift {
        out += "Shift+";
    }
    if alt {
        out += "Alt+";
    }
    if let Some(k) = key {
        out += k;
    } else if out.ends_with('+') {
        out.pop();
    }
    out
}

fn is_canonical_modifier(part: &str) -> bool {
    matches!(canonical_part(part), "Ctrl" | "Shift" | "Alt")
}

fn chord_with_modifier(chord: &str, modifier: &str) -> String {
    canonical_chord(&format!("{modifier}+{chord}"))
}

pub fn normalize_keymap(map: Keymap) -> Keymap {
    let mut out = Keymap::new();
    for (k, v) in map {
        if k.starts_with("unbind:") {
            out.insert(k, v);
            continue;
        }
        let (scope, chord) = parse_key(&k);
        out.insert(scope_key(scope, &canonical_chord(chord)), v);
    }
    out
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

/// The binding that wins for a chord: a scoped one, otherwise the application one.
fn lookup<'a>(map: &'a Keymap, chord: &str, scope: Scope) -> Option<&'a str> {
    if let Some(a) = map.get(&scope_key(scope, chord)) {
        return Some(a);
    }
    if scope != Scope::Application {
        return map.get(chord).map(String::as_str);
    }
    None
}

/// The action for a key press in the given scope: a scoped binding first, then an application
/// binding, then the Maya default unless it was removed. A release binding is taken, so its
/// press action is suppressed too.
pub fn resolve(app: &IncaApp, key: Key, m: Modifiers, scope: Scope) -> Option<String> {
    let c = chord(key, m);
    if let Some(a) = lookup(&app.keymap, &c, scope) {
        let (id, release) = split_action(a);
        return (!release).then(|| id.to_string());
    }
    let a = crate::actions::hotkey(key, m, app)?;
    (!app.keymap.contains_key(&format!("unbind:{a}"))).then(|| a.to_string())
}

/// The action a key should run when released in the given scope, if it was bound on release.
pub fn release_action(app: &IncaApp, key: Key, m: Modifiers, scope: Scope) -> Option<String> {
    lookup(&app.keymap, &chord(key, m), scope).and_then(|a| a.strip_prefix("release:").map(str::to_string))
}

fn split_action(a: &str) -> (&str, bool) {
    match a.strip_prefix("release:") {
        Some(id) => (id, true),
        None => (a, false),
    }
}

fn action_value(id: &str, release: bool) -> String {
    if release { format!("release:{id}") } else { id.to_string() }
}

fn bound_action(app: &IncaApp, c: &str, scope: Scope) -> Option<(String, bool)> {
    if let Some(a) = app.keymap.get(&scope_key(scope, c)) {
        let (id, release) = split_action(a);
        return Some((id.to_string(), release));
    }
    // Only the application scope inherits the Maya defaults; an editor binding may shadow them.
    if scope == Scope::Application {
        return crate::actions::ACTIONS.iter().find(|(id, _, hk)| *hk == c && !app.keymap.contains_key(&format!("unbind:{id}"))).map(|(id, _, _)| (id.to_string(), false));
    }
    None
}

/// Every binding for an action in a scope, with its direction (press vs release). The default
/// press hotkey belongs to the application scope.
fn binding_chords(app: &IncaApp, id: &str, scope: Scope) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = app
        .keymap
        .iter()
        .filter(|(c, _)| !c.starts_with("unbind:"))
        .filter_map(|(c, a)| {
            let (key_scope, chord) = parse_key(c);
            if key_scope != scope {
                return None;
            }
            let (action, release) = split_action(a);
            (action == id).then(|| (chord.to_string(), release))
        })
        .collect();
    if scope == Scope::Application {
        let def = crate::actions::hotkey_label(id);
        if !def.is_empty() && !app.keymap.contains_key(&format!("unbind:{id}")) && !app.keymap.contains_key(def) {
            out.push((def.to_string(), false));
        }
    }
    out
}

/// Application and editor bindings for an action, used for search ranking and keyboard highlight.
/// Creation order is preserved; the application bindings come first.
fn action_chords(app: &IncaApp, id: &str, scope: Scope) -> Vec<(String, bool)> {
    let mut out = binding_chords(app, id, Scope::Application);
    if scope != Scope::Application {
        for binding in binding_chords(app, id, scope) {
            if !out.contains(&binding) {
                out.push(binding);
            }
        }
    }
    out
}

fn binding_text_from(bindings: &[(String, bool)]) -> String {
    bindings
        .iter()
        .map(|(c, release)| {
            let c = display_chord(c);
            if *release { format!("{c} (release)") } else { c }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Display a key by its symbol/short label rather than its internal name.
fn display_key(key: &str) -> &str {
    match key {
        "Semicolon" => ";",
        "Period" => ".",
        "Comma" => ",",
        "Quote" => "'",
        "Slash" => "/",
        "Backslash" => "\\",
        "OpenBracket" => "[",
        "CloseBracket" => "]",
        "Minus" => "-",
        "Equals" => "=",
        "Backtick" => "`",
        "ControlLeft" | "ControlRight" => "Ctrl",
        "ShiftLeft" | "ShiftRight" => "Shift",
        "AltLeft" | "AltRight" => "Alt",
        "Backspace" => "Bksp",
        "Escape" => "Esc",
        "Delete" => "Del",
        "Insert" => "Ins",
        "PageUp" => "PgUp",
        "PageDown" => "PgDn",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        _ => key,
    }
}

fn display_chord(c: &str) -> String {
    match c.rsplit_once('+') {
        Some((prefix, key)) => format!("{prefix}+{}", display_key(key)),
        None => display_key(c).to_string(),
    }
}

fn binding_text(app: &IncaApp, id: &str, scope: Scope) -> String {
    binding_text_from(&binding_chords(app, id, scope))
}

/// The key part of a binding, ignoring modifiers: "Ctrl+Shift+K" -> "K", "Insert / hold D" -> "D".
fn binding_key(c: &str) -> &str {
    c.rsplit(|ch: char| ch == '+' || ch.is_whitespace()).find(|s| !s.is_empty()).unwrap_or(c)
}

/// Does any word in the text start with `q`? "Quad Draw" matches "d"; "Target Weld" does not.
fn starts_word(text: &str, q: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|w| w.to_lowercase().starts_with(q))
}

/// Lower is a better search match. Hotkey matches rank above command-name matches so typing
/// "D" surfaces the command bound to D instead of every command containing the letter.
fn search_rank(q: &str, id: &str, label: &str, cat: &str, bindings: &[(String, bool)]) -> Option<u8> {
    if q.is_empty() {
        return Some(0);
    }
    if bindings.iter().any(|(c, _)| c.eq_ignore_ascii_case(q)) {
        return Some(1);
    }
    // A plain key (no modifiers) is more specific than Ctrl+D etc.
    if bindings.iter().any(|(c, _)| !c.contains('+') && binding_key(c).eq_ignore_ascii_case(q)) {
        return Some(2);
    }
    if bindings.iter().any(|(c, _)| binding_key(c).eq_ignore_ascii_case(q)) {
        return Some(3);
    }
    if bindings.iter().any(|(c, _)| c.to_lowercase().contains(q)) {
        return Some(4);
    }
    let label = label.to_lowercase();
    if starts_word(&label, q) {
        return Some(5);
    }
    // One-letter searches must match a word start, otherwise "D" matches every "Weld"/"Edge".
    if q.chars().count() == 1 {
        return None;
    }
    if label.contains(q) {
        return Some(6);
    }
    if id.to_lowercase().contains(q) {
        return Some(7);
    }
    if cat.to_lowercase().contains(q) {
        return Some(8);
    }
    None
}

/// The chords that run an action on press (menus and shortcut columns). Release-only bindings
/// are deliberately omitted; the Hotkey Editor shows them with their direction.
pub fn chords_for(app: &IncaApp, id: &str) -> Vec<String> {
    binding_chords(app, id, Scope::Application).into_iter().filter(|(_, release)| !release).map(|(c, _)| c).collect()
}

fn binding_map(app: &IncaApp, scope: Scope) -> BTreeMap<String, (String, bool)> {
    let mut map = BTreeMap::new();
    for (id, _, hk) in crate::actions::ACTIONS {
        if !hk.is_empty() && !app.keymap.contains_key(&format!("unbind:{id}")) {
            map.insert((*hk).to_string(), ((*id).to_string(), false));
        }
    }
    for (c, a) in &app.keymap {
        if c.starts_with("unbind:") {
            continue;
        }
        let (key_scope, chord) = parse_key(c);
        if key_scope == Scope::Application {
            let (id, release) = split_action(a);
            map.insert(chord.to_string(), (id.to_string(), release));
        }
    }
    if scope != Scope::Application {
        // Editor bindings shadow application ones only in their own scope.
        for (c, a) in &app.keymap {
            if c.starts_with("unbind:") {
                continue;
            }
            let (key_scope, chord) = parse_key(c);
            if key_scope == scope {
                let (id, release) = split_action(a);
                map.insert(chord.to_string(), (id.to_string(), release));
            }
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
const KEY_HELP: &str = "Hover over a key to see its command. Hold Ctrl / Alt / Shift to preview layers; click them to toggle.";

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
    ensure_editable_set(app);
    if app.hk.selected.is_some() && !app.hk.capturing {
        app.hk.capturing = true;
        app.hk.pending = None;
        app.hk.restore_confirm = false;
        let scope = if app.hk.scope == Scope::Application { String::new() } else { format!(" in the {}", app.hk.scope.label()) };
        app.hk.status = format!("Press a key combination for the selected command{scope}… (Esc cancels)");
    }
}

fn assign_key(app: &mut IncaApp, c: &str, id: &str, scope: Scope) {
    app.keymap.insert(scope_key(scope, c), id.to_string());
}

/// Re-key a binding without moving it in the creation order.
fn replace_key(map: &mut Keymap, old: &str, new: &str, value: String) {
    if let Some(index) = map.get_index_of(old) {
        map.shift_remove(old);
        map.shift_insert(index, new.to_string(), value);
    } else {
        map.insert(new.to_string(), value);
    }
}

/// Switch one existing (or default) binding between press and release.
fn set_binding_release(app: &mut IncaApp, id: &str, chord: &str, release: bool, scope: Scope) {
    let key = scope_key(scope, chord);
    let action = app.keymap.get(&key).map(|a| split_action(a).0.to_string()).unwrap_or_else(|| id.to_string());
    app.keymap.insert(key, action_value(&action, release));
    app.hk.status = format!("{chord} now runs on {}.", if release { "release" } else { "press" });
}

/// Remove one binding, or switch off an Application default if it was implicit.
fn remove_binding(app: &mut IncaApp, id: &str, chord: &str, scope: Scope) {
    let key = scope_key(scope, chord);
    if app.keymap.shift_remove(&key).is_none() && scope == Scope::Application && crate::actions::hotkey_label(id) == chord {
        app.keymap.insert(format!("unbind:{id}"), id.to_string());
    }
    match canonical_part(chord) {
        "Ctrl" => app.hk.layer_ctrl = false,
        "Shift" => app.hk.layer_shift = false,
        "Alt" => app.hk.layer_alt = false,
        _ => {}
    }
    app.hk.restore_layers = None;
    app.hk.status = format!("Removed {chord} from {}.", command_label(id));
}

/// Add a modifier to an existing binding, keeping its main key and press/release direction.
fn add_chord_part(app: &mut IncaApp, id: &str, chord: &str, modifier: &str, scope: Scope) {
    let new_chord = chord_with_modifier(chord, modifier);
    if new_chord == chord {
        return;
    }
    let key = scope_key(scope, chord);
    let new_key = scope_key(scope, &new_chord);
    if app.keymap.contains_key(&new_key) {
        app.hk.status = format!("{new_chord} is already assigned.");
        return;
    }
    if let Some(value) = app.keymap.get(&key).cloned() {
        replace_key(&mut app.keymap, &key, &new_key, value);
        app.hk.status = format!("Added {modifier} to {chord}.");
    } else if scope == Scope::Application && crate::actions::hotkey_label(id) == chord {
        app.keymap.insert(format!("unbind:{id}"), id.to_string());
        app.keymap.insert(new_key, id.to_string());
        app.hk.status = format!("Added {modifier} to {chord}.");
    }
}

/// Drop one modifier from a chord, keeping the rest and its press/release direction.
fn remove_chord_part(app: &mut IncaApp, id: &str, chord: &str, index: usize, scope: Scope) {
    let mut parts: Vec<&str> = chord.split('+').collect();
    if index + 1 >= parts.len() {
        remove_binding(app, id, chord, scope);
        return;
    }
    match canonical_part(parts[index]) {
        "Ctrl" => app.hk.layer_ctrl = false,
        "Shift" => app.hk.layer_shift = false,
        "Alt" => app.hk.layer_alt = false,
        _ => {}
    }
    app.hk.restore_layers = None;
    parts.remove(index);
    let new_chord = parts.join("+");
    let key = scope_key(scope, chord);
    let new_key = scope_key(scope, &new_chord);
    if app.keymap.contains_key(&new_key) {
        app.hk.status = format!("{new_chord} is already assigned.");
        return;
    }
    if let Some(value) = app.keymap.get(&key).cloned() {
        replace_key(&mut app.keymap, &key, &new_key, value);
        app.hk.status = format!("Removed a key from {chord}.");
    } else if scope == Scope::Application && crate::actions::hotkey_label(id) == chord {
        app.keymap.insert(format!("unbind:{id}"), id.to_string());
        app.keymap.insert(new_key, id.to_string());
        app.hk.status = format!("Removed a key from {chord}.");
    }
}

fn override_pending(app: &mut IncaApp) {
    let Some(p) = app.hk.pending.take() else { return };
    let key = scope_key(p.scope, &p.chord);
    if !app.keymap.contains_key(&key) && p.scope == Scope::Application && !p.existing_release && crate::actions::hotkey_label(&p.existing) == p.chord {
        app.keymap.insert(format!("unbind:{}", p.existing), p.existing.clone());
    }
    app.keymap.insert(key, p.action.clone());
    clear_modifier_layers(app);
    app.hk.status = format!("Assigned {} to {} ({}).", p.chord, command_label(&p.action), p.scope.label());
}

fn remove_hotkeys(app: &mut IncaApp) {
    if app.active_keymap == 0 {
        return;
    }
    let Some(id) = app.hk.selected.clone() else { return };
    let scope = app.hk.scope;
    app.keymap.retain(|k, a| {
        if k.starts_with("unbind:") {
            return true;
        }
        let (key_scope, _) = parse_key(k);
        !(key_scope == scope && split_action(a).0 == id)
    });
    if scope == Scope::Application && !crate::actions::hotkey_label(&id).is_empty() {
        app.keymap.insert(format!("unbind:{id}"), id.clone());
    }
    app.hk.pending = None;
    app.hk.status = format!("Removed all {} hotkeys from {}.", scope.label().to_lowercase(), command_label(&id));
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
    let chords = action_chords(app, id, app.hk.scope);
    if chords.is_empty() {
        clear_modifier_layers(app);
        return;
    }
    let current = (app.hk.layer_ctrl, app.hk.layer_shift, app.hk.layer_alt);
    if chords.iter().any(|(c, _)| chord_layer(c) == current) {
        return;
    }
    if let Some((best, _)) = chords.iter().min_by_key(|(c, _)| chord_pref(c.as_str())) {
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

/// The default set is read-only; the first edit creates a custom set from the defaults so the
/// user does not hit a dead end.
fn ensure_editable_set(app: &mut IncaApp) {
    if app.active_keymap > 0 && app.active_keymap <= app.keymaps.len() {
        return;
    }
    let name = unique_set_name(app, "Custom 1");
    create_set(app, name, Keymap::new());
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

fn submit_chord(app: &mut IncaApp, c: String, id: String, scope: Scope) {
    ensure_editable_set(app);
    app.hk.capturing = false;
    let count = binding_chords(app, &id, scope).len();
    match bound_action(app, &c, scope) {
        Some((existing, existing_release)) if existing == id => {
            app.hk.status = if existing_release {
                format!("{c} is already assigned to this command on release in {} scope.", scope.label())
            } else {
                format!("{c} is already assigned to this command in {} scope.", scope.label())
            };
        }
        _ if count >= 4 => {
            app.hk.status = format!("A command can have at most 4 hotkeys in {} scope.", scope.label());
        }
        Some((existing, existing_release)) => {
            app.hk.status = format!("{c} is assigned to {}{} in {} scope.", command_label(&existing), if existing_release { " (release)" } else { "" }, scope.label());
            app.hk.pending = Some(Pending { chord: c, action: id, existing, existing_release, scope });
        }
        None => {
            assign_key(app, &c, &id, scope);
            clear_modifier_layers(app);
            app.hk.status = format!("Assigned {c} to {} ({}).", command_label(&id), scope.label());
        }
    }
}

fn is_modifier_key(key: Key) -> bool {
    matches!(key, Key::ControlLeft | Key::ControlRight | Key::ShiftLeft | Key::ShiftRight | Key::AltLeft | Key::AltRight | Key::SuperLeft | Key::SuperRight)
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
            // Wait for a real key: pressing Ctrl/Shift/Alt first must not end the capture.
            egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } if !is_modifier_key(*key) => Some((*key, *modifiers)),
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
    let scope = app.hk.scope;
    submit_chord(app, chord(key, m), id, scope);
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
    bindings: &BTreeMap<String, (String, bool)>,
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
            .map(|(c, (a, release))| (c.clone(), a.clone(), *release))
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
        } else if let Some((a, release)) = &bound {
            format!("{combo}  ->  {}{}", command_label(a), if *release { "  (on release)" } else { "" })
        } else if let Some((c, a, release)) = &hint {
            format!("{c}  ->  {}{}  (other modifier layer)", command_label(a), if *release { "  (on release)" } else { "" })
        } else {
            format!("{combo}  —  no command assigned")
        };
    }
    if click.is_some_and(|p| rect.contains(p)) {
        if let Some(m) = spec.modifier {
            *clicked = Some(KeyClick::Modifier(m));
        } else if let Some((a, _)) = bound {
            *clicked = Some(KeyClick::Action(a));
        } else if let Some((c, a, _)) = &hint {
            *clicked = Some(KeyClick::Hint { action: a.clone(), chord: c.clone() });
        }
    }
}

fn keyboard(app: &mut IncaApp, ui: &mut egui::Ui) -> String {
    let mut bindings = binding_map(app, app.hk.scope);
    // The keyboard follows the category filter, like the command list.
    if !app.hk.category.is_empty() {
        bindings.retain(|_, (action, _)| category_of(action) == app.hk.category);
    }
    let selected_chords: HashSet<String> = app.hk.selected.as_ref()
        .filter(|id| app.hk.category.is_empty() || category_of(id) == app.hk.category)
        .map(|id| action_chords(app, id, app.hk.scope).into_iter().map(|(c, _)| c).collect())
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
        let c = layer_chord(
            key,
            layers.0 || mods.ctrl || mods.command,
            layers.1 || mods.shift,
            layers.2 || mods.alt,
        );
        info = format!("Release to assign {} to {c}.", command_label(action.as_str()));
    }
    if info.is_empty() && !hover.is_some_and(|p| rect.contains(p)) {
        info = KEY_HELP.to_string();
    }
    if let Some(action) = resp.dnd_release_payload::<String>() {
        let action = (*action).clone();
        if let Some(key) = drop_key {
            let ctrl = layers.0 || mods.ctrl || mods.command;
            let shift = layers.1 || mods.shift;
            let alt = layers.2 || mods.alt;
            app.hk.selected = Some(action.clone());
            app.hk.pending = None;
            app.hk.status.clear();
            let scope = app.hk.scope;
            submit_chord(app, layer_chord(&key, ctrl, shift, alt), action, scope);
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

const KEY_CHIP_W: f32 = 80.0;
const KEY_CHIP_H: f32 = 26.0;

/// A large amber key chip, like the selected keys on the keyboard. Every chip is the same size
/// so the details area never reflows; long labels shrink instead.
fn key_chip(ui: &mut egui::Ui, text: &str, clickable: bool) -> egui::Response {
    let mut size = 12.0;
    let mut galley = ui.painter().layout_no_wrap(text.to_string(), FontId::monospace(size), Color32::WHITE);
    while galley.size().x > KEY_CHIP_W - 8.0 && size > 7.0 {
        size -= 0.5;
        galley = ui.painter().layout_no_wrap(text.to_string(), FontId::monospace(size), Color32::WHITE);
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(KEY_CHIP_W, KEY_CHIP_H), if clickable { Sense::click() } else { Sense::hover() });
    let fill = if clickable && resp.hovered() { Color32::from_rgb(0xd6, 0x9d, 0x4a) } else { Color32::from_rgb(0xc5, 0x8b, 0x3a) };
    ui.painter().rect_filled(rect, 3.0, fill);
    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
    resp
}

/// The per-key press/release switch below a key chip; amber means on release.
fn on_release_button(ui: &mut egui::Ui, active: bool, clickable: bool, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 12.0), if clickable { Sense::click() } else { Sense::hover() });
    let amber = Color32::from_rgb(0xc5, 0x8b, 0x3a);
    let fill = if active {
        if clickable && resp.hovered() { Color32::from_rgb(0xd6, 0x9d, 0x4a) } else { amber }
    } else if clickable && resp.hovered() {
        Color32::from_rgb(0x6f, 0x6f, 0x6f)
    } else {
        Color32::from_rgb(0x5d, 0x5d, 0x5d)
    };
    ui.painter().rect_filled(rect, 2.0, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "On Release", FontId::proportional(8.5), if active { Color32::WHITE } else { Color32::from_gray(0xdd) });
    resp
}

fn command_details(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(id) = app.hk.selected.clone() else { return };
    let scope = app.hk.scope;
    let editable = app.active_keymap > 0;
    ui.label(RichText::new(command_label(&id)).strong().size(15.0));
    if let Some(desc) = command_description(&id) {
        ui.label(RichText::new(desc).color(theme::TEXT_DIM));
    }
    ui.label(RichText::new(category_of(&id)).color(theme::TEXT_DIM));
    let bindings = binding_chords(app, &id, scope);
    if bindings.is_empty() {
        let text = if scope == Scope::Application { "No hotkey assigned".to_string() } else { format!("No {} hotkey", scope.label()) };
        ui.label(RichText::new(text).monospace().color(theme::TEXT_DIM));
    } else {
        let title = if scope == Scope::Application { "Hotkeys:".to_string() } else { format!("Hotkeys ({}):", scope.label()) };
        ui.label(RichText::new(title).color(theme::TEXT_DIM));
        let mut toggle = None;
        let mut remove = None;
        let mut remove_part = None;
        let mut add_mod = None;
        let gap = ui.spacing().item_spacing.x;
        let col_w = ((ui.available_width() - gap - 12.0) / 2.0).max(280.0);
        for (row, pair) in bindings.chunks(2).enumerate() {
            if row > 0 {
                ui.add_space(6.0);
            }
            ui.horizontal_top(|ui| {
                for (i, (chord, release)) in pair.iter().enumerate() {
                    if i > 0 {
                        // A plain ui.separator() would stretch to the whole details area and push
                        // the second row out of view; draw a fixed-height divider instead.
                        let (r, _) = ui.allocate_exact_size(vec2(8.0, 50.0), Sense::hover());
                        ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, Color32::from_rgb(0x5a, 0x5a, 0x5a)));
                    }
                    ui.allocate_ui_with_layout(vec2(col_w, 50.0), egui::Layout::top_down(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let raw: Vec<&str> = chord.split('+').collect();
                        if raw.is_empty() {
                            return;
                        }
                        let display: Vec<String> = raw.iter().map(|p| display_key(p).to_string()).collect();
                        let main_idx = raw.iter().rposition(|p| !is_canonical_modifier(p));
                        let absent: Vec<&str> = ["Ctrl", "Shift", "Alt"].into_iter().filter(|m| !raw.iter().any(|p| canonical_part(p) == *m)).collect();
                        ui.horizontal_top(|ui| {
                            for modifier in ["Ctrl", "Shift", "Alt"] {
                                if raw.iter().any(|p| canonical_part(p) == modifier) {
                                    if key_chip(ui, modifier, editable).on_hover_text(format!("Remove {modifier}")).clicked() {
                                        if let Some(i) = raw.iter().position(|p| canonical_part(p) == modifier) {
                                            remove_part = Some((chord.clone(), i));
                                        }
                                    }
                                }
                            }
                            // The add-modifier control sits immediately left of the main key.
                            if editable && !absent.is_empty() {
                                let menu = ui.menu_button("+", |ui| {
                                    for m in absent {
                                        if ui.button(m).clicked() {
                                            add_mod = Some((chord.clone(), m.to_string()));
                                            ui.close();
                                        }
                                    }
                                });
                                menu.response.on_hover_text("Add a modifier");
                            }
                            if let Some(main_idx) = main_idx {
                                ui.vertical(|ui| {
                                    if key_chip(ui, &display[main_idx], editable).on_hover_text("Remove this hotkey").clicked() {
                                        remove = Some(chord.clone());
                                    }
                                    if on_release_button(ui, *release, editable, KEY_CHIP_W).on_hover_text("Run this key on release instead of press").clicked() {
                                        toggle = Some((chord.clone(), !*release));
                                    }
                                });
                            } else if on_release_button(ui, *release, editable, KEY_CHIP_W).on_hover_text("Run this key on release instead of press").clicked() {
                                toggle = Some((chord.clone(), !*release));
                            }
                        });
                    });
                }
            });
        }
        if scope != Scope::Application {
            let global = binding_text(app, &id, Scope::Application);
            if !global.is_empty() {
                ui.label(RichText::new(format!("Application: {global}")).monospace().color(theme::TEXT_DIM));
            }
        }
        if let Some((chord, release)) = toggle {
            set_binding_release(app, &id, &chord, release, scope);
        }
        if let Some(chord) = remove {
            remove_binding(app, &id, &chord, scope);
        }
        if let Some((chord, index)) = remove_part {
            remove_chord_part(app, &id, &chord, index, scope);
        }
        if let Some((chord, modifier)) = add_mod {
            add_chord_part(app, &id, &chord, &modifier, scope);
        }
    }
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
            let id = action.strip_prefix("release:").unwrap_or(action);
            let id = id.strip_prefix("unbind:").unwrap_or(id);
            let known = crate::actions::available(id);
            if !known {
                dropped += 1;
            }
            known
        })
        .collect();
    (normalize_keymap(map), dropped)
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

fn paint_lock(painter: &egui::Painter, at: Pos2, color: Color32) {
    let body = Rect::from_min_size(at, vec2(9.0, 6.0));
    painter.rect_filled(body, 1.0, color);
    let c = pos2(at.x + 4.5, at.y);
    let pts: Vec<Pos2> = (0..=8)
        .map(|i| {
            let a = std::f32::consts::PI + std::f32::consts::PI * (i as f32 / 8.0);
            pos2(c.x + 3.0 * a.cos(), c.y + 3.0 * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.2, color)));
}

fn keymap_bar(app: &mut IncaApp, ui: &mut egui::Ui) {
    let editable = app.active_keymap > 0 && app.active_keymap <= app.keymaps.len();
    ui.horizontal_wrapped(|ui| {
        ui.label("Hotkey set:");
        let selected = if editable { app.keymaps[app.active_keymap - 1].name.clone() } else { "Inca Default".to_string() };
        let sets: Vec<(usize, String)> = app.keymaps.iter().enumerate().map(|(i, s)| (i + 1, s.name.clone())).collect();
        let combo = egui::ComboBox::from_id_salt("hk_set").width(190.0).selected_text(selected.clone()).show_ui(ui, |ui| {
            if ui.selectable_label(app.active_keymap == 0, "Inca Default").clicked() {
                activate_set(app, 0);
            }
            for (idx, name) in sets {
                if ui.selectable_label(app.active_keymap == idx, name).clicked() {
                    activate_set(app, idx);
                }
            }
        });
        if !editable {
            // A padlock next to the read-only default set's name.
            let r = combo.response.rect;
            let text = ui.painter().layout_no_wrap(selected, egui::TextStyle::Button.resolve(ui.style()), theme::TEXT);
            let x = r.left() + ui.spacing().button_padding.x + text.size().x + 5.0;
            paint_lock(ui.painter(), pos2(x, r.center().y - 1.0), theme::TEXT_DIM);
            combo.response.on_hover_text("Inca Default is read-only");
        }
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
            ui.label(RichText::new("Inca Default is read-only — assigning a hotkey creates a new set.").color(theme::TEXT_DIM));
        }
    });
    ui.separator();
}

fn left_panel(app: &mut IncaApp, ui: &mut egui::Ui) {
    keymap_bar(app, ui);
    ui.horizontal(|ui| {
        ui.label("Scope:");
        let scope_combo = egui::ComboBox::from_id_salt("hk_scope").width(120.0).selected_text(app.hk.scope.label()).show_ui(ui, |ui| {
            for scope in Scope::ALL {
                if ui.selectable_label(app.hk.scope == scope, scope.label()).clicked() && app.hk.scope != scope {
                    app.hk.scope = scope;
                    app.hk.pending = None;
                    app.hk.capturing = false;
                }
            }
        });
        scope_combo.response.on_hover_text("Editor hotkeys apply only while the pointer is over that editor. Application hotkeys still work unless overridden.");
    });
    let info = keyboard(app, ui);
    ui.label(RichText::new(if info.is_empty() { " " } else { &info }).color(theme::TEXT_DIM));
    legend(ui);
    // Fixed-height details quadrant: selecting a command with no bindings must not resize the
    // window or make the content above it jump.
    let details_h = 260.0;
    let avail = ui.available_height();
    if avail > details_h {
        ui.add_space(avail - details_h);
    }
    egui::ScrollArea::vertical().id_salt("hk_details").max_height(details_h).auto_shrink([false, false]).show(ui, |ui| {
        command_details(app, ui);
    });
}

fn command_row(ui: &mut egui::Ui, id: &str, label: &str, keys: &str, sel: bool) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let resp = ui.interact(rect, ui.make_persistent_id(("hk_cmd", id)), Sense::click_and_drag());
    resp.dnd_set_drag_payload(id.to_string());
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
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let max_w = (rect.width() * 0.55).max(70.0);
        let mut shown = keys.to_string();
        let mut galley = ui.painter().layout_no_wrap(shown.clone(), font.clone(), color);
        if galley.size().x > max_w {
            while !shown.is_empty() {
                shown.pop();
                let candidate = format!("{}…", shown);
                galley = ui.painter().layout_no_wrap(candidate.clone(), font.clone(), color);
                if galley.size().x <= max_w {
                    shown = candidate;
                    break;
                }
            }
        }
        ui.painter().with_clip_rect(rect).text(rect.right_center() - vec2(6.0, 0.0), Align2::RIGHT_CENTER, shown, font, color);
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
    let reset_scroll = app.hk.filter != app.hk.filter_seen || app.hk.category != app.hk.category_seen;
    app.hk.filter_seen = app.hk.filter.clone();
    app.hk.category_seen = app.hk.category.clone();
    let q = app.hk.filter.trim().to_lowercase();
    let cat_filter = app.hk.category.clone();
    let mut groups: BTreeMap<&'static str, Vec<(&'static str, String, String, u8)>> = BTreeMap::new();
    for (id, label) in &commands {
        let cat = category_of(id);
        if !cat_filter.is_empty() && cat != cat_filter {
            continue;
        }
        let search = action_chords(app, id, app.hk.scope);
        let Some(rank) = search_rank(&q, id, label, cat, &search) else {
            continue;
        };
        groups.entry(cat).or_default().push((id, label.clone(), binding_text(app, id, app.hk.scope), rank));
    }
    let mut groups: Vec<(&'static str, Vec<(&'static str, String, String, u8)>)> = groups.into_iter().collect();
    groups.sort_by(|(a, ai), (b, bi)| {
        let ra = ai.iter().map(|x| x.3).min().unwrap_or(0);
        let rb = bi.iter().map(|x| x.3).min().unwrap_or(0);
        ra.cmp(&rb).then(category_rank(a).cmp(&category_rank(b))).then(a.cmp(b))
    });
    let list_h = (ui.available_height() - 54.0).max(120.0);
    let mut list = egui::ScrollArea::vertical().id_salt("hk_command_list").auto_shrink([false, false]).max_height(list_h);
    if reset_scroll {
        list = list.vertical_scroll_offset(0.0);
    }
    list.show(ui, |ui| {
        // Keep the floating scrollbar off the shortcut column.
        egui::Frame::NONE.inner_margin(egui::Margin { right: 20, ..Default::default() }).show(ui, |ui| {
            for (cat, mut items) in groups {
                items.sort_by(|a, b| a.3.cmp(&b.3).then(a.1.cmp(&b.1)));
                let reveal_here = app.hk.reveal && items.iter().any(|(id, _, _, _)| app.hk.selected.as_deref() == Some(*id));
                let header = format!("{} ({})", cat, items.len());
                egui::CollapsingHeader::new(RichText::new(header).strong())
                    .id_salt(("hk_cat", cat))
                    .default_open(true)
                    .open(reveal_here.then_some(true))
                    .show(ui, |ui| {
                        for (id, label, keys, _) in items {
                            let sel = app.hk.selected.as_deref() == Some(id);
                            let tip = if keys.is_empty() { id.to_string() } else { format!("{id}\n{keys}") };
                            let resp = command_row(ui, id, &label, &keys, sel).on_hover_text(tip);
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
    let has_keys = selected.as_ref().is_some_and(|id| !binding_chords(app, id, app.hk.scope).is_empty());
    ui.horizontal(|ui| {
        let txt = if app.hk.capturing { "Press a key… (Esc cancels)" } else { "Assign New Hotkey" };
        if ui.add_enabled(has_sel, egui::Button::new(txt)).clicked() {
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
            let dir = if p.existing_release { " (release)" } else { "" };
            let scope = if p.scope == Scope::Application { String::new() } else { format!(" in {} scope", p.scope.label()) };
            ui.label(RichText::new(format!("{} is assigned to {}{dir}{scope}.", p.chord, command_label(&p.existing))).color(Color32::from_rgb(0xe6, 0xc5, 0x4a)));
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
    fn keymap_preserves_creation_order() {
        let mut map = Keymap::new();
        map.insert("Z".into(), "a".into());
        map.insert("A".into(), "b".into());
        map.insert("M".into(), "c".into());
        assert_eq!(map.keys().cloned().collect::<Vec<_>>(), vec!["Z", "A", "M"]);
    }

    #[test]
    fn left_and_right_modifiers_share_one_chord() {
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(chord(Key::ControlLeft, ctrl), "Ctrl");
        assert_eq!(chord(Key::ControlRight, ctrl), "Ctrl");
        let shift = Modifiers { shift: true, ..Default::default() };
        assert_eq!(chord(Key::ShiftLeft, shift), "Shift");
        assert_eq!(chord(Key::ShiftRight, shift), "Shift");
        assert_eq!(canonical_chord("Ctrl+ControlLeft"), "Ctrl");
        assert_eq!(canonical_chord("Ctrl+Shift+ShiftLeft"), "Ctrl+Shift");
        assert!(is_modifier_key(Key::ControlLeft));
        assert!(is_modifier_key(Key::ShiftRight));
        assert!(!is_modifier_key(Key::L));
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(chord(Key::L, ctrl), "Ctrl+L");
        assert_eq!(chord_with_modifier("L", "Ctrl"), "Ctrl+L");
        assert_eq!(chord_with_modifier("Shift+L", "Ctrl"), "Ctrl+Shift+L");
        assert_eq!(chord_with_modifier("Ctrl+L", "Shift"), "Ctrl+Shift+L");

        let mut map = Keymap::new();
        map.insert("viewport:Ctrl+ControlLeft".into(), "pivotEdit".into());
        map.insert("viewport:Ctrl+ControlRight".into(), "pivotEdit".into());
        let map = normalize_keymap(map);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("viewport:Ctrl"));
    }

    #[test]
    fn scoped_keys_round_trip() {
        assert_eq!(parse_key("W"), (Scope::Application, "W"));
        assert_eq!(scope_key(Scope::Viewport, "Ctrl+W"), "viewport:Ctrl+W");
        assert_eq!(parse_key("viewport:Ctrl+W"), (Scope::Viewport, "Ctrl+W"));
        assert_eq!(parse_key("hypershade:Q"), (Scope::Hypershade, "Q"));
        assert_eq!(parse_key("uv:F"), (Scope::UvEditor, "F"));
        assert_eq!(parse_key("unbind:copy"), (Scope::Application, "unbind:copy"));
    }

    #[test]
    fn scoped_bindings_shadow_application_keys() {
        let mut map = Keymap::new();
        map.insert("W".into(), "moveTool".into());
        map.insert("viewport:W".into(), "pivotEdit".into());
        assert_eq!(lookup(&map, "W", Scope::Application), Some("moveTool"));
        assert_eq!(lookup(&map, "W", Scope::Viewport), Some("pivotEdit"));
        assert_eq!(lookup(&map, "Q", Scope::Viewport), None);
    }

    #[test]
    fn one_letter_search_matches_word_starts_only() {
        assert!(starts_word("Quad Draw", "d"));
        assert!(starts_word("Duplicate", "d"));
        assert!(starts_word("Decrease Manipulator Size", "d"));
        assert!(!starts_word("Target Weld", "d"));
        assert!(!starts_word("Insert Edge Loop Tool", "d"));
        assert!(search_rank("d", "targetWeld", "Target Weld", "Tools", &[]).is_none());
        assert!(search_rank("d", "quadDraw", "Quad Draw", "Tools", &[]).is_some());
    }

    #[test]
    fn hotkey_search_prefers_plain_keys() {
        assert_eq!(binding_key("Ctrl+Shift+K"), "K");
        assert_eq!(binding_key("Insert / hold D"), "D");
        let plain_d = vec![("Insert / hold D".to_string(), false)];
        let ctrl_d = vec![("Ctrl+D".to_string(), false)];
        let plain = search_rank("d", "pivotEdit", "Edit Pivot", "Tools", &plain_d).unwrap();
        let modified = search_rank("d", "duplicate", "Duplicate", "Edit", &ctrl_d).unwrap();
        assert!(plain < modified, "plain D should rank above Ctrl+D");
    }

    #[test]
    fn release_bindings_round_trip() {
        assert_eq!(split_action("copy"), ("copy", false));
        assert_eq!(split_action("release:copy"), ("copy", true));
        for release in [false, true] {
            let value = action_value("copy", release);
            assert_eq!(split_action(&value), ("copy", release));
        }
    }

    #[test]
    fn import_drops_unknown_command_ids() {
        let mut map = Keymap::new();
        map.insert("Ctrl+C".into(), "copy".into());
        map.insert("Ctrl+X".into(), "notARealAction".into());
        map.insert("Ctrl+R".into(), "release:repeatLast".into());
        map.insert("Ctrl+Y".into(), "release:notARealAction".into());
        map.insert("unbind:copy".into(), "copy".into());
        let (map, dropped) = known_keymap(map);
        assert_eq!(dropped, 2);
        assert!(map.contains_key("Ctrl+C"));
        assert!(map.contains_key("unbind:copy"));
        assert_eq!(map.get("Ctrl+R").map(String::as_str), Some("release:repeatLast"));
        assert!(!map.contains_key("Ctrl+X"));
        assert!(!map.contains_key("Ctrl+Y"));
    }
}

pub fn window(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.win.hotkeys;
    if !open {
        app.hk.window_rect = None;
        return;
    }
    capture(app, ctx);
    // A distinct id discards window state stored by older builds. That state had ratcheted to
    // the full screen, so a default size alone could not shrink it again.
    let shown = egui::Window::new("Hotkey Editor").id(egui::Id::new("HotkeyEditorWindow2")).open(&mut open).default_size([1280.0, 720.0]).min_size([900.0, 620.0]).resizable([true, false]).show(ctx, |ui| {
        // A fixed content height keeps the window from resizing when the selected command,
        // its hotkeys or the details area content change.
        let height = 720.0;
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
    app.hk.window_rect = if open { shown.map(|r| r.response.rect) } else { None };
    if !open {
        app.hk.capturing = false;
        app.hk.restore_layers = None;
    }
    app.win.hotkeys = open;
}
