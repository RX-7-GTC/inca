//! Hypergraph: the DAG as a top-down tree, or the selected objects'
//! input and output connections (transform → history → shape → material); and the Connection
//! Editor for direct attribute connections.

use crate::app::IncaApp;
use crate::theme;
use egui::{pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use inca_core::scene::{NodeRef, Scene, SelMode, Shape};
use std::collections::{HashMap, HashSet};

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
    /// selected output attribute (the role follows `ce_dir`)
    ce_out: Option<String>,
    /// selected input attributes (Maya allows several for Make/Break)
    ce_in: Vec<String>,
    /// left side shows outputs (true) or inputs (false)
    ce_dir: bool,
    /// expanded compound attributes per side
    ce_open: [HashSet<String>; 2],
    /// per-side display filters
    ce_filter: [CeFilter; 2],
    /// Maya Options > Channel Names
    ce_names: CeName,
    /// Maya Options > Attribute Order
    ce_order: CeOrder,
    /// Maya Options > Auto-connect, on by default
    pub ce_auto: bool,
    /// raise the Connection Editor window on the next frame
    pub ce_raise: bool,
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
            ce_out: None,
            ce_in: Vec::new(),
            ce_dir: true,
            ce_open: std::array::from_fn(|_| HashSet::new()),
            ce_filter: std::array::from_fn(|_| CeFilter::default()),
            ce_names: CeName::Nice,
            ce_order: CeOrder::Default,
            ce_auto: true,
            ce_raise: false,
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
    if n.kind == "container" {
        return "container".into();
    }
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
            NodeRef::Material(i) | NodeRef::Texture(i) | NodeRef::Place2D(i) => i,
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
    egui::Window::new(title).id(egui::Id::new("hypergraph")).open(&mut open).default_size([820.0, 500.0]).constrain(false).show(ctx, |ui| {
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
        match crate::windows::rename_prompt(ctx, egui::Id::new("hg_rename"), "Rename", &mut new, "Rename") {
            Some(crate::windows::RenameAction::Commit(new)) => {
                if !new.is_empty() && new != old {
                    let _ = app.run_mel("rename", &format!("rename \"{old}\" \"{}\";", new.replace('"', "")));
                }
            }
            Some(crate::windows::RenameAction::Cancel) => {}
            None => app.hg.rename = Some((old, new)),
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
                    app.hg.ce_raise = true;
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
                app.s.checkpoint_selection("select");
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
                app.s.checkpoint_selection("select");
                app.select_names(std::slice::from_ref(&o), if shift { SelMode::Toggle } else { SelMode::Replace });
                if app.hg.connections && !boxes[i].key.starts_with("xf:") {
                    // history / shape nodes: show them in the Attribute Editor
                    app.show_right = true;
                    app.right_tab = crate::app::RightTab::AttributeEditor;
                }
            }
            None => {
                if let Some(i) = hovered.filter(|i| boxes[*i].material) {
                    let mid = boxes[i].key.clone();
                    app.hg.active_key = Some(mid.clone());
                    crate::hypershade::select_shading_node(app, &mid);
                    app.win.hypershade = true;
                } else if !shift && !app.s.sel.is_empty() {
                    app.s.checkpoint_selection("select");
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CeType {
    Float,
    Int,
    Bool,
    Vector,
    TexColor,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CeMode {
    Readable,
    Outputs,
    Inputs,
    Published,
}

/// Maya Options > Channel Names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CeName {
    Nice,
    Long,
    Short,
}

/// Maya Options > Attribute Order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CeOrder {
    Default,
    Ascending,
    Descending,
}

#[derive(Clone)]
struct CeFilter {
    mode: CeMode,
    non_keyable: bool,
    connected_only: bool,
    hidden: bool,
}

impl Default for CeFilter {
    fn default() -> Self {
        CeFilter { mode: CeMode::Readable, non_keyable: true, connected_only: false, hidden: false }
    }
}

/// One row in a Connection Editor pane. Compounds carry their children (translate -> translateX...).
#[derive(Clone)]
struct CeAttr {
    name: String,
    label: String,
    short: String,
    ty: CeType,
    keyable: bool,
    readable: bool,
    writable: bool,
    mappable: bool,
    published: bool,
    children: Vec<CeAttr>,
    parent: Option<String>,
    hidden: bool,
}

/// Maya's "Nice" channel name: camel case split with acronyms kept together, digits detached.
fn ce_label(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_ascii_digit() {
            if prev.is_some_and(|p| p.is_alphabetic()) {
                out.push(' ');
            }
            out.push(c);
        } else if c.is_uppercase() {
            if prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) || (prev.is_some_and(|p| p.is_uppercase()) && next.is_some_and(|n| n.is_lowercase())) {
                out.push(' ');
            }
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

/// Maya's attribute short names for the attributes these nodes expose.
fn short_name(name: &str) -> String {
    if let Some(s) = component_short(name) {
        return s;
    }
    match name {
        "translate" => "t",
        "rotate" => "r",
        "scale" => "s",
        "visibility" => "v",
        "template" => "tmp",
        "rotatePivot" => "rp",
        "scalePivot" => "sp",
        "smoothLevel" => "lev",
        "base" => "b",
        "baseColor" => "bc",
        "diffuseRoughness" => "dr",
        "metalness" => "m",
        "specular" => "s",
        "specularColor" => "sc",
        "specularRoughness" => "sr",
        "specularIOR" => "sior",
        "transmission" => "t",
        "transmissionColor" => "trc",
        "coat" => "ct",
        "coatRoughness" => "ctr",
        "sheen" => "sh",
        "sheenColor" => "shc",
        "emission" => "e",
        "emissionColor" => "ec",
        "opacity" => "op",
        "thinWalled" => "tw",
        "color" => "c",
        "transparency" => "it",
        "ambientColor" => "ambc",
        "incandescence" => "ic",
        "diffuse" => "dc",
        "eccentricity" => "ec",
        "specularRollOff" => "sro",
        "reflectivity" => "rfl",
        "cosinePower" => "cp",
        "outColor" => "oc",
        "outAlpha" => "oa",
        "outTransparency" => "ot",
        "color1" => "c1",
        "color2" => "c2",
        "repeatU" => "ru",
        "repeatV" => "rv",
        "offsetU" => "ou",
        "offsetV" => "ov",
        "rotateUV" => "ro",
        "fileTextureName" => "ftn",
        "colorSpace" => "cs",
        "contrast" => "ct",
        "bump" => "b",
        "width" => "w",
        "height" => "h",
        "depth" => "d",
        "subdivisionsWidth" => "sw",
        "subdivisionsHeight" => "sh",
        "subdivisionsDepth" => "sd",
        "focalLength" => "fl",
        "intensity" => "i",
        "shadowRadius" => "sr",
        "shadowColor" => "sc",
        _ => return generic_short(name),
    }
    .to_string()
}

/// R/G/B and X/Y/Z children use their compound's short name plus the axis letter.
fn component_short(name: &str) -> Option<String> {
    let last = name.chars().last()?;
    if !matches!(last, 'R' | 'G' | 'B' | 'X' | 'Y' | 'Z') {
        return None;
    }
    let base = match &name[..name.len() - 1] {
        "translate" => "t",
        "rotate" => "r",
        "scale" => "s",
        "rotatePivot" => "rp",
        "scalePivot" => "sp",
        "baseColor" => "bc",
        "specularColor" => "sc",
        "emissionColor" => "ec",
        "transmissionColor" => "trc",
        "sheenColor" => "shc",
        "opacity" => "op",
        "color" => "c",
        "transparency" => "it",
        "ambientColor" => "ambc",
        "incandescence" => "ic",
        "color1" => "c1",
        "color2" => "c2",
        _ => return None,
    };
    Some(format!("{base}{}", last.to_ascii_lowercase()))
}

fn generic_short(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i == 0 || c.is_uppercase() || c.is_ascii_digit() {
            out.push(c.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        name.to_lowercase()
    } else {
        out
    }
}

impl CeAttr {
    fn new(name: impl Into<String>, ty: CeType) -> Self {
        let name = name.into();
        let label = ce_label(&name);
        let short = short_name(&name);
        CeAttr { name, label, short, ty, keyable: true, readable: true, writable: true, mappable: false, published: false, children: vec![], parent: None, hidden: false }
    }
    fn scalar(name: &str, ty: CeType) -> Self {
        Self::new(name, ty)
    }
    fn vector(name: &str, axes: [&str; 3]) -> Self {
        Self::vector_flags(name, axes, true, true)
    }
    fn vector_flags(name: &str, axes: [&str; 3], keyable: bool, writable: bool) -> Self {
        let mut a = Self::new(name, CeType::Vector);
        a.keyable = keyable;
        a.writable = writable;
        a.children = axes
            .iter()
            .map(|k| {
                let mut c = Self::new(format!("{name}{k}"), CeType::Float);
                c.parent = Some(name.to_string());
                c.keyable = keyable;
                c.writable = writable;
                c
            })
            .collect();
        a
    }
    fn with_keyable(mut self, yes: bool) -> Self {
        self.keyable = yes;
        self
    }
    fn with_writable(mut self, yes: bool) -> Self {
        self.writable = yes;
        self
    }
    fn with_mappable(mut self, yes: bool) -> Self {
        self.mappable = yes;
        self
    }
    fn with_hidden(mut self, yes: bool) -> Self {
        self.hidden = yes;
        self
    }
    #[cfg(test)]
    fn with_published(mut self, yes: bool) -> Self {
        self.published = yes;
        self
    }
}

/// Input rows allow several selections; Ctrl/Cmd toggles, a plain click replaces.
fn select_input(sel: &mut Vec<String>, name: &str, additive: bool) {
    let selected = sel.iter().any(|x| x == name);
    if additive {
        if selected {
            sel.retain(|x| x != name);
        } else {
            sel.push(name.to_string());
        }
    } else if selected && sel.len() == 1 {
        sel.clear();
    } else {
        sel.clear();
        sel.push(name.to_string());
    }
}

fn sort_attrs(attrs: &mut [CeAttr], order: CeOrder) {
    if order == CeOrder::Default {
        return;
    }
    attrs.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    if order == CeOrder::Descending {
        attrs.reverse();
    }
    for a in attrs.iter_mut() {
        sort_attrs(&mut a.children, order);
    }
}

/// Values from a light / material / texture bag, typed the way the Connection Editor needs them.
fn bag_attrs(params: &inca_core::params::Params, mappable: &[&str], keyable: impl Fn(&str) -> bool) -> Vec<CeAttr> {
    let mut out = Vec::new();
    for (k, v) in params {
        let mut a = match v {
            serde_json::Value::Number(_) => CeAttr::scalar(k, CeType::Float),
            serde_json::Value::Bool(_) => CeAttr::scalar(k, CeType::Bool),
            serde_json::Value::Array(a) if a.len() == 3 && a.iter().all(|x| x.is_number()) => CeAttr::vector(k, ["R", "G", "B"]),
            _ => continue,
        };
        a.keyable = keyable(k);
        a.mappable = mappable.contains(&k.as_str());
        out.push(a);
    }
    out
}

fn dag_attrs(s: &Scene, id: &str) -> Vec<CeAttr> {
    let Some(n) = s.node(id) else { return vec![] };
    let mut out = vec![
        CeAttr::scalar("visibility", CeType::Bool),
        CeAttr::scalar("template", CeType::Bool).with_keyable(false),
        CeAttr::vector("translate", ["X", "Y", "Z"]),
        CeAttr::vector("rotate", ["X", "Y", "Z"]),
        CeAttr::vector("scale", ["X", "Y", "Z"]),
        CeAttr::vector_flags("rotatePivot", ["X", "Y", "Z"], false, true),
        CeAttr::vector_flags("scalePivot", ["X", "Y", "Z"], false, true),
    ];
    if let Some(e) = n.extra.as_object() {
        for (k, v) in e {
            match v {
                serde_json::Value::Number(_) => out.push(CeAttr::scalar(k, CeType::Float)),
                serde_json::Value::Bool(_) => out.push(CeAttr::scalar(k, CeType::Bool)),
                _ => {}
            }
        }
    }
    match &n.shape {
        Shape::Mesh(_) => out.push(CeAttr::scalar("smoothLevel", CeType::Int).with_keyable(false)),
        Shape::Light(l) => out.extend(bag_attrs(&l.attrs, &[], |_| true)),
        Shape::Camera(c) => {
            if let Ok(serde_json::Value::Object(o)) = serde_json::to_value(c) {
                out.extend(bag_attrs(&o, &[], |k| k == "focalLength"));
            }
        }
        _ => {}
    }
    for (name, p) in &n.published {
        if out.iter().any(|a| a.name == *name) {
            continue;
        }
        let Some(target) = s.published_ref(p) else { continue };
        if matches!(&target, NodeRef::Dag(tid) if s.node(tid).is_some_and(|t| t.kind == "container")) {
            continue;
        }
        let Some(bound) = find_attr(&ce_attrs(s, &target), &p.attr).cloned() else { continue };
        let mut a = CeAttr::new(name, bound.ty);
        a.keyable = bound.keyable;
        a.readable = bound.readable;
        a.writable = bound.writable;
        a.mappable = bound.mappable;
        a.published = true;
        out.push(a);
    }
    out
}

fn history_attrs(s: &Scene, id: &str, i: usize) -> Vec<CeAttr> {
    let Some(h) = s.node(id).and_then(|n| n.mesh()).and_then(|m| m.history.get(i)) else { return vec![] };
    let mut out = Vec::new();
    if let Some((_, _, channels)) = inca_core::ops::info(&h.ty) {
        for ch in channels {
            let ty = match ch.kind {
                inca_core::primitives::ChanKind::Float => CeType::Float,
                inca_core::primitives::ChanKind::Int => CeType::Int,
                inca_core::primitives::ChanKind::Bool => CeType::Bool,
                inca_core::primitives::ChanKind::Enum(_) => continue,
            };
            let mut a = CeAttr::scalar(ch.key, ty);
            if let Some(label) = ch.label {
                a.label = label.to_string();
            }
            out.push(a);
        }
    }
    if out.is_empty() {
        for (k, v) in &h.params {
            if v.is_number() {
                out.push(CeAttr::scalar(k, CeType::Float));
            } else if v.is_boolean() {
                out.push(CeAttr::scalar(k, CeType::Bool));
            }
        }
    }
    out
}

fn material_attrs(s: &Scene, id: &str) -> Vec<CeAttr> {
    let Some(m) = s.mats.get(id) else { return vec![] };
    let mappable = inca_core::materials::mat_type(&m.ty).mappable;
    let mut out = bag_attrs(&m.attrs, mappable, |_| true);
    for slot in mappable {
        if !m.attrs.contains_key(*slot) {
            out.push(CeAttr::scalar(slot, CeType::Float).with_mappable(true).with_writable(false));
        }
    }
    out
}

fn texture_attrs(s: &Scene, id: &str) -> Vec<CeAttr> {
    let Some(t) = s.texs.get(id) else { return vec![] };
    let mut out = vec![
        CeAttr::new("outColor", CeType::TexColor).with_keyable(false).with_writable(false),
        CeAttr::new("outAlpha", CeType::Float).with_keyable(false).with_writable(false).with_hidden(true),
    ];
    out.extend(bag_attrs(&t.attrs, &[], |_| true));
    out
}

fn ce_attrs(s: &Scene, r: &NodeRef) -> Vec<CeAttr> {
    match r {
        NodeRef::Dag(id) => dag_attrs(s, id),
        NodeRef::History(id, i) => history_attrs(s, id, *i),
        NodeRef::Material(id) => material_attrs(s, id),
        NodeRef::Texture(id) => texture_attrs(s, id),
        NodeRef::Place2D(id) => s.places.get(id).map(|p| bag_attrs(&p.attrs, &[], |_| true)).unwrap_or_default(),
    }
}

fn find_attr<'a>(attrs: &'a [CeAttr], name: &str) -> Option<&'a CeAttr> {
    for a in attrs {
        if a.name == name {
            return Some(a);
        }
        if let Some(c) = a.children.iter().find(|c| c.name == name) {
            return Some(c);
        }
    }
    None
}

/// Can `src` (an output) drive `dst` (an input)? Textures with outColor drive any mappable slot.
fn compatible(src: &CeAttr, dst: &CeAttr) -> bool {
    if src.ty == CeType::TexColor {
        return dst.mappable;
    }
    if !dst.writable {
        return false;
    }
    match (src.ty, dst.ty) {
        (CeType::Vector, CeType::Vector) => true,
        (CeType::Float | CeType::Int | CeType::Bool, CeType::Float | CeType::Int | CeType::Bool) => true,
        _ => false,
    }
}

/// Is this attribute, one of its children, or its parent compound wired on that side?
fn attr_connected(s: &Scene, node: &NodeRef, attr: &CeAttr, as_source: bool) -> bool {
    if let Some((target, target_attr)) = published_plug(s, node, &attr.name) {
        if let Some(target_attr) = find_attr(&ce_attrs(s, &target), &target_attr) {
            return attr_connected(s, &target, target_attr, as_source);
        }
    }
    let id = s.ref_id(node);
    let fam: Vec<&str> = std::iter::once(attr.name.as_str())
        .chain(attr.children.iter().map(|c| c.name.as_str()))
        .chain(attr.parent.as_deref())
        .collect();
    if as_source {
        if s.conns.iter().any(|c| c.src == id && fam.contains(&c.src_attr.as_str())) {
            return true;
        }
        if matches!(node, NodeRef::Texture(_)) && (attr.name == "outColor" || attr.name == "outAlpha") {
            return output_wired(s, node, &attr.name);
        }
        return false;
    }
    if s.conns.iter().any(|c| c.dst == id && fam.contains(&c.dst_attr.as_str())) {
        return true;
    }
    if let NodeRef::Material(mid) = node {
        if let Some(m) = s.mats.get(mid) {
            return m.maps.keys().any(|k| fam.contains(&k.as_str()));
        }
    }
    false
}

/// Exact connection, including a texture map stored on the material.
fn connected(s: &Scene, src: &NodeRef, src_attr: &str, dst: &NodeRef, dst_attr: &str) -> bool {
    let (src, src_attr) = resolve_ce_plug(s, src, src_attr);
    let (dst, dst_attr) = resolve_ce_plug(s, dst, dst_attr);
    if matches!(src, NodeRef::Texture(_)) && matches!(dst, NodeRef::Material(_)) && (src_attr == "outColor" || src_attr == "outAlpha") {
        if let NodeRef::Material(mid) = &dst {
            return s.mats.get(mid).and_then(|m| m.maps.get(&dst_attr)).and_then(|v| v.as_str()) == Some(s.ref_id(&src).as_str());
        }
    }
    let sid = s.ref_id(&src);
    let did = s.ref_id(&dst);
    s.conns.iter().any(|c| c.src == sid && c.src_attr == src_attr && c.dst == did && c.dst_attr == dst_attr)
}

fn shown(s: &Scene, node: &NodeRef, attr: &CeAttr, as_source: bool, filter: &CeFilter) -> bool {
    if filter.mode == CeMode::Inputs && !attr.writable && !attr.mappable {
        return false;
    }
    if filter.mode == CeMode::Outputs && !attr.readable {
        return false;
    }
    if filter.mode == CeMode::Published && !attr.published {
        return false;
    }
    if !filter.non_keyable && !attr.keyable {
        return false;
    }
    if filter.connected_only && !attr_connected(s, node, attr, as_source) {
        return false;
    }
    !(!filter.hidden && attr.hidden)
}

/// Nodes upstream or downstream of `node`: direct connections, texture maps, the history chain,
/// the object's last history node, and its material.
fn neighbours(s: &Scene, node: &NodeRef, down: bool) -> Vec<NodeRef> {
    let id = s.ref_id(node);
    let mut out: Vec<NodeRef> = Vec::new();
    let mut push = |r: Option<NodeRef>| {
        if let Some(r) = r {
            if !out.contains(&r) {
                out.push(r);
            }
        }
    };
    for c in &s.conns {
        if down && c.src == id {
            push(s.ref_by_id(&c.dst));
        }
        if !down && c.dst == id {
            push(s.ref_by_id(&c.src));
        }
    }
    for m in s.mats.values() {
        for tid in m.maps.values() {
            let Some(tid) = tid.as_str() else { continue };
            if down && tid == id {
                push(Some(NodeRef::Material(m.id.clone())));
            }
            if !down && m.id == id {
                push(Some(NodeRef::Texture(tid.to_string())));
            }
        }
    }
    if let NodeRef::Dag(id2) = node {
        if let Some(n) = s.node(id2) {
            if n.kind == "container" {
                for p in n.published.values() {
                    if let Some(target) = s.published_ref(p) {
                        for next in neighbours(s, &target, down) {
                            push(Some(next));
                        }
                    }
                }
            }
        }
    }
    match node {
        NodeRef::History(id2, i) => {
            let hlen = s.node(id2).and_then(|n| n.mesh()).map(|m| m.history.len()).unwrap_or(0);
            if down {
                if *i + 1 < hlen {
                    push(Some(NodeRef::History(id2.clone(), *i + 1)));
                } else {
                    push(Some(NodeRef::Dag(id2.clone())));
                }
            } else if *i > 0 {
                push(Some(NodeRef::History(id2.clone(), *i - 1)));
            }
        }
        NodeRef::Dag(id2) => {
            let n = s.node(id2);
            if !down {
                if let Some(m) = n.and_then(|n| n.mesh()) {
                    if !m.history.is_empty() {
                        push(Some(NodeRef::History(id2.clone(), m.history.len() - 1)));
                    }
                }
            } else if let Some(mid) = n.and_then(|n| n.mesh()).map(|m| m.material.clone()) {
                push(Some(NodeRef::Material(mid)));
            }
        }
        _ => {}
    }
    out
}

/// The node a Reload button should load: the Hypershade material/texture when that window is
/// open and has a selection, otherwise the DAG lead.
fn selected_node(app: &IncaApp) -> Option<NodeRef> {
    if app.win.hypershade {
        if let Some(id) = &app.windows_state.hs_selected {
            if app.s.mats.contains_key(id) {
                return Some(NodeRef::Material(id.clone()));
            }
            if app.s.texs.contains_key(id) {
                return Some(NodeRef::Texture(id.clone()));
            }
        }
    }
    app.s.lead().cloned().map(NodeRef::Dag)
}

/// Load the selected node(s): two DAG objects fill both sides, one fills the left side only.
pub fn load_connection_editor(app: &mut IncaApp) {
    let mut nodes: Vec<NodeRef> = app.s.sel.iter().map(|id| NodeRef::Dag(id.clone())).collect();
    if nodes.is_empty() {
        if let Some(n) = selected_node(app) {
            nodes.push(n);
        }
    }
    app.hg.ce_left = nodes.first().cloned();
    app.hg.ce_right = if nodes.len() > 1 { nodes.last().cloned() } else { None };
    app.hg.ce_out = None;
    app.hg.ce_in.clear();
    app.hg.ce_raise = true;
}

/// Open the Connection Editor with the Hypershade's selected material or texture.
pub fn open_for_hypershade(app: &mut IncaApp) {
    let Some(id) = app.windows_state.hs_selected.clone() else { return };
    let node = if app.s.mats.contains_key(&id) {
        NodeRef::Material(id)
    } else if app.s.texs.contains_key(&id) {
        NodeRef::Texture(id)
    } else {
        return;
    };
    // A texture is normally the output and a material the input, so put each on its natural
    // side: this makes the common texture -> material connection a two-click operation.
    if matches!(&node, NodeRef::Texture(_)) {
        app.hg.ce_left = Some(node);
        if app.hg.ce_right.as_ref().is_some_and(|n| matches!(n, NodeRef::Texture(_))) {
            app.hg.ce_right = None;
        }
    } else if matches!(&node, NodeRef::Material(_)) {
        app.hg.ce_right = Some(node);
        if app.hg.ce_left.as_ref().is_some_and(|n| matches!(n, NodeRef::Material(_))) {
            app.hg.ce_left = None;
        }
    }
    app.hg.ce_out = None;
    app.hg.ce_in.clear();
    app.hg.ce_raise = true;
    app.win.connection_editor = true;
}

/// A container's published name resolved to its internal plug.
fn published_plug(s: &Scene, node: &NodeRef, attr: &str) -> Option<(NodeRef, String)> {
    if let NodeRef::Dag(id) = node {
        s.published_target(id, attr)
    } else {
        None
    }
}

fn resolve_ce_plug(s: &Scene, node: &NodeRef, attr: &str) -> (NodeRef, String) {
    match published_plug(s, node, attr) {
        Some((target, target_attr)) => (target, target_attr),
        None => (node.clone(), attr.to_string()),
    }
}

/// The connection driving `dst.attr`, including a texture map stored on a material. Returns
/// (source id, source attribute, is a texture map).
fn input_conn(s: &Scene, dst: &NodeRef, attr: &str) -> Option<(String, String, bool)> {
    if let Some((target, target_attr)) = published_plug(s, dst, attr) {
        return input_conn(s, &target, &target_attr);
    }
    if let NodeRef::Material(mid) = dst {
        if let Some(m) = s.mats.get(mid) {
            if let Some(tid) = m.maps.get(attr).and_then(|v| v.as_str()) {
                return Some((tid.to_string(), m.map_out(attr).to_string(), true));
            }
        }
    }
    let did = s.ref_id(dst);
    s.conns.iter().find(|c| c.dst == did && c.dst_attr == attr).map(|c| (c.src.clone(), c.src_attr.clone(), false))
}

/// Whether this output attribute feeds any input, including texture maps.
fn output_wired(s: &Scene, src: &NodeRef, attr: &str) -> bool {
    if let Some((target, target_attr)) = published_plug(s, src, attr) {
        return output_wired(s, &target, &target_attr);
    }
    let sid = s.ref_id(src);
    if s.conns.iter().any(|c| c.src == sid && c.src_attr == attr) {
        return true;
    }
    if matches!(src, NodeRef::Texture(_)) {
        return s.mats.values().any(|m| m.maps.iter().any(|(slot, v)| v.as_str() == Some(sid.as_str()) && m.map_out(slot) == attr));
    }
    false
}

fn reload_side(app: &mut IncaApp, side: usize) {
    if let Some(n) = selected_node(app) {
        if side == 0 {
            app.hg.ce_left = Some(n);
        } else {
            app.hg.ce_right = Some(n);
        }
        app.hg.ce_out = None;
        app.hg.ce_in.clear();
    } else {
        app.warn("// Warning: Nothing selected to load into the Connection Editor.");
    }
}

fn navigate(app: &mut IncaApp, down: bool, pick: Option<NodeRef>) {
    let (l, r) = (app.hg.ce_left.clone(), app.hg.ce_right.clone());
    let base = if down { r.clone().or(l.clone()) } else { l.clone().or(r.clone()) };
    let Some(base) = base else { return };
    let next = pick.or_else(|| neighbours(&app.s, &base, down).first().cloned());
    let Some(next) = next else {
        app.help = format!("{} has no {} nodes", app.s.ref_name(&base), if down { "downstream" } else { "upstream" });
        return;
    };
    if down {
        app.hg.ce_left = Some(base);
        app.hg.ce_right = Some(next);
    } else {
        app.hg.ce_left = Some(next);
        app.hg.ce_right = Some(base);
    }
    app.hg.ce_out = None;
    app.hg.ce_in.clear();
    app.hg.ce_raise = true;
}

fn nav_menu(app: &IncaApp, ui: &mut egui::Ui, down: bool, nav: &mut Option<(bool, Option<NodeRef>)>) {
    let base = if down { app.hg.ce_right.clone().or(app.hg.ce_left.clone()) } else { app.hg.ce_left.clone().or(app.hg.ce_right.clone()) };
    let Some(base) = base else {
        ui.label("(no node)");
        return;
    };
    let cands = neighbours(&app.s, &base, down);
    if cands.is_empty() {
        ui.label("(none)");
        return;
    }
    for n in cands {
        if ui.button(app.s.ref_name(&n)).clicked() {
            *nav = Some((down, Some(n)));
            ui.close();
        }
    }
}

fn ce_display_menu(ui: &mut egui::Ui, f: &mut CeFilter) {
    ui.radio_value(&mut f.mode, CeMode::Readable, "Show Readable");
    ui.radio_value(&mut f.mode, CeMode::Outputs, "Show Outputs Only");
    ui.radio_value(&mut f.mode, CeMode::Inputs, "Show Inputs Only");
    ui.radio_value(&mut f.mode, CeMode::Published, "Show Published Only");
    ui.separator();
    ui.checkbox(&mut f.non_keyable, "Show Non-Keyable");
    ui.checkbox(&mut f.connected_only, "Show Connected Only");
    ui.checkbox(&mut f.hidden, "Show Hidden");
}

enum CeEdit {
    Connect { src: String, src_attr: String, dst: String, dst_attr: String },
    Disconnect { src: String, src_attr: String, dst: String, dst_attr: String },
}

fn connect_edit(s: &Scene, src: &NodeRef, src_attr: &str, dst: &NodeRef, dst_attr: &str) -> CeEdit {
    CeEdit::Connect { src: s.ref_name(src), src_attr: src_attr.into(), dst: s.ref_name(dst), dst_attr: dst_attr.into() }
}

fn disconnect_edit(s: &Scene, src_id: &str, src_attr: &str, dst: &NodeRef, dst_attr: &str) -> CeEdit {
    let src = s.ref_by_id(src_id).map(|r| s.ref_name(&r)).unwrap_or_else(|| src_id.to_string());
    CeEdit::Disconnect { src, src_attr: src_attr.into(), dst: s.ref_name(dst), dst_attr: dst_attr.into() }
}

fn apply_edit(app: &mut IncaApp, edit: CeEdit) {
    let (label, cmd) = match edit {
        CeEdit::Connect { src, src_attr, dst, dst_attr } => ("connect", format!("connectAttr -f {src}.{src_attr} {dst}.{dst_attr};")),
        CeEdit::Disconnect { src, src_attr, dst, dst_attr } => ("disconnect", format!("disconnectAttr {src}.{src_attr} {dst}.{dst_attr};")),
    };
    if let Err(e) = app.run_mel(label, &cmd) {
        app.warn(e);
    }
}

struct CeRow {
    clicked: bool,
    toggle: bool,
}

fn ce_row(ui: &mut egui::Ui, a: &CeAttr, child: bool, open: bool, selected: bool, connected: bool, disabled: bool, names: CeName) -> CeRow {
    let (mut clicked, mut toggle) = (false, false);
    ui.horizontal(|ui| {
        if child {
            ui.add_space(14.0);
        }
        if a.children.is_empty() {
            ui.add_space(10.0);
        } else {
            let (r, resp) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::click());
            let c = r.center();
            let pts = if open {
                vec![
                    c + egui::vec2(-3.5, -2.0),
                    c + egui::vec2(3.5, -2.0),
                    c + egui::vec2(0.0, 2.5),
                ]
            } else {
                vec![
                    c + egui::vec2(-2.0, -3.5),
                    c + egui::vec2(2.5, 0.0),
                    c + egui::vec2(-2.0, 3.5),
                ]
            };
            let col = if resp.hovered() {
                theme::TEXT
            } else {
                theme::TEXT_DIM
            };
            ui.painter()
                .add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
            toggle = resp
                .on_hover_text(if open { "Collapse" } else { "Expand" })
                .clicked();
        }
        let text = match names {
            CeName::Nice => &a.label,
            CeName::Long => &a.name,
            CeName::Short => &a.short,
        };
        let mut txt = RichText::new(text);
        if connected {
            txt = txt.strong().italics();
        }
        if disabled {
            txt = txt.color(Color32::from_rgb(107, 107, 107));
        } else if connected {
            txt = txt.color(Color32::from_rgb(140, 200, 255));
        }
        let r = ui.selectable_label(selected, txt).on_hover_text(&a.name);
        clicked = r.clicked() && !disabled;
    });
    CeRow { clicked, toggle }
}

pub fn connection_editor(app: &mut IncaApp, ctx: &egui::Context) {
    let mut open = app.win.connection_editor;
    if !open {
        return;
    }
    if app.hg.ce_raise {
        ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("Connection Editor")));
        app.hg.ce_raise = false;
    }
    let mut edits: Vec<CeEdit> = Vec::new();
    let mut clear_all = false;
    let mut remove_output = false;
    let mut close = false;
    let mut nav: Option<(bool, Option<NodeRef>)> = None;
    egui::Window::new("Connection Editor").order(egui::Order::Foreground).open(&mut open).default_size([560.0, 520.0]).constrain(false).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("Options", |ui| {
                if ui.checkbox(&mut app.hg.ce_auto, "Auto-connect").changed() {
                    app.hg.ce_in.clear();
                }
                ui.menu_button("Channel Names", |ui| {
                    ui.radio_value(&mut app.hg.ce_names, CeName::Nice, "Nice");
                    ui.radio_value(&mut app.hg.ce_names, CeName::Long, "Long");
                    ui.radio_value(&mut app.hg.ce_names, CeName::Short, "Short");
                });
                ui.menu_button("Attribute Order", |ui| {
                    ui.radio_value(&mut app.hg.ce_order, CeOrder::Default, "Default");
                    ui.radio_value(&mut app.hg.ce_order, CeOrder::Ascending, "Alphabetical, ascend");
                    ui.radio_value(&mut app.hg.ce_order, CeOrder::Descending, "Alphabetical, descend");
                });
            });
            ui.menu_button("Left Display", |ui| ce_display_menu(ui, &mut app.hg.ce_filter[0]));
            ui.menu_button("Right Display", |ui| ce_display_menu(ui, &mut app.hg.ce_filter[1]));
            ui.menu_button("Help", |ui| {
                if ui.button("Help on Connection Editor...").clicked() {
                    app.help = "Click an output attribute, then a highlighted input attribute to connect them; click a connected input again to break it. Reload Left / Right load the selected node.".into();
                    ui.close();
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Reload Left").clicked() {
                reload_side(app, 0);
            }
            if ui.button("Reload Right").clicked() {
                reload_side(app, 1);
            }
            ui.separator();
            if ui.button(if app.hg.ce_dir { "from -> to" } else { "to <- from" }).on_hover_text("Swap which side shows outputs and inputs").clicked() {
                app.hg.ce_dir = !app.hg.ce_dir;
                app.hg.ce_out = None;
                app.hg.ce_in.clear();
            }
            ui.separator();
            let up = ui.button("<").on_hover_text("Upstream: load the node feeding the left node (right-click: choose)");
            if up.clicked() {
                nav = Some((false, None));
            }
            let down = ui.button(">").on_hover_text("Downstream: load the node the right node feeds (right-click: choose)");
            if down.clicked() {
                nav = Some((true, None));
            }
            up.context_menu(|ui| nav_menu(app, ui, false, &mut nav));
            down.context_menu(|ui| nav_menu(app, ui, true, &mut nav));
        });
        ui.separator();
        let (l, r) = (app.hg.ce_left.clone(), app.hg.ce_right.clone());
        let out_node = if app.hg.ce_dir { l.as_ref() } else { r.as_ref() };
        let in_node = if app.hg.ce_dir { r.as_ref() } else { l.as_ref() };
        let mut out_attrs = out_node.map(|n| ce_attrs(&app.s, n)).unwrap_or_default();
        let mut in_attrs = in_node.map(|n| ce_attrs(&app.s, n)).unwrap_or_default();
        sort_attrs(&mut out_attrs, app.hg.ce_order);
        sort_attrs(&mut in_attrs, app.hg.ce_order);
        let out_sel = app.hg.ce_out.as_deref().and_then(|n| find_attr(&out_attrs, n)).cloned();
        let mut ce_out = app.hg.ce_out.clone();
        let mut ce_in = app.hg.ce_in.clone();
        let additive = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let mut open_sets = [app.hg.ce_open[0].clone(), app.hg.ce_open[1].clone()];
        ui.columns(2, |cols| {
            for side in 0..2 {
                let node = if side == 0 { &l } else { &r };
                let out_role = (side == 0) == app.hg.ce_dir;
                let title = if out_role { "Outputs" } else { "Inputs" };
                let name = node.as_ref().map(|n| app.s.ref_name(n)).unwrap_or_else(|| "-".into());
                cols[side].label(RichText::new(format!("{title}: {name}")).strong());
                let Some(node) = node else {
                    cols[side].label(
                        RichText::new(format!("Select a node and click Reload {}", if side == 0 { "Left" } else { "Right" }))
                            .italics()
                            .color(theme::TEXT_DIM)
                            .size(10.5),
                    );
                    continue;
                };
                let attrs = if out_role { &out_attrs } else { &in_attrs };
                let filter = &app.hg.ce_filter[side];
                cols[side].with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt(if side == 0 { "ceL" } else { "ceR" }).show(ui, |ui| {
                    let mut any = false;
                    for a in attrs {
                        let visible: Vec<&CeAttr> = a.children.iter().filter(|c| shown(&app.s, node, c, out_role, filter)).collect();
                        if !shown(&app.s, node, a, out_role, filter) && visible.is_empty() {
                            continue;
                        }
                        any = true;
                        let is_open = open_sets[side].contains(&a.name);
                        let wired = attr_connected(&app.s, node, a, out_role);
                        let selected = if out_role { ce_out.as_deref() == Some(a.name.as_str()) } else { ce_in.iter().any(|x| x == &a.name) };
                        let disabled = !out_role
                            && match &out_sel {
                                Some(sa) => !compatible(sa, a),
                                None => !a.writable && !a.mappable,
                            };
                        let row = ce_row(ui, a, false, is_open, selected, wired, disabled, app.hg.ce_names);
                        if row.toggle {
                            if is_open {
                                open_sets[side].remove(&a.name);
                            } else {
                                open_sets[side].insert(a.name.clone());
                            }
                        }
                        if row.clicked {
                            if out_role {
                                ce_out = if selected { None } else { Some(a.name.clone()) };
                                ce_in.clear();
                            } else if app.hg.ce_auto {
                                let from = input_conn(&app.s, node, &a.name);
                                if let Some(oa) = ce_out.clone() {
                                    if out_node.is_some_and(|on| connected(&app.s, on, &oa, node, &a.name)) {
                                        if let Some((s, sa, _)) = from {
                                            edits.push(disconnect_edit(&app.s, &s, &sa, node, &a.name));
                                        }
                                    } else if let Some(on) = out_node {
                                        edits.push(connect_edit(&app.s, on, &oa, node, &a.name));
                                    }
                                } else {
                                    select_input(&mut ce_in, &a.name, additive);
                                }
                            } else {
                                select_input(&mut ce_in, &a.name, additive);
                            }
                        }
                        if is_open {
                            for c in visible {
                                let c_wired = attr_connected(&app.s, node, c, out_role);
                                let c_selected = if out_role { ce_out.as_deref() == Some(c.name.as_str()) } else { ce_in.iter().any(|x| x == &c.name) };
                                let c_disabled = !out_role
                                    && match &out_sel {
                                        Some(sa) => !compatible(sa, c),
                                        None => !c.writable && !c.mappable,
                                    };
                                let c_row = ce_row(ui, c, true, false, c_selected, c_wired, c_disabled, app.hg.ce_names);
                                if c_row.clicked {
                                    if out_role {
                                        ce_out = if c_selected { None } else { Some(c.name.clone()) };
                                        ce_in.clear();
                                    } else if app.hg.ce_auto {
                                        let from = input_conn(&app.s, node, &c.name);
                                        if let Some(oa) = ce_out.clone() {
                                            if out_node.is_some_and(|on| connected(&app.s, on, &oa, node, &c.name)) {
                                                if let Some((s, sa, _)) = from {
                                                    edits.push(disconnect_edit(&app.s, &s, &sa, node, &c.name));
                                                }
                                            } else if let Some(on) = out_node {
                                                edits.push(connect_edit(&app.s, on, &oa, node, &c.name));
                                            }
                                        } else {
                                            select_input(&mut ce_in, &c.name, additive);
                                        }
                                    } else {
                                        select_input(&mut ce_in, &c.name, additive);
                                    }
                                }
                            }
                        }
                    }
                    if !any {
                        ui.label(RichText::new("No attributes match the display filters.").italics().color(theme::TEXT_DIM).size(10.5));
                    }
                });
                });
            }
        });
        app.hg.ce_out = ce_out.clone();
        app.hg.ce_in = ce_in.clone();
        app.hg.ce_open = open_sets;
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Clear All").on_hover_text("Remove the nodes from both sides").clicked() {
                clear_all = true;
            }
            if ui.add_enabled(out_node.is_some(), egui::Button::new("Remove")).on_hover_text("Remove the node from the output pane").clicked() {
                remove_output = true;
            }
            let make_ok = !app.hg.ce_auto && ce_out.is_some() && !ce_in.is_empty() && out_node.is_some() && in_node.is_some();
            if ui.add_enabled(make_ok, egui::Button::new("Make")).on_hover_text("Connect the highlighted attributes").clicked() {
                if let (Some(on), Some(inn), Some(oa)) = (out_node, in_node, &ce_out) {
                    let src = find_attr(&out_attrs, oa.as_str()).cloned();
                    for ia in &ce_in {
                        if let (Some(sa), Some(da)) = (&src, find_attr(&in_attrs, ia.as_str())) {
                            if compatible(sa, da) {
                                edits.push(connect_edit(&app.s, on, oa, inn, ia));
                            }
                        }
                    }
                }
            }
            let break_ok = !app.hg.ce_auto && ce_out.is_some() && !ce_in.is_empty() && out_node.is_some() && in_node.is_some();
            if ui.add_enabled(break_ok, egui::Button::new("Break")).on_hover_text("Break the highlighted connection").clicked() {
                if let (Some(on), Some(inn), Some(oa)) = (out_node, in_node, &ce_out) {
                    for ia in &ce_in {
                        if connected(&app.s, on, oa, inn, ia) {
                            edits.push(disconnect_edit(&app.s, &app.s.ref_id(on), oa, inn, ia));
                        }
                    }
                }
            }
            if ui.button("Close").clicked() {
                close = true;
            }
            ui.separator();
            ui.label(RichText::new(if app.hg.ce_auto { "Auto-connect is on" } else { "Auto-connect is off" }).color(theme::TEXT_DIM).size(10.5));
        });
    });
    for e in edits {
        apply_edit(app, e);
    }
    if clear_all {
        app.hg.ce_left = None;
        app.hg.ce_right = None;
        app.hg.ce_out = None;
        app.hg.ce_in.clear();
        app.hg.ce_open = std::array::from_fn(|_| HashSet::new());
    }
    if remove_output {
        if app.hg.ce_dir {
            app.hg.ce_left = None;
        } else {
            app.hg.ce_right = None;
        }
        app.hg.ce_out = None;
        app.hg.ce_in.clear();
    }
    if let Some((down, pick)) = nav {
        navigate(app, down, pick);
    }
    if close {
        open = false;
    }
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
    fn connection_helpers_see_regular_and_map_connections() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), Some("a"));
        let b = s.create_mesh("polyCube", &Params::new(), Some("b"));
        s.conns.push(inca_core::scene::Conn { src: a.clone(), src_attr: "translateX".into(), dst: b.clone(), dst_attr: "translateY".into() });
        assert!(output_wired(&s, &NodeRef::Dag(a.clone()), "translateX"));
        assert!(!output_wired(&s, &NodeRef::Dag(a.clone()), "translateY"));
        assert_eq!(input_conn(&s, &NodeRef::Dag(b.clone()), "translateY"), Some((a, "translateX".into(), false)));
        // texture maps live on the material, not in `Scene::conns`
        let m = s.create_material("standardSurface", Some("m"));
        let t = s.create_texture("checker", Some("tex"));
        s.mats.get_mut(&m).unwrap().maps.insert("baseColor".into(), serde_json::json!(t));
        assert_eq!(input_conn(&s, &NodeRef::Material(m.clone()), "baseColor"), Some((t.clone(), "outColor".into(), true)));
        assert!(output_wired(&s, &NodeRef::Texture(t.clone()), "outColor"));
        assert!(!output_wired(&s, &NodeRef::Texture(t), "outAlpha"), "maps are recorded as outColor");
        assert_eq!(input_conn(&s, &NodeRef::Material(m), "specularRoughness"), None);
    }

    #[test]
    fn connection_editor_lists_typed_compound_attributes() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), Some("a"));
        let attrs = ce_attrs(&s, &NodeRef::Dag(a.clone()));
        let translate = attrs.iter().find(|x| x.name == "translate").unwrap();
        assert_eq!(translate.ty, CeType::Vector);
        assert_eq!(translate.label, "Translate");
        assert_eq!(translate.short, "t");
        assert_eq!(translate.children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["translateX", "translateY", "translateZ"]);
        assert_eq!(translate.children[1].short, "ty");
        assert_eq!(ce_label("specularIOR"), "Specular IOR");
        assert_eq!(ce_label("color1"), "Color 1");
        assert!(attrs.iter().any(|x| x.name == "visibility"));
        assert!(attrs.iter().any(|x| x.name == "smoothLevel"));
        assert!(find_attr(&attrs, "translateY").is_some());
        let template = attrs.iter().find(|x| x.name == "template").unwrap();
        assert!(!template.keyable);
        assert!(!shown(&s, &NodeRef::Dag(a.clone()), template, true, &CeFilter { non_keyable: false, ..CeFilter::default() }));
        assert!(!attrs.iter().find(|x| x.name == "smoothLevel").unwrap().keyable);

        let history = ce_attrs(&s, &NodeRef::History(a.clone(), 0));
        assert!(history.iter().any(|x| x.name == "width"), "generator channels are listed");
        assert!(history.iter().any(|x| x.name == "subdivisionsWidth"));

        let m = s.create_material("standardSurface", Some("m"));
        let ma = ce_attrs(&s, &NodeRef::Material(m));
        let base = ma.iter().find(|x| x.name == "baseColor").unwrap();
        assert_eq!(base.ty, CeType::Vector);
        assert!(base.mappable);
        assert_eq!(base.label, "Base Color");
        assert_eq!(base.short, "bc");
        assert_eq!(base.children[0].name, "baseColorR");
        assert_eq!(base.children[0].short, "bcr");

        let t = s.create_texture("checker", Some("tex"));
        let ta = ce_attrs(&s, &NodeRef::Texture(t));
        let out = ta.iter().find(|x| x.name == "outColor").unwrap();
        assert_eq!(out.ty, CeType::TexColor);
        assert_eq!(out.short, "oc");
        assert!(!out.writable);
        assert!(ta.iter().find(|x| x.name == "outAlpha").unwrap().hidden, "outAlpha is behind Show Hidden");
    }

    #[test]
    fn connection_editor_shows_published_container_attributes() {
        let mut s = Scene::new();
        let mat = s.create_material("standardSurface", Some("m"));
        let tex = s.create_texture("checker", Some("t"));
        let cube = s.create_mesh("polyCube", &Params::new(), Some("a"));
        let other = s.create_mesh("polyCube", &Params::new(), Some("b"));
        let container = s.create_container("asset1", None);
        s.conns.push(inca_core::scene::Conn { src: cube.clone(), src_attr: "translateX".into(), dst: other, dst_attr: "translateY".into() });
        s.mats.get_mut(&mat).unwrap().maps.insert("baseColor".into(), serde_json::json!(tex.clone()));
        let published = |kind: &str, node: &str, attr: &str| inca_core::scene::PublishedAttr { kind: kind.into(), node: node.into(), index: 0, attr: attr.into() };
        {
            let n = s.nodes.get_mut(&container).unwrap();
            n.published.insert("pubBase".into(), published("material", &mat, "baseColor"));
            n.published.insert("pubTex".into(), published("texture", &tex, "outColor"));
            n.published.insert("pubTx".into(), published("dag", &cube, "translateX"));
        }
        let attrs = ce_attrs(&s, &NodeRef::Dag(container.clone()));
        let base = attrs.iter().find(|x| x.name == "pubBase" && x.published).unwrap();
        assert_eq!(base.ty, CeType::Vector);
        assert!(base.mappable);
        let filter = CeFilter { mode: CeMode::Published, ..CeFilter::default() };
        assert!(shown(&s, &NodeRef::Dag(container.clone()), base, true, &filter));

        assert_eq!(input_conn(&s, &NodeRef::Dag(container.clone()), "pubBase"), Some((tex.clone(), "outColor".into(), true)));
        assert!(output_wired(&s, &NodeRef::Dag(container.clone()), "pubTex"));
        assert!(output_wired(&s, &NodeRef::Dag(container.clone()), "pubTx"));
        assert_eq!(s.get_attr(&NodeRef::Dag(container), "pubBase"), s.get_attr(&NodeRef::Material(mat), "baseColor"));
    }

    #[test]
    fn connection_editor_selects_multiple_inputs() {
        let mut sel = Vec::new();
        select_input(&mut sel, "a", false);
        assert_eq!(sel, ["a"]);
        select_input(&mut sel, "b", true);
        assert_eq!(sel, ["a", "b"]);
        select_input(&mut sel, "a", true);
        assert_eq!(sel, ["b"]);
        select_input(&mut sel, "c", false);
        assert_eq!(sel, ["c"]);
        select_input(&mut sel, "c", false);
        assert!(sel.is_empty());
    }

    #[test]
    fn connection_editor_sorts_attributes_and_filters_published() {
        let mut attrs = vec![CeAttr::scalar("visibility", CeType::Bool), CeAttr::scalar("translate", CeType::Vector), CeAttr::scalar("rotate", CeType::Vector)];
        sort_attrs(&mut attrs, CeOrder::Ascending);
        assert_eq!(attrs.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["rotate", "translate", "visibility"]);
        sort_attrs(&mut attrs, CeOrder::Descending);
        assert_eq!(attrs.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["visibility", "translate", "rotate"]);

        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), Some("a"));
        let filter = CeFilter { mode: CeMode::Published, ..CeFilter::default() };
        let published = CeAttr::scalar("publishedAttr", CeType::Float).with_published(true);
        let normal = CeAttr::scalar("normalAttr", CeType::Float);
        assert!(shown(&s, &NodeRef::Dag(id.clone()), &published, true, &filter));
        assert!(!shown(&s, &NodeRef::Dag(id), &normal, true, &filter));
    }

    #[test]
    fn connection_editor_filters_and_compatibility() {
        let f = CeAttr::scalar("from", CeType::Float);
        let i = CeAttr::scalar("to", CeType::Int);
        let v = CeAttr::vector("v", ["X", "Y", "Z"]);
        let tex = CeAttr::new("outColor", CeType::TexColor);
        let mut map = CeAttr::scalar("baseColor", CeType::Float);
        map.mappable = true;
        map.writable = false;
        assert!(compatible(&f, &i));
        assert!(compatible(&v, &v));
        assert!(!compatible(&v, &f));
        assert!(compatible(&tex, &map));
        assert!(!compatible(&tex, &f));
        assert!(!compatible(&f, &map));

        let filter = CeFilter { non_keyable: false, ..CeFilter::default() };
        let mut hidden = CeAttr::scalar("x", CeType::Float);
        hidden.keyable = false;
        hidden.hidden = true;
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), Some("a"));
        assert!(!shown(&s, &NodeRef::Dag(id.clone()), &hidden, true, &filter));
        let filter = CeFilter { hidden: true, ..CeFilter::default() };
        assert!(shown(&s, &NodeRef::Dag(id), &hidden, true, &filter));
    }

    #[test]
    fn connection_editor_connected_families_and_neighbours() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), Some("a"));
        let b = s.create_mesh("polyCube", &Params::new(), Some("b"));
        let attrs = ce_attrs(&s, &NodeRef::Dag(a.clone()));
        let translate = attrs.iter().find(|x| x.name == "translate").cloned().unwrap();
        s.conns.push(inca_core::scene::Conn { src: a.clone(), src_attr: "translateX".into(), dst: b.clone(), dst_attr: "translateY".into() });
        assert!(attr_connected(&s, &NodeRef::Dag(a.clone()), &translate, true), "a child connection lights up the compound");
        let b_attrs = ce_attrs(&s, &NodeRef::Dag(b.clone()));
        let b_translate = b_attrs.iter().find(|x| x.name == "translate").unwrap();
        assert!(attr_connected(&s, &NodeRef::Dag(b.clone()), b_translate, false));

        let down = neighbours(&s, &NodeRef::Dag(a.clone()), true);
        assert_eq!(down.first(), Some(&NodeRef::Dag(b.clone())));
        assert!(down.iter().any(|n| matches!(n, NodeRef::Material(_))), "the object's material is downstream");
        assert!(neighbours(&s, &NodeRef::Dag(b.clone()), false).contains(&NodeRef::Dag(a.clone())));
        assert_eq!(neighbours(&s, &NodeRef::Dag(a.clone()), false), vec![NodeRef::History(a, 0)], "the object's last history node is upstream");

        let mat = s.create_material("standardSurface", Some("m"));
        let tex = s.create_texture("checker", Some("t"));
        s.mats.get_mut(&mat).unwrap().maps.insert("baseColor".into(), serde_json::json!(tex.clone()));
        let ma = ce_attrs(&s, &NodeRef::Material(mat.clone()));
        assert!(attr_connected(&s, &NodeRef::Material(mat.clone()), ma.iter().find(|x| x.name == "baseColor").unwrap(), false));
        let ta = ce_attrs(&s, &NodeRef::Texture(tex.clone()));
        assert!(attr_connected(&s, &NodeRef::Texture(tex.clone()), ta.iter().find(|x| x.name == "outColor").unwrap(), true));
        assert_eq!(neighbours(&s, &NodeRef::Texture(tex), true), vec![NodeRef::Material(mat.clone())]);
        assert!(neighbours(&s, &NodeRef::Material(mat), false).iter().any(|n| matches!(n, NodeRef::Texture(_))));
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
