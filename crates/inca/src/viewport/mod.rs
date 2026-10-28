//! Viewports: cameras & navigation, scene -> GPU draw lists, picking and component display.

pub mod gpu;

use egui::{Color32, Pos2, Rect, Stroke};
use glam::{DMat4, DQuat, DVec3, Mat4};
use gpu::*;
use inca_core::math::{euler_deg_from_quat, quat_from_euler_deg, Aabb, Ray};
use inca_core::mesh::{PolyMesh, Topo};
use inca_core::scene::{CompMode, Id, Scene, Shape};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub const C_WIRE: [f32; 4] = [0.063, 0.063, 0.43, 1.0];
pub const C_LEAD: [f32; 4] = [0.36, 0.96, 0.64, 1.0];
pub const C_SEL: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const C_HILITE: [f32; 4] = [0.53, 0.78, 1.0, 1.0];
pub const C_TEMPLATE: [f32; 4] = [0.55, 0.55, 0.55, 1.0];
/// a selected template (Maya's light pink, #ff9999)
pub const C_TEMPLATE_SEL: [f32; 4] = [1.0, 0.6, 0.6, 1.0];
pub const C_ICON: [f32; 4] = [0.063, 0.063, 0.063, 1.0];
pub const C_CURVE: [f32; 4] = [0.05, 0.05, 0.4, 1.0];
pub const C_FACE_SEL: [f32; 3] = [1.0, 0.54, 0.12];
/// unselected face-centre dots in face mode (the JS `COMP.dot`)
pub const C_DOT: [f32; 3] = [74.0 / 255.0, 138.0 / 255.0, 214.0 / 255.0];
/// objects driven by the selection (JS COLORS.affected)
pub const C_AFFECTED: [f32; 4] = [0.78, 0.235, 0.82, 1.0];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadeMode {
    Wire,
    Shaded,
    Textured,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LightMode {
    Default,
    All,
    Flat,
    None,
}

#[derive(Clone, Debug)]
pub struct Viewport {
    pub camera: String,
    pub shade: ShadeMode,
    pub lights: LightMode,
    pub wire_on_shaded: bool,
    pub xray: bool,
    pub backface: bool,
    pub grid: bool,
    pub hud: bool,
    pub show_lights: bool,
    pub show_cameras: bool,
    pub show_curves: bool,
    pub show_meshes: bool,
    pub show_locators: bool,
    pub show_manips: bool,
    pub show_sel_highlight: bool,
    pub film_gate: bool,
    pub res_gate: bool,
    /// display exposure (stops) and gamma, applied to shaded surfaces
    pub exposure: f32,
    pub gamma: f32,
    pub isolate: Option<Vec<Id>>,
    pub rect: Rect,
    /// final-render look (hardware renderer): meshes only, no wires or highlights
    pub beauty: bool,
    /// grid length/width, line spacing, major line every n lines (Preferences > Display)
    pub grid_cfg: (f64, f64, i32),
}

impl Viewport {
    /// Show menu filters by object type.
    pub fn shows(&self, n: &inca_core::scene::Node) -> bool {
        match &n.shape {
            Shape::Mesh(_) => self.show_meshes,
            Shape::Light(_) => self.show_lights,
            Shape::Camera(_) => self.show_cameras,
            Shape::Curve(_) => self.show_curves,
            Shape::Locator if n.kind == "locator" => self.show_locators,
            _ => true,
        }
    }
    /// Show > All / None.
    pub fn show_all(&mut self, on: bool) {
        self.show_meshes = on;
        self.show_lights = on;
        self.show_cameras = on;
        self.show_curves = on;
        self.show_locators = on;
        self.grid = on;
        self.show_sel_highlight = on;
    }
    pub fn new(camera: &str) -> Self {
        Viewport {
            camera: camera.into(),
            shade: ShadeMode::Shaded,
            lights: LightMode::Default,
            wire_on_shaded: false,
            xray: false,
            backface: false,
            grid: true,
            hud: true,
            show_lights: true,
            show_cameras: true,
            show_curves: true,
            show_meshes: true,
            show_locators: true,
            show_manips: true,
            show_sel_highlight: true,
            beauty: false,
            grid_cfg: (12.0, 1.0, 5),
            film_gate: false,
            res_gate: false,
            exposure: 0.0,
            gamma: 1.0,
            isolate: None,
            rect: Rect::NOTHING,
        }
    }
}

/// Camera state derived from a camera node.
#[derive(Clone, Copy, Debug)]
pub struct CamView {
    pub view: DMat4,
    pub proj: DMat4,
    pub eye: DVec3,
    pub fwd: DVec3,
    pub up: DVec3,
    pub right: DVec3,
    pub ortho: bool,
    pub ortho_width: f64,
    pub fov_y: f64,
    pub coi: f64,
    pub size: [f32; 2],
}

impl CamView {
    pub fn view_proj(&self) -> DMat4 {
        self.proj * self.view
    }
    /// World -> screen (relative to the viewport rect). Returns None behind the camera.
    pub fn project(&self, p: DVec3, rect: Rect) -> Option<Pos2> {
        let c = self.view_proj() * p.extend(1.0);
        if c.w <= 1e-9 && !self.ortho {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(Pos2::new(rect.min.x + ((n.x + 1.0) * 0.5) as f32 * rect.width(), rect.min.y + ((1.0 - n.y) * 0.5) as f32 * rect.height()))
    }
    pub fn depth(&self, p: DVec3) -> f64 {
        (p - self.eye).dot(self.fwd)
    }
    pub fn ray(&self, pos: Pos2, rect: Rect) -> Ray {
        let nx = ((pos.x - rect.min.x) / rect.width()) as f64 * 2.0 - 1.0;
        let ny = 1.0 - ((pos.y - rect.min.y) / rect.height()) as f64 * 2.0;
        let inv = self.view_proj().inverse();
        let a = inv.project_point3(DVec3::new(nx, ny, 0.0));
        let b = inv.project_point3(DVec3::new(nx, ny, 0.5));
        Ray { origin: a, dir: (b - a).normalize() }
    }
    /// World units per pixel at the centre of interest.
    pub fn units_per_px(&self) -> f64 {
        if self.ortho {
            self.ortho_width / self.size[0].max(1.0) as f64
        } else {
            2.0 * self.coi * (self.fov_y / 2.0).tan() / self.size[1].max(1.0) as f64
        }
    }
}

pub fn camera_id(s: &Scene, name: &str) -> Option<Id> {
    s.find(name).filter(|i| matches!(s.node(i).map(|n| &n.shape), Some(Shape::Camera(_)))).or_else(|| s.find("persp"))
}

pub fn cam_view(s: &Scene, cam: &str, size: [f32; 2]) -> CamView {
    let id = camera_id(s, cam);
    let (m, c) = match id.as_ref().and_then(|i| s.node(i).map(|n| (i, n))) {
        Some((i, n)) => (s.world_matrix(i), if let Shape::Camera(c) = &n.shape { c.clone() } else { Default::default() }),
        None => (DMat4::look_at_rh(DVec3::new(28.0, 21.0, 28.0), DVec3::ZERO, DVec3::Y).inverse(), Default::default()),
    };
    let aspect = (size[0] / size[1].max(1.0)) as f64;
    let eye = m.transform_point3(DVec3::ZERO);
    let fwd = m.transform_vector3(-DVec3::Z).normalize();
    let up = m.transform_vector3(DVec3::Y).normalize();
    let right = fwd.cross(up).normalize();
    let view = DMat4::look_to_rh(eye, fwd, up);
    let fov_y = c.fov_y(aspect);
    let proj = if c.ortho {
        let w = c.ortho_width / 2.0;
        let h = w / aspect;
        DMat4::orthographic_rh(-w, w, -h, h, c.near.max(0.01), c.far.max(10.0) + 2000.0)
    } else {
        DMat4::perspective_rh(fov_y, aspect, c.near.max(0.001), c.far.max(1.0))
    };
    CamView { view, proj, eye, fwd, up, right, ortho: c.ortho, ortho_width: c.ortho_width, fov_y, coi: c.coi.max(0.01), size }
}

// ------------------------------------------------------------------ navigation

/// Alt+LMB tumble around the centre of interest.
pub fn tumble(s: &mut Scene, cam: &str, dx: f32, dy: f32, allow_ortho: bool) {
    let Some(id) = camera_id(s, cam) else { return };
    let Some(n) = s.nodes.get(&id) else { return };
    let Shape::Camera(c) = &n.shape else { return };
    if c.ortho && !allow_ortho {
        return;
    }
    let coi = c.coi;
    let q = quat_from_euler_deg(n.xf.r, &n.xf.ro);
    let eye = glam::DVec3::from_array(n.xf.t);
    let fwd = q * -DVec3::Z;
    let center = eye + fwd * coi;
    let r = euler_deg_from_quat(q, "xyz");
    // derive yaw / pitch, then apply
    let yaw = fwd.x.atan2(fwd.z).to_degrees() + 180.0;
    let pitch = fwd.y.clamp(-1.0, 1.0).asin().to_degrees();
    let _ = r;
    let yaw = yaw - dx as f64 * 0.3;
    let pitch = (pitch - dy as f64 * 0.3).clamp(-89.9, 89.9);
    let nq = DQuat::from_euler(glam::EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), 0.0);
    let nf = nq * -DVec3::Z;
    let ne = center - nf * coi;
    let e = euler_deg_from_quat(nq, "xyz");
    let n = s.nodes.get_mut(&id).unwrap();
    n.xf.t = ne.to_array();
    n.xf.r = e;
    n.xf.ro = "xyz".into();
    s.rev += 1;
}

/// Alt+MMB track (pan).
pub fn track(s: &mut Scene, cv: &CamView, cam: &str, dx: f32, dy: f32) {
    let Some(id) = camera_id(s, cam) else { return };
    let k = cv.units_per_px();
    let d = cv.right * (-dx as f64 * k) + cv.up * (dy as f64 * k);
    if let Some(n) = s.nodes.get_mut(&id) {
        n.xf.t = (DVec3::from_array(n.xf.t) + d).to_array();
    }
    s.rev += 1;
}

/// Alt+RMB / wheel dolly: `amount` > 0 moves in.
pub fn dolly(s: &mut Scene, cam: &str, amount: f64) {
    let Some(id) = camera_id(s, cam) else { return };
    let Some(n) = s.nodes.get_mut(&id) else { return };
    let q = quat_from_euler_deg(n.xf.r, &n.xf.ro);
    let fwd = q * -DVec3::Z;
    if let Shape::Camera(c) = &mut n.shape {
        let f = (-amount).exp();
        if c.ortho {
            c.ortho_width = (c.ortho_width * f).clamp(0.01, 100000.0);
        } else {
            let center = DVec3::from_array(n.xf.t) + fwd * c.coi;
            c.coi = (c.coi * f).clamp(0.01, 1e6);
            n.xf.t = (center - fwd * c.coi).to_array();
        }
    }
    s.rev += 1;
}

/// Place a camera (translate, rotate in degrees, centre of interest).
pub fn set_view(s: &mut Scene, cam: &str, t: [f64; 3], r: [f64; 3], coi: Option<f64>) {
    let Some(id) = camera_id(s, cam) else { return };
    let Some(n) = s.nodes.get_mut(&id) else { return };
    n.xf.t = t;
    n.xf.r = r;
    n.xf.ro = "xyz".into();
    if let (Shape::Camera(c), Some(d)) = (&mut n.shape, coi) {
        c.coi = d;
    }
    s.rev += 1;
}

/// View > Default View (the JS defaultView).
pub fn default_view(s: &mut Scene, cam: &str) {
    let Some(id) = camera_id(s, cam) else { return };
    let (ortho, name) = match s.node(&id) {
        Some(n) => (matches!(&n.shape, Shape::Camera(c) if c.ortho), n.name.clone()),
        None => return,
    };
    if !ortho || name == "persp" {
        set_view(s, cam, [28.0, 21.0, 28.0], [-27.938, 45.0, 0.0], Some(44.82));
        return;
    }
    let d = match name.as_str() {
        "top" => Some(([0.0, 1000.1, 0.0], [-90.0, 0.0, 0.0])),
        "front" => Some(([0.0, 0.0, 1000.1], [0.0, 0.0, 0.0])),
        "side" => Some(([1000.1, 0.0, 0.0], [0.0, 90.0, 0.0])),
        _ => None,
    };
    if let Some((t, r)) = d {
        set_view(s, cam, t, r, None);
    }
    if let Some(Shape::Camera(c)) = s.nodes.get_mut(&id).map(|n| &mut n.shape) {
        c.ortho_width = 30.0;
    }
}

/// View > Look at Selection: aim the camera at a point, keeping its position.
pub fn look_at(s: &mut Scene, cam: &str, target: DVec3) {
    let Some(id) = camera_id(s, cam) else { return };
    let Some(n) = s.nodes.get(&id) else { return };
    if matches!(&n.shape, Shape::Camera(c) if c.ortho) {
        return;
    }
    let eye = DVec3::from_array(n.xf.t);
    let d = target - eye;
    if d.length() < 1e-9 {
        return;
    }
    let f = d.normalize();
    let yaw = f.x.atan2(f.z).to_degrees() + 180.0;
    let pitch = f.y.clamp(-1.0, 1.0).asin().to_degrees();
    let q = DQuat::from_euler(glam::EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), 0.0);
    let t = n.xf.t;
    set_view(s, cam, t, euler_deg_from_quat(q, "xyz"), Some(d.length()));
}

/// Film / resolution gate: darken outside the gate and outline it (as the JS drawGate).
pub fn draw_gate(s: &Scene, vp: &Viewport, painter: &egui::Painter) {
    if !(vp.res_gate || vp.film_gate) {
        return;
    }
    let rs = inca_core::render::RenderSettings::from_scene(s);
    let asp = if vp.res_gate {
        rs.width as f32 / rs.height.max(1) as f32
    } else {
        match camera_id(s, &vp.camera).and_then(|id| s.node(&id)).map(|n| &n.shape) {
            Some(Shape::Camera(c)) => (c.horizontal_film_aperture / c.vertical_film_aperture.max(1e-6)) as f32,
            _ => 1.5,
        }
    };
    let r = vp.rect;
    let mut gw = r.width() * 0.9;
    let mut gh = gw / asp;
    if gh > r.height() * 0.9 {
        gh = r.height() * 0.9;
        gw = gh * asp;
    }
    let g = Rect::from_center_size(r.center(), egui::vec2(gw, gh));
    let mask = Color32::from_black_alpha(90);
    painter.rect_filled(Rect::from_min_max(r.min, egui::pos2(r.max.x, g.min.y)), 0.0, mask);
    painter.rect_filled(Rect::from_min_max(egui::pos2(r.min.x, g.max.y), r.max), 0.0, mask);
    painter.rect_filled(Rect::from_min_max(egui::pos2(r.min.x, g.min.y), egui::pos2(g.min.x, g.max.y)), 0.0, mask);
    painter.rect_filled(Rect::from_min_max(egui::pos2(g.max.x, g.min.y), egui::pos2(r.max.x, g.max.y)), 0.0, mask);
    let col = if vp.res_gate { Color32::from_gray(0xdd) } else { Color32::from_rgb(0xe0, 0xb0, 0x30) };
    painter.rect_stroke(g, 0.0, egui::Stroke::new(1.0, col), egui::StrokeKind::Inside);
    let label = if vp.res_gate { format!("{} x {}", rs.width, rs.height) } else { "Film Gate".to_string() };
    painter.text(egui::pos2(g.min.x + 4.0, g.min.y - 3.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(11.0), Color32::from_gray(0xee));
}

/// Frame a world box (F / A).
pub fn frame_box(s: &mut Scene, cam: &str, b: &Aabb, aspect: f64) {
    if b.is_empty() {
        return;
    }
    let Some(id) = camera_id(s, cam) else { return };
    let Some(n) = s.nodes.get_mut(&id) else { return };
    let q = quat_from_euler_deg(n.xf.r, &n.xf.ro);
    let fwd = q * -DVec3::Z;
    let center = b.center();
    let radius = (b.size().length() * 0.5).max(0.05);
    if let Shape::Camera(c) = &mut n.shape {
        if c.ortho {
            c.ortho_width = radius * 2.0 * 1.2 * aspect.max(1.0);
            let d = c.coi;
            n.xf.t = (center - fwd * d).to_array();
        } else {
            let fov = c.fov_y(aspect).min(2.0 * ((c.fov_y(aspect) / 2.0).tan() * aspect).atan());
            let d = radius / (fov / 2.0).sin() * 1.05;
            c.coi = d;
            n.xf.t = (center - fwd * d).to_array();
        }
    }
    s.rev += 1;
}

// ------------------------------------------------------------------ display meshes

fn to_f32(p: DVec3) -> [f32; 3] {
    [p.x as f32, p.y as f32, p.z as f32]
}

fn hash_f(h: &mut DefaultHasher, x: f32) {
    x.to_bits().hash(h);
}

/// The mesh shown in the viewport (smooth mesh preview applied).
pub fn display_mesh(s: &Scene, id: &str, cache: &mut std::collections::HashMap<String, (u64, PolyMesh)>) -> Option<PolyMesh> {
    let n = s.node(id)?;
    let m = n.mesh()?;
    if m.smooth_level == 0 {
        return Some(m.mesh.clone());
    }
    let key = n.rev * 4 + m.smooth_level as u64;
    if let Some((k, pm)) = cache.get(id) {
        if *k == key {
            return Some(pm.clone());
        }
    }
    let pm = inca_core::mesh::subdivide(&m.mesh, m.smooth_level.min(3), true);
    cache.insert(id.to_string(), (key, pm.clone()));
    Some(pm)
}

pub fn build_mesh_buffers(pm: &PolyMesh, color_of: &dyn Fn(Option<&String>) -> [f32; 3]) -> (Vec<MeshVertex>, Vec<LineVertex>) {
    let normals = pm.corner_normals();
    let mut tris = Vec::with_capacity(pm.tri_count() * 3);
    for fi in 0..pm.f.len() {
        let f = &pm.f[fi];
        let col = color_of(pm.face_mat(fi));
        for t in pm.triangulate_face(fi) {
            for &c in &t {
                tris.push(MeshVertex { pos: to_f32(pm.v[f[c] as usize]), normal: to_f32(normals[fi][c]), color: col });
            }
        }
    }
    let mut wire = vec![];
    let col = [1.0, 1.0, 1.0, 1.0];
    match &pm.display_edges {
        Some(de) => {
            for e in de {
                wire.push(LineVertex { pos: to_f32(pm.v[e[0] as usize]), color: col, ..Default::default() });
                wire.push(LineVertex { pos: to_f32(pm.v[e[1] as usize]), color: col, ..Default::default() });
            }
        }
        None => {
            // each edge carries the normals of its (up to two) faces so the shader can hide
            // wires of back-facing faces, like the JS `cullA` / `cullB`
            let topo = pm.topo();
            let fnorm: Vec<DVec3> = (0..pm.f.len()).map(|i| pm.face_normal(i)).collect();
            for &(a, b) in &topo.edges {
                let (na, nb) = edge_normals(&topo, &fnorm, a, b);
                for &v in &[a, b] {
                    wire.push(LineVertex { pos: to_f32(pm.v[v as usize]), color: col, normal: to_f32(na), normal_b: to_f32(nb) });
                }
            }
        }
    }
    (tris, wire)
}

/// Grid on the ground plane (or facing an ortho camera).
pub fn grid_lines(cv: &CamView, cfg: (f64, f64, i32)) -> Vec<LineVertex> {
    let mut out = vec![];
    // JS makeGrid colours
    let (sub, maj, axis) = (hex(0x5c5c5c), hex(0x4d4d4d), hex(0x1e1e1e));
    let (size, step, major) = cfg;
    // ortho cameras get a grid in their view plane
    let (u, v) = if cv.ortho {
        let f = cv.fwd.abs();
        if f.y > 0.9 {
            (DVec3::X, DVec3::Z)
        } else if f.z > 0.9 {
            (DVec3::X, DVec3::Y)
        } else {
            (DVec3::Z, DVec3::Y)
        }
    } else {
        (DVec3::X, DVec3::Z)
    };
    let n = ((size / step) as i32).min(2000);
    for i in -n..=n {
        let c = if i == 0 { axis } else if i % major.max(1) == 0 { maj } else { sub };
        let k = i as f64 * step;
        for (a, b) in [(u * k - v * size, u * k + v * size), (v * k - u * size, v * k + u * size)] {
            out.push(LineVertex { pos: to_f32(a), color: c, ..Default::default() });
            out.push(LineVertex { pos: to_f32(b), color: c, ..Default::default() });
        }
    }
    out
}

fn push_line(out: &mut Vec<LineVertex>, a: DVec3, b: DVec3, c: [f32; 4]) {
    out.push(LineVertex { pos: to_f32(a), color: c, ..Default::default() });
    out.push(LineVertex { pos: to_f32(b), color: c, ..Default::default() });
}

/// A line that participates in backface culling: `na`/`nb` are the normals of the (up to two)
/// faces sharing the edge, as the JS `cullA`/`cullB`.
fn push_line_n(out: &mut Vec<LineVertex>, a: DVec3, b: DVec3, c: [f32; 4], na: DVec3, nb: DVec3) {
    out.push(LineVertex { pos: to_f32(a), color: c, normal: to_f32(na), normal_b: to_f32(nb) });
    out.push(LineVertex { pos: to_f32(b), color: c, normal: to_f32(na), normal_b: to_f32(nb) });
}

/// The (up to two) face normals of one edge of a mesh, zero-filled when absent. Matches the
/// JS `edgeCullNormals`: a single face duplicates its normal, more than two mean never culled.
fn edge_normals(topo: &Topo, fnorm: &[DVec3], a: u32, b: u32) -> (DVec3, DVec3) {
    let fs = topo.edge(a, b).map_or(&[][..], |ei| topo.edge_faces[ei].as_slice());
    match fs {
        [f] => (fnorm[*f], fnorm[*f]),
        [f, g] => (fnorm[*f], fnorm[*g]),
        _ => (DVec3::ZERO, DVec3::ZERO),
    }
}

fn circle(out: &mut Vec<LineVertex>, c: DVec3, u: DVec3, v: DVec3, r: f64, col: [f32; 4]) {
    let n = 24;
    for i in 0..n {
        let a0 = std::f64::consts::TAU * i as f64 / n as f64;
        let a1 = std::f64::consts::TAU * (i + 1) as f64 / n as f64;
        push_line(out, c + (u * a0.cos() + v * a0.sin()) * r, c + (u * a1.cos() + v * a1.sin()) * r, col);
    }
}

/// s/t/u divisions of a lattice node.
pub fn lattice_div(s: &Scene, id: &str) -> [usize; 3] {
    ["sDivisions", "tDivisions", "uDivisions"].map(|k| s.extra_f(id, k, 2.0).round().max(2.0) as usize)
}

/// Object-space icon lines for lights, cameras, locators and curves.
pub fn icon_lines(s: &Scene, id: &str, col: [f32; 4]) -> Vec<LineVertex> {
    let mut o = vec![];
    let Some(n) = s.node(id) else { return o };
    match &n.shape {
        Shape::Locator if n.kind == "joint" => {
            let r = n.passthrough.get("rig").and_then(|r| r["radius"].as_f64()).unwrap_or(0.5) * 0.5;
            circle(&mut o, DVec3::ZERO, DVec3::X, DVec3::Y, r, col);
            circle(&mut o, DVec3::ZERO, DVec3::Y, DVec3::Z, r, col);
            circle(&mut o, DVec3::ZERO, DVec3::Z, DVec3::X, r, col);
        }
        Shape::Locator if n.kind == "emitter" && n.extra["emitterType"].as_str() == Some("particles") => {
            // particle object: a small diamond at its pivot
            let d = [DVec3::X, DVec3::Z, -DVec3::X, -DVec3::Z].map(|v| v * 0.25);
            for i in 0..4 {
                push_line(&mut o, d[i], d[(i + 1) % 4], col);
                push_line(&mut o, d[i], DVec3::Y * 0.25, col);
                push_line(&mut o, d[i], -DVec3::Y * 0.25, col);
            }
        }
        Shape::Locator if n.kind == "emitter" => {
            for ax in [DVec3::X, DVec3::Y, DVec3::Z, DVec3::new(1.0, 1.0, 0.0).normalize(), DVec3::new(0.0, 1.0, 1.0).normalize(), DVec3::new(1.0, 0.0, 1.0).normalize()] {
                push_line(&mut o, -ax * 0.4, ax * 0.4, col);
            }
            circle(&mut o, DVec3::ZERO, DVec3::X, DVec3::Z, 0.25, col);
        }
        Shape::Locator if n.kind == "field" => {
            circle(&mut o, DVec3::ZERO, DVec3::X, DVec3::Z, 0.5, col);
            let d = DVec3::new(n.extra["directionX"].as_f64().unwrap_or(0.0), n.extra["directionY"].as_f64().unwrap_or(-1.0), n.extra["directionZ"].as_f64().unwrap_or(0.0)).normalize_or(-DVec3::Y);
            push_line(&mut o, DVec3::ZERO, d, col);
            let s = d.any_orthonormal_vector() * 0.15;
            push_line(&mut o, d, d * 0.75 + s, col);
            push_line(&mut o, d, d * 0.75 - s, col);
        }
        Shape::Locator if n.kind == "deformer" => {
            push_line(&mut o, -DVec3::Y, DVec3::Y, col);
            for y in [-1.0, 0.0, 1.0] {
                circle(&mut o, DVec3::Y * y, DVec3::X, DVec3::Z, 0.5, col);
            }
        }
        Shape::Locator if n.kind == "distanceDimension" => {
            if let Some((a, b, _)) = s.distance_of(id) {
                let inv = s.world_matrix(id).inverse();
                let (a, b) = (inv.transform_point3(a), inv.transform_point3(b));
                push_line(&mut o, a, b, col);
                let t = (b - a).normalize_or_zero().any_orthonormal_vector() * 0.1;
                for p in [a, b] {
                    push_line(&mut o, p - t, p + t, col);
                }
            }
        }
        Shape::Locator if n.kind == "ikHandle" => {
            for ax in [DVec3::X, DVec3::Y, DVec3::Z] {
                push_line(&mut o, -ax * 0.3, ax * 0.3, col);
            }
            // a line back to the start joint
            if let Some(sj) = s.rig(id)["start"].as_str() {
                let p = s.world_matrix(id).inverse().transform_point3(s.world_pos(sj));
                push_line(&mut o, DVec3::ZERO, p, col);
            }
        }
        Shape::Locator if n.kind == "clusterHandle" => {
            // the "C" glyph
            let n = 16;
            for i in 0..n {
                let a0 = 0.8 + (std::f64::consts::TAU - 1.6) * i as f64 / n as f64;
                let a1 = 0.8 + (std::f64::consts::TAU - 1.6) * (i + 1) as f64 / n as f64;
                push_line(&mut o, DVec3::new(a0.cos(), a0.sin(), 0.0) * 0.4, DVec3::new(a1.cos(), a1.sin(), 0.0) * 0.4, col);
            }
        }
        Shape::Locator if n.kind == "lattice" => {
            let div = lattice_div(s, id);
            let pts: Vec<DVec3> = s.rig(id)["pts"].as_array().map(|a| a.iter().map(|p| DVec3::new(p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0), p[2].as_f64().unwrap_or(0.0))).collect()).unwrap_or_default();
            if pts.len() == div[0] * div[1] * div[2] {
                let ix = |i, j, k| pts[inca_core::deformers::lattice_index(div, i, j, k)];
                for i in 0..div[0] {
                    for j in 0..div[1] {
                        for k in 0..div[2] {
                            if i + 1 < div[0] {
                                push_line(&mut o, ix(i, j, k), ix(i + 1, j, k), col);
                            }
                            if j + 1 < div[1] {
                                push_line(&mut o, ix(i, j, k), ix(i, j + 1, k), col);
                            }
                            if k + 1 < div[2] {
                                push_line(&mut o, ix(i, j, k), ix(i, j, k + 1), col);
                            }
                        }
                    }
                }
            }
        }
        Shape::Locator => {
            for ax in [DVec3::X, DVec3::Y, DVec3::Z] {
                push_line(&mut o, -ax, ax, col);
            }
        }
        Shape::Curve(c) => {
            let pts = c.sample(12);
            for w in pts.windows(2) {
                push_line(&mut o, w[0], w[1], col);
            }
        }
        Shape::Light(l) => {
            let z = DVec3::ZERO;
            match l.ty.as_str() {
                "directionalLight" => {
                    for dx in [-0.3, 0.0, 0.3] {
                        let a = DVec3::new(dx, 0.0, 0.0);
                        push_line(&mut o, a, a - DVec3::Z * 1.5, col);
                        push_line(&mut o, a - DVec3::Z * 1.5, a - DVec3::Z * 1.2 + DVec3::Y * 0.15, col);
                    }
                }
                "spotLight" => {
                    let ang = l.attrs.get("coneAngle").and_then(|v| v.as_f64()).unwrap_or(40.0).to_radians() / 2.0;
                    let r = ang.tan() * 1.5;
                    circle(&mut o, -DVec3::Z * 1.5, DVec3::X, DVec3::Y, r, col);
                    for k in 0..4 {
                        let a = k as f64 * std::f64::consts::FRAC_PI_2;
                        push_line(&mut o, z, -DVec3::Z * 1.5 + (DVec3::X * a.cos() + DVec3::Y * a.sin()) * r, col);
                    }
                }
                "areaLight" => {
                    let c = [DVec3::new(-1.0, -1.0, 0.0), DVec3::new(1.0, -1.0, 0.0), DVec3::new(1.0, 1.0, 0.0), DVec3::new(-1.0, 1.0, 0.0)];
                    for i in 0..4 {
                        push_line(&mut o, c[i], c[(i + 1) % 4], col);
                    }
                    push_line(&mut o, z, -DVec3::Z, col);
                }
                "ambientLight" | "skyDomeLight" => {
                    circle(&mut o, z, DVec3::X, DVec3::Y, 0.5, col);
                    circle(&mut o, z, DVec3::X, DVec3::Z, 0.5, col);
                }
                _ => {
                    for ax in [DVec3::X, DVec3::Y, DVec3::Z] {
                        push_line(&mut o, -ax * 0.4, ax * 0.4, col);
                    }
                    circle(&mut o, z, DVec3::X, DVec3::Y, 0.3, col);
                    circle(&mut o, z, DVec3::Y, DVec3::Z, 0.3, col);
                }
            }
        }
        Shape::Camera(c) if !c.startup => {
            let b = [DVec3::new(-0.4, -0.3, 0.6), DVec3::new(0.4, -0.3, 0.6), DVec3::new(0.4, 0.3, 0.6), DVec3::new(-0.4, 0.3, 0.6), DVec3::new(-0.4, -0.3, -0.4), DVec3::new(0.4, -0.3, -0.4), DVec3::new(0.4, 0.3, -0.4), DVec3::new(-0.4, 0.3, -0.4)];
            for i in 0..4 {
                push_line(&mut o, b[i], b[(i + 1) % 4], col);
                push_line(&mut o, b[i + 4], b[(i + 1) % 4 + 4], col);
                push_line(&mut o, b[i], b[i + 4], col);
            }
            let lens = [DVec3::new(-0.2, -0.2, -0.4), DVec3::new(0.2, -0.2, -0.4), DVec3::new(0.2, 0.2, -0.4), DVec3::new(-0.2, 0.2, -0.4)];
            for i in 0..4 {
                push_line(&mut o, lens[i], lens[i] * DVec3::new(1.6, 1.6, 2.0), col);
            }
        }
        _ => {}
    }
    o
}

pub fn mat_color(s: &Scene, mid: &str) -> [f32; 3] {
    match s.mats.get(mid) {
        Some(m) => {
            let p = m.pbr();
            // texture-mapped colour: show the texture's average tone
            if let Some(tid) = m.maps.get(m.color_attr()).and_then(|v| v.as_str()) {
                if let Some(t) = s.texs.get(tid) {
                    if let Some(c) = t.sample(0.25, 0.25) {
                        return c;
                    }
                }
            }
            if p.unlit {
                p.base
            } else {
                [p.base[0].max(p.emission[0]), p.base[1].max(p.emission[1]), p.base[2].max(p.emission[2])]
            }
        }
        None => [0.4, 0.4, 0.4],
    }
}

/// Key for the GPU cache of a mesh node: geometry revision, smoothing and material colours.
pub fn mesh_key(s: &Scene, id: &str) -> u64 {
    let mut h = DefaultHasher::new();
    let Some(n) = s.node(id) else { return 0 };
    n.rev.hash(&mut h);
    if let Some(m) = n.mesh() {
        m.smooth_level.hash(&mut h);
        m.material.hash(&mut h);
        let mut mats: Vec<&String> = vec![&m.material];
        if let Some(fm) = &m.mesh.fm {
            mats.extend(fm.iter().flatten());
        }
        mats.sort();
        mats.dedup();
        for mid in mats {
            for c in mat_color(s, mid) {
                hash_f(&mut h, c);
            }
        }
    }
    h.finish()
}

/// Cache key for a mesh's component dots: mode, geometry, the relevant selection and
/// soft-selection settings. Deliberately not `Scene::rev`: camera moves, object transforms and
/// animation bump that every frame but leave the dots (which live in object space) unchanged.
fn points_key(s: &Scene, id: &str, mode: CompMode, soft: Option<(f64, bool)>) -> u64 {
    let mut h = DefaultHasher::new();
    if let Some(n) = s.node(id) {
        n.rev.hash(&mut h);
    }
    mode.hash(&mut h);
    if let Some(cs) = s.comp.get(id) {
        match mode {
            CompMode::Vertex => {
                cs.v.len().hash(&mut h);
                for v in &cs.v {
                    v.hash(&mut h);
                }
            }
            CompMode::Face => {
                cs.f.len().hash(&mut h);
                for f in &cs.f {
                    f.hash(&mut h);
                }
            }
            CompMode::Edge => {}
        }
    }
    soft.map(|(r, lin)| (r.to_bits(), lin)).hash(&mut h);
    h.finish()
}

/// One dot per vertex face corner, coloured like the JS overlay (selected, soft falloff,
/// plain). Duplicating the dot per adjacent face normal lets backface culling hide a dot
/// only when every adjacent face is turned away, which one dot per vertex cannot express.
fn component_points(s: &Scene, id: &str, soft: Option<(f64, bool)>, mesh_cache: &mut MeshCache) -> Vec<PointVertex> {
    let Some(pm) = s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { return vec![] };
    let cs = s.comp_sel(id);
    let weights: HashMap<u32, f64> = match soft {
        Some((r, lin)) if !cs.v.is_empty() => soft_weights(s, id, &cs.v.iter().copied().collect::<Vec<_>>(), r, lin).into_iter().collect(),
        _ => HashMap::new(),
    };
    let topo = mesh_cache.topo(s, id);
    let geo = mesh_cache.faces(s, id);
    let mut pts = Vec::with_capacity(pm.v.len());
    for (i, p) in pm.v.iter().enumerate() {
        let i = i as u32;
        let color = if cs.v.contains(&i) {
            [1.0, 230.0 / 255.0, 0.0, 1.0]
        } else if let Some(&w) = weights.get(&i) {
            let c = falloff_color(w);
            [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
        } else {
            [194.0 / 255.0, 58.0 / 255.0, 214.0 / 255.0, 1.0]
        };
        let fs = topo.as_ref().map(|t| &t.vert_faces[i as usize]);
        let ns = match (fs, geo.as_ref()) {
            (Some(fs), Some(geo)) if !fs.is_empty() => fs.iter().map(|&f| to_f32(geo.normals[f].normalize_or_zero())).collect::<Vec<_>>(),
            // no topology (or an isolated vertex): a zero normal is never culled
            _ => vec![[0.0; 3]],
        };
        for n in ns {
            pts.push(PointVertex { pos: to_f32(*p), color, normal: n });
        }
    }
    pts
}

/// One dot per face centre, as the JS face-mode overlay: selected faces orange, the rest blue.
/// The face normal lets the shader cull dots of back-facing faces.
fn face_center_points(s: &Scene, id: &str, geo: &FaceGeo) -> Vec<PointVertex> {
    let cs = s.comp_sel(id);
    let mut pts = Vec::with_capacity(geo.centers.len());
    for (fi, (&c, &n)) in geo.centers.iter().zip(&geo.normals).enumerate() {
        let color = if cs.f.contains(&fi) {
            [C_FACE_SEL[0], C_FACE_SEL[1], C_FACE_SEL[2], 1.0]
        } else {
            [C_DOT[0], C_DOT[1], C_DOT[2], 1.0]
        };
        pts.push(PointVertex { pos: to_f32(c), color, normal: to_f32(n.normalize_or_zero()) });
    }
    pts
}

/// Background colours (top, bottom) per Alt+B mode, as the JS `BG` table:
/// grey, black, dark grey, light grey, gradient.
pub const BG_MODES: [&str; 5] = ["Gray", "Black", "Dark Gray", "Light Gray", "Gradient"];

fn hex(c: u32) -> [f32; 4] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0]
}

pub fn background(mode: usize) -> ([f32; 4], [f32; 4]) {
    match mode {
        1 => (hex(0x000000), hex(0x000000)),
        2 => (hex(0x3a3a3a), hex(0x3a3a3a)),
        3 => (hex(0xa0a0a0), hex(0xa0a0a0)),
        4 => (hex(0x8a8f96), hex(0x3d3f42)),
        _ => (hex(0x6b6b6b), hex(0x6b6b6b)),
    }
}

pub fn frame_uniform(s: &Scene, vp: &Viewport, cv: &CamView, bg_mode: usize) -> FrameUniform {
    let (bg_top, bg_bottom) = background(bg_mode);
    let m4 = |m: DMat4| m.as_mat4().to_cols_array_2d();
    let mut lights = [GpuLight { pos: [0.0; 4], dir: [0.0; 4], color: [0.0; 4], params: [0.0; 4] }; 8];
    let mut count = 0;
    let light_mode = match vp.lights {
        LightMode::Default => 0.0,
        LightMode::All => 1.0,
        LightMode::Flat => 2.0,
        LightMode::None => 3.0,
    };
    if vp.lights == LightMode::All {
        for n in s.nodes.values() {
            if count >= 8 {
                break;
            }
            let Shape::Light(l) = &n.shape else { continue };
            if !s.is_visible(&n.id) {
                continue;
            }
            let wm = s.world_matrix(&n.id);
            let pos = wm.transform_point3(DVec3::ZERO);
            let dir = wm.transform_vector3(-DVec3::Z).normalize();
            let col = l.attrs.get("color").and_then(|c| c.as_array()).map(|a| [a[0].as_f64().unwrap_or(1.0), a[1].as_f64().unwrap_or(1.0), a[2].as_f64().unwrap_or(1.0)]).unwrap_or([1.0; 3]);
            let mut i = l.attrs.get("intensity").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let ty = match l.ty.as_str() {
                "ambientLight" => 0.0,
                "directionalLight" => 1.0,
                "pointLight" => 2.0,
                "spotLight" => 3.0,
                "areaLight" => 4.0,
                _ => {
                    i *= 0.5;
                    0.0
                }
            };
            let cone = l.attrs.get("coneAngle").and_then(|v| v.as_f64()).unwrap_or(40.0).to_radians() / 2.0;
            let pen = l.attrs.get("penumbraAngle").and_then(|v| v.as_f64()).unwrap_or(0.0).to_radians();
            let outer = (cone + pen.max(0.0)).cos();
            let inner = (cone + pen.min(0.0)).cos().max(outer + 1e-4);
            let decay = l.attrs.get("decayRate").and_then(|v| v.as_f64()).unwrap_or(0.0);
            lights[count] = GpuLight {
                pos: [pos.x as f32, pos.y as f32, pos.z as f32, ty],
                dir: [dir.x as f32, dir.y as f32, dir.z as f32, 0.0],
                color: [(col[0] * i) as f32, (col[1] * i) as f32, (col[2] * i) as f32, 1.0],
                params: [outer as f32, inner as f32, decay as f32, 0.0],
            };
            count += 1;
        }
    }
    FrameUniform {
        view_proj: m4(cv.view_proj()),
        view: m4(cv.view),
        cam_pos: [cv.eye.x as f32, cv.eye.y as f32, cv.eye.z as f32, if cv.ortho { 1.0 } else { 0.0 }],
        cam_dir: [cv.fwd.x as f32, cv.fwd.y as f32, cv.fwd.z as f32, 0.0],
        bg_top,
        bg_bottom,
        mode: [light_mode, count as f32, if vp.xray { 1.0 } else { 0.0 }, if vp.backface { 1.0 } else { 0.0 }],
        post: [vp.exposure, vp.gamma.max(0.05), cv.size[0].max(1.0), cv.size[1].max(1.0)],
        lights,
    }
}

/// Draw list for one viewport; uploads changed meshes.
pub fn build_draws(
    s: &Scene,
    vp: &Viewport,
    cv: &CamView,
    g: &mut Gpu,
    smooth_cache: &mut std::collections::HashMap<String, (u64, PolyMesh)>,
    live: &std::collections::HashSet<String>,
    mesh_cache: &mut MeshCache,
    soft: Option<(f64, bool)>,
) -> Vec<DrawCmd> {
    let mut draws = vec![];
    if vp.grid {
        // drawn first without depth so any geometry (e.g. a ground plane) covers it
        draws.push(DrawCmd { pass: Pass::LinesUnder, geo: Geo::TransientLines(grid_lines(cv, vp.grid_cfg)), u: DrawUniform::new(Mat4::IDENTITY) });
    }
    let hilite = s.hilite();
    let lead = s.lead().cloned();
    let affected = if vp.show_sel_highlight && !s.sel.is_empty() && !vp.beauty { s.affected_by(&s.sel) } else { Default::default() };
    for id in s.all_dag(false) {
        if !s.is_visible(&id) {
            continue;
        }
        if let Some(iso) = &vp.isolate {
            if !iso.iter().any(|r| r == &id || s.is_ancestor(r, &id)) {
                continue;
            }
        }
        let n = s.node(&id).unwrap();
        if !vp.shows(n) || (vp.beauty && !matches!(n.shape, Shape::Mesh(_))) {
            continue;
        }
        let wm = s.world_matrix(&id).as_mat4();
        let selected = vp.show_sel_highlight && s.sel.iter().any(|x| x == &id || s.is_ancestor(x, &id));
        let is_lead = vp.show_sel_highlight && lead.as_ref().is_some_and(|l| l == &id || s.is_ancestor(l, &id));
        let template = s.is_templated_display(&id);
        let is_affected = !selected && affected.contains(&id);
        let sel_col = if template && selected { C_TEMPLATE_SEL } else if template { C_TEMPLATE } else if is_lead { C_LEAD } else if selected { C_SEL } else if is_affected { C_AFFECTED } else { C_WIRE };
        match &n.shape {
            Shape::Mesh(m) => {
                let key = mesh_key(s, &id);
                if g.mesh_key(&id) != Some(key) || live.contains(&id) {
                    if let Some(pm) = display_mesh(s, &id, smooth_cache) {
                        let (t, w) = build_mesh_buffers(&pm, &|fm| mat_color(s, fm.map(|x| x.as_str()).unwrap_or(&m.material)));
                        g.upload_mesh(&id, if live.contains(&id) { 0 } else { key }, &t, &w);
                    }
                }
                let pbr = s.mats.get(&m.material).map(|x| x.pbr()).unwrap_or_default();
                let in_comp = hilite.contains(&id) && !vp.beauty;
                // templates draw as wireframe only, whatever the shading mode
                if vp.shade != ShadeMode::Wire && !template {
                    let mut u = DrawUniform::new(wm).spec(pbr.specular, pbr.roughness);
                    if pbr.unlit {
                        u = u.unlit();
                    }
                    if vp.xray || pbr.opacity < 0.99 {
                        let a = if vp.xray { 0.45 } else { pbr.opacity.max(0.1) };
                        draws.push(DrawCmd { pass: Pass::MeshBlend, geo: Geo::MeshTris(id.clone()), u: u.alpha(a) });
                    } else {
                        draws.push(DrawCmd { pass: Pass::Mesh, geo: Geo::MeshTris(id.clone()), u });
                    }
                }
                let show_wire = !vp.beauty && (vp.shade == ShadeMode::Wire || vp.wire_on_shaded || selected || in_comp || is_affected || template);
                if show_wire {
                    let c = if in_comp {
                        C_HILITE
                    } else if vp.shade == ShadeMode::Wire || selected || template || is_affected {
                        sel_col
                    } else {
                        [0.0, 0.0, 0.0, 1.0]
                    };
                    // wires of back-facing faces are culled in the shader from the cached
                    // per-edge normals (the JS `cullable`), so no per-frame Edge rebuild
                    draws.push(DrawCmd {
                        pass: Pass::Lines,
                        geo: Geo::MeshWire(id.clone()),
                        u: DrawUniform::new(wm).tint([c[0], c[1], c[2]], 1.0).bias(0.00012).cull_wire(vp.backface),
                    });
                }
                if in_comp {
                    match s.comp_mode {
                        Some(CompMode::Vertex) => {
                            // dots live on the GPU (cached by geometry + selection): drawing them
                            // with egui every frame is what made component mode unusable on heavy meshes
                            let key = points_key(s, &id, CompMode::Vertex, soft);
                            if g.point_key(&id) != Some(key) {
                                let pts = component_points(s, &id, soft, mesh_cache);
                                g.upload_points(&id, key, &pts);
                            }
                            draws.push(DrawCmd { pass: Pass::Points, geo: Geo::MeshPoints(id.clone()), u: DrawUniform::new(wm).point_size(3.0).bias(0.0003).cull_wire(vp.backface) });
                        }
                        Some(CompMode::Face) => {
                            // the JS face mode shows a dot on every face centre, not just the selection
                            if let Some(geo) = mesh_cache.faces(s, &id) {
                                let key = points_key(s, &id, CompMode::Face, soft);
                                if g.point_key(&id) != Some(key) {
                                    let pts = face_center_points(s, &id, &geo);
                                    g.upload_points(&id, key, &pts);
                                }
                                draws.push(DrawCmd { pass: Pass::Points, geo: Geo::MeshPoints(id.clone()), u: DrawUniform::new(wm).point_size(2.0).bias(0.0003).cull_wire(vp.backface) });
                            }
                        }
                        _ => {}
                    }
                }
                if in_comp && s.comp_mode == Some(CompMode::Face) {
                    if s.comp.get(&id).is_some_and(|c| !c.f.is_empty()) {
                        // cached by geometry + selected faces: rebuilding this list every frame
                        // made large face selections crawl (100k faces was ~44 ms/frame)
                        if let Some(key) = mesh_cache.comp_key(s, &id, CompMode::Face) {
                            if g.comp_tri_key(&id) != Some(key) {
                                let cs = s.comp_sel(&id);
                                let mut tris = vec![];
                                let pm = &m.mesh;
                                for &fi in &cs.f {
                                    let Some(f) = pm.f.get(fi) else { continue };
                                    for t in pm.triangulate_face(fi) {
                                        for &c in &t {
                                            tris.push(MeshVertex { pos: to_f32(pm.v[f[c] as usize]), normal: [0.0, 1.0, 0.0], color: C_FACE_SEL });
                                        }
                                    }
                                }
                                g.upload_comp_tris(&id, key, &tris);
                            }
                            draws.push(DrawCmd { pass: Pass::MeshBlend, geo: Geo::CompTris(id.clone()), u: DrawUniform::new(wm).unlit().alpha(0.45).bias(0.0002) });
                        }
                    }
                }
                if in_comp && s.comp_mode == Some(CompMode::Edge) {
                    if s.comp.get(&id).is_some_and(|c| !c.e.is_empty()) {
                        // same for the selected-edge overlay (200k edges was ~110 ms/frame)
                        if let Some(key) = mesh_cache.comp_key(s, &id, CompMode::Edge) {
                            if g.comp_line_key(&id) != Some(key) {
                                let cs = s.comp_sel(&id);
                                let pm = &m.mesh;
                                let topo = mesh_cache.topo(s, &id);
                                let fnorm: Vec<DVec3> = topo.as_ref().map_or_else(Vec::new, |_| (0..pm.f.len()).map(|f| pm.face_normal(f)).collect());
                                let mut l = vec![];
                                for &(a, b) in &cs.e {
                                    if (a as usize) < pm.v.len() && (b as usize) < pm.v.len() {
                                        let (na, nb) = topo.as_ref().map_or((DVec3::ZERO, DVec3::ZERO), |t| edge_normals(t, &fnorm, a, b));
                                        push_line_n(&mut l, pm.v[a as usize], pm.v[b as usize], [1.0, 0.54, 0.12, 1.0], na, nb);
                                    }
                                }
                                g.upload_comp_lines(&id, key, &l);
                            }
                            draws.push(DrawCmd { pass: Pass::Lines, geo: Geo::CompLines(id.clone()), u: DrawUniform::new(wm).bias(0.0004).cull_wire(vp.backface) });
                        }
                    }
                }
            }
            Shape::Light(_) => {
                let c = if selected || is_affected { sel_col } else { [0.9, 0.85, 0.3, 1.0] };
                draws.push(DrawCmd { pass: Pass::Lines, geo: Geo::TransientLines(icon_lines(s, &id, c)), u: DrawUniform::new(wm) });
            }
            Shape::Camera(_) => {
                let c = if selected || is_affected { sel_col } else { C_ICON };
                draws.push(DrawCmd { pass: Pass::Lines, geo: Geo::TransientLines(icon_lines(s, &id, c)), u: DrawUniform::new(wm) });
            }
            Shape::Curve(_) => {
                let c = if selected || is_affected { sel_col } else if template { C_TEMPLATE } else { C_CURVE };
                draws.push(DrawCmd { pass: Pass::Lines, geo: Geo::TransientLines(icon_lines(s, &id, c)), u: DrawUniform::new(wm) });
            }
            Shape::Locator => {
                let c = if selected || is_affected {
                    sel_col
                } else if n.kind == "joint" {
                    [0.1, 0.35, 0.9, 1.0]
                } else if n.kind == "deformer" {
                    [0.75, 0.3, 0.85, 1.0]
                } else if n.kind == "emitter" || n.kind == "field" {
                    [0.95, 0.6, 0.2, 1.0]
                } else if n.kind == "ikHandle" {
                    [0.85, 0.75, 0.2, 1.0]
                } else if n.kind == "lattice" || n.kind == "clusterHandle" {
                    [0.75, 0.3, 0.85, 1.0]
                } else {
                    C_WIRE
                };
                draws.push(DrawCmd { pass: Pass::LinesTop, geo: Geo::TransientLines(icon_lines(s, &id, c)), u: DrawUniform::new(wm) });
                if n.kind == "joint" {
                    // bones: a thin pyramid from the joint to each child joint (world space)
                    let a = wm.as_dmat4().transform_point3(DVec3::ZERO);
                    let mut l = vec![];
                    for k in n.children.iter().filter(|k| s.node(k).is_some_and(|c| c.kind == "joint")) {
                        let b = s.world_matrix(k).transform_point3(DVec3::ZERO);
                        let d = b - a;
                        let len = d.length();
                        if len < 1e-6 {
                            continue;
                        }
                        let u = d.any_orthonormal_vector() * len * 0.08;
                        let v = d.normalize().cross(u);
                        let base = a + d * 0.15;
                        for corner in [u, v, -u, -v] {
                            push_line(&mut l, a, base + corner, c);
                            push_line(&mut l, base + corner, b, c);
                        }
                    }
                    draws.push(DrawCmd { pass: Pass::LinesTop, geo: Geo::TransientLines(l), u: DrawUniform::new(Mat4::IDENTITY) });
                }
            }
            _ => {}
        }
    }
    draws
}

// ------------------------------------------------------------------ picking

/// Nearest object under the cursor.
pub fn pick_object(s: &Scene, vp: &Viewport, cv: &CamView, pos: Pos2, mesh_cache: &mut MeshCache) -> Option<Id> {
    let ray = cv.ray(pos, vp.rect);
    let mut best: Option<(f64, Id)> = None;
    for id in s.all_dag(false) {
        if !s.is_visible(&id) || s.is_template(&id) {
            continue;
        }
        if let Some(iso) = &vp.isolate {
            if !iso.iter().any(|r| r == &id || s.is_ancestor(r, &id)) {
                continue;
            }
        }
        let n = s.node(&id).unwrap();
        if !vp.shows(n) {
            continue;
        }
        let wm = s.world_matrix(&id);
        match &n.shape {
            Shape::Mesh(m) => {
                let lr = ray.transformed(&wm.inverse());
                let bb = m.mesh.bbox();
                if inca_core::math::ray_aabb(&lr, &bb).is_none() {
                    continue;
                }
                // the cached BVH makes this a few microseconds even at 400k faces
                let hit = match mesh_cache.bvh(s, &id) {
                    Some(b) => b.raycast(&m.mesh, &lr, vp.backface),
                    None => m.mesh.raycast(&lr, vp.backface),
                };
                if let Some((t, _)) = hit {
                    let wp = wm.transform_point3(lr.at(t));
                    let d = cv.depth(wp);
                    if best.as_ref().is_none_or(|b| d < b.0) {
                        best = Some((d, id.clone()));
                    }
                }
            }
            Shape::Group => {}
            // the JS pick intersects the sampled display line (`ud.line`, 16 per span) and only
            // wins ties by 0.001: a mesh in front still takes the pick
            Shape::Curve(c) => {
                let pts: Vec<DVec3> = c.sample(16).iter().map(|p| wm.transform_point3(*p)).collect();
                for seg in pts.windows(2) {
                    let (dist, t) = ray_seg_closest(&ray, seg[0], seg[1]);
                    if dist < 1.0 {
                        let d = cv.depth(seg[0].lerp(seg[1], t)) - 0.001;
                        if best.as_ref().is_none_or(|x| d < x.0) {
                            best = Some((d, id.clone()));
                        }
                    }
                }
            }
            Shape::Camera(c) if c.startup => {}
            _ => {
                // icon objects: screen distance to their lines
                let lines = icon_lines(s, &id, [1.0; 4]);
                for seg in lines.chunks(2) {
                    let a = wm.transform_point3(DVec3::new(seg[0].pos[0] as f64, seg[0].pos[1] as f64, seg[0].pos[2] as f64));
                    let b = wm.transform_point3(DVec3::new(seg[1].pos[0] as f64, seg[1].pos[1] as f64, seg[1].pos[2] as f64));
                    if let (Some(pa), Some(pb)) = (cv.project(a, vp.rect), cv.project(b, vp.rect)) {
                        if seg_dist(pos, pa, pb) < 6.0 {
                            let d = cv.depth(a) - 1e6; // icons win over meshes behind them
                            if best.as_ref().is_none_or(|x| d < x.0) {
                                best = Some((d, id.clone()));
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// Distance from `p` to the segment `ab` and the closest parameter along it.
fn seg_closest(p: Pos2, a: Pos2, b: Pos2) -> (f32, f32) {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    ((a + ab * t).distance(p), t)
}

/// Closest distance between a ray and a segment (world space) and the segment parameter.
fn ray_seg_closest(r: &Ray, a: DVec3, b: DVec3) -> (f64, f64) {
    let u = b - a;
    let w0 = r.origin - a;
    let (uu, ud, uw, c) = (u.length_squared(), u.dot(r.dir), u.dot(w0), r.dir.dot(w0));
    // s = ud*t - c along the ray, t along the segment (Ericson's segment/segment closest)
    let mut t = if uu < 1e-12 { 0.0 } else { (ud * uw - c * uu) / (uu - ud * ud).max(1e-12) };
    t = t.clamp(0.0, 1.0);
    let mut ray_t = ud * t - c;
    if ray_t < 0.0 {
        // the closest point is behind the eye: clamp to the origin and reproject the segment
        ray_t = 0.0;
        t = if uu < 1e-12 { 0.0 } else { (uw / uu).clamp(0.0, 1.0) };
    }
    ((r.at(ray_t) - a.lerp(b, t)).length(), t)
}
pub fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    seg_closest(p, a, b).0
}

/// Objects inside a marquee (any projected vertex or the centre inside).
#[cfg(test)]
pub fn marquee_objects(s: &Scene, vp: &Viewport, cv: &CamView, r: Rect) -> Vec<Id> {
    region_objects(s, vp, cv, &|q| r.contains(q))
}

/// Screen-space point-in-polygon (even-odd), for the lasso.
pub fn in_polygon(poly: &[Pos2], p: Pos2) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Within `r` pixels of a brush stroke (paint selection).
pub fn on_stroke(stroke: &[Pos2], r: f32, p: Pos2) -> bool {
    match stroke {
        [] => false,
        [a] => a.distance(p) <= r,
        _ => stroke.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= r),
    }
}

/// Objects with a projected vertex (or icon point) satisfying `inside`.
pub fn region_objects(s: &Scene, vp: &Viewport, cv: &CamView, inside: &dyn Fn(Pos2) -> bool) -> Vec<Id> {
    let mut out = vec![];
    for id in s.all_dag(false) {
        if !s.is_visible(&id) || s.is_template(&id) {
            continue;
        }
        let n = s.node(&id).unwrap();
        if !vp.shows(n) {
            continue;
        }
        let wm = s.world_matrix(&id);
        let hit = match &n.shape {
            Shape::Mesh(m) => {
                let step = (m.mesh.v.len() / 4000).max(1);
                m.mesh.v.iter().step_by(step).any(|p| cv.project(wm.transform_point3(*p), vp.rect).is_some_and(inside))
            }
            Shape::Group => false,
            Shape::Camera(c) if c.startup => false,
            _ => icon_lines(s, &id, [1.0; 4]).iter().any(|v| cv.project(wm.transform_point3(DVec3::new(v.pos[0] as f64, v.pos[1] as f64, v.pos[2] as f64)), vp.rect).is_some_and(inside)),
        };
        if hit {
            out.push(id);
        }
    }
    out
}

/// A picked component.
#[derive(Clone, Debug, PartialEq)]
pub enum Comp {
    V(u32),
    E(u32, u32),
    F(usize),
}

/// A mesh that can hide a component: the JS `occluded` checks every visible mesh, so a vertex
/// behind another object must not be pickable either.
struct Occluder<'a> {
    pm: &'a PolyMesh,
    wm: DMat4,
    bvh: Option<Arc<inca_core::mesh::TriBvh>>,
}

/// All visible meshes, with their cached BVHs; rebuilt per pick (the list is short and the
/// BVHs are reused).
fn scene_occluders<'a>(s: &'a Scene, mesh_cache: &mut MeshCache) -> Vec<Occluder<'a>> {
    let mut out = vec![];
    for id in s.all_dag(false) {
        if !s.is_visible(&id) {
            continue;
        }
        let Some(n) = s.node(&id) else { continue };
        let Some(m) = n.mesh() else { continue };
        let bvh = mesh_cache.bvh(s, &id);
        out.push(Occluder { pm: &m.mesh, wm: s.world_matrix(&id), bvh });
    }
    out
}

/// Is a world point visible (not hidden behind any visible mesh)?
fn visible_point(occluders: &[Occluder], cv: &CamView, wp: DVec3) -> bool {
    let to = if cv.ortho { -cv.fwd } else { (cv.eye - wp).normalize() };
    let r = Ray { origin: wp + to * 1e-4 * (1.0 + wp.length()), dir: to };
    let dist = if cv.ortho { f64::INFINITY } else { (cv.eye - wp).length() };
    for o in occluders {
        let lr = r.transformed(&o.wm.inverse());
        let hit = match &o.bvh {
            Some(b) => b.raycast(o.pm, &lr, false),
            None => o.pm.raycast(&lr, false),
        };
        if let Some((t, _)) = hit {
            let hit = o.wm.transform_point3(lr.at(t));
            // a hit at or beyond the point is the far side of a mesh, which cannot hide it
            if (hit - wp).length() < dist - 1e-6 {
                return false;
            }
        }
    }
    true
}

/// Per-face normals and centres, computed once per revision. Besides the front-facing test
/// this feeds the face-centre dots and marquee selection, which would otherwise call
/// `face_normal` / `face_center` for every face on every pick.
#[derive(Default)]
pub struct FaceGeo {
    pub normals: Vec<DVec3>,
    pub centers: Vec<DVec3>,
}

/// Derived per-mesh data cached by node revision: edge topology (the JS `PolyMesh.topo`) and
/// the face-picking BVH (the JS three-mesh-bvh). Both would otherwise be rebuilt on every
/// pointer move, which is far too slow on heavy meshes.
#[derive(Default)]
pub struct MeshCache {
    topo: HashMap<String, (u64, Arc<Topo>)>,
    /// `None` = the mesh is small enough to raycast triangle by triangle
    bvh: HashMap<String, (u64, Option<Arc<inca_core::mesh::TriBvh>>)>,
    faces: HashMap<String, (u64, Arc<FaceGeo>)>,
    /// Last camera-dependent front-facing test per node. Hovering picks every 30 ms and the
    /// marquee picks every component, but the camera only moves while hovering is disabled,
    /// so recomputing the 400k face tests each time is wasted work.
    facing: HashMap<String, (u64, Arc<Vec<bool>>)>,
    /// Hash of the component selection per node/mode, reused while `Scene::rev` is unchanged
    /// (all viewports draw the same frame). Edge/face overlay buffers are cached by this.
    sel: HashMap<(String, u8), (u64, usize, u64)>,
}

fn hash_f64s(h: &mut impl Hasher, vals: impl IntoIterator<Item = f64>) {
    for v in vals {
        v.to_bits().hash(h);
    }
}

fn facing_key(rev: u64, wm: &DMat4, cv: &CamView) -> u64 {
    let mut h = DefaultHasher::new();
    rev.hash(&mut h);
    hash_f64s(&mut h, wm.to_cols_array());
    hash_f64s(&mut h, [cv.eye.x, cv.eye.y, cv.eye.z, cv.fwd.x, cv.fwd.y, cv.fwd.z]);
    cv.ortho.hash(&mut h);
    h.finish()
}

impl MeshCache {
    pub fn topo(&mut self, s: &Scene, id: &str) -> Option<Arc<Topo>> {
        let rev = s.node(id)?.rev;
        if let Some((r, t)) = self.topo.get(id) {
            if *r == rev {
                return Some(t.clone());
            }
        }
        let t = Arc::new(s.node(id)?.mesh()?.mesh.topo());
        self.topo.insert(id.to_string(), (rev, t.clone()));
        Some(t)
    }

    /// Face picking BVH, like the JS `computeBoundsTree` (built lazily above 300 faces).
    pub fn bvh(&mut self, s: &Scene, id: &str) -> Option<Arc<inca_core::mesh::TriBvh>> {
        let rev = s.node(id)?.rev;
        if let Some((r, b)) = self.bvh.get(id) {
            if *r == rev {
                return b.clone();
            }
        }
        let pm = &s.node(id)?.mesh()?.mesh;
        let b = (pm.f.len() > 300).then(|| Arc::new(pm.tri_bvh()));
        self.bvh.insert(id.to_string(), (rev, b.clone()));
        b
    }

    /// Per-face normals and centres of a mesh, cached by revision.
    pub fn faces(&mut self, s: &Scene, id: &str) -> Option<Arc<FaceGeo>> {
        let rev = s.node(id)?.rev;
        if let Some((r, f)) = self.faces.get(id) {
            if *r == rev {
                return Some(f.clone());
            }
        }
        let pm = &s.node(id)?.mesh()?.mesh;
        let mut normals = Vec::with_capacity(pm.f.len());
        let mut centers = Vec::with_capacity(pm.f.len());
        for fi in 0..pm.f.len() {
            normals.push(pm.face_normal(fi));
            centers.push(pm.face_center(fi));
        }
        let f = Arc::new(FaceGeo { normals, centers });
        self.faces.insert(id.to_string(), (rev, f.clone()));
        Some(f)
    }

    /// Backface culling's front-facing flags for a node and camera (the JS `frontFaces`),
    /// cached while the mesh, its world matrix and the camera stay put.
    pub fn facing(&mut self, s: &Scene, id: &str, wm: DMat4, cv: &CamView) -> Option<Arc<Vec<bool>>> {
        let rev = s.node(id)?.rev;
        let key = facing_key(rev, &wm, cv);
        if let Some((k, f)) = self.facing.get(id) {
            if *k == key {
                return Some(f.clone());
            }
        }
        let geo = self.faces(s, id)?;
        let inv = wm.inverse();
        let out: Vec<bool> = if cv.ortho {
            let to = -inv.transform_vector3(cv.fwd);
            geo.normals.iter().map(|n| n.dot(to) > 0.0).collect()
        } else {
            let eye = inv.transform_point3(cv.eye);
            geo.normals.iter().zip(&geo.centers).map(|(n, c)| n.dot(eye - *c) > 0.0).collect()
        };
        let out = Arc::new(out);
        self.facing.insert(id.to_string(), (key, out.clone()));
        Some(out)
    }

    /// Cache key of a node's component selection for the edge/face overlays: geometry revision
    /// plus the selected set. Selection changes bump `Scene::rev` (transforms do too, but the
    /// cached hash is just recomputed then, not the GPU buffers).
    pub fn comp_key(&mut self, s: &Scene, id: &str, mode: CompMode) -> Option<u64> {
        let n = s.node(id)?;
        let cs = s.comp.get(id);
        let len = match mode {
            CompMode::Vertex => cs.map_or(0, |c| c.v.len()),
            CompMode::Edge => cs.map_or(0, |c| c.e.len()),
            CompMode::Face => cs.map_or(0, |c| c.f.len()),
        };
        let k = (id.to_string(), mode as u8);
        if let Some((rev, cached_len, h)) = self.sel.get(&k) {
            if *rev == s.rev && *cached_len == len {
                return Some(*h);
            }
        }
        let mut h = DefaultHasher::new();
        n.rev.hash(&mut h);
        match mode {
            CompMode::Vertex => cs.map(|c| c.v.iter().for_each(|v| v.hash(&mut h))),
            CompMode::Edge => cs.map(|c| c.e.iter().for_each(|e| e.hash(&mut h))),
            CompMode::Face => cs.map(|c| c.f.iter().for_each(|f| f.hash(&mut h))),
        };
        let h = h.finish();
        self.sel.insert(k, (s.rev, len, h));
        Some(h)
    }

    pub fn retain(&mut self, alive: &std::collections::HashSet<&str>) {
        self.topo.retain(|k, _| alive.contains(k.as_str()));
        self.bvh.retain(|k, _| alive.contains(k.as_str()));
        self.faces.retain(|k, _| alive.contains(k.as_str()));
        self.facing.retain(|k, _| alive.contains(k.as_str()));
        self.sel.retain(|(k, _), _| alive.contains(k.as_str()));
    }
}

/// The JS marquee skips per-component raycasts above 4000 vertices (its `many`).
fn marquee_exact(pm: &PolyMesh) -> bool {
    pm.v.len() <= 4000
}

/// Component visibility: exact raycasts where the JS does them, plus the front-facing test
/// when backface culling is on (the JS `frontFaces`, which is a no-op when culling is off).
struct Vis<'a> {
    pm: &'a PolyMesh,
    wm: DMat4,
    cv: &'a CamView,
    exact: bool,
    /// Front-facing flags from `MeshCache::facing`, `None` when the test does not apply.
    facing: Option<Arc<Vec<bool>>>,
    topo: Option<Arc<Topo>>,
    occluders: &'a [Occluder<'a>],
}

impl<'a> Vis<'a> {
    /// `facing` and its `topo` are only needed for the front-facing test, `occluders` only
    /// when `exact` is set.
    fn new(
        pm: &'a PolyMesh,
        wm: DMat4,
        cv: &'a CamView,
        exact: bool,
        facing: Option<Arc<Vec<bool>>>,
        topo: Option<Arc<Topo>>,
        occluders: &'a [Occluder<'a>],
    ) -> Self {
        Vis { pm, wm, cv, exact, facing, topo, occluders }
    }
    fn visible(&self, p: DVec3) -> bool {
        !self.exact || visible_point(self.occluders, self.cv, self.wm.transform_point3(p))
    }
    pub fn vertex(&self, v: u32) -> bool {
        if let (Some(facing), Some(topo)) = (&self.facing, &self.topo) {
            let fs = &topo.vert_faces[v as usize];
            if !fs.is_empty() && !fs.iter().any(|&f| facing[f]) {
                return false;
            }
        }
        self.visible(self.pm.v[v as usize])
    }
    /// `e` is the index in `topo.edges`; doing the front-facing lookup by index avoids a
    /// hash probe per edge, which added ~150 ms to a 798k-edge marquee.
    pub fn edge_idx(&self, e: usize, a: u32, b: u32) -> bool {
        self.edge_idx_at(e, a, b, 0.5)
    }
    /// Visibility at parameter `t` along the edge, as the JS click picker uses the point
    /// closest to the cursor rather than the midpoint.
    pub fn edge_idx_at(&self, e: usize, a: u32, b: u32, t: f64) -> bool {
        if let Some(facing) = &self.facing {
            let Some(topo) = &self.topo else { return false };
            if !topo.edge_faces[e].iter().any(|&f| facing[f]) {
                return false;
            }
        }
        let (pa, pb) = (self.pm.v[a as usize], self.pm.v[b as usize]);
        self.visible(pa + (pb - pa) * t)
    }
    pub fn face(&self, f: usize) -> bool {
        self.facing.as_ref().is_none_or(|x| x[f]) && self.visible(self.pm.face_center(f))
    }
}

/// One component hit plus the values the JS `pickComponent` compares between meshes.
pub struct CompHit {
    pub comp: Comp,
    /// Cursor distance in pixels (vertex, edge and face-dot picks).
    d: f32,
    /// Eye distance for vertices, world distance along the pick ray for faces.
    z: f64,
    /// Face-centre dot fallback (wireframe/x-ray): only used when no face is hit.
    dot: bool,
}

/// Score one mesh's closest component; `pick_best_component` merges the hits of all
/// highlighted meshes.
pub fn pick_component_hit(s: &Scene, vp: &Viewport, cv: &CamView, id: &str, mode: CompMode, pos: Pos2, mesh_cache: &mut MeshCache) -> Option<CompHit> {
    let n = s.node(id)?;
    let wm = s.world_matrix(id);
    // a curve is a vertex cloud to the JS picker (`o.inca.curve.cvs`); there are no edges or faces
    if let Some(c) = n.curve() {
        if mode != CompMode::Vertex {
            return None;
        }
        let mut best: Option<CompHit> = None;
        for (i, p) in c.cvs.iter().enumerate() {
            let wp = wm.transform_point3(*p);
            let Some(q) = cv.project(wp, vp.rect) else { continue };
            let d = q.distance(pos);
            let wins = best.as_ref().is_none_or(|b| d < b.d - 0.5 || ((d - b.d).abs() < 0.5 && cv.depth(wp) < b.z));
            if d < 12.0 && wins {
                best = Some(CompHit { comp: Comp::V(i as u32), d, z: cv.depth(wp), dot: false });
            }
        }
        // the JS `occluded` always returns false for curve owners
        return best;
    }
    let pm = &n.mesh()?.mesh;
    // the JS `occluded` returns false in x-ray and wireframe
    let opaque = !vp.xray && vp.shade != ShadeMode::Wire;
    match mode {
        CompMode::Vertex => {
            let facing = if vp.backface { mesh_cache.facing(s, id, wm, cv) } else { None };
            let topo = if facing.is_some() { mesh_cache.topo(s, id) } else { None };
            let occ = if opaque { scene_occluders(s, mesh_cache) } else { vec![] };
            let vis = Vis::new(pm, wm, cv, opaque, facing, topo, &occ);
            let mut best: Option<CompHit> = None;
            for (i, p) in pm.v.iter().enumerate() {
                let wp = wm.transform_point3(*p);
                let Some(q) = cv.project(wp, vp.rect) else { continue };
                let d = q.distance(pos);
                // the JS keeps a vertex within half a pixel of the best when it is nearer
                let wins = best.as_ref().is_none_or(|b| d < b.d - 0.5 || ((d - b.d).abs() < 0.5 && cv.depth(wp) < b.z));
                if d < 12.0 && wins && vis.vertex(i as u32) {
                    best = Some(CompHit { comp: Comp::V(i as u32), d, z: cv.depth(wp), dot: false });
                }
            }
            best
        }
        CompMode::Edge => {
            let facing = if vp.backface { mesh_cache.facing(s, id, wm, cv) } else { None };
            let topo = mesh_cache.topo(s, id)?;
            let occ = if opaque { scene_occluders(s, mesh_cache) } else { vec![] };
            let vis = Vis::new(pm, wm, cv, opaque, facing, Some(topo.clone()), &occ);
            let screen: Vec<Option<Pos2>> = pm.v.iter().map(|p| cv.project(wm.transform_point3(*p), vp.rect)).collect();
            let mut best: Option<CompHit> = None;
            for (ei, &(a, b)) in topo.edges.iter().enumerate() {
                let (Some(qa), Some(qb)) = (screen[a as usize], screen[b as usize]) else { continue };
                let (d, t) = seg_closest(pos, qa, qb);
                if d < 9.0 && best.as_ref().is_none_or(|x| d < x.d) && vis.edge_idx_at(ei, a, b, t as f64) {
                    best = Some(CompHit { comp: Comp::E(a, b), d, z: 0.0, dot: false });
                }
            }
            best
        }
        CompMode::Face => {
            let ray = cv.ray(pos, vp.rect);
            let lr = ray.transformed(&wm.inverse());
            // the BVH turns this from an O(faces) scan (~60 ms at 400k) into a few µs
            let hit = match mesh_cache.bvh(s, id) {
                Some(b) => b.raycast(pm, &lr, vp.backface),
                None => pm.raycast(&lr, vp.backface),
            };
            if let Some((t, f)) = hit {
                // compare hits across meshes by world distance along the pick ray
                let wp = wm.transform_point3(lr.at(t));
                return Some(CompHit { comp: Comp::F(f), d: 0.0, z: (wp - ray.origin).dot(ray.dir), dot: false });
            }
            // the JS fallback picks the nearest face-centre dot in wireframe/x-ray
            if !(vp.shade == ShadeMode::Wire || vp.xray) {
                return None;
            }
            let facing = if vp.backface { mesh_cache.facing(s, id, wm, cv) } else { None };
            let geo = mesh_cache.faces(s, id)?;
            let mut best: Option<CompHit> = None;
            for (fi, c) in geo.centers.iter().enumerate() {
                if facing.as_ref().is_some_and(|ff| !ff[fi]) {
                    continue;
                }
                let Some(q) = cv.project(wm.transform_point3(*c), vp.rect) else { continue };
                let d = q.distance(pos);
                if d < 10.0 && best.as_ref().is_none_or(|b| d < b.d) {
                    best = Some(CompHit { comp: Comp::F(fi), d, z: 0.0, dot: true });
                }
            }
            best
        }
    }
}

/// Pick one mesh's component (no cross-mesh comparison).
#[cfg(test)]
pub fn pick_component(s: &Scene, vp: &Viewport, cv: &CamView, id: &str, mode: CompMode, pos: Pos2, mesh_cache: &mut MeshCache) -> Option<Comp> {
    pick_component_hit(s, vp, cv, id, mode, pos, mesh_cache).map(|h| h.comp)
}

/// Pick the nearest component over several meshes, as the JS `pickComponent` scans the
/// whole highlight list: a closer edge on another object beats a farther one here.
pub fn pick_best_component(s: &Scene, vp: &Viewport, cv: &CamView, ids: &[Id], mode: CompMode, pos: Pos2, mesh_cache: &mut MeshCache) -> Option<(Id, Comp)> {
    let mut best: Option<(Id, CompHit)> = None;
    let mut best_dot: Option<(Id, CompHit)> = None;
    for id in ids {
        let Some(hit) = pick_component_hit(s, vp, cv, id, mode, pos, mesh_cache) else { continue };
        if hit.dot {
            if best_dot.as_ref().is_none_or(|(_, b)| hit.d < b.d) {
                best_dot = Some((id.clone(), hit));
            }
            continue;
        }
        let wins = match &best {
            None => true,
            Some((_, b)) => match mode {
                CompMode::Vertex => hit.d < b.d - 0.5 || ((hit.d - b.d).abs() < 0.5 && hit.z < b.z),
                CompMode::Edge => hit.d < b.d,
                CompMode::Face => hit.z < b.z,
            },
        };
        if wins {
            best = Some((id.clone(), hit));
        }
    }
    best.or(best_dot).map(|(id, hit)| (id, hit.comp))
}

#[cfg(test)]
pub fn marquee_components(s: &Scene, vp: &Viewport, cv: &CamView, id: &str, mode: CompMode, r: Rect) -> Vec<Comp> {
    region_components(s, vp, cv, id, mode, &|q| r.contains(q), &mut MeshCache::default())
}

pub fn region_components(s: &Scene, vp: &Viewport, cv: &CamView, id: &str, mode: CompMode, region: &dyn Fn(Pos2) -> bool, mesh_cache: &mut MeshCache) -> Vec<Comp> {
    let Some(n) = s.node(id) else { return vec![] };
    let wm = s.world_matrix(id);
    let inside = |p: DVec3| cv.project(wm.transform_point3(p), vp.rect).is_some_and(region);
    // curve CVs behave like vertices (and are never occluded)
    if let Some(c) = n.curve() {
        if mode != CompMode::Vertex {
            return vec![];
        }
        return (0..c.cvs.len()).filter(|&i| inside(c.cvs[i])).map(|i| Comp::V(i as u32)).collect();
    }
    let Some(pm) = n.mesh().map(|m| &m.mesh) else { return vec![] };
    let shaded = !vp.xray && vp.shade != ShadeMode::Wire;
    // in shaded mode the JS marquee hides back faces even with backface culling off: you
    // cannot select what you cannot see (wireframe and x-ray show every face)
    let facing = if vp.backface || (mode == CompMode::Face && shaded) { mesh_cache.facing(s, id, wm, cv) } else { None };
    let topo = if mode == CompMode::Edge || (facing.is_some() && mode == CompMode::Vertex) { mesh_cache.topo(s, id) } else { None };
    let exact = shaded && marquee_exact(pm);
    let occ = if exact { scene_occluders(s, mesh_cache) } else { vec![] };
    let vis = Vis::new(pm, wm, cv, exact, facing, topo.clone(), &occ);
    match mode {
        CompMode::Vertex => (0..pm.v.len()).filter(|&i| inside(pm.v[i]) && vis.vertex(i as u32)).map(|i| Comp::V(i as u32)).collect(),
        CompMode::Edge => {
            let Some(topo) = topo else { return vec![] };
            let screen: Vec<Option<Pos2>> = pm.v.iter().map(|p| cv.project(wm.transform_point3(*p), vp.rect)).collect();
            topo.edges
                .iter()
                .enumerate()
                .filter(|(ei, &(a, b))| {
                    let postedge = screen[a as usize].is_some_and(region) && screen[b as usize].is_some_and(region);
                    (postedge || inside((pm.v[a as usize] + pm.v[b as usize]) * 0.5)) && vis.edge_idx(*ei, a, b)
                })
                .map(|(_, &(a, b))| Comp::E(a, b))
                .collect()
        }
        CompMode::Face => (0..pm.f.len()).filter(|&f| inside(pm.face_center(f)) && vis.face(f)).map(Comp::F).collect(),
    }
}

/// Particles as soft dots (drawn with egui on top of the image).
/// Distance dimension labels at the middle of their lines.
pub fn draw_annotations(s: &Scene, vp: &Viewport, cv: &CamView, painter: &egui::Painter) {
    for n in s.nodes.values().filter(|n| n.kind == "distanceDimension") {
        if !s.is_visible(&n.id) {
            continue;
        }
        let Some((a, b, d)) = s.distance_of(&n.id) else { continue };
        if let Some(q) = cv.project((a + b) * 0.5, vp.rect) {
            painter.text(q + egui::vec2(0.0, -8.0), egui::Align2::CENTER_BOTTOM, inca_core::math::fmt_num(d), egui::FontId::proportional(12.0), Color32::WHITE);
        }
    }
}

pub fn draw_particles(s: &Scene, vp: &Viewport, cv: &CamView, painter: &egui::Painter) {
    for (em, list) in &s.dynamics.particles {
        let size = s.node(em).and_then(|n| n.extra["size"].as_f64()).unwrap_or(3.0) as f32;
        for p in list {
            if let Some(q) = cv.project(p.p, vp.rect) {
                if !vp.rect.contains(q) {
                    continue;
                }
                let c = inca_core::dynamics::Dynamics::particle_color(s, em, p);
                let col = Color32::from_rgba_unmultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 200.0) as u8);
                painter.circle_filled(q, size * 0.5, col);
            }
        }
    }
}

/// Vertex dots for component mode (drawn with egui on top of the image).
/// Soft Selection weights (JS softWeights): selected verts 1, others by world distance to the nearest selected one.
pub fn soft_weights(s: &Scene, id: &str, verts: &[u32], radius: f64, linear: bool) -> Vec<(u32, f64)> {
    let Some(pm) = s.node(id).and_then(|n| n.mesh()).map(|m| &m.mesh) else { return vec![] };
    let wm = s.world_matrix(id);
    let p: Vec<DVec3> = pm.v.iter().map(|q| wm.transform_point3(*q)).collect();
    let sel: std::collections::HashSet<u32> = verts.iter().copied().collect();
    let sp: Vec<DVec3> = verts.iter().map(|&v| p[v as usize]).collect();
    let mut out = vec![];
    for (i, q) in p.iter().enumerate() {
        if sel.contains(&(i as u32)) {
            out.push((i as u32, 1.0));
            continue;
        }
        let d = sp.iter().map(|s| (*s - *q).length()).fold(f64::INFINITY, f64::min);
        if d < radius {
            let t = 1.0 - d / radius;
            out.push((i as u32, if linear { t } else { t * t * (3.0 - 2.0 * t) }));
        }
    }
    out
}

/// Symmetry partners (JS symmetryPartners): appends the mirror image of each vertex to `verts`
/// and returns, per entry, the index of the entry it mirrors (-1 for originals).
/// `mode`: 1 object X, 2 world X, 3 world Y, 4 world Z.
pub fn symmetry_partners(s: &Scene, id: &str, verts: &mut Vec<u32>, weights: &mut Vec<f64>, mode: usize) -> Option<Vec<i64>> {
    if mode == 0 || verts.is_empty() {
        return None;
    }
    let node = s.node(id)?;
    // curves mirror their CVs too (the JS passes `curve.cvs` as the point list)
    let pts: &[DVec3] = match node.curve() {
        Some(c) => &c.cvs,
        None => &node.mesh()?.mesh.v,
    };
    let ax = if mode == 1 { 0 } else { mode - 2 };
    let wm = s.world_matrix(id);
    let p: Vec<DVec3> = pts.iter().map(|q| if mode == 1 { *q } else { wm.transform_point3(*q) }).collect();
    let b = p.iter().fold(Aabb::EMPTY, |mut b, q| {
        b.add(*q);
        b
    });
    let tol = (b.size().length() * 1e-4).max(1e-6);
    let cell = |q: DVec3| ((q.x / tol).floor() as i64, (q.y / tol).floor() as i64, (q.z / tol).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    for (i, q) in p.iter().enumerate() {
        grid.entry(cell(*q)).or_default().push(i);
    }
    let find = |q: DVec3| -> Option<usize> {
        let (cx, cy, cz) = cell(q);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &j in grid.get(&(cx + dx, cy + dy, cz + dz)).into_iter().flatten() {
                        let d = (p[j] - q).length();
                        if d <= tol && best.is_none_or(|b| d < b.0) {
                            best = Some((d, j));
                        }
                    }
                }
            }
        }
        best.map(|b| b.1)
    };
    let mut idx: HashMap<u32, usize> = verts.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let mut map = vec![-1i64; verts.len()];
    let n = verts.len();
    for i in 0..n {
        let mut q = p[verts[i] as usize];
        q[ax] = -q[ax];
        let Some(j) = find(q) else { continue };
        if j as u32 == verts[i] || idx.contains_key(&(j as u32)) {
            continue;
        }
        idx.insert(j as u32, verts.len());
        verts.push(j as u32);
        map.push(i as i64);
        if !weights.is_empty() {
            weights.push(weights[i]);
        }
    }
    map.iter().any(|&m| m >= 0).then_some(map)
}

/// Mirror a world/local point across the symmetry plane (see `symmetry_partners`).
pub fn mirror_point(p: DVec3, mode: usize) -> DVec3 {
    let ax = if mode == 1 { 0 } else { mode - 2 };
    let mut q = p;
    q[ax] = -q[ax];
    q
}

/// Soft Selection falloff colour (black - red - yellow), as Maya shows it.
pub fn falloff_color(w: f64) -> Color32 {
    let w = w.clamp(0.0, 1.0) as f32;
    if w < 0.5 {
        Color32::from_rgb((w * 2.0 * 255.0) as u8, 0, 0)
    } else {
        Color32::from_rgb(255, ((w - 0.5) * 2.0 * 255.0) as u8, 0)
    }
}

/// Component dots are drawn by the GPU (`build_draws`); this adds only the preselection
/// highlight, which is at most one component.
pub fn draw_component_overlay(s: &Scene, vp: &Viewport, cv: &CamView, painter: &egui::Painter, hover: Option<(&str, &Comp)>) {
    let Some((id, c)) = hover else { return };
    let Some(n) = s.node(id) else { return };
    let wm = s.world_matrix(id);
    let red = Color32::from_rgb(255, 42, 42);
    // curve CVs are the curve's vertices (the JS hover point uses `curve.cvs`)
    let pr = |v: u32| {
        let p = match n.curve() {
            Some(cu) => cu.cvs.get(v as usize).copied()?,
            None => n.mesh()?.mesh.v.get(v as usize).copied()?,
        };
        cv.project(wm.transform_point3(p), vp.rect)
    };
    match c {
        Comp::V(v) => {
            if let Some(q) = pr(*v) {
                // the JS hover point is 9 px, square, always on top
                painter.rect_filled(Rect::from_center_size(q, egui::vec2(9.0, 9.0)), 0.0, red);
            }
        }
        Comp::E(a, b) => {
            if let (Some(qa), Some(qb)) = (pr(*a), pr(*b)) {
                painter.line_segment([qa, qb], egui::Stroke::new(2.0, red));
            }
        }
        Comp::F(f) => {
            let Some(pm) = n.mesh().map(|m| &m.mesh) else { return };
            // the JS hover face is a translucent red fill (0.35, no outline)
            let pts: Vec<Pos2> = pm.f[*f].iter().filter_map(|&v| pr(v)).collect();
            if pts.len() >= 3 {
                painter.add(egui::Shape::convex_polygon(pts, Color32::from_rgba_unmultiplied(255, 42, 42, 89), egui::Stroke::NONE));
            }
        }
    }
}

/// Component overlay for curves: 6 px CV dots (purple, selected yellow, always on top — the
/// JS `pointsMatX` has depthTest off) and the dashed CV hull, as `updateComponentOverlays`.
pub fn draw_curve_components(s: &Scene, vp: &Viewport, cv: &CamView, painter: &egui::Painter) {
    if s.comp_mode.is_none() {
        return;
    }
    let dot = Color32::from_rgb(0xd1, 0x6f, 0xf0);
    let dot_sel = Color32::from_rgb(0xff, 0xe6, 0x00);
    let hull = Color32::from_rgb(0xc0, 0x70, 0xff);
    for id in s.hilite() {
        let Some(c) = s.node(&id).and_then(|n| n.curve()) else { continue };
        let wm = s.world_matrix(&id);
        let sel = s.comp_sel(&id);
        let pts: Vec<Option<Pos2>> = c.cvs.iter().map(|p| cv.project(wm.transform_point3(*p), vp.rect)).collect();
        // the hull is a line through the CVs in order (the JS `THREE.Line`, not the sampled curve)
        let mut run: Vec<Pos2> = vec![];
        for p in pts.iter().chain(std::iter::once(&None)) {
            match p {
                Some(q) => run.push(*q),
                None => {
                    if run.len() > 1 {
                        painter.add(egui::Shape::dashed_line(&run, Stroke::new(1.0, hull), 6.0, 5.0));
                    }
                    run.clear();
                }
            }
        }
        for (i, p) in pts.iter().enumerate() {
            if let Some(q) = p {
                let color = if sel.v.contains(&(i as u32)) { dot_sel } else { dot };
                painter.rect_filled(Rect::from_center_size(*q, egui::vec2(6.0, 6.0)), 0.0, color);
            }
        }
    }
}

/// World bbox of the current selection (components when in component mode).
pub fn selection_bbox(s: &Scene) -> Aabb {
    let mut b = Aabb::EMPTY;
    if s.comp_mode.is_some() {
        for id in s.hilite() {
            let Some(n) = s.node(&id) else { continue };
            let wm = s.world_matrix(&id);
            for v in s.comp_verts(&id) {
                if let Some(c) = n.curve() {
                    if let Some(p) = c.cvs.get(v as usize) {
                        b.add(wm.transform_point3(*p));
                    }
                } else if let Some(pm) = n.mesh() {
                    if let Some(p) = pm.mesh.v.get(v as usize) {
                        b.add(wm.transform_point3(*p));
                    }
                }
            }
        }
        if !b.is_empty() {
            return b;
        }
    }
    for id in &s.sel {
        b.union(&s.world_bbox(id));
        if s.world_bbox(id).is_empty() {
            b.add(s.world_matrix(id).transform_point3(DVec3::ZERO));
        }
    }
    b
}

pub fn scene_bbox(s: &Scene) -> Aabb {
    let mut b = Aabb::EMPTY;
    for id in s.all_dag(false) {
        if s.is_visible(&id) && s.node(&id).is_some_and(|n| !matches!(n.shape, Shape::Group)) {
            b.union(&s.local_bbox(&id).transformed(&s.world_matrix(&id)));
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use inca_core::params::Params;
    use egui::pos2;

    fn setup() -> (Scene, Viewport, CamView, Id) {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let mut vp = Viewport::new("persp");
        vp.rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let b = s.world_bbox(&id);
        frame_box(&mut s, "persp", &b, 800.0 / 600.0);
        let cv = cam_view(&s, "persp", [800.0, 600.0]);
        (s, vp, cv, id)
    }

    #[test]
    fn projection_round_trip() {
        let (_, vp, cv, _) = setup();
        let p = DVec3::new(0.2, 0.3, -0.1);
        let q = cv.project(p, vp.rect).unwrap();
        let r = cv.ray(q, vp.rect);
        // the ray through the projected point passes the point
        let t = (p - r.origin).dot(r.dir);
        assert!((r.at(t) - p).length() < 1e-6);
    }

    #[test]
    fn cached_facing_matches_direct_and_reuses() {
        let (s, _vp, cv, id) = setup();
        let wm = s.world_matrix(&id);
        let mut cache = MeshCache::default();
        let facing = cache.facing(&s, &id, wm, &cv).unwrap();
        let pm = &s.node(&id).unwrap().mesh().unwrap().mesh;
        // the cached local-space test is equivalent to the world-space inverse-transpose one
        let eye = wm.inverse().transform_point3(cv.eye);
        for f in 0..pm.f.len() {
            assert_eq!(facing[f], pm.face_normal(f).dot(eye - pm.face_center(f)) > 0.0, "face {f}");
        }
        // the same camera reuses the flags; a moved camera is a different key
        assert!(Arc::ptr_eq(&facing, &cache.facing(&s, &id, wm, &cv).unwrap()));
        let mut moved = cv;
        moved.eye += DVec3::X * 2.0;
        assert!(!Arc::ptr_eq(&facing, &cache.facing(&s, &id, wm, &moved).unwrap()));

        // edge visibility by index matches the per-edge face test (xray skips exact occlusion)
        let topo = cache.topo(&s, &id).unwrap();
        let vis = Vis::new(pm, wm, &cv, false, Some(facing.clone()), Some(topo.clone()), &[]);
        for (ei, &(a, b)) in topo.edges.iter().enumerate() {
            assert_eq!(vis.edge_idx(ei, a, b), topo.edge_faces[ei].iter().any(|&f| facing[f]), "edge {ei}");
        }
    }

    #[test]
    fn backface_pick_uses_cached_facing() {
        let (s, mut vp, cv, id) = setup();
        vp.backface = true;
        let pm = &s.node(&id).unwrap().mesh().unwrap().mesh;
        let depth = |i: usize| cv.depth(pm.v[i]);
        let near = (0..pm.v.len()).min_by(|&a, &b| depth(a).total_cmp(&depth(b))).unwrap();
        let q = cv.project(pm.v[near], vp.rect).unwrap();
        let mut cache = MeshCache::default();
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, q, &mut cache), Some(Comp::V(near as u32)));
        // the warm cache returns the same pick
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, q, &mut cache), Some(Comp::V(near as u32)));
        // the far corner is culled
        let far = (0..pm.v.len()).max_by(|&a, &b| depth(a).total_cmp(&depth(b))).unwrap();
        let fq = cv.project(pm.v[far], vp.rect).unwrap();
        assert_ne!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, fq, &mut cache), Some(Comp::V(far as u32)));
    }

    #[test]
    fn picks_respect_other_meshes() {
        let (mut s, vp, cv, id) = setup();
        let (near, wp, q) = {
            let pm = &s.node(&id).unwrap().mesh().unwrap().mesh;
            let near = (0..pm.v.len()).min_by(|&a, &b| cv.depth(pm.v[a]).total_cmp(&cv.depth(pm.v[b]))).unwrap();
            let wp = s.world_matrix(&id).transform_point3(pm.v[near]);
            (near, wp, cv.project(wp, vp.rect).unwrap())
        };
        let mut cache = MeshCache::default();
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, q, &mut cache), Some(Comp::V(near as u32)));
        // a big cube between the camera and the vertex hides it (the JS `occluded` checks all meshes)
        let mid = (cv.eye + wp) * 0.5;
        let occ = s.create_mesh("polyCube", &Params::new(), None);
        s.set_world_matrix(&occ, DMat4::from_scale_rotation_translation(DVec3::splat(0.6), DQuat::IDENTITY, mid));
        assert_ne!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, q, &mut cache), Some(Comp::V(near as u32)));
    }

    #[test]
    fn pick_prefers_nearer_mesh_across_objects() {
        let (mut s, vp, cv, a) = setup();
        let (v, p) = {
            let pm = &s.node(&a).unwrap().mesh().unwrap().mesh;
            let v = (0..pm.v.len()).min_by(|&i, &j| cv.depth(pm.v[i]).total_cmp(&cv.depth(pm.v[j]))).unwrap();
            (v, s.world_matrix(&a).transform_point3(pm.v[v]))
        };
        let q = cv.project(p, vp.rect).unwrap();
        // put the second cube behind the first along the eye-to-vertex ray, so its vertex
        // projects to the same pixel but is farther away
        let b = s.create_mesh("polyCube", &Params::new(), None);
        let local = s.node(&a).unwrap().mesh().unwrap().mesh.v[v];
        let far = p + (p - cv.eye).normalize() * 3.0;
        s.set_world_matrix(&b, DMat4::from_translation(far - local));
        let mut cache = MeshCache::default();
        // the farther cube comes first in the highlight list, as it might by DAG order
        let (id, c) = pick_best_component(&s, &vp, &cv, &[b, a.clone()], CompMode::Vertex, q, &mut cache).unwrap();
        assert_eq!(id, a);
        assert_eq!(c, Comp::V(v as u32));
    }

    #[test]
    fn face_pick_prefers_nearer_mesh_across_objects() {
        let (mut s, vp, cv, a) = setup();
        let center = s.world_bbox(&a).center();
        let q = cv.project(center, vp.rect).unwrap();
        // a cube behind the first along the same ray: both are hit, the nearer wins
        let b = s.create_mesh("polyCube", &Params::new(), None);
        s.set_world_matrix(&b, DMat4::from_translation((center - cv.eye).normalize() * 3.0));
        let mut cache = MeshCache::default();
        let (id, _) = pick_best_component(&s, &vp, &cv, &[b, a.clone()], CompMode::Face, q, &mut cache).unwrap();
        assert_eq!(id, a);
    }

    #[test]
    fn curve_cvs_pick_and_region() {
        use inca_core::curves::Curve;
        let mut s = Scene::new();
        let c = s.create_curve(Curve::new(vec![DVec3::new(0.0, 0.0, 0.0), DVec3::new(0.5, 0.5, 0.0)], 1, "open", "cv"), "curve1");
        let mut vp = Viewport::new("persp");
        vp.rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let b = s.world_bbox(&c);
        frame_box(&mut s, "persp", &b, 800.0 / 600.0);
        let cv = cam_view(&s, "persp", [800.0, 600.0]);
        let p = s.world_matrix(&c).transform_point3(DVec3::ZERO);
        let q = cv.project(p, vp.rect).unwrap();
        let mut cache = MeshCache::default();
        // a small cube between the eye and the CV: mesh picks would be occluded, curves never are
        let mesh = s.create_mesh("polyCube", &Params::new(), None);
        s.set_world_matrix(&mesh, DMat4::from_translation(cv.eye + (p - cv.eye) * 0.5) * DMat4::from_scale(DVec3::splat(0.2)));
        let (id, comp) = pick_best_component(&s, &vp, &cv, &[mesh, c.clone()], CompMode::Vertex, q, &mut cache).unwrap();
        assert_eq!(id, c);
        assert_eq!(comp, Comp::V(0));
        // the marquee treats CVs like vertices
        let inside = |r: Pos2| vp.rect.contains(r);
        assert_eq!(region_components(&s, &vp, &cv, &c, CompMode::Vertex, &inside, &mut cache), vec![Comp::V(0), Comp::V(1)]);
        // curves have no edge or face components
        assert!(region_components(&s, &vp, &cv, &c, CompMode::Edge, &inside, &mut cache).is_empty());
    }

    #[test]
    fn pick_radii_match_js() {
        let (s, vp, cv, id) = setup();
        let pm = &s.node(&id).unwrap().mesh().unwrap().mesh;
        let near = (0..pm.v.len()).min_by(|&a, &b| cv.depth(pm.v[a]).total_cmp(&cv.depth(pm.v[b]))).unwrap();
        let q = cv.project(pm.v[near], vp.rect).unwrap();
        let mut cache = MeshCache::default();
        // the JS vertex radius is 12 px (was 10)
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, q + egui::vec2(11.0, 0.0), &mut cache), Some(Comp::V(near as u32)));
        // the JS edge radius is 9 px (was 8), measured from the closest point on the segment
        let topo = cache.topo(&s, &id).unwrap();
        let e = topo.edges.iter().position(|&(a, b)| a == near as u32 || b == near as u32).unwrap();
        let (a, b) = topo.edges[e];
        let (qa, qb) = (cv.project(pm.v[a as usize], vp.rect).unwrap(), cv.project(pm.v[b as usize], vp.rect).unwrap());
        let mid = egui::pos2((qa.x + qb.x) * 0.5, (qa.y + qb.y) * 0.5);
        let dir = (qb - qa).normalized();
        let p = mid + egui::vec2(-dir.y, dir.x) * 8.5;
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Edge, p, &mut cache), Some(Comp::E(a, b)));
    }

    #[test]
    fn marquee_face_skips_backfaces_when_shaded() {
        let (s, vp, cv, id) = setup();
        let faces = s.node(&id).unwrap().mesh().unwrap().mesh.f.len();
        let mut cache = MeshCache::default();
        let shaded = region_components(&s, &vp, &cv, &id, CompMode::Face, &|_| true, &mut cache);
        assert!(shaded.len() < faces, "back faces are not selectable in shaded mode");
        assert!(!shaded.is_empty());
        // x-ray shows every face, so all of them can be selected
        let mut xray = vp;
        xray.xray = true;
        let all = region_components(&s, &xray, &cv, &id, CompMode::Face, &|_| true, &mut cache);
        assert_eq!(all.len(), faces);
    }

    #[test]
    fn face_center_dots_follow_selection() {
        let (mut s, _vp, _cv, id) = setup();
        let mut cache = MeshCache::default();
        let geo = cache.faces(&s, &id).unwrap();
        let pts = face_center_points(&s, &id, &geo);
        assert_eq!(pts.len(), geo.centers.len());
        for (p, c) in pts.iter().zip(&geo.centers) {
            assert_eq!(p.pos, to_f32(*c));
            assert_eq!(p.color, [C_DOT[0], C_DOT[1], C_DOT[2], 1.0]);
            // face normals drive the shader's backface culling of the dots
            assert!(p.normal.iter().any(|x| *x != 0.0));
        }
        // selecting a face turns only its dot orange
        s.comp.entry(id.clone()).or_default().f.insert(0);
        let pts = face_center_points(&s, &id, &geo);
        assert_eq!(pts[0].color, [C_FACE_SEL[0], C_FACE_SEL[1], C_FACE_SEL[2], 1.0]);
        assert_eq!(pts[1].color, [C_DOT[0], C_DOT[1], C_DOT[2], 1.0]);
    }

    #[test]
    fn comp_key_tracks_selection_and_geometry() {
        let (mut s, _vp, _cv, id) = setup();
        let mut cache = MeshCache::default();
        let k0 = cache.comp_key(&s, &id, CompMode::Face).unwrap();
        // four viewports redraw the same frame: the hash is computed once
        assert_eq!(cache.comp_key(&s, &id, CompMode::Face), Some(k0));
        // a different selection with the same number of faces must change the key
        s.comp.entry(id.clone()).or_default().f.insert(0);
        s.rev += 1;
        let k1 = cache.comp_key(&s, &id, CompMode::Face).unwrap();
        assert_ne!(k0, k1);
        let f = &mut s.comp.get_mut(&id).unwrap().f;
        f.clear();
        f.insert(1);
        s.rev += 1;
        let k2 = cache.comp_key(&s, &id, CompMode::Face).unwrap();
        assert_ne!(k1, k2);
        // geometry edits bump both revisions (see `Scene::touch`)
        s.node_mut(&id).unwrap().rev += 1;
        s.touch();
        let k3 = cache.comp_key(&s, &id, CompMode::Face).unwrap();
        assert_ne!(k2, k3);
        // edge and face overlays have independent keys
        assert_ne!(cache.comp_key(&s, &id, CompMode::Edge).unwrap(), k3);
    }

    #[test]
    fn curve_object_pick_competes_by_depth() {
        // the JS intersects the sampled curve line at ray distance (-0.001): a mesh in front wins
        let (mut s, vp, cv, mesh) = setup();
        let front = cv.eye + (DVec3::ZERO - cv.eye).normalize() * (cv.eye.length() - 2.0);
        let behind = cv.eye + (DVec3::ZERO - cv.eye).normalize() * (cv.eye.length() + 2.0);
        let mk = |s: &mut Scene, at: DVec3| {
            s.create_curve(
                inca_core::curves::Curve::new(vec![at - cv.right * 0.3, at + cv.right * 0.3], 1, "open", "cv"),
                "c1",
            )
        };
        let c = mk(&mut s, front);
        let p = cv.project(front, vp.rect).unwrap();
        assert_eq!(pick_object(&s, &vp, &cv, p, &mut MeshCache::default()), Some(c.clone()));
        // now behind the cube: the cube occludes the curve pick
        s.set_world_matrix(&c, DMat4::from_translation(behind - front));
        assert_eq!(pick_object(&s, &vp, &cv, p, &mut MeshCache::default()), Some(mesh));
    }

    #[test]
    fn pick_object_and_components() {
        let (s, vp, cv, id) = setup();
        let c = cv.project(DVec3::ZERO, vp.rect).unwrap();
        assert_eq!(pick_object(&s, &vp, &cv, c, &mut MeshCache::default()), Some(id.clone()));
        assert_eq!(pick_object(&s, &vp, &cv, Pos2::new(2.0, 2.0), &mut MeshCache::default()), None);
        // the corner facing the camera (+x +y +z) is visible and pickable
        let pm = &s.node(&id).unwrap().mesh().unwrap().mesh;
        let vi = pm.v.iter().position(|v| v.x > 0.0 && v.y > 0.0 && v.z > 0.0).unwrap();
        let q = cv.project(pm.v[vi], vp.rect).unwrap();
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, q + egui::vec2(3.0, 2.0), &mut MeshCache::default()), Some(Comp::V(vi as u32)));
        // the hidden corner is not picked through the cube
        let hidden = pm.v.iter().position(|v| v.x < 0.0 && v.y < 0.0 && v.z < 0.0).unwrap();
        let hq = cv.project(pm.v[hidden], vp.rect).unwrap();
        assert_ne!(pick_component(&s, &vp, &cv, &id, CompMode::Vertex, hq, &mut MeshCache::default()), Some(Comp::V(hidden as u32)));
        // top face under the cursor
        let top = (0..pm.f.len()).find(|&f| pm.face_normal(f).y > 0.9).unwrap();
        let tq = cv.project(pm.face_center(top), vp.rect).unwrap();
        assert_eq!(pick_component(&s, &vp, &cv, &id, CompMode::Face, tq, &mut MeshCache::default()), Some(Comp::F(top)));
        // marquee over everything selects the cube and only visible vertices
        assert_eq!(marquee_objects(&s, &vp, &cv, vp.rect), vec![id.clone()]);
        let vs = marquee_components(&s, &vp, &cv, &id, CompMode::Vertex, vp.rect);
        assert_eq!(vs.len(), 7);
    }

    #[test]
    fn navigation_keeps_center_of_interest() {
        let (mut s, _, cv, _) = setup();
        let center = cv.eye + cv.fwd * cv.coi;
        tumble(&mut s, "persp", 40.0, 10.0, false);
        let cv2 = cam_view(&s, "persp", [800.0, 600.0]);
        let center2 = cv2.eye + cv2.fwd * cv2.coi;
        assert!((center - center2).length() < 1e-6);
        dolly(&mut s, "persp", 0.5);
        let cv3 = cam_view(&s, "persp", [800.0, 600.0]);
        assert!(cv3.coi < cv2.coi);
        assert!(((cv3.eye + cv3.fwd * cv3.coi) - center).length() < 1e-6);
    }

    #[test]
    fn lasso_and_brush_regions() {
        let tri = [pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(0.0, 10.0)];
        assert!(in_polygon(&tri, pos2(2.0, 2.0)));
        assert!(!in_polygon(&tri, pos2(8.0, 8.0)));
        let stroke = [pos2(0.0, 0.0), pos2(100.0, 0.0)];
        assert!(on_stroke(&stroke, 5.0, pos2(50.0, 4.0)));
        assert!(!on_stroke(&stroke, 5.0, pos2(50.0, 6.0)));
        let (s, vp, cv, id) = setup();
        let r = vp.rect;
        let poly = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        assert_eq!(region_objects(&s, &vp, &cv, &|q| in_polygon(&poly, q)), vec![id]);
    }
}
