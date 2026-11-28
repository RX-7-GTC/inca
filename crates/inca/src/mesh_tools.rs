//! Interactive Mesh Tools: Insert Edge Loop, Multi-Cut, Target Weld,
//! Sculpt brushes, Quad Draw and Make Live.

use crate::app::{IncaApp, Tool};
use crate::viewport::{self, CamView};
use egui::{vec2, Align2, Color32, FontId, Pos2, Rect, Stroke};
use glam::{DMat4, DVec3};
use inca_core::mesh::{edge_ring, PolyMesh, SplitPoint, Topo};
use inca_core::params::Params;
use inca_core::scene::{NodeRef, Scene, SelMode};
use serde_json::json;
use std::collections::HashSet;

/// Sculpt brushes, as the JS Tool Settings list them.
pub const BRUSHES: &[&str] = &["sculpt", "smooth", "relax", "grab", "pinch", "flatten", "inflate"];

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MeshToolOpts {
    pub edge_loops: u32,
    pub brush: usize,
    pub size: f64,
    pub strength: f64,
    pub mirror: bool,
}

impl Default for MeshToolOpts {
    fn default() -> Self {
        MeshToolOpts { edge_loops: 1, brush: 0, size: 1.0, strength: 0.3, mirror: false }
    }
}

/// A Multi-Cut point: a vertex, or a parameter along an edge (with its world position for drawing).
#[derive(Clone, Debug)]
pub struct CutPt {
    vertex: Option<u32>,
    edge: Option<(u32, u32, f64)>,
    /// A point inside a face: face index and object-space position.
    face: Option<(usize, DVec3)>,
    /// Nearest face vertex for a face point (the start of its preview edge).
    anchor: Option<u32>,
    w: DVec3,
}

pub struct SculptStroke {
    id: String,
    orig: PolyMesh,
    touched: HashSet<u32>,
    brush: usize,
    sign: f64,
    grab: Option<(DVec3, Vec<(u32, f64)>)>,
}

#[derive(Clone)]
struct CutDrag {
    vp: usize,
    id: String,
    a: u32,
    b: u32,
    t: f64,
    start: Pos2,
}

#[derive(Default)]
pub struct MeshToolState {
    pub cut: Vec<CutPt>,
    pub cut_obj: Option<String>,
    /// Viewport the cut points were placed in; its camera expands the cut across edges.
    pub cut_vp: Option<usize>,
    /// Camera at the last placed point, so committing after orbiting keeps the cut path stable.
    pub cut_cam: Option<CamView>,
    /// Camera when each cut point was placed; a segment uses the camera of its second point.
    pub cut_cams: Vec<CamView>,
    /// Point being slid along an edge before the LMB is released.
    cut_drag: Option<CutDrag>,
    /// Placed point under the cursor, and the one being dragged (viewport, index).
    pub hover_point: Option<usize>,
    pub move_drag: Option<(usize, usize)>,
    /// MMB midpoint cut sampled while the button is down: (viewport, mesh, point).
    mid_cut: Option<(usize, String, CutPt)>,
    /// Insert Edge Loop drag: mesh, history index, screen ends of the clicked edge
    loop_drag: Option<(String, usize, Pos2, Pos2)>,
    /// the ring of edges (world ends) the dragged loop slides along, and whether it closes
    loop_ring: Option<(Vec<(DVec3, DVec3)>, bool)>,
    /// Target Weld drag: mesh, source vertex
    weld: Option<(String, u32)>,
    sculpt: Option<SculptStroke>,
    /// Quad Draw dots (world) and the mesh faces go into
    pub dots: Vec<DVec3>,
    pub quad_target: Option<String>,
    quad_drag: Option<QuadPick>,
}

impl MeshToolState {
    /// Drop a Multi-Cut session: placed points, the mesh they belong to, and any live drags.
    pub fn clear_cut(&mut self) {
        self.cut.clear();
        self.cut_obj = None;
        self.cut_vp = None;
        self.cut_cam = None;
        self.cut_cams.clear();
        self.mid_cut = None;
        self.cut_drag = None;
        self.hover_point = None;
        self.move_drag = None;
    }
}

pub fn is_mesh_tool(t: Tool) -> bool {
    matches!(t, Tool::EdgeLoop | Tool::MultiCut | Tool::TargetWeld | Tool::Sculpt | Tool::QuadDraw)
}

fn meshes(app: &IncaApp) -> Vec<String> {
    // hilite now includes curves (component mode), but these tools only edit meshes
    let hil: Vec<String> = app.s.hilite().into_iter().filter(|i| app.s.node(i).is_some_and(|n| n.mesh().is_some())).collect();
    if !hil.is_empty() {
        return hil;
    }
    let sel: Vec<String> = app.s.sel.iter().filter(|i| app.s.node(i).is_some_and(|n| n.mesh().is_some())).cloned().collect();
    if !sel.is_empty() {
        return sel;
    }
    app.s.all_dag(false).into_iter().filter(|i| app.s.is_visible(i) && app.s.node(i).is_some_and(|n| n.mesh().is_some())).collect()
}

fn world_verts(app: &IncaApp, id: &str) -> Vec<DVec3> {
    world_verts_of(&app.s, id)
}

fn world_verts_of(s: &Scene, id: &str) -> Vec<DVec3> {
    let wm = s.world_matrix(id);
    s.node(id).and_then(|n| n.mesh()).map(|m| m.mesh.v.iter().map(|p| wm.transform_point3(*p)).collect()).unwrap_or_default()
}

struct EdgeHit {
    id: String,
    a: u32,
    b: u32,
    t: f64,
    w: Vec<DVec3>,
    d: f32,
}

/// Nearest visible edge within 8 px; `t` runs from `a` to `b`.
fn pick_edge(app: &mut IncaApp, idx: usize, cv: &CamView, rect: Rect, p: Pos2, objs: &[String]) -> Option<EdgeHit> {
    let pick = viewport::MeshPick::new(&app.s, &app.vps[idx], &mut app.mesh_cache);
    let mut best: Option<EdgeHit> = None;
    for id in objs {
        let w = world_verts_of(&app.s, id);
        let scr: Vec<Option<Pos2>> = w.iter().map(|q| cv.project(*q, rect)).collect();
        let Some(topo) = app.mesh_cache.topo(&app.s, id) else { continue };
        for &(a, b) in &topo.edges {
            let (Some(sa), Some(sb)) = (scr[a as usize], scr[b as usize]) else { continue };
            let ab = sb - sa;
            let t = ((p - sa).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
            let d = (sa + ab * t).distance(p);
            if d < 8.0 && best.as_ref().is_none_or(|x| d < x.d) && pick.edge(&app.s, cv, id, a, b, t as f64, &mut app.mesh_cache) {
                best = Some(EdgeHit { id: id.clone(), a, b, t: t as f64, w: w.clone(), d });
            }
        }
    }
    best
}

/// A point inside a face, with the face's nearest vertex in screen space.
struct FaceHit {
    id: String,
    face: usize,
    local: DVec3,
    w: DVec3,
    anchor: u32,
}

fn pick_face(app: &IncaApp, idx: usize, cv: &CamView, rect: Rect, p: Pos2, objs: &[String]) -> Option<FaceHit> {
    let ray = cv.ray(p, rect);
    let mut best: Option<(f64, FaceHit)> = None;
    for id in objs {
        let Some(pm) = app.s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { continue };
        let wm = app.s.world_matrix(id);
        let lr = ray.transformed(&wm.inverse());
        let Some((t, fi)) = pm.raycast(&lr, app.vps[idx].backface) else { continue };
        let local = lr.at(t);
        let w = wm.transform_point3(local);
        let d = (w - ray.origin).length();
        let mut anchor = None;
        let mut best_px = f32::MAX;
        for &v in &pm.f[fi] {
            if let Some(q) = cv.project(wm.transform_point3(pm.v[v as usize]), rect) {
                let px = q.distance(p);
                if px < best_px {
                    best_px = px;
                    anchor = Some(v);
                }
            }
        }
        let Some(anchor) = anchor else { continue };
        if best.as_ref().is_none_or(|b| d < b.0) {
            best = Some((d, FaceHit { id: id.clone(), face: fi, local, w, anchor }));
        }
    }
    best.map(|b| b.1)
}

/// Nearest visible vertex within `r` px: (mesh, index, world position, distance).
fn pick_vertex(app: &mut IncaApp, idx: usize, cv: &CamView, rect: Rect, p: Pos2, objs: &[String], r: f32) -> Option<(String, u32, DVec3, f32)> {
    let pick = viewport::MeshPick::new(&app.s, &app.vps[idx], &mut app.mesh_cache);
    let mut best: Option<(String, u32, DVec3, f32)> = None;
    for id in objs {
        for (i, w) in world_verts_of(&app.s, id).into_iter().enumerate() {
            if let Some(s) = cv.project(w, rect) {
                let d = s.distance(p);
                if d < r && best.as_ref().is_none_or(|b| d < b.3) && pick.vertex(&app.s, cv, id, i as u32, &mut app.mesh_cache) {
                    best = Some((id.clone(), i as u32, w, d));
                }
            }
        }
    }
    best
}

/// Ray hit on meshes: (mesh, world point, world face normal).
fn hit_mesh(app: &IncaApp, cv: &CamView, rect: Rect, p: Pos2, objs: &[String]) -> Option<(String, DVec3, DVec3)> {
    let ray = cv.ray(p, rect);
    let mut best: Option<(f64, String, DVec3, DVec3)> = None;
    for id in objs {
        let Some(pm) = app.s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { continue };
        let wm = app.s.world_matrix(id);
        let lr = ray.transformed(&wm.inverse());
        if let Some((t, f)) = pm.raycast(&lr, false) {
            let w = wm.transform_point3(lr.at(t));
            let d = (w - ray.origin).length();
            if best.as_ref().is_none_or(|b| d < b.0) {
                let n = wm.inverse().transpose().transform_vector3(pm.face_normal(f)).normalize_or_zero();
                best = Some((d, id.clone(), w, n));
            }
        }
    }
    best.map(|b| (b.1, b.2, b.3))
}

fn line(p: &egui::Painter, cv: &CamView, rect: Rect, pts: &[DVec3], closed: bool, c: Color32) {
    let mut s: Vec<Pos2> = pts.iter().filter_map(|q| cv.project(*q, rect)).collect();
    if closed && s.len() > 2 {
        s.push(s[0]);
    }
    p.add(egui::Shape::line(s, Stroke::new(1.5, c)));
}

fn dot(p: &egui::Painter, cv: &CamView, rect: Rect, w: DVec3, size: f32, c: Color32) {
    if let Some(q) = cv.project(w, rect) {
        p.rect_filled(Rect::from_center_size(q, vec2(size, size)), 1.0, c);
    }
}

/// Maya's Multi-Cut snap marks: small black squares.
fn snap_mark(p: &egui::Painter, cv: &CamView, rect: Rect, w: DVec3, size: f32) {
    if let Some(q) = cv.project(w, rect) {
        p.rect_filled(Rect::from_center_size(q, vec2(size, size)), 0.0, Color32::BLACK);
    }
}

const GREEN: Color32 = Color32::from_rgb(92, 255, 92);
const YELLOW: Color32 = Color32::from_rgb(255, 225, 74);
const ORANGE: Color32 = Color32::from_rgb(255, 165, 40);
const BLUE: Color32 = Color32::from_rgb(63, 169, 255);
const RED: Color32 = Color32::from_rgb(255, 64, 64);

/// Viewport input for the active mesh tool.
pub fn interact(app: &mut IncaApp, idx: usize, resp: &egui::Response, cv: &CamView, painter: &egui::Painter) {
    let rect = app.vps[idx].rect;
    match app.tool {
        Tool::EdgeLoop => edge_loop(app, idx, resp, cv, rect, painter),
        Tool::MultiCut => multi_cut(app, idx, resp, cv, rect, painter),
        Tool::TargetWeld => target_weld(app, idx, resp, cv, rect, painter),
        Tool::Sculpt => sculpt(app, resp, cv, rect, painter),
        Tool::QuadDraw => quad_draw(app, resp, cv, rect, painter),
        _ => {}
    }
}

// ---------------------------------------------------------------- insert edge loop

fn loop_ts(n: u32, t: f64) -> Vec<f64> {
    if n <= 1 {
        vec![t]
    } else {
        (0..n).map(|i| (i + 1) as f64 / (n + 1) as f64).collect()
    }
}

/// The edge ring a loop through the hit edge crosses, as world-space edge ends in a consistent
/// direction (so one t lines up across the loop), and whether the ring closes.
fn ring_of(app: &IncaApp, h: &EdgeHit) -> (Vec<(DVec3, DVec3)>, bool) {
    let pm = &app.s.node(&h.id).unwrap().mesh().unwrap().mesh;
    let topo = pm.topo();
    let ring = edge_ring(pm, &topo, h.a, h.b);
    let closed = ring.len() > 2 && {
        let (f0, f1) = (ring[0], ring[ring.len() - 1]);
        let fa = topo.edge(f0.0, f0.1).map(|e| topo.edge_faces[e].clone()).unwrap_or_default();
        let fb = topo.edge(f1.0, f1.1).map(|e| topo.edge_faces[e].clone()).unwrap_or_default();
        fa.iter().any(|f| fb.contains(f))
    };
    (ring.iter().map(|&(a, b)| (h.w[a as usize], h.w[b as usize])).collect(), closed)
}

fn draw_loops(painter: &egui::Painter, cv: &CamView, rect: Rect, ring: &(Vec<(DVec3, DVec3)>, bool), ts: &[f64]) {
    for &t in ts {
        let pts: Vec<DVec3> = ring.0.iter().map(|(a, b)| a.lerp(*b, t)).collect();
        line(painter, cv, rect, &pts, ring.1, GREEN);
    }
}

fn edge_loop(app: &mut IncaApp, idx: usize, resp: &egui::Response, cv: &CamView, rect: Rect, painter: &egui::Painter) {
    let n = app.mesh_opts.edge_loops.max(1);
    if let Some((id, hi, sa, sb)) = app.mesh_tool.loop_drag.clone() {
        // drag slides a single new loop along the edge; the loop stays highlighted while it moves
        if n == 1 {
            if let Some(q) = resp.interact_pointer_pos() {
                let ab = sb - sa;
                let t = ((q - sa).dot(ab) / ab.length_sq().max(1.0)).clamp(0.01, 0.99) as f64;
                app.s.set_attr(&NodeRef::History(id.clone(), hi), "t", json!(t));
            }
        }
        if let Some(ring) = &app.mesh_tool.loop_ring {
            let t = app.s.history_node(&id, hi).and_then(|h| h.params.get("t").and_then(|v| v.as_f64())).unwrap_or(0.5);
            draw_loops(painter, cv, rect, ring, &loop_ts(n, t));
        }
        if resp.drag_stopped() || resp.clicked() || !resp.is_pointer_button_down_on() {
            let t = app.s.history_node(&id, hi).and_then(|h| h.params.get("t").and_then(|v| v.as_f64())).unwrap_or(0.5);
            app.echo(format!("polySplitRing -ch on -splitType 1 -weight {t:.4} -smoothingAngle 30 -fixQuads 1 -insertWithEdgeFlow 0;"));
            app.mesh_tool.loop_drag = None;
            app.mesh_tool.loop_ring = None;
        }
        return;
    }
    let objs = meshes(app);
    let hover = resp.hover_pos().and_then(|p| pick_edge(app, idx, cv, rect, p, &objs));
    if let Some(h) = &hover {
        draw_loops(painter, cv, rect, &ring_of(app, h), &loop_ts(n, h.t));
    }
    let press = resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary);
    // the edge under the press, not where the drag has already moved to
    let at_press = resp.interact_pointer_pos().map(|p| if resp.drag_started() { p - resp.drag_delta() } else { p }).and_then(|p| pick_edge(app, idx, cv, rect, p, &objs));
    if let (true, Some(h)) = (press, at_press) {
        app.s.checkpoint("insert edge loop");
        app.mesh_tool.loop_ring = Some(ring_of(app, &h));
        if app.s.comp_mode.is_none() && !app.s.sel.contains(&h.id) {
            app.select_names(std::slice::from_ref(&h.id), SelMode::Replace);
        }
        let mut p = Params::new();
        p.insert("edge".into(), json!([h.a, h.b]));
        p.insert("t".into(), json!(h.t.clamp(0.01, 0.99)));
        p.insert("multiple".into(), json!(n));
        app.s.apply_op(&h.id, "polySplitRing", p, app.history_on);
        let hi = app.s.node(&h.id).and_then(|n| n.mesh()).map(|m| m.history.len().saturating_sub(1)).unwrap_or(0);
        let (sa, sb) = (cv.project(h.w[h.a as usize], rect), cv.project(h.w[h.b as usize], rect));
        if let (Some(sa), Some(sb), true) = (sa, sb, app.history_on) {
            app.mesh_tool.loop_drag = Some((h.id.clone(), hi, sa, sb));
        }
        if resp.clicked() || !app.history_on {
            app.echo(format!("polySplitRing -ch on -splitType 1 -weight {:.4} -smoothingAngle 30 -fixQuads 1 -insertWithEdgeFlow 0;", h.t));
            app.mesh_tool.loop_drag = None;
            app.mesh_tool.loop_ring = None;
        }
    }
}

// ---------------------------------------------------------------- multi-cut

/// The cut point under `p`. Shift snaps the edge position to 10% steps. `mid` (the middle mouse
/// button, as in Maya) puts the point exactly halfway along the edge under the cursor, even next
/// to one of its vertices.
fn cut_pick(app: &mut IncaApp, idx: usize, cv: &CamView, rect: Rect, p: Pos2, shift: bool, mid: bool) -> Option<(String, CutPt)> {
    let objs = match &app.mesh_tool.cut_obj {
        Some(o) => vec![o.clone()],
        None => meshes(app),
    };
    if mid {
        if let Some(e) = pick_edge(app, idx, cv, rect, p, &objs) {
            return Some((e.id, CutPt { vertex: None, edge: Some((e.a, e.b, 0.5)), w: e.w[e.a as usize].lerp(e.w[e.b as usize], 0.5) , face: None, anchor: None }));
        }
    }
    let r = if app.snap_point { 40.0 } else { 8.0 };
    // the cut's own points snap like vertices (click the first one again to close a loop)
    let own = app.mesh_tool.cut.iter().filter(|c| c.vertex.is_none()).filter_map(|c| cv.project(c.w, rect).map(|s| (c.clone(), s.distance(p)))).filter(|x| x.1 < r).min_by(|a, b| a.1.total_cmp(&b.1));
    let v = pick_vertex(app, idx, cv, rect, p, &objs, r);
    if let Some((c, d)) = own {
        if v.as_ref().is_none_or(|v| d <= v.3) {
            return Some((app.mesh_tool.cut_obj.clone()?, c));
        }
    }
    if let Some((id, i, w, _)) = v {
        return Some((id, CutPt { vertex: Some(i), edge: None, w , face: None, anchor: None }));
    }
    if let Some(e) = pick_edge(app, idx, cv, rect, p, &objs) {
        let mut t = e.t;
        if shift {
            t = (t * 10.0).round() / 10.0;
        }
        let t = t.clamp(0.001, 0.999);
        return Some((e.id, CutPt { vertex: None, edge: Some((e.a, e.b, t)), w: e.w[e.a as usize].lerp(e.w[e.b as usize], t) , face: None, anchor: None }));
    }
    // Middle-click is edge-only; a normal click can start on a face.
    if !mid {
        if let Some(f) = pick_face(app, idx, cv, rect, p, &objs) {
            return Some((f.id, CutPt { vertex: None, edge: None, face: Some((f.face, f.local)), anchor: Some(f.anchor), w: f.w }));
        }
    }
    None
}

/// Nearest placed cut point after the first, for repositioning.
fn nearest_cut_point(app: &IncaApp, cv: &CamView, rect: Rect, p: Pos2) -> Option<usize> {
    app.mesh_tool
        .cut
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(i, c)| cv.project(c.w, rect).map(|q| (i, q.distance(p))))
        .filter(|&(_, d)| d < 9.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Pick where a repositioned cut point should go: vertex, edge, or face (face points keep their
/// nearest vertex as the preview anchor).
fn cut_target(app: &mut IncaApp, idx: usize, cv: &CamView, rect: Rect, p: Pos2, shift: bool) -> Option<(String, CutPt)> {
    let objs = match &app.mesh_tool.cut_obj {
        Some(o) => vec![o.clone()],
        None => meshes(app),
    };
    let r = if app.snap_point { 40.0 } else { 8.0 };
    if let Some((id, i, w, _)) = pick_vertex(app, idx, cv, rect, p, &objs, r) {
        return Some((id, CutPt { vertex: Some(i), edge: None, w, face: None, anchor: None }));
    }
    if let Some(e) = pick_edge(app, idx, cv, rect, p, &objs) {
        let mut t = e.t;
        if shift {
            t = (t * 10.0).round() / 10.0;
        }
        let t = t.clamp(0.001, 0.999);
        return Some((e.id, CutPt { vertex: None, edge: Some((e.a, e.b, t)), w: e.w[e.a as usize].lerp(e.w[e.b as usize], t), face: None, anchor: None }));
    }
    if let Some(f) = pick_face(app, idx, cv, rect, p, &objs) {
        return Some((f.id, CutPt { vertex: None, edge: None, face: Some((f.face, f.local)), anchor: Some(f.anchor), w: f.w }));
    }
    None
}

fn split_point(c: &CutPt) -> Option<SplitPoint> {
    if let Some(v) = c.vertex {
        Some(SplitPoint::Vertex(v))
    } else if let Some((fi, p)) = c.face {
        Some(SplitPoint::Face(fi, p))
    } else {
        c.edge.map(|(a, b, t)| SplitPoint::Edge(a, b, t))
    }
}

/// Parameters along two 2D segments where they cross; the caller checks both ranges.
fn segment_cross(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> Option<(f32, f32)> {
    let r = b - a;
    let s = d - c;
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-6 {
        return None;
    }
    let q = c - a;
    let t = (q.x * s.y - q.y * s.x) / den;
    let u = (q.x * r.y - q.y * r.x) / den;
    (t.is_finite() && u.is_finite()).then_some((t, u))
}

/// Parameter along the 3D segment `a -> b` closest to the camera ray.
fn ray_edge_t(origin: DVec3, dir: DVec3, a: DVec3, b: DVec3) -> f64 {
    let u = dir.normalize_or_zero();
    let v = b - a;
    let w0 = origin - a;
    let (aa, bb, cc) = (u.dot(u), u.dot(v), v.dot(v));
    let (dd, ee) = (u.dot(w0), v.dot(w0));
    let den = aa * cc - bb * bb;
    if den.abs() < 1e-12 {
        0.5
    } else {
        ((aa * ee - bb * dd) / den).clamp(0.0, 1.0)
    }
}

fn push_cut(out: &mut Vec<SplitPoint>, p: SplitPoint) {
    let same = out.last().is_some_and(|last| match (last, &p) {
        (SplitPoint::Vertex(a), SplitPoint::Vertex(b)) => a == b,
        (SplitPoint::Edge(a, b, t), SplitPoint::Edge(c, d, u)) => (*a).min(*b) == (*c).min(*d) && (*a).max(*b) == (*c).max(*d) && (*t - *u).abs() < 1e-4,
        _ => false,
    });
    if !same {
        out.push(p);
    }
}

/// Is `q` inside the projected polygon of face `f`?
fn face_contains_2d(cv: &CamView, rect: Rect, wm: DMat4, pm: &PolyMesh, f: usize, q: Pos2) -> bool {
    let pts: Vec<Pos2> = pm.f[f].iter().filter_map(|&v| cv.project(wm.transform_point3(pm.v[v as usize]), rect)).collect();
    if pts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (pi, pj) = (pts[i], pts[j]);
        if (pi.y > q.y) != (pj.y > q.y) && q.x < (pj.x - pi.x) * (q.y - pi.y) / (pj.y - pi.y) + pi.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// How much face `f` points at the camera; larger is more front-facing.
fn face_front(cv: &CamView, wm: DMat4, pm: &PolyMesh, f: usize) -> f64 {
    let n = wm.inverse().transpose().transform_vector3(pm.face_normal(f)).normalize_or_zero();
    let c = wm.transform_point3(pm.face_center(f));
    let view = if cv.ortho { -cv.fwd } else { (cv.eye - c).normalize_or_zero() };
    n.dot(view)
}

/// The face the cut enters from a start vertex/edge, using the first bit of the screen line.
fn cut_start_face(pm: &PolyMesh, topo: &Topo, wm: DMat4, cv: &CamView, rect: Rect, start: &CutPt, q0: Pos2, q1: Pos2) -> Option<usize> {
    let sample = q0 + (q1 - q0) * 0.01;
    let mut cands: Vec<usize> = match (start.vertex, start.edge) {
        (Some(v), _) => topo.vert_faces.get(v as usize).cloned().unwrap_or_default(),
        (_, Some((a, b, _))) => topo.edge(a, b).map(|e| topo.edge_faces[e].clone()).unwrap_or_default(),
        _ => return None,
    };
    cands.retain(|&f| f < pm.f.len() && face_contains_2d(cv, rect, wm, pm, f, sample));
    if cands.is_empty() {
        return None;
    }
    cands.sort_by(|&a, &b| face_front(cv, wm, pm, b).total_cmp(&face_front(cv, wm, pm, a)));
    cands.first().copied()
}

/// Walk the connected faces under the screen line and return the intermediate edge points. This
/// follows shared edges, so the cut stays on the surface instead of splitting back faces that
/// merely project across the front.
fn surface_mids(pm: &PolyMesh, topo: &Topo, wm: DMat4, cv: &CamView, rect: Rect, start: &CutPt, end: &CutPt) -> Option<Vec<SplitPoint>> {
    let q0 = cv.project(start.w, rect)?;
    let q1 = cv.project(end.w, rect)?;
    let dir = q1 - q0;
    if dir.length() < 1.0 {
        return Some(vec![]);
    }
    let mut f = cut_start_face(pm, topo, wm, cv, rect, start, q0, q1)?;
    let end_edge = end.edge.map(|(a, b, _)| (a.min(b), a.max(b)));
    let end_vertex = end.vertex;
    let mut mids = vec![];
    let mut entry = 0.0f32;
    for _ in 0..128 {
        let face = &pm.f[f];
        let mut best: Option<(f32, u32, u32)> = None;
        for i in 0..face.len() {
            let (a, b) = (face[i], face[(i + 1) % face.len()]);
            let (Some(sa), Some(sb)) = (
                cv.project(wm.transform_point3(pm.v[a as usize]), rect),
                cv.project(wm.transform_point3(pm.v[b as usize]), rect),
            ) else {
                continue;
            };
            let Some((t, u)) = segment_cross(q0, q1, sa, sb) else { continue };
            if t <= entry + 1e-4 || t > 1.0 + 1e-4 {
                continue;
            }
            if u <= 0.001 || u >= 0.999 {
                // The line passes through a boundary vertex of this face.
                let v = if u <= 0.001 { a } else { b };
                if end_vertex == Some(v) {
                    return Some(mids);
                }
                if best.is_none_or(|b| t < b.0) {
                    best = Some((t, v, v));
                }
                continue;
            }
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, a, b));
            }
        }
        let Some((t, a, b)) = best else { break };
        if end_edge == Some((a.min(b), a.max(b))) {
            break;
        }
        if a == b {
            // vertex crossing: step into the adjacent face without inserting an edge point
            let Some(next) = topo.vert_faces.get(a as usize).and_then(|fs| fs.iter().copied().find(|&nf| nf != f)) else {
                break;
            };
            f = next;
            entry = t;
            continue;
        }
        let ray = cv.ray(q0 + dir * t, rect);
        let tt = ray_edge_t(ray.origin, ray.dir, wm.transform_point3(pm.v[a as usize]), wm.transform_point3(pm.v[b as usize]));
        if !(0.001..0.999).contains(&tt) {
            break;
        }
        mids.push(SplitPoint::Edge(a, b, tt));
        let Some(e) = topo.edge(a, b) else { break };
        let Some(&next) = topo.edge_faces[e].iter().find(|&&nf| nf != f) else { break };
        f = next;
        entry = t;
    }
    Some(mids)
}

/// Maya creates the intermediate points where the cut line crosses other edges. Walk the surface
/// between every pair of clicked points and insert the edges it crosses, following connected
/// faces rather than cutting through the mesh.
fn expanded_cut(app: &mut IncaApp, id: &str, pts: &[CutPt], cams: &[CamView], cv: &CamView, vp: &viewport::Viewport) -> Vec<SplitPoint> {
    let Some(pm) = app.s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { return vec![] };
    // A stale point (for example after undo restored an older topology) must never index the mesh.
    let vlen = pm.v.len();
    let pts: Vec<CutPt> = pts
        .iter()
        .filter(|c| match (c.vertex, c.edge) {
            (Some(v), _) => (v as usize) < vlen,
            (_, Some((a, b, t))) => (a as usize) < vlen && (b as usize) < vlen && a != b && t.is_finite(),
            _ => false,
        })
        .cloned()
        .collect();
    if pts.len() < 2 {
        return pts.iter().filter_map(split_point).collect();
    }
    let wm = app.s.world_matrix(id);
    let Some(topo) = app.mesh_cache.topo(&app.s, id) else { return pts.iter().filter_map(split_point).collect() };
    let mut out = vec![];
    if let Some(p) = pts.first().and_then(split_point) {
        push_cut(&mut out, p);
    }
    for (i, pair) in pts.windows(2).enumerate() {
        let pair_cv = cams.get(i + 1).unwrap_or(cv);
        if let Some(mids) = surface_mids(pm, &topo, wm, pair_cv, vp.rect, &pair[0], &pair[1]) {
            for p in mids {
                push_cut(&mut out, p);
            }
        }
        if let Some(p) = split_point(&pair[1]) {
            push_cut(&mut out, p);
        }
    }
    out
}

pub fn multi_cut_complete(app: &mut IncaApp) {
    app.mesh_tool.mid_cut = None;
    app.mesh_tool.cut_drag = None;
    let pts = std::mem::take(&mut app.mesh_tool.cut);
    let obj = app.mesh_tool.cut_obj.take();
    let vp = app.mesh_tool.cut_vp.take();
    let cam = app.mesh_tool.cut_cam.take();
    let cams = std::mem::take(&mut app.mesh_tool.cut_cams);
    let Some(id) = obj.filter(|_| !pts.is_empty()) else { return };
    app.s.checkpoint("multi-cut");
    let has_face = pts.iter().any(|c| c.face.is_some());
    let points = if has_face {
        pts.iter().filter_map(split_point).collect()
    } else {
        match vp.filter(|i| *i < app.vps.len()) {
            Some(i) => {
                let vpv = app.vps[i].clone();
                let cv = cam.unwrap_or_else(|| viewport::cam_view(&app.s, &vpv.camera, [vpv.rect.width(), vpv.rect.height()]));
                expanded_cut(app, &id, &pts, &cams, &cv, &vpv)
            }
            None => pts.iter().filter_map(split_point).collect(),
        }
    };
    let vlen = app.s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.v.len()).unwrap_or(0);
    let flen = app.s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.f.len()).unwrap_or(0);
    let points: Vec<SplitPoint> = points
        .into_iter()
        .filter(|p| match *p {
            SplitPoint::Vertex(v) => (v as usize) < vlen,
            SplitPoint::Edge(a, b, t) => (a as usize) < vlen && (b as usize) < vlen && a != b && t.is_finite(),
            SplitPoint::Face(fi, p) => fi < flen && p.is_finite(),
        })
        .collect();
    if points.is_empty() {
        return;
    }
    let list: Vec<serde_json::Value> = points
        .iter()
        .map(|p| match *p {
            SplitPoint::Vertex(v) => json!({ "vertex": v }),
            SplitPoint::Edge(a, b, t) => json!({ "edge": [a, b], "t": t }),
            SplitPoint::Face(fi, p) => json!({ "face": fi, "point": [p.x, p.y, p.z] }),
        })
        .collect();
    let mut p = Params::new();
    p.insert("points".into(), serde_json::Value::Array(list));
    app.s.apply_op(&id, "polySplit", p, app.history_on);
    app.s.comp.remove(&id);
    app.echo("polySplit -ch 1 -sma 180 -ep ...;");
}

fn multi_cut(app: &mut IncaApp, idx: usize, resp: &egui::Response, cv: &CamView, rect: Rect, painter: &egui::Painter) {
    // a deleted mesh must not leave its cut preview floating
    if app.mesh_tool.cut_obj.as_ref().is_some_and(|id| app.s.node(id).is_none_or(|n| n.mesh().is_none())) {
        app.mesh_tool.clear_cut();
    }
    let mods = resp.ctx.input(|i| i.modifiers);
    let ctrl = mods.ctrl || mods.command;
    let pointer = resp.ctx.input(|i| i.pointer.clone());
    let lmb_down = pointer.button_down(egui::PointerButton::Primary);
    let lmb_pressed = pointer.button_pressed(egui::PointerButton::Primary);
    let lmb_released = pointer.button_released(egui::PointerButton::Primary);
    let mid_down = pointer.button_down(egui::PointerButton::Middle);
    let mid_pressed = pointer.button_pressed(egui::PointerButton::Middle);
    let mid_released = pointer.button_released(egui::PointerButton::Middle);
    if !mid_down && !mid_pressed && !mid_released {
        app.mesh_tool.mid_cut = None;
    }
    if !lmb_down && !lmb_released {
        if app.mesh_tool.cut_drag.as_ref().is_some_and(|d| d.vp == idx) {
            app.mesh_tool.cut_drag = None;
        }
        if app.mesh_tool.move_drag.is_some_and(|(v, _)| v == idx) {
            app.mesh_tool.move_drag = None;
        }
    }
    if app.active_vp == idx {
        app.mesh_tool.hover_point = resp.hover_pos().and_then(|p| nearest_cut_point(app, cv, rect, p));
    }
    // LMB press on a placed point (after the first) starts moving it; otherwise an edge starts
    // sliding a new point. Release places it. Shift starts at the midpoint and snaps to 10%.
    if lmb_pressed && resp.hovered() && !ctrl {
        if let Some(i) = app.mesh_tool.hover_point {
            app.mesh_tool.move_drag = Some((idx, i));
            app.mesh_tool.cut_drag = None;
        } else if let Some(p) = resp.hover_pos() {
            if let Some((id, c)) = cut_pick(app, idx, cv, rect, p, false, false) {
                if let Some((a, b, t)) = c.edge {
                    if !app.mesh_tool.cut_obj.as_ref().is_some_and(|o| *o != id) {
                        app.mesh_tool.cut_drag = Some(CutDrag { vp: idx, id, a, b, t: if mods.shift { 0.5 } else { t }, start: p });
                    }
                }
            }
        }
    }
    if lmb_down {
        if let Some((v, i)) = app.mesh_tool.move_drag {
            if v == idx {
                if let Some(p) = resp.hover_pos() {
                    if let Some((id, c)) = cut_target(app, idx, cv, rect, p, mods.shift) {
                        if !app.mesh_tool.cut_obj.as_ref().is_some_and(|o| *o != id) {
                            if let Some(slot) = app.mesh_tool.cut.get_mut(i) {
                                *slot = c;
                            }
                        }
                    }
                }
            }
        }
        if let Some(d) = app.mesh_tool.cut_drag.as_mut() {
            if d.vp == idx {
                let w = world_verts_of(&app.s, &d.id);
                if let (Some(q), Some(sa), Some(sb)) = (resp.hover_pos(), cv.project(w[d.a as usize], rect), cv.project(w[d.b as usize], rect)) {
                    if q.distance(d.start) >= 2.0 {
                        let ab = sb - sa;
                        let t = (((q - sa).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0)) as f64;
                        d.t = if mods.shift { ((t * 10.0).round() / 10.0).clamp(0.1, 0.9) } else { t.clamp(0.001, 0.999) };
                    }
                }
            }
        }
    }
    // MMB samples the edge midpoint under the cursor. Re-sample while the button is held, so the
    // cut follows the edge under the cursor, and commit on release even if the pointer moved
    // (a plain egui click is lost as soon as the MMB turns into a drag).
    if mid_pressed && resp.hovered() {
        app.mesh_tool.mid_cut = None;
    }
    if mid_down && resp.hovered() {
        if let Some((id, c)) = resp.hover_pos().and_then(|p| cut_pick(app, idx, cv, rect, p, false, true)) {
            app.mesh_tool.mid_cut = Some((idx, id, c));
        }
    }
    let drag = app.mesh_tool.cut_drag.as_ref().filter(|d| d.vp == idx).cloned();
    let drag_pt = drag.as_ref().and_then(|d| {
        let w = world_verts_of(&app.s, &d.id);
        let (a, b) = (d.a as usize, d.b as usize);
        (a < w.len() && b < w.len()).then(|| (d.id.clone(), CutPt { vertex: None, edge: Some((d.a, d.b, d.t)), w: w[a].lerp(w[b], d.t) , face: None, anchor: None }))
    });
    // while the middle button is down, preview the midpoint (or the last sampled one off the mesh)
    let point_hover = app
        .mesh_tool
        .move_drag
        .map(|(_, i)| i)
        .or(app.mesh_tool.hover_point)
        .and_then(|i| app.mesh_tool.cut_obj.clone().and_then(|id| app.mesh_tool.cut.get(i).cloned().map(|c| (id, c))));
    let mut hover = if point_hover.is_some() {
        None
    } else {
        drag_pt.clone().or_else(|| {
            resp.hover_pos().and_then(|p| cut_pick(app, idx, cv, rect, p, mods.shift, mid_down)).or_else(|| {
                mid_down.then(|| app.mesh_tool.mid_cut.as_ref().filter(|(v, _, _)| *v == idx).map(|(_, id, c)| (id.clone(), c.clone()))).flatten()
            })
        })
    };
    // Shift without dragging previews the edge midpoint; Shift-dragging shows the snap marks.
    if mods.shift && drag.is_none() && point_hover.is_none() {
        if let Some((id, c)) = &hover {
            if let Some((a, b, _)) = c.edge {
                let w = world_verts_of(&app.s, id);
                hover = Some((id.clone(), CutPt { vertex: None, edge: Some((a, b, 0.5)), w: w[a as usize].lerp(w[b as usize], 0.5) , face: None, anchor: None }));
            }
        }
    }
    // Multi-Cut's red preselection follows its own pick, so the highlighted component is always
    // the one a click would cut (vertex snap included), independent of the component mode.
    if point_hover.is_none() {
        if let Some((id, c)) = &hover {
            if let Some(anchor) = c.anchor {
                let w = world_verts_of(&app.s, id);
                if let Some(a) = w.get(anchor as usize).copied() {
                    line(painter, cv, rect, &[a, c.w], false, ORANGE);
                    if let Some(q) = cv.project(a, rect) {
                        painter.rect_filled(Rect::from_center_size(q, vec2(7.0, 7.0)), 0.0, Color32::from_rgb(255, 225, 74));
                    }
                }
                // Maya's face ghost point is light grey, with the yellow anchored vertex beside it.
                if let Some(q) = cv.project(c.w, rect) {
                    painter.rect_filled(Rect::from_center_size(q, vec2(6.0, 6.0)), 0.0, Color32::from_gray(205));
                }
            } else {
                let comp = match (c.vertex, c.edge) {
                    (Some(v), _) => Some(viewport::Comp::V(v)),
                    (_, Some((a, b, _))) => Some(viewport::Comp::E(a, b)),
                    _ => None,
                };
                if let Some(comp) = comp {
                    viewport::draw_component_overlay(&app.s, &app.vps[idx], cv, painter, Some((id.as_str(), &comp, false)));
                }
            }
        }
    }
    // Ctrl: the edge loop a click would insert (Ctrl+middle-click: through the edge's midpoint)
    if ctrl {
        let objs = meshes(app);
        let sampled = app.mesh_tool.mid_cut.as_ref().filter(|(v, _, _)| *v == idx).and_then(|(_, id, c)| c.edge.map(|(a, b, t)| (id.clone(), a, b, t)));
        let hovered = resp.hover_pos().and_then(|p| pick_edge(app, idx, cv, rect, p, &objs));
        // Between edges the hover is empty, but the release still cuts the last sampled edge:
        // keep previewing that edge's loop so the preview matches what MMB will do.
        let h = hovered.or_else(|| sampled.map(|(id, a, b, t)| EdgeHit { id: id.clone(), a, b, t, w: world_verts_of(&app.s, &id), d: 0.0 }));
        if let Some(h) = h {
            let t = if mid_down { 0.5 } else { h.t.clamp(0.001, 0.999) };
            draw_loops(painter, cv, rect, &ring_of(app, &h), &[t]);
        }
    }
    let mut pts: Vec<DVec3> = app.mesh_tool.cut.iter().map(|c| c.w).collect();
    if let Some((_, h)) = hover.as_ref().filter(|_| !ctrl) {
        pts.push(h.w);
    }
    // Shift snap presentation: the midpoint dot before dragging, the full step marks while
    // dragging the point along the edge.
    if mods.shift && drag.is_none() {
        if let Some((id, c)) = &hover {
            if let Some((a, b, _)) = c.edge {
                let w = world_verts_of(&app.s, id);
                snap_mark(painter, cv, rect, w[a as usize].lerp(w[b as usize], 0.5), 6.0);
            }
        }
    }
    if let Some(d) = &drag {
        let w = world_verts_of(&app.s, &d.id);
        if mods.shift {
            for i in 1..10 {
                snap_mark(painter, cv, rect, w[d.a as usize].lerp(w[d.b as usize], i as f64 / 10.0), 4.0);
            }
        }
    }
    line(painter, cv, rect, &pts, false, YELLOW);
    for c in &app.mesh_tool.cut {
        dot(painter, cv, rect, c.w, 6.0, BLUE);
    }
    if let Some((_, c)) = &point_hover {
        if let Some(q) = cv.project(c.w, rect) {
            painter.rect_filled(Rect::from_center_size(q, vec2(8.0, 8.0)), 0.0, RED);
        }
    }
    if let Some((_, c)) = &drag_pt {
        dot(painter, cv, rect, c.w, 7.0, BLUE);
        if let (Some((_, _, t)), Some(q)) = (c.edge, cv.project(c.w, rect)) {
            let txt = format!("{:.0}%", t * 100.0);
            let pos = q + vec2(8.0, -8.0);
            painter.text(pos + vec2(1.0, 1.0), Align2::LEFT_BOTTOM, &txt, FontId::monospace(11.0), Color32::BLACK);
            painter.text(pos, Align2::LEFT_BOTTOM, &txt, FontId::monospace(11.0), YELLOW);
        }
    }
    if resp.clicked_by(egui::PointerButton::Secondary) {
        multi_cut_complete(app);
        return;
    }
    // MMB commit on release, at the last edge sampled while it was held
    if mid_released && app.mesh_tool.mid_cut.as_ref().is_some_and(|(v, _, _)| *v == idx) {
        if let Some((_, id, c)) = app.mesh_tool.mid_cut.take() {
            if ctrl {
                if let Some((a, b, _)) = c.edge {
                    app.s.checkpoint("insert edge loop");
                    let mut p = Params::new();
                    p.insert("edge".into(), json!([a, b]));
                    p.insert("t".into(), json!(0.5));
                    p.insert("multiple".into(), json!(1));
                    app.s.apply_op(&id, "polySplitRing", p, app.history_on);
                }
            } else if !app.mesh_tool.cut_obj.as_ref().is_some_and(|o| *o != id) {
                if app.mesh_tool.cut.is_empty() {
                    app.mesh_tool.cut_vp = Some(idx);
                }
                app.mesh_tool.cut_obj = Some(id);
                app.mesh_tool.cut_cam = Some(*cv);
                app.mesh_tool.cut_cams.push(*cv);
                app.mesh_tool.cut.push(c);
            }
            return;
        }
    }
    // LMB release finishes moving an existing point.
    if lmb_released && app.mesh_tool.move_drag.is_some_and(|(v, _)| v == idx) {
        if let Some((_, i)) = app.mesh_tool.move_drag.take() {
            if let Some(c) = app.mesh_tool.cut_cams.get_mut(i) {
                *c = *cv;
            }
        }
        app.mesh_tool.hover_point = None;
        app.mesh_tool.cut_cam = Some(*cv);
        return;
    }
    // LMB release places the point that was being slid along the edge.
    if lmb_released && app.mesh_tool.cut_drag.as_ref().is_some_and(|d| d.vp == idx) {
        if let Some(d) = app.mesh_tool.cut_drag.take() {
            let w = world_verts_of(&app.s, &d.id);
            if let (Some(wa), Some(wb)) = (w.get(d.a as usize), w.get(d.b as usize)) {
                let c = CutPt { vertex: None, edge: Some((d.a, d.b, d.t)), w: wa.lerp(*wb, d.t) , face: None, anchor: None };
                if !app.mesh_tool.cut_obj.as_ref().is_some_and(|o| *o != d.id) {
                    if app.mesh_tool.cut.is_empty() {
                        app.mesh_tool.cut_vp = Some(idx);
                    }
                    app.mesh_tool.cut_obj = Some(d.id);
                    app.mesh_tool.cut_cam = Some(*cv);
                    app.mesh_tool.cut_cams.push(*cv);
                    app.mesh_tool.cut.push(c);
                }
            }
            return;
        }
    }
    if !resp.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    let Some((id, c)) = hover else {
        // a click off the mesh completes the cut
        multi_cut_complete(app);
        return;
    };
    if mods.ctrl || mods.command {
        // Ctrl+click on an edge inserts an edge loop
        if let Some((a, b, t)) = c.edge {
            app.s.checkpoint("insert edge loop");
            let mut p = Params::new();
            p.insert("edge".into(), json!([a, b]));
            p.insert("t".into(), json!(t));
            p.insert("multiple".into(), json!(1));
            app.s.apply_op(&id, "polySplitRing", p, app.history_on);
        }
        return;
    }
    if app.mesh_tool.cut_obj.as_ref().is_some_and(|o| *o != id) {
        return;
    }
    if app.mesh_tool.cut.is_empty() {
        app.mesh_tool.cut_vp = Some(idx);
    }
    app.mesh_tool.cut_obj = Some(id.clone());
    app.mesh_tool.cut_cam = Some(*cv);
    if c.face.is_some() {
        if let Some(anchor) = c.anchor {
            if app.mesh_tool.cut.last().is_none_or(|l| l.vertex != Some(anchor)) {
                let w = world_verts_of(&app.s, &id);
                if let Some(a) = w.get(anchor as usize).copied() {
                    app.mesh_tool.cut.push(CutPt { vertex: Some(anchor), edge: None, w: a, face: None, anchor: None });
                    app.mesh_tool.cut_cams.push(*cv);
                }
            }
        }
    }
    app.mesh_tool.cut.push(c);
    app.mesh_tool.cut_cams.push(*cv);
}

// ---------------------------------------------------------------- target weld

fn target_weld(app: &mut IncaApp, idx: usize, resp: &egui::Response, cv: &CamView, rect: Rect, painter: &egui::Painter) {
    let objs = meshes(app);
    if let Some((id, src)) = app.mesh_tool.weld.clone() {
        let ws = world_verts(app, &id);
        let q = resp.interact_pointer_pos().or(resp.hover_pos());
        let tgt = q.and_then(|q| pick_vertex(app, idx, cv, rect, q, std::slice::from_ref(&id), 10.0)).filter(|t| t.1 != src);
        let end = tgt.as_ref().map(|t| t.2).unwrap_or(ws[src as usize]);
        line(painter, cv, rect, &[ws[src as usize], end], false, YELLOW);
        dot(painter, cv, rect, ws[src as usize], 7.0, BLUE);
        if let Some(t) = &tgt {
            dot(painter, cv, rect, t.2, 9.0, RED);
        }
        if resp.drag_stopped() || !resp.is_pointer_button_down_on() {
            app.mesh_tool.weld = None;
            if let Some((_, ti, _, _)) = tgt {
                let pm = app.s.node(&id).unwrap().mesh().unwrap().mesh.clone();
                let d = pm.v[ti as usize] - pm.v[src as usize];
                app.s.checkpoint("target weld");
                let mut p = Params::new();
                p.insert("verts".into(), json!([src]));
                p.insert("deltas".into(), json!([d.to_array()]));
                app.s.apply_op(&id, "polyTweak", p, app.history_on);
                let mut p = Params::new();
                p.insert("verts".into(), json!([src, ti]));
                p.insert("threshold".into(), json!(1e-4));
                app.s.apply_op(&id, "polyMergeVert", p, app.history_on);
                app.s.comp.remove(&id);
                let n = app.s.name_of(&id);
                app.echo(format!("polyMergeVertex -d 0.0001 -am 1 -ch 1 {n}.vtx[{src}] {n}.vtx[{ti}];"));
            }
        }
        return;
    }
    let hover = resp.hover_pos().and_then(|p| pick_vertex(app, idx, cv, rect, p, &objs, 10.0));
    if let Some(h) = &hover {
        dot(painter, cv, rect, h.2, 9.0, RED);
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some((id, i, _, _)) = pick_vertex(app, idx, cv, rect, p - resp.drag_delta(), &objs, 10.0) {
                app.mesh_tool.weld = Some((id, i));
            }
        }
    }
}

// ---------------------------------------------------------------- sculpt

fn ring(painter: &egui::Painter, cv: &CamView, rect: Rect, c: DVec3, n: DVec3, r: f64) {
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let pts: Vec<DVec3> = (0..=32).map(|i| {
        let a = std::f64::consts::TAU * i as f64 / 32.0;
        c + (u * a.cos() + v * a.sin()) * r
    }).collect();
    line(painter, cv, rect, &pts, false, RED);
}

fn sculpt_targets(app: &IncaApp) -> Vec<String> {
    app.s.all_dag(false).into_iter().filter(|i| app.s.is_visible(i) && app.s.node(i).is_some_and(|n| n.mesh().is_some())).collect()
}

fn sculpt(app: &mut IncaApp, resp: &egui::Response, cv: &CamView, rect: Rect, painter: &egui::Painter) {
    let size = app.mesh_opts.size;
    let objs = sculpt_targets(app);
    let q = resp.interact_pointer_pos().or(resp.hover_pos());
    let hit = q.and_then(|q| hit_mesh(app, cv, rect, q, &objs));
    if let Some((_, w, n)) = &hit {
        ring(painter, cv, rect, *w, *n, size);
    }
    let mods = resp.ctx.input(|i| i.modifiers);
    if resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary) {
        if let Some((id, w, _)) = &hit {
            if let Some(m) = app.s.nodes.get_mut(id).and_then(|n| n.mesh_mut()) {
                m.smooth_level = 0;
            }
            let orig = app.s.node(id).unwrap().mesh().unwrap().mesh.clone();
            let brush = if mods.shift { 1 } else { app.mesh_opts.brush };
            let grab = (BRUSHES[brush] == "grab").then(|| (*w, vec![]));
            app.mesh_tool.sculpt = Some(SculptStroke { id: id.clone(), orig, touched: HashSet::new(), brush, sign: if mods.ctrl || mods.command { -1.0 } else { 1.0 }, grab });
        }
    }
    let Some(st) = app.mesh_tool.sculpt.as_mut() else { return };
    let id = st.id.clone();
    let wm = app.s.world_matrix(&id);
    let inv = wm.inverse();
    if BRUSHES[st.brush] == "grab" {
        if let (Some(q), Some((start, _))) = (q, st.grab.clone()) {
            // drag in the view plane through the grab point
            let ray = cv.ray(q, rect);
            let nrm = if cv.ortho { -cv.fwd } else { (cv.eye - start).normalize() };
            let denom = ray.dir.dot(nrm);
            if denom.abs() > 1e-9 {
                let t = (start - ray.origin).dot(nrm) / denom;
                let delta = ray.at(t) - start;
                stroke(app, &wm, &inv, start, nrm, Some(delta));
            }
        }
    } else if let Some((hid, w, n)) = &hit {
        if *hid == id && (resp.dragged() || resp.drag_started() || resp.clicked()) {
            stroke(app, &wm, &inv, *w, *n, None);
        }
    }
    if resp.drag_stopped() || resp.clicked() || !resp.is_pointer_button_down_on() {
        finish_sculpt(app);
    }
}

fn stroke(app: &mut IncaApp, wm: &DMat4, inv: &DMat4, center: DVec3, normal: DVec3, grab_delta: Option<DVec3>) {
    let (size, k, mirror) = (app.mesh_opts.size, app.mesh_opts.strength, app.mesh_opts.mirror);
    let Some(st) = app.mesh_tool.sculpt.as_mut() else { return };
    let Some(m) = app.s.nodes.get_mut(&st.id).and_then(|n| n.mesh_mut()) else { return };
    let pm = &mut m.mesh;
    let c = inv.transform_point3(center);
    let scale = wm.transform_vector3(DVec3::X).length().max(1e-6);
    let rl = size / scale;
    let nrm = inv.transpose().inverse().transform_vector3(normal).normalize_or_zero();
    let nrm = if nrm == DVec3::ZERO { inv.transform_vector3(normal).normalize_or(DVec3::Y) } else { nrm };
    let fall = |d: f64| {
        let t = (1.0 - d / rl).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let brush = BRUSHES[st.brush];
    let ids: Vec<(u32, f64)> = pm.v.iter().enumerate().filter_map(|(i, p)| {
        let d = (*p - c).length();
        (d < rl).then_some((i as u32, d))
    }).collect();
    if brush == "grab" {
        let Some((_, set)) = st.grab.as_mut() else { return };
        if set.is_empty() {
            let c0 = inv.transform_point3(center);
            *set = st.orig.v.iter().enumerate().filter_map(|(i, p)| {
                let d = (*p - c0).length();
                (d < rl).then(|| (i as u32, fall(d)))
            }).collect();
        }
        let ld = inv.transform_vector3(grab_delta.unwrap_or_default());
        for &(i, w) in set.iter() {
            pm.v[i as usize] = st.orig.v[i as usize] + ld * w;
            st.touched.insert(i);
        }
    } else {
        let topo = pm.topo();
        let nbr = |i: u32| -> Vec<u32> {
            topo.vert_edges[i as usize].iter().map(|&e| {
                let (a, b) = topo.edges[e];
                if a == i { b } else { a }
            }).collect()
        };
        let vnormal = |pm: &PolyMesh, i: u32| -> DVec3 {
            topo.vert_faces[i as usize].iter().map(|&f| pm.face_normal(f)).sum::<DVec3>().normalize_or_zero()
        };
        let plane = if brush == "flatten" && !ids.is_empty() { ids.iter().map(|(i, _)| pm.v[*i as usize]).sum::<DVec3>() / ids.len() as f64 } else { DVec3::ZERO };
        let snapshot = pm.v.clone();
        for &(i, d) in &ids {
            let w = fall(d) * k;
            let v = snapshot[i as usize];
            let nv = match brush {
                "sculpt" => v + nrm * (st.sign * w * rl * 0.15),
                "inflate" => v + vnormal(pm, i) * (st.sign * w * rl * 0.12),
                "smooth" | "relax" => {
                    let ns = nbr(i);
                    if ns.is_empty() {
                        continue;
                    }
                    let mut avg = ns.iter().map(|&j| snapshot[j as usize]).sum::<DVec3>() / ns.len() as f64;
                    if brush == "relax" {
                        let n = vnormal(pm, i);
                        avg -= n * (avg - v).dot(n);
                    }
                    v.lerp(avg, (w * 2.0).min(1.0))
                }
                "pinch" => v.lerp(c, st.sign * w * 0.1),
                "flatten" => v - nrm * ((v - plane).dot(nrm) * w),
                _ => v,
            };
            pm.v[i as usize] = nv;
            st.touched.insert(i);
        }
    }
    if mirror {
        // mirror across local X: the vertex at (-x, y, z) in the original mesh follows
        let touched: Vec<u32> = st.touched.iter().copied().collect();
        for i in touched {
            let o = st.orig.v[i as usize];
            let target = DVec3::new(-o.x, o.y, o.z);
            if let Some((j, _)) = st.orig.v.iter().enumerate().map(|(j, q)| (j, (*q - target).length())).filter(|x| x.1 < 1e-3).min_by(|a, b| a.1.total_cmp(&b.1)) {
                if j as u32 != i {
                    let p = pm.v[i as usize];
                    pm.v[j] = DVec3::new(-p.x, p.y, p.z);
                    st.touched.insert(j as u32);
                }
            }
        }
    }
    if let Some(n) = app.s.nodes.get_mut(&st.id) {
        n.rev += 1;
    }
    app.s.rev += 1;
}

/// Commit the stroke as one polyTweak (one undo step), like a component move.
fn finish_sculpt(app: &mut IncaApp) {
    let Some(st) = app.mesh_tool.sculpt.take() else { return };
    let cur = app.s.node(&st.id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()).unwrap_or_default();
    let mut verts: Vec<u32> = st.touched.iter().copied().collect();
    verts.sort();
    let deltas: Vec<[f64; 3]> = verts.iter().map(|&v| (cur.v[v as usize] - st.orig.v[v as usize]).to_array()).collect();
    if let Some(m) = app.s.nodes.get_mut(&st.id).and_then(|n| n.mesh_mut()) {
        m.mesh = st.orig.clone();
    }
    if verts.is_empty() {
        return;
    }
    app.s.checkpoint("sculpt");
    let mut p = Params::new();
    p.insert("verts".into(), json!(verts));
    p.insert("deltas".into(), json!(deltas));
    app.s.apply_op(&st.id, "polyTweak", p, true);
    app.echo(format!("// sculpt ({}) {} vertices", BRUSHES[st.brush], verts.len()));
}

// ---------------------------------------------------------------- quad draw

#[derive(Clone, Copy, PartialEq)]
enum QuadPick {
    Dot(usize),
    Vert(u32),
}

fn quad_surfaces(app: &IncaApp) -> Vec<String> {
    if let Some(l) = app.live_surface.as_ref().filter(|l| app.s.node(l).is_some()) {
        return vec![l.clone()];
    }
    let t = app.mesh_tool.quad_target.clone();
    sculpt_targets(app).into_iter().filter(|i| Some(i) != t.as_ref()).collect()
}

/// A point on the live surface, nudged off it so new faces don't z-fight.
fn surface_point(app: &IncaApp, cv: &CamView, rect: Rect, p: Pos2) -> Option<DVec3> {
    let (id, w, n) = hit_mesh(app, cv, rect, p, &quad_surfaces(app))?;
    let off = app.s.world_bbox(&id).size().length() * 0.003;
    Some(w + n * off)
}

fn quad_candidates(app: &IncaApp) -> Vec<(QuadPick, DVec3)> {
    let mut out: Vec<(QuadPick, DVec3)> = app.mesh_tool.dots.iter().enumerate().map(|(i, w)| (QuadPick::Dot(i), *w)).collect();
    if let Some(t) = &app.mesh_tool.quad_target {
        out.extend(world_verts(app, t).into_iter().enumerate().map(|(i, w)| (QuadPick::Vert(i as u32), w)));
    }
    out
}

fn quad_nearest(app: &IncaApp, cv: &CamView, rect: Rect, p: Pos2) -> Option<QuadPick> {
    quad_candidates(app).into_iter().filter_map(|(k, w)| cv.project(w, rect).map(|s| (k, s.distance(p)))).filter(|x| x.1 < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
}

fn cross2(o: Pos2, a: Pos2, b: Pos2) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// The smallest convex quad (or triangle) of nearby dots / target vertices around the cursor.
fn quad_region(app: &IncaApp, cv: &CamView, rect: Rect, p: Pos2) -> Option<Vec<(QuadPick, DVec3)>> {
    let mut cs: Vec<(QuadPick, DVec3, Pos2)> = quad_candidates(app).into_iter().filter_map(|(k, w)| cv.project(w, rect).map(|s| (k, w, s))).collect();
    cs.sort_by(|a, b| a.2.distance(p).total_cmp(&b.2.distance(p)));
    cs.truncate(8);
    if cs.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, Vec<usize>)> = None;
    let mut consider = |set: Vec<usize>| {
        let c = set.iter().fold(vec2(0.0, 0.0), |a, &i| a + cs[i].2.to_vec2()) / set.len() as f32;
        let mut ord = set.clone();
        ord.sort_by(|&a, &b| (cs[a].2.y - c.y).atan2(cs[a].2.x - c.x).total_cmp(&(cs[b].2.y - c.y).atan2(cs[b].2.x - c.x)));
        let poly: Vec<Pos2> = ord.iter().map(|&i| cs[i].2).collect();
        let n = poly.len();
        let mut sgn = 0.0f32;
        for i in 0..n {
            let cr = cross2(poly[i], poly[(i + 1) % n], poly[(i + 2) % n]);
            if cr.abs() < 1e-3 || (sgn != 0.0 && cr.signum() != sgn) {
                return;
            }
            sgn = cr.signum();
        }
        if !viewport::in_polygon(&poly, p) {
            return;
        }
        if (0..cs.len()).any(|i| !set.contains(&i) && viewport::in_polygon(&poly, cs[i].2)) {
            return;
        }
        let area: f32 = (0..n).map(|i| cross2(poly[0], poly[i], poly[(i + 1) % n])).sum::<f32>().abs();
        // prefer quads
        let score = area * if n == 4 { 0.5 } else { 1.0 };
        if best.as_ref().is_none_or(|b| score < b.0) {
            best = Some((score, ord));
        }
    };
    let m = cs.len();
    for a in 0..m {
        for b in a + 1..m {
            for c in b + 1..m {
                consider(vec![a, b, c]);
                for d in c + 1..m {
                    consider(vec![a, b, c, d]);
                }
            }
        }
    }
    best.map(|(_, ord)| ord.into_iter().map(|i| (cs[i].0, cs[i].1)).collect())
}

/// Add a face to the Quad Draw target mesh (created on first use), consuming dots.
fn quad_add_face(app: &mut IncaApp, region: Vec<(QuadPick, DVec3)>, cv: &CamView) {
    app.s.checkpoint("quad draw");
    let tid = match app.mesh_tool.quad_target.clone().filter(|t| app.s.node(t).is_some()) {
        Some(t) => t,
        None => {
            let id = app.s.create_mesh_from(PolyMesh::new(vec![], vec![]), "polySurface1", None, None);
            app.mesh_tool.quad_target = Some(id.clone());
            id
        }
    };
    let inv = app.s.world_matrix(&tid).inverse();
    let mut pm = app.s.node(&tid).unwrap().mesh().unwrap().mesh.clone();
    let mut used_dots = vec![];
    let mut face: Vec<u32> = vec![];
    for (k, w) in &region {
        match k {
            QuadPick::Vert(i) => face.push(*i),
            QuadPick::Dot(d) => {
                pm.v.push(inv.transform_point3(*w));
                face.push((pm.v.len() - 1) as u32);
                used_dots.push(*d);
            }
        }
    }
    // face the camera
    let pts: Vec<DVec3> = region.iter().map(|r| r.1).collect();
    let n = inca_core::mesh::newell(&pts);
    let toward = if cv.ortho { -cv.fwd } else { cv.eye - pts[0] };
    if n.dot(toward) < 0.0 {
        face.reverse();
    }
    pm.add_face(face, None, None);
    used_dots.sort();
    for d in used_dots.into_iter().rev() {
        app.mesh_tool.dots.remove(d);
    }
    app.s.bake_mesh(&tid, pm);
}

fn quad_draw(app: &mut IncaApp, resp: &egui::Response, cv: &CamView, rect: Rect, painter: &egui::Painter) {
    let mods = resp.ctx.input(|i| i.modifiers);
    // the live surface defaults to the selected mesh
    if app.live_surface.is_none() {
        if let Some(l) = app.s.lead().cloned().filter(|l| app.s.node(l).is_some_and(|n| n.mesh().is_some_and(|m| !m.mesh.f.is_empty())) && Some(l) != app.mesh_tool.quad_target.as_ref()) {
            app.live_surface = Some(l.clone());
            app.echo(format!("makeLive {};", app.s.name_of(&l)));
        }
    }
    // drawing: dots blue, target vertices yellow, target wire green
    if let Some(t) = &app.mesh_tool.quad_target {
        if let Some(pm) = app.s.node(t).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) {
            let w = world_verts(app, t);
            for f in &pm.f {
                let pts: Vec<DVec3> = f.iter().map(|&i| w[i as usize]).collect();
                line(painter, cv, rect, &pts, true, GREEN);
            }
            for p in &w {
                dot(painter, cv, rect, *p, 6.0, YELLOW);
            }
        }
    }
    for d in &app.mesh_tool.dots {
        dot(painter, cv, rect, *d, 8.0, BLUE);
    }
    let hover = resp.hover_pos();
    if let Some(h) = hover.and_then(|p| quad_nearest(app, cv, rect, p)) {
        if let Some((_, w)) = quad_candidates(app).into_iter().find(|c| c.0 == h) {
            dot(painter, cv, rect, w, 11.0, RED);
        }
    }
    let region = (mods.shift && !mods.ctrl).then(|| hover.and_then(|p| quad_region(app, cv, rect, p))).flatten();
    if let Some(r) = &region {
        let pts: Vec<Pos2> = r.iter().filter_map(|x| cv.project(x.1, rect)).collect();
        if pts.len() >= 3 {
            painter.add(egui::Shape::convex_polygon(pts.clone(), Color32::from_rgba_unmultiplied(92, 255, 92, 70), Stroke::new(1.5, GREEN)));
        }
    }
    // drag a dot / vertex along the surface
    if let Some(k) = app.mesh_tool.quad_drag {
        if let Some(w) = resp.interact_pointer_pos().and_then(|q| surface_point(app, cv, rect, q)) {
            match k {
                QuadPick::Dot(i) => app.mesh_tool.dots[i] = w,
                QuadPick::Vert(i) => {
                    if let Some(t) = app.mesh_tool.quad_target.clone() {
                        let inv = app.s.world_matrix(&t).inverse();
                        if let Some(m) = app.s.nodes.get_mut(&t).and_then(|n| n.mesh_mut()) {
                            m.mesh.v[i as usize] = inv.transform_point3(w);
                        }
                        if let Some(n) = app.s.nodes.get_mut(&t) {
                            n.rev += 1;
                        }
                        app.s.rev += 1;
                    }
                }
            }
        }
        if resp.drag_stopped() || !resp.is_pointer_button_down_on() {
            if let (QuadPick::Vert(_), Some(t)) = (k, app.mesh_tool.quad_target.clone()) {
                let pm = app.s.node(&t).unwrap().mesh().unwrap().mesh.clone();
                app.s.bake_mesh(&t, pm);
            }
            app.mesh_tool.quad_drag = None;
        }
        return;
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(k) = quad_nearest(app, cv, rect, p - resp.drag_delta()) {
                if let QuadPick::Vert(_) = k {
                    app.s.checkpoint("quad draw: move vertex");
                }
                app.mesh_tool.quad_drag = Some(k);
            }
        }
        return;
    }
    if !resp.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    let Some(p) = resp.interact_pointer_pos() else { return };
    if mods.shift && (mods.ctrl || mods.command) {
        // delete the dot under the cursor
        if let Some(QuadPick::Dot(i)) = quad_nearest(app, cv, rect, p) {
            app.mesh_tool.dots.remove(i);
        }
        return;
    }
    if mods.shift {
        if let Some(r) = region {
            quad_add_face(app, r, cv);
        }
        return;
    }
    if let Some(w) = surface_point(app, cv, rect, p) {
        app.mesh_tool.dots.push(w);
    }
}

/// Enter in Quad Draw: select the result.
pub fn quad_complete(app: &mut IncaApp) {
    app.mesh_tool.dots.clear();
    if let Some(t) = app.mesh_tool.quad_target.take().filter(|t| app.s.node(t).is_some()) {
        app.select_names(&[t], SelMode::Replace);
    }
}

/// Modify > Make Live: toggle the lead mesh as the live surface.
pub fn make_live(app: &mut IncaApp) {
    let lead = app.s.lead().cloned().filter(|l| app.s.node(l).is_some_and(|n| n.mesh().is_some()));
    app.live_surface = if app.live_surface.is_some() && app.live_surface == lead { None } else { lead };
    app.help = match &app.live_surface {
        Some(l) => format!("Live surface: {}", app.s.name_of(l)),
        None => "No live surface".into(),
    };
}

/// Point on the live surface under the cursor (creation tools draw on it).
pub fn live_point(app: &IncaApp, cv: &CamView, rect: Rect, p: Pos2) -> Option<DVec3> {
    let l = app.live_surface.as_ref().filter(|l| app.s.node(l).is_some())?;
    hit_mesh(app, cv, rect, p, std::slice::from_ref(l)).map(|h| h.1)
}

/// Tool Settings rows for the mesh tools.
pub fn settings(app: &mut IncaApp, ui: &mut egui::Ui) {
    let o = &mut app.mesh_opts;
    match app.tool {
        Tool::EdgeLoop => {
            ui.horizontal(|ui| {
                ui.label("Number of edge loops");
                ui.add(egui::Slider::new(&mut o.edge_loops, 1..=20));
            });
        }
        Tool::Sculpt => {
            ui.horizontal(|ui| {
                ui.label("Brush");
                egui::ComboBox::from_id_salt("brush").selected_text(cap(BRUSHES[o.brush])).show_ui(ui, |ui| {
                    for (i, b) in BRUSHES.iter().enumerate() {
                        ui.selectable_value(&mut o.brush, i, cap(b));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Size (world)");
                ui.add(egui::Slider::new(&mut o.size, 0.05..=10.0).logarithmic(true));
            });
            ui.horizontal(|ui| {
                ui.label("Strength");
                ui.add(egui::Slider::new(&mut o.strength, 0.0..=1.0));
            });
            ui.checkbox(&mut o.mirror, "Mirror X");
            ui.label(egui::RichText::new("Ctrl inverts, Shift smooths").color(crate::theme::TEXT_DIM));
        }
        Tool::MultiCut => {
            ui.label("Click edges / vertices to cut, Enter or RMB completes, Backspace/Delete removes the last point. Shift snaps to 10% steps, middle-click cuts an edge at its midpoint, Ctrl+click inserts an edge loop (Ctrl+middle-click: through the midpoint).");
        }
        Tool::QuadDraw => {
            ui.label("Click to place dots on the live surface, Shift+click fills the quad around the cursor, drag a dot or vertex to slide it, Ctrl+Shift+click deletes a dot. Enter selects the result.");
            if ui.button("Clear dots").clicked() {
                app.mesh_tool.dots.clear();
            }
        }
        _ => {}
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(ctx: &egui::Context) -> IncaApp {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        IncaApp::new(&cc)
    }

    /// An app with one cube under the Multi-Cut tool, the persp view and its rect.
    fn cube_scene(ctx: &egui::Context) -> (IncaApp, String, CamView, Rect) {
        let mut app = test_app(ctx);
        let cube = app.s.create_mesh("polyCube", &Params::new(), None);
        for a in ["sx", "sy", "sz"] {
            app.s.set_attr_f(&cube, a, 12.0);
        }
        app.s.select(std::slice::from_ref(&cube), SelMode::Replace);
        app.tool = Tool::MultiCut;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let persp = app.s.find("persp").unwrap();
        let cv = viewport::cam_view(&app.s, &persp, [rect.width(), rect.height()]);
        (app, cube, cv, rect)
    }

    /// Display > Polygons > Soft/Hard Edges toggles the display on the selected meshes only, and
    /// the menu lists it in place of the Soften/Harden Edge commands (#37).
    #[test]
    fn soft_hard_edges_toggles_on_the_selected_meshes() {
        let ctx = egui::Context::default();
        let (mut app, cube, _, _) = cube_scene(&ctx);
        let other = app.s.create_mesh("polySphere", &Params::new(), None);
        app.s.select(std::slice::from_ref(&cube), SelMode::Replace);
        app.action("toggleSoftHardEdges");
        assert!(app.overlay_softhard.contains(&cube) && !app.overlay_softhard.contains(&other));
        assert!(app.overlay_border.is_empty() && app.overlay_normals.is_empty());
        app.action("toggleSoftHardEdges");
        assert!(app.overlay_softhard.is_empty());
        let polys: Vec<&str> = crate::menus::common("Display")
            .into_iter()
            .find_map(|m| match m {
                crate::menus::M::Sub("Polygons", items) => Some(items.into_iter().filter_map(|i| if let crate::menus::M::A(a) = i { Some(a) } else { None }).collect()),
                _ => None,
            })
            .expect("Display > Polygons");
        assert!(polys.contains(&"toggleSoftHardEdges"), "{polys:?}");
        assert!(!polys.contains(&"softenEdge") && !polys.contains(&"hardenEdge"), "{polys:?}");
    }

    /// A screen point `frac` of the way along the edge `pick_edge` finds there, with that edge.
    fn point_on_edge(app: &mut IncaApp, cube: &str, cv: &CamView, rect: Rect, frac: f32) -> (Pos2, u32, u32) {
        let pm = app.s.node(cube).unwrap().mesh().unwrap().mesh.clone();
        let w = world_verts(app, cube);
        for (a, b) in pm.topo().edges {
            let (Some(sa), Some(sb)) = (cv.project(w[a as usize], rect), cv.project(w[b as usize], rect)) else { continue };
            if !rect.contains(sa) || !rect.contains(sb) || sa.distance(sb) < 80.0 {
                continue;
            }
            let p = sa + (sb - sa) * frac;
            // use an edge whose point under the cursor isn't nearer to another edge
            if let Some(e) = pick_edge(app, 0, cv, rect, p, &[cube.to_string()]) {
                if (e.a, e.b) == (a, b) {
                    return (p, a, b);
                }
            }
        }
        panic!("no clearly visible edge");
    }

    fn midpoint(app: &IncaApp, cube: &str, a: u32, b: u32) -> DVec3 {
        let w = world_verts(app, cube);
        (w[a as usize] + w[b as usize]) * 0.5
    }

    /// A screen point on a visible face, and that face.
    fn point_on_face(app: &mut IncaApp, cube: &str, cv: &CamView, rect: Rect) -> (Pos2, usize) {
        let pm = app.s.node(cube).unwrap().mesh().unwrap().mesh.clone();
        let wm = app.s.world_matrix(cube);
        let vp = app.vps[0].clone();
        for f in 0..pm.f.len() {
            let Some(p) = cv.project(wm.transform_point3(pm.face_center(f)), rect) else { continue };
            if !rect.contains(p) {
                continue;
            }
            if let Some((id, crate::viewport::Comp::F(g))) = crate::viewport::pick_best_component(&app.s, &vp, cv, &app.s.hilite(), inca_core::scene::CompMode::Face, p, &mut app.mesh_cache) {
                if id == cube && g == f {
                    return (p, f);
                }
            }
        }
        panic!("no visible face");
    }

    /// Move the OS cursor without clicking, so `ctx.pointer_hover_pos()` sees it.
    fn set_cursor(ctx: &egui::Context, rect: Rect, p: Pos2) {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(rect);
        raw.events = vec![egui::Event::PointerMoved(p)];
        let mut out = ctx.run_ui(raw, |_| {});
        out.textures_delta.clear();
    }

    /// Two clearly visible edges and a point on each, far enough apart to tell them apart.
    fn two_visible_edges(app: &mut IncaApp, cube: &str, cv: &CamView, rect: Rect) -> ((Pos2, u32, u32), (Pos2, u32, u32)) {
        let pm = app.s.node(cube).unwrap().mesh().unwrap().mesh.clone();
        let w = world_verts(app, cube);
        let mut found: Vec<(Pos2, u32, u32)> = vec![];
        for (a, b) in pm.topo().edges {
            let (Some(sa), Some(sb)) = (cv.project(w[a as usize], rect), cv.project(w[b as usize], rect)) else { continue };
            if !rect.contains(sa) || !rect.contains(sb) || sa.distance(sb) < 80.0 {
                continue;
            }
            let p = sa + (sb - sa) * 0.3;
            if let Some(e) = pick_edge(app, 0, cv, rect, p, &[cube.to_string()]) {
                if (e.a, e.b) == (a, b) && found.iter().all(|f| f.0.distance(p) > 30.0) {
                    found.push((p, a, b));
                    if found.len() == 2 {
                        return (found[0], found[1]);
                    }
                }
            }
        }
        panic!("need two clearly visible edges");
    }

    /// A screen point with no edge under it.
    fn empty_spot(app: &mut IncaApp, cube: &str, cv: &CamView, rect: Rect) -> Pos2 {
        for y in (20..580).step_by(20) {
            for x in (20..780).step_by(20) {
                let p = Pos2::new(x as f32, y as f32);
                if pick_edge(app, 0, cv, rect, p, &[cube.to_string()]).is_none() {
                    return p;
                }
            }
        }
        panic!("no empty spot");
    }

    /// Drive an MMB press, a move while held, then release, with real egui frames.
    fn mmb_drag(ctx: &egui::Context, app: &mut IncaApp, cv: &CamView, rect: Rect, from: Pos2, to: Pos2, modifiers: egui::Modifiers, t0: f64) {
        let frames = [
            vec![egui::Event::ModifiersChanged(modifiers), egui::Event::PointerMoved(from)],
            vec![egui::Event::PointerButton { pos: from, button: egui::PointerButton::Middle, pressed: true, modifiers }],
            vec![egui::Event::PointerMoved(to)],
            vec![egui::Event::PointerButton { pos: to, button: egui::PointerButton::Middle, pressed: false, modifiers }],
            vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
        ];
        for (i, events) in frames.into_iter().enumerate() {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(rect);
            raw.time = Some(t0 + i as f64 * 0.05);
            raw.events = events;
            let mut out = ctx.run_ui(raw, |ui| {
                let resp = ui.interact(rect, egui::Id::new("multi-cut drag test"), egui::Sense::click_and_drag());
                let painter = ui.painter().clone();
                multi_cut(app, 0, &resp, cv, rect, &painter);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn middle_pick_cuts_the_edge_at_its_midpoint() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (p, a, b) = point_on_edge(&mut app, &cube, &cv, rect, 0.2);
        // a plain pick stays where the cursor is
        let (_, c) = cut_pick(&mut app, 0, &cv, rect, p, false, false).unwrap();
        let (_, _, t) = c.edge.unwrap();
        assert!((t - 0.2).abs() < 0.05, "plain pick t = {t}");
        // the middle button puts it exactly halfway, also in world space
        let (id, c) = cut_pick(&mut app, 0, &cv, rect, p, false, true).unwrap();
        assert_eq!(id, cube);
        assert_eq!(c.edge, Some((a, b, 0.5)));
        assert!(c.w.distance(midpoint(&app, &cube, a, b)) < 1e-12);
        // Shift's 10% steps don't move it off the midpoint
        let (_, c) = cut_pick(&mut app, 0, &cv, rect, p, true, true).unwrap();
        assert_eq!(c.edge, Some((a, b, 0.5)));
    }

    #[test]
    fn middle_pick_next_to_a_vertex_still_takes_the_edge_midpoint() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (p, a, _) = point_on_edge(&mut app, &cube, &cv, rect, 0.5);
        let w = world_verts(&app, &cube);
        let sa = cv.project(w[a as usize], rect).unwrap();
        // 3 px from vertex a, along the edge: a plain pick snaps to the vertex
        let near = sa + (p - sa).normalized() * 3.0;
        let (_, c) = cut_pick(&mut app, 0, &cv, rect, near, false, false).unwrap();
        assert!(c.vertex.is_some(), "plain pick should snap to the vertex");
        let (_, c) = cut_pick(&mut app, 0, &cv, rect, near, false, true).unwrap();
        let (ea, eb, t) = c.edge.expect("middle pick should take the edge");
        assert_eq!(t, 0.5);
        assert!(c.w.distance(midpoint(&app, &cube, ea, eb)) < 1e-12);
    }

    /// Drive the tool with real egui frames: move, then press and release a button.
    fn click(ctx: &egui::Context, app: &mut IncaApp, cv: &CamView, rect: Rect, p: Pos2, button: egui::PointerButton, modifiers: egui::Modifiers, t0: f64) {
        let frames = [
            vec![egui::Event::ModifiersChanged(modifiers), egui::Event::PointerMoved(p)],
            vec![egui::Event::PointerButton { pos: p, button, pressed: true, modifiers }],
            vec![egui::Event::PointerButton { pos: p, button, pressed: false, modifiers }],
            vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
        ];
        for (i, events) in frames.into_iter().enumerate() {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(rect);
            raw.time = Some(t0 + i as f64 * 0.05);
            raw.events = events;
            let mut out = ctx.run_ui(raw, |ui| {
                let resp = ui.interact(rect, egui::Id::new("multi-cut test"), egui::Sense::click_and_drag());
                let painter = ui.painter().clone();
                multi_cut(app, 0, &resp, cv, rect, &painter);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn middle_click_adds_a_midpoint_cut_and_left_click_does_not() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (p, a, b) = point_on_edge(&mut app, &cube, &cv, rect, 0.2);
        click(&ctx, &mut app, &cv, rect, p, egui::PointerButton::Middle, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert_eq!(app.mesh_tool.cut[0].edge, Some((a, b, 0.5)));
        click(&ctx, &mut app, &cv, rect, p, egui::PointerButton::Primary, egui::Modifiers::NONE, 1.0);
        assert_eq!(app.mesh_tool.cut.len(), 2);
        let (_, _, t) = app.mesh_tool.cut[1].edge.unwrap();
        assert!((t - 0.2).abs() < 0.05, "left click t = {t}");
        // a middle click on empty space neither adds a point nor completes the cut
        click(&ctx, &mut app, &cv, rect, Pos2::new(5.0, 5.0), egui::PointerButton::Middle, egui::Modifiers::NONE, 2.0);
        assert_eq!(app.mesh_tool.cut.len(), 2);
    }

    /// Pressing MMB and moving off the edge before releasing must still cut at the sampled midpoint
    /// (a plain click is lost once the pointer moves far enough to become a drag).
    #[test]
    fn mmb_drag_still_cuts_at_the_sampled_midpoint() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let ((p, a, b), _) = two_visible_edges(&mut app, &cube, &cv, rect);
        let away = empty_spot(&mut app, &cube, &cv, rect);
        mmb_drag(&ctx, &mut app, &cv, rect, p, away, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert_eq!(app.mesh_tool.cut[0].edge, Some((a, b, 0.5)));
    }

    /// Dragging MMB to another edge while held moves the cut there, as in Maya.
    #[test]
    fn mmb_drag_to_another_edge_cuts_there() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (first, second) = two_visible_edges(&mut app, &cube, &cv, rect);
        mmb_drag(&ctx, &mut app, &cv, rect, first.0, second.0, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert_eq!(app.mesh_tool.cut[0].edge, Some((second.1, second.2, 0.5)));
    }

    /// Shaded mode must not pick an edge hidden behind the front surface (the Ctrl loop preview
    /// used to sample back edges).
    #[test]
    fn back_edge_is_not_picked_in_shaded_mode() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let w = world_verts(&app, &cube);
        let mut far: Option<((u32, u32), f64, Pos2)> = None;
        for (a, b) in pm.topo().edges {
            let (Some(sa), Some(sb)) = (cv.project(w[a as usize], rect), cv.project(w[b as usize], rect)) else { continue };
            if !rect.contains(sa) || !rect.contains(sb) {
                continue;
            }
            let mid = (w[a as usize] + w[b as usize]) * 0.5;
            let depth = cv.depth(mid);
            if far.as_ref().is_none_or(|f| depth > f.1) {
                far = Some(((a, b), depth, sa + (sb - sa) * 0.5));
            }
        }
        let ((a, b), depth, p) = far.expect("a back edge");
        if let Some(hit) = pick_edge(&mut app, 0, &cv, rect, p, &[cube.clone()]) {
            assert!(!((hit.a, hit.b) == (a, b) || (hit.a, hit.b) == (b, a)), "picked the hidden back edge");
            let hit_mid = (w[hit.a as usize] + w[hit.b as usize]) * 0.5;
            assert!(cv.depth(hit_mid) < depth - 1.0, "picked a hidden edge");
        }
    }

    /// Shaded mode must not pick a vertex hidden behind the front surface either.
    #[test]
    fn back_vertex_is_not_picked_in_shaded_mode() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let w = world_verts(&app, &cube);
        let (v, depth, p) = w.iter().enumerate().map(|(i, w)| (i as u32, cv.depth(*w), cv.project(*w, rect).unwrap())).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        if let Some(hit) = pick_vertex(&mut app, 0, &cv, rect, p, &[cube], 8.0) {
            assert_ne!(hit.1, v, "picked the hidden back vertex");
            assert!(cv.depth(w[hit.1 as usize]) < depth - 1.0, "picked a hidden vertex");
        }
    }

    /// In a mesh tool, Delete removes the last point like Backspace instead of deleting the mesh.
    #[test]
    fn delete_removes_the_last_cut_point_instead_of_the_mesh() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let none = egui::Modifiers::NONE;
        assert_eq!(crate::actions::hotkey(egui::Key::Delete, none, &app), Some("deleteSel"));
        let ((p, _, _), _) = two_visible_edges(&mut app, &cube, &cv, rect);
        click(&ctx, &mut app, &cv, rect, p, egui::PointerButton::Middle, none, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert_eq!(crate::actions::hotkey(egui::Key::Backspace, none, &app), Some("toolUndoPoint"));
        assert_eq!(crate::actions::hotkey(egui::Key::Delete, none, &app), Some("toolUndoPoint"));
    }

    /// If the cut mesh disappears, the floating cut preview is dropped.
    #[test]
    fn deleting_the_cut_mesh_clears_the_preview() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let ((p, _, _), _) = two_visible_edges(&mut app, &cube, &cv, rect);
        click(&ctx, &mut app, &cv, rect, p, egui::PointerButton::Middle, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert!(app.mesh_tool.cut_obj.is_some());
        app.s.delete_node(&cube);
        // one frame of the active tool drops the stale points
        click(&ctx, &mut app, &cv, rect, Pos2::new(5.0, 5.0), egui::PointerButton::Primary, egui::Modifiers::NONE, 1.0);
        assert!(app.mesh_tool.cut.is_empty());
        assert!(app.mesh_tool.cut_obj.is_none());
    }

    /// Maya's Delete dissolves the highlighted edge even without selecting it.
    #[test]
    fn delete_dissolves_the_highlighted_edge_without_selecting_it() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.s.set_comp_mode(Some(inca_core::scene::CompMode::Edge));
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        let ((p, _, _), _) = two_visible_edges(&mut app, &cube, &cv, rect);
        set_cursor(&ctx, rect, p);
        let faces = app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len();
        assert!(crate::vp_ui::delete_hovered_component(&mut app, &ctx));
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        assert_eq!(pm.f.len(), faces - 1, "dissolving an edge merges its two faces");
    }

    /// The same for a highlighted face, which leaves a hole.
    #[test]
    fn delete_removes_the_highlighted_face_without_selecting_it() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.s.set_comp_mode(Some(inca_core::scene::CompMode::Face));
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        let (p, _) = point_on_face(&mut app, &cube, &cv, rect);
        set_cursor(&ctx, rect, p);
        let faces = app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len();
        assert!(crate::vp_ui::delete_hovered_component(&mut app, &ctx));
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        assert_eq!(pm.f.len(), faces - 1);
    }

    /// Maya does not hover-delete vertices, and a component selection wins over the hover.
    #[test]
    fn hover_delete_skips_vertices_and_active_selections() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.s.set_comp_mode(Some(inca_core::scene::CompMode::Vertex));
        set_cursor(&ctx, rect, rect.center());
        assert!(!crate::vp_ui::delete_hovered_component(&mut app, &ctx));
        // an edge selection must be deleted by the normal action, not by the hover
        app.s.set_comp_mode(Some(inca_core::scene::CompMode::Edge));
        let ((p, a, b), _) = two_visible_edges(&mut app, &cube, &cv, rect);
        app.s.comp.entry(cube.clone()).or_default().e.insert((a.min(b), a.max(b)));
        set_cursor(&ctx, rect, p);
        assert!(!crate::vp_ui::delete_hovered_component(&mut app, &ctx));
    }

    /// A cut between two edges inserts points on the visible edges its screen line crosses.
    #[test]
    fn cut_line_adds_points_on_the_edges_it_crosses() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        let w = world_verts(&app, &cube);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let topo = pm.topo();
        let vp = app.vps[0].clone();
        let mut found: Option<[CutPt; 2]> = None;
        'search: for (i, &(a, b)) in topo.edges.iter().enumerate() {
            for &(c, d) in &topo.edges[i + 1..] {
                let pts = [
                    CutPt { vertex: None, edge: Some((a, b, 0.5)), w: (w[a as usize] + w[b as usize]) * 0.5 , face: None, anchor: None },
                    CutPt { vertex: None, edge: Some((c, d, 0.5)), w: (w[c as usize] + w[d as usize]) * 0.5 , face: None, anchor: None },
                ];
                if expanded_cut(&mut app, &cube, &pts, &[], &cv, &vp).len() > 2 {
                    found = Some(pts);
                    break 'search;
                }
            }
        }
        let pts = found.expect("a cut line crossing an intermediate edge");
        let paths = expanded_cut(&mut app, &cube, &pts, &[], &cv, &vp);
        assert!(paths.len() > 2, "no intermediate points");
        let before = app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len();
        let list: Vec<serde_json::Value> = paths
            .iter()
            .map(|p| match *p {
                SplitPoint::Vertex(v) => json!({ "vertex": v }),
                SplitPoint::Edge(a, b, t) => json!({ "edge": [a, b], "t": t }),
                SplitPoint::Face(fi, p) => json!({ "face": fi, "point": [p.x, p.y, p.z] }),
            })
            .collect();
        let mut p = Params::new();
        p.insert("points".into(), serde_json::Value::Array(list));
        app.s.apply_op(&cube, "polySplit", p, true);
        let after = app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len();
        assert!(after > before, "cut did not split any face");
    }

    /// Enabling Multi-Cut from object mode enters component mode on the selected mesh.
    #[test]
    fn multi_cut_enters_component_mode_from_object_mode() {
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx);
        let cube = app.s.create_mesh("polyCube", &Params::new(), None);
        app.s.select(std::slice::from_ref(&cube), SelMode::Replace);
        assert!(app.s.comp_mode.is_none());
        app.set_tool(Tool::MultiCut);
        assert!(app.s.comp_mode.is_some(), "Multi-Cut should enter component mode");
        assert!(app.s.hilite().contains(&cube), "the selected mesh should be hilited");
    }

    /// Drive an LMB press, a move while held, then release, with real egui frames.
    fn lmb_drag(ctx: &egui::Context, app: &mut IncaApp, cv: &CamView, rect: Rect, from: Pos2, to: Pos2, modifiers: egui::Modifiers, t0: f64) {
        let frames = [
            vec![egui::Event::ModifiersChanged(modifiers), egui::Event::PointerMoved(from)],
            vec![egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers }],
            vec![egui::Event::PointerMoved(to)],
            vec![egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers }],
            vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
        ];
        for (i, events) in frames.into_iter().enumerate() {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(rect);
            raw.time = Some(t0 + i as f64 * 0.05);
            raw.events = events;
            let mut out = ctx.run_ui(raw, |ui| {
                let resp = ui.interact(rect, egui::Id::new("multi-cut lmb drag test"), egui::Sense::click_and_drag());
                let painter = ui.painter().clone();
                multi_cut(app, 0, &resp, cv, rect, &painter);
            });
            out.textures_delta.clear();
        }
    }

    /// A point on the given edge at `frac`, for dragging tests.
    fn point_on(app: &IncaApp, cube: &str, cv: &CamView, rect: Rect, a: u32, b: u32, frac: f32) -> Pos2 {
        let w = world_verts(app, cube);
        let (sa, sb) = (cv.project(w[a as usize], rect).unwrap(), cv.project(w[b as usize], rect).unwrap());
        sa + (sb - sa) * frac
    }

    #[test]
    fn lmb_drag_slides_the_cut_point_along_the_edge() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (p0, a, b) = point_on_edge(&mut app, &cube, &cv, rect, 0.2);
        let p1 = point_on(&app, &cube, &cv, rect, a, b, 0.7);
        lmb_drag(&ctx, &mut app, &cv, rect, p0, p1, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        let (_, _, t) = app.mesh_tool.cut[0].edge.unwrap();
        assert!((t - 0.7).abs() < 0.05, "slid t = {t}");
    }

    #[test]
    fn shift_click_places_the_midpoint_and_shift_drag_snaps_to_steps() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (p0, a, b) = point_on_edge(&mut app, &cube, &cv, rect, 0.2);
        click(&ctx, &mut app, &cv, rect, p0, egui::PointerButton::Primary, egui::Modifiers::SHIFT, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert_eq!(app.mesh_tool.cut[0].edge, Some((a, b, 0.5)));
        app.mesh_tool.cut.clear();
        app.mesh_tool.cut_obj = None;
        app.mesh_tool.cut_vp = None;
        let p1 = point_on(&app, &cube, &cv, rect, a, b, 0.33);
        lmb_drag(&ctx, &mut app, &cv, rect, p0, p1, egui::Modifiers::SHIFT, 1.0);
        assert_eq!(app.mesh_tool.cut.len(), 1);
        assert_eq!(app.mesh_tool.cut[0].edge, Some((a, b, 0.3)));
    }

    /// Undo after a committed cut must restore the mesh and drop the stale cut preview.
    #[test]
    fn undo_after_a_cut_restores_the_mesh_and_clears_the_preview() {
        let ctx = egui::Context::default();
        let (mut app, cube, _cv, rect) = cube_scene(&ctx);
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let f = pm.f[0].clone();
        let w = world_verts(&app, &cube);
        let cut = |a: u32, b: u32| CutPt { vertex: None, edge: Some((a, b, 0.5)), w: (w[a as usize] + w[b as usize]) * 0.5 , face: None, anchor: None };
        app.mesh_tool.cut = vec![cut(f[0], f[1]), cut(f[2], f[3])];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.mesh_tool.cut_vp = Some(0);
        let before = pm.f.len();
        crate::mesh_tools::multi_cut_complete(&mut app);
        assert!(app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len() > before, "the cut did not split the face");
        app.action("undo");
        assert_eq!(app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len(), before, "undo restored the wrong step");
        assert!(app.mesh_tool.cut.is_empty());
        assert!(app.mesh_tool.cut_obj.is_none());
        // Enter with no points left must be harmless rather than indexing a restored mesh.
        crate::mesh_tools::multi_cut_complete(&mut app);
    }

    /// Ctrl+Z while points are still being placed removes them one by one, not the tool session.
    #[test]
    fn undo_removes_pending_cut_points_first() {
        let ctx = egui::Context::default();
        let (mut app, cube, _cv, _rect) = cube_scene(&ctx);
        app.set_tool(Tool::MultiCut);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let f = pm.f[0].clone();
        let w = world_verts(&app, &cube);
        let cut = |a: u32, b: u32| CutPt { vertex: None, edge: Some((a, b, 0.5)), w: (w[a as usize] + w[b as usize]) * 0.5 , face: None, anchor: None };
        app.mesh_tool.cut = vec![cut(f[0], f[1]), cut(f[2], f[3])];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.action("undo");
        assert_eq!(app.mesh_tool.cut.len(), 1);
        app.action("undo");
        assert!(app.mesh_tool.cut.is_empty());
        assert!(app.mesh_tool.cut_obj.is_none());
    }

    /// One point on an edge is a valid commit: it inserts a vertex on that edge.
    #[test]
    fn a_single_edge_point_creates_a_vertex() {
        let ctx = egui::Context::default();
        let (mut app, cube, _cv, rect) = cube_scene(&ctx);
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let f = pm.f[0].clone();
        let w = world_verts(&app, &cube);
        app.mesh_tool.cut = vec![CutPt { vertex: None, edge: Some((f[0], f[1], 0.5)), w: (w[f[0] as usize] + w[f[1] as usize]) * 0.5 , face: None, anchor: None }];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.mesh_tool.cut_vp = Some(0);
        crate::mesh_tools::multi_cut_complete(&mut app);
        let mesh = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        assert_eq!(mesh.v.len(), pm.v.len() + 1, "single point should split the edge");
        assert_eq!(mesh.f.len(), pm.f.len(), "single point should not split a face");
    }

    /// Hovering/clicking a face adds its nearest vertex and an interior point.
    #[test]
    fn face_click_adds_the_anchor_and_face_point() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let (p, _) = point_on_face(&mut app, &cube, &cv, rect);
        click(&ctx, &mut app, &cv, rect, p, egui::PointerButton::Primary, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 2, "a face click adds the anchor vertex and the face point");
        assert!(app.mesh_tool.cut[1].face.is_some());
        assert_eq!(app.mesh_tool.cut[0].vertex, app.mesh_tool.cut[1].anchor);
    }

    /// A face point between two face vertices splits that face into two n-gons.
    #[test]
    fn face_point_cut_splits_a_face() {
        let ctx = egui::Context::default();
        let (mut app, cube, _cv, rect) = cube_scene(&ctx);
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let f = pm.f[0].clone();
        let w = world_verts(&app, &cube);
        let p = (pm.v[f[0] as usize] + pm.v[f[2] as usize]) * 0.5;
        let wp = app.s.world_matrix(&cube).transform_point3(p);
        app.mesh_tool.cut = vec![
            CutPt { vertex: Some(f[0]), edge: None, w: w[f[0] as usize], face: None, anchor: None },
            CutPt { vertex: None, edge: None, w: wp, face: Some((0, p)), anchor: Some(f[0]) },
            CutPt { vertex: Some(f[2]), edge: None, w: w[f[2] as usize], face: None, anchor: None },
        ];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.mesh_tool.cut_vp = Some(0);
        let before = pm.f.len();
        crate::mesh_tools::multi_cut_complete(&mut app);
        let mesh = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        assert_eq!(mesh.f.len(), before + 1, "the face should split in two");
        assert_eq!(mesh.v.len(), pm.v.len() + 1, "the face point should add one vertex");
    }

    /// Dragging a placed point after the first repositions it, like Maya's Multi-Cut.
    #[test]
    fn dragging_a_placed_point_repositions_it() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.active_vp = 0;
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let ((_, a1, b1), (_, a2, b2)) = two_visible_edges(&mut app, &cube, &cv, rect);
        let w = world_verts(&app, &cube);
        let mid = |a: u32, b: u32| CutPt { vertex: None, edge: Some((a, b, 0.5)), w: (w[a as usize] + w[b as usize]) * 0.5, face: None, anchor: None };
        app.mesh_tool.cut = vec![mid(a1, b1), mid(a2, b2)];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.mesh_tool.cut_vp = Some(0);
        let start = cv.project(app.mesh_tool.cut[1].w, rect).unwrap();
        let target = point_on(&app, &cube, &cv, rect, a1, b1, 0.7);
        lmb_drag(&ctx, &mut app, &cv, rect, start, target, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 2, "moving a point must not add a new one");
        let (a, b, t) = app.mesh_tool.cut[1].edge.unwrap();
        assert_eq!((a.min(b), a.max(b)), (a1.min(b1), a1.max(b1)), "the point should move to the target edge");
        assert!((t - 0.7).abs() < 0.05, "moved t = {t}");
    }

    /// A placed point can be moved from an edge onto a face.
    #[test]
    fn dragging_a_placed_point_onto_a_face_changes_its_type() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.active_vp = 0;
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let ((_, a1, b1), (_, a2, b2)) = two_visible_edges(&mut app, &cube, &cv, rect);
        let w = world_verts(&app, &cube);
        let mid = |a: u32, b: u32| CutPt { vertex: None, edge: Some((a, b, 0.5)), w: (w[a as usize] + w[b as usize]) * 0.5, face: None, anchor: None };
        app.mesh_tool.cut = vec![mid(a1, b1), mid(a2, b2)];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.mesh_tool.cut_vp = Some(0);
        let start = cv.project(app.mesh_tool.cut[1].w, rect).unwrap();
        let (target, _) = point_on_face(&mut app, &cube, &cv, rect);
        lmb_drag(&ctx, &mut app, &cv, rect, start, target, egui::Modifiers::NONE, 0.0);
        assert_eq!(app.mesh_tool.cut.len(), 2);
        assert!(app.mesh_tool.cut[1].face.is_some(), "the point should become a face point");
        assert!(app.mesh_tool.cut[1].anchor.is_some());
    }

    /// A closed cut around a cube splits all four side faces, including the occluded ones.
    #[test]
    fn closed_cut_around_a_cube_has_no_gaps() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        app.active_vp = 0;
        app.vps[0].rect = rect;
        app.vps[0].camera = "persp".into();
        app.set_tool(Tool::MultiCut);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let w = world_verts(&app, &cube);
        let topo = pm.topo();
        let mut sides: Vec<(f64, u32, u32)> = vec![];
        for &(a, b) in &topo.edges {
            let (pa, pb) = (w[a as usize], w[b as usize]);
            if (pa.x - pb.x).abs() < 1e-6 && (pa.z - pb.z).abs() < 1e-6 && (pa.y - pb.y).abs() > 1.0 {
                let mid = (pa + pb) * 0.5;
                sides.push((mid.z.atan2(mid.x), a, b));
            }
        }
        assert_eq!(sides.len(), 4, "cube should have four vertical edges");
        sides.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut cut: Vec<CutPt> = sides
            .iter()
            .map(|&(_, a, b)| CutPt { vertex: None, edge: Some((a, b, 0.5)), w: (w[a as usize] + w[b as usize]) * 0.5, face: None, anchor: None })
            .collect();
        cut.push(cut[0].clone());
        app.mesh_tool.cut = cut;
        app.mesh_tool.cut_cams = vec![cv; app.mesh_tool.cut.len()];
        app.mesh_tool.cut_obj = Some(cube.clone());
        app.mesh_tool.cut_vp = Some(0);
        app.mesh_tool.cut_cam = Some(cv);
        let before = pm.f.len();
        crate::mesh_tools::multi_cut_complete(&mut app);
        let after = app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len();
        assert!(after >= before + 4, "the four side faces should each split: {before} -> {after}");
    }

    #[test]
    fn ctrl_middle_click_inserts_an_edge_loop_through_the_midpoint() {
        let ctx = egui::Context::default();
        let (mut app, cube, cv, rect) = cube_scene(&ctx);
        let (p, a, b) = point_on_edge(&mut app, &cube, &cv, rect, 0.2);
        let mid = midpoint(&app, &cube, a, b);
        let faces = app.s.node(&cube).unwrap().mesh().unwrap().mesh.f.len();
        click(&ctx, &mut app, &cv, rect, p, egui::PointerButton::Middle, egui::Modifiers::CTRL, 0.0);
        let pm = app.s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        assert_eq!(pm.f.len(), faces + 4, "a loop around the cube splits 4 faces");
        let w = world_verts(&app, &cube);
        assert!(w.iter().any(|v| v.distance(mid) < 1e-9), "no new vertex at the edge midpoint {mid}");
        assert!(app.mesh_tool.cut.is_empty());
    }
}
