//! Hypershade: material / texture palette with rendered swatches, a node
//! graph work area (drag a texture's output onto a material slot to connect, a material's output
//! onto an object to assign) and the property editor for the selected node.

use crate::app::IncaApp;
use crate::marking::{Act, Item, open_custom};
use crate::theme;
use egui::{pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use inca_core::materials::{MAT_TYPES, TEX_TYPES};
use inca_core::scene::{NodeRef, SelMode, Shape};
use serde_json::json;
use std::collections::{HashMap, HashSet};

const NODE_W: f32 = 150.0;
const HEAD_H: f32 = 20.0;
const SWATCH: f32 = 44.0;
const ROW_H: f32 = 16.0;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Texture,
    Material,
    ShadingGroup,
    Object,
}

#[derive(Clone)]
struct GNode {
    id: String,
    kind: Kind,
    name: String,
    /// input ports (attribute names), left side
    inputs: Vec<String>,
    /// output ports, right side
    outputs: Vec<String>,
}

#[derive(Clone, PartialEq)]
enum Wire {
    /// texture output (outColor / outAlpha) -> material.attr
    Tex(String, usize),
    /// material -> object or shading group
    Mat(String),
}

pub struct HsState {
    pub(crate) pos: HashMap<String, Pos2>,
    pan: Vec2,
    zoom: f32,
    pub(crate) graph_sel: HashSet<String>,
    /// Consecutive creations with no free spot; offsets the overlap so the nodes stay visible.
    overflow: u32,
    drag_start: Option<Vec<(String, Pos2)>>,
    marquee: Option<(Pos2, Pos2)>,
    marquee_base: HashSet<String>,
    pub(crate) frame: Option<bool>,
    wire: Option<Wire>,
    pub selected_only: bool,
    swatches: HashMap<String, (u64, egui::TextureHandle)>,
    pub tab: usize,
    pub filter: String,
    pub create_filter: String,
    pub rename: Option<(String, String)>,
    pub rename_pos: Option<Pos2>,
    /// Screen position where the empty marking menu was opened; new nodes go there.
    pub(crate) create_pos: Option<Pos2>,
    pub palette_drag: Option<String>,
    pub graph_rect: Option<Rect>,
    pub window_rect: Option<Rect>,
}

impl Default for HsState {
    fn default() -> Self {
        HsState {
            pos: HashMap::new(),
            pan: vec2(20.0, 20.0),
            zoom: 1.0,
            graph_sel: HashSet::new(),
            overflow: 0,
            drag_start: None,
            marquee: None,
            marquee_base: HashSet::new(),
            frame: None,
            wire: None,
            selected_only: false,
            swatches: HashMap::new(),
            tab: 0,
            filter: String::new(),
            create_filter: String::new(),
            rename: None,
            rename_pos: None,
            create_pos: None,
            palette_drag: None,
            graph_rect: None,
            window_rect: None,
        }
    }
}

/// Place a freshly created material or texture near `at` (screen position) or the view center,
/// avoiding the visible nodes. Falls back to the preferred spot when the view is full.
pub(crate) fn place_new_node(app: &mut IncaApp, id: &str, at: Option<Pos2>) {
    let (size, material) = if let Some(m) = app.s.mats.get(id) {
        let rows = inca_core::materials::mat_type(&m.ty).mappable.len() + 1;
        let h = HEAD_H + SWATCH + 6.0 + ROW_H * rows as f32 + 4.0;
        let sg_h = HEAD_H + SWATCH + 6.0 + ROW_H + 4.0;
        (vec2(NODE_W * 2.0 + 36.0, h.max(sg_h)), true)
    } else if app.s.texs.contains_key(id) {
        (vec2(NODE_W, HEAD_H + SWATCH + 6.0 + ROW_H * 2.0 + 4.0), false)
    } else {
        return;
    };
    let Some(rect) = app.windows_state.hs.graph_rect else { return };
    let (pan, zoom) = (app.windows_state.hs.pan, app.windows_state.hs.zoom);
    let to_graph = |q: Pos2| {
        let v = (q - rect.min - pan) / zoom;
        pos2(v.x, v.y)
    };
    let occupied: Vec<Rect> = {
        let sg_id = format!("sg:{id}");
        graph_nodes(app)
            .iter()
            .filter(|n| n.id != id && (!material || n.id != sg_id))
            .filter_map(|n| app.windows_state.hs.pos.get(&n.id).map(|p| Rect::from_min_size(*p, vec2(NODE_W, node_h(n))).expand(8.0)))
            .collect()
    };
    let lo = (-pan) / zoom;
    let hi = (rect.size() - pan) / zoom;
    let (min_x, max_x) = (lo.x, (hi.x - size.x).max(lo.x));
    let (min_y, max_y) = (lo.y, (hi.y - size.y).max(lo.y));
    let preferred = if let Some(q) = at {
        let g = to_graph(q);
        pos2(g.x - NODE_W * 0.5, g.y)
    } else {
        let v = (rect.center() - rect.min - pan) / zoom;
        pos2(v.x - size.x * 0.5, v.y - size.y * 0.5)
    };
    let preferred = pos2(preferred.x.clamp(min_x, max_x), preferred.y.clamp(min_y, max_y));
    let hits = |p: Pos2| occupied.iter().any(|r| r.intersects(Rect::from_min_size(p, size)));
    let mut best: Option<(f32, Pos2)> = (!hits(preferred)).then_some((0.0, preferred));
    if best.is_none() {
        let step = 24.0;
        let mut x = min_x;
        while x <= max_x {
            let mut y = min_y;
            while y <= max_y {
                let p = pos2(x, y);
                if !hits(p) {
                    let d = (p - preferred).length_sq();
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, p));
                    }
                }
                y += step;
            }
            x += step;
        }
    }
    let (p, free) = match best {
        Some((_, p)) => (p, true),
        None => {
            // The view is full. Cascade the overlap diagonally so the stacked nodes are visible.
            let k = (app.windows_state.hs.overflow % 10 + 1) as f32;
            let step = 20.0;
            let dx = if preferred.x + size.x + step * k <= hi.x { step } else { -step };
            let dy = if preferred.y + size.y + step * k <= hi.y { step } else { -step };
            let p = pos2(preferred.x + dx * k, preferred.y + dy * k);
            (pos2(p.x.clamp(min_x, max_x), p.y.clamp(min_y, max_y)), false)
        }
    };
    app.windows_state.hs.overflow = if free { 0 } else { app.windows_state.hs.overflow + 1 };
    app.windows_state.hs.pos.insert(id.to_string(), p);
    if material {
        app.windows_state.hs.pos.insert(format!("sg:{id}"), p + vec2(NODE_W + 36.0, 0.0));
    }
}

fn hash_of<T: std::hash::Hash>(t: &T) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// Rendered swatch for a material or texture (cached until its attributes change).
fn swatch(app: &mut IncaApp, ctx: &egui::Context, id: &str) -> Option<egui::TextureId> {
    let (key, px) = if let Some(m) = app.s.mats.get(id) {
        let tex = m.maps.get(m.color_attr()).and_then(|t| t.as_str()).and_then(|t| app.s.texs.get(t));
        let key = hash_of(&(serde_json::to_string(&m.attrs).unwrap_or_default(), &m.ty, tex.map(|t| serde_json::to_string(&t.attrs).unwrap_or_default())));
        if let Some((k, t)) = app.windows_state.hs.swatches.get(id) {
            if *k == key {
                return Some(t.id());
            }
        }
        (key, inca_core::materials::swatch(m, tex, 64))
    } else if let Some(t) = app.s.texs.get(id) {
        let key = hash_of(&serde_json::to_string(&t.attrs).unwrap_or_default());
        if let Some((k, h)) = app.windows_state.hs.swatches.get(id) {
            if *k == key {
                return Some(h.id());
            }
        }
        let px = match inca_core::materials::texture_swatch(t, 64) {
            Some(p) => p,
            None => file_swatch(t.attrs.get("fileTextureName").and_then(|v| v.as_str()).unwrap_or(""), 64),
        };
        (key, px)
    } else {
        return None;
    };
    let img = egui::ColorImage::from_rgba_unmultiplied([64, 64], &px);
    let h = ctx.load_texture(format!("hs_{id}"), img, egui::TextureOptions::LINEAR);
    let tid = h.id();
    app.windows_state.hs.swatches.insert(id.to_string(), (key, h));
    Some(tid)
}

/// A file texture's image, shrunk (gray when it cannot be read).
fn file_swatch(path: &str, size: u32) -> Vec<u8> {
    match image::open(path) {
        Ok(img) => image::imageops::resize(&img.to_rgba8(), size, size, image::imageops::FilterType::Triangle).into_raw(),
        Err(_) => vec![90; (size * size * 4) as usize],
    }
}

/// Objects (meshes) that use a material.
fn users(app: &IncaApp, mid: &str) -> Vec<String> {
    app.s.nodes.values().filter(|n| n.mesh().is_some_and(|m| m.material == mid || m.mesh.fm.as_ref().is_some_and(|fm| fm.iter().flatten().any(|x| x == mid)))).map(|n| n.id.clone()).collect()
}

fn select_dragged(app: &mut IncaApp, id: &str) {
    app.windows_state.hs_selected = Some(id.to_string());
    app.windows_state.hs.graph_sel.clear();
    if app.s.mats.contains_key(id) || app.s.texs.contains_key(id) {
        app.windows_state.hs.graph_sel.insert(id.to_string());
        select_shading_node(app, id);
    }
}

/// Maya's selection is shared: picking a shading node clears the viewport selection so the
/// main Attribute Editor shows that node instead of an object.
pub(crate) fn select_shading_node(app: &mut IncaApp, id: &str) {
    if !app.s.mats.contains_key(id) && !app.s.texs.contains_key(id) {
        return;
    }
    app.snode_sel = Some(id.to_string());
    app.s.select(&[], SelMode::Replace);
    app.windows_state.hs_selected = Some(id.to_string());
    // The viewport selection is gone, so drop DAG nodes from the graph selection too.
    let mats: HashSet<String> = app.s.mats.keys().cloned().collect();
    let texs: HashSet<String> = app.s.texs.keys().cloned().collect();
    app.windows_state.hs.graph_sel.retain(|g| {
        let base = g.strip_prefix("sg:").unwrap_or(g);
        mats.contains(base) || texs.contains(base)
    });
    app.panels.ae_tab = 0;
    app.panels.ae_locked = None;
}

fn connect_texture(app: &mut IncaApp, tex: &str, mid: &str, attr: &str) {
    let Some(mat) = app.s.mats.get(mid) else { return };
    let (mat_name, tex_name) = (mat.name.clone(), app.s.texs.get(tex).map(|t| t.name.clone()).unwrap_or_default());
    app.s.checkpoint("connect texture");
    if let Some(mat) = app.s.mats.get_mut(mid) {
        mat.set_map(attr, tex, "outColor");
    }
    app.s.touch();
    app.echo(format!("connectAttr -force {tex_name}.outColor {mat_name}.{attr};"));
}

fn assign_to_object(app: &mut IncaApp, mat: &str, obj: &str) {
    let Some(name) = app.s.mats.get(mat).map(|m| m.name.clone()) else { return };
    app.s.checkpoint("assign material");
    app.s.assign_material(obj, mat, None);
    app.s.touch();
    app.echo(format!("hyperShade -assign {name};"));
}

fn drop_on_viewport(app: &mut IncaApp, id: &str, q: Pos2) -> bool {
    let Some(idx) = app.vps.iter().position(|vp| vp.rect.contains(q)) else { return false };
    let rect = app.vps[idx].rect;
    let cam = app.vps[idx].camera.clone();
    let cv = crate::viewport::cam_view(&app.s, &cam, [rect.width(), rect.height()]);
    let Some(obj) = crate::viewport::pick_object_if(&app.s, &app.vps[idx], &cv, q, &mut app.mesh_cache, &|_| true) else { return false };
    if app.s.node(&obj).and_then(|n| n.mesh()).is_none() {
        return false;
    }
    if app.s.mats.contains_key(id) {
        assign_to_object(app, id, &obj);
    } else if app.s.texs.contains_key(id) {
        let mid = app.s.node(&obj).and_then(|n| n.mesh().map(|m| m.material.clone())).unwrap_or_else(|| app.s.default_mat.clone());
        let attr = app.s.mats.get(&mid).map(|m| m.color_attr().to_string()).unwrap_or_else(|| "color".into());
        connect_texture(app, id, &mid, &attr);
    } else {
        return false;
    }
    select_dragged(app, id);
    true
}

fn drop_on_graph(app: &mut IncaApp, id: &str, q: Pos2) -> bool {
    let Some(rect) = app.windows_state.hs.graph_rect else { return false };
    if !rect.contains(q) {
        return false;
    }
    let nodes = graph_nodes(app);
    let (pan, zoom) = (app.windows_state.hs.pan, app.windows_state.hs.zoom);
    let pos = app.windows_state.hs.pos.clone();
    for n in &nodes {
        let Some(p) = pos.get(&n.id) else { continue };
        let r = Rect::from_min_size(rect.min + pan + p.to_vec2() * zoom, vec2(NODE_W, node_h(n)) * zoom);
        if !r.contains(q) {
            continue;
        }
        if n.kind == Kind::Object && app.s.mats.contains_key(id) {
            assign_to_object(app, id, &n.id);
            select_dragged(app, id);
            return true;
        }
        if n.kind == Kind::Material && app.s.texs.contains_key(id) {
            let attr = n.inputs.iter().enumerate().map(|(i, s)| (in_port_pos(n, r, i, zoom).distance(q), s)).filter(|(d, _)| *d < 20.0 * zoom).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, s)| s.clone()).unwrap_or_else(|| app.s.mats.get(&n.id).map(|m| m.color_attr().to_string()).unwrap_or_else(|| "color".into()));
            connect_texture(app, id, &n.id, &attr);
            select_dragged(app, id);
            return true;
        }
        if n.kind == Kind::Object && app.s.texs.contains_key(id) {
            let mid = app.s.node(&n.id).and_then(|node| node.mesh().map(|m| m.material.clone())).unwrap_or_else(|| app.s.default_mat.clone());
            let attr = app.s.mats.get(&mid).map(|m| m.color_attr().to_string()).unwrap_or_else(|| "color".into());
            connect_texture(app, id, &mid, &attr);
            select_dragged(app, id);
            return true;
        }
        return false;
    }
    select_dragged(app, id);
    true
}

fn handle_palette_drop(app: &mut IncaApp, id: &str, q: Pos2) {
    if drop_on_graph(app, id, q) {
        return;
    }
    // Don't drop through the Hypershade window onto an object behind it.
    if app.windows_state.hs.window_rect.is_some_and(|r| r.contains(q)) {
        return;
    }
    drop_on_viewport(app, id, q);
}

fn graph_nodes(app: &IncaApp) -> Vec<GNode> {
    let sel = app.windows_state.hs_selected.clone().filter(|m| app.s.mats.contains_key(m));
    let only = app.windows_state.hs.selected_only;
    let mats: Vec<&inca_core::materials::Material> = app.s.mats.values().filter(|m| !only || Some(&m.id) == sel.as_ref()).collect();
    let mut out = vec![];
    let used: Vec<String> = mats.iter().flat_map(|m| m.maps.values().filter_map(|v| v.as_str().map(String::from))).collect();
    for t in app.s.texs.values().filter(|t| !only || used.contains(&t.id)) {
        out.push(GNode {
            id: t.id.clone(),
            kind: Kind::Texture,
            name: t.name.clone(),
            inputs: vec![],
            outputs: vec!["outColor".into(), "outAlpha".into()],
        });
    }
    for m in &mats {
        let slots = inca_core::materials::mat_type(&m.ty).mappable.iter().map(|s| s.to_string()).collect();
        out.push(GNode { id: m.id.clone(), kind: Kind::Material, name: m.name.clone(), inputs: slots, outputs: vec!["outColor".into()] });
        let name = if m.id == app.s.default_mat { "initialShadingGroup".into() } else { format!("{}SG", m.name) };
        out.push(GNode { id: format!("sg:{}", m.id), kind: Kind::ShadingGroup, name, inputs: vec!["surfaceShader".into()], outputs: vec![] });
    }
    if let Some(mid) = &sel {
        for o in users(app, mid) {
            out.push(GNode { id: o.clone(), kind: Kind::Object, name: app.s.name_of(&o), inputs: vec!["material".into()], outputs: vec![] });
        }
    }
    out
}

fn port_top(n: &GNode) -> f32 {
    HEAD_H + if n.kind == Kind::Object { 0.0 } else { SWATCH + 6.0 }
}

fn port_rows(n: &GNode) -> usize {
    (n.inputs.len() + n.outputs.len()).max(1)
}

fn node_h(n: &GNode) -> f32 {
    port_top(n) + ROW_H * port_rows(n) as f32 + 4.0
}

/// Default column layout: textures, materials, shading groups, objects.
fn default_pos(nodes: &[GNode], i: usize) -> Pos2 {
    let col = match nodes[i].kind {
        Kind::Texture => 0.0,
        Kind::Material | Kind::ShadingGroup => 220.0,
        Kind::Object => 440.0,
    };
    let y: f32 = nodes[..i].iter().filter(|n| n.kind == nodes[i].kind).map(|n| node_h(n) + 16.0).sum();
    pos2(col, y)
}

fn frame_graph(app: &mut IncaApp, rect: Rect, selected: bool) {
    let nodes = graph_nodes(app);
    let list: Vec<&GNode> = if selected && !app.windows_state.hs.graph_sel.is_empty() {
        nodes.iter().filter(|n| app.windows_state.hs.graph_sel.contains(&n.id)).collect()
    } else {
        nodes.iter().collect()
    };
    if list.is_empty() {
        return;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for n in list {
        let p = *app.windows_state.hs.pos.get(&n.id).unwrap_or(&pos2(0.0, 0.0));
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x + NODE_W);
        y1 = y1.max(p.y + node_h(n));
    }
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let zoom = ((rect.width() - 40.0) / w).min((rect.height() - 40.0) / h).clamp(0.2, 2.5);
    app.windows_state.hs.zoom = zoom;
    app.windows_state.hs.pan = vec2((rect.width() - w * zoom) * 0.5 - x0 * zoom, (rect.height() - h * zoom) * 0.5 - y0 * zoom);
}

fn port_pos(n: &GNode, r: Rect, row: usize, zoom: f32) -> Pos2 {
    pos2(r.min.x, r.min.y + (port_top(n) + ROW_H * (row as f32 + 0.5)) * zoom)
}

fn in_port_pos(n: &GNode, r: Rect, i: usize, zoom: f32) -> Pos2 {
    port_pos(n, r, n.outputs.len() + i, zoom)
}

fn out_port_pos(n: &GNode, r: Rect, i: usize, zoom: f32) -> Pos2 {
    pos2(r.max.x, port_pos(n, r, i, zoom).y)
}

fn wire_compatible(w: &Wire, n: &GNode, slot: Option<usize>) -> bool {
    match w {
        Wire::Tex(..) => n.kind == Kind::Material,
        Wire::Mat(_) => matches!(n.kind, Kind::Object | Kind::ShadingGroup) && (slot.is_none() || slot == Some(0)),
    }
}

fn graph_drop_target(w: &Wire, nodes: &[GNode], rects: &[Rect], q: Pos2, zoom: f32) -> Option<(usize, Option<usize>)> {
    let mut best: Option<(f32, usize, usize)> = None;
    for (i, n) in nodes.iter().enumerate() {
        for (si, _) in n.inputs.iter().enumerate() {
            if !wire_compatible(w, n, Some(si)) {
                continue;
            }
            let d = in_port_pos(n, rects[i], si, zoom).distance(q);
            if d < 22.0 && best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, i, si));
            }
        }
    }
    if let Some((_, i, si)) = best {
        return Some((i, Some(si)));
    }
    nodes.iter().enumerate().find(|(i, n)| rects[*i].contains(q) && wire_compatible(w, n, None)).map(|(i, _)| (i, None))
}

pub(crate) fn material_users(app: &IncaApp, mid: &str) -> Vec<String> {
    users(app, mid)
}

pub(crate) fn read_only(app: &IncaApp, id: &str) -> bool {
    // shortcut: name check; startup nodes are not renamed, so the names stay stable
    id == app.s.default_mat || app.s.mats.get(id).is_some_and(|m| m.name == "standardSurface1")
}

pub(crate) fn delete_graph_nodes(app: &mut IncaApp, ids: &HashSet<String>) {
    // Deleting a shading-group node deletes its material (Maya behavior).
    let ids: HashSet<String> = ids.iter().map(|id| id.strip_prefix("sg:").unwrap_or(id).to_string()).collect();
    let ids = &ids;
    let mut mats = vec![];
    let mut texs = vec![];
    let mut blocked = vec![];
    for id in ids {
        if let Some(m) = app.s.mats.get(id) {
            if read_only(app, id) {
                blocked.push(m.name.clone());
            } else {
                mats.push(id.clone());
            }
        } else if app.s.texs.contains_key(id) {
            texs.push(id.clone());
        }
    }
    for name in blocked {
        app.error(format!("file: C:/Program Files/Autodesk/Maya2020/scripts/others/hyperShadePanel.mel line 6837: Non-deletable node '{name}' cannot be deleted"));
    }
    if mats.is_empty() && texs.is_empty() {
        return;
    }
    app.s.checkpoint("delete shading nodes");
    for id in &mats {
        app.s.delete_material(id);
    }
    for id in &texs {
        for m in app.s.mats.values_mut() {
            m.maps.retain(|_, v| v.as_str() != Some(id.as_str()));
            m.map_outs.retain(|slot, _| m.maps.contains_key(slot));
        }
        if let Some(t) = app.s.texs.shift_remove(id) {
            app.s.release_name(&t.name);
        }
    }
    app.s.touch();
    for id in ids {
        app.windows_state.hs.pos.remove(id);
        app.windows_state.hs.pos.remove(&format!("sg:{id}"));
        app.windows_state.hs.graph_sel.remove(id);
        app.windows_state.hs.graph_sel.remove(&format!("sg:{id}"));
        if app.windows_state.hs_selected.as_deref() == Some(id.as_str()) {
            app.windows_state.hs_selected = None;
        }
        if app.snode_sel.as_deref() == Some(id.as_str()) {
            app.snode_sel = None;
        }
    }
}

fn rename_shading_node(app: &mut IncaApp, id: &str, name: &str) {
    if read_only(app, id) {
        app.error("line 1: Cannot rename a read only node.");
        return;
    }
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if let Some(m) = app.s.mats.get(id).cloned() {
        if m.name == name {
            return;
        }
        app.s.checkpoint("rename shading node");
        app.s.release_name(&m.name);
        let nn = app.s.unique_name(name);
        if let Some(m) = app.s.mats.get_mut(id) {
            m.name = nn;
        }
        app.s.touch();
    } else if let Some(t) = app.s.texs.get(id).cloned() {
        if t.name == name {
            return;
        }
        app.s.checkpoint("rename shading node");
        app.s.release_name(&t.name);
        let nn = app.s.unique_name(name);
        if let Some(t) = app.s.texs.get_mut(id) {
            t.name = nn;
        }
        app.s.touch();
    }
}

pub(crate) fn duplicate_shading_network(app: &mut IncaApp, mid: &str) {
    let Some(m) = app.s.mats.get(mid).cloned() else { return };
    app.s.checkpoint("duplicate shading network");
    let mut remap = HashMap::new();
    for tid in m.maps.values().filter_map(|v| v.as_str()) {
        if remap.contains_key(tid) {
            continue;
        }
        let Some(t) = app.s.texs.get(tid).cloned() else { continue };
        let nid = app.s.create_texture(&t.ty, Some(&t.name));
        if let Some(nt) = app.s.texs.get_mut(&nid) {
            nt.attrs = t.attrs;
        }
        remap.insert(tid.to_string(), nid);
    }
    let nid = app.s.create_material(&m.ty, Some(&m.name));
    if let Some(nm) = app.s.mats.get_mut(&nid) {
        nm.attrs = m.attrs;
        nm.maps = m.maps.iter().filter_map(|(k, v)| v.as_str().and_then(|t| remap.get(t).map(|n| (k.clone(), json!(n))))).collect();
        nm.map_outs = m.map_outs.clone();
    }
    app.s.touch();
    app.echo(format!("duplicate -upstreamNodes {};", m.name));
    app.windows_state.hs_selected = Some(nid.clone());
    app.windows_state.hs.graph_sel.clear();
    app.windows_state.hs.graph_sel.insert(nid);
}

fn menu_item(label: &str, act: Act) -> Item {
    Item { label: label.into(), act: Some(act), sub: vec![], check: None, enabled: true }
}

fn menu_sub(label: &str, sub: Vec<Item>) -> Item {
    Item { label: label.into(), act: None, sub, check: None, enabled: true }
}

fn open_node_marking(app: &mut IncaApp, id: &str, pos: Pos2, now: f64) {
    let name = app.s.mats.get(id).map(|m| m.name.clone()).or_else(|| app.s.texs.get(id).map(|t| t.name.clone())).or_else(|| app.s.node(id).map(|n| n.name.clone())).unwrap_or_default();
    let mut radial = vec![];
    let mut list = vec![];
    if app.s.mats.contains_key(id) {
        radial.push(("N", menu_item("Assign to Selection", Act::HypershadeAssign(id.into()))));
        radial.push(("E", menu_item("Select Objects With Material", Act::HypershadeSelectUsers(id.into()))));
        radial.push(("S", menu_item("Duplicate Network", Act::HypershadeDuplicate(id.into()))));
        radial.push(("W", menu_item("Rename...", Act::HypershadeRename(id.into()))));
        list.push(Some(menu_item("Frame Selected", Act::HypershadeFrameSelected)));
        list.push(Some(menu_item("Frame All", Act::HypershadeFrameAll)));
    } else if app.s.texs.contains_key(id) {
        radial.push(("N", menu_item("Rename...", Act::HypershadeRename(id.into()))));
        radial.push(("E", menu_item("Frame Selected", Act::HypershadeFrameSelected)));
        radial.push(("W", menu_item("Frame All", Act::HypershadeFrameAll)));
    } else if app.s.node(id).is_some() {
        radial.push(("N", menu_item("Select", Act::HypershadeSelectObject(id.into()))));
        radial.push(("E", menu_item("Frame Selected", Act::HypershadeFrameSelected)));
        radial.push(("S", menu_item("Frame All", Act::HypershadeFrameAll)));
    }
    open_custom(app, pos, Some(format!("{name}...")), radial, list, now);
}

fn open_empty_marking(app: &mut IncaApp, pos: Pos2, now: f64) {
    app.windows_state.hs.create_pos = Some(pos);
    let mats = MAT_TYPES.iter().map(|t| menu_item(t.label, Act::HypershadeCreateMaterial(t.ty.into()))).collect();
    let texs = TEX_TYPES.iter().map(|t| menu_item(t.label, Act::HypershadeCreateTexture(t.ty.into()))).collect();
    let radial = vec![
        ("N", menu_sub("Create Material", mats)),
        ("E", menu_sub("Create Texture", texs)),
        ("S", menu_item("Frame All", Act::HypershadeFrameAll)),
        ("W", menu_item("Rearrange", Act::HypershadeRearrange)),
    ];
    open_custom(app, pos, Some("Hypershade".into()), radial, vec![], now);
}

fn select_graph_node(app: &mut IncaApp, id: &str, additive: bool) {
    let selected_now;
    {
        let hs = &mut app.windows_state.hs;
        if additive {
            selected_now = hs.graph_sel.insert(id.to_string());
            if !selected_now {
                hs.graph_sel.remove(id);
            }
        } else if hs.graph_sel.contains(id) {
            selected_now = true;
        } else {
            hs.graph_sel.clear();
            hs.graph_sel.insert(id.to_string());
            selected_now = true;
        }
    }
    let target = id.strip_prefix("sg:").unwrap_or(id).to_string();
    if selected_now {
        if app.s.mats.contains_key(&target) || app.s.texs.contains_key(&target) {
            select_shading_node(app, &target);
        } else {
            app.select_names(std::slice::from_ref(&target), SelMode::Replace);
            app.windows_state.hs_selected = Some(target.clone());
            app.windows_state.hs.graph_sel.insert(target.clone());
        }
    } else if app.windows_state.hs_selected.as_deref() == Some(target.as_str()) {
        app.windows_state.hs_selected = None;
        if app.snode_sel.as_deref() == Some(target.as_str()) {
            app.snode_sel = None;
        }
    }
}

pub fn window(app: &mut IncaApp, ui: &mut egui::Ui) {
    app.windows_state.hs.window_rect = Some(ui.max_rect());
    // ---- menu bar
    ui.horizontal(|ui| {
        ui.menu_button("Edit", |ui| {
            let lead = app.snode_sel.clone().or_else(|| app.windows_state.hs_selected.clone()).filter(|id| app.s.mats.contains_key(id) || app.s.texs.contains_key(id));
            let mut delete_ids = app.windows_state.hs.graph_sel.clone();
            if let Some(id) = &app.snode_sel {
                delete_ids.insert(id.clone());
            }
            delete_ids.retain(|id| {
                let base = id.strip_prefix("sg:").unwrap_or(id);
                app.s.mats.contains_key(base) || app.s.texs.contains_key(base)
            });
            let can_delete = delete_ids.iter().any(|id| !read_only(app, id.strip_prefix("sg:").unwrap_or(id)));
            if ui.add_enabled(can_delete, egui::Button::new("Delete")).clicked() {
                delete_graph_nodes(app, &delete_ids);
                ui.close();
            }
            let is_mat = lead.as_deref().is_some_and(|id| app.s.mats.contains_key(id));
            if ui.add_enabled(is_mat, egui::Button::new("Duplicate Shading Network")).clicked() {
                if let Some(id) = lead.clone() {
                    duplicate_shading_network(app, &id);
                }
                ui.close();
            }
            if ui.add_enabled(lead.is_some(), egui::Button::new("Rename...")).clicked() {
                if let Some(id) = lead.clone() {
                    let name = app.s.mats.get(&id).map(|m| m.name.clone()).or_else(|| app.s.texs.get(&id).map(|t| t.name.clone())).unwrap_or_default();
                    app.windows_state.hs.rename_pos = Some(ui.min_rect().left_top());
                    app.windows_state.hs.rename = Some((id, name));
                }
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(is_mat, egui::Button::new("Select Objects with Materials")).clicked() {
                if let Some(id) = lead.clone() {
                    let ids = material_users(app, &id);
                    app.select_names(&ids, SelMode::Replace);
                }
                ui.close();
            }
            if ui.add_enabled(is_mat, egui::Button::new("Assign Material to Viewport Selection")).clicked() {
                if let Some(id) = lead.clone() {
                    if let Some(m) = app.s.mats.get(&id) {
                        let _ = app.run_mel("assign material", &format!("hyperShade -assign {};", m.name));
                    }
                }
                ui.close();
            }
        });
    });
    // ---- create bar
    ui.horizontal(|ui| {
        ui.label("Create:");
        ui.add(egui::TextEdit::singleline(&mut app.windows_state.hs.create_filter).hint_text("Filter nodes...").desired_width(120.0));
    });
    let create_filter = app.windows_state.hs.create_filter.to_lowercase();
    ui.horizontal_wrapped(|ui| {
        for t in MAT_TYPES {
            if !create_filter.is_empty() && !t.label.to_lowercase().contains(&create_filter) {
                continue;
            }
            if ui.small_button(t.label).on_hover_text(format!("Create a {} material", t.label)).clicked() {
                let _ = app.run_mel("create material", &format!("shadingNode -asShader {};", t.ty));
                if let Some(id) = app.s.mats.keys().last().cloned() {
                    place_new_node(app, &id, None);
                    app.windows_state.hs_selected = Some(id);
                }
            }
        }
        ui.separator();
        for t in TEX_TYPES {
            if !create_filter.is_empty() && !t.label.to_lowercase().contains(&create_filter) {
                continue;
            }
            if ui.small_button(t.label).on_hover_text(format!("Create a {} texture", t.label)).clicked() {
                let _ = app.run_mel("create texture", &format!("shadingNode -asTexture {};", t.ty));
                if let Some(id) = app.s.texs.keys().last().cloned() {
                    place_new_node(app, &id, None);
                    app.windows_state.hs_selected = Some(id);
                }
            }
        }
    });
    ui.separator();
    let side = 300.0f32; // fixed like the JS property panel; the work area takes the remainder
    let palette_w = (side * 0.75).min((ui.available_width() - side - 120.0).max(120.0));
    // egui panels divide the remaining width exactly; sizing the columns from
    // `available_width` inside a horizontal layout made the window grow every frame.
    egui::Panel::left("hypershade_palette")
        .exact_size(palette_w)
        .resizable(false)
        .show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut app.windows_state.hs.filter).hint_text("Search...").desired_width(ui.available_width()));
            ui.horizontal_wrapped(|ui| {
                for (i, label) in ["Materials", "Textures", "Lights", "Cameras", "Shading Groups"].into_iter().enumerate() {
                    ui.selectable_value(&mut app.windows_state.hs.tab, i, label);
                }
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("hspal").show(ui, |ui| palette(app, ui));
        });
    egui::Panel::right("hypershade_properties")
        .exact_size(side)
        .resizable(false)
        .show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("hsattr").show(ui, |ui| properties(app, ui));
        });
    // ---- work area
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Work Area").strong());
            ui.checkbox(&mut app.windows_state.hs.selected_only, "Graph selected network only");
            if ui.small_button("Frame All").on_hover_text("Fit the whole graph").clicked() {
                app.windows_state.hs.frame = Some(false);
            }
            if ui.small_button("Frame Sel").on_hover_text("Fit the selected nodes").clicked() {
                app.windows_state.hs.frame = Some(true);
            }
            if ui.small_button("Rearrange").clicked() {
                app.windows_state.hs.pos.clear();
                app.windows_state.hs.frame = Some(false);
            }
            if ui.small_button("Connection Editor").on_hover_text("Load the selected material or texture into the Connection Editor").clicked() {
                crate::hypergraph::open_for_hypershade(app);
            }
        });
        work_area(app, ui);
    });
    if let Some((id, mut name)) = app.windows_state.hs.rename.take() {
        let pos = app.windows_state.hs.rename_pos.take();
        let mut open = true;
        let mut commit = false;
        let mut cancel = false;
        let mut win = egui::Window::new("Rename Node").collapsible(false).resizable(false);
        if let Some(p) = pos {
            win = win.fixed_pos(pos2(p.x - 85.0, p.y + 84.0));
        }
        win.open(&mut open).show(ui.ctx(), |ui| {
            let resp = ui.text_edit_singleline(&mut name);
            ui.horizontal(|ui| {
                if ui.button("Rename").clicked() {
                    commit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && (resp.has_focus() || resp.lost_focus()) {
                commit = true;
            }
        });
        if commit {
            rename_shading_node(app, &id, &name);
        } else if open && !cancel {
            app.windows_state.hs.rename = Some((id, name));
        }
    }
}

fn palette(app: &mut IncaApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let filter = app.windows_state.hs.filter.to_lowercase();
    let mut items: Vec<(String, String, Option<String>)> = vec![];
    match app.windows_state.hs.tab {
        0 => {
            for m in app.s.mats.values() {
                items.push((m.id.clone(), m.name.clone(), None));
            }
        }
        1 => {
            for t in app.s.texs.values() {
                items.push((t.id.clone(), t.name.clone(), None));
            }
        }
        2 => {
            for n in app.s.nodes.values() {
                if let Shape::Light(l) = &n.shape {
                    items.push((n.id.clone(), n.name.clone(), Some(l.ty.clone())));
                }
            }
        }
        3 => {
            for n in app.s.nodes.values() {
                if matches!(n.shape, Shape::Camera(_)) {
                    items.push((n.id.clone(), n.name.clone(), Some("camera".into())));
                }
            }
        }
        _ => {
            for m in app.s.mats.values() {
                let label = if m.id == app.s.default_mat { "initialShadingGroup".to_string() } else { format!("{}SG", m.name) };
                items.push((m.id.clone(), label, None));
            }
        }
    }
    items.retain(|(_, label, _)| filter.is_empty() || label.to_lowercase().contains(&filter));
    // Fixed-width grid: wrap by whole tiles instead of relying on label widths.
    let cols = (((ui.available_width() + 4.0) / 60.0).floor() as usize).max(1);
    for row in items.chunks(cols) {
        ui.horizontal(|ui| {
            for (id, name, icon) in row {
                let sel = app.windows_state.hs_selected.as_deref() == Some(id.as_str());
                let tex = swatch(app, &ctx, id);
                ui.vertical(|ui| {
                    ui.set_width(56.0);
                    let (r, resp) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::click_and_drag());
                    ui.painter().rect_filled(r, 2.0, if sel { theme::HI } else { theme::BG_DARK });
                    if let Some(t) = tex {
                        ui.painter().image(t, r.shrink(3.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    } else if let Some(icon) = icon {
                        crate::icons::paint(ui, r.shrink(6.0), icon, Color32::WHITE);
                    }
                    ui.add_sized([56.0, 14.0], egui::Label::new(RichText::new(name).size(10.0)).truncate());
                    if resp.clicked() {
                        app.windows_state.hs.graph_sel.clear();
                        if app.s.mats.contains_key(id) || app.s.texs.contains_key(id) {
                            app.windows_state.hs_selected = Some(id.clone());
                            app.windows_state.hs.graph_sel.insert(id.clone());
                            select_shading_node(app, id);
                        } else if app.s.node(id).is_some() {
                            app.select_names(std::slice::from_ref(id), SelMode::Replace);
                            app.windows_state.hs_selected = Some(id.clone());
                        }
                    }
                    if resp.double_clicked() && app.s.mats.contains_key(id) {
                        let _ = app.run_mel("assign material", &format!("hyperShade -assign {name};"));
                    }
                    if resp.drag_started() {
                        app.windows_state.hs.palette_drag = Some(id.clone());
                    }
                    if resp.drag_stopped() {
                        if let Some(drag) = app.windows_state.hs.palette_drag.take() {
                            if let Some(q) = ctx.pointer_hover_pos() {
                                handle_palette_drop(app, &drag, q);
                            }
                        }
                    }
                    resp.on_hover_text(format!("{name} — click to edit, drag to assign/connect"));
                });
            }
        });
    }
    if let (Some(id), Some(q)) = (app.windows_state.hs.palette_drag.clone(), ctx.pointer_hover_pos()) {
        let name = app.s.mats.get(&id).map(|m| m.name.clone()).or_else(|| app.s.texs.get(&id).map(|t| t.name.clone())).unwrap_or_default();
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("hs-palette-drag")));
        let rect = Rect::from_min_size(q + vec2(12.0, 12.0), vec2(120.0, 20.0));
        p.rect_filled(rect, 3.0, Color32::from_gray(0x44));
        p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_gray(0x2a)), egui::StrokeKind::Inside);
        p.text(rect.min + vec2(6.0, 10.0), egui::Align2::LEFT_CENTER, format!("{name}"), egui::FontId::proportional(11.0), Color32::WHITE);
    }
}

fn texture_icon(ty: &str) -> &str {
    match ty {
        "file" => "fileTex",
        "grid" => "gridTex",
        other => other,
    }
}

fn property_tabs(app: &IncaApp, id: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    if let Some(m) = app.s.mats.get(id) {
        out.push((m.id.clone(), m.name.clone()));
        let mut seen = HashSet::new();
        for tid in m.maps.values().filter_map(|v| v.as_str()) {
            if seen.insert(tid.to_string()) {
                if let Some(t) = app.s.texs.get(tid) {
                    out.push((t.id.clone(), t.name.clone()));
                }
            }
        }
    } else if let Some(t) = app.s.texs.get(id) {
        out.push((t.id.clone(), t.name.clone()));
        for m in app.s.mats.values().filter(|m| m.maps.values().any(|v| v.as_str() == Some(id))) {
            out.push((m.id.clone(), m.name.clone()));
        }
    } else if let Some(n) = app.s.node(id) {
        out.push((n.id.clone(), n.name.clone()));
        if let Some(mid) = n.mesh().map(|m| m.material.clone()) {
            if let Some(m) = app.s.mats.get(&mid) {
                out.push((m.id.clone(), m.name.clone()));
                let mut seen = HashSet::new();
                for tid in m.maps.values().filter_map(|v| v.as_str()) {
                    if seen.insert(tid.to_string()) {
                        if let Some(t) = app.s.texs.get(tid) {
                            out.push((t.id.clone(), t.name.clone()));
                        }
                    }
                }
            }
        }
    }
    out
}

fn property_header(app: &mut IncaApp, ui: &mut egui::Ui, id: &str) {
    let (name, ty, icon, editable) = if let Some(m) = app.s.mats.get(id) {
        (m.name.clone(), inca_core::materials::mat_type(&m.ty).label.to_string(), m.ty.clone(), true)
    } else if let Some(t) = app.s.texs.get(id) {
        let label = inca_core::materials::tex_type(&t.ty).map(|x| x.label).unwrap_or(&t.ty).to_string();
        (t.name.clone(), label, texture_icon(&t.ty).to_string(), true)
    } else if let Some(n) = app.s.node(id) {
        let (ty, icon) = match &n.shape {
            Shape::Light(l) => (l.ty.clone(), l.ty.clone()),
            Shape::Camera(_) => ("camera".into(), "camera".into()),
            _ => (n.kind.clone(), "hypershade".into()),
        };
        (n.name.clone(), ty, icon, false)
    } else {
        return;
    };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        crate::icons::paint(ui, r.shrink(2.0), &icon, Color32::WHITE);
        if editable {
            let mut new_name = name.clone();
            let resp = ui.text_edit_singleline(&mut new_name);
            let committed = resp.lost_focus() || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if committed && new_name != name && !new_name.trim().is_empty() {
                rename_shading_node(app, id, &new_name);
            }
        } else {
            ui.label(RichText::new(&name).strong());
        }
        ui.label(RichText::new(&ty).color(theme::TEXT_DIM));
    });
}

fn properties(app: &mut IncaApp, ui: &mut egui::Ui) {
    let Some(id) = app.windows_state.hs_selected.clone() else {
        ui.label(RichText::new("Select a node to edit it").color(theme::TEXT_DIM));
        return;
    };
    // Two-column attribute rows must fit the narrow properties panel; the default 280px
    // text field overflowed it and pushed the whole grid left under the work area.
    ui.set_max_width(ui.available_width());
    ui.spacing_mut().text_edit_width = 120.0;
    property_header(app, ui, &id);
    let tabs = property_tabs(app, &id);
    if tabs.len() > 1 {
        let mut pick = None;
        ui.horizontal_wrapped(|ui| {
            for (tid, label) in &tabs {
                if ui.selectable_label(tid == &id, label).clicked() {
                    pick = Some(tid.clone());
                }
            }
        });
        if let Some(tid) = pick {
            select_graph_node(app, &tid, false);
            return;
        }
    }
    ui.separator();
    if app.s.mats.contains_key(&id) {
        ui.horizontal(|ui| {
            if ui.button("Assign to Selection").clicked() {
                let n = app.s.mats[&id].name.clone();
                let _ = app.run_mel("assign material", &format!("hyperShade -assign {n};"));
            }
            if !read_only(app, &id) && ui.button("Delete").clicked() {
                app.s.checkpoint("delete material");
                app.s.delete_material(&id);
                app.windows_state.hs_selected = None;
                app.snode_sel = None;
            }
        });
        if app.s.mats.contains_key(&id) {
            crate::windows::material_editor(app, ui, &id, false);
        }
    } else if let Some(t) = app.s.texs.get(&id).cloned() {
        crate::panels::texture_fields_editor(app, ui, &id, &t.ty);
        if t.ty == "file" && ui.button("Browse image...").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "hdr"]).pick_file() {
                app.s.set_attr(&NodeRef::Texture(id.clone()), "fileTextureName", json!(p.to_string_lossy()));
            }
        }
        if ui.button("Delete").clicked() {
            app.s.checkpoint("delete texture");
            for m in app.s.mats.values_mut() {
                m.maps.retain(|_, v| v.as_str() != Some(id.as_str()));
                m.map_outs.retain(|slot, _| m.maps.contains_key(slot));
            }
            app.s.texs.shift_remove(&id);
            app.s.touch();
            app.windows_state.hs_selected = None;
            app.snode_sel = None;
        }
        let conns: Vec<(String, String)> = app.s.mats.values().flat_map(|m| {
            m.maps.iter().filter(|(_, v)| v.as_str() == Some(id.as_str())).map(move |(slot, _)| (m.id.clone(), format!("{}.{}", m.name, slot)))
        }).collect();
        if !conns.is_empty() {
            theme::section_header(ui, "Connections");
            for (mid, label) in conns {
                if ui.button(label).clicked() {
                    select_graph_node(app, &mid, false);
                    return;
                }
            }
        }
    } else if let Some(n) = app.s.node(&id) {
        let keys: Vec<String> = match &n.shape {
            Shape::Light(l) => l.attrs.keys().cloned().collect(),
            Shape::Camera(_) => app.s.shape_channels(&id),
            _ => ["translateX", "translateY", "translateZ", "rotateX", "rotateY", "rotateZ", "scaleX", "scaleY", "scaleZ", "visibility"].iter().map(|s| s.to_string()).collect(),
        };
        theme::section_header(ui, "Attributes");
        crate::panels::attr_map_editor(app, ui, &NodeRef::Dag(id.clone()), &keys);
    } else {
        ui.label(RichText::new("Select a node to edit it").color(theme::TEXT_DIM));
    }
}

fn bezier(p: &egui::Painter, a: Pos2, b: Pos2, col: Color32, w: f32) {
    let d = ((b.x - a.x).abs() * 0.5).max(30.0);
    let (c1, c2) = (a + vec2(d, 0.0), b - vec2(d, 0.0));
    let point = |t: f32| {
        let u = 1.0 - t;
        pos2(
            u * u * u * a.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * b.x,
            u * u * u * a.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * b.y,
        )
    };
    let pts: Vec<Pos2> = (0..=24).map(|i| point(i as f32 / 24.0)).collect();
    p.add(egui::Shape::line(pts, Stroke::new(w, col)));
    // Direction marker near the destination.
    let t = 0.78;
    let u = 1.0 - t;
    let pt = point(t);
    let tangent = (c1 - a) * (3.0 * u * u) + (c2 - c1) * (6.0 * u * t) + (b - c2) * (3.0 * t * t);
    if tangent.length_sq() > 1e-6 {
        let dir = tangent.normalized();
        let perp = vec2(-dir.y, dir.x) * 2.5;
        let base = pt - dir * 4.0;
        p.add(egui::Shape::convex_polygon(vec![pt + dir * 2.0, base + perp, base - perp], col, Stroke::NONE));
    }
}

fn work_area(app: &mut IncaApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    app.windows_state.hs.graph_rect = Some(rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(38, 38, 38));
    let nodes = graph_nodes(app);
    // positions (graph space) -> screen
    for (i, n) in nodes.iter().enumerate() {
        if !app.windows_state.hs.pos.contains_key(&n.id) {
            // Shading groups sit to the right of their material.
            let p = n
                .id
                .strip_prefix("sg:")
                .and_then(|mid| app.windows_state.hs.pos.get(mid).copied())
                .map(|m| m + vec2(NODE_W + 36.0, 0.0))
                .unwrap_or_else(|| default_pos(&nodes, i));
            app.windows_state.hs.pos.insert(n.id.clone(), p);
        }
    }
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    if pointer.is_some_and(|q| rect.contains(q)) {
        if ctx.input(|i| i.key_pressed(egui::Key::A)) {
            app.windows_state.hs.frame = Some(false);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            app.windows_state.hs.frame = Some(true);
        }
    }
    if let Some(sel) = app.windows_state.hs.frame.take() {
        frame_graph(app, rect, sel);
    }
    let (pan, zoom) = (app.windows_state.hs.pan, app.windows_state.hs.zoom);
    let to_scr = |p: Pos2| rect.min + pan + p.to_vec2() * zoom;
    let node_rect = |n: &GNode, pos: &HashMap<String, Pos2>| Rect::from_min_size(to_scr(pos[&n.id]), vec2(NODE_W, node_h(n)) * zoom);
    let pos = app.windows_state.hs.pos.clone();
    let rects: Vec<Rect> = nodes.iter().map(|n| node_rect(n, &pos)).collect();
    let find = |id: &str| nodes.iter().position(|n| n.id == id);
    let wire = app.windows_state.hs.wire.clone();
    let drop_target = wire.as_ref().and_then(|w| pointer.and_then(|q| graph_drop_target(w, &nodes, &rects, q, zoom)));
    // ---- wires
    for (mi, m) in nodes.iter().enumerate().filter(|(_, n)| n.kind == Kind::Material) {
        let Some(mat) = app.s.mats.get(&m.id) else { continue };
        for (si, slot) in m.inputs.iter().enumerate() {
            if let Some(ti) = mat.maps.get(slot).and_then(|v| v.as_str()).and_then(find) {
                let out = if mat.map_out(slot) == "outAlpha" { 1 } else { 0 };
                bezier(&painter, out_port_pos(&nodes[ti], rects[ti], out, zoom), in_port_pos(m, rects[mi], si, zoom), Color32::from_rgb(110, 190, 255), 2.0);
            }
        }
        for (oi, o) in nodes.iter().enumerate().filter(|(_, n)| n.kind == Kind::Object) {
            if users(app, &m.id).contains(&o.id) {
                bezier(&painter, out_port_pos(m, rects[mi], 0, zoom), in_port_pos(o, rects[oi], 0, zoom), Color32::from_rgb(120, 220, 120), 2.0);
            }
        }
        if let Some(si) = find(&format!("sg:{}", m.id)) {
            bezier(&painter, out_port_pos(m, rects[mi], 0, zoom), in_port_pos(&nodes[si], rects[si], 0, zoom), Color32::from_rgb(120, 220, 120), 2.0);
        }
    }
    // ---- nodes
    let font = egui::FontId::proportional(11.0 * zoom.clamp(0.6, 1.4));
    let wire_src = wire.as_ref().map(|w| match w {
        Wire::Tex(t, _) | Wire::Mat(t) => t.as_str(),
    });
    let mut hovered_port: Option<(usize, Option<usize>)> = None; // (node, input index or None = body)
    for (i, n) in nodes.iter().enumerate() {
        let r = rects[i];
        if !rect.intersects(r) {
            continue;
        }
        let sel = app.windows_state.hs.graph_sel.contains(&n.id) || (n.kind == Kind::Object && app.s.sel.contains(&n.id));
        let is_source = wire_src == Some(n.id.as_str());
        let accepts = wire.as_ref().is_none_or(|w| n.inputs.iter().enumerate().any(|(si, _)| wire_compatible(w, n, Some(si))) || wire_compatible(w, n, None));
        let inactive = wire.is_some() && !is_source && !accepts;
        let target_node = drop_target.is_some_and(|(ti, _)| ti == i);
        let head = match n.kind {
            Kind::Texture => Color32::from_rgb(120, 90, 50),
            Kind::Material => Color32::from_rgb(60, 95, 130),
            Kind::ShadingGroup => Color32::from_rgb(79, 91, 69),
            Kind::Object => Color32::from_rgb(70, 110, 70),
        };
        let head = if inactive { head.gamma_multiply(0.45) } else { head };
        painter.rect_filled(r, 4.0, if inactive { Color32::from_rgb(44, 44, 44) } else { Color32::from_rgb(58, 58, 58) });
        painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), HEAD_H * zoom)), 4.0, head);
        if target_node {
            painter.rect_stroke(r, 4.0, Stroke::new(2.0, Color32::from_rgb(255, 230, 80)), egui::StrokeKind::Outside);
        } else if sel {
            painter.rect_stroke(r, 4.0, Stroke::new(2.0, theme::HI), egui::StrokeKind::Outside);
        }
        painter.text(r.min + vec2(6.0, HEAD_H * 0.5 * zoom), egui::Align2::LEFT_CENTER, &n.name, font.clone(), if inactive { Color32::from_gray(120) } else { Color32::WHITE });
        if n.kind != Kind::Object {
            let sr = Rect::from_min_size(r.min + vec2(6.0, (HEAD_H + 3.0) * zoom), vec2(SWATCH, SWATCH) * zoom);
            if let Some(t) = swatch(app, &ctx, &n.id) {
                painter.image(t, sr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else if n.kind == Kind::ShadingGroup {
                painter.rect_filled(sr, 1.0, Color32::from_gray(0x2b));
                painter.rect_stroke(sr.shrink(8.0 * zoom), 0.0, Stroke::new(1.0, Color32::from_rgb(120, 220, 120)), egui::StrokeKind::Inside);
                painter.text(sr.center(), egui::Align2::CENTER_CENTER, "SG", egui::FontId::proportional(10.0 * zoom), Color32::from_gray(0x9a));
            }
        }
        for (si, slot) in n.inputs.iter().enumerate() {
            let p = in_port_pos(n, r, si, zoom);
            let connected = n.kind == Kind::ShadingGroup || app.s.mats.get(&n.id).is_some_and(|m| m.maps.contains_key(slot));
            let compatible = wire.as_ref().is_none_or(|w| wire_compatible(w, n, Some(si)));
            let target = drop_target == Some((i, Some(si))) || (drop_target == Some((i, None)) && si == 0);
            let col = if target {
                Color32::from_rgb(255, 230, 80)
            } else if wire.is_some() && !compatible {
                Color32::from_gray(70)
            } else if wire.is_some() {
                Color32::from_rgb(120, 220, 120)
            } else if connected {
                if n.kind == Kind::ShadingGroup { Color32::from_rgb(120, 220, 120) } else { Color32::from_rgb(110, 190, 255) }
            } else {
                Color32::from_gray(150)
            };
            painter.circle_filled(p, if target { 5.5 * zoom } else { 4.0 * zoom }, col);
            painter.text(p + vec2(8.0 * zoom, 0.0), egui::Align2::LEFT_CENTER, inca_core::ops::nice_name(slot), font.clone(), if wire.is_some() && !compatible { Color32::from_gray(80) } else { theme::TEXT_DIM });
            if pointer.is_some_and(|q| q.distance(p) < 8.0 * zoom.max(0.8)) {
                hovered_port = Some((i, Some(si)));
            }
        }
        for (oi, port) in n.outputs.iter().enumerate() {
            let p = out_port_pos(n, r, oi, zoom);
            let source_out = match &wire {
                Some(Wire::Tex(t, wi)) => *t == n.id && *wi == oi,
                Some(Wire::Mat(t)) => *t == n.id && oi == 0,
                None => false,
            };
            let alpha = port == "outAlpha";
            let col = if wire.is_some() && !source_out {
                Color32::from_gray(70)
            } else if alpha {
                Color32::from_rgb(127, 208, 232)
            } else {
                Color32::from_rgb(230, 200, 90)
            };
            painter.circle_filled(p, if source_out { 5.5 * zoom } else { 4.5 * zoom }, col);
            painter.text(p - vec2(8.0 * zoom, 0.0), egui::Align2::RIGHT_CENTER, inca_core::ops::nice_name(port), font.clone(), if wire.is_some() && !source_out { Color32::from_gray(80) } else { theme::TEXT_DIM });
        }
        if hovered_port.is_none() && pointer.is_some_and(|q| r.contains(q)) {
            hovered_port = Some((i, None));
        }
    }
    // ---- interaction
    let press = ctx.input(|i| i.pointer.press_origin());
    let additive = ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command);
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = press {
            // an output port starts a wire, a node body moves the selection, empty space marquees
            let out_hit = nodes.iter().enumerate().find_map(|(i, n)| {
                n.outputs.iter().enumerate().find_map(|(oi, _)| (out_port_pos(n, rects[i], oi, zoom).distance(p) < 9.0).then_some((i, oi)))
            });
            let in_hit = nodes.iter().enumerate().find_map(|(i, n)| {
                n.inputs.iter().enumerate().find_map(|(si, _)| (in_port_pos(n, rects[i], si, zoom).distance(p) < 9.0).then_some((i, si)))
            });
            if let Some((i, oi)) = out_hit {
                let n = &nodes[i];
                app.windows_state.hs.wire = match n.kind {
                    Kind::Texture => Some(Wire::Tex(n.id.clone(), oi)),
                    Kind::Material => Some(Wire::Mat(n.id.clone())),
                    _ => None,
                };
            } else if let Some((i, si)) = in_hit {
                let mid = nodes[i].id.clone();
                let attr = nodes[i].inputs[si].clone();
                if let Some(tex) = app.s.mats.get(&mid).and_then(|m| m.maps.get(&attr)).and_then(|v| v.as_str()).map(String::from) {
                    let out = if app.s.mats[&mid].map_out(&attr) == "outAlpha" { 1 } else { 0 };
                    app.s.checkpoint("disconnect texture");
                    app.s.mats.get_mut(&mid).unwrap().clear_map(&attr);
                    app.s.touch();
                    app.s.echo(format!("disconnectAttr {}.{attr};", app.s.mats[&mid].name));
                    app.windows_state.hs.wire = Some(Wire::Tex(tex, out));
                }
            } else if let Some(i) = rects.iter().rposition(|r| r.contains(p)) {
                select_graph_node(app, &nodes[i].id, additive);
                let start = app.windows_state.hs.graph_sel.iter().filter_map(|id| app.windows_state.hs.pos.get(id).map(|p| (id.clone(), *p))).collect();
                app.windows_state.hs.drag_start = Some(start);
            } else {
                let base = if additive { app.windows_state.hs.graph_sel.clone() } else { HashSet::new() };
                if !additive {
                    app.windows_state.hs.graph_sel.clear();
                }
                app.windows_state.hs.marquee_base = base;
                app.windows_state.hs.marquee = Some((p, p));
            }
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle) {
        app.windows_state.hs.pan += resp.drag_delta();
    }
    if let Some(d) = app.windows_state.hs.drag_start.clone() {
        let delta = resp.drag_delta() / zoom;
        for (id, _) in d {
            if let Some(p) = app.windows_state.hs.pos.get_mut(&id) {
                *p += delta;
            }
        }
    }
    if let Some((a, b)) = app.windows_state.hs.marquee {
        let q = pointer.unwrap_or(b);
        app.windows_state.hs.marquee = Some((a, q));
        let m = Rect::from_two_pos(a, q);
        let mut sel = app.windows_state.hs.marquee_base.clone();
        for (i, n) in nodes.iter().enumerate() {
            if m.intersects(rects[i]) {
                sel.insert(n.id.clone());
            }
        }
        app.windows_state.hs.graph_sel = sel;
        painter.rect_filled(m, 0.0, Color32::from_rgba_unmultiplied(120, 170, 220, 30));
        painter.rect_stroke(m, 0.0, Stroke::new(1.0, Color32::from_rgb(156, 208, 245)), egui::StrokeKind::Inside);
    }
    if let (Some(w), Some(q)) = (wire.clone(), pointer) {
        let src = match &w {
            Wire::Tex(t, oi) => find(t).map(|i| out_port_pos(&nodes[i], rects[i], (*oi).min(nodes[i].outputs.len().saturating_sub(1)), zoom)),
            Wire::Mat(t) => find(t).map(|i| out_port_pos(&nodes[i], rects[i], 0, zoom)),
        };
        if let Some(a) = src {
            let end = drop_target.map(|(i, slot)| in_port_pos(&nodes[i], rects[i], slot.unwrap_or(0), zoom)).unwrap_or(q);
            bezier(&painter, a, end, Color32::from_rgb(255, 230, 120), 1.5);
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        if let Some(w) = app.windows_state.hs.wire.take() {
            match (w, drop_target) {
                (Wire::Tex(t, oi), Some((ni, slot))) if nodes[ni].kind == Kind::Material => {
                    let mid = nodes[ni].id.clone();
                    let attr = match slot {
                        Some(si) => nodes[ni].inputs[si].clone(),
                        None => app.s.mats[&mid].color_attr().to_string(),
                    };
                    let out = if oi == 1 { "outAlpha" } else { "outColor" };
                    app.s.checkpoint("connect texture");
                    app.s.mats.get_mut(&mid).unwrap().set_map(&attr, &t, out);
                    app.s.touch();
                    app.s.echo(format!("connectAttr -force {}.{out} {}.{attr};", app.s.texs.get(&t).map(|x| x.name.clone()).unwrap_or_default(), app.s.mats[&mid].name));
                }
                (Wire::Mat(m), Some((ni, _))) if nodes[ni].kind == Kind::Object => {
                    let name = app.s.mats[&m].name.clone();
                    let _ = app.run_mel("assign material", &format!("select -r {}; hyperShade -assign {name};", nodes[ni].name));
                }
                (Wire::Mat(m), Some((ni, _))) if nodes[ni].kind == Kind::ShadingGroup => {
                    let target = nodes[ni].id.strip_prefix("sg:").unwrap_or("").to_string();
                    if target != m {
                        // Reroute whole-object members only; face assignments keep their material.
                        let members: Vec<String> = app
                            .s
                            .nodes
                            .values()
                            .filter(|n| n.mesh().is_some_and(|mesh| mesh.material == target))
                            .map(|n| n.id.clone())
                            .collect();
                        if members.is_empty() {
                            app.warn(format!("{} has no members.", nodes[ni].name));
                        } else {
                            let name = app.s.mats[&m].name.clone();
                            app.s.checkpoint("assign material");
                            for o in members {
                                app.s.assign_material(&o, &m, None);
                            }
                            app.s.touch();
                            app.echo(format!("hyperShade -assign {name};"));
                        }
                    }
                }
                _ => {}
            }
        }
        app.windows_state.hs.drag_start = None;
        if app.windows_state.hs.marquee.take().is_some() {
            app.windows_state.hs_selected = nodes
                .iter()
                .rev()
                .find(|n| app.windows_state.hs.graph_sel.contains(&n.id))
                .map(|n| n.id.strip_prefix("sg:").unwrap_or(&n.id).to_string());
            app.windows_state.hs.marquee_base.clear();
        }
    }
    if resp.clicked() {
        if let Some((i, _)) = hovered_port {
            let id = nodes[i].id.clone();
            select_graph_node(app, &id, additive);
        } else {
            app.windows_state.hs.graph_sel.clear();
            app.windows_state.hs_selected = None;
            app.select_names(&[], SelMode::Replace);
        }
    }
    if resp.double_clicked() {
        if let Some((i, None)) = hovered_port {
            if nodes[i].kind == Kind::Material {
                let _ = app.run_mel("assign material", &format!("hyperShade -assign {};", nodes[i].name));
            }
        }
    }
    // right press opens the node/work-area marking menu; input ports keep the quick break
    if app.marking.is_none() && resp.hovered() && ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
        if !matches!(hovered_port, Some((_, Some(_)))) {
            let now = ctx.input(|i| i.time);
            let pos = pointer.unwrap_or(rect.center());
            if let Some(i) = pointer.and_then(|q| rects.iter().rposition(|r| r.contains(q))) {
                // RMB opens the node menu without selecting, so a viewport selection can
                // still be the target of "Assign to Selection" (Maya behavior).
                let id = nodes[i].id.strip_prefix("sg:").unwrap_or(&nodes[i].id).to_string();
                open_node_marking(app, &id, pos, now);
            } else {
                open_empty_marking(app, pos, now);
            }
        }
    }
    // right-click a connected input to break the connection
    if app.marking.is_none() && resp.secondary_clicked() {
        if let Some((i, Some(si))) = hovered_port {
            let mid = nodes[i].id.clone();
            let attr = nodes[i].inputs[si].clone();
            if app.s.mats.get(&mid).is_some_and(|m| m.maps.contains_key(&attr)) {
                app.s.checkpoint("disconnect texture");
                app.s.mats.get_mut(&mid).unwrap().clear_map(&attr);
                app.s.touch();
                app.s.echo(format!("disconnectAttr {}.{attr};", app.s.mats[&mid].name));
            }
        }
    }
    // wheel zooms around the cursor
    if let Some(q) = pointer.filter(|q| rect.contains(*q)) {
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let nz = (zoom * (1.0 + scroll * 0.002)).clamp(0.3, 2.5);
            let g = (q - rect.min - pan) / zoom;
            app.windows_state.hs.zoom = nz;
            app.windows_state.hs.pan = q - rect.min - g * nz;
        }
    }
    painter.text(rect.left_bottom() + vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, "Drag output to connect · Shift/Ctrl multi-select · primary-drag empty space to marquee · MMB pan · wheel zoom · A/F frame", egui::FontId::proportional(10.0), theme::TEXT_DIM);

}
