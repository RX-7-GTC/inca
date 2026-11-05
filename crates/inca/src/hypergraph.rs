//! Hypergraph: the DAG as a top-down tree, or the selected objects'
//! input and output connections (transform → history → shape → material); and the Connection
//! Editor for direct attribute connections.

use crate::app::IncaApp;
use crate::theme;
use egui::{pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use inca_core::scene::{NodeRef, Scene, SelMode, Shape};
use std::collections::HashMap;

const NW: f32 = 112.0;
const NH: f32 = 46.0;
const GX: f32 = 16.0;
const GY: f32 = 34.0;

pub struct HgState {
    pub connections: bool,
    /// objects graphed in the connections view
    pub io: Vec<String>,
    pan: Vec2,
    zoom: f32,
    pub show_cameras: bool,
    pub show_lights: bool,
    pub show_curves: bool,
    pub show_locators: bool,
    pub show_shapes: bool,
    /// fit the graph on the next draw (all nodes, or the selection only)
    fit: Option<Fit>,
    /// primary-button marquee start in screen coordinates
    marquee: Option<Pos2>,
    drag: Option<String>,
    /// connection-view middle-drag node (index into the drawn boxes)
    conn_drag: Option<usize>,
    /// box last clicked in the connection view (history / shape / material feedback)
    pub active_key: Option<String>,
    /// inline rename target (old name, edit buffer)
    pub rename: Option<(String, String)>,
    // Connection Editor
    pub ce_left: Option<NodeRef>,
    pub ce_right: Option<NodeRef>,
    ce_attr: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    All,
    Selection,
}

impl Default for HgState {
    fn default() -> Self {
        HgState {
            connections: false,
            io: vec![],
            pan: vec2(70.0, 30.0),
            zoom: 1.0,
            show_cameras: false,
            show_lights: true,
            show_curves: true,
            show_locators: true,
            show_shapes: false,
            fit: Some(Fit::All),
            marquee: None,
            drag: None,
            conn_drag: None,
            active_key: None,
            rename: None,
            ce_left: None,
            ce_right: None,
            ce_attr: None,
        }
    }
}

impl HgState {
    /// Frame the whole graph the next time it is drawn (menu / open action).
    pub fn frame_all(&mut self) {
        self.fit = Some(Fit::All);
    }
    /// Frame the selected objects the next time the graph is drawn.
    pub fn frame_selection(&mut self) {
        self.fit = Some(Fit::Selection);
    }
}

/// A box on the graph: what it stands for, its label lines and where it is (graph space).
struct Box_ {
    key: String,
    obj: Option<String>,
    label: String,
    sub: String,
    at: Pos2,
    material: bool,
    /// the node the box represents, for the Connection Editor drag
    node: Option<NodeRef>,
}

fn kind_label(s: &Scene, id: &str) -> String {
    let Some(n) = s.node(id) else { return String::new() };
    match &n.shape {
        Shape::Mesh(_) => "mesh".into(),
        Shape::Light(l) => l.ty.clone(),
        Shape::Camera(_) => "camera".into(),
        Shape::Curve(_) => "nurbsCurve".into(),
        Shape::Group => "transform".into(),
        _ => n.kind.clone(),
    }
}

fn visible_kind(s: &Scene, hg: &HgState, id: &str) -> bool {
    let Some(n) = s.node(id) else { return false };
    match &n.shape {
        Shape::Camera(c) => !c.startup || hg.show_cameras,
        Shape::Light(_) => hg.show_lights,
        Shape::Curve(_) => hg.show_curves,
        Shape::Locator => hg.show_locators,
        _ => true,
    }
}

/// Visible children, lifting the visible descendants of a hidden node (like the JS `kidsOf`).
fn visible_children(s: &Scene, hg: &HgState, id: Option<&str>, depth: usize) -> Vec<String> {
    if depth > 256 {
        return vec![];
    }
    let mut out = vec![];
    for c in s.children(id) {
        if visible_kind(s, hg, c) {
            out.push(c.clone());
        } else {
            out.extend(visible_children(s, hg, Some(c), depth + 1));
        }
    }
    out
}

/// Top-down tidy tree: roots side by side, children centred under their parent.
fn layout_tree(s: &Scene, hg: &HgState) -> Vec<Box_> {
    let kids = |id: Option<&str>| -> Vec<String> { visible_children(s, hg, id, 0) };
    fn width(id: &str, kids: &dyn Fn(Option<&str>) -> Vec<String>, memo: &mut HashMap<String, f32>, depth: usize) -> f32 {
        let ks = if depth > 256 { vec![] } else { kids(Some(id)) };
        let sum: f32 = ks.iter().enumerate().map(|(i, k)| width(k, kids, memo, depth + 1) + if i > 0 { GX } else { 0.0 }).sum();
        let w = NW.max(sum);
        memo.insert(id.to_string(), w);
        w
    }
    fn place(s: &Scene, hg: &HgState, id: &str, left: f32, depth: usize, kids: &dyn Fn(Option<&str>) -> Vec<String>, memo: &HashMap<String, f32>, out: &mut Vec<Box_>) {
        let w = memo[id];
        let sub = if hg.show_shapes {
            s.node(id).and_then(|n| n.shape_name.clone()).unwrap_or_else(|| kind_label(s, id))
        } else {
            kind_label(s, id)
        };
        out.push(Box_ { key: id.to_string(), obj: Some(id.to_string()), label: s.name_of(id), sub, at: pos2(left + (w - NW) / 2.0, depth as f32 * (NH + GY)), material: false, node: Some(NodeRef::Dag(id.to_string())) });
        if depth > 256 {
            return;
        }
        let ks = kids(Some(id));
        let sum: f32 = ks.iter().enumerate().map(|(i, k)| memo[k] + if i > 0 { GX } else { 0.0 }).sum();
        let mut x = left + (w - sum) / 2.0;
        for k in ks {
            place(s, hg, &k, x, depth + 1, kids, memo, out);
            x += memo[&k] + GX;
        }
    }
    let mut memo = HashMap::new();
    let mut out = vec![];
    let mut x = 0.0;
    for (i, r) in kids(None).iter().enumerate() {
        if i > 0 {
            x += GX * 2.0;
        }
        let w = width(r, &kids, &mut memo, 0);
        place(s, hg, r, x, 0, &kids, &memo, &mut out);
        x += w;
    }
    out
}

/// One row per graphed object: transform, history (oldest first), shape; shared materials once.
fn layout_connections(s: &Scene, io: &[String]) -> (Vec<Box_>, Vec<(String, String)>) {
    let mut objs: Vec<String> = io.iter().filter(|i| s.node(i).is_some()).cloned().collect();
    // objects wired to the graphed ones by direct connections get rows too
    for c in &s.conns {
        let owner = |id: &str| s.ref_by_id(id).map(|r| match r {
            NodeRef::Dag(i) | NodeRef::History(i, _) => i,
            NodeRef::Material(i) | NodeRef::Texture(i) => i,
        });
        if let (Some(a), Some(b)) = (owner(&c.src), owner(&c.dst)) {
            if s.node(&a).is_some() && s.node(&b).is_some() {
                if objs.contains(&a) && !objs.contains(&b) {
                    objs.push(b);
                } else if objs.contains(&b) && !objs.contains(&a) {
                    objs.push(a);
                }
            }
        }
    }
    let (col, rowh) = (NW + 50.0, NH + 40.0);
    let max_hist = objs.iter().map(|o| s.node(o).and_then(|n| n.mesh()).map(|m| m.history.len()).unwrap_or(0)).max().unwrap_or(0);
    let shape_x = (max_hist + 1) as f32 * col;
    let mut boxes = vec![];
    let mut links = vec![];
    let mut mat_users: Vec<(String, Vec<f32>, Vec<String>)> = vec![];
    for (r, o) in objs.iter().enumerate() {
        let y = r as f32 * rowh;
        let n = s.node(o).unwrap();
        boxes.push(Box_ { key: format!("xf:{o}"), obj: Some(o.clone()), label: n.name.clone(), sub: "transform".into(), at: pos2(0.0, y), material: false, node: Some(NodeRef::Dag(o.clone())) });
        let hist = n.mesh().map(|m| m.history.clone()).unwrap_or_default();
        let mut prev: Option<String> = None;
        for (i, h) in hist.iter().enumerate() {
            let k = h.id.clone();
            boxes.push(Box_ { key: k.clone(), obj: Some(o.clone()), label: h.name.clone(), sub: h.ty.clone(), at: pos2((max_hist - hist.len() + i + 1) as f32 * col, y), material: false, node: Some(NodeRef::History(o.clone(), i)) });
            if let Some(p) = prev {
                links.push((p, k.clone()));
            }
            prev = Some(k);
        }
        let sk = format!("sh:{o}");
        boxes.push(Box_ { key: sk.clone(), obj: Some(o.clone()), label: n.shape_name.clone().unwrap_or(format!("{}Shape", n.name)), sub: format!("{} shape", kind_label(s, o)), at: pos2(shape_x, y), material: false, node: Some(NodeRef::Dag(o.clone())) });
        if let Some(p) = prev {
            links.push((p, sk.clone()));
        }
        if let Some(m) = n.mesh() {
            match mat_users.iter_mut().find(|u| u.0 == m.material) {
                Some(u) => {
                    u.1.push(y);
                    u.2.push(sk);
                }
                None => mat_users.push((m.material.clone(), vec![y], vec![sk])),
            }
        }
    }
    let mut min_y = f32::NEG_INFINITY;
    for (mid, ys, shapes) in mat_users {
        let Some(m) = s.mats.get(&mid) else { continue };
        let y = (ys.iter().sum::<f32>() / ys.len() as f32).max(min_y);
        min_y = y + NH + 20.0;
        boxes.push(Box_ { key: mid.clone(), obj: None, label: m.name.clone(), sub: m.ty.clone(), at: pos2(shape_x + col, y), material: true, node: Some(NodeRef::Material(mid.clone())) });
        for s in shapes {
            links.push((s, mid.clone()));
        }
    }
    (boxes, links)
}

/// Vector (triple) attributes: translate / rotate / scale / colours and the like (not their X/Y/Z parts).
fn is_triple(attr: &str) -> bool {
    matches!(attr, "translate" | "rotate" | "scale" | "shear" | "color" | "outColor" | "rotatePivot" | "scalePivot" | "jointOrient" | "position" | "normal" | "outColorRGB" | "incandescence" | "transparency" | "ambientColor" | "specularColor" | "baseColor" | "emissionColor" | "worldPosition" | "boundingBoxCenter")
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Shorten a node label to fit its box, like the JS `text-overflow: ellipsis`.
fn fit_label(painter: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> String {
    let width = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut tmp = out.clone();
        tmp.push(ch);
        tmp.push('…');
        if width(&tmp) > max_w {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        "…".into()
    } else {
        format!("{out}…")
    }
}

fn arrow(p: &egui::Painter, a: Pos2, b: Pos2, col: Color32) {
    let mx = (a.x + b.x) / 2.0;
    let pts: Vec<Pos2> = (0..=16).map(|i| {
        let t = i as f32 / 16.0;
        let u = 1.0 - t;
        let (c1, c2) = (pos2(mx, a.y), pos2(mx, b.y));
        pos2(u * u * u * a.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * b.x, u * u * u * a.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * b.y)
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.6, col)));
    p.add(egui::Shape::convex_polygon(vec![b, b + vec2(-7.0, -4.0), b + vec2(-7.0, 4.0)], col, Stroke::NONE));
}

pub fn window(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.win.hypergraph;
    if !open {
        return;
    }
    let title = if app.hg.connections { "Hypergraph InputOutput" } else { "Hypergraph Hierarchy" };
    egui::Window::new(title).id(egui::Id::new("hypergraph")).open(&mut open).default_size([820.0, 500.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.selectable_label(!app.hg.connections, "Scene Hierarchy").on_hover_text("Show the scene DAG").clicked() && app.hg.connections {
                app.hg.connections = false;
                app.hg.active_key = None;
                app.hg.frame_all();
            }
            if ui.selectable_label(app.hg.connections, "Input and Output Connections").on_hover_text("Graph the selected objects").clicked() && !app.hg.connections {
                app.hg.connections = true;
                app.hg.io = app.s.sel.clone();
                app.hg.active_key = None;
                app.hg.frame_all();
            }
            ui.separator();
            ui.menu_button("Edit", |ui| {
                if ui.button("Select All").clicked() {
                    let ids: Vec<String> = app.s.all_dag(false).into_iter().filter(|id| app.s.is_visible(id)).collect();
                    app.select_names(&ids, SelMode::Replace);
                    ui.close();
                }
                if ui.button("Unparent").clicked() {
                    let mel: String = app.s.sel.iter().filter(|id| app.s.node(id).and_then(|n| n.parent.clone()).is_some()).map(|id| format!("parent -w {};\n", app.s.name_of(id))).collect();
                    if !mel.is_empty() {
                        let _ = app.run_mel("unparent", &mel);
                    }
                    ui.close();
                }
                if ui.button("Rename...").clicked() {
                    if let Some(l) = app.s.lead() {
                        let name = app.s.name_of(l);
                        app.hg.rename = Some((name.clone(), name));
                    }
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Frame All  (A)").clicked() {
                    app.hg.frame_all();
                    ui.close();
                }
                if ui.button("Frame Selection  (F)").clicked() {
                    app.hg.frame_selection();
                    ui.close();
                }
                if ui.button("Reset Zoom").clicked() {
                    app.hg.zoom = 1.0;
                    ui.close();
                }
            });
            ui.menu_button("Show", |ui| {
                ui.checkbox(&mut app.hg.show_cameras, "Cameras");
                ui.checkbox(&mut app.hg.show_lights, "Lights");
                ui.checkbox(&mut app.hg.show_curves, "Curves");
                ui.checkbox(&mut app.hg.show_locators, "Locators");
                ui.separator();
                ui.checkbox(&mut app.hg.show_shapes, "Shape Names");
            });
            ui.menu_button("Graph", |ui| {
                if ui.button("Scene Hierarchy").clicked() {
                    app.hg.connections = false;
                    app.hg.active_key = None;
                    app.hg.frame_all();
                    ui.close();
                }
                if ui.button("Input and Output Connections").clicked() {
                    app.hg.connections = true;
                    app.hg.io = app.s.sel.clone();
                    app.hg.active_key = None;
                    app.hg.frame_all();
                    ui.close();
                }
                if ui.button("Rebuild").clicked() {
                    app.hg.frame_all();
                    ui.close();
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Frame All").on_hover_text("Fit the whole graph in the view (A)").clicked() {
                app.hg.frame_all();
            }
            if ui.button("Frame Selection").on_hover_text("Fit the selected objects in the view (F)").clicked() {
                app.hg.frame_selection();
            }
            if ui.button("Reset Zoom").clicked() {
                app.hg.zoom = 1.0;
            }
            ui.separator();
            ui.checkbox(&mut app.hg.show_shapes, "Shape Names").on_hover_text("Show the shape name under each transform");
            ui.checkbox(&mut app.hg.show_cameras, "Cameras");
            ui.label(RichText::new(format!("{}%", (app.hg.zoom * 100.0).round() as i32)).color(theme::TEXT_DIM).size(10.5));
        });
        graph(app, ui);
    });
    // Rename dialog for the Hypergraph (renames the box's object directly, like the JS
    // double-click rename; the generic Rename window acts on the current selection).
    if let Some((old, mut new)) = app.hg.rename.take() {
        let mut keep = true;
        let mut commit = false;
        egui::Window::new("Rename").id(egui::Id::new("hg_rename")).collapsible(false).resizable(false).show(ctx, |ui| {
            let r = ui.text_edit_singleline(&mut new);
            r.request_focus();
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    commit = true;
                }
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
        });
        if commit {
            if !new.trim().is_empty() && new.trim() != old {
                let _ = app.run_mel("rename", &format!("rename \"{old}\" \"{}\";", new.trim().replace('"', "")));
            }
            keep = false;
        }
        if keep {
            app.hg.rename = Some((old, new));
        }
    }
    app.win.hypergraph = open;
}

/// Fit the graph (or just the selected objects) into the visible graph rectangle.
fn fit_graph(sel: &[String], hg: &mut HgState, rect: Rect, boxes: &[Box_], only_selection: bool) {
    let mut bounds: Option<Rect> = None;
    for b in boxes {
        if only_selection && !b.obj.as_ref().is_some_and(|o| sel.contains(o)) {
            continue;
        }
        let r = Rect::from_min_size(b.at, vec2(NW, NH));
        bounds = Some(match bounds {
            Some(u) => u.union(r),
            None => r,
        });
    }
    // an empty selection falls back to the whole graph, like the JS `frame(true)`
    let bounds = bounds.or_else(|| boxes.iter().map(|b| Rect::from_min_size(b.at, vec2(NW, NH))).reduce(|a, b| a.union(b)));
    let Some(b) = bounds else { return };
    let avail = rect.shrink(34.0);
    let bw = b.width().max(1.0);
    let bh = b.height().max(1.0);
    let z = (avail.width().max(1.0) / bw).min(avail.height().max(1.0) / bh).clamp(0.15, if only_selection { 1.5 } else { 1.2 });
    hg.zoom = z;
    hg.pan = rect.size() * 0.5 - b.center().to_vec2() * z;
}

fn graph(app: &mut IncaApp, ui: &mut egui::Ui) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size() - vec2(0.0, 18.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0x6b, 0x6b, 0x6b));
    let (boxes, links) = if app.hg.connections {
        if app.hg.io.is_empty() && !app.s.sel.is_empty() {
            app.hg.io = app.s.sel.clone();
        }
        layout_connections(&app.s, &app.hg.io.clone())
    } else {
        (layout_tree(&app.s, &app.hg), vec![])
    };
    if let Some(fit) = app.hg.fit.take() {
        fit_graph(&app.s.sel, &mut app.hg, rect, &boxes, fit == Fit::Selection);
    }
    if boxes.is_empty() {
        let msg = if app.hg.connections { "Select objects, then Graph > Input and Output Connections" } else { "The scene is empty" };
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(13.0), Color32::from_gray(0xd0));
    }
    let (pan, zoom) = (app.hg.pan, app.hg.zoom);
    let to_scr = |p: Pos2| rect.min + pan + p.to_vec2() * zoom;
    let rect_of = |b: &Box_| Rect::from_min_size(to_scr(b.at), vec2(NW, NH) * zoom);
    let idx: HashMap<&str, usize> = boxes.iter().enumerate().map(|(i, b)| (b.key.as_str(), i)).collect();
    // edges: parent → child, or the connection rows
    if app.hg.connections {
        for (a, b) in &links {
            if let (Some(&i), Some(&j)) = (idx.get(a.as_str()), idx.get(b.as_str())) {
                let (ra, rb) = (rect_of(&boxes[i]), rect_of(&boxes[j]));
                arrow(&painter, pos2(ra.max.x, ra.center().y), pos2(rb.min.x - 1.0, rb.center().y), Color32::from_rgb(0xd0, 0x40, 0xd0));
            }
        }
        // direct connections: one line per connection; several between the same two nodes are
        // stacked in parallel; green for vector (triple) attributes, blue for single ones
        let mut pairs: Vec<((usize, usize), Vec<&inca_core::scene::Conn>)> = vec![];
        for c in &app.s.conns {
            let find = |id: &str| boxes.iter().position(|b| b.key == id || b.key == format!("xf:{id}"));
            if let (Some(i), Some(j)) = (find(&c.src), find(&c.dst)) {
                if i == j {
                    continue;
                }
                match pairs.iter_mut().find(|p| p.0 == (i, j)) {
                    Some(p) => p.1.push(c),
                    None => pairs.push(((i, j), vec![c])),
                }
            }
        }
        let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
        let mut tip: Option<String> = None;
        for ((i, j), list) in pairs {
            let (ra, rb) = (rect_of(&boxes[i]), rect_of(&boxes[j]));
            let n = list.len();
            let gap = if n > 1 { (9.0f32).min((NH - 12.0) / (n - 1) as f32) * zoom } else { 0.0 };
            let same = (ra.min.x - rb.min.x).abs() < NW * zoom;
            for (k, c) in list.iter().enumerate() {
                let dy = (k as f32 - (n - 1) as f32 / 2.0) * gap;
                let col = if is_triple(&c.src_attr) { Color32::from_rgb(0x3c, 0xcf, 0x3c) } else { Color32::from_rgb(0x28, 0x48, 0xd8) };
                let (a, b) = if same { (pos2(ra.min.x, ra.center().y + dy), pos2(rb.min.x - 1.0, rb.center().y + dy)) } else { (pos2(ra.max.x, ra.center().y + dy), pos2(rb.min.x - 1.0, rb.center().y + dy)) };
                let pts: Vec<Pos2> = if same {
                    // same column: loop out to the left of both nodes
                    let lx = ra.min.x.min(rb.min.x) - 45.0 * zoom;
                    (0..=16).map(|q| {
                        let t = q as f32 / 16.0;
                        let u = 1.0 - t;
                        pos2(u * u * u * a.x + 3.0 * u * t * lx + t * t * t * b.x, u * u * u * a.y + 3.0 * u * u * t * a.y + 3.0 * u * t * t * b.y + t * t * t * b.y)
                    }).collect()
                } else {
                    let mx = (a.x + b.x) / 2.0;
                    (0..=16).map(|q| {
                        let t = q as f32 / 16.0;
                        let u = 1.0 - t;
                        pos2(u * u * u * a.x + 3.0 * u * u * t * mx + 3.0 * u * t * t * mx + t * t * t * b.x, u * u * u * a.y + 3.0 * u * u * t * a.y + 3.0 * u * t * t * b.y + t * t * t * b.y)
                    }).collect()
                };
                if let Some(q) = pointer {
                    if pts.windows(2).any(|w| seg_dist(q, w[0], w[1]) < 4.0) {
                        let name = |id: &str| app.s.ref_by_id(id).map(|r| app.s.ref_name(&r)).unwrap_or_default();
                        tip = Some(format!("{}.{} → {}.{}", name(&c.src), c.src_attr, name(&c.dst), c.dst_attr));
                    }
                }
                painter.add(egui::Shape::line(pts, Stroke::new(1.6, col)));
                painter.add(egui::Shape::convex_polygon(vec![b, b + vec2(-7.0, -4.0), b + vec2(-7.0, 4.0)], col, Stroke::NONE));
            }
        }
        if let (Some(t), Some(q)) = (tip, pointer) {
            let g = painter.layout_no_wrap(t, egui::FontId::proportional(11.0), Color32::WHITE);
            let r = Rect::from_min_size(q + vec2(12.0, 12.0), g.size() + vec2(8.0, 4.0));
            painter.rect_filled(r, 3.0, Color32::from_black_alpha(200));
            painter.galley(r.min + vec2(4.0, 2.0), g, Color32::WHITE);
        }
    } else {
        for b in &boxes {
            let Some(p) = b.obj.as_ref().and_then(|o| app.s.node(o)).and_then(|n| n.parent.clone()) else { continue };
            if let Some(&pi) = idx.get(p.as_str()) {
                let (ra, rb) = (rect_of(&boxes[pi]), rect_of(b));
                painter.line_segment([pos2(ra.center().x, ra.max.y), pos2(rb.center().x, rb.min.y)], Stroke::new(1.2, Color32::from_gray(40)));
            }
        }
    }
    // nodes
    let font = egui::FontId::proportional(11.0 * zoom.clamp(0.6, 1.4));
    let small = egui::FontId::proportional(9.5 * zoom.clamp(0.6, 1.4));
    let lead = app.s.lead().cloned();
    let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
    let mut hovered: Option<usize> = None;
    for (i, b) in boxes.iter().enumerate() {
        let r = rect_of(b);
        if !rect.intersects(r) {
            continue;
        }
        let active = app.hg.connections && app.hg.active_key.as_deref() == Some(b.key.as_str());
        let sel = active || (b.obj.as_ref().is_some_and(|o| app.s.sel.contains(o)) && (!app.hg.connections || b.key.starts_with("xf:")));
        let is_lead = sel && b.obj.as_ref() == lead.as_ref();
        let hidden = b.obj.as_ref().is_some_and(|o| !app.s.is_visible(o));
        let fill = if is_lead { Color32::from_rgb(0xc8, 0xc8, 0x00) } else if sel { Color32::from_rgb(0xb3, 0xb3, 0x00) } else if b.material { Color32::from_rgb(0x4e, 0x5e, 0x70) } else { Color32::from_gray(0x5a) };
        painter.rect_filled(r, 3.0, if hidden { fill.gamma_multiply(0.55) } else { fill });
        painter.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::from_gray(0x2a)), egui::StrokeKind::Inside);
        let dragging = app.hg.drag.as_ref().is_some_and(|d| b.obj.as_ref() != Some(d)) || app.hg.conn_drag.is_some_and(|src| src != i);
        if dragging && pointer.is_some_and(|q| r.contains(q)) {
            painter.rect_stroke(r, 3.0, Stroke::new(2.0, Color32::from_rgb(0xe6, 0xc5, 0x4a)), egui::StrokeKind::Outside);
        } else if pointer.is_some_and(|q| r.contains(q)) {
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::from_rgb(0xe6, 0xc5, 0x4a)), egui::StrokeKind::Outside);
        }
        let tc = if sel { Color32::from_gray(0x11) } else { Color32::from_gray(0xe8) };
        let max_text = (NW * zoom - 10.0).max(10.0);
        let label = fit_label(&painter, &b.label, &font, max_text);
        let sub = fit_label(&painter, &b.sub, &small, max_text);
        painter.text(r.center() - vec2(0.0, 6.0 * zoom), egui::Align2::CENTER_CENTER, &label, font.clone(), tc);
        painter.text(r.center() + vec2(0.0, 9.0 * zoom), egui::Align2::CENTER_CENTER, &sub, small.clone(), if sel { Color32::from_gray(0x33) } else { Color32::from_gray(0xbb) });
        if pointer.is_some_and(|q| r.contains(q)) {
            hovered = Some(i);
        }
    }
    // interaction: click selects; MMB-drag a node onto another parents it (hierarchy) or opens
    // the Connection Editor (input/output); primary-drag on empty space marquee-selects;
    // MMB or Alt-drag pans; wheel zooms.
    let (shift, alt) = ui.ctx().input(|i| (i.modifiers.shift, i.modifiers.alt));
    let press = ui.ctx().input(|i| i.pointer.press_origin());
    if resp.drag_started_by(egui::PointerButton::Middle) {
        if let Some(p) = press {
            if let Some(i) = boxes.iter().rposition(|b| rect_of(b).contains(p)) {
                if app.hg.connections {
                    app.hg.conn_drag = Some(i);
                } else {
                    app.hg.drag = boxes[i].obj.clone();
                }
            }
        }
    }
    if (resp.dragged_by(egui::PointerButton::Middle) || (alt && resp.dragged_by(egui::PointerButton::Primary))) && app.hg.drag.is_none() && app.hg.conn_drag.is_none() {
        app.hg.pan += resp.drag_delta();
    }
    if resp.drag_started_by(egui::PointerButton::Primary) && !alt {
        if let Some(p) = press {
            if !boxes.iter().any(|b| rect_of(b).contains(p)) {
                app.hg.marquee = Some(p);
            }
        }
    }
    if let Some(start) = app.hg.marquee {
        if resp.dragged_by(egui::PointerButton::Primary) {
            let q = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, q);
            painter.rect_filled(r, 0.0, Color32::from_white_alpha(20));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
        }
    }
    if let (Some(d), Some(q)) = (app.hg.drag.clone(), pointer) {
        painter.text(q + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, app.s.name_of(&d), font.clone(), Color32::WHITE);
    }
    if let (Some(i), Some(q)) = (app.hg.conn_drag, pointer) {
        painter.text(q + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, &boxes[i].label, font.clone(), Color32::WHITE);
    }
    if resp.drag_stopped_by(egui::PointerButton::Middle) {
        if let Some(i) = app.hg.conn_drag.take() {
            let over_self = hovered == Some(i);
            if !over_self {
                if let (Some(src), Some(tgt)) = (boxes[i].node.clone(), hovered.and_then(|j| boxes[j].node.clone())) {
                    app.hg.ce_left = Some(src);
                    app.hg.ce_right = Some(tgt);
                    app.win.connection_editor = true;
                }
            }
        } else if let Some(d) = app.hg.drag.take() {
            let over_self = hovered.and_then(|i| boxes[i].obj.as_ref()) == Some(&d);
            if !over_self {
                let target = hovered.and_then(|i| boxes[i].obj.clone()).filter(|t| *t != d);
                let mel = match target {
                    Some(t) => format!("parent {} {};", app.s.name_of(&d), app.s.name_of(&t)),
                    None if app.s.node(&d).and_then(|n| n.parent.clone()).is_some() => format!("parent -w {};", app.s.name_of(&d)),
                    None => String::new(),
                };
                if !mel.is_empty() {
                    let _ = app.run_mel("parent", &mel);
                }
            }
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        if let Some(start) = app.hg.marquee.take() {
            let q = pointer.unwrap_or(start);
            let sel = Rect::from_two_pos(start, q);
            if sel.width() > 3.0 || sel.height() > 3.0 {
                let a = (sel.min - rect.min - pan) / zoom;
                let b = (sel.max - rect.min - pan) / zoom;
                let world = Rect::from_two_pos(pos2(a.x, a.y), pos2(b.x, b.y));
                let mut ids: Vec<String> = vec![];
                for b in &boxes {
                    if let Some(o) = &b.obj {
                        if world.intersects(Rect::from_min_size(b.at, vec2(NW, NH))) && !ids.contains(o) {
                            ids.push(o.clone());
                        }
                    }
                }
                app.select_names(&ids, if shift { SelMode::Toggle } else { SelMode::Replace });
            }
        }
    }
    if resp.clicked() {
        match hovered.and_then(|i| boxes[i].obj.clone().map(|o| (i, o))) {
            Some((i, o)) => {
                if app.hg.connections {
                    app.hg.active_key = Some(boxes[i].key.clone());
                }
                app.select_names(std::slice::from_ref(&o), if shift { SelMode::Toggle } else { SelMode::Replace });
                if app.hg.connections && !boxes[i].key.starts_with("xf:") {
                    // history / shape nodes: show them in the Attribute Editor
                    app.show_right = true;
                    app.right_tab = crate::app::RightTab::AttributeEditor;
                }
            }
            None => {
                if let Some(i) = hovered.filter(|i| boxes[*i].material) {
                    app.hg.active_key = Some(boxes[i].key.clone());
                    app.windows_state.hs_selected = Some(boxes[i].key.clone());
                    app.win.hypershade = true;
                } else if !shift {
                    app.select_names(&[], SelMode::Replace);
                }
            }
        }
    }
    if resp.double_clicked() {
        if let Some(i) = hovered {
            let rename_transforms = !app.hg.connections || boxes[i].key.starts_with("xf:");
            if rename_transforms {
                if let Some(NodeRef::Dag(id)) = &boxes[i].node {
                    let name = app.s.name_of(id);
                    app.select_names(std::slice::from_ref(id), SelMode::Replace);
                    app.hg.rename = Some((name.clone(), name));
                }
            }
        }
    }
    if resp.hovered() {
        let (frame_all, frame_sel) = ui.ctx().input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::A), i.consume_key(egui::Modifiers::NONE, egui::Key::F)));
        if frame_all {
            app.hg.frame_all();
        }
        if frame_sel {
            app.hg.frame_selection();
        }
    }
    if let Some(q) = pointer.filter(|q| rect.contains(*q)) {
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let nz = (zoom * (1.0 + scroll * 0.002)).clamp(0.2, 3.0);
            let g = (q - rect.min - pan) / zoom;
            app.hg.zoom = nz;
            app.hg.pan = q - rect.min - g * nz;
        }
    }
    let hint = if app.hg.connections {
        "click a node to select it · MMB-drag node → node to connect · drag empty space to marquee · MMB/Alt-drag to pan · wheel zoom"
    } else {
        "MMB-drag a node onto another to parent (empty space: unparent) · drag empty space to marquee · MMB/Alt-drag to pan · wheel zoom"
    };
    let hovered_name = hovered.map(|i| boxes[i].label.clone()).filter(|s| !s.is_empty());
    let status = match hovered_name {
        Some(n) => format!("{} selected   |   {n}   |   {hint}", app.s.sel.len()),
        None => format!("{} selected   |   {hint}", app.s.sel.len()),
    };
    ui.label(RichText::new(status).color(theme::TEXT_DIM).size(10.5));
}

// ---------------------------------------------------------------- Connection Editor

/// Attributes listed for a node.
fn attrs_of(app: &IncaApp, r: &NodeRef) -> Vec<String> {
    match r {
        NodeRef::Dag(id) => {
            let mut v: Vec<String> = inca_core::scene::CHANNELS.iter().map(|c| c.to_string()).collect();
            if let Some(e) = app.s.node(id).and_then(|n| n.extra.as_object()) {
                v.extend(e.iter().filter(|(_, x)| x.is_number() || x.is_boolean()).map(|(k, _)| k.clone()));
            }
            v.extend(app.s.shape_channels(id).into_iter().filter(|k| !v.contains(k)).collect::<Vec<_>>());
            v
        }
        NodeRef::History(id, i) => app.s.node(id).and_then(|n| n.mesh()).and_then(|m| m.history.get(*i)).map(|h| h.params.iter().filter(|(_, x)| x.is_number()).map(|(k, _)| k.clone()).collect()).unwrap_or_default(),
        NodeRef::Material(id) => app.s.mats.get(id).map(|m| m.attrs.keys().cloned().collect()).unwrap_or_default(),
        NodeRef::Texture(id) => {
            let mut v = vec!["outColor".to_string()];
            v.extend(app.s.texs.get(id).map(|t| t.attrs.iter().filter(|(_, x)| x.is_number()).map(|(k, _)| k.clone()).collect::<Vec<_>>()).unwrap_or_default());
            v
        }
    }
}

/// Load the selection: first object on the left (outputs), last on the right (inputs).
pub fn load_connection_editor(app: &mut IncaApp) {
    if let Some(f) = app.s.sel.first() {
        app.hg.ce_left = Some(NodeRef::Dag(f.clone()));
    }
    if let Some(l) = app.s.sel.last() {
        app.hg.ce_right = Some(NodeRef::Dag(l.clone()));
    }
}

pub fn connection_editor(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.win.connection_editor;
    if !open {
        return;
    }
    egui::Window::new("Connection Editor").open(&mut open).default_size([520.0, 440.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Reload Left").clicked() {
                app.hg.ce_left = app.s.lead().map(|l| NodeRef::Dag(l.clone()));
                app.hg.ce_attr = None;
            }
            if ui.button("Reload Right").clicked() {
                app.hg.ce_right = app.s.lead().map(|l| NodeRef::Dag(l.clone()));
            }
            if ui.button("Swap").clicked() {
                std::mem::swap(&mut app.hg.ce_left, &mut app.hg.ce_right);
                app.hg.ce_attr = None;
            }
        });
        ui.label(RichText::new("Click an output on the left, then an input on the right to connect (click a connected input to break it).").color(theme::TEXT_DIM).size(10.5));
        ui.separator();
        let (l, r) = (app.hg.ce_left.clone(), app.hg.ce_right.clone());
        let (lid, rid) = (l.as_ref().map(|x| app.s.ref_id(x)), r.as_ref().map(|x| app.s.ref_id(x)));
        let mut connect: Option<(String, String)> = None;
        let mut disconnect: Option<(String, String, String)> = None;
        ui.columns(2, |cols| {
            cols[0].label(RichText::new(format!("Outputs: {}", l.as_ref().map(|x| app.s.ref_name(x)).unwrap_or("—".into()))).strong());
            if let Some(l) = &l {
                egui::ScrollArea::vertical().id_salt("ceL").show(&mut cols[0], |ui| {
                    for a in attrs_of(app, l) {
                        let wired = app.s.conns.iter().any(|c| Some(&c.src) == lid.as_ref() && c.src_attr == a);
                        let txt = if wired { RichText::new(&a).italics().color(Color32::from_rgb(140, 200, 255)) } else { RichText::new(&a) };
                        if ui.selectable_label(app.hg.ce_attr.as_deref() == Some(a.as_str()), txt).clicked() {
                            app.hg.ce_attr = Some(a.clone());
                        }
                    }
                });
            }
            cols[1].label(RichText::new(format!("Inputs: {}", r.as_ref().map(|x| app.s.ref_name(x)).unwrap_or("—".into()))).strong());
            if let Some(r) = &r {
                egui::ScrollArea::vertical().id_salt("ceR").show(&mut cols[1], |ui| {
                    for a in attrs_of(app, r) {
                        let from = app.s.conns.iter().find(|c| Some(&c.dst) == rid.as_ref() && c.dst_attr == a).map(|c| (c.src.clone(), c.src_attr.clone()));
                        let txt = match &from {
                            Some(_) => RichText::new(&a).italics().color(Color32::from_rgb(140, 200, 255)),
                            None => RichText::new(&a),
                        };
                        let resp = ui.selectable_label(false, txt);
                        let resp = match &from {
                            Some((s, sa)) => resp.on_hover_text(format!("← {}.{sa}", app.s.ref_by_id(s).map(|x| app.s.ref_name(&x)).unwrap_or_default())),
                            None => resp,
                        };
                        if resp.clicked() {
                            match (&from, &app.hg.ce_attr) {
                                (Some((s, sa)), _) => disconnect = Some((s.clone(), sa.clone(), a.clone())),
                                (None, Some(la)) => connect = Some((la.clone(), a.clone())),
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
        if let (Some((la, ra)), Some(l), Some(r)) = (connect, &l, &r) {
            let _ = app.run_mel("connect", &format!("connectAttr {}.{la} {}.{ra};", app.s.ref_name(l), app.s.ref_name(r)));
        }
        if let (Some((s, sa, ra)), Some(r)) = (disconnect, &r) {
            let sn = app.s.ref_by_id(&s).map(|x| app.s.ref_name(&x)).unwrap_or_default();
            let _ = app.run_mel("disconnect", &format!("disconnectAttr {sn}.{sa} {}.{ra};", app.s.ref_name(r)));
        }
    });
    app.win.connection_editor = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    use inca_core::params::Params;

    fn labels(boxes: &[Box_]) -> Vec<String> {
        boxes.iter().map(|b| b.label.clone()).collect()
    }

    #[test]
    fn hierarchy_filters_startup_cameras_and_lights() {
        let mut s = Scene::new();
        let mut hg = HgState::default();
        assert!(!labels(&layout_tree(&s, &hg)).contains(&"persp".to_string()), "startup cameras are hidden by default");
        hg.show_cameras = true;
        assert!(labels(&layout_tree(&s, &hg)).contains(&"persp".to_string()));
        let l = s.create_light("pointLight");
        let name = s.name_of(&l);
        assert!(labels(&layout_tree(&s, &hg)).contains(&name), "lights are shown by default");
        hg.show_lights = false;
        assert!(!labels(&layout_tree(&s, &hg)).contains(&name));
    }

    #[test]
    fn hidden_kinds_lift_their_visible_children() {
        let mut s = Scene::new();
        let g = s.create_group("g", None);
        let l = s.create_light("pointLight");
        let m = s.create_mesh("polyCube", &Params::new(), Some("child"));
        assert!(s.reparent(&l, Some(&g), None));
        assert!(s.reparent(&m, Some(&l), None));
        let mut hg = HgState::default();
        hg.show_lights = false;
        let boxes = layout_tree(&s, &hg);
        let names = labels(&boxes);
        assert!(names.contains(&"g".into()));
        assert!(names.contains(&"child".into()), "the mesh is lifted out of the hidden light");
        assert!(!names.iter().any(|n| n.contains("pointLight")));
        assert_eq!(boxes.iter().find(|b| b.label == "child").unwrap().at.y, NH + GY);
    }

    #[test]
    fn fit_graph_centres_and_scales_the_bounds() {
        let boxes = vec![
            Box_ { key: "a".into(), obj: Some("a".into()), label: "a".into(), sub: String::new(), at: pos2(0.0, 0.0), material: false, node: None },
            Box_ { key: "b".into(), obj: Some("b".into()), label: "b".into(), sub: String::new(), at: pos2(1000.0, 400.0), material: false, node: None },
        ];
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0));
        let mut hg = HgState::default();
        fit_graph(&[], &mut hg, rect, &boxes, false);
        assert!(hg.zoom > 0.15 && hg.zoom <= 1.2, "zoom {}", hg.zoom);
        for b in &boxes {
            let r = Rect::from_min_size(rect.min + hg.pan + b.at.to_vec2() * hg.zoom, vec2(NW, NH) * hg.zoom);
            assert!(rect.expand(0.1).contains(r.min) && rect.expand(0.1).contains(r.max), "{r:?} is not inside {rect:?}");
        }
        // framing the selection ignores the unselected box
        let mut hg = HgState::default();
        fit_graph(&["b".into()], &mut hg, rect, &boxes, true);
        assert!(hg.zoom > 1.0, "a single box can use the selection zoom {}", hg.zoom);
    }
}
